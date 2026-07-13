use std::path::PathBuf;
#[cfg(feature = "az-wire")]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(feature = "az-wire")]
use std::sync::Arc;

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
    #[cfg(feature = "az-wire")]
    topology: Option<::az_wire::AzWireTopology>,
    #[cfg(feature = "az-wire")]
    observer: Option<PrimaryObserver>,
}

impl PrimaryHandle {
    pub fn dsn(&self) -> &str {
        &self.dsn
    }

    #[cfg(feature = "az-wire")]
    pub async fn attach_child(
        &mut self,
        node: impl Into<String>,
        topology: ::az_wire::TopologyConfig,
    ) -> Result<(), CacheError> {
        if self.topology.is_some() {
            return Err(CacheError::Config("primary child is already attached".into()));
        }
        if topology.host.is_some() || topology.parent.is_none() {
            return Err(CacheError::Config("embedded primary requires a listenerless parent topology".into()));
        }
        let (tables, pk, full) = crate::di::scan_schema(&self.db).await?;
        let versions = crate::version::VersionIndex::new(pk.clone(), full);
        let bridge = crate::cdc::CdcBridge::primary(versions.clone())?;
        let live = crate::live::LiveHub::start(&bridge, self.db.clone(), Arc::new(pk));
        let security_version = Arc::new(AtomicU64::new(0));
        let operations = crate::operations::ReadOperations::primary(
            self.db.clone(),
            tables.clone(),
            crate::cache::QueryCache::new(64 * 1024 * 1024),
            versions,
            live,
            security_version.clone(),
        );
        let observer = PrimaryObserver::start(&self.db, &tables, bridge, security_version).await?;
        let node = node.into();
        let built = crate::register_az_wire(
            ::az_wire::NodeBuilder::new(&node).insecure_accept_declared_peer_identities(),
            operations,
        )
        .build()
        .map_err(|error| CacheError::Config(error.to_string()))?;
        match built.start_topology(topology).await {
            Ok(running) => {
                self.observer = Some(observer);
                self.topology = Some(running);
                Ok(())
            }
            Err(error) => {
                observer.shutdown(&self.db).await?;
                Err(CacheError::Config(error.to_string()))
            }
        }
    }

    #[cfg(feature = "az-wire")]
    pub async fn shutdown(mut self) -> Result<(), CacheError> {
        if let Some(topology) = self.topology.take() {
            topology.shutdown().await.map_err(|error| CacheError::Config(error.to_string()))?;
        }
        if let Some(observer) = self.observer.take() {
            observer.shutdown(&self.db).await?;
        }
        self.db.close().await?;
        Ok(())
    }

    #[cfg(not(feature = "az-wire"))]
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
    let bootstrap = PGlite::open_multi_process(&config.data_dir, options.clone()).await?;
    if config.database == "postgres" {
        let db = bootstrap;
        return finish_primary(config, db).await;
    }
    {
        let literal = config.database.replace('\'', "''");
        let database = config.database.replace('"', "\"\"");
        if bootstrap
            .query(
                &format!("SELECT 1 FROM pg_database WHERE datname = '{literal}'"),
                &[],
            )
            .await?
            .is_empty()
        {
            bootstrap.exec(&format!("CREATE DATABASE \"{database}\"")).await?;
        }
        bootstrap.close().await?;
    }
    let db = PGlite::open_multi_process(&config.data_dir, MultiProcessOptions {
        database: config.database.clone(),
        ..options
    }).await?;
    finish_primary(config, db).await
}

async fn finish_primary(config: &PrimaryConfig, db: PGlite) -> Result<PrimaryHandle, CacheError> {
    let prepared = async {
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
    Ok(PrimaryHandle {
        db,
        dsn,
        #[cfg(feature = "az-wire")]
        topology: None,
        #[cfg(feature = "az-wire")]
        observer: None,
    })
}

#[cfg(feature = "az-wire")]
struct PrimaryObserver {
    channel: String,
    token: u64,
    bridge: crate::cdc::CdcBridge,
}

#[cfg(feature = "az-wire")]
impl PrimaryObserver {
    async fn start(
        db: &PGlite,
        tables: &std::collections::HashSet<String>,
        bridge: crate::cdc::CdcBridge,
        security_version: Arc<AtomicU64>,
    ) -> Result<PrimaryObserver, CacheError> {
        let channel = format!("pgpaw_primary_{}", std::process::id());
        let callback_bridge = bridge.clone();
        let token = db.listen(&channel, move |payload| {
            let Some((txid, table)) = payload.split_once(':') else { return };
            let Ok(xid) = txid.parse::<u32>() else { return };
            let lsn = security_version.fetch_add(1, Ordering::SeqCst) + 1;
            callback_bridge.publish(pglite::CommittedTransaction {
                xid,
                commit_lsn: pglite::Lsn(lsn),
                end_lsn: pglite::Lsn(lsn),
                commit_ts: 0,
                changes: vec![pglite::RowChange::Truncate { schema: "public".into(), table: table.into() }],
            });
        }).await?;
        for table in tables {
            let quoted = table.replace('"', "\"\"");
            let function = format!("_pgpaw_observe_{}", table.replace(|c: char| !c.is_ascii_alphanumeric(), "_"));
            db.exec(&format!(
                "CREATE OR REPLACE FUNCTION {function}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_notify('{channel}', txid_current()::text || ':' || TG_TABLE_NAME); RETURN NULL; END $$; DROP TRIGGER IF EXISTS {function} ON \"{quoted}\"; CREATE TRIGGER {function} AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE ON \"{quoted}\" FOR EACH STATEMENT EXECUTE FUNCTION {function}()"
            )).await?;
        }
        Ok(PrimaryObserver { channel, token, bridge })
    }

    async fn shutdown(self, db: &PGlite) -> Result<(), CacheError> {
        self.bridge.stop();
        db.unlisten_token(&self.channel, self.token).await?;
        Ok(())
    }
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
