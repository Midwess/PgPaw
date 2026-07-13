use std::path::PathBuf;

use pglite::{MultiProcessOptions, PGlite};

use crate::error::CacheError;

pub struct PrimaryConfig {
    pub data_dir: PathBuf,
    pub database: String,
    pub listen_addresses: String,
    pub port: u16,
    pub min_connections: usize,
    pub max_connections: usize,
}

impl PrimaryConfig {
    pub fn embedded(data_dir: impl Into<PathBuf>) -> PrimaryConfig {
        PrimaryConfig {
            data_dir: data_dir.into(),
            database: "postgres".into(),
            listen_addresses: String::new(),
            port: 0,
            min_connections: 0,
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
    if config.max_connections == 0 || config.min_connections > config.max_connections {
        return Err(CacheError::Config(
            "primary connections require max_connections greater than zero and min_connections no greater than max_connections".into(),
        ));
    }
    if config.database.is_empty()
        || config.database.len() > 63
        || !config
            .database
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(CacheError::Config(
            "primary database must use letters, digits, or underscores and contain at most 63 bytes"
                .into(),
        ));
    }
    let listen_addresses = if config.listen_addresses.is_empty() {
        None
    } else {
        Some(config.listen_addresses.clone())
    };
    let port = if config.port == 0 {
        None
    } else {
        Some(config.port)
    };
    log::info!(
        "event=primary_open_start listen_addr={} port={} data_dir={:?} max_connections={}",
        config.listen_addresses,
        config.port,
        config.data_dir,
        config.max_connections,
    );
    let options = MultiProcessOptions {
        database: "postgres".into(),
        min_connections: config.min_connections,
        listen_addresses,
        port,
        max_connections: config.max_connections,
        ..Default::default()
    };
    let db = PGlite::open_multi_process(&config.data_dir, options).await?;
    let prepared = async {
        if config.database != "postgres" {
            let literal = config.database.replace('\'', "''");
            let database = config.database.replace('"', "\"\"");
            if db
                .query(
                    &format!("SELECT 1 FROM pg_database WHERE datname = '{literal}'"),
                    &[],
                )
                .await?
                .is_empty()
            {
                db.exec(&format!("CREATE DATABASE \"{database}\"")).await?;
            }
        }
        let base = db
            .connection_uri()
            .ok_or_else(|| CacheError::Config("primary engine exposes no connection_uri".into()))?;
        let (address, query) = base
            .split_once('?')
            .map(|(address, query)| (address, Some(query)))
            .unwrap_or((base.as_str(), None));
        let (prefix, _) = address.rsplit_once('/').ok_or_else(|| {
            CacheError::Config("primary engine exposes an invalid connection_uri".into())
        })?;
        let dsn = match query {
            Some(query) => format!("{prefix}/{}?{query}", config.database),
            None => format!("{prefix}/{}", config.database),
        };
        Ok::<String, CacheError>(dsn)
    }
    .await;
    let dsn = match prepared {
        Ok(dsn) => dsn,
        Err(error) => {
            let _ = db.close().await;
            return Err(error);
        }
    };
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
