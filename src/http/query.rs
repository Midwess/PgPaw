use std::sync::Arc;

use actix_web::http::StatusCode;
use actix_web::{web, HttpResponse};
use serde::Deserialize;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_stream::StreamExt;

use crate::auth::{AuthOutcome, Principal};
use crate::cache::CachedResult;
use crate::classify::CacheableQuery;
use crate::di::Di;
use crate::error::CacheError;
use crate::rows;

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
) -> HttpResponse {
    let di = Di::instance();
    if di.replica().is_halted() {
        return error_response(CacheError::Halted(
            di.replica()
                .halt_reason()
                .unwrap_or_else(|| "unknown".to_string()),
        ));
    }

    let principal = match auth.0 {
        Ok(principal) => principal,
        Err(error) => return error_response(error),
    };

    let query = match di.classifier().classify(body.sql.as_str()) {
        Ok(query) => query,
        Err(error) => return error_response(error),
    };
    let fingerprint = format!("{:x}", query.fingerprint);
    let tables = tables_csv(&query.tables);

    if query.tables.len() > 1 {
        if let Err(error) = rows::ensure_unique_columns(di.db(), &query.sql).await {
            return error_response(error);
        }
    }

    let private = match di.is_private(&query.tables).await {
        Ok(private) => private,
        Err(error) => return error_response(error),
    };
    let live = params.live.unwrap_or(false);
    log::info!(
        "event=query_classified fingerprint={} tables={} live={} scope={}",
        fingerprint,
        tables,
        live,
        if private { "private" } else { "public" },
    );

    if private {
        let Some(principal) = principal else {
            return error_response(CacheError::Unauthorized(
                "this query is access-controlled; a bearer token is required".to_string(),
            ));
        };
        if live {
            return live_query(di, query, Some(principal)).await;
        }
        return private_response(di, &query, &principal, &fingerprint, &tables).await;
    }

    if live {
        return live_query(di, query, None).await;
    }
    match materialize(di, &query).await {
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
    di: &Di,
    query: &CacheableQuery,
    principal: &Principal,
    fingerprint: &str,
    tables: &str,
) -> HttpResponse {
    match rows::query_json_as(di.db(), &principal.role, &principal.claims_json, &query.sql).await {
        Ok(body) => {
            let version = di.versions().version_of(&query.tables, &query.eq_filters).0;
            log::info!(
                "event=query_snapshot scope=private fingerprint={} tables={} version={} role={} response=inline snapshot_bytes={}",
                fingerprint,
                tables,
                version,
                principal.role,
                body.len(),
            );
            HttpResponse::Ok()
                .insert_header(("Cache-Control", "private, no-store"))
                .content_type("application/json")
                .body(body)
        }
        Err(error) => error_response(map_db_denial(error)),
    }
}

fn map_db_denial(error: CacheError) -> CacheError {
    if let CacheError::Pglite(pglite::Error::Database { sqlstate, .. }) = &error {
        if matches!(sqlstate.as_str(), "42501" | "42704" | "28000") {
            return CacheError::Forbidden(error.to_string());
        }
    }
    error
}

async fn live_query(
    di: &'static Di,
    query: CacheableQuery,
    principal: Option<Principal>,
) -> HttpResponse {
    let fingerprint = format!("{:x}", query.fingerprint);
    let tables = tables_csv(&query.tables);
    let receiver = match principal {
        Some(p) => {
            let body = match rows::query_json_as(di.db(), &p.role, &p.claims_json, &query.sql).await
            {
                Ok(body) => body,
                Err(error) => return error_response(map_db_denial(error)),
            };
            let version = di.versions().version_of(&query.tables, &query.eq_filters).0;
            log::info!(
                "event=live_subscribe scope=private fingerprint={} tables={} version={} role={} snapshot_bytes={}",
                fingerprint,
                tables,
                version,
                p.role,
                body.len(),
            );
            di.live().subscribe(
                query.sql,
                query.tables,
                String::new(),
                version,
                &body,
                Some(p),
            )
        }
        None => {
            let (hash, version, snapshot) = match materialize(di, &query).await {
                Ok(parts) => parts,
                Err(error) => return error_response(error),
            };
            log::info!(
                "event=live_subscribe scope=public fingerprint={} tables={} version={} cursor=/q/{}/{} snapshot_bytes={}",
                fingerprint,
                tables,
                version,
                hash,
                version,
                snapshot.body.len(),
            );
            di.live()
                .subscribe(query.sql, query.tables, hash, version, &snapshot.body, None)
        }
    };
    let stream = UnboundedReceiverStream::new(receiver)
        .map(|event| Ok::<_, actix_web::Error>(web::Bytes::from(event)));
    HttpResponse::Ok()
        .insert_header(("Cache-Control", "no-store"))
        .content_type("text/event-stream")
        .streaming(stream)
}

pub async fn cursor(path: web::Path<(String, String)>) -> HttpResponse {
    let (hash, version) = path.into_inner();
    let key = format!("{hash}:{version}");
    match Di::instance().cache().get(&key).await {
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

async fn materialize(
    di: &'static Di,
    query: &CacheableQuery,
) -> Result<(String, u64, Arc<CachedResult>), CacheError> {
    let hash = format!("{:x}", query.fingerprint);
    let version = di.versions().version_of(&query.tables, &query.eq_filters).0;
    let key = format!("{hash}:{version}");
    let snapshot_sql = query.sql.clone();
    let snapshot = di
        .cache()
        .get_or_compute(key, async move {
            rows::query_json(di.db(), &snapshot_sql).await
        })
        .await?;
    Ok((hash, version, snapshot))
}

pub(crate) fn error_response(error: CacheError) -> HttpResponse {
    let code = match &error {
        CacheError::Rejected(_) | CacheError::Parse(_) => StatusCode::BAD_REQUEST,
        CacheError::Halted(_) => StatusCode::SERVICE_UNAVAILABLE,
        CacheError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
        CacheError::Forbidden(_) => StatusCode::FORBIDDEN,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
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

fn tables_csv(tables: &[String]) -> String {
    if tables.is_empty() {
        "-".to_string()
    } else {
        tables.join(",")
    }
}

#[cfg(test)]
mod tests {
    use super::map_db_denial;
    use crate::error::CacheError;

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
}
