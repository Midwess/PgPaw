use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use pgpaw::{init, run, CacheError, ServerConfig, UpstreamConfig};

#[derive(Parser)]
#[command(
    name = "cache-server",
    version,
    about = "Read-only Postgres cache + realtime server over a pglite replica"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[command(flatten)]
    args: Options,
}

#[derive(Subcommand)]
enum Command {
    /// Interactively prepare the upstream Postgres (create publication, checks)
    Init,
    /// Run the cache server (default)
    Serve,
}

#[derive(Args)]
struct Options {
    /// HTTP bind host
    #[arg(long, env = "CACHE_HOST", global = true, default_value = "127.0.0.1")]
    host: String,
    /// HTTP bind port
    #[arg(long, env = "CACHE_PORT", global = true, default_value_t = 8080)]
    port: u16,
    /// Replica data directory
    #[arg(
        long,
        env = "CACHE_DATA_DIR",
        global = true,
        default_value = "./cache-data"
    )]
    data_dir: PathBuf,
    /// Max pooled connections to the embedded replica
    #[arg(
        long,
        env = "CACHE_MAX_CONNECTIONS",
        global = true,
        default_value_t = 8
    )]
    max_connections: usize,
    /// Result-cache byte budget
    #[arg(
        long,
        env = "CACHE_SIZE_BYTES",
        global = true,
        default_value_t = 268_435_456
    )]
    cache_size_bytes: u64,
    /// Upstream Postgres host
    #[arg(
        long = "pg-host",
        env = "UPSTREAM_HOST",
        global = true,
        default_value = "127.0.0.1"
    )]
    pg_host: String,
    /// Upstream Postgres port
    #[arg(
        long = "pg-port",
        env = "UPSTREAM_PORT",
        global = true,
        default_value_t = 5432
    )]
    pg_port: u16,
    /// Upstream Postgres user
    #[arg(
        long = "pg-user",
        env = "UPSTREAM_USER",
        global = true,
        default_value = "postgres"
    )]
    pg_user: String,
    /// Upstream Postgres password
    #[arg(
        long = "pg-password",
        env = "UPSTREAM_PASSWORD",
        global = true,
        default_value = ""
    )]
    pg_password: String,
    /// Upstream Postgres database
    #[arg(
        long = "pg-database",
        env = "UPSTREAM_DATABASE",
        global = true,
        default_value = "postgres"
    )]
    pg_database: String,
    /// Upstream publication to replicate
    #[arg(
        long,
        env = "UPSTREAM_PUBLICATION",
        global = true,
        default_value = "cache_server_pub"
    )]
    publication: String,
    /// Logical replication slot name
    #[arg(
        long,
        env = "UPSTREAM_SLOT",
        global = true,
        default_value = "cache_server_slot"
    )]
    slot: String,
    /// TLS mode: disable | prefer | require | verify-full
    #[arg(
        long,
        env = "UPSTREAM_SSLMODE",
        global = true,
        default_value = "disable"
    )]
    sslmode: String,
    /// JWT HS256 shared secret (one-of with --jwt-public-key / --jwt-jwks-url)
    #[arg(long = "jwt-secret", env = "JWT_SECRET", global = true)]
    jwt_secret: Option<String>,
    /// JWT RS256/ES256 verification public key, PEM (one-of)
    #[arg(long = "jwt-public-key", env = "JWT_PUBLIC_KEY", global = true)]
    jwt_public_key: Option<String>,
    /// JWKS endpoint URL for RS256/ES256 verification (one-of)
    #[arg(long = "jwt-jwks-url", env = "JWT_JWKS_URL", global = true)]
    jwt_jwks_url: Option<String>,
    /// JWT claim naming the Postgres role to SET LOCAL ROLE
    #[arg(
        long = "jwt-role-claim",
        env = "JWT_ROLE_CLAIM",
        global = true,
        default_value = "role"
    )]
    jwt_role_claim: String,
    /// Allow browser cross-origin requests: an origin, a comma list, or "*"
    #[arg(long = "cors-origin", env = "CORS_ORIGIN", global = true)]
    cors_origin: Option<String>,
}

impl Options {
    fn upstream(&self) -> UpstreamConfig {
        UpstreamConfig {
            host: self.pg_host.clone(),
            port: self.pg_port,
            user: self.pg_user.clone(),
            password: self.pg_password.clone(),
            database: self.pg_database.clone(),
            publication: self.publication.clone(),
            slot: self.slot.clone(),
            sslmode: self.sslmode.clone(),
        }
    }
}

#[actix_web::main]
async fn main() {
    if let Err(error) = run_cli().await {
        eprintln!("cache-server: {error}");
        std::process::exit(1);
    }
}

async fn run_cli() -> Result<(), CacheError> {
    let cli = Cli::parse();
    let options = cli.args;

    match cli.command.unwrap_or(Command::Serve) {
        Command::Init => init(options.upstream()).await?,
        Command::Serve => {
            let config = ServerConfig {
                bind_addr: format!("{}:{}", options.host, options.port),
                data_dir: options.data_dir.clone(),
                max_connections: options.max_connections,
                cache_size_bytes: options.cache_size_bytes,
                jwt_secret: options.jwt_secret.clone(),
                jwt_public_key: options.jwt_public_key.clone(),
                jwt_jwks_url: options.jwt_jwks_url.clone(),
                jwt_role_claim: options.jwt_role_claim.clone(),
                cors_origin: options.cors_origin.clone(),
                upstream: options.upstream(),
            };
            run(config).await?;
        }
    }
    Ok(())
}
