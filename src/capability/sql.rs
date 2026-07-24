use pglite::PGlite;

use crate::capability::auth::{Principal, Verifier};
use crate::capability::read::map_db_denial;
use crate::capability::rows;
use crate::capability::sql_validate;
use crate::error::CacheError;

pub const MAX_SQL_BYTES: usize = 1024 * 1024;
pub const MAX_RESULT_BYTES: usize = 8 * 1024 * 1024;
pub const SQL_DEADLINE_SECS: u64 = 30;
pub const DEFAULT_PUBLIC_ROLE: &str = "pgpaw_public";

#[derive(Clone)]
pub struct SqlOperations {
    db: PGlite,
    verifier: Option<Verifier>,
    public_role: String,
    read_only: bool,
}

pub struct SqlOutcome {
    pub command: String,
    pub rows_json: String,
    pub rows_affected: u64,
}

impl SqlOperations {
    pub(crate) fn new(
        db: PGlite,
        verifier: Option<Verifier>,
        public_role: Option<String>,
        read_only: bool,
    ) -> SqlOperations {
        SqlOperations {
            db,
            verifier,
            public_role: public_role.unwrap_or_else(|| DEFAULT_PUBLIC_ROLE.to_string()),
            read_only,
        }
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

    pub async fn execute(
        &self,
        sql: &str,
        params: &[serde_json::Value],
        principal: Option<Principal>,
        headers: Option<&str>,
    ) -> Result<SqlOutcome, CacheError> {
        let validated = sql_validate::validate_one_statement(sql, MAX_SQL_BYTES)?;
        let (role, claims) = match &principal {
            Some(principal) => (principal.role.clone(), Some(principal.claims_json.as_str())),
            None => (self.public_role.clone(), None),
        };
        self.deny_owner_roles(&role).await?;
        if self.read_only && validated.command != "SELECT" {
            return Err(CacheError::Rejected(
                "this node serves a read replica; only SELECT may execute here — send writes \
                 to the primary data node"
                    .to_string(),
            ));
        }
        let outcome = rows::run_sql_as(&self.db, &role, claims, headers, &validated, sql, params)
            .await
            .map_err(map_db_denial)?;
        if outcome.rows_json.len() > MAX_RESULT_BYTES {
            if validated.command == "SELECT" {
                return Err(CacheError::Rejected(format!(
                    "result is {} bytes; the advertised bound is {MAX_RESULT_BYTES} — narrow the \
                     query or paginate with LIMIT/OFFSET",
                    outcome.rows_json.len()
                )));
            }
            return Err(CacheError::Rejected(format!(
                "the statement committed, but its RETURNING payload is {} bytes over the \
                 {MAX_RESULT_BYTES}-byte bound — do not retry the write; refetch the rows with \
                 a narrower SELECT",
                outcome.rows_json.len() - MAX_RESULT_BYTES
            )));
        }
        Ok(SqlOutcome {
            command: validated.command,
            rows_json: outcome.rows_json,
            rows_affected: outcome.rows_affected,
        })
    }

    async fn deny_owner_roles(&self, role: &str) -> Result<(), CacheError> {
        if role == "postgres" {
            return Err(CacheError::Forbidden(
                "public SQL never executes as the engine owner".to_string(),
            ));
        }
        let rows = self
            .db
            .query(
                "SELECT rolsuper::int FROM pg_roles WHERE rolname = $1",
                &[&role],
            )
            .await?;
        if let Some(row) = rows.first() {
            let superuser: i32 = row.get(0)?;
            if superuser == 1 {
                return Err(CacheError::Forbidden(
                    "public SQL never executes as a superuser role".to_string(),
                ));
            }
        }
        Ok(())
    }
}
