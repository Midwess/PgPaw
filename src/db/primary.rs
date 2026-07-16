#[cfg(feature = "read")]
use pglite::{MultiProcessOptions, PGlite};
use std::path::Path;
#[cfg(feature = "read")]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(feature = "read")]
use std::sync::Arc;

#[cfg(feature = "read")]
use crate::composition::PrimarySource;
use crate::error::{CacheError, LifecycleErrorKind};

#[cfg(feature = "read")]
pub(crate) async fn open_primary_db(
    source: &PrimarySource,
) -> Result<(PGlite, String), CacheError> {
    if source.max_connections == 0 || source.min_connections > source.max_connections {
        return Err(CacheError::Config(
            "primary connections require max_connections greater than zero and min_connections no greater than max_connections".into(),
        ));
    }
    if source.database.is_empty()
        || source.database.len() > 63
        || !source
            .database
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(CacheError::Config(
            "primary database must use letters, digits, or underscores and contain at most 63 bytes"
                .into(),
        ));
    }
    let listen_addresses = if source.listen_addresses.is_empty() {
        None
    } else {
        Some(source.listen_addresses.clone())
    };
    let port = if source.port == 0 {
        None
    } else {
        Some(source.port)
    };
    log::info!(
        "event=primary_open_start listen_addr={} port={} data_dir={:?} max_connections={}",
        source.listen_addresses,
        source.port,
        source.data_dir,
        source.max_connections,
    );
    let options = MultiProcessOptions {
        database: "postgres".into(),
        min_connections: source.min_connections,
        listen_addresses,
        port,
        max_connections: source.max_connections,
        ..Default::default()
    };
    recover_primary(&source.data_dir)?;
    let bootstrap = PGlite::open_multi_process(&source.data_dir, options.clone())
        .await
        .map_err(|error| primary_start_error(error, &source.data_dir))?;
    let db = if source.database == "postgres" {
        bootstrap
    } else {
        let literal = source.database.replace('\'', "''");
        let database = source.database.replace('"', "\"\"");
        let ensured = async {
            if bootstrap
                .query(
                    &format!("SELECT 1 FROM pg_database WHERE datname = '{literal}'"),
                    &[],
                )
                .await?
                .is_empty()
            {
                bootstrap
                    .exec(&format!("CREATE DATABASE \"{database}\""))
                    .await?;
            }
            Ok::<(), CacheError>(())
        }
        .await;
        if let Err(error) = ensured {
            let _ = bootstrap.close().await;
            return Err(error);
        }
        bootstrap.close().await?;
        PGlite::open_multi_process(
            &source.data_dir,
            MultiProcessOptions {
                database: source.database.clone(),
                ..options
            },
        )
        .await
        .map_err(|error| primary_start_error(error, &source.data_dir))?
    };
    let prepared = async {
        let base = db.connection_uri().ok_or_else(|| {
            CacheError::Config("primary engine exposes no connection_uri".into())
        })?;
        let (address, query) = base
            .split_once('?')
            .map(|(address, query)| (address, Some(query)))
            .unwrap_or((base.as_str(), None));
        let (prefix, _) = address.rsplit_once('/').ok_or_else(|| {
            CacheError::Config("primary engine exposes an invalid connection_uri".into())
        })?;
        Ok::<String, CacheError>(match query {
            Some(query) => format!("{prefix}/{}?{query}", source.database),
            None => format!("{prefix}/{}", source.database),
        })
    }
    .await;
    match prepared {
        Ok(dsn) => {
            log::info!("event=primary_open_complete data_dir={:?}", source.data_dir);
            Ok((db, dsn))
        }
        Err(error) => {
            let _ = db.close().await;
            Err(error)
        }
    }
}

pub fn recover_primary(data_dir: impl AsRef<Path>) -> Result<(), CacheError> {
    recover_primary_inner(data_dir.as_ref())
}

#[cfg(feature = "read")]
fn primary_start_error(error: pglite::Error, data_dir: &Path) -> CacheError {
    if matches!(error, pglite::Error::PostmasterStart(_)) && primary_is_busy(data_dir) {
        return CacheError::lifecycle(LifecycleErrorKind::DataDirectoryBusy, data_dir.display());
    }
    let kind = match error {
        pglite::Error::PoolExhausted | pglite::Error::Closed | pglite::Error::Protocol(_) => {
            LifecycleErrorKind::Connection
        }
        _ => LifecycleErrorKind::Startup,
    };
    CacheError::lifecycle(kind, error)
}

#[cfg(unix)]
fn recover_primary_inner(data_dir: &Path) -> Result<(), CacheError> {
    let path = data_dir.join("postmaster.pid");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(CacheError::lifecycle(LifecycleErrorKind::Recovery, error));
        }
    };
    let Some(pid) = text
        .lines()
        .next()
        .and_then(|line| line.trim().parse::<i32>().ok())
    else {
        return Err(CacheError::lifecycle(
            LifecycleErrorKind::Recovery,
            format!("invalid ownership metadata in {}", path.display()),
        ));
    };
    if !process_is_alive(pid) {
        return remove_pid_file(&path)
            .map_err(|error| CacheError::lifecycle(LifecycleErrorKind::Recovery, error));
    }
    Err(CacheError::lifecycle(
        LifecycleErrorKind::DataDirectoryBusy,
        data_dir.display(),
    ))
}

#[cfg(not(unix))]
fn recover_primary_inner(_data_dir: &Path) -> Result<(), CacheError> {
    Ok(())
}

#[cfg(all(unix, feature = "read"))]
fn primary_is_busy(data_dir: &Path) -> bool {
    std::fs::read_to_string(data_dir.join("postmaster.pid"))
        .ok()
        .and_then(|text| text.lines().next()?.trim().parse::<i32>().ok())
        .is_some_and(process_is_alive)
}

#[cfg(all(not(unix), feature = "read"))]
fn primary_is_busy(_data_dir: &Path) -> bool {
    false
}

#[cfg(unix)]
fn process_is_alive(pid: i32) -> bool {
    if pid <= 1 {
        return true;
    }
    let result = unsafe { kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() != Some(3)
}

#[cfg(unix)]
fn remove_pid_file(path: &Path) -> Result<(), std::io::Error> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

#[cfg(feature = "read")]
pub(crate) struct PrimaryObserver {
    channel: String,
    token: u64,
    bridge: crate::cdc::CdcBridge,
}

#[cfg(feature = "read")]
impl PrimaryObserver {
    pub(crate) async fn start(
        db: &PGlite,
        tables: &std::collections::HashSet<String>,
        bridge: crate::cdc::CdcBridge,
        security_version: Arc<AtomicU64>,
    ) -> Result<PrimaryObserver, CacheError> {
        let channel = format!("pgpaw_primary_{}", std::process::id());
        let callback_bridge = bridge.clone();
        let token = db
            .listen(&channel, move |payload| {
                let Some((txid, table)) = payload.split_once(':') else {
                    return;
                };
                let Ok(xid) = txid.parse::<u32>() else { return };
                let lsn = security_version.fetch_add(1, Ordering::SeqCst) + 1;
                callback_bridge.publish(pglite::CommittedTransaction {
                    xid,
                    commit_lsn: pglite::Lsn(lsn),
                    end_lsn: pglite::Lsn(lsn),
                    commit_ts: 0,
                    changes: vec![pglite::RowChange::Truncate {
                        schema: "public".into(),
                        table: table.into(),
                    }],
                });
            })
            .await?;
        let mut installation = String::from("BEGIN;");
        for table in tables {
            let quoted = table.replace('"', "\"\"");
            let function = format!(
                "_pgpaw_observe_{}",
                table.replace(|c: char| !c.is_ascii_alphanumeric(), "_")
            );
            installation.push_str(&format!(
                "CREATE OR REPLACE FUNCTION {function}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_notify('{channel}', txid_current()::text || ':' || TG_TABLE_NAME); RETURN NULL; END $$; DROP TRIGGER IF EXISTS {function} ON \"{quoted}\"; CREATE TRIGGER {function} AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE ON \"{quoted}\" FOR EACH STATEMENT EXECUTE FUNCTION {function}();"
            ));
        }
        installation.push_str("COMMIT;");
        if let Err(error) = db.exec(&installation).await {
            db.unlisten_token(&channel, token).await?;
            return Err(error.into());
        }
        Ok(PrimaryObserver {
            channel,
            token,
            bridge,
        })
    }

    pub(crate) async fn shutdown(self, db: &PGlite) -> Result<(), CacheError> {
        self.bridge.stop();
        db.unlisten_token(&self.channel, self.token).await?;
        Ok(())
    }
}

#[cfg(all(test, feature = "read"))]
mod tests {
    use super::*;

    #[test]
    fn startup_errors_have_stable_categories() {
        let dir = Path::new("missing");
        assert_eq!(
            primary_start_error(pglite::Error::Boot("failed".into()), dir).lifecycle_kind(),
            Some(LifecycleErrorKind::Startup)
        );
        assert_eq!(
            primary_start_error(pglite::Error::PoolExhausted, dir).lifecycle_kind(),
            Some(LifecycleErrorKind::Connection)
        );
    }
}
