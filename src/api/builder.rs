#[cfg(feature = "unb")]
use crate::api::config::UnbConfig;
#[cfg(feature = "server")]
use crate::api::config::HttpConfig;
use crate::api::config::{CacheConfig, PgSource};
use crate::api::runtime::PgPaw;
use crate::capability::auth::AuthConfig;
use crate::error::CacheError;
#[cfg(feature = "unb")]
use crate::error::LifecycleErrorKind;

pub struct PgPawBuilder {
    source: Option<PgSource>,
    cache: CacheConfig,
    auth: AuthConfig,
    #[cfg(feature = "server")]
    http: Option<HttpConfig>,
    #[cfg(feature = "unb")]
    unb: Vec<UnbConfig>,
}

impl PgPaw {
    pub fn builder() -> PgPawBuilder {
        PgPawBuilder {
            source: None,
            cache: CacheConfig::default(),
            auth: AuthConfig::default(),
            #[cfg(feature = "server")]
            http: None,
            #[cfg(feature = "unb")]
            unb: Vec::new(),
        }
    }
}

impl PgPawBuilder {
    pub fn source(mut self, source: PgSource) -> Self {
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

    #[cfg(feature = "unb")]
    pub fn unb(
        mut self,
        node: ::unb::NodeBuilder,
        topology: ::unb::TopologyConfig,
    ) -> Self {
        self.unb.push(UnbConfig { node, topology });
        self
    }

    pub async fn open(self) -> Result<PgPaw, CacheError> {
        let source = self
            .source
            .ok_or_else(|| CacheError::Config("PgPaw requires a source".into()))?;
        let (read, db, dsn, shutdown_state) =
            crate::source::build_read_core(source, self.cache, self.auth).await?;
        #[cfg_attr(not(any(feature = "server", feature = "unb")), allow(unused_mut))]
        let mut pgpaw = PgPaw {
            read,
            db,
            dsn,
            shutdown_state,
            #[cfg(feature = "server")]
            http_handle: None,
            #[cfg(feature = "server")]
            http_task: None,
            #[cfg(feature = "unb")]
            unb: Vec::new(),
        };
        if matches!(
            pgpaw.shutdown_state,
            crate::api::runtime::SourceShutdown::Primary { .. }
        ) {
            if let Err(error) = pgpaw.schema_ops().handoff_legacy_ledgers().await {
                return Err(pgpaw.abort_open(error).await);
            }
        }
        #[cfg(feature = "server")]
        if let Some(http) = self.http {
            let data = actix_web::web::Data::new(pgpaw.read.clone());
            match crate::binding::http::server::bind_at(http.addr, http.cors_origin, data) {
                Ok(server) => {
                    pgpaw.http_handle = Some(server.handle());
                    pgpaw.http_task = Some(tokio::spawn(server));
                }
                Err(error) => return Err(pgpaw.abort_open(error).await),
            }
        }
        #[cfg(feature = "unb")]
        for config in self.unb {
            let node =
                match crate::binding::unb::register_unb(config.node, pgpaw.read.clone())
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
                Ok(topology) => pgpaw.unb.push(topology),
                Err(error) => {
                    return Err(pgpaw
                        .abort_open(CacheError::lifecycle(LifecycleErrorKind::Topology, error))
                        .await)
                }
            }
        }
        Ok(pgpaw)
    }
}
