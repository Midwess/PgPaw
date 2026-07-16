use std::io::{self, Write};

use tokio_postgres::{Client, Config, NoTls};

use crate::api::config::UpstreamConfig;
use crate::error::CacheError;

pub async fn prepare(upstream: &UpstreamConfig, publication: &str) -> Result<(), CacheError> {
    if !is_identifier(publication) {
        return Err(CacheError::Config(format!(
            "Invalid publication name `{publication}`. Use only letters, digits, and underscore."
        )));
    }

    log::info!(
        "event=init_start upstream_host={} upstream_port={} upstream_user={} upstream_database={} publication={}",
        upstream.host,
        upstream.port,
        upstream.user,
        upstream.database,
        publication,
    );
    println!("PgPaw init");
    println!(
        "Upstream Postgres: {}@{}:{}/{}",
        upstream.user, upstream.host, upstream.port, upstream.database
    );
    println!();
    println!("PgPaw will prepare logical replication on this database:");
    println!("  1. Ensure wal_level=logical and max_wal_senders/max_replication_slots >= 10.");
    println!("     These settings require a Postgres restart if PgPaw changes them.");
    println!("  2. Ensure publication \"{publication}\" exists for all tables.");
    println!("  3. Install a DDL event trigger so PgPaw notices schema changes while running.");
    println!("PgPaw creates the replication slot automatically on the first `pgpaw serve` start.");
    print!("Apply changes? [y/N] ");
    io::stdout().flush().ok();

    if !confirm() {
        log::info!("event=init_aborted result=no_changes");
        println!("No changes made.");
        return Ok(());
    }

    log::info!("event=init_confirmed");
    let client = connect(upstream).await?;
    log::info!("event=init_connected");
    apply(&client, publication).await
}

pub async fn preflight(upstream: &UpstreamConfig, publication: &str) -> Result<(), CacheError> {
    let client = connect(upstream).await.map_err(|e| {
        CacheError::Config(format!(
            "Could not connect to upstream Postgres at {}:{} as {} ({e}). Check --pg-host, --pg-port, --pg-user, and --pg-password.",
            upstream.host, upstream.port, upstream.user
        ))
    })?;

    let wal_level = setting(&client, "wal_level").await?;
    if wal_level != "logical" {
        return Err(CacheError::Config(format!(
            "Upstream Postgres has wal_level='{wal_level}', but PgPaw requires 'logical'. Run `pgpaw init`; if it changes WAL settings, restart Postgres before `pgpaw serve`."
        )));
    }

    for param in ["max_wal_senders", "max_replication_slots"] {
        let value: i64 = setting(&client, param).await?.parse().unwrap_or(0);
        if value < 1 {
            return Err(CacheError::Config(format!(
                "Upstream Postgres has {param}={value}, but PgPaw requires at least 1. Run `pgpaw init`; if it changes WAL settings, restart Postgres before `pgpaw serve`."
            )));
        }
    }

    let exists: bool = client
        .query_one(
            "select exists(select 1 from pg_publication where pubname = $1)",
            &[&publication],
        )
        .await?
        .get(0);
    if !exists {
        return Err(CacheError::Config(format!(
            "Publication \"{publication}\" does not exist on upstream Postgres. Run `pgpaw init` to create it, or pass --publication with an existing publication."
        )));
    }

    Ok(())
}

async fn connect(upstream: &UpstreamConfig) -> Result<Client, CacheError> {
    let (client, connection) = Config::new()
        .host(&upstream.host)
        .port(upstream.port)
        .user(&upstream.user)
        .password(&upstream.password)
        .dbname(&upstream.database)
        .connect(NoTls)
        .await?;
    tokio::spawn(connection);
    Ok(client)
}

async fn apply(client: &Client, publication: &str) -> Result<(), CacheError> {
    let mut needs_restart = false;
    let mut needs_manual_settings = false;

    let wal_level = setting(client, "wal_level").await?;
    if wal_level == "logical" {
        log::info!("event=init_setting_ok setting=wal_level value=logical");
        println!("  ✓ wal_level is already logical");
    } else if alter_system(client, "wal_level", "logical").await {
        log::info!(
            "event=init_setting_changed setting=wal_level old_value={:?} new_value=logical restart_required=true",
            wal_level,
        );
        println!("  ✓ wal_level set to logical (was '{wal_level}')");
        needs_restart = true;
    } else {
        needs_manual_settings = true;
    }

    for param in ["max_wal_senders", "max_replication_slots"] {
        let current: i64 = setting(client, param).await?.parse().unwrap_or(0);
        if current < 10 && alter_system(client, param, "10").await {
            log::info!(
                "event=init_setting_changed setting={} old_value={} new_value=10 restart_required=true",
                param,
                current,
            );
            println!("  ✓ {param} set to 10 (was {current})");
            needs_restart = true;
        } else if current < 10 {
            log::warn!(
                "event=init_setting_manual_required setting={} current_value={} required_min=10",
                param,
                current,
            );
            needs_manual_settings = true;
        } else {
            log::info!(
                "event=init_setting_ok setting={} value={} required_min=10",
                param,
                current,
            );
        }
    }

    let exists: bool = client
        .query_one(
            "select exists(select 1 from pg_publication where pubname = $1)",
            &[&publication],
        )
        .await?
        .get(0);
    if exists {
        log::info!(
            "event=init_publication_ok publication={} existed=true",
            publication
        );
        println!("  ✓ publication \"{publication}\" already exists");
    } else {
        client
            .batch_execute(&format!(
                "CREATE PUBLICATION \"{publication}\" FOR ALL TABLES"
            ))
            .await?;
        log::info!(
            "event=init_publication_created publication={} all_tables=true",
            publication
        );
        println!("  ✓ publication \"{publication}\" created for all tables");
    }

    match install_ddl_trigger(client).await {
        Ok(()) => {
            log::info!("event=init_ddl_trigger_installed result=ok");
            println!("  ✓ DDL event trigger installed")
        }
        Err(error) => {
            log::warn!(
                "event=init_ddl_trigger_skipped result=error error={:?}",
                error.to_string()
            );
            println!(
                "  ⚠ DDL event trigger not installed ({error}). PgPaw can still recover from schema changes, but online detection requires a superuser."
            )
        }
    }

    if needs_manual_settings {
        log::warn!("event=init_complete result=manual_wal_settings_required");
        println!();
        println!("PgPaw init finished with manual WAL settings still required.");
        println!(
            "Apply the ALTER SYSTEM statements above, restart Postgres, then run `pgpaw serve`."
        );
    } else if needs_restart {
        log::warn!("event=init_complete result=restart_required");
        println!();
        println!("⚠ Restart Postgres before running `pgpaw serve`; WAL settings are restart-only:");
        println!("    docker compose restart <postgres-service>");
        println!("    # or: pg_ctl restart");
        println!("PgPaw init complete. After Postgres restarts, run `pgpaw serve`.");
    } else {
        log::info!("event=init_complete result=ready");
        println!("PgPaw init complete. Run `pgpaw serve` to start PgPaw.");
    }
    Ok(())
}

async fn install_ddl_trigger(client: &Client) -> Result<(), CacheError> {
    let prefix = pglite::DDL_SIGNAL_PREFIX;
    client
        .batch_execute(&format!(
            "CREATE OR REPLACE FUNCTION pglite_emit_ddl() RETURNS event_trigger \
             LANGUAGE plpgsql AS $fn$ BEGIN \
               PERFORM pg_logical_emit_message(true, '{prefix}', ''); \
             END $fn$; \
             DROP EVENT TRIGGER IF EXISTS pglite_ddl_watch; \
             CREATE EVENT TRIGGER pglite_ddl_watch ON ddl_command_end \
               EXECUTE FUNCTION pglite_emit_ddl();"
        ))
        .await?;
    Ok(())
}

async fn setting(client: &Client, name: &str) -> Result<String, CacheError> {
    Ok(client
        .query_one("select current_setting($1)", &[&name])
        .await?
        .get(0))
}

async fn alter_system(client: &Client, param: &str, value: &str) -> bool {
    match client
        .batch_execute(&format!("ALTER SYSTEM SET {param} = '{value}'"))
        .await
    {
        Ok(_) => true,
        Err(error) => {
            log::warn!(
                "event=init_setting_change_failed setting={} requested_value={} error={:?}",
                param,
                value,
                error.to_string(),
            );
            println!("  ⚠ {param} was not changed ({error}). Set it manually with a superuser:");
            println!("    ALTER SYSTEM SET {param} = '{value}';");
            false
        }
    }
}

fn confirm() -> bool {
    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_err() {
        return false;
    }
    matches!(input.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

fn is_identifier(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}
