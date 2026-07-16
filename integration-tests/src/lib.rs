use std::path::PathBuf;
use std::time::{Duration, Instant};

use postgresql_embedded::{PostgreSQL, Settings};
use serde_json::Value;
use tokio_postgres::{Client, NoTls};

pub const JWT_SECRET: &str = "pgpaw-test-secret-please-change";
pub const PUBLICATION: &str = "pgpaw_pub";

pub struct Upstream {
    _pg: PostgreSQL,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub database: String,
}

impl Upstream {
    pub async fn start() -> Upstream {
        let mut settings = Settings {
            username: "postgres".to_string(),
            password: "postgres".to_string(),
            temporary: true,
            ..Default::default()
        };
        settings
            .configuration
            .insert("wal_level".to_string(), "logical".to_string());
        settings
            .configuration
            .insert("max_wal_senders".to_string(), "10".to_string());
        settings
            .configuration
            .insert("max_replication_slots".to_string(), "10".to_string());

        let mut pg = PostgreSQL::new(settings);
        pg.setup().await.expect("embedded postgres setup");
        pg.start().await.expect("embedded postgres start");

        let s = pg.settings();
        let host = if s.host.is_empty() {
            "127.0.0.1".to_string()
        } else {
            s.host.clone()
        };
        let (port, user, password) = (s.port, s.username.clone(), s.password.clone());

        Upstream {
            _pg: pg,
            host,
            port,
            user,
            password,
            database: "postgres".to_string(),
        }
    }

    pub async fn client(&self) -> Client {
        let (client, connection) = tokio_postgres::Config::new()
            .host(&self.host)
            .port(self.port)
            .user(&self.user)
            .password(&self.password)
            .dbname(&self.database)
            .connect(NoTls)
            .await
            .expect("connect to embedded postgres");
        tokio::spawn(connection);
        client
    }

    pub async fn run_sql(&self, sql: &str) {
        let client = self.client().await;
        client
            .batch_execute(sql)
            .await
            .unwrap_or_else(|e| panic!("upstream sql failed:\n{sql}\n--> {e}"));
    }

    pub async fn install_ddl_trigger(&self) {
        let prefix = pglite::DDL_SIGNAL_PREFIX;
        self.run_sql(&format!(
            "CREATE OR REPLACE FUNCTION pglite_emit_ddl() RETURNS event_trigger \
             LANGUAGE plpgsql AS $fn$ BEGIN \
               PERFORM pg_logical_emit_message(true, '{prefix}', ''); \
             END $fn$; \
             DROP EVENT TRIGGER IF EXISTS pglite_ddl_watch; \
             CREATE EVENT TRIGGER pglite_ddl_watch ON ddl_command_end \
               EXECUTE FUNCTION pglite_emit_ddl();"
        ))
        .await;
    }

    pub async fn setting(&self, name: &str) -> String {
        self.client()
            .await
            .query_one("select current_setting($1)", &[&name])
            .await
            .expect("read setting")
            .get(0)
    }
}

pub struct Server {
    pub base: String,
    http: reqwest::Client,
    _data: tempfile::TempDir,
}

impl Server {
    pub async fn start(up: &Upstream, jwt_secret: Option<&str>) -> Server {
        up.run_sql(&format!("CREATE PUBLICATION {PUBLICATION} FOR ALL TABLES"))
            .await;
        Self::launch(up, jwt_secret).await
    }

    pub async fn launch(up: &Upstream, jwt_secret: Option<&str>) -> Server {
        let data = tempfile::tempdir().expect("tempdir");
        let port = free_port();
        let addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().expect("http addr");
        let source = pgpaw::ReplicaSource {
            upstream: pgpaw::UpstreamConfig {
                host: up.host.clone(),
                port: up.port,
                user: up.user.clone(),
                password: up.password.clone(),
                database: up.database.clone(),
                sslmode: "disable".to_string(),
            },
            data_dir: PathBuf::from(data.path()),
            publication: PUBLICATION.to_string(),
            slot: "pgpaw_slot".to_string(),
            max_connections: 8,
        };
        let auth = pgpaw::AuthConfig {
            jwt_secret: jwt_secret.map(str::to_string),
            ..pgpaw::AuthConfig::default()
        };

        let handle = std::thread::spawn(move || {
            actix_web::rt::System::new().block_on(async move {
                let mut pgpaw = pgpaw::PgPaw::builder()
                    .source(pgpaw::Source::replica(source))
                    .cache(pgpaw::CacheConfig {
                        max_bytes: 64 * 1024 * 1024,
                    })
                    .auth(auth)
                    .http(pgpaw::HttpConfig {
                        addr,
                        cors_origin: None,
                    })
                    .open()
                    .await?;
                pgpaw.wait().await
            })
        });
        let base = format!("http://127.0.0.1:{port}");
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("reqwest client");

        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if handle.is_finished() {
                let outcome = handle.join();
                panic!("pgpaw server exited before becoming ready: {outcome:?}");
            }
            if let Ok(resp) = http.get(format!("{base}/healthz")).send().await {
                if resp.status().is_success() {
                    break;
                }
            }
            if Instant::now() > deadline {
                panic!("pgpaw server did not become healthy within 60s");
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }

        Server {
            base,
            http,
            _data: data,
        }
    }

    pub async fn query(&self, sql: &str, token: Option<&str>) -> reqwest::Response {
        let mut req = self
            .http
            .post(format!("{}/query", self.base))
            .json(&serde_json::json!({ "sql": sql }));
        if let Some(token) = token {
            req = req.bearer_auth(token);
        }
        req.send().await.expect("query request")
    }

    pub async fn live(&self, sql: &str, token: Option<&str>) -> reqwest::Response {
        let mut req = self
            .http
            .post(format!("{}/query?live=true", self.base))
            .json(&serde_json::json!({ "sql": sql }));
        if let Some(token) = token {
            req = req.bearer_auth(token);
        }
        req.send().await.expect("live query request")
    }

    pub async fn query_auth(&self, sql: &str, authorization: &str) -> reqwest::Response {
        self.http
            .post(format!("{}/query", self.base))
            .header("authorization", authorization)
            .json(&serde_json::json!({ "sql": sql }))
            .send()
            .await
            .expect("query request")
    }

    pub async fn cursor(&self, location: &str) -> reqwest::Response {
        self.http
            .get(format!("{}{}", self.base, location))
            .send()
            .await
            .expect("cursor request")
    }

    pub async fn rows(&self, sql: &str, token: Option<&str>) -> Vec<Value> {
        let resp = self.query(sql, token).await;
        let status = resp.status().as_u16();
        if status == 303 {
            let location = resp
                .headers()
                .get("location")
                .expect("303 has location")
                .to_str()
                .unwrap()
                .to_string();
            let followed = self.cursor(&location).await;
            return as_array(followed).await;
        }
        assert_eq!(status, 200, "expected rows but got {status}");
        as_array(resp).await
    }

    pub async fn wait_status(&self, sql: &str, token: Option<&str>, want: u16, secs: u64) {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let got = self.query(sql, token).await.status().as_u16();
            if got == want {
                return;
            }
            if Instant::now() > deadline {
                panic!("status did not reach {want} for `{sql}` within {secs}s (last saw {got})");
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    pub async fn wait_rows(&self, sql: &str, token: Option<&str>, want: usize) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let resp = self.query(sql, token).await;
            let status = resp.status().as_u16();
            let got = match status {
                200 => Some(as_array(resp).await.len()),
                303 => match resp.headers().get("location").and_then(|v| v.to_str().ok()) {
                    Some(location) => {
                        let location = location.to_string();
                        Some(as_array(self.cursor(&location).await).await.len())
                    }
                    None => None,
                },
                _ => None,
            };
            let last = match got {
                Some(n) if n == want => return,
                Some(n) => format!("{n} rows"),
                None => format!("status {status}"),
            };
            if Instant::now() > deadline {
                panic!("replication did not yield {want} rows for `{sql}` (last saw {last})");
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}

pub async fn as_array(resp: reqwest::Response) -> Vec<Value> {
    let body = resp.text().await.expect("read body");
    let value: Value = serde_json::from_str(&body).unwrap_or_else(|e| {
        panic!("body was not JSON: {e}\n{body}");
    });
    value
        .as_array()
        .unwrap_or_else(|| panic!("body was not a JSON array:\n{body}"))
        .clone()
}

pub async fn run_and_capture_error(
    up: &Upstream,
    jwt_secret: Option<&str>,
    jwt_jwks_url: Option<&str>,
) -> pgpaw::CacheError {
    up.run_sql(&format!("CREATE PUBLICATION {PUBLICATION} FOR ALL TABLES"))
        .await;
    let data = tempfile::tempdir().expect("tempdir");
    let port = free_port();
    let addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().expect("http addr");
    pgpaw::PgPaw::builder()
        .source(pgpaw::Source::replica(pgpaw::ReplicaSource {
            upstream: pgpaw::UpstreamConfig {
                host: up.host.clone(),
                port: up.port,
                user: up.user.clone(),
                password: up.password.clone(),
                database: up.database.clone(),
                sslmode: "disable".to_string(),
            },
            data_dir: PathBuf::from(data.path()),
            publication: PUBLICATION.to_string(),
            slot: "pgpaw_slot".to_string(),
            max_connections: 8,
        }))
        .cache(pgpaw::CacheConfig {
            max_bytes: 64 * 1024 * 1024,
        })
        .auth(pgpaw::AuthConfig {
            jwt_secret: jwt_secret.map(str::to_string),
            jwt_jwks_url: jwt_jwks_url.map(str::to_string),
            ..pgpaw::AuthConfig::default()
        })
        .http(pgpaw::HttpConfig {
            addr,
            cors_origin: None,
        })
        .open()
        .await
        .expect_err("expected pgpaw open to fail")
}

pub fn mint(secret: &str, claims: Value) -> String {
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .expect("sign jwt")
}

pub fn cache_control(resp: &reqwest::Response) -> String {
    resp.headers()
        .get("cache-control")
        .map(|v| v.to_str().unwrap_or("").to_string())
        .unwrap_or_default()
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}
