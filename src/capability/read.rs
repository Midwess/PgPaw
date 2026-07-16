use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use pglite::PGlite;
#[cfg(feature = "server")]
use pglite::Replica;

use crate::capability::auth::{Principal, Verifier};
use crate::capability::cache::{CachedResult, QueryCache};
use crate::capability::classify::{CacheableQuery, ReadClassifier};
use crate::error::CacheError;
use crate::capability::live::{LiveHub, LiveSubscription};
use crate::capability::rows;
use crate::capability::version::VersionIndex;

type SecurityCache = Arc<Mutex<(u64, HashMap<String, bool>)>>;

#[derive(Clone)]
pub struct ReadOperations {
    db: PGlite,
    #[cfg(feature = "server")]
    replica: Option<Replica>,
    security_version: Arc<AtomicU64>,
    classifier: ReadClassifier,
    verifier: Option<Verifier>,
    security_cache: Option<SecurityCache>,
    cache: QueryCache,
    versions: VersionIndex,
    live: LiveHub,
}

pub struct PreparedRead {
    pub query: CacheableQuery,
    pub principal: Option<Principal>,
    pub private: bool,
}

#[cfg(feature = "server")]
pub struct HealthStatus {
    pub halted: bool,
    pub reason: Option<String>,
    pub watermark: Option<u64>,
}

impl ReadOperations {
    #[cfg(feature = "server")]
    pub fn new(
        db: PGlite,
        replica: Replica,
        replicated: HashSet<String>,
        verifier: Option<Verifier>,
        cache: QueryCache,
        versions: VersionIndex,
        live: LiveHub,
    ) -> ReadOperations {
        ReadOperations {
            db,
            replica: Some(replica),
            security_version: Arc::new(AtomicU64::new(0)),
            classifier: ReadClassifier::new(replicated),
            verifier,
            security_cache: Some(Arc::new(Mutex::new((0, HashMap::new())))),
            cache,
            versions,
            live,
        }
    }

    pub(crate) fn primary(
        db: PGlite,
        tables: HashSet<String>,
        cache: QueryCache,
        versions: VersionIndex,
        live: LiveHub,
        security_version: Arc<AtomicU64>,
        verifier: Option<Verifier>,
    ) -> ReadOperations {
        ReadOperations {
            db,
            #[cfg(feature = "server")]
            replica: None,
            classifier: ReadClassifier::new(tables),
            verifier,
            security_cache: None,
            cache,
            versions,
            live,
            security_version,
        }
    }

    #[cfg(feature = "server")]
    pub fn health(&self) -> HealthStatus {
        match &self.replica {
            Some(replica) => HealthStatus {
                halted: replica.is_halted(),
                reason: replica.halt_reason(),
                watermark: Some(replica.watermark().0),
            },
            None => HealthStatus {
                halted: false,
                reason: None,
                watermark: None,
            },
        }
    }

    pub(crate) fn live_subscription_count(&self) -> usize {
        self.live.subscription_count()
    }

    pub fn authenticate(&self, bearer: Option<&str>) -> Result<Option<Principal>, CacheError> {
        let Some(token) = bearer else {
            return Ok(None);
        };
        self.verifier
            .as_ref()
            .ok_or_else(|| {
                CacheError::Unauthorized(
                    "a token was presented but JWT verification is not configured".to_string(),
                )
            })?
            .verify(token)
            .map(Some)
    }

    pub async fn prepare(
        &self,
        sql: &str,
        principal: Option<Principal>,
    ) -> Result<PreparedRead, CacheError> {
        #[cfg(feature = "server")]
        if let Some(replica) = &self.replica {
            if replica.is_halted() {
                return Err(CacheError::Halted(
                    replica
                        .halt_reason()
                        .unwrap_or_else(|| "unknown".to_string()),
                ));
            }
        }
        let query = self.classifier.classify(sql)?;
        if query.tables.len() > 1 {
            rows::ensure_unique_columns(&self.db, &query.sql).await?;
        }
        let private = self.is_private(&query.tables).await?;
        if private && principal.is_none() {
            return Err(CacheError::Unauthorized(
                "this query is access-controlled; a bearer token is required".to_string(),
            ));
        }
        Ok(PreparedRead {
            query,
            principal,
            private,
        })
    }

    pub async fn execute_private(&self, read: &PreparedRead) -> Result<String, CacheError> {
        let principal = read.principal.as_ref().ok_or_else(|| {
            CacheError::Unauthorized(
                "this query is access-controlled; a bearer token is required".to_string(),
            )
        })?;
        rows::query_json_as(
            &self.db,
            &principal.role,
            &principal.claims_json,
            &read.query.sql,
        )
        .await
        .map_err(map_db_denial)
    }

    pub async fn execute_public(&self, read: &PreparedRead) -> Result<String, CacheError> {
        rows::query_json(&self.db, &read.query.sql).await
    }

    pub async fn materialize(
        &self,
        read: &PreparedRead,
    ) -> Result<(String, u64, Arc<CachedResult>), CacheError> {
        let hash = format!("{:x}", read.query.fingerprint);
        let version = self
            .versions
            .version_of(&read.query.tables, &read.query.eq_filters)
            .0;
        let key = format!("{hash}:{version}");
        let sql = read.query.sql.clone();
        let db = self.db.clone();
        let snapshot = self
            .cache
            .get_or_compute(key, async move { rows::query_json(&db, &sql).await })
            .await?;
        Ok((hash, version, snapshot))
    }

    pub async fn cursor(&self, hash: &str, version: &str) -> Option<Arc<CachedResult>> {
        self.cache.get(&format!("{hash}:{version}")).await
    }

    pub fn materialize_version(&self, read: &PreparedRead) -> (String, u64, String) {
        let hash = format!("{:x}", read.query.fingerprint);
        let version = self
            .versions
            .version_of(&read.query.tables, &read.query.eq_filters)
            .0;
        let key = format!("{hash}:{version}");
        (hash, version, key)
    }

    pub async fn subscribe(&self, read: PreparedRead) -> Result<LiveSubscription, CacheError> {
        let version = self
            .versions
            .version_of(&read.query.tables, &read.query.eq_filters)
            .0;
        match read.principal {
            Some(principal) => {
                let body = rows::query_json_as(
                    &self.db,
                    &principal.role,
                    &principal.claims_json,
                    &read.query.sql,
                )
                .await
                .map_err(map_db_denial)?;
                Ok(self.live.subscribe(
                    read.query.sql,
                    read.query.tables,
                    String::new(),
                    version,
                    &body,
                    Some(principal),
                ))
            }
            None => {
                let (hash, version, snapshot) = self.materialize(&read).await?;
                Ok(self.live.subscribe(
                    read.query.sql,
                    read.query.tables,
                    hash,
                    version,
                    &snapshot.body,
                    None,
                ))
            }
        }
    }

    async fn is_private(&self, tables: &[String]) -> Result<bool, CacheError> {
        let Some(security_cache) = &self.security_cache else {
            return Ok(merge_verdicts(
                tables,
                &self.classify_security(tables).await?,
            ));
        };
        #[cfg(feature = "server")]
        let version = match &self.replica {
            Some(replica) => replica.security_version().await?,
            None => self.security_version.load(Ordering::SeqCst),
        };
        #[cfg(not(feature = "server"))]
        let version = self.security_version.load(Ordering::SeqCst);
        {
            let cache = security_cache.lock().unwrap();
            if cache.0 == version && tables.iter().all(|table| cache.1.contains_key(table)) {
                return Ok(merge_verdicts(tables, &cache.1));
            }
        }
        let verdicts = self.classify_security(tables).await?;
        let mut cache = security_cache.lock().unwrap();
        if cache.0 != version {
            cache.0 = version;
            cache.1.clear();
        }
        for (table, private) in &verdicts {
            cache.1.insert(table.clone(), *private);
        }
        Ok(merge_verdicts(tables, &verdicts))
    }

    async fn classify_security(
        &self,
        tables: &[String],
    ) -> Result<HashMap<String, bool>, CacheError> {
        if tables.is_empty() {
            return Ok(HashMap::new());
        }
        let names = tables.to_vec();
        let rows = self.db.query(
            "select lower(c.relname), (c.relrowsecurity or not has_table_privilege('public', c.oid, 'SELECT'))::int from pg_class c join pg_namespace n on n.oid = c.relnamespace where c.relkind = 'r' and n.nspname not in ('pg_catalog', 'information_schema') and lower(c.relname) = any($1)",
            &[&names],
        ).await?;
        let mut verdicts = HashMap::new();
        for row in &rows {
            let name: String = row.get(0)?;
            let private: i32 = row.get(1)?;
            verdicts
                .entry(name)
                .and_modify(|existing| *existing = true)
                .or_insert(private == 1);
        }
        Ok(verdicts)
    }
}

pub(crate) fn merge_verdicts(tables: &[String], verdicts: &HashMap<String, bool>) -> bool {
    tables
        .iter()
        .any(|table| verdicts.get(table).copied().unwrap_or(true))
}

pub(crate) fn map_db_denial(error: CacheError) -> CacheError {
    if let CacheError::Pglite(pglite::Error::Database { sqlstate, .. }) = &error {
        if matches!(sqlstate.as_str(), "42501" | "42704" | "28000") {
            return CacheError::Forbidden(error.to_string());
        }
    }
    error
}

#[cfg(test)]
mod tests {
    use super::{map_db_denial, merge_verdicts};
    use crate::error::CacheError;
    use std::collections::HashMap;

    #[test]
    fn security_classification_fails_closed() {
        let verdicts = HashMap::from([("public".to_string(), false)]);
        assert!(!merge_verdicts(&["public".to_string()], &verdicts));
        assert!(merge_verdicts(&["missing".to_string()], &verdicts));
    }

    #[test]
    fn database_denials_are_forbidden() {
        let error = CacheError::Pglite(pglite::Error::Database {
            sqlstate: "42501".to_string(),
            message: "denied".to_string(),
            detail: None,
            hint: None,
        });
        assert!(matches!(map_db_denial(error), CacheError::Forbidden(_)));
    }
}
