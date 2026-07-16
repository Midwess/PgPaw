use actix_web::{web, HttpResponse};
use serde_json::json;

use crate::capability::read::ReadOperations;

pub async fn healthz(operations: web::Data<ReadOperations>) -> HttpResponse {
    let health = operations.health();
    if health.halted {
        log::warn!("event=health_check status=halted reason={:?}", health.reason);
        return HttpResponse::ServiceUnavailable().json(json!({
            "status": "halted",
            "reason": health.reason,
        }));
    }
    match health.watermark {
        Some(watermark) => {
            log::info!("event=health_check status=ok watermark={}", watermark);
            HttpResponse::Ok().json(json!({
                "status": "ok",
                "watermark": watermark,
            }))
        }
        None => {
            log::info!("event=health_check status=ok");
            HttpResponse::Ok().json(json!({
                "status": "ok",
            }))
        }
    }
}
