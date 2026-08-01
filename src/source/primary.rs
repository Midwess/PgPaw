use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use pglite::PGlite;

use crate::api::config::{CacheConfig, EmbeddedPrimarySource};
use crate::api::runtime::SourceShutdown;
use crate::capability::auth::AuthConfig;
use crate::capability::cache::QueryCache;
use crate::capability::cdc::CdcBridge;
use crate::capability::live::LiveHub;
use crate::capability::read::ReadOperations;
use crate::capability::version::VersionIndex;
use crate::db::primary::PrimaryObserver;
use crate::error::CacheError;

pub(crate) async fn build_primary_core(
    source: EmbeddedPrimarySource,
    cache: CacheConfig,
    auth: AuthConfig,
) -> Result<(ReadOperations, PGlite, Option<String>, SourceShutdown), CacheError> {
    let (db, dsn) = crate::db::primary::open_primary_db(&source).await?;
    let assembled = async {
        db.exec(
            "DO $$ BEGIN \
               IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'pgpaw_public') THEN \
                 CREATE ROLE pgpaw_public NOLOGIN NOBYPASSRLS; \
               END IF; \
             END $$; \
             GRANT USAGE ON SCHEMA public TO pgpaw_public; \
             GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO pgpaw_public; \
             GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO pgpaw_public; \
             ALTER DEFAULT PRIVILEGES IN SCHEMA public \
               GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO pgpaw_public; \
             ALTER DEFAULT PRIVILEGES IN SCHEMA public \
               GRANT USAGE, SELECT ON SEQUENCES TO pgpaw_public; \
             DO $$ BEGIN \
               EXECUTE format('ALTER DATABASE %I SET request.headers = %L', \
                              current_database(), '{}'); \
             END $$",
        )
        .await?;
        let (tables, pk, full) = crate::capability::schema::scan_schema(&db).await?;
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
        Ok((read, observer)) => Ok((read, db, Some(dsn), SourceShutdown::Primary { observer })),
        Err(error) => {
            let _ = db.close().await;
            Err(error)
        }
    }
}
