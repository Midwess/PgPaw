use std::path::PathBuf;
use std::sync::Once;

use clap::{Args, Parser, Subcommand};
use log::{LevelFilter, Log, Metadata, Record};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use pgpaw::{init, run, run_primary, CacheError, PrimaryConfig, ServerConfig, UpstreamConfig};

#[derive(Parser)]
#[command(
    name = "pgpaw",
    version,
    about = "PgPaw serves read-only Postgres queries from a local pglite replica",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[command(flatten)]
    serve: ServeOptions,
}

#[derive(Subcommand)]
enum Command {
    /// Prepare upstream Postgres for PgPaw replication
    Init(InitOptions),
    /// Run the PgPaw HTTP query and realtime server (default)
    Serve(ServeOptions),
    /// Run PgPaw as an embedded writable Postgres primary
    Primary(PrimaryOptions),
}

#[derive(Args, Clone)]
struct ServeOptions {
    /// Host for the PgPaw HTTP API
    #[arg(long, env = "PGPAW_HOST", default_value = "127.0.0.1")]
    host: String,
    /// Port for the PgPaw HTTP API
    #[arg(long, env = "PGPAW_PORT", default_value_t = 8080)]
    port: u16,
    #[cfg(feature = "az-wire")]
    /// Port for the native az-wire host
    #[arg(long, env = "PGPAW_AZ_WIRE_PORT")]
    az_wire_port: Option<u16>,
    /// Data directory for the local pglite replica
    #[arg(long, env = "PGPAW_DATA_DIR", default_value = "./cache-data")]
    data_dir: PathBuf,
    /// Maximum pooled connections to the local replica
    #[arg(long, env = "PGPAW_MAX_CONNECTIONS", default_value_t = 8)]
    max_connections: usize,
    /// Maximum bytes for cached query snapshots
    #[arg(long, env = "PGPAW_CACHE_SIZE_BYTES", default_value_t = 268_435_456)]
    cache_size_bytes: u64,
    #[command(flatten)]
    upstream: UpstreamOptions,
    #[command(flatten)]
    auth: AuthOptions,
    /// Allowed browser origin, comma-separated origins, or "*"
    #[arg(long = "cors-origin", env = "CORS_ORIGIN")]
    cors_origin: Option<String>,
}

#[derive(Args, Clone)]
struct InitOptions {
    #[command(flatten)]
    postgres: PostgresOptions,
}

#[derive(Args, Clone)]
struct PrimaryOptions {
    /// Data directory for embedded Postgres
    #[arg(long, env = "PGPAW_DATA_DIR", default_value = "./cache-data")]
    data_dir: PathBuf,
    /// Maximum pooled connections to embedded Postgres
    #[arg(long, env = "PGPAW_MAX_CONNECTIONS", default_value_t = 8)]
    max_connections: usize,
    /// TCP listen address for embedded Postgres
    #[arg(
        long = "primary-listen",
        env = "PRIMARY_LISTEN",
        default_value = "127.0.0.1"
    )]
    primary_listen: String,
    /// TCP port for embedded Postgres
    #[arg(long = "primary-port", env = "PRIMARY_PORT", default_value_t = 5432)]
    primary_port: u16,
}

#[derive(Args, Clone)]
struct UpstreamOptions {
    #[command(flatten)]
    postgres: PostgresOptions,
    /// Logical replication slot PgPaw should use
    #[arg(long, env = "UPSTREAM_SLOT", default_value = "pgpaw_slot")]
    slot: String,
    /// TLS mode for upstream Postgres: disable | prefer | require | verify-full
    #[arg(long, env = "UPSTREAM_SSLMODE", default_value = "disable")]
    sslmode: String,
}

#[derive(Args, Clone)]
struct PostgresOptions {
    /// Upstream Postgres host
    #[arg(long = "pg-host", env = "UPSTREAM_HOST", default_value = "127.0.0.1")]
    pg_host: String,
    /// Upstream Postgres port
    #[arg(long = "pg-port", env = "UPSTREAM_PORT", default_value_t = 5432)]
    pg_port: u16,
    /// Upstream Postgres user
    #[arg(long = "pg-user", env = "UPSTREAM_USER", default_value = "postgres")]
    pg_user: String,
    /// Upstream Postgres password
    #[arg(long = "pg-password", env = "UPSTREAM_PASSWORD", default_value = "")]
    pg_password: String,
    /// Upstream Postgres database
    #[arg(
        long = "pg-database",
        env = "UPSTREAM_DATABASE",
        default_value = "postgres"
    )]
    pg_database: String,
    /// Logical replication publication PgPaw should read
    #[arg(long, env = "UPSTREAM_PUBLICATION", default_value = "pgpaw_pub")]
    publication: String,
}

#[derive(Args, Clone)]
struct AuthOptions {
    /// Shared secret for HS256 JWT verification
    #[arg(long = "jwt-secret", env = "JWT_SECRET")]
    jwt_secret: Option<String>,
    /// PEM public key for RS256/ES256 JWT verification
    #[arg(long = "jwt-public-key", env = "JWT_PUBLIC_KEY")]
    jwt_public_key: Option<String>,
    /// JWKS endpoint URL for RS256/ES256 JWT verification
    #[arg(long = "jwt-jwks-url", env = "JWT_JWKS_URL")]
    jwt_jwks_url: Option<String>,
    /// JWT claim containing the Postgres role for SET LOCAL ROLE
    #[arg(
        long = "jwt-role-claim",
        env = "JWT_ROLE_CLAIM",
        default_value = "role"
    )]
    jwt_role_claim: String,
}

impl PostgresOptions {
    fn upstream_for_init(&self) -> UpstreamConfig {
        UpstreamConfig {
            host: self.pg_host.clone(),
            port: self.pg_port,
            user: self.pg_user.clone(),
            password: self.pg_password.clone(),
            database: self.pg_database.clone(),
            publication: self.publication.clone(),
            slot: "pgpaw_slot".to_string(),
            sslmode: "disable".to_string(),
        }
    }
}

impl UpstreamOptions {
    fn config(&self) -> UpstreamConfig {
        UpstreamConfig {
            host: self.postgres.pg_host.clone(),
            port: self.postgres.pg_port,
            user: self.postgres.pg_user.clone(),
            password: self.postgres.pg_password.clone(),
            database: self.postgres.pg_database.clone(),
            publication: self.postgres.publication.clone(),
            slot: self.slot.clone(),
            sslmode: self.sslmode.clone(),
        }
    }
}

impl ServeOptions {
    fn config(&self) -> ServerConfig {
        ServerConfig {
            bind_addr: format!("{}:{}", self.host, self.port),
            data_dir: self.data_dir.clone(),
            max_connections: self.max_connections,
            cache_size_bytes: self.cache_size_bytes,
            jwt_secret: self.auth.jwt_secret.clone(),
            jwt_public_key: self.auth.jwt_public_key.clone(),
            jwt_jwks_url: self.auth.jwt_jwks_url.clone(),
            jwt_role_claim: self.auth.jwt_role_claim.clone(),
            cors_origin: self.cors_origin.clone(),
            upstream: self.upstream.config(),
        }
    }
}

impl PrimaryOptions {
    fn config(&self) -> PrimaryConfig {
        PrimaryConfig {
            data_dir: self.data_dir.clone(),
            database: "postgres".into(),
            listen_addresses: self.primary_listen.clone(),
            port: self.primary_port,
            min_connections: 0,
            max_connections: self.max_connections,
        }
    }
}

#[actix_web::main]
async fn main() {
    if let Err(error) = run_cli().await {
        log::error!("event=command_failed error={:?}", error.to_string());
        std::process::exit(1);
    }
}

async fn run_cli() -> Result<(), CacheError> {
    let cli = Cli::parse();
    init_logging();

    match cli.command {
        Some(Command::Init(options)) => {
            log::info!("event=command_start command=init");
            init(options.postgres.upstream_for_init()).await?
        }
        Some(Command::Serve(options)) => {
            log::info!("event=command_start command=serve");
            run(options.config()).await?
        }
        Some(Command::Primary(options)) => {
            log::info!("event=command_start command=primary");
            run_primary(options.config()).await?
        }
        None => {
            log::info!("event=command_start command=serve implicit=true");
            run(cli.serve.config()).await?
        }
    }
    Ok(())
}

static LOGGER: PgpawLogger = PgpawLogger;
static LOG_INIT: Once = Once::new();

struct PgpawLogger;

impl Log for PgpawLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }

        eprintln!(
            "ts={} level={} pid={} target={} {}",
            timestamp(),
            record.level(),
            std::process::id(),
            record.target(),
            record.args()
        );
    }

    fn flush(&self) {}
}

fn init_logging() {
    LOG_INIT.call_once(|| {
        if log::set_logger(&LOGGER).is_ok() {
            log::set_max_level(LevelFilter::Info);
        }
    });
    log::info!("event=logger_ready min_level=INFO format=logfmt");
}

fn timestamp() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

#[cfg(all(test, feature = "az-wire"))]
mod tests {
    use super::{Cli, Command};
    use clap::Parser;

    #[test]
    fn az_wire_port_is_optional_for_implicit_and_explicit_serve() {
        let implicit = Cli::try_parse_from(["pgpaw"]).unwrap();
        assert_eq!(implicit.serve.az_wire_port, None);

        let explicit = Cli::try_parse_from(["pgpaw", "serve"]).unwrap();
        let Some(Command::Serve(explicit)) = explicit.command else {
            panic!("expected serve command");
        };
        assert_eq!(explicit.az_wire_port, None);
    }

    #[test]
    fn az_wire_port_parses_for_implicit_and_explicit_serve() {
        let implicit = Cli::try_parse_from(["pgpaw", "--az-wire-port", "9000"]).unwrap();
        assert_eq!(implicit.serve.az_wire_port, Some(9000));

        let explicit = Cli::try_parse_from(["pgpaw", "serve", "--az-wire-port", "9001"]).unwrap();
        let Some(Command::Serve(explicit)) = explicit.command else {
            panic!("expected serve command");
        };
        assert_eq!(explicit.az_wire_port, Some(9001));
    }
}
