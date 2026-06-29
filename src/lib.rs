#[cfg(feature = "server")]
mod auth;
#[cfg(feature = "server")]
mod cache;
#[cfg(feature = "server")]
mod cdc;
#[cfg(feature = "server")]
mod classify;
#[cfg(feature = "server")]
mod di;
#[cfg(feature = "server")]
mod diff;
mod error;
#[cfg(feature = "server")]
mod http;
#[cfg(feature = "server")]
mod live;
mod primary;
#[cfg(feature = "server")]
mod rows;
#[cfg(feature = "server")]
mod setup;
mod shadow;
#[cfg(feature = "server")]
mod version;

#[cfg(all(test, feature = "server"))]
mod tests;

#[cfg(feature = "server")]
pub use di::{Di, ServerConfig, UpstreamConfig};
pub use error::CacheError;
pub use primary::{open_primary, run_primary, PrimaryConfig, PrimaryHandle};
pub use shadow::{open_shadow, ShadowHandle};

#[cfg(feature = "server")]
pub async fn run(config: ServerConfig) -> Result<(), CacheError> {
    log::info!(
        "event=server_starting bind_addr={} data_dir={:?} max_connections={} cache_size_bytes={} upstream_host={} upstream_port={} upstream_user={} upstream_database={} publication={} slot={} sslmode={} auth_configured={} cors_origin={:?}",
        config.bind_addr,
        config.data_dir,
        config.max_connections,
        config.cache_size_bytes,
        config.upstream.host,
        config.upstream.port,
        config.upstream.user,
        config.upstream.database,
        config.upstream.publication,
        config.upstream.slot,
        config.upstream.sslmode,
        config.jwt_secret.is_some() || config.jwt_public_key.is_some() || config.jwt_jwks_url.is_some(),
        config.cors_origin,
    );
    Di::init(config).await?;
    log::info!(
        "event=server_ready bind_addr={} health_path=/healthz query_path=/query",
        Di::instance().bind_addr()
    );
    let result = http::server::serve().await;
    match &result {
        Ok(()) => log::info!("event=server_stopped result=ok"),
        Err(error) => log::error!(
            "event=server_stopped result=error error={:?}",
            error.to_string()
        ),
    }
    log::info!("event=server_shutdown_start");
    Di::instance().shutdown().await;
    log::info!("event=server_shutdown_complete");
    result
}

#[cfg(feature = "server")]
pub async fn init(upstream: UpstreamConfig) -> Result<(), CacheError> {
    setup::prepare(&upstream).await
}
