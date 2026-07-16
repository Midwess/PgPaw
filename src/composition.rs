use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

#[cfg(feature = "server")]
use pglite::{MultiProcessOptions, Replica, ReplicaConfig, SslMode};
use pglite::PGlite;

use crate::auth::AuthConfig;
use crate::cache::QueryCache;
use crate::cdc::CdcBridge;
use crate::error::{CacheError, LifecycleErrorKind};
use crate::live::LiveHub;
use crate::operations::ReadOperations;
use crate::db::primary::PrimaryObserver;
use crate::version::VersionIndex;

#[cfg(feature = "server")]
pub struct UpstreamConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub database: String,
    pub sslmode: String,
}

#[cfg(feature = "server")]
impl Default for UpstreamConfig {
    fn default() -> UpstreamConfig {
        UpstreamConfig {
            host: "127.0.0.1".into(),
            port: 5432,
            user: "postgres".into(),
            password: String::new(),
            database: "postgres".into(),
            sslmode: "disable".into(),
        }
    }
}

#[cfg(feature = "server")]
impl UpstreamConfig {
    fn sslmode(&self) -> SslMode {
        match self.sslmode.to_ascii_lowercase().as_str() {
            "prefer" => SslMode::Prefer,
            "require" => SslMode::Require,
            "verify-full" | "verify_full" | "verifyfull" => SslMode::VerifyFull,
            _ => SslMode::Disable,
        }
    }
}

#[cfg(feature = "server")]
pub struct ReplicaSource {
    pub upstream: UpstreamConfig,
    pub data_dir: PathBuf,
    pub publication: String,
    pub slot: String,
    pub max_connections: usize,
}

#[cfg(feature = "server")]
impl Default for ReplicaSource {
    fn default() -> ReplicaSource {
        ReplicaSource {
            upstream: UpstreamConfig::default(),
            data_dir: "./cache-data".into(),
            publication: "pgpaw_pub".into(),
            slot: "pgpaw_slot".into(),
            max_connections: 8,
        }
    }
}

pub struct PrimarySource {
    pub data_dir: PathBuf,
    pub database: String,
    pub listen_addresses: String,
    pub port: u16,
    pub min_connections: usize,
    pub max_connections: usize,
}

impl Default for PrimarySource {
    fn default() -> PrimarySource {
        PrimarySource {
            data_dir: "./cache-data".into(),
            database: "postgres".into(),
            listen_addresses: String::new(),
            port: 0,
            min_connections: 0,
            max_connections: 8,
        }
    }
}

impl PrimarySource {
    pub fn embedded(data_dir: impl Into<PathBuf>) -> PrimarySource {
        PrimarySource {
            data_dir: data_dir.into(),
            ..PrimarySource::default()
        }
    }
}

pub enum Source {
    #[cfg(feature = "server")]
    Replica(ReplicaSource),
    Primary(PrimarySource),
}

impl Source {
    #[cfg(feature = "server")]
    pub fn replica(source: ReplicaSource) -> Source {
        Source::Replica(source)
    }

    pub fn primary(source: PrimarySource) -> Source {
        Source::Primary(source)
    }
}

pub struct CacheConfig {
    pub max_bytes: u64,
}

impl Default for CacheConfig {
    fn default() -> CacheConfig {
        CacheConfig {
            max_bytes: 268_435_456,
        }
    }
}

#[cfg(feature = "server")]
pub struct HttpConfig {
    pub addr: std::net::SocketAddr,
    pub cors_origin: Option<String>,
}

#[cfg(feature = "az-wire")]
pub struct AzWireConfig {
    node: ::az_wire::NodeBuilder,
    topology: ::az_wire::TopologyConfig,
}

pub struct PgPawBuilder {
    source: Option<Source>,
    cache: CacheConfig,
    auth: AuthConfig,
    #[cfg(feature = "server")]
    http: Option<HttpConfig>,
    #[cfg(feature = "az-wire")]
    az_wire: Vec<AzWireConfig>,
}

impl PgPawBuilder {
    pub fn source(mut self, source: Source) -> Self {
        self.source = Some(source);
        self
    }

    pub fn cache(mut self, cache: CacheConfig) -> Self {
        self.cache = cache;
        self
    }

    pub fn auth(mut self, auth: AuthConfig) -> Self {
        self.auth = auth;
        self
    }

    #[cfg(feature = "server")]
    pub fn http(mut self, http: HttpConfig) -> Self {
        self.http = Some(http);
        self
    }

    #[cfg(feature = "az-wire")]
    pub fn az_wire(
        mut self,
        node: ::az_wire::NodeBuilder,
        topology: ::az_wire::TopologyConfig,
    ) -> Self {
        self.az_wire.push(AzWireConfig { node, topology });
        self
    }

    pub async fn open(self) -> Result<PgPaw, CacheError> {
        let source = self
            .source
            .ok_or_else(|| CacheError::Config("PgPaw requires a source".into()))?;
        let (read, db, dsn, shutdown_state) =
            Self::build_read_core(source, self.cache, self.auth).await?;
        #[cfg_attr(
            not(any(feature = "server", feature = "az-wire")),
            allow(unused_mut)
        )]
        let mut pgpaw = PgPaw {
            read,
            db,
            dsn,
            shutdown_state,
            #[cfg(feature = "server")]
            http_handle: None,
            #[cfg(feature = "server")]
            http_task: None,
            #[cfg(feature = "az-wire")]
            az_wire: Vec::new(),
        };
        #[cfg(feature = "server")]
        if let Some(http) = self.http {
            let data = actix_web::web::Data::new(pgpaw.read.clone());
            match crate::http::server::bind_at(http.addr, http.cors_origin, data) {
                Ok(server) => {
                    pgpaw.http_handle = Some(server.handle());
                    pgpaw.http_task = Some(tokio::spawn(server));
                }
                Err(error) => return Err(pgpaw.abort_open(error).await),
            }
        }
        #[cfg(feature = "az-wire")]
        for config in self.az_wire {
            let node = match crate::az_wire::register_az_wire(config.node, pgpaw.read.clone())
                .build()
            {
                Ok(node) => node,
                Err(error) => {
                    return Err(pgpaw
                        .abort_open(CacheError::lifecycle(LifecycleErrorKind::Topology, error))
                        .await)
                }
            };
            match node.start_topology(config.topology).await {
                Ok(topology) => pgpaw.az_wire.push(topology),
                Err(error) => {
                    return Err(pgpaw
                        .abort_open(CacheError::lifecycle(LifecycleErrorKind::Topology, error))
                        .await)
                }
            }
        }
        Ok(pgpaw)
    }

    async fn build_read_core(
        source: Source,
        cache: CacheConfig,
        auth: AuthConfig,
    ) -> Result<(ReadOperations, PGlite, Option<String>, SourceShutdown), CacheError> {
        match source {
            #[cfg(feature = "server")]
            Source::Replica(source) => Self::build_replica_core(source, cache, auth).await,
            Source::Primary(source) => Self::build_primary_core(source, cache, auth).await,
        }
    }

    #[cfg(feature = "server")]
    async fn build_replica_core(
        source: ReplicaSource,
        cache: CacheConfig,
        auth: AuthConfig,
    ) -> Result<(ReadOperations, PGlite, Option<String>, SourceShutdown), CacheError> {
        log::info!(
            "event=preflight_start upstream_host={} upstream_port={} upstream_user={} upstream_database={} publication={}",
            source.upstream.host,
            source.upstream.port,
            source.upstream.user,
            source.upstream.database,
            source.publication,
        );
        crate::db::setup::preflight(&source.upstream, &source.publication).await?;
        log::info!("event=preflight_complete result=ok");

        let options = MultiProcessOptions {
            max_connections: source.max_connections,
            ..Default::default()
        };
        log::info!(
            "event=replica_open_start data_dir={:?} max_connections={}",
            source.data_dir,
            source.max_connections,
        );
        let db = PGlite::open_multi_process(&source.data_dir, options).await?;
        log::info!("event=replica_open_complete result=ok");

        let replica_config = ReplicaConfig {
            host: source.upstream.host.clone(),
            port: source.upstream.port,
            user: source.upstream.user.clone(),
            password: source.upstream.password.clone(),
            database: source.upstream.database.clone(),
            publication: source.publication.clone(),
            slot_name: source.slot.clone(),
            sslmode: source.upstream.sslmode(),
            ..Default::default()
        };
        log::info!(
            "event=replication_start upstream_host={} upstream_port={} upstream_database={} publication={} slot={}",
            source.upstream.host,
            source.upstream.port,
            source.upstream.database,
            source.publication,
            source.slot,
        );
        let replica = match Replica::start(db.clone(), replica_config).await {
            Ok(replica) => replica,
            Err(error) => {
                let _ = db.shutdown().await;
                return Err(error.into());
            }
        };
        log::info!("event=replication_started result=ok");

        let assembled = async {
            let (replicated, pk, full) = crate::schema::scan_schema(&db).await?;
            log::info!(
                "event=schema_scan_complete tables={} primary_key_tables={} replica_identity_full_tables={}",
                replicated.len(),
                pk.len(),
                full.len(),
            );
            let versions = VersionIndex::new(pk.clone(), full);
            let cdc = CdcBridge::start(&replica, versions.clone())?;
            log::info!("event=cdc_bridge_started");
            let live = LiveHub::start(&cdc, db.clone(), Arc::new(pk));
            log::info!("event=live_hub_started");
            let store = QueryCache::new(cache.max_bytes);
            log::info!("event=query_cache_configured max_bytes={}", cache.max_bytes);
            let verifier = auth.into_verifier()?;
            log::info!(
                "event=auth_configured jwt_verification={}",
                verifier.is_some()
            );
            let read = ReadOperations::new(
                db.clone(),
                replica.clone(),
                replicated,
                verifier,
                store,
                versions,
                live,
            );
            Ok::<_, CacheError>((read, cdc))
        }
        .await;
        match assembled {
            Ok((read, cdc)) => Ok((
                read,
                db,
                None,
                SourceShutdown::Replica { replica, cdc },
            )),
            Err(error) => {
                replica.stop();
                let _ = db.shutdown().await;
                Err(error)
            }
        }
    }

    async fn build_primary_core(
        source: PrimarySource,
        cache: CacheConfig,
        auth: AuthConfig,
    ) -> Result<(ReadOperations, PGlite, Option<String>, SourceShutdown), CacheError> {
        let (db, dsn) = crate::db::primary::open_primary_db(&source).await?;
        let assembled = async {
            let (tables, pk, full) = crate::schema::scan_schema(&db).await?;
            let versions = VersionIndex::new(pk.clone(), full);
            let bridge = CdcBridge::primary(versions.clone())?;
            let live = LiveHub::start(&bridge, db.clone(), Arc::new(pk));
            let store = QueryCache::new(cache.max_bytes);
            let verifier = auth.into_verifier()?;
            let security_version = Arc::new(AtomicU64::new(0));
            let read = ReadOperations::primary(
                db.clone(),
                tables.clone(),
                store,
                versions,
                live,
                security_version.clone(),
                verifier,
            );
            let observer = PrimaryObserver::start(&db, &tables, bridge, security_version).await?;
            Ok::<_, CacheError>((read, observer))
        }
        .await;
        match assembled {
            Ok((read, observer)) => Ok((
                read,
                db,
                Some(dsn),
                SourceShutdown::Primary { observer },
            )),
            Err(error) => {
                let _ = db.close().await;
                Err(error)
            }
        }
    }
}

enum SourceShutdown {
    #[cfg(feature = "server")]
    Replica { replica: Replica, cdc: CdcBridge },
    Primary { observer: PrimaryObserver },
}

pub struct PgPaw {
    read: ReadOperations,
    db: PGlite,
    dsn: Option<String>,
    shutdown_state: SourceShutdown,
    #[cfg(feature = "server")]
    http_handle: Option<actix_web::dev::ServerHandle>,
    #[cfg(feature = "server")]
    http_task: Option<tokio::task::JoinHandle<std::io::Result<()>>>,
    #[cfg(feature = "az-wire")]
    az_wire: Vec<::az_wire::AzWireTopology>,
}

impl std::fmt::Debug for PgPaw {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgPaw")
            .field("primary_dsn", &self.dsn)
            .finish_non_exhaustive()
    }
}

impl PgPaw {
    pub fn builder() -> PgPawBuilder {
        PgPawBuilder {
            source: None,
            cache: CacheConfig::default(),
            auth: AuthConfig::default(),
            #[cfg(feature = "server")]
            http: None,
            #[cfg(feature = "az-wire")]
            az_wire: Vec::new(),
        }
    }

    pub fn primary_dsn(&self) -> Option<&str> {
        self.dsn.as_deref()
    }

    pub fn live_subscription_count(&self) -> usize {
        self.read.live_subscription_count()
    }

    pub async fn wait(&mut self) -> Result<(), CacheError> {
        #[allow(unused_mut)]
        let mut has_bindings = false;
        #[cfg(feature = "server")]
        {
            has_bindings |= self.http_task.is_some();
        }
        #[cfg(feature = "az-wire")]
        {
            has_bindings |= !self.az_wire.is_empty();
        }
        if !has_bindings {
            std::future::pending::<()>().await;
        }
        loop {
            #[cfg(feature = "server")]
            if self
                .http_task
                .as_ref()
                .is_some_and(tokio::task::JoinHandle::is_finished)
            {
                let task = self.http_task.take().expect("http task checked above");
                return task
                    .await
                    .map_err(|error| CacheError::Io(std::io::Error::other(error)))
                    .and_then(|served| served.map_err(CacheError::Io));
            }
            #[cfg(feature = "az-wire")]
            if let Some(topology) = self
                .az_wire
                .iter_mut()
                .find(|topology| topology.is_finished())
            {
                return topology.wait().await.map_err(|error| {
                    CacheError::lifecycle(LifecycleErrorKind::Topology, error)
                });
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    #[cfg_attr(
        not(any(feature = "server", feature = "az-wire")),
        allow(unused_mut)
    )]
    pub async fn shutdown(mut self) -> Result<(), CacheError> {
        let mut result = Ok(());
        #[cfg(feature = "server")]
        {
            if let Some(handle) = self.http_handle.take() {
                handle.stop(true).await;
            }
            if let Some(task) = self.http_task.take() {
                let served = task
                    .await
                    .map_err(|error| CacheError::Io(std::io::Error::other(error)))
                    .and_then(|served| served.map_err(CacheError::Io));
                if let Err(error) = served {
                    if result.is_ok() {
                        result = Err(error);
                    }
                }
            }
        }
        #[cfg(feature = "az-wire")]
        for topology in self.az_wire.drain(..) {
            if let Err(error) = topology.shutdown().await {
                if result.is_ok() {
                    result = Err(CacheError::lifecycle(LifecycleErrorKind::Shutdown, error));
                }
            }
        }
        match self.shutdown_state {
            #[cfg(feature = "server")]
            SourceShutdown::Replica { replica, cdc } => {
                log::info!("event=replication_stop_start");
                replica.stop();
                cdc.stop();
                let _ = self.db.shutdown().await;
                log::info!("event=replication_stop_complete");
            }
            SourceShutdown::Primary { observer } => {
                if let Err(error) = observer.shutdown(&self.db).await {
                    if result.is_ok() {
                        result = Err(error);
                    }
                }
                if let Err(error) = self.db.close().await {
                    if result.is_ok() {
                        result =
                            Err(CacheError::lifecycle(LifecycleErrorKind::Shutdown, error));
                    }
                }
            }
        }
        result
    }

    #[cfg(any(feature = "server", feature = "az-wire"))]
    async fn abort_open(self, error: CacheError) -> CacheError {
        if let Err(cleanup) = self.shutdown().await {
            log::warn!(
                "event=open_rollback_failed error={:?}",
                cleanup.to_string()
            );
        }
        error
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ensure_runtime_dir() {
        if std::env::var_os("PGLITE_RUNTIME_DIR").is_none() {
            std::env::set_var(
                "PGLITE_RUNTIME_DIR",
                concat!(env!("CARGO_MANIFEST_DIR"), "/target/pglite-rt"),
            );
        }
    }

    #[tokio::test]
    async fn open_requires_a_source() {
        let error = PgPaw::builder().open().await.unwrap_err();
        assert_eq!(
            error.lifecycle_kind(),
            Some(LifecycleErrorKind::InvalidConfiguration)
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn primary_source_opens_reports_dsn_and_releases_on_shutdown() {
        ensure_runtime_dir();
        let dir = tempfile::tempdir().unwrap();
        let pgpaw = PgPaw::builder()
            .source(Source::primary(PrimarySource::embedded(dir.path())))
            .open()
            .await
            .unwrap();
        assert!(pgpaw.primary_dsn().is_some());
        pgpaw.shutdown().await.unwrap();

        let reopened = PgPaw::builder()
            .source(Source::primary(PrimarySource::embedded(dir.path())))
            .open()
            .await
            .unwrap();
        reopened.shutdown().await.unwrap();
    }

    #[cfg(feature = "server")]
    fn http_get(addr: std::net::SocketAddr, path: &str) -> String {
        use std::io::{Read, Write};
        let mut stream = std::net::TcpStream::connect(addr).unwrap();
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }

    #[cfg(feature = "server")]
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn http_binding_over_primary_serves_health() {
        ensure_runtime_dir();
        let dir = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let pgpaw = PgPaw::builder()
            .source(Source::primary(PrimarySource::embedded(dir.path())))
            .http(HttpConfig {
                addr,
                cors_origin: None,
            })
            .open()
            .await
            .unwrap();
        let response = http_get(addr, "/healthz");
        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("\"status\":\"ok\""));
        assert!(!response.contains("watermark"));
        pgpaw.shutdown().await.unwrap();
        std::net::TcpListener::bind(addr).unwrap();
    }

    #[cfg(feature = "server")]
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn http_bind_conflict_rolls_back_the_primary_source() {
        ensure_runtime_dir();
        let dir = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let error = PgPaw::builder()
            .source(Source::primary(PrimarySource::embedded(dir.path())))
            .http(HttpConfig {
                addr,
                cors_origin: None,
            })
            .open()
            .await
            .unwrap_err();
        assert!(matches!(error, CacheError::Io(_)));
        drop(listener);

        let reopened = PgPaw::builder()
            .source(Source::primary(PrimarySource::embedded(dir.path())))
            .open()
            .await
            .unwrap();
        reopened.shutdown().await.unwrap();
    }
}
