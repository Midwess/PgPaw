use az_wire::{handler, ErrorCode, HandlerError, NodeBuilder, Reply, Request, State, Streaming};
use futures_util::StreamExt;
use serde_json::Value;

use crate::error::CacheError;
use crate::capability::read::ReadOperations;
use crate::protocol::payload::{
    CursorRequest, CursorResponse, LiveEvent, LiveRequest, ReadRequest, ReadResponse,
};
use crate::protocol::subjects::{CURSOR_SUBJECT, LIVE_SUBJECT, READ_SUBJECT};

pub(crate) fn register_az_wire(builder: NodeBuilder, operations: ReadOperations) -> NodeBuilder {
    use az_wire::Handler;

    builder
        .state(operations)
        .service(read.at_subject(READ_SUBJECT))
        .service(cursor.at_subject(CURSOR_SUBJECT))
        .service(live.at_subject(LIVE_SUBJECT))
}

#[handler]
async fn read(
    operations: State<ReadOperations>,
    request: Request<ReadRequest>,
) -> Result<Reply<ReadResponse>, HandlerError> {
    let request = request.into_payload();
    let principal = operations
        .authenticate(request.bearer.as_deref())
        .map_err(handler_error)?;
    let prepared = operations
        .prepare(&request.sql, principal)
        .await
        .map_err(handler_error)?;
    if prepared.private {
        let rows = operations
            .execute_private(&prepared)
            .await
            .and_then(parse_rows)
            .map_err(handler_error)?;
        let (_, version, _) = operations.materialize_version(&prepared);
        Ok(Reply::new(ReadResponse::Private { rows, version }))
    } else {
        let (hash, version, _) = operations
            .materialize(&prepared)
            .await
            .map_err(handler_error)?;
        Ok(Reply::new(ReadResponse::Public { hash, version }))
    }
}

#[handler]
async fn cursor(
    operations: State<ReadOperations>,
    request: Request<CursorRequest>,
) -> Result<Reply<CursorResponse>, HandlerError> {
    let request = request.into_payload();
    let result = operations
        .cursor(&request.hash, &request.version)
        .await
        .ok_or_else(|| HandlerError::new(ErrorCode::InvalidInput, "unknown cursor"))?;
    let rows = parse_rows(result.body.clone()).map_err(handler_error)?;
    Ok(Reply::new(CursorResponse {
        etag: result.etag.clone(),
        rows,
    }))
}

#[handler]
async fn live(
    operations: State<ReadOperations>,
    request: Request<LiveRequest>,
) -> Result<Streaming<LiveEvent, HandlerError>, HandlerError> {
    let request = request.into_payload();
    let principal = operations
        .authenticate(request.bearer.as_deref())
        .map_err(handler_error)?;
    let prepared = operations
        .prepare(&request.sql, principal)
        .await
        .map_err(handler_error)?;
    let subscription = operations
        .subscribe(prepared)
        .await
        .map_err(handler_error)?;
    Ok(Streaming::new(
        subscription.map(|event| decode_event(&event)),
    ))
}

fn parse_rows(body: String) -> Result<Value, CacheError> {
    serde_json::from_str(&body).map_err(|error| CacheError::Cache(error.to_string()))
}

fn decode_event(event: &str) -> Result<LiveEvent, HandlerError> {
    let value: Value =
        serde_json::from_str(event.trim().strip_prefix("data: ").unwrap_or(event.trim()))
            .map_err(|error| HandlerError::new(ErrorCode::Internal, error.to_string()))?;
    if value.get("type").and_then(Value::as_str) == Some("snapshot") {
        let version = value
            .get("version")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        return Ok(LiveEvent::Snapshot {
            rows: value.get("rows").cloned(),
            hash: value
                .get("url")
                .and_then(Value::as_str)
                .and_then(|url| url.split('/').nth(2))
                .map(str::to_string),
            version,
        });
    }
    match value.get("op").and_then(Value::as_str) {
        Some("insert") => Ok(LiveEvent::Insert {
            key: string(&value, "key")?,
            row: value["row"].clone(),
            txid: number(&value, "txid")?,
        }),
        Some("update") => Ok(LiveEvent::Update {
            key: string(&value, "key")?,
            row: value["row"].clone(),
            txid: number(&value, "txid")?,
        }),
        Some("delete") => Ok(LiveEvent::Delete {
            key: string(&value, "key")?,
            row: value["row"].clone(),
            txid: number(&value, "txid")?,
        }),
        Some("up-to-date") => Ok(LiveEvent::UpToDate {
            txid: number(&value, "txid")?,
        }),
        Some("reset") => Ok(LiveEvent::Reset),
        _ => Err(HandlerError::new(ErrorCode::Internal, "invalid live event")),
    }
}

fn string(value: &Value, field: &str) -> Result<String, HandlerError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            HandlerError::new(ErrorCode::Internal, format!("live event missing {field}"))
        })
}

fn number(value: &Value, field: &str) -> Result<u32, HandlerError> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .and_then(|value| value.try_into().ok())
        .ok_or_else(|| {
            HandlerError::new(ErrorCode::Internal, format!("live event missing {field}"))
        })
}

fn handler_error(error: CacheError) -> HandlerError {
    let code = match error {
        CacheError::Rejected(_) | CacheError::Parse(_) => ErrorCode::InvalidInput,
        CacheError::Unauthorized(_) | CacheError::Forbidden(_) => ErrorCode::Unauthorized,
        CacheError::Halted(_) => ErrorCode::Busy,
        _ => ErrorCode::Internal,
    };
    HandlerError::new(code, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{decode_event, handler_error};
    use crate::error::CacheError;
    use crate::protocol::payload::LiveEvent;
    use az_wire::{ErrorCode, HostConfig, NodeBuilder, TopologyConfig};
    use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};

    #[test]
    fn decodes_sse_delta_as_typed_event() {
        let event = decode_event(
            "data: {\"op\":\"insert\",\"key\":\"1\",\"row\":{\"id\":1},\"txid\":7}\n\n",
        )
        .unwrap();
        assert!(matches!(event, LiveEvent::Insert { txid: 7, .. }));
    }

    #[test]
    fn shared_errors_keep_wire_semantics() {
        assert_eq!(
            handler_error(CacheError::Rejected("write".into())).code,
            ErrorCode::InvalidInput
        );
        assert_eq!(
            handler_error(CacheError::Unauthorized("token".into())).code,
            ErrorCode::Unauthorized
        );
        assert_eq!(
            handler_error(CacheError::Forbidden("rls".into())).code,
            ErrorCode::Unauthorized
        );
        assert_eq!(
            handler_error(CacheError::Halted("replica".into())).code,
            ErrorCode::Busy
        );
    }

    #[test]
    fn mutation_rejection_is_typed_for_wire() {
        let error = handler_error(CacheError::Rejected(
            "only read-only SELECT queries are cacheable; writes and DDL are not supported".into(),
        ));
        assert_eq!(error.code, ErrorCode::InvalidInput);
    }

    #[tokio::test]
    async fn native_topology_binds_and_releases_its_listener() {
        let address = free_address();
        let node = NodeBuilder::new("pgpaw-test")
            .insecure_accept_declared_peer_identities()
            .build()
            .unwrap();
        let topology = node
            .start_topology(TopologyConfig::host(websocket_host(address)))
            .await
            .unwrap();
        assert_eq!(topology.websocket_addr(), Some(address));
        assert!(TcpListener::bind(address).is_err());
        topology.shutdown().await.unwrap();
        TcpListener::bind(address).unwrap();
    }

    #[tokio::test]
    async fn native_topology_reports_port_collision() {
        let address = free_address();
        let collision = TcpListener::bind(address).unwrap();
        let node = NodeBuilder::new("pgpaw-test")
            .insecure_accept_declared_peer_identities()
            .build()
            .unwrap();
        assert!(node
            .start_topology(TopologyConfig::host(websocket_host(address)))
            .await
            .is_err());
        drop(collision);
    }

    fn websocket_host(address: SocketAddr) -> HostConfig {
        HostConfig::tcp(address, ::az_wire::TcpTransport::plain())
    }

    fn free_address() -> SocketAddr {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
    }
}
