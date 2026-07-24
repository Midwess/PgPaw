use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Once;

use clap::{Args, Parser, Subcommand};
use log::{LevelFilter, Log, Metadata, Record};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use pgpaw::{
    init, AuthConfig, CacheConfig, CacheError, EmbeddedPrimarySource, HttpConfig, PgPaw,
    PgPawBuilder, PgSource, ReplicaSource, UpstreamConfig,
};

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
    Serve(Box<ServeOptions>),
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
    #[cfg(feature = "az-wire")]
    /// Host for the native az-wire listener
    #[arg(long, env = "PGPAW_AZ_WIRE_HOST", default_value = "127.0.0.1")]
    az_wire_host: std::net::IpAddr,
    #[cfg(feature = "az-wire")]
    /// Node identity for native az-wire
    #[arg(long, env = "PGPAW_AZ_WIRE_NODE", default_value = "pgpaw")]
    az_wire_node: String,
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
    /// Database served by embedded Postgres
    #[arg(long, env = "PGPAW_DATABASE", default_value = "postgres")]
    database: String,
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
    #[cfg(feature = "az-wire")]
    /// Node identity for native az-wire
    #[arg(long, env = "PGPAW_AZ_WIRE_NODE", default_value = "pgpaw")]
    az_wire_node: String,
    #[cfg(feature = "az-wire")]
    /// Parent node identity for the az-wire child link
    #[arg(long = "az-wire-parent-node", env = "PGPAW_AZ_WIRE_PARENT_NODE")]
    az_wire_parent_node: Option<String>,
    #[cfg(feature = "az-wire")]
    /// Unix socket path of the az-wire parent
    #[arg(long = "az-wire-parent-unix", env = "PGPAW_AZ_WIRE_PARENT_UNIX")]
    az_wire_parent_unix: Option<PathBuf>,
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
    fn upstream(&self) -> UpstreamConfig {
        UpstreamConfig {
            host: self.pg_host.clone(),
            port: self.pg_port,
            user: self.pg_user.clone(),
            password: self.pg_password.clone(),
            database: self.pg_database.clone(),
            sslmode: "disable".to_string(),
        }
    }
}

impl AuthOptions {
    fn config(&self) -> AuthConfig {
        AuthConfig {
            jwt_secret: self.jwt_secret.clone(),
            jwt_public_key: self.jwt_public_key.clone(),
            jwt_jwks_url: self.jwt_jwks_url.clone(),
            role_claim: Some(self.jwt_role_claim.clone()),
        }
    }
}

impl ServeOptions {
    fn addr(&self) -> Result<SocketAddr, CacheError> {
        use std::net::ToSocketAddrs;
        format!("{}:{}", self.host, self.port)
            .to_socket_addrs()
            .map_err(CacheError::Io)?
            .next()
            .ok_or_else(|| {
                CacheError::Config(format!("could not resolve {}:{}", self.host, self.port))
            })
    }

    #[cfg(feature = "az-wire")]
    fn az_wire_addr(&self) -> Option<SocketAddr> {
        self.az_wire_port
            .map(|port| SocketAddr::new(self.az_wire_host, port))
    }

    fn source(&self) -> ReplicaSource {
        ReplicaSource {
            upstream: UpstreamConfig {
                host: self.upstream.postgres.pg_host.clone(),
                port: self.upstream.postgres.pg_port,
                user: self.upstream.postgres.pg_user.clone(),
                password: self.upstream.postgres.pg_password.clone(),
                database: self.upstream.postgres.pg_database.clone(),
                sslmode: self.upstream.sslmode.clone(),
            },
            data_dir: self.data_dir.clone(),
            publication: self.upstream.postgres.publication.clone(),
            slot: self.upstream.slot.clone(),
            max_connections: self.max_connections,
        }
    }

    fn builder(&self) -> Result<PgPawBuilder, CacheError> {
        let builder = PgPaw::builder()
            .source(PgSource::replica(self.source()))
            .cache(CacheConfig {
                max_bytes: self.cache_size_bytes,
            })
            .auth(self.auth.config())
            .http(HttpConfig {
                addr: self.addr()?,
                cors_origin: self.cors_origin.clone(),
            });
        #[cfg(feature = "az-wire")]
        let builder = match self.az_wire_addr() {
            Some(addr) => builder.az_wire(
                az_wire::NodeBuilder::new(&self.az_wire_node)
                    .insecure_accept_declared_peer_identities(),
                az_wire::TopologyConfig::host(az_wire::HostConfig::tcp(
                    addr,
                    az_wire::TcpTransport::plain(),
                )),
            ),
            None => builder,
        };
        Ok(builder)
    }
}

impl PrimaryOptions {
    fn builder(&self) -> PgPawBuilder {
        let builder = PgPaw::builder().source(PgSource::primary(EmbeddedPrimarySource {
            data_dir: self.data_dir.clone(),
            database: self.database.clone(),
            listen_addresses: self.primary_listen.clone(),
            port: self.primary_port,
            min_connections: 0,
            max_connections: self.max_connections,
        }));
        #[cfg(feature = "az-wire")]
        let builder = match (&self.az_wire_parent_node, &self.az_wire_parent_unix) {
            (Some(parent_node), Some(parent_unix)) => builder.az_wire(
                az_wire::NodeBuilder::new(&self.az_wire_node)
                    .insecure_accept_declared_peer_identities(),
                az_wire::TopologyConfig::parent(az_wire::ParentLink::unix(
                    parent_node,
                    parent_unix,
                )),
            ),
            _ => builder,
        };
        builder
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
            init(options.postgres.upstream(), &options.postgres.publication).await?
        }
        Some(Command::Serve(options)) => {
            log::info!("event=command_start command=serve");
            run_pgpaw(options.builder()?).await?
        }
        Some(Command::Primary(options)) => {
            log::info!("event=command_start command=primary");
            run_pgpaw(options.builder()).await?
        }
        None => {
            log::info!("event=command_start command=serve implicit=true");
            run_pgpaw(cli.serve.builder()?).await?
        }
    }
    Ok(())
}

async fn run_pgpaw(builder: PgPawBuilder) -> Result<(), CacheError> {
    let mut pgpaw = builder.open().await?;
    if let Some(dsn) = pgpaw.primary_dsn() {
        log::info!("event=primary_ready dsn={}", dsn);
    }
    let result = tokio::select! {
        biased;
        result = shutdown_signal() => result,
        result = pgpaw.wait() => result,
    };
    match &result {
        Ok(()) => log::info!("event=server_stopped result=ok"),
        Err(error) => log::error!(
            "event=server_stopped result=error error={:?}",
            error.to_string()
        ),
    }
    log::info!("event=server_shutdown_start");
    pgpaw.shutdown().await?;
    log::info!("event=server_shutdown_complete");
    result
}

#[cfg(unix)]
async fn shutdown_signal() -> Result<(), CacheError> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(CacheError::Io)?;
    tokio::select! {
        biased;
        result = tokio::signal::ctrl_c() => result.map_err(CacheError::Io),
        _ = terminate.recv() => Ok(()),
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() -> Result<(), CacheError> {
    tokio::signal::ctrl_c().await.map_err(CacheError::Io)
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

#[cfg(test)]
mod tests {
    use super::{Cli, Command};
    use clap::Parser;
    #[cfg(unix)]
    use std::process::Command as ProcessCommand;
    #[cfg(unix)]
    use std::time::Duration;

    #[cfg(feature = "az-wire")]
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

    #[cfg(feature = "az-wire")]
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

    #[cfg(feature = "az-wire")]
    #[test]
    fn az_wire_host_and_node_parse_with_minimal_defaults() {
        let defaults = Cli::try_parse_from(["pgpaw"]).unwrap().serve;
        assert_eq!(defaults.az_wire_host.to_string(), "127.0.0.1");
        assert_eq!(defaults.az_wire_node, "pgpaw");

        let configured = Cli::try_parse_from([
            "pgpaw",
            "--az-wire-port",
            "9000",
            "--az-wire-host",
            "0.0.0.0",
            "--az-wire-node",
            "cache",
        ])
        .unwrap()
        .serve;
        assert_eq!(configured.az_wire_host.to_string(), "0.0.0.0");
        assert_eq!(configured.az_wire_node, "cache");
    }

    #[cfg(feature = "az-wire")]
    #[test]
    fn az_wire_host_requires_an_ip_literal() {
        assert!(Cli::try_parse_from([
            "pgpaw",
            "--az-wire-port",
            "9000",
            "--az-wire-host",
            "localhost",
        ])
        .is_err());
    }

    #[cfg(feature = "az-wire")]
    #[test]
    fn serve_is_http_only_without_az_wire_port() {
        let options = Cli::try_parse_from(["pgpaw"]).unwrap().serve;
        assert_eq!(options.addr().unwrap().to_string(), "127.0.0.1:8080");
        assert_eq!(options.az_wire_addr(), None);
    }

    #[cfg(feature = "az-wire")]
    #[test]
    fn serve_keeps_http_and_az_wire_addresses_independent() {
        let options = Cli::try_parse_from([
            "pgpaw",
            "--host",
            "127.0.0.2",
            "--port",
            "8081",
            "--az-wire-host",
            "127.0.0.3",
            "--az-wire-port",
            "9000",
        ])
        .unwrap()
        .serve;
        assert_eq!(options.addr().unwrap().to_string(), "127.0.0.2:8081");
        assert_eq!(
            options.az_wire_addr().unwrap().to_string(),
            "127.0.0.3:9000"
        );
    }

    #[cfg(feature = "az-wire")]
    #[test]
    fn primary_parses_database_and_parent_link_flags() {
        let parsed = Cli::try_parse_from([
            "pgpaw",
            "primary",
            "--database",
            "app",
            "--az-wire-node",
            "pgpaw",
            "--az-wire-parent-node",
            "worldant",
            "--az-wire-parent-unix",
            "/tmp/worldant.sock",
        ])
        .unwrap();
        let Some(Command::Primary(options)) = parsed.command else {
            panic!("expected primary command");
        };
        assert_eq!(options.database, "app");
        assert_eq!(options.az_wire_node, "pgpaw");
        assert_eq!(options.az_wire_parent_node.as_deref(), Some("worldant"));
        assert_eq!(
            options.az_wire_parent_unix.as_deref(),
            Some(std::path::Path::new("/tmp/worldant.sock"))
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_signal_helper() {
        if std::env::var_os("PGPAW_SIGNAL_HELPER").is_some() {
            super::shutdown_signal().await.unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn sigterm_completes_the_production_signal_wait() {
        let mut child = ProcessCommand::new(std::env::current_exe().unwrap())
            .args(["--exact", "tests::shutdown_signal_helper", "--nocapture"])
            .env("PGPAW_SIGNAL_HELPER", "1")
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(200));
        let sent = unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
        assert_eq!(sent, 0);
        assert!(child.wait().unwrap().success());
    }
}
