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

    let private = match di.is_private(&query.tables).await {
        Ok(private) => private,
        Err(error) => return error_response(error),
    };
    let live = params.live.unwrap_or(false);

    if private {
        let Some(principal) = principal else {
            return error_response(CacheError::Unauthorized(
                "this query is access-controlled; a bearer token is required".to_string(),
            ));
        };
        if live {
            return error_response(CacheError::Forbidden(
                "live streaming is not available for access-controlled queries".to_string(),
            ));
        }
        return private_response(di, &query, &principal).await;
    }

    if live {
        return live_query(di, query).await;
    }
    match materialize(di, &query).await {
        Ok((hash, version, _)) => HttpResponse::SeeOther()
            .insert_header(("Location", format!("/q/{hash}/{version}")))
            .insert_header(("Cache-Control", "no-store"))
            .finish(),
        Err(error) => error_response(error),
    }
}

async fn private_response(di: &Di, query: &CacheableQuery, principal: &Principal) -> HttpResponse {
    match rows::query_json_as(di.db(), &principal.role, &principal.claims_json, &query.sql).await {
        Ok(body) => HttpResponse::Ok()
            .insert_header(("Cache-Control", "private, no-store"))
            .content_type("application/json")
            .body(body),
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

async fn live_query(di: &'static Di, query: CacheableQuery) -> HttpResponse {
    let (hash, version, snapshot) = match materialize(di, &query).await {
        Ok(parts) => parts,
        Err(error) => return error_response(error),
    };
    let receiver = di
        .live()
        .subscribe(query.sql, query.tables, hash, version, &snapshot.body);
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
        Some(result) => HttpResponse::Ok()
            .insert_header(("ETag", result.etag.clone()))
            .insert_header(("Cache-Control", "public, max-age=259200"))
            .content_type("application/json")
            .body(result.body.clone()),
        None => HttpResponse::NotFound()
            .content_type("application/json")
            .body("{\"name\":\"NotFound\",\"message\":\"unknown cursor\"}"),
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
    HttpResponse::build(code)
        .content_type("application/json")
        .body(error.envelope())
}
