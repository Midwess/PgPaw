use actix_web::HttpResponse;
use serde_json::json;

use crate::di::Di;

pub async fn healthz() -> HttpResponse {
    let replica = Di::instance().replica();
    if replica.is_halted() {
        log::warn!(
            "event=health_check status=halted reason={:?}",
            replica.halt_reason(),
        );
        HttpResponse::ServiceUnavailable().json(json!({
            "status": "halted",
            "reason": replica.halt_reason(),
        }))
    } else {
        log::info!(
            "event=health_check status=ok watermark={}",
            replica.watermark().0,
        );
        HttpResponse::Ok().json(json!({
            "status": "ok",
            "watermark": replica.watermark().0,
        }))
    }
}
