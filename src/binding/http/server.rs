use actix_cors::Cors;
use actix_web::middleware::Logger;
use actix_web::{dev::Server, web, App, HttpServer};

use crate::capability::read::ReadOperations;
use crate::error::CacheError;

pub(crate) fn bind_at(
    addr: std::net::SocketAddr,
    cors_origin: Option<String>,
    operations: web::Data<ReadOperations>,
) -> Result<Server, CacheError> {
    log::info!("event=http_server_bind_start bind_addr={}", addr);
    let server = HttpServer::new(move || {
        App::new()
            .app_data(operations.clone())
            .wrap(Logger::new(
                "event=http_request remote_addr=%a request=\"%r\" status=%s response_bytes=%b duration_ms=%D user_agent=\"%{User-Agent}i\"",
            ))
            .wrap(cors(cors_origin.as_deref()))
            .route("/healthz", web::get().to(super::health::healthz))
            .route("/query", web::post().to(super::query::query))
            .route("/q/{hash}/{version}", web::get().to(super::query::cursor))
    })
    .disable_signals()
    .bind(addr)
    .map_err(|error| {
        log::error!(
            "event=http_server_bind_failed bind_addr={} error={:?}",
            addr,
            error.to_string(),
        );
        CacheError::Io(error)
    })?;
    log::info!(
        "event=http_server_listening bind_addr={} url=http://{}",
        addr,
        addr
    );
    Ok(server.run())
}

fn cors(origin: Option<&str>) -> Cors {
    match origin {
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
