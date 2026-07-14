use std::collections::HashMap;
use std::pin::Pin;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use futures_util::{Stream, StreamExt};
use postgresql_embedded::{PostgreSQL, Settings};
use serde_json::json;
use tokio::time::{sleep, timeout};

const USERS: i64 = 5_000;
const ORDERS: i64 = 25_000;
const READ_ITERS: usize = 300;
const RT_ITERS: usize = 50;

static BASE: OnceLock<String> = OnceLock::new();

fn base() -> &'static str {
    BASE.get().expect("base url not initialized")
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

type EvStream = Pin<Box<dyn Stream<Item = reqwest::Result<bytes::Bytes>> + Send>>;

#[tokio::main]
async fn main() -> Result<()> {
    let settings = Settings {
        configuration: HashMap::from([
            ("wal_level".into(), "logical".into()),
            ("max_wal_senders".into(), "10".into()),
            ("max_replication_slots".into(), "10".into()),
        ]),
        ..Default::default()
    };
    let mut pg = PostgreSQL::new(settings);
    println!("[bench] downloading + starting embedded postgres ...");
    pg.setup().await?;
    pg.start().await?;
    pg.create_database("bench").await?;
    let s = pg.settings();
    let (host, port, user, pass) = (
        s.host.clone(),
        s.port,
        s.username.clone(),
        s.password.clone(),
    );
    println!("[bench] upstream postgres ready at {host}:{port}");
    {
        let (c, _h) = upstream(&host, port, &user, &pass).await?;
        let wal: String = c.query_one("SHOW wal_level", &[]).await?.get(0);
        let senders: String = c.query_one("SHOW max_wal_senders", &[]).await?.get(0);
        println!("[bench] upstream wal_level={wal} max_wal_senders={senders}");
    }

    seed(&host, port, &user, &pass).await?;
    println!("[bench] seeded {USERS} users, {ORDERS} orders");

    let http_port = free_port();
    BASE.set(format!("http://127.0.0.1:{http_port}")).unwrap();
    spawn_pgpaw(host.clone(), port, user.clone(), pass.clone(), http_port);
    wait_ready().await?;
    println!("[bench] PgPaw ready (replica backfilled)\n");

    let http = reqwest::Client::builder().build()?;
    let (mut up, _conn) = upstream(&host, port, &user, &pass).await?;

    println!("== reads · NO JOIN (SELECT ... FROM bench_users WHERE id = ?) ==");
    bench_reads(&http, &mut up, &|id| {
        format!("SELECT id, name, status FROM bench_users WHERE id = {id}")
    })
    .await?;

    println!("\n== reads · WITH JOIN (bench_users ⋈ bench_orders WHERE u.id = ?) ==");
    bench_reads(&http, &mut up, &|id| {
        format!(
            "SELECT u.id, u.name, o.amount FROM bench_users u \
             JOIN bench_orders o ON o.user_id = u.id WHERE u.id = {id}"
        )
    })
    .await?;

    println!("\n== realtime · upstream write -> live SSE delta ==");
    bench_realtime(&http, &mut up).await?;

    println!("\n[bench] done");
    Ok(())
}

async fn bench_reads(
    http: &reqwest::Client,
    up: &mut tokio_postgres::Client,
    sql_for: &dyn Fn(i64) -> String,
) -> Result<()> {
    let mut direct = Vec::with_capacity(READ_ITERS);
    for i in 0..READ_ITERS {
        let q = sql_for(1 + (i as i64 % USERS));
        let t = Instant::now();
        up.query(&q, &[]).await?;
        direct.push(t.elapsed());
    }
    report("upstream direct", direct);

    let mut cold = Vec::with_capacity(READ_ITERS);
    for i in 0..READ_ITERS {
        let q = sql_for(1 + (i as i64 % USERS));
        let t = Instant::now();
        query(http, &q).await?;
        cold.push(t.elapsed());
    }
    report("pgpaw cold (cache miss)", cold);

    let warm_q = sql_for(1);
    query(http, &warm_q).await?;
    let mut warm = Vec::with_capacity(READ_ITERS);
    for _ in 0..READ_ITERS {
        let t = Instant::now();
        query(http, &warm_q).await?;
        warm.push(t.elapsed());
    }
    report("pgpaw warm (cache hit)", warm);
    Ok(())
}

async fn bench_realtime(http: &reqwest::Client, up: &mut tokio_postgres::Client) -> Result<()> {
    let id = 1i64;
    let resp = http
        .post(format!("{}/query?live=true", base()))
        .json(&json!({ "sql": format!("SELECT id, status FROM bench_users WHERE id = {id}") }))
        .send()
        .await?
        .error_for_status()?;
    let mut stream: EvStream = Box::pin(resp.bytes_stream());
    let mut buf = String::new();
    next_event(&mut stream, &mut buf).await?;

    let mut latencies = Vec::with_capacity(RT_ITERS);
    for i in 0..RT_ITERS {
        let t = Instant::now();
        up.execute(
            &format!("UPDATE bench_users SET status = 'rt{i}' WHERE id = {id}"),
            &[],
        )
        .await?;
        loop {
            let evt = timeout(Duration::from_secs(10), next_event(&mut stream, &mut buf)).await??;
            if evt.contains("\"op\"") && !evt.contains("up-to-date") {
                break;
            }
        }
        latencies.push(t.elapsed());
    }
    report("realtime write->delta", latencies);
    Ok(())
}

async fn next_event(stream: &mut EvStream, buf: &mut String) -> Result<String> {
    loop {
        if let Some(pos) = buf.find("\n\n") {
            let evt: String = buf.drain(..pos + 2).collect();
            let evt = evt.trim().to_string();
            if evt.is_empty() {
                continue;
            }
            return Ok(evt);
        }
        match stream.next().await {
            Some(chunk) => buf.push_str(&String::from_utf8_lossy(&chunk?)),
            None => bail!("sse stream closed by server"),
        }
    }
}

async fn query(http: &reqwest::Client, sql: &str) -> Result<String> {
    Ok(http
        .post(format!("{}/query", base()))
        .json(&json!({ "sql": sql }))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?)
}

fn report(name: &str, mut samples: Vec<Duration>) {
    samples.sort();
    let n = samples.len();
    let ms = |d: &Duration| d.as_secs_f64() * 1000.0;
    let pct = |q: f64| ms(&samples[(((n as f64) * q) as usize).min(n - 1)]);
    let mean = samples.iter().map(ms).sum::<f64>() / n as f64;
    println!(
        "  {name:<26} n={n:<4} mean={mean:7.3}ms  p50={:7.3}  p95={:7.3}  p99={:7.3}",
        pct(0.50),
        pct(0.95),
        pct(0.99)
    );
}

async fn upstream(
    host: &str,
    port: u16,
    user: &str,
    pass: &str,
) -> Result<(tokio_postgres::Client, tokio::task::JoinHandle<()>)> {
    let (client, conn) = tokio_postgres::connect(
        &format!("host={host} port={port} user={user} password={pass} dbname=bench"),
        tokio_postgres::NoTls,
    )
    .await?;
    let handle = tokio::spawn(async move {
        let _ = conn.await;
    });
    Ok((client, handle))
}

async fn seed(host: &str, port: u16, user: &str, pass: &str) -> Result<()> {
    let (c, _h) = upstream(host, port, user, pass).await?;
    c.batch_execute(
        "CREATE TABLE bench_users (id bigint PRIMARY KEY, name text NOT NULL, status text NOT NULL);
         CREATE TABLE bench_orders (id bigint PRIMARY KEY, user_id bigint NOT NULL, amount numeric NOT NULL);
         CREATE INDEX bench_orders_user ON bench_orders (user_id);",
    )
    .await?;
    c.batch_execute(&format!(
        "INSERT INTO bench_users SELECT g, 'user' || g, 'active' FROM generate_series(1, {USERS}) g;
         INSERT INTO bench_orders SELECT g, ((g - 1) % {USERS}) + 1, (g % 100)::numeric \
           FROM generate_series(1, {ORDERS}) g;"
    ))
    .await?;
    c.batch_execute("CREATE PUBLICATION pgpaw_pub FOR ALL TABLES;")
        .await?;
    Ok(())
}

fn spawn_pgpaw(host: String, port: u16, user: String, pass: String, http_port: u16) {
    std::thread::spawn(move || {
        let config = pgpaw::ServerConfig {
            bind_addr: format!("127.0.0.1:{http_port}"),
            #[cfg(feature = "az-wire")]
            az_wire_addr: None,
            #[cfg(feature = "az-wire")]
            az_wire_node: "pgpaw".into(),
            data_dir: std::env::temp_dir().join(format!("pgpaw-bench-{}", std::process::id())),
            max_connections: 8,
            cache_size_bytes: 256 * 1024 * 1024,
            jwt_secret: None,
            jwt_public_key: None,
            jwt_jwks_url: None,
            jwt_role_claim: "role".into(),
            cors_origin: None,
            upstream: pgpaw::UpstreamConfig {
                host,
                port,
                user,
                password: pass,
                database: "bench".into(),
                publication: "pgpaw_pub".into(),
                slot: "pgpaw_bench_slot".into(),
                sslmode: "disable".into(),
            },
        };
        if let Err(error) = actix_web::rt::System::new().block_on(pgpaw::run(config)) {
            eprintln!("[bench] pgpaw exited: {error}");
        }
    });
}

async fn wait_ready() -> Result<()> {
    let http = reqwest::Client::new();
    for _ in 0..1200 {
        let ok = http
            .post(format!("{}/query", base()))
            .json(&json!({ "sql": "SELECT id FROM bench_users WHERE id = 1" }))
            .send()
            .await
            .ok()
            .filter(|r| r.status().is_success())
            .is_some();
        if ok {
            return Ok(());
        }
        sleep(Duration::from_millis(100)).await;
    }
    bail!("pgpaw did not become ready in time")
}
