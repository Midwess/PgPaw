use actix_cors::Cors;
use actix_web::{web, App, HttpServer};

use crate::di::Di;
use crate::error::CacheError;

pub async fn serve() -> Result<(), CacheError> {
    let bind = Di::instance().bind_addr().to_string();
    HttpServer::new(|| {
        App::new()
            .wrap(cors())
            .route("/healthz", web::get().to(super::health::healthz))
            .route("/query", web::post().to(super::query::query))
            .route("/q/{hash}/{version}", web::get().to(super::query::cursor))
    })
    .bind(bind)
    .map_err(CacheError::Io)?
    .run()
    .await
    .map_err(CacheError::Io)?;
    Ok(())
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
