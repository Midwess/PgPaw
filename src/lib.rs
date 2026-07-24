#[cfg(feature = "read")]
mod api;
mod binding;
#[cfg(feature = "read")]
mod capability;
mod db;
mod error;
#[cfg(feature = "read")]
pub mod protocol;
pub mod schema;
pub use capability::sql::{SqlOperations, SqlOutcome};
#[cfg(feature = "read")]
mod source;

#[cfg(all(test, feature = "server"))]
mod tests;

#[cfg(feature = "read")]
pub use api::builder::PgPawBuilder;
#[cfg(feature = "az-wire")]
pub use api::config::AzWireConfig;
#[cfg(feature = "read")]
pub use api::config::{CacheConfig, EmbeddedPrimarySource, PgSource};
#[cfg(feature = "server")]
pub use api::config::{HttpConfig, ReplicaSource, UpstreamConfig};
#[cfg(feature = "read")]
pub use api::runtime::PgPaw;
#[cfg(feature = "read")]
pub use capability::auth::AuthConfig;
#[cfg(feature = "server")]
pub use capability::read::HealthStatus;
#[cfg(feature = "read")]
pub use capability::read::{PreparedRead, ReadOperations};
pub use db::primary::recover_primary;
pub use db::shadow::{open_shadow, ShadowHandle};
pub use error::{CacheError, LifecycleErrorKind};

#[cfg(feature = "server")]
pub async fn init(upstream: UpstreamConfig, publication: &str) -> Result<(), CacheError> {
    db::setup::prepare(&upstream, publication).await
}
