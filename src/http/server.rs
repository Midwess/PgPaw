use actix_cors::Cors;
use actix_web::middleware::Logger;
use actix_web::{dev::Server, web, App, HttpServer};

use crate::di::Di;
use crate::error::CacheError;

pub fn bind() -> Result<Server, CacheError> {
    let bind = Di::instance().bind_addr().to_string();
    log::info!("event=http_server_bind_start bind_addr={}", bind);
    let server = HttpServer::new(|| {
        App::new()
            .wrap(Logger::new(
                "event=http_request remote_addr=%a request=\"%r\" status=%s response_bytes=%b duration_ms=%D user_agent=\"%{User-Agent}i\"",
            ))
            .wrap(cors())
            .route("/healthz", web::get().to(super::health::healthz))
            .route("/query", web::post().to(super::query::query))
            .route("/q/{hash}/{version}", web::get().to(super::query::cursor))
    })
    .disable_signals()
    .bind(bind.clone())
    .map_err(|error| {
        log::error!(
            "event=http_server_bind_failed bind_addr={} error={:?}",
            bind,
            error.to_string(),
        );
        CacheError::Io(error)
    })?;
    log::info!(
        "event=http_server_listening bind_addr={} url=http://{}",
        bind,
        bind
    );
    Ok(server.run())
}

fn cors() -> Cors {
    match Di::instance().cors_origin() {
        None => Cors::default(),
        Some("*") => Cors::default()
            .allow_any_origin()
            .allow_any_method()
            .allow_any_header(),
        Some(list) => {
            let mut cors = Cors::default().allow_any_method().allow_any_header();
            for origin in list.split(',') {
                cors = cors.allowed_origin(origin.trim());
            }
            cors
        }
    }
}
