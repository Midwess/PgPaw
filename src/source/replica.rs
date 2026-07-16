use std::sync::Arc;

use pglite::{MultiProcessOptions, PGlite, Replica, ReplicaConfig};

use crate::api::config::{CacheConfig, ReplicaSource};
use crate::api::runtime::SourceShutdown;
use crate::capability::auth::AuthConfig;
use crate::capability::cache::QueryCache;
use crate::capability::cdc::CdcBridge;
use crate::capability::live::LiveHub;
use crate::capability::read::ReadOperations;
use crate::capability::version::VersionIndex;
use crate::error::CacheError;

pub(crate) async fn build_replica_core(
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
        let (replicated, pk, full) = crate::capability::schema::scan_schema(&db).await?;
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
