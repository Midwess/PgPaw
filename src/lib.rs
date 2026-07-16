#[cfg(feature = "az-wire")]
mod az_wire;
#[cfg(feature = "read")]
mod capability;
#[cfg(feature = "read")]
mod composition;
mod db;
mod error;
#[cfg(feature = "server")]
mod http;
#[cfg(feature = "read")]
pub mod protocol;

#[cfg(all(test, feature = "server"))]
mod tests;

#[cfg(feature = "read")]
pub use capability::auth::AuthConfig;
#[cfg(feature = "az-wire")]
pub use composition::AzWireConfig;
#[cfg(feature = "server")]
pub use composition::{HttpConfig, ReplicaSource, UpstreamConfig};
#[cfg(feature = "read")]
pub use composition::{CacheConfig, PgPaw, PgPawBuilder, PrimarySource, Source};
pub use error::{CacheError, LifecycleErrorKind};
#[cfg(feature = "server")]
pub use capability::read::HealthStatus;
#[cfg(feature = "read")]
pub use capability::read::{PreparedRead, ReadOperations};
pub use db::primary::recover_primary;
pub use db::shadow::{open_shadow, ShadowHandle};

#[cfg(feature = "server")]
pub async fn init(upstream: UpstreamConfig, publication: &str) -> Result<(), CacheError> {
    db::setup::prepare(&upstream, publication).await
}
