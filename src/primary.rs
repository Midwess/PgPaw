use std::path::PathBuf;

use pglite::{MultiProcessOptions, PGlite};

use crate::error::CacheError;

pub struct PrimaryConfig {
    pub data_dir: PathBuf,
    pub listen_addresses: String,
    pub port: u16,
    pub max_connections: usize,
}

pub async fn open_primary(config: &PrimaryConfig) -> Result<PGlite, CacheError> {
    let options = MultiProcessOptions {
        listen_addresses: Some(config.listen_addresses.clone()),
        port: Some(config.port),
        max_connections: config.max_connections,
        ..Default::default()
    };
    Ok(PGlite::open_multi_process(&config.data_dir, options).await?)
}

pub async fn run_primary(config: PrimaryConfig) -> Result<(), CacheError> {
    let _db = open_primary(&config).await?;
    eprintln!(
        "pgpaw primary ready on {}:{} (data_dir={})",
        config.listen_addresses,
        config.port,
        config.data_dir.display()
    );
    std::future::pending::<()>().await;
    Ok(())
}
