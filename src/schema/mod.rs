use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use pglite::PGlite;

use crate::error::CacheError;

pub struct MigrationFile {
    pub filename: String,
    pub ordinal: u32,
    pub sql: String,
    pub checksum: String,
}

pub struct AppChain {
    pub app: String,
    pub rel_dir: PathBuf,
    pub files: Vec<MigrationFile>,
}

pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub has_default: bool,
}

pub struct TableInfo {
    pub name: String,
    pub columns: Vec<ColumnInfo>,
    pub primary_key: Vec<String>,
    pub foreign_keys: Vec<(String, String, String)>,
    pub rls_enabled: bool,
    pub rls_forced: bool,
}

#[derive(Debug)]
pub struct AppliedReport {
    pub applied: Vec<(String, String)>,
    pub already_applied: usize,
}

pub fn discover_migrations(
    world_dir: &Path,
    world_id: &str,
) -> Result<(Vec<AppChain>, Vec<String>), CacheError> {
    let mut chains = Vec::new();
    let mut warnings = Vec::new();
    let mut apps = vec![(world_id.to_string(), PathBuf::new())];
    let apps_dir = world_dir.join("apps");
    if apps_dir.is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(apps_dir)
            .map_err(io_error)?
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().is_dir())
            .collect();
        entries.sort_by_key(|entry| entry.file_name());
        apps.extend(entries.into_iter().map(|entry| {
            let app = entry.file_name().to_string_lossy().to_string();
            (app.clone(), PathBuf::from("apps").join(app))
        }));
    }
    for (app, app_rel) in apps {
        let dir = world_dir.join(&app_rel).join("migrations");
        if !dir.is_dir() {
            continue;
        }
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .map_err(io_error)?
            .filter_map(|e| {
                e.ok()
                    .and_then(|e| e.file_name().to_str().map(str::to_string))
            })
            .filter(|n| n.ends_with(".sql"))
            .collect();
        names.sort();
        let mut files = Vec::new();
        let mut seen = BTreeSet::new();
        for filename in names {
            let label = app_rel
                .join("migrations")
                .join(&filename)
                .to_string_lossy()
                .to_string();
            let ordinal: u32 = filename
                .split(|c: char| !c.is_ascii_digit())
                .next()
                .filter(|s| !s.is_empty())
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| {
                    CacheError::Rejected(format!(
                        "{label}: migration filenames start with an ordinal: NNNN_<name>.sql"
                    ))
                })?;
            if !seen.insert(ordinal) {
                return Err(CacheError::Rejected(format!(
                    "{label}: duplicate migration ordinal {ordinal}"
                )));
            }
            let sql = std::fs::read_to_string(dir.join(&filename)).map_err(io_error)?;
            let checksum = content_hash(&sql);
            files.push(MigrationFile {
                filename,
                ordinal,
                sql,
                checksum,
            });
        }
        files.sort_by_key(|f| f.ordinal);
        let ordinals: Vec<u32> = files.iter().map(|f| f.ordinal).collect();
        for pair in ordinals.windows(2) {
            if pair[1] != pair[0] + 1 {
                warnings.push(format!(
                    "{}/: ordinal gap between {} and {}",
                    app_rel.join("migrations").display(),
                    pair[0],
                    pair[1]
                ));
            }
        }
        chains.push(AppChain {
            app,
            rel_dir: app_rel.join("migrations"),
            files,
        });
    }
    Ok((chains, warnings))
}

pub const DESTRUCTIVE_ACK: &str = "pgpaw: acknowledge-destructive";

fn check_destructive_acknowledged(rel_dir: &Path, file: &MigrationFile) -> Result<(), CacheError> {
    use sqlparser::ast::{AlterTableOperation, ObjectType, Statement};
    let destructive = match sqlparser::parser::Parser::parse_sql(
        &sqlparser::dialect::PostgreSqlDialect {},
        &file.sql,
    ) {
        Ok(statements) => statements.iter().any(|statement| match statement {
            Statement::Drop { object_type, .. } => matches!(object_type, ObjectType::Table),
            Statement::Truncate { .. } => true,
            Statement::AlterTable { operations, .. } => operations
                .iter()
                .any(|op| matches!(op, AlterTableOperation::DropColumn { .. })),
            _ => false,
        }),
        Err(_) => {
            let lowered = file.sql.to_lowercase();
            lowered.contains("drop table")
                || lowered.contains("drop column")
                || lowered.contains("truncate ")
        }
    };
    if destructive && !file.sql.to_lowercase().contains(DESTRUCTIVE_ACK) {
        return Err(CacheError::Rejected(format!(
            "{}/{} contains a destructive contraction (DROP TABLE / DROP COLUMN / TRUNCATE); \
             schema evolution is expand/contract — confirm every run and snapshot depending on \
             the prior schema has retired, then acknowledge by adding the comment \
             `-- {DESTRUCTIVE_ACK}` to this migration file",
            rel_dir.display(),
            file.filename
        )));
    }
    Ok(())
}

pub fn content_hash(source: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in source.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

pub struct SchemaOps {
    db: PGlite,
}

impl SchemaOps {
    pub(crate) fn new(db: PGlite) -> SchemaOps {
        SchemaOps { db }
    }

    pub async fn ensure_ledgers(&self) -> Result<(), CacheError> {
        self.db
            .exec(
                "CREATE TABLE IF NOT EXISTS pgpaw_applied_migrations ( \
                   world_id text NOT NULL DEFAULT '', \
                   app text NOT NULL, \
                   filename text NOT NULL, \
                   checksum text NOT NULL, \
                   applied_at timestamptz NOT NULL DEFAULT now(), \
                   PRIMARY KEY (world_id, app, filename) \
                 ); \
                 CREATE TABLE IF NOT EXISTS pgpaw_app_schemas ( \
                   world_id text NOT NULL DEFAULT '', \
                   app text NOT NULL, \
                   tables jsonb NOT NULL DEFAULT '[]'::jsonb, \
                   applied_at timestamptz NOT NULL DEFAULT now(), \
                   PRIMARY KEY (world_id, app) \
                 )",
            )
            .await?;
        Ok(())
    }

    pub async fn handoff_legacy_ledgers(&self) -> Result<(), CacheError> {
        self.ensure_ledgers().await?;
        let tx = self.db.transaction().await?;
        let outcome: Result<(), CacheError> = async {
            let legacy_migrations = {
                let rows = tx
                    .query("SELECT (to_regclass('applied_app_migrations') IS NOT NULL)::int", &[])
                    .await?;
                rows.first().map(|row| row.get::<i32>(0)).transpose()?.unwrap_or(0) == 1
            };
            let legacy_schemas = {
                let rows = tx
                    .query("SELECT (to_regclass('app_schemas') IS NOT NULL)::int", &[])
                    .await?;
                rows.first().map(|row| row.get::<i32>(0)).transpose()?.unwrap_or(0) == 1
            };
            if legacy_migrations {
                let conflicts = tx
                    .query(
                        "SELECT count(*)::int8 FROM applied_app_migrations legacy \
                         JOIN pgpaw_applied_migrations new \
                           ON new.world_id = legacy.world_id AND new.app = legacy.app \
                              AND new.filename = legacy.filename \
                         WHERE new.checksum <> legacy.checksum",
                        &[],
                    )
                    .await?;
                let conflicts: i64 = conflicts
                    .first()
                    .map(|row| row.get::<Option<i64>>(0))
                    .transpose()?
                    .flatten()
                    .unwrap_or(0);
                if conflicts > 0 {
                    return Err(CacheError::Rejected(format!(
                        "ledger handoff found {conflicts} migration rows whose checksums \
                         disagree with already-migrated rows; resolve before retrying"
                    )));
                }
                tx.query(
                    "INSERT INTO pgpaw_applied_migrations (world_id, app, filename, checksum, applied_at) \
                     SELECT world_id, app, filename, checksum, applied_at FROM applied_app_migrations \
                     ON CONFLICT (world_id, app, filename) DO NOTHING",
                    &[],
                )
                .await?;
                let source: i64 = {
                    let rows = tx
                        .query("SELECT count(*)::int8 FROM applied_app_migrations", &[])
                        .await?;
                    rows.first()
                        .map(|row| row.get::<Option<i64>>(0))
                        .transpose()?
                        .flatten()
                        .unwrap_or(0)
                };
                let copied: i64 = tx
                    .query(
                        "SELECT count(*)::int8 FROM pgpaw_applied_migrations new \
                         WHERE EXISTS (SELECT 1 FROM applied_app_migrations legacy \
                           WHERE legacy.world_id = new.world_id AND legacy.app = new.app \
                             AND legacy.filename = new.filename)",
                        &[],
                    )
                    .await?
                    .first()
                    .map(|row| row.get::<Option<i64>>(0))
                    .transpose()?
                    .flatten()
                    .unwrap_or(0);
                if copied != source {
                    return Err(CacheError::Rejected(format!(
                        "ledger handoff copied {copied} of {source} migration rows; aborting"
                    )));
                }
                tx.query("DROP TABLE applied_app_migrations", &[]).await?;
            }
            if legacy_schemas {
                let conflicts: i64 = tx
                    .query(
                        "SELECT count(*)::int8 FROM app_schemas legacy \
                         JOIN pgpaw_app_schemas new \
                           ON new.world_id = legacy.world_id AND new.app = legacy.app \
                         WHERE new.tables IS DISTINCT FROM coalesce(legacy.tables, '[]'::jsonb)",
                        &[],
                    )
                    .await?
                    .first()
                    .map(|row| row.get::<Option<i64>>(0))
                    .transpose()?
                    .flatten()
                    .unwrap_or(0);
                if conflicts > 0 {
                    return Err(CacheError::Rejected(format!(
                        "ledger handoff found {conflicts} schema rows whose tables disagree \
                         with already-migrated rows; resolve before retrying"
                    )));
                }
                tx.query(
                    "INSERT INTO pgpaw_app_schemas (world_id, app, tables, applied_at) \
                     SELECT world_id, app, coalesce(tables, '[]'::jsonb), applied_at FROM app_schemas \
                     ON CONFLICT (world_id, app) DO NOTHING",
                    &[],
                )
                .await?;
                let source: i64 = {
                    let rows = tx
                        .query("SELECT count(*)::int8 FROM app_schemas", &[])
                        .await?;
                    rows.first()
                        .map(|row| row.get::<Option<i64>>(0))
                        .transpose()?
                        .flatten()
                        .unwrap_or(0)
                };
                let copied: i64 = tx
                    .query(
                        "SELECT count(*)::int8 FROM pgpaw_app_schemas new \
                         WHERE EXISTS (SELECT 1 FROM app_schemas legacy \
                           WHERE legacy.world_id = new.world_id AND legacy.app = new.app)",
                        &[],
                    )
                    .await?
                    .first()
                    .map(|row| row.get::<Option<i64>>(0))
                    .transpose()?
                    .flatten()
                    .unwrap_or(0);
                if copied != source {
                    return Err(CacheError::Rejected(format!(
                        "ledger handoff copied {copied} of {source} schema rows; aborting"
                    )));
                }
                tx.query("DROP TABLE app_schemas", &[]).await?;
            }
            Ok(())
        }
        .await;
        match outcome {
            Ok(()) => {
                tx.commit().await?;
                Ok(())
            }
            Err(error) => {
                let _ = tx.rollback().await;
                Err(error)
            }
        }
    }

    pub async fn apply_migrations(
        &self,
        world_id: &str,
        chains: &[AppChain],
    ) -> Result<AppliedReport, CacheError> {
        self.ensure_ledgers().await?;
        let mut applied = Vec::new();
        let mut already_applied = 0;
        for chain in chains {
            let ledger = self
                .db
                .query(
                    "SELECT filename FROM pgpaw_applied_migrations \
                     WHERE world_id = $1 AND app = $2 ORDER BY filename",
                    &[&world_id, &chain.app.as_str()],
                )
                .await?;
            for row in &ledger {
                let filename: String = row.get(0)?;
                if !chain.files.iter().any(|file| file.filename == filename) {
                    return Err(CacheError::Rejected(format!(
                        "{}/{filename} is applied but missing on disk (renamed or deleted); \
                         applied statements never re-execute — restore the file",
                        chain.rel_dir.display()
                    )));
                }
            }
            for file in &chain.files {
                let existing = self
                    .db
                    .query(
                        "SELECT checksum FROM pgpaw_applied_migrations \
                         WHERE world_id = $1 AND app = $2 AND filename = $3",
                        &[&world_id, &chain.app.as_str(), &file.filename.as_str()],
                    )
                    .await?;
                if let Some(row) = existing.first() {
                    let checksum: String = row.get(0)?;
                    if checksum != file.checksum {
                        return Err(CacheError::Rejected(format!(
                            "{}/{} changed after it was applied (checksum {} vs {}); \
                             migrations are immutable once applied",
                            chain.rel_dir.display(),
                            file.filename,
                            file.checksum,
                            checksum
                        )));
                    }
                    already_applied += 1;
                    continue;
                }
                check_destructive_acknowledged(&chain.rel_dir, file)?;
                let tx = self.db.transaction().await?;
                let outcome: Result<(), CacheError> = async {
                    tx.exec(&file.sql).await?;
                    tx.query(
                        "INSERT INTO pgpaw_applied_migrations (world_id, app, filename, checksum) \
                         VALUES ($1, $2, $3, $4)",
                        &[
                            &world_id,
                            &chain.app.as_str(),
                            &file.filename.as_str(),
                            &file.checksum.as_str(),
                        ],
                    )
                    .await?;
                    Ok(())
                }
                .await;
                match outcome {
                    Ok(()) => tx.commit().await?,
                    Err(error) => {
                        let _ = tx.rollback().await;
                        return Err(CacheError::Rejected(format!(
                            "{}/{}: {error}",
                            chain.rel_dir.display(),
                            file.filename
                        )));
                    }
                }
                applied.push((chain.app.clone(), file.filename.clone()));
            }
            let tables = self.reflected_table_names().await?;
            let tables_param = crate::capability::rows::JsonParam(serde_json::json!(tables));
            self.db
                .query(
                    "INSERT INTO pgpaw_app_schemas (world_id, app, tables, applied_at) \
                     VALUES ($1, $2, $3, now()) \
                     ON CONFLICT (world_id, app) DO UPDATE \
                     SET tables = EXCLUDED.tables, applied_at = now()",
                    &[&world_id, &chain.app.as_str(), &tables_param],
                )
                .await?;
        }
        Ok(AppliedReport {
            applied,
            already_applied,
        })
    }

    pub async fn foreign_key_graph(&self) -> Result<Vec<(String, String)>, CacheError> {
        let rows = self
            .db
            .query(
                "SELECT c.conrelid::regclass::text, c.confrelid::regclass::text \
                 FROM pg_constraint c WHERE c.contype = 'f'",
                &[],
            )
            .await?;
        let mut graph = Vec::new();
        for row in rows {
            graph.push((row.get(0)?, row.get(1)?));
        }
        Ok(graph)
    }

    pub async fn reflect_tables(&self, tables: &[String]) -> Result<Vec<TableInfo>, CacheError> {
        let mut out = Vec::new();
        for table in tables {
            let raw = self
                .db
                .query(
                    "SELECT column_name::text, data_type::text, is_nullable::text, \
                            (column_default IS NOT NULL)::int \
                     FROM information_schema.columns \
                     WHERE table_schema = 'public' AND table_name = $1 ORDER BY ordinal_position",
                    &[&table.as_str()],
                )
                .await?;
            let mut columns = Vec::new();
            for row in raw {
                columns.push(ColumnInfo {
                    name: row.get(0)?,
                    data_type: row.get(1)?,
                    nullable: row.get::<String>(2)? == "YES",
                    has_default: row.get::<i32>(3)? == 1,
                });
            }
            let pk = self
                .db
                .query(
                    "SELECT a.attname::text FROM pg_index i \
                     JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY(i.indkey) \
                     WHERE i.indrelid = ($1::text)::regclass AND i.indisprimary ORDER BY a.attnum",
                    &[&table.as_str()],
                )
                .await?;
            let mut primary_key = Vec::new();
            for row in pk {
                primary_key.push(row.get(0)?);
            }
            let fk = self
                .db
                .query(
                    "SELECT kcu.column_name::text, ccu.table_name::text, ccu.column_name::text \
                     FROM information_schema.table_constraints tc \
                     JOIN information_schema.key_column_usage kcu \
                       ON kcu.constraint_name = tc.constraint_name AND kcu.table_schema = tc.table_schema \
                     JOIN information_schema.constraint_column_usage ccu \
                       ON ccu.constraint_name = tc.constraint_name AND ccu.table_schema = tc.table_schema \
                     WHERE tc.constraint_type = 'FOREIGN KEY' AND tc.table_name = $1",
                    &[&table.as_str()],
                )
                .await?;
            let mut foreign_keys = Vec::new();
            for row in fk {
                foreign_keys.push((row.get(0)?, row.get(1)?, row.get(2)?));
            }
            let rls = self
                .db
                .query(
                    "SELECT relrowsecurity::int, relforcerowsecurity::int FROM pg_class \
                     WHERE oid = ($1::text)::regclass",
                    &[&table.as_str()],
                )
                .await?;
            let (rls_enabled, rls_forced) = match rls.first() {
                Some(row) => (row.get::<i32>(0)? == 1, row.get::<i32>(1)? == 1),
                None => (false, false),
            };
            out.push(TableInfo {
                name: table.clone(),
                columns,
                primary_key,
                foreign_keys,
                rls_enabled,
                rls_forced,
            });
        }
        Ok(out)
    }

    async fn reflected_table_names(&self) -> Result<Vec<String>, CacheError> {
        let rows = self
            .db
            .query(
                "SELECT tablename::text FROM pg_catalog.pg_tables \
                 WHERE schemaname = 'public' \
                   AND tablename NOT LIKE 'pgpaw\\_%' AND tablename <> '_pglite_replica' \
                 ORDER BY tablename",
                &[],
            )
            .await?;
        let mut names = Vec::new();
        for row in rows {
            names.push(row.get(0)?);
        }
        Ok(names)
    }
}

fn io_error(error: std::io::Error) -> CacheError {
    CacheError::Rejected(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::content_hash;

    #[test]
    fn checksums_match_the_worldant_ledger_format() {
        assert_eq!(content_hash(""), "cbf29ce484222325");
        assert_eq!(content_hash("a"), "af63dc4c8601ec8c");
    }
}
