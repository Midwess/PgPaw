use actix_web::http::StatusCode;
use actix_web::{web, HttpResponse};
use serde::Deserialize;
use tokio_stream::StreamExt;

use crate::capability::auth::{AuthOutcome, Principal};
use crate::capability::classify::CacheableQuery;
use crate::capability::read::ReadOperations;
use crate::error::CacheError;

#[derive(Deserialize)]
pub struct QueryParams {
    live: Option<bool>,
}

#[derive(Deserialize)]
pub struct QueryBody {
    sql: String,
}

pub async fn query(
    params: web::Query<QueryParams>,
    body: web::Json<QueryBody>,
    auth: AuthOutcome,
    operations: web::Data<ReadOperations>,
) -> HttpResponse {
    let principal = match auth.0 {
        Ok(principal) => principal,
        Err(error) => return error_response(error),
    };

    let read = match operations
        .prepare(body.sql.as_str(), principal.clone(), None, &[])
        .await
    {
        Ok(read) => read,
        Err(error) => return error_response(error),
    };
    let query = &read.query;
    let fingerprint = format!("{:x}", query.fingerprint);
    let tables = tables_csv(&query.tables);

    let private = read.private;
    let live = params.live.unwrap_or(false);
    log::info!(
        "event=query_classified fingerprint={} tables={} live={} scope={}",
        fingerprint,
        tables,
        live,
        if private { "private" } else { "public" },
    );

    if private {
        if live {
            return live_query(&operations, read.query, read.principal).await;
        }
        return private_response(&operations, &read, &fingerprint, &tables).await;
    }

    if live {
        return live_query(&operations, read.query, None).await;
    }
    match operations.materialize(&read).await {
        Ok((hash, version, snapshot)) => {
            log::info!(
                "event=query_snapshot scope=public fingerprint={} tables={} version={} cursor=/q/{}/{} response=redirect snapshot_bytes={}",
                fingerprint,
                tables,
                version,
                hash,
                version,
                snapshot.body.len(),
            );
            HttpResponse::SeeOther()
                .insert_header(("Location", format!("/q/{hash}/{version}")))
                .insert_header(("Cache-Control", "no-store"))
                .finish()
        }
        Err(error) => error_response(error),
    }
}

async fn private_response(
    operations: &ReadOperations,
    read: &crate::capability::read::PreparedRead,
    fingerprint: &str,
    tables: &str,
) -> HttpResponse {
    match operations.execute_private(read).await {
        Ok(body) => {
            let (_, version, _) = operations.materialize_version(read);
            log::info!(
                "event=query_snapshot scope=private fingerprint={} tables={} version={} role={} response=inline snapshot_bytes={}",
                fingerprint,
                tables,
                version,
                read.principal.as_ref().unwrap().role,
                body.len(),
            );
            HttpResponse::Ok()
                .insert_header(("Cache-Control", "private, no-store"))
                .content_type("application/json")
                .body(body)
        }
        Err(error) => error_response(crate::capability::read::map_db_denial(error)),
    }
}

async fn live_query(
    operations: &ReadOperations,
    query: CacheableQuery,
    principal: Option<Principal>,
) -> HttpResponse {
    let read = crate::capability::read::PreparedRead {
        query,
        principal,
        headers: None,
        private: false,
    };
    let subscription = match operations.subscribe(read).await {
        Ok(subscription) => subscription,
        Err(error) => return error_response(error),
    };
    let stream = subscription.map(|event| Ok::<_, actix_web::Error>(web::Bytes::from(event)));
    HttpResponse::Ok()
        .insert_header(("Cache-Control", "no-store"))
        .content_type("text/event-stream")
        .streaming(stream)
}

pub async fn cursor(
    path: web::Path<(String, String)>,
    operations: web::Data<ReadOperations>,
) -> HttpResponse {
    let (hash, version) = path.into_inner();
    match operations.cursor(&hash, &version).await {
        Some(result) => {
            log::info!(
                "event=cursor_hit hash={} version={} bytes={}",
                hash,
                version,
                result.body.len(),
            );
            HttpResponse::Ok()
                .insert_header(("ETag", result.etag.clone()))
                .insert_header(("Cache-Control", "public, max-age=259200"))
                .content_type("application/json")
                .body(result.body.clone())
        }
        None => {
            log::warn!("event=cursor_miss hash={} version={}", hash, version);
            HttpResponse::NotFound()
                .content_type("application/json")
                .body("{\"name\":\"NotFound\",\"message\":\"unknown cursor\"}")
        }
    }
}

pub(crate) fn error_response(error: CacheError) -> HttpResponse {
    let code = error_status(&error);
    match code {
        StatusCode::INTERNAL_SERVER_ERROR | StatusCode::SERVICE_UNAVAILABLE => log::error!(
            "event=http_error status={} error_name={} error={:?}",
            code.as_u16(),
            error.name(),
            error.to_string(),
        ),
        _ => log::warn!(
            "event=http_error status={} error_name={} error={:?}",
            code.as_u16(),
            error.name(),
            error.to_string(),
        ),
    }
    HttpResponse::build(code)
        .content_type("application/json")
        .body(error.envelope())
}

pub(crate) fn error_status(error: &CacheError) -> StatusCode {
    match error {
        CacheError::Rejected(_) | CacheError::Parse(_) => StatusCode::BAD_REQUEST,
        CacheError::Halted(_) => StatusCode::SERVICE_UNAVAILABLE,
        CacheError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
        CacheError::Forbidden(_) => StatusCode::FORBIDDEN,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn tables_csv(tables: &[String]) -> String {
    if tables.is_empty() {
        "-".to_string()
    } else {
        tables.join(",")
    }
}

#[cfg(test)]
mod tests {
    use super::error_status;
    use crate::capability::read::map_db_denial;
    use crate::error::CacheError;
    use actix_web::http::StatusCode;

    fn db_error(sqlstate: &str) -> CacheError {
        CacheError::Pglite(pglite::Error::Database {
            sqlstate: sqlstate.to_string(),
            message: "denied".to_string(),
            detail: None,
            hint: None,
        })
    }

    #[test]
    fn denial_sqlstates_map_to_forbidden() {
        for code in ["42501", "42704", "28000"] {
            assert!(
                matches!(map_db_denial(db_error(code)), CacheError::Forbidden(_)),
                "sqlstate {code} should map to Forbidden"
            );
        }
    }

    #[test]
    fn unrelated_database_error_passes_through() {
        assert!(matches!(
            map_db_denial(db_error("23505")),
            CacheError::Pglite(_)
        ));
    }

    #[test]
    fn non_database_error_is_untouched() {
        assert!(matches!(
            map_db_denial(CacheError::Cache("x".to_string())),
            CacheError::Cache(_)
        ));
    }

    #[test]
    fn shared_errors_keep_http_semantics() {
        assert_eq!(
            error_status(&CacheError::Rejected("write".into())),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            error_status(&CacheError::Unauthorized("token".into())),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            error_status(&CacheError::Forbidden("rls".into())),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            error_status(&CacheError::Halted("replica".into())),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}
