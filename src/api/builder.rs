#[cfg(feature = "az-wire")]
use crate::api::config::AzWireConfig;
#[cfg(feature = "server")]
use crate::api::config::HttpConfig;
use crate::api::config::{CacheConfig, PgSource};
use crate::api::runtime::PgPaw;
use crate::capability::auth::AuthConfig;
use crate::error::CacheError;
#[cfg(feature = "az-wire")]
use crate::error::LifecycleErrorKind;

pub struct PgPawBuilder {
    source: Option<PgSource>,
    cache: CacheConfig,
    auth: AuthConfig,
    #[cfg(feature = "server")]
    http: Option<HttpConfig>,
    #[cfg(feature = "az-wire")]
    az_wire: Vec<AzWireConfig>,
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
            crate::source::build_read_core(source, self.cache, self.auth).await?;
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
            match crate::binding::http::server::bind_at(http.addr, http.cors_origin, data) {
                Ok(server) => {
                    pgpaw.http_handle = Some(server.handle());
                    pgpaw.http_task = Some(tokio::spawn(server));
                }
                Err(error) => return Err(pgpaw.abort_open(error).await),
            }
        }
        #[cfg(feature = "az-wire")]
        for config in self.az_wire {
            let node = match crate::binding::az_wire::register_az_wire(config.node, pgpaw.read.clone())
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
}
