mod primary;
#[cfg(feature = "server")]
mod replica;

use pglite::PGlite;

use crate::api::config::{CacheConfig, PgSource};
use crate::api::runtime::SourceShutdown;
use crate::capability::auth::AuthConfig;
use crate::capability::read::ReadOperations;
use crate::error::CacheError;

pub(crate) async fn build_read_core(
    source: PgSource,
    cache: CacheConfig,
    auth: AuthConfig,
) -> Result<(ReadOperations, PGlite, Option<String>, SourceShutdown), CacheError> {
    match source {
        #[cfg(feature = "server")]
        PgSource::Replica(source) => replica::build_replica_core(source, cache, auth).await,
        PgSource::Primary(source) => primary::build_primary_core(source, cache, auth).await,
    }
}
