#[cfg(feature = "read")]
mod auth;
#[cfg(feature = "az-wire")]
mod az_wire;
#[cfg(feature = "read")]
mod cache;
#[cfg(feature = "read")]
mod cdc;
#[cfg(feature = "read")]
mod classify;
#[cfg(feature = "read")]
mod composition;
mod db;
#[cfg(feature = "read")]
mod diff;
mod error;
#[cfg(feature = "server")]
mod http;
#[cfg(feature = "read")]
mod live;
#[cfg(feature = "read")]
mod operations;
#[cfg(feature = "read")]
mod rows;
#[cfg(feature = "read")]
mod schema;
#[cfg(feature = "read")]
mod version;
#[cfg(feature = "read")]
pub mod protocol;

#[cfg(all(test, feature = "server"))]
mod tests;

#[cfg(feature = "read")]
pub use auth::AuthConfig;
#[cfg(feature = "az-wire")]
pub use composition::AzWireConfig;
#[cfg(feature = "server")]
pub use composition::{HttpConfig, ReplicaSource, UpstreamConfig};
#[cfg(feature = "read")]
pub use composition::{CacheConfig, PgPaw, PgPawBuilder, PrimarySource, Source};
pub use error::{CacheError, LifecycleErrorKind};
#[cfg(feature = "server")]
pub use operations::HealthStatus;
#[cfg(feature = "read")]
pub use operations::{PreparedRead, ReadOperations};
pub use db::primary::recover_primary;
pub use db::shadow::{open_shadow, ShadowHandle};

#[cfg(feature = "server")]
pub async fn init(upstream: UpstreamConfig, publication: &str) -> Result<(), CacheError> {
    db::setup::prepare(&upstream, publication).await
}
