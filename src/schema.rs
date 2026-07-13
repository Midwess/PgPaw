use std::collections::{HashMap, HashSet};

use pglite::PGlite;

use crate::error::CacheError;

pub(crate) async fn scan_schema(db: &PGlite) -> Result<(HashSet<String>, HashMap<String, String>, HashSet<String>), CacheError> {
    let table_rows = db.query("select tablename from pg_tables where schemaname not in ('pg_catalog', 'information_schema')", &[]).await?;
    let mut tables = HashSet::new();
    for row in table_rows {
        let name: String = row.get(0)?;
        if name != "_pglite_replica" { tables.insert(name.to_ascii_lowercase()); }
    }
    let pk_rows = db.query("select tc.table_name, kcu.column_name from information_schema.table_constraints tc join information_schema.key_column_usage kcu on kcu.constraint_name = tc.constraint_name and kcu.table_schema = tc.table_schema where tc.constraint_type = 'PRIMARY KEY' and tc.table_schema not in ('pg_catalog', 'information_schema')", &[]).await?;
    let mut pk_columns: HashMap<String, Vec<String>> = HashMap::new();
    for row in pk_rows {
        let table: String = row.get(0)?;
        let column: String = row.get(1)?;
        pk_columns.entry(table.to_ascii_lowercase()).or_default().push(column);
    }
    let pk = pk_columns.into_iter().filter(|(_, columns)| columns.len() == 1).map(|(table, mut columns)| (table, columns.remove(0))).collect();
    let full_rows = db.query("select c.relname from pg_class c join pg_namespace n on n.oid = c.relnamespace where c.relkind = 'r' and c.relreplident = 'f' and n.nspname not in ('pg_catalog', 'information_schema')", &[]).await?;
    let mut full = HashSet::new();
    for row in full_rows {
        let name: String = row.get(0)?;
        full.insert(name.to_ascii_lowercase());
    }
    Ok((tables, pk, full))
}
