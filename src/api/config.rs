use std::path::PathBuf;

#[cfg(feature = "server")]
use pglite::SslMode;

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
    pub(crate) fn sslmode(&self) -> SslMode {
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

pub struct EmbeddedPrimarySource {
    pub data_dir: PathBuf,
    pub database: String,
    pub listen_addresses: String,
    pub port: u16,
    pub min_connections: usize,
    pub max_connections: usize,
}

impl Default for EmbeddedPrimarySource {
    fn default() -> EmbeddedPrimarySource {
        EmbeddedPrimarySource {
            data_dir: "./cache-data".into(),
            database: "postgres".into(),
            listen_addresses: String::new(),
            port: 0,
            min_connections: 0,
            max_connections: 8,
        }
    }
}

impl EmbeddedPrimarySource {
    pub fn embedded(data_dir: impl Into<PathBuf>) -> EmbeddedPrimarySource {
        EmbeddedPrimarySource {
            data_dir: data_dir.into(),
            ..EmbeddedPrimarySource::default()
        }
    }
}

pub enum PgSource {
    #[cfg(feature = "server")]
    Replica(ReplicaSource),
    Primary(EmbeddedPrimarySource),
}

impl PgSource {
    #[cfg(feature = "server")]
    pub fn replica(source: ReplicaSource) -> PgSource {
        PgSource::Replica(source)
    }

    pub fn primary(source: EmbeddedPrimarySource) -> PgSource {
        PgSource::Primary(source)
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
    pub(crate) node: ::az_wire::NodeBuilder,
    pub(crate) topology: ::az_wire::TopologyConfig,
}
