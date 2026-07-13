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
#[cfg(feature = "server")]
mod operations;
#[cfg(feature = "az-wire")]
mod az_wire;
mod primary;
#[cfg(feature = "server")]
mod rows;
#[cfg(feature = "server")]
mod setup;
mod shadow;
#[cfg(feature = "server")]
mod version;
#[cfg(feature = "server")]
pub mod wire;

#[cfg(all(test, feature = "server"))]
mod tests;

#[cfg(feature = "server")]
pub use di::{Di, ServerConfig, UpstreamConfig};
#[cfg(feature = "server")]
pub use operations::{PreparedRead, ReadOperations};
#[cfg(feature = "az-wire")]
pub use az_wire::register_az_wire;
pub use error::CacheError;
pub use primary::{open_primary, run_primary, PrimaryConfig, PrimaryHandle};
pub use shadow::{open_shadow, ShadowHandle};

#[cfg(feature = "server")]
pub async fn run(config: ServerConfig) -> Result<(), CacheError> {
    #[cfg(feature = "az-wire")]
    let az_wire = config
        .az_wire_addr
        .map(|address| (address, config.az_wire_node.clone()));
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
    let server = match http::server::bind() {
        Ok(server) => server,
        Err(error) => {
            Di::instance().shutdown().await;
            return Err(error);
        }
    };
    #[cfg(feature = "az-wire")]
    let mut topology = match az_wire {
        Some((address, node)) => {
            let builder = ::az_wire::NodeBuilder::new(&node)
                .insecure_accept_declared_peer_identities();
            let started = match register_az_wire(builder, Di::instance().operations().clone()).build()
            {
                Ok(node) => node
                    .start_topology(::az_wire::TopologyConfig::host(::az_wire::HostConfig::new(address)))
                    .await
                    .map_err(|error| CacheError::Config(error.to_string())),
                Err(error) => Err(CacheError::Config(error.to_string())),
            };
            match started {
                Ok(topology) => Some(topology),
                Err(error) => {
                    drop(server);
                    Di::instance().shutdown().await;
                    return Err(error);
                }
            }
        }
        None => None,
    };
    log::info!(
        "event=server_ready bind_addr={} health_path=/healthz query_path=/query",
        Di::instance().bind_addr()
    );
    let handle = server.handle();
    let mut server = Box::pin(server);
    #[cfg(unix)]
    let signal = async {
        let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .map_err(CacheError::Io)?;
        tokio::select! {
            biased;
            result = tokio::signal::ctrl_c() => result.map_err(CacheError::Io),
            _ = terminate.recv() => Ok(()),
        }
    };
    #[cfg(not(unix))]
    let signal = async { tokio::signal::ctrl_c().await.map_err(CacheError::Io) };
    tokio::pin!(signal);
    #[cfg(feature = "az-wire")]
    let (result, http_complete) = match &mut topology {
        Some(topology) => tokio::select! {
            biased;
            result = &mut signal => (result, false),
            result = &mut server => (result.map_err(CacheError::Io), true),
            result = topology.wait() => (result.map_err(|error| CacheError::Config(error.to_string())), false),
        },
        None => tokio::select! {
            biased;
            result = &mut signal => (result, false),
            result = &mut server => (result.map_err(CacheError::Io), true),
        },
    };
    #[cfg(not(feature = "az-wire"))]
    let (result, http_complete) = tokio::select! {
        biased;
        result = &mut signal => (result, false),
        result = &mut server => (result.map_err(CacheError::Io), true),
    };
    match &result {
        Ok(()) => log::info!("event=server_stopped result=ok"),
        Err(error) => log::error!(
            "event=server_stopped result=error error={:?}",
            error.to_string()
        ),
    }
    log::info!("event=server_shutdown_start");
    handle.stop(true).await;
    if !http_complete {
        server.await.map_err(CacheError::Io)?;
    }
    #[cfg(feature = "az-wire")]
    if let Some(topology) = topology {
        topology
            .shutdown()
            .await
            .map_err(|error| CacheError::Config(error.to_string()))?;
    }
    Di::instance().shutdown().await;
    log::info!("event=server_shutdown_complete");
    result
}

#[cfg(feature = "server")]
pub async fn init(upstream: UpstreamConfig) -> Result<(), CacheError> {
    setup::prepare(&upstream).await
}
