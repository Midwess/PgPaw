use pglite::{MultiProcessOptions, PGlite};
use tempfile::TempDir;

use crate::error::{CacheError, LifecycleErrorKind};

pub struct ShadowHandle {
    _db: PGlite,
    dsn: String,
    _data_dir: TempDir,
}

impl ShadowHandle {
    pub fn dsn(&self) -> &str {
        &self.dsn
    }
}

pub async fn open_shadow() -> Result<ShadowHandle, CacheError> {
    let data_dir = tempfile::tempdir()?;
    let options = MultiProcessOptions {
        listen_addresses: None,
        port: None,
        max_connections: 4,
        ..Default::default()
    };
    let db = PGlite::open_multi_process(data_dir.path(), options)
        .await
        .map_err(|error| CacheError::lifecycle(LifecycleErrorKind::Startup, error))?;
    let dsn = db
        .connection_uri()
        .ok_or_else(|| CacheError::Config("shadow engine exposes no connection_uri".into()))?;
    Ok(ShadowHandle {
        _db: db,
        dsn,
        _data_dir: data_dir,
    })
}
