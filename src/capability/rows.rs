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

pub struct SqlOutcome {
    pub rows_json: String,
    pub rows_affected: u64,
}

pub fn to_sql_params(
    params: &[serde_json::Value],
) -> Vec<Box<dyn pglite::ToSql + Sync + Send>> {
    params
        .iter()
        .map(|param| -> Box<dyn pglite::ToSql + Sync + Send> {
            match param {
                serde_json::Value::Null => Box::new(Option::<String>::None),
                serde_json::Value::Bool(b) => Box::new(*b),
                serde_json::Value::Number(n) => match (n.as_i64(), n.as_f64()) {
                    (Some(i), _) => Box::new(i),
                    (None, Some(f)) => Box::new(f),
                    (None, None) => Box::new(n.to_string()),
                },
                serde_json::Value::String(s) => Box::new(s.clone()),
                other => Box::new(other.to_string()),
            }
        })
        .collect()
}

pub async fn run_sql_as(
    db: &PGlite,
    role: &str,
    claims: Option<&str>,
    validated: &crate::capability::sql_validate::ValidatedSql,
    sql: &str,
    params: &[serde_json::Value],
) -> Result<SqlOutcome, CacheError> {
    let boxed = to_sql_params(params);
    let refs: Vec<&(dyn pglite::ToSql + Sync)> = boxed
        .iter()
        .map(|param| param.as_ref() as &(dyn pglite::ToSql + Sync))
        .collect();
    let tx = db.transaction().await?;
    if let Some(claims) = claims {
        tx.query(
            &format!("SET LOCAL request.jwt.claims = {}", sql_literal(claims)),
            &[],
        )
        .await?;
    }
    tx.query(&format!("SET LOCAL ROLE {}", sql_ident(role)), &[])
        .await?;
    let outcome = async {
        if validated.command == "SELECT" {
            let probe = tx
                .query(
                    &format!("select * from ({sql}) _pgpaw_cols limit 1"),
                    &refs,
                )
                .await?;
            if let Some(row) = probe.first() {
                let mut seen = HashSet::new();
                for column in row.columns() {
                    if !seen.insert(column.name().to_ascii_lowercase()) {
                        return Err(CacheError::Rejected(format!(
                            "result has more than one column named `{}`; alias the columns so \
                             each output name is unique",
                            column.name()
                        )));
                    }
                }
            }
            let rows = tx.query(&wrap_json(sql), &refs).await?;
            let body = match rows.first() {
                Some(row) => row.get::<Option<String>>(0)?,
                None => None,
            };
            let rows_json = body.unwrap_or_else(|| "[]".to_string());
            return Ok(SqlOutcome {
                rows_json,
                rows_affected: 0,
            });
        }
        if validated.returns_rows {
            let trimmed = sql.trim_end().trim_end_matches(';');
            let wrapped = format!(
                "WITH _pgpaw_dml AS ({trimmed}) \
                 select coalesce(jsonb_agg(to_jsonb(_t)), '[]'::jsonb)::text as j \
                 from _pgpaw_dml _t"
            );
            let rows = tx.query(&wrapped, &refs).await?;
            let body = match rows.first() {
                Some(row) => row.get::<Option<String>>(0)?,
                None => None,
            };
            let rows_json = body.unwrap_or_else(|| "[]".to_string());
            let affected = serde_json::from_str::<serde_json::Value>(&rows_json)
                .ok()
                .and_then(|value| value.as_array().map(|rows| rows.len() as u64))
                .unwrap_or(0);
            return Ok(SqlOutcome {
                rows_json,
                rows_affected: affected,
            });
        }
        if validated.needs_count_wrap {
            let wrapped = crate::capability::sql_validate::count_wrapped(sql);
            let rows = tx.query(&wrapped, &refs).await?;
            let affected: i64 = match rows.first() {
                Some(row) => row.get::<Option<i64>>(0)?.unwrap_or(0),
                None => 0,
            };
            return Ok(SqlOutcome {
                rows_json: "[]".into(),
                rows_affected: affected.max(0) as u64,
            });
        }
        tx.query(sql, &refs).await?;
        Ok(SqlOutcome {
            rows_json: "[]".into(),
            rows_affected: 0,
        })
    }
    .await;
    match outcome {
        Ok(outcome) => {
            tx.commit().await?;
            Ok(outcome)
        }
        Err(error) => {
            let _ = tx.rollback().await;
            Err(error)
        }
    }
}

fn sql_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn sql_ident(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}
