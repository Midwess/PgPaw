use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use pglite::{MultiProcessOptions, PGlite, Replica, ReplicaConfig, SslMode};
use tokio::sync::OnceCell;

use crate::auth::Verifier;
use crate::cache::QueryCache;
use crate::cdc::CdcBridge;
use crate::classify::ReadClassifier;
use crate::error::CacheError;
use crate::live::LiveHub;
use crate::version::VersionIndex;

static INSTANCE: OnceCell<Di> = OnceCell::const_new();

pub struct UpstreamConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub database: String,
    pub publication: String,
    pub slot: String,
    pub sslmode: String,
}

pub struct ServerConfig {
    pub bind_addr: String,
    pub data_dir: PathBuf,
    pub max_connections: usize,
    pub cache_size_bytes: u64,
    pub jwt_secret: Option<String>,
    pub jwt_public_key: Option<String>,
    pub jwt_jwks_url: Option<String>,
    pub jwt_role_claim: String,
    pub upstream: UpstreamConfig,
}

pub struct Di {
    db: PGlite,
    replica: Replica,
    versions: VersionIndex,
    cache: QueryCache,
    classifier: ReadClassifier,
    live: LiveHub,
    tables: HashSet<String>,
    bind_addr: String,
    verifier: Option<Verifier>,
    security_cache: Arc<Mutex<(u64, HashMap<String, bool>)>>,
    #[allow(dead_code)]
    cdc: CdcBridge,
}

impl Di {
    pub async fn init(config: ServerConfig) -> Result<(), CacheError> {
        crate::setup::preflight(&config.upstream).await?;

        let options = MultiProcessOptions {
            max_connections: config.max_connections,
            ..Default::default()
        };
        let db = PGlite::open_multi_process(&config.data_dir, options).await?;

        let replica_config = ReplicaConfig {
            host: config.upstream.host.clone(),
            port: config.upstream.port,
            user: config.upstream.user.clone(),
            password: config.upstream.password.clone(),
            database: config.upstream.database.clone(),
            publication: config.upstream.publication.clone(),
            slot_name: config.upstream.slot.clone(),
            sslmode: parse_sslmode(&config.upstream.sslmode),
            ..Default::default()
        };
        let replica = Replica::start(db.clone(), replica_config).await?;

        let (replicated, pk, full) = scan_schema(&db).await?;
        let versions = VersionIndex::new(pk.clone(), full);
        let cdc = CdcBridge::start(&replica, versions.clone())?;
        let live = LiveHub::start(&cdc, db.clone(), Arc::new(pk));
        let cache = QueryCache::new(config.cache_size_bytes);
        let classifier = ReadClassifier::new(replicated.clone());
        let verifier = Verifier::build(
            config.jwt_secret,
            config.jwt_public_key,
            config.jwt_jwks_url,
            config.jwt_role_claim,
        )?;

        let di = Di {
            db,
            replica,
            versions,
            cache,
            classifier,
            live,
            tables: replicated,
            bind_addr: config.bind_addr,
            verifier,
            security_cache: Arc::new(Mutex::new((0, HashMap::new()))),
            cdc,
        };

        INSTANCE
            .set(di)
            .map_err(|_| CacheError::Config("dependencies already initialized".to_string()))
    }

    pub fn instance() -> &'static Di {
        INSTANCE.get().expect("dependencies not initialized")
    }

    pub async fn shutdown(&self) {
        self.replica.stop();
        self.cdc.stop();
        let _ = self.db.shutdown().await;
    }

    pub fn db(&self) -> &PGlite {
        &self.db
    }

    pub fn replica(&self) -> &Replica {
        &self.replica
    }

    pub fn versions(&self) -> &VersionIndex {
        &self.versions
    }

    pub fn cache(&self) -> &QueryCache {
        &self.cache
    }

    pub fn classifier(&self) -> &ReadClassifier {
        &self.classifier
    }

    pub fn live(&self) -> &LiveHub {
        &self.live
    }

    pub fn tables(&self) -> &HashSet<String> {
        &self.tables
    }

    pub fn bind_addr(&self) -> &str {
        &self.bind_addr
    }

    pub fn verifier(&self) -> Option<&Verifier> {
        self.verifier.as_ref()
    }

    pub async fn is_private(&self, tables: &[String]) -> Result<bool, CacheError> {
        let version = self.replica.security_version().await?;
        {
            let cache = self.security_cache.lock().unwrap();
            if cache.0 == version && tables.iter().all(|table| cache.1.contains_key(table)) {
                return Ok(Self::merge_verdicts(tables, &cache.1));
            }
        }
        let verdicts = self.classify_security(tables).await?;
        let mut cache = self.security_cache.lock().unwrap();
        if cache.0 != version {
            cache.0 = version;
            cache.1.clear();
        }
        for (table, private) in &verdicts {
            cache.1.insert(table.clone(), *private);
        }
        Ok(Self::merge_verdicts(tables, &verdicts))
    }

    fn merge_verdicts(tables: &[String], verdicts: &HashMap<String, bool>) -> bool {
        tables
            .iter()
            .any(|table| verdicts.get(table).copied().unwrap_or(true))
    }

    async fn classify_security(
        &self,
        tables: &[String],
    ) -> Result<HashMap<String, bool>, CacheError> {
        if tables.is_empty() {
            return Ok(HashMap::new());
        }
        let names: Vec<String> = tables.to_vec();
        let rows = self
            .db
            .query(
                "select c.relname, \
                        (c.relrowsecurity or not has_table_privilege('public', c.oid, 'SELECT'))::int \
                 from pg_class c join pg_namespace n on n.oid = c.relnamespace \
                 where c.relkind = 'r' \
                   and n.nspname not in ('pg_catalog', 'information_schema') \
                   and c.relname = any($1)",
                &[&names],
            )
            .await?;
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

async fn scan_schema(
    db: &PGlite,
) -> Result<(HashSet<String>, HashMap<String, String>, HashSet<String>), CacheError> {
    let table_rows = db
        .query(
            "select tablename from pg_tables \
             where schemaname not in ('pg_catalog', 'information_schema')",
            &[],
        )
        .await?;
    let mut tables = HashSet::new();
    for row in table_rows {
        let name: String = row.get(0)?;
        if name != "_pglite_replica" {
            tables.insert(name);
        }
    }

    let pk_rows = db
        .query(
            "select tc.table_name, kcu.column_name \
             from information_schema.table_constraints tc \
             join information_schema.key_column_usage kcu \
               on kcu.constraint_name = tc.constraint_name \
               and kcu.table_schema = tc.table_schema \
             where tc.constraint_type = 'PRIMARY KEY' \
               and tc.table_schema not in ('pg_catalog', 'information_schema')",
            &[],
        )
        .await?;
    let mut pk_columns: HashMap<String, Vec<String>> = HashMap::new();
    for row in pk_rows {
        let table: String = row.get(0)?;
        let column: String = row.get(1)?;
        pk_columns.entry(table).or_default().push(column);
    }
    let pk = pk_columns
        .into_iter()
        .filter(|(_, columns)| columns.len() == 1)
        .map(|(table, mut columns)| (table, columns.remove(0)))
        .collect();

    let full_rows = db
        .query(
            "select c.relname from pg_class c \
             join pg_namespace n on n.oid = c.relnamespace \
             where c.relkind = 'r' and c.relreplident = 'f' \
               and n.nspname not in ('pg_catalog', 'information_schema')",
            &[],
        )
        .await?;
    let mut full = HashSet::new();
    for row in full_rows {
        let name: String = row.get(0)?;
        full.insert(name);
    }

    Ok((tables, pk, full))
}

fn parse_sslmode(value: &str) -> SslMode {
    match value.to_ascii_lowercase().as_str() {
        "prefer" => SslMode::Prefer,
        "require" => SslMode::Require,
        "verify-full" | "verify_full" | "verifyfull" => SslMode::VerifyFull,
        _ => SslMode::Disable,
    }
}

#[cfg(test)]
mod tests {
    use super::Di;
    use std::collections::HashMap;

    fn verdicts(pairs: &[(&str, bool)]) -> HashMap<String, bool> {
        pairs.iter().map(|(t, p)| (t.to_string(), *p)).collect()
    }

    #[test]
    fn any_private_table_makes_query_private() {
        let v = verdicts(&[("pub", false), ("secret", true)]);
        assert!(Di::merge_verdicts(&["pub".into(), "secret".into()], &v));
    }

    #[test]
    fn all_public_tables_stay_public() {
        let v = verdicts(&[("a", false), ("b", false)]);
        assert!(!Di::merge_verdicts(&["a".into(), "b".into()], &v));
    }

    #[test]
    fn unknown_table_fails_closed_to_private() {
        let v = verdicts(&[("a", false)]);
        assert!(Di::merge_verdicts(&["ghost".into()], &v));
    }

    #[test]
    fn empty_tables_are_public() {
        assert!(!Di::merge_verdicts(&[], &verdicts(&[])));
    }
}
