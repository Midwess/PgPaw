use actix_cors::Cors;
use actix_web::middleware::Logger;
use actix_web::{dev::Server, web, App, HttpServer};

use crate::di::Di;
use crate::error::CacheError;

pub fn bind() -> Result<Server, CacheError> {
    let bind = Di::instance().bind_addr().to_string();
    bind_at(&bind, Di::instance().cors_origin().map(str::to_string))
}

pub(crate) fn bind_at(bind: &str, cors_origin: Option<String>) -> Result<Server, CacheError> {
    let bind = bind.to_string();
    log::info!("event=http_server_bind_start bind_addr={}", bind);
    let server = HttpServer::new(move || {
        App::new()
            .wrap(Logger::new(
                "event=http_request remote_addr=%a request=\"%r\" status=%s response_bytes=%b duration_ms=%D user_agent=\"%{User-Agent}i\"",
            ))
            .wrap(cors(cors_origin.as_deref()))
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

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, TcpListener};

    #[actix_web::test]
    async fn stopped_server_releases_port_before_native_start_error_returns() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let server = super::bind_at(&address.to_string(), None).unwrap();
        let handle = server.handle();
        let task = actix_web::rt::spawn(server);
        handle.stop(true).await;
        task.await.unwrap().unwrap();
        TcpListener::bind(address).unwrap();
    }
}
