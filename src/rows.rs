use std::collections::HashSet;

use pglite::PGlite;

use crate::error::CacheError;

pub fn wrap_json(sql: &str) -> String {
    format!("select coalesce(jsonb_agg(to_jsonb(_t)), '[]'::jsonb)::text as j from ({sql}) _t")
}

pub async fn ensure_unique_columns(db: &PGlite, sql: &str) -> Result<(), CacheError> {
    let rows = db
        .query(&format!("select * from ({sql}) _pgpaw_cols limit 1"), &[])
        .await?;
    let Some(row) = rows.first() else {
        return Ok(());
    };
    let mut seen = HashSet::new();
    for column in row.columns() {
        if !seen.insert(column.name().to_ascii_lowercase()) {
            return Err(CacheError::Rejected(format!(
                "result has more than one column named `{}`; alias the columns so each is unique \
                 (e.g. select a.id as a_id, b.id as b_id) — a live query needs a stable per-row identity",
                column.name()
            )));
        }
    }
    Ok(())
}

pub async fn query_json(db: &PGlite, sql: &str) -> Result<String, CacheError> {
    let rows = db.query(&wrap_json(sql), &[]).await?;
    let body = match rows.first() {
        Some(row) => row.get::<Option<String>>(0)?,
        None => None,
    };
    Ok(body.unwrap_or_else(|| "[]".to_string()))
}

pub async fn query_json_as(
    db: &PGlite,
    role: &str,
    claims: &str,
    sql: &str,
) -> Result<String, CacheError> {
    let rows = db
        .query_as(role, Some(claims), &wrap_json(sql), &[])
        .await?;
    let body = match rows.first() {
        Some(row) => row.get::<Option<String>>(0)?,
        None => None,
    };
    Ok(body.unwrap_or_else(|| "[]".to_string()))
}
