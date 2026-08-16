use pglite::PGlite;
#[cfg(feature = "server")]
use pglite::Replica;

#[cfg(feature = "server")]
use crate::capability::cdc::CdcBridge;
use crate::capability::read::ReadOperations;
use crate::db::primary::PrimaryObserver;
use crate::error::{CacheError, LifecycleErrorKind};

pub(crate) enum SourceShutdown {
    #[cfg(feature = "server")]
    Replica {
        replica: Replica,
        cdc: CdcBridge,
    },
    Primary {
        observer: PrimaryObserver,
    },
}

pub struct PgPaw {
    pub(crate) read: ReadOperations,
    pub(crate) db: PGlite,
    pub(crate) dsn: Option<String>,
    pub(crate) shutdown_state: SourceShutdown,
    #[cfg(feature = "server")]
    pub(crate) http_handle: Option<actix_web::dev::ServerHandle>,
    #[cfg(feature = "server")]
    pub(crate) http_task: Option<tokio::task::JoinHandle<std::io::Result<()>>>,
    #[cfg(feature = "unb")]
    pub(crate) unb: Vec<(std::sync::Arc<::unb::Node>, ::unb::UnbTopology)>,
}

impl std::fmt::Debug for PgPaw {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgPaw")
            .field("primary_dsn", &self.dsn)
            .finish_non_exhaustive()
    }
}

impl PgPaw {
    pub fn sql_operations(
        &self,
        public_role: Option<String>,
    ) -> crate::capability::sql::SqlOperations {
        self.read.sql_operations(public_role)
    }

    pub fn schema_ops(&self) -> crate::schema::SchemaOps {
        crate::schema::SchemaOps::new(self.db.clone())
    }

    pub fn primary_dsn(&self) -> Option<&str> {
        self.dsn.as_deref()
    }

    pub fn live_subscription_count(&self) -> usize {
        self.read.live_subscription_count()
    }

    #[cfg(feature = "unb")]
    pub async fn attach_unb(
        &mut self,
        node: ::unb::NodeBuilder,
        topology: ::unb::TopologyConfig,
    ) -> Result<(), CacheError> {
        let node = crate::binding::unb::register_unb(node, self.read.clone())
            .build()
            .map_err(|error| CacheError::lifecycle(LifecycleErrorKind::Topology, error))?;
        let topology = node
            .start_topology(topology)
            .await
            .map_err(|error| CacheError::lifecycle(LifecycleErrorKind::Topology, error))?;
        self.unb.push((node, topology));
        Ok(())
    }

    pub async fn wait(&mut self) -> Result<(), CacheError> {
        #[allow(unused_mut)]
        let mut has_bindings = false;
        #[cfg(feature = "server")]
        {
            has_bindings |= self.http_task.is_some();
        }
        #[cfg(feature = "unb")]
        {
            has_bindings |= !self.unb.is_empty();
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
            #[cfg(feature = "unb")]
            if let Some((_, topology)) = self
                .unb
                .iter_mut()
                .find(|(_, topology)| topology.is_finished())
            {
                return topology
                    .wait()
                    .await
                    .map_err(|error| CacheError::lifecycle(LifecycleErrorKind::Topology, error));
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    #[cfg_attr(not(any(feature = "server", feature = "unb")), allow(unused_mut))]
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
        #[cfg(feature = "unb")]
        for (_, topology) in self.unb.drain(..) {
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
                        result = Err(CacheError::lifecycle(LifecycleErrorKind::Shutdown, error));
                    }
                }
            }
        }
        result
    }

    #[cfg(any(feature = "server", feature = "unb"))]
    pub(crate) async fn abort_open(self, error: CacheError) -> CacheError {
        if let Err(cleanup) = self.shutdown().await {
            log::warn!("event=open_rollback_failed error={:?}", cleanup.to_string());
        }
        error
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "server")]
    use crate::api::config::HttpConfig;
    use crate::api::config::{EmbeddedPrimarySource, PgSource};

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
            .source(PgSource::primary(EmbeddedPrimarySource::embedded(
                dir.path(),
            )))
            .open()
            .await
            .unwrap();
        assert!(pgpaw.primary_dsn().is_some());
        pgpaw.shutdown().await.unwrap();

        let reopened = PgPaw::builder()
            .source(PgSource::primary(EmbeddedPrimarySource::embedded(
                dir.path(),
            )))
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
            .source(PgSource::primary(EmbeddedPrimarySource::embedded(
                dir.path(),
            )))
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
            .source(PgSource::primary(EmbeddedPrimarySource::embedded(
                dir.path(),
            )))
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
            .source(PgSource::primary(EmbeddedPrimarySource::embedded(
                dir.path(),
            )))
            .open()
            .await
            .unwrap();
        reopened.shutdown().await.unwrap();
    }
}
