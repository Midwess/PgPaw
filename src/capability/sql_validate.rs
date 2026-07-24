use sqlparser::ast::{Expr, SelectItem, Statement};
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::Parser;

use crate::error::CacheError;

#[derive(Debug)]
pub struct ValidatedSql {
    pub command: String,
    pub returns_rows: bool,
    pub needs_count_wrap: bool,
}

pub fn validate_one_statement(sql: &str, max_sql_bytes: usize) -> Result<ValidatedSql, CacheError> {
    if sql.len() > max_sql_bytes {
        return Err(CacheError::Rejected(format!(
            "statement is {} bytes; the advertised bound is {max_sql_bytes}",
            sql.len()
        )));
    }
    let statements = Parser::parse_sql(&PostgreSqlDialect {}, sql).map_err(|error| {
        CacheError::Rejected(format!(
            "pgpaw.sql accepts one parseable PostgreSQL statement: {error}"
        ))
    })?;
    if statements.len() != 1 {
        return Err(CacheError::Rejected(format!(
            "pgpaw.sql executes exactly one statement per request; got {}",
            statements.len()
        )));
    }
    let statement = &statements[0];
    if let Some(rejected) = connection_scoped(statement) {
        return Err(CacheError::Rejected(format!(
            "{rejected} is connection-scoped and unsupported: each pgpaw.sql request commits or \
             rolls back independently and no connection state survives the call"
        )));
    }
    check_returning_duplicates(statement)?;
    Ok(shape(statement, sql))
}

fn connection_scoped(statement: &Statement) -> Option<&'static str> {
    match statement {
        Statement::StartTransaction { .. } => Some("BEGIN"),
        Statement::Commit { .. } => Some("COMMIT"),
        Statement::Rollback { .. } => Some("ROLLBACK"),
        Statement::Savepoint { .. } => Some("SAVEPOINT"),
        Statement::ReleaseSavepoint { .. } => Some("RELEASE SAVEPOINT"),
        Statement::SetTransaction { .. } => Some("SET TRANSACTION"),
        Statement::SetVariable { .. } => Some("SET"),
        Statement::SetTimeZone { .. } => Some("SET TIME ZONE"),
        Statement::SetNames { .. } | Statement::SetNamesDefault {} => Some("SET NAMES"),
        Statement::SetRole { .. } => Some("SET ROLE"),
        Statement::LISTEN { .. } => Some("LISTEN"),
        
        Statement::NOTIFY { .. } => Some("NOTIFY"),
        Statement::Discard { .. } => Some("DISCARD"),
        Statement::Deallocate { .. } => Some("DEALLOCATE"),
        Statement::Prepare { .. } => Some("PREPARE"),
        Statement::Execute { .. } => Some("EXECUTE"),
        Statement::Copy { .. } => Some("COPY"),
        _ => None,
    }
}

fn shape(statement: &Statement, sql: &str) -> ValidatedSql {
    match statement {
        Statement::Query(_) => ValidatedSql {
            command: "SELECT".into(),
            returns_rows: true,
            needs_count_wrap: false,
        },
        Statement::Insert(insert) => dml("INSERT", insert.returning.is_some()),
        Statement::Update { returning, .. } => dml("UPDATE", returning.is_some()),
        Statement::Delete(delete) => dml("DELETE", delete.returning.is_some()),
        Statement::CreateTable(_) => ddl("CREATE TABLE"),
        Statement::AlterTable { .. } => ddl("ALTER TABLE"),
        Statement::CreateIndex(_) => ddl("CREATE INDEX"),
        Statement::CreateView { .. } => ddl("CREATE VIEW"),
        Statement::CreateSchema { .. } => ddl("CREATE SCHEMA"),
        Statement::CreateFunction { .. } => ddl("CREATE FUNCTION"),
        Statement::Truncate { .. } => ddl("TRUNCATE TABLE"),
        Statement::Drop { object_type, .. } => ValidatedSql {
            command: format!("DROP {object_type}").to_uppercase(),
            returns_rows: false,
            needs_count_wrap: false,
        },
        _ => ValidatedSql {
            command: first_words(sql),
            returns_rows: false,
            needs_count_wrap: false,
        },
    }
}

fn check_returning_duplicates(statement: &Statement) -> Result<(), CacheError> {
    let returning = match statement {
        Statement::Insert(insert) => insert.returning.as_ref(),
        Statement::Update { returning, .. } => returning.as_ref(),
        Statement::Delete(delete) => delete.returning.as_ref(),
        _ => None,
    };
    let Some(items) = returning else {
        return Ok(());
    };
    let wildcards = items
        .iter()
        .filter(|item| {
            matches!(
                item,
                SelectItem::Wildcard(_) | SelectItem::QualifiedWildcard(_, _)
            )
        })
        .count();
    if wildcards > 1 || (wildcards == 1 && items.len() > 1) {
        return Err(CacheError::Rejected(
            "RETURNING mixes a wildcard with other output columns; alias every column \
             explicitly so each output name is unique"
                .into(),
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for item in items {
        let name = match item {
            SelectItem::ExprWithAlias { alias, .. } => alias.value.to_ascii_lowercase(),
            SelectItem::UnnamedExpr(Expr::Identifier(ident)) => ident.value.to_ascii_lowercase(),
            SelectItem::UnnamedExpr(Expr::CompoundIdentifier(parts)) => parts
                .last()
                .map(|ident| ident.value.to_ascii_lowercase())
                .unwrap_or_default(),
            _ => continue,
        };
        if !seen.insert(name.clone()) {
            return Err(CacheError::Rejected(format!(
                "RETURNING has more than one output column named `{name}`; alias the columns \
                 so each output name is unique"
            )));
        }
    }
    Ok(())
}

fn dml(command: &str, has_returning: bool) -> ValidatedSql {
    ValidatedSql {
        command: command.into(),
        returns_rows: has_returning,
        needs_count_wrap: !has_returning,
    }
}

fn ddl(command: &str) -> ValidatedSql {
    ValidatedSql {
        command: command.into(),
        returns_rows: false,
        needs_count_wrap: false,
    }
}

fn first_words(sql: &str) -> String {
    sql.split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ")
        .to_uppercase()
}

pub fn count_wrapped(sql: &str) -> String {
    let trimmed = sql.trim_end().trim_end_matches(';');
    format!(
        "WITH _pgpaw_dml AS ({trimmed} RETURNING 1) \
         SELECT count(*)::int8 AS _pgpaw_affected FROM _pgpaw_dml"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_shapes_as_row_returning() {
        let v = validate_one_statement("SELECT 1 AS one", 1024).unwrap();
        assert_eq!(v.command, "SELECT");
        assert!(v.returns_rows);
        assert!(!v.needs_count_wrap);
    }

    #[test]
    fn dml_without_returning_needs_the_count_wrap() {
        let v = validate_one_statement("INSERT INTO t (a) VALUES (1)", 1024).unwrap();
        assert_eq!(v.command, "INSERT");
        assert!(!v.returns_rows);
        assert!(v.needs_count_wrap);
        assert_eq!(
            count_wrapped("INSERT INTO t (a) VALUES (1);"),
            "WITH _pgpaw_dml AS (INSERT INTO t (a) VALUES (1) RETURNING 1) \
             SELECT count(*)::int8 AS _pgpaw_affected FROM _pgpaw_dml"
        );
    }

    #[test]
    fn dml_with_returning_returns_rows_directly() {
        let v = validate_one_statement(
            "UPDATE t SET a = 2 WHERE id = $1 RETURNING id, a",
            1024,
        )
        .unwrap();
        assert_eq!(v.command, "UPDATE");
        assert!(v.returns_rows);
        assert!(!v.needs_count_wrap);
    }

    #[test]
    fn ddl_is_permitted_with_its_tag() {
        let v = validate_one_statement("CREATE TABLE t (id int primary key)", 1024).unwrap();
        assert_eq!(v.command, "CREATE TABLE");
        assert!(!v.returns_rows);
        let v = validate_one_statement("DROP TABLE t", 1024).unwrap();
        assert_eq!(v.command, "DROP TABLE");
    }

    #[test]
    fn multi_statement_is_rejected() {
        let error = validate_one_statement("SELECT 1; SELECT 2", 1024).unwrap_err();
        assert!(error.to_string().contains("exactly one statement"));
    }

    #[test]
    fn connection_scoped_commands_are_rejected() {
        for sql in [
            "BEGIN",
            "COMMIT",
            "ROLLBACK",
            "SAVEPOINT s1",
            "SET search_path TO public",
            "SET ROLE admin",
            "LISTEN updates",
            "NOTIFY updates, 'x'",
            "DISCARD ALL",
            "PREPARE q AS SELECT 1",
        ] {
            let error = validate_one_statement(sql, 1024).unwrap_err();
            assert!(
                error.to_string().contains("connection-scoped"),
                "{sql} must be rejected as connection-scoped, got: {error}"
            );
        }
    }

    #[test]
    fn oversized_statement_is_rejected_before_parse() {
        let sql = format!("SELECT '{}'", "x".repeat(64));
        let error = validate_one_statement(&sql, 32).unwrap_err();
        assert!(error.to_string().contains("bytes"));
    }

    #[test]
    fn duplicate_returning_names_are_rejected() {
        let error = validate_one_statement(
            "UPDATE t SET a = 1 WHERE id = 1 RETURNING id, b AS id",
            1024,
        )
        .unwrap_err();
        assert!(error.to_string().contains("more than one output column named `id`"));

        let error = validate_one_statement(
            "DELETE FROM t WHERE id = 1 RETURNING *, id",
            1024,
        )
        .unwrap_err();
        assert!(error.to_string().contains("wildcard"));

        validate_one_statement("DELETE FROM t WHERE id = 1 RETURNING *", 1024).unwrap();
    }

    #[test]
    fn unparseable_sql_is_rejected_explicitly() {
        let error = validate_one_statement("FLY ME TO THE MOON", 1024).unwrap_err();
        assert!(error.to_string().contains("one parseable PostgreSQL statement"));
    }
}
