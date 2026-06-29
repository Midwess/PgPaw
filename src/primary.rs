use std::path::PathBuf;

use pglite::{MultiProcessOptions, PGlite};

use crate::error::CacheError;

pub struct PrimaryConfig {
    pub data_dir: PathBuf,
    pub listen_addresses: String,
    pub port: u16,
    pub max_connections: usize,
}

impl PrimaryConfig {
    pub fn embedded(data_dir: impl Into<PathBuf>) -> PrimaryConfig {
        PrimaryConfig {
            data_dir: data_dir.into(),
            listen_addresses: String::new(),
            port: 0,
            max_connections: 8,
        }
    }
}

pub struct PrimaryHandle {
    db: PGlite,
    dsn: String,
}

impl PrimaryHandle {
    pub fn dsn(&self) -> &str {
        &self.dsn
    }

    pub async fn shutdown(self) -> Result<(), CacheError> {
        self.db.close().await?;
        Ok(())
    }
}

pub async fn open_primary(config: &PrimaryConfig) -> Result<PrimaryHandle, CacheError> {
    let listen_addresses = if config.listen_addresses.is_empty() {
        None
    } else {
        Some(config.listen_addresses.clone())
    };
    let port = if config.port == 0 { None } else { Some(config.port) };
    log::info!(
        "event=primary_open_start listen_addr={} port={} data_dir={:?} max_connections={}",
        config.listen_addresses,
        config.port,
        config.data_dir,
        config.max_connections,
    );
    let options = MultiProcessOptions {
        listen_addresses,
        port,
        max_connections: config.max_connections,
        ..Default::default()
    };
    let db = PGlite::open_multi_process(&config.data_dir, options).await?;
    let dsn = db
        .connection_uri()
        .ok_or_else(|| CacheError::Config("primary engine exposes no connection_uri".into()))?;
    log::info!("event=primary_open_complete data_dir={:?}", config.data_dir);
    Ok(PrimaryHandle { db, dsn })
}

pub async fn run_primary(config: PrimaryConfig) -> Result<(), CacheError> {
    let handle = open_primary(&config).await?;
    log::info!(
        "event=primary_ready listen_addr={} port={} data_dir={:?} dsn={}",
        config.listen_addresses,
        config.port,
        config.data_dir,
        handle.dsn(),
    );
    std::future::pending::<()>().await;
    Ok(())
}
