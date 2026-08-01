use std::collections::HashSet;

use pglite::PGlite;

use crate::capability::auth::Principal;
use crate::error::CacheError;

pub fn wrap_json(sql: &str) -> String {
    format!("select coalesce(jsonb_agg(to_jsonb(_t)), '[]'::jsonb)::text as j from ({sql}) _t")
}

pub async fn begin_scoped<'db>(
    db: &'db PGlite,
    role: Option<&str>,
    claims: Option<&str>,
    headers: Option<&str>,
) -> Result<pglite::Transaction<'db>, CacheError> {
    let tx = db.transaction().await?;
    tx.query(
        &format!(
            "SET LOCAL statement_timeout = '{}s'",
            crate::capability::sql::SQL_DEADLINE_SECS
        ),
        &[],
    )
    .await?;
    if let Some(claims) = claims {
        tx.query(
            &format!("SET LOCAL request.jwt.claims = {}", sql_literal(claims)),
            &[],
        )
        .await?;
    }
    tx.query(
        &format!(
            "SET LOCAL request.headers = {}",
            sql_literal(headers.unwrap_or("{}"))
        ),
        &[],
    )
    .await?;
    if let Some(role) = role {
        tx.query(&format!("SET LOCAL ROLE {}", sql_ident(role)), &[])
            .await?;
    }
    Ok(tx)
}

pub async fn ensure_unique_columns(
    db: &PGlite,
    principal: Option<&Principal>,
    headers: Option<&str>,
    sql: &str,
    params: &[serde_json::Value],
) -> Result<(), CacheError> {
    let boxed = to_sql_params(params);
    let refs: Vec<&(dyn pglite::ToSql + Sync)> = boxed
        .iter()
        .map(|param| param.as_ref() as &(dyn pglite::ToSql + Sync))
        .collect();
    let probe = format!("select * from ({sql}) _pgpaw_cols limit 1");
    let rows = if principal.is_none() && headers.is_none() {
        db.query(&probe, &refs).await?
    } else {
        let tx = begin_scoped(
            db,
            Some(scoped_role(principal)),
            principal.map(|principal| principal.claims_json.as_str()),
            headers,
        )
        .await?;
        let rows = tx.query(&probe, &refs).await?;
        tx.commit().await?;
        rows
    };
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

pub async fn query_json_scoped(
    db: &PGlite,
    principal: Option<&Principal>,
    headers: Option<&str>,
    sql: &str,
    params: &[serde_json::Value],
) -> Result<String, CacheError> {
    let boxed = to_sql_params(params);
    let refs: Vec<&(dyn pglite::ToSql + Sync)> = boxed
        .iter()
        .map(|param| param.as_ref() as &(dyn pglite::ToSql + Sync))
        .collect();
    let body = if principal.is_none() && headers.is_none() {
        let rows = db.query(&wrap_json(sql), &refs).await?;
        match rows.first() {
            Some(row) => row.get::<Option<String>>(0)?,
            None => None,
        }
    } else {
        let tx = begin_scoped(
            db,
            Some(scoped_role(principal)),
            principal.map(|principal| principal.claims_json.as_str()),
            headers,
        )
        .await?;
        let rows = tx.query(&wrap_json(sql), &refs).await?;
        let body = match rows.first() {
            Some(row) => row.get::<Option<String>>(0)?,
            None => None,
        };
        tx.commit().await?;
        body
    };
    Ok(body.unwrap_or_else(|| "[]".to_string()))
}

fn scoped_role(principal: Option<&Principal>) -> &str {
    principal
        .map(|principal| principal.role.as_str())
        .unwrap_or(crate::capability::sql::DEFAULT_PUBLIC_ROLE)
}

pub struct SqlOutcome {
    pub rows_json: String,
    pub rows_affected: u64,
}

pub fn to_sql_params(params: &[serde_json::Value]) -> Vec<Box<dyn pglite::ToSql + Sync + Send>> {
    params
        .iter()
        .map(|param| -> Box<dyn pglite::ToSql + Sync + Send> { Box::new(JsonParam(param.clone())) })
        .collect()
}

#[derive(Debug)]
pub struct JsonParam(pub serde_json::Value);

impl pglite::ToSql for JsonParam {
    fn to_sql(
        &self,
        ty: &postgres_types::Type,
        out: &mut bytes::BytesMut,
    ) -> Result<postgres_types::IsNull, Box<dyn std::error::Error + Sync + Send>> {
        use postgres_types::{IsNull, Type};
        use serde_json::Value;
        if matches!(self.0, Value::Null) {
            return Ok(IsNull::Yes);
        }
        match *ty {
            Type::BOOL => match &self.0 {
                Value::Bool(b) => b.to_sql(ty, out),
                other => text_error(other, ty),
            },
            Type::INT2 => integer(&self.0, ty)?.map_or_else(
                || text_error(&self.0, ty),
                |i| i16::try_from(i).map_err(box_error)?.to_sql(ty, out),
            ),
            Type::INT4 => integer(&self.0, ty)?.map_or_else(
                || text_error(&self.0, ty),
                |i| i32::try_from(i).map_err(box_error)?.to_sql(ty, out),
            ),
            Type::INT8 => integer(&self.0, ty)?.map_or_else(
                || text_error(&self.0, ty),
                |i| i.to_sql(ty, out),
            ),
            Type::FLOAT4 => match self.0.as_f64() {
                Some(f) => (f as f32).to_sql(ty, out),
                None => text_error(&self.0, ty),
            },
            Type::FLOAT8 => match self.0.as_f64() {
                Some(f) => f.to_sql(ty, out),
                None => text_error(&self.0, ty),
            },
            Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME | Type::UNKNOWN => {
                match &self.0 {
                    Value::String(s) => s.as_str().to_sql(ty, out),
                    other => other.to_string().to_sql(ty, out),
                }
            }
            Type::JSON => {
                out.extend_from_slice(serialized(&self.0).as_bytes());
                Ok(IsNull::No)
            }
            Type::JSONB => {
                out.extend_from_slice(&[1]);
                out.extend_from_slice(serialized(&self.0).as_bytes());
                Ok(IsNull::No)
            }
            ref other => Err(format!(
                "a JSON parameter cannot bind to PostgreSQL type `{other}`; bind it as text and                  cast in SQL, e.g. ($1::text)::{other}"
            )
            .into()),
        }
    }

    fn accepts(_: &postgres_types::Type) -> bool {
        true
    }

    postgres_types::to_sql_checked!();
}

fn integer(
    value: &serde_json::Value,
    _ty: &postgres_types::Type,
) -> Result<Option<i64>, Box<dyn std::error::Error + Sync + Send>> {
    Ok(value.as_i64())
}

fn serialized(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn text_error(
    value: &serde_json::Value,
    ty: &postgres_types::Type,
) -> Result<postgres_types::IsNull, Box<dyn std::error::Error + Sync + Send>> {
    Err(format!(
        "JSON parameter {value} cannot bind to PostgreSQL type `{ty}`; cast explicitly in SQL"
    )
    .into())
}

fn box_error<E: std::error::Error + Sync + Send + 'static>(
    error: E,
) -> Box<dyn std::error::Error + Sync + Send> {
    Box::new(error)
}

pub async fn run_sql_as(
    db: &PGlite,
    role: &str,
    claims: Option<&str>,
    headers: Option<&str>,
    validated: &crate::capability::sql_validate::ValidatedSql,
    sql: &str,
    params: &[serde_json::Value],
    max_result_bytes: usize,
) -> Result<SqlOutcome, CacheError> {
    let boxed = to_sql_params(params);
    let refs: Vec<&(dyn pglite::ToSql + Sync)> = boxed
        .iter()
        .map(|param| param.as_ref() as &(dyn pglite::ToSql + Sync))
        .collect();
    let tx = begin_scoped(db, Some(role), claims, headers).await?;
    let outcome = async {
        if validated.command == "SELECT" {
            let wrapped = format!(
                "WITH _pgpaw_q AS ({sql}), \
                 _pgpaw_rows AS ( \
                   SELECT to_jsonb(_pgpaw_q) AS r, \
                          (SELECT count(*) FROM json_object_keys(row_to_json(_pgpaw_q))) AS total, \
                          (SELECT count(*) FROM jsonb_object_keys(to_jsonb(_pgpaw_q))) AS distinct_keys \
                   FROM _pgpaw_q) \
                 SELECT coalesce(jsonb_agg(r), '[]'::jsonb)::text AS j, \
                        coalesce(bool_or(total <> distinct_keys), false) AS duplicated \
                 FROM _pgpaw_rows"
            );
            let rows = tx.query(&wrapped, &refs).await?;
            let (body, duplicated) = match rows.first() {
                Some(row) => (row.get::<Option<String>>(0)?, row.get::<Option<bool>>(1)?),
                None => (None, None),
            };
            if duplicated.unwrap_or(false) {
                return Err(CacheError::Rejected(
                    "result has more than one column with the same name; alias the columns so \
                     each output name is unique"
                        .into(),
                ));
            }
            let rows_json = body.unwrap_or_else(|| "[]".to_string());
            check_result_bound(&rows_json, max_result_bytes)?;
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
            check_result_bound(&rows_json, max_result_bytes)?;
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

fn check_result_bound(rows_json: &str, max_result_bytes: usize) -> Result<(), CacheError> {
    if rows_json.len() > max_result_bytes {
        return Err(CacheError::Rejected(format!(
            "result is {} bytes; the advertised bound is {max_result_bytes} — nothing was \
             committed; narrow the query or paginate with LIMIT/OFFSET",
            rows_json.len()
        )));
    }
    Ok(())
}

fn sql_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn sql_ident(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}
