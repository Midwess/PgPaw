use pgpaw::{recover_primary, LifecycleErrorKind, PgPaw, PrimarySource, Source};
use tokio_postgres::NoTls;

#[cfg(feature = "az-wire")]
use az_wire::{http, Node, ParentLink, SendExt, TopologyConfig};
#[cfg(feature = "az-wire")]
use futures_util::StreamExt;
#[cfg(feature = "az-wire")]
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
#[cfg(feature = "az-wire")]
use pgpaw::protocol::payload::LiveEvent;
#[cfg(feature = "az-wire")]
use pgpaw::protocol::subjects::{LIVE_SUBJECT, READ_SUBJECT};
#[cfg(feature = "az-wire")]
use pgpaw::AuthConfig;
#[cfg(feature = "az-wire")]
use serde_json::json;

fn ensure_runtime_dir() {
    if std::env::var_os("PGLITE_RUNTIME_DIR").is_none() {
        std::env::set_var(
            "PGLITE_RUNTIME_DIR",
            concat!(env!("CARGO_MANIFEST_DIR"), "/target/pglite-rt"),
        );
    }
}

#[tokio::test]
#[serial_test::serial]
async fn primary_serves_writable_postgres_over_tcp() {
    ensure_runtime_dir();
    let port: u16 = 50000 + (std::process::id() % 15000) as u16;
    let dir = std::env::temp_dir().join(format!("pgpaw-primary-{}-{port}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    let database = "worldant_test";
    let pgpaw = PgPaw::builder()
        .source(Source::primary(PrimarySource {
            data_dir: dir,
            database: database.into(),
            listen_addresses: "127.0.0.1".into(),
            port,
            min_connections: 0,
            max_connections: 5,
        }))
        .open()
        .await
        .expect("open primary over TCP");

    let dsn = format!("postgres://postgres@127.0.0.1:{port}/{database}");
    let (client, connection) = tokio_postgres::connect(&dsn, NoTls)
        .await
        .expect("tcp connect to primary");
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let current: String = client
        .query_one("SELECT current_database()", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(current, database);

    client
        .batch_execute("CREATE TABLE t (id int primary key); INSERT INTO t VALUES (1)")
        .await
        .unwrap();
    let rows = client.query("SELECT id FROM t", &[]).await.unwrap();
    assert_eq!(rows[0].get::<_, i32>(0), 1);

    pgpaw.shutdown().await.unwrap();
}

#[tokio::test]
#[serial_test::serial]
async fn primary_rejects_invalid_connection_bounds_before_starting() {
    let dir = std::env::temp_dir().join(format!(
        "pgpaw-primary-invalid-bounds-{}",
        std::process::id()
    ));
    let result = PgPaw::builder()
        .source(Source::primary(PrimarySource {
            data_dir: dir.clone(),
            database: "postgres".into(),
            listen_addresses: String::new(),
            port: 0,
            min_connections: 2,
            max_connections: 1,
        }))
        .open()
        .await;
    let error = match result {
        Ok(pgpaw) => {
            pgpaw.shutdown().await.unwrap();
            panic!("invalid connection bounds must fail");
        }
        Err(error) => error,
    };

    assert!(error.to_string().contains("primary connections require"));
    assert_eq!(
        error.lifecycle_kind(),
        Some(LifecycleErrorKind::InvalidConfiguration)
    );
    assert!(!dir.exists());
}

#[tokio::test]
#[serial_test::serial]
async fn primary_reports_a_busy_data_directory() {
    ensure_runtime_dir();
    let dir = tempfile::tempdir().unwrap();
    let primary = PgPaw::builder()
        .source(Source::primary(PrimarySource::embedded(dir.path())))
        .open()
        .await
        .unwrap();

    let error = match PgPaw::builder()
        .source(Source::primary(PrimarySource::embedded(dir.path())))
        .open()
        .await
    {
        Ok(pgpaw) => {
            pgpaw.shutdown().await.unwrap();
            panic!("busy primary must fail");
        }
        Err(error) => error,
    };

    assert_eq!(
        error.lifecycle_kind(),
        Some(LifecycleErrorKind::DataDirectoryBusy)
    );
    primary.shutdown().await.unwrap();
}

#[cfg(unix)]
#[test]
fn primary_recovery_removes_a_dead_postmaster_pid() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("postmaster.pid");
    std::fs::write(&path, "2147483647\n").unwrap();

    recover_primary(dir.path()).unwrap();

    assert!(!path.exists());
}

#[cfg(unix)]
#[test]
fn primary_recovery_leaves_unrelated_runtime_processes_untouched() {
    let requested = tempfile::tempdir().unwrap();
    let requested_pid = requested.path().join("postmaster.pid");
    std::fs::write(&requested_pid, "2147483647\n").unwrap();

    let runtime = tempfile::tempdir().unwrap();
    let candidate = runtime.path().join("pgl-2147483647-unrelated/bin");
    std::fs::create_dir_all(&candidate).unwrap();
    let executable = candidate.join("postgres");
    std::fs::copy("/bin/sleep", &executable).unwrap();
    let mut unrelated = std::process::Command::new(executable)
        .arg("30")
        .spawn()
        .unwrap();
    let stale_candidate = runtime.path().join("pgl-2147483647-stale");
    std::fs::create_dir_all(&stale_candidate).unwrap();

    recover_primary(requested.path()).unwrap();

    assert!(!requested_pid.exists());
    assert!(unrelated.try_wait().unwrap().is_none());
    assert!(candidate.exists());
    assert!(stale_candidate.exists());
    unrelated.kill().unwrap();
    unrelated.wait().unwrap();
}

#[cfg(unix)]
#[test]
fn primary_recovery_reports_a_live_data_directory_as_busy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("postmaster.pid");
    std::fs::write(&path, format!("{}\n", std::process::id())).unwrap();

    let error = recover_primary(dir.path()).unwrap_err();

    assert_eq!(
        error.lifecycle_kind(),
        Some(LifecycleErrorKind::DataDirectoryBusy)
    );
    assert!(path.exists());
}

#[cfg(unix)]
#[test]
fn primary_recovery_rejects_unprovable_ownership_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("postmaster.pid");
    std::fs::write(&path, "not-a-pid\n").unwrap();

    let error = recover_primary(dir.path()).unwrap_err();

    assert_eq!(error.lifecycle_kind(), Some(LifecycleErrorKind::Recovery));
    assert!(path.exists());
}

#[cfg(all(feature = "az-wire", unix))]
#[tokio::test]
#[serial_test::serial]
async fn child_startup_failure_is_topology_and_rolls_back_the_primary() {
    ensure_runtime_dir();
    let dir = tempfile::tempdir().unwrap();
    let error = PgPaw::builder()
        .source(Source::primary(PrimarySource::embedded(
            dir.path().join("primary"),
        )))
        .az_wire(
            az_wire::NodeBuilder::new("pgpaw").insecure_accept_declared_peer_identities(),
            TopologyConfig::parent(ParentLink::unix(
                "worldant",
                dir.path().join("missing.sock"),
            )),
        )
        .open()
        .await
        .unwrap_err();

    assert_eq!(error.lifecycle_kind(), Some(LifecycleErrorKind::Topology));

    let reopened = PgPaw::builder()
        .source(Source::primary(PrimarySource::embedded(
            dir.path().join("primary"),
        )))
        .open()
        .await
        .expect("failed child startup must release the primary data directory");
    reopened.shutdown().await.unwrap();
}

#[cfg(all(feature = "az-wire", unix))]
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn interrupted_wait_still_shuts_down_a_parent_linked_child() {
    ensure_runtime_dir();
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("parent.sock");
    let parent = Node::builder("worldant")
        .insecure_accept_declared_peer_identities()
        .build()
        .unwrap();
    let hosting = parent.host_unix(az_wire_transport::unix::UnixListener::bind(&socket).unwrap());
    let mut pgpaw = PgPaw::builder()
        .source(Source::primary(PrimarySource::embedded(
            dir.path().join("primary"),
        )))
        .az_wire(
            az_wire::NodeBuilder::new("pgpaw").insecure_accept_declared_peer_identities(),
            TopologyConfig::parent(ParentLink::unix("worldant", &socket)),
        )
        .open()
        .await
        .unwrap();

    let interrupted =
        tokio::time::timeout(std::time::Duration::from_millis(100), pgpaw.wait()).await;
    assert!(interrupted.is_err(), "wait must still be pending");

    pgpaw.shutdown().await.unwrap();
    hosting.shutdown().await.unwrap();
}

#[cfg(all(feature = "az-wire", unix))]
fn configured_primary(data_dir: std::path::PathBuf, port: u16) -> PrimarySource {
    PrimarySource {
        data_dir,
        database: "configured".into(),
        listen_addresses: "127.0.0.1".into(),
        port,
        min_connections: 0,
        max_connections: 5,
    }
}

#[cfg(all(feature = "az-wire", unix))]
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn embedded_child_reads_configured_database_and_observes_external_commits() {
    ensure_runtime_dir();
    let port: u16 = 35000 + (std::process::id() % 15000) as u16;
    let dir = tempfile::tempdir().unwrap();
    let primary_dir = dir.path().join("primary");
    let external_dsn = format!("postgres://postgres@127.0.0.1:{port}/configured");

    let bootstrap = PgPaw::builder()
        .source(Source::primary(configured_primary(primary_dir.clone(), port)))
        .open()
        .await
        .unwrap();
    {
        let (client, connection) = tokio_postgres::connect(&external_dsn, NoTls).await.unwrap();
        let connection = tokio::spawn(async move {
            let _ = connection.await;
        });
        client.batch_execute("CREATE TABLE items (id int primary key, name text); GRANT SELECT ON items TO PUBLIC; INSERT INTO items VALUES (1, 'first')").await.unwrap();
        client.batch_execute("CREATE ROLE authenticated; CREATE TABLE private_items (id int primary key, owner int, name text); ALTER TABLE private_items ENABLE ROW LEVEL SECURITY; GRANT SELECT ON private_items TO authenticated; CREATE POLICY private_owner ON private_items FOR SELECT TO authenticated USING (owner = ((current_setting('request.jwt.claims', true))::jsonb ->> 'owner')::int); INSERT INTO private_items VALUES (1, 7, 'allowed'), (2, 8, 'denied')").await.unwrap();
        drop(client);
        connection.abort();
    }
    bootstrap.shutdown().await.unwrap();

    let socket = dir.path().join("parent.sock");
    let parent = Node::builder("worldant")
        .insecure_accept_declared_peer_identities()
        .build()
        .unwrap();
    let hosting = parent.host_unix(az_wire_transport::unix::UnixListener::bind(&socket).unwrap());
    let pgpaw = PgPaw::builder()
        .source(Source::primary(configured_primary(primary_dir, port)))
        .auth(AuthConfig::jwt_secret("embedded-secret"))
        .az_wire(
            az_wire::NodeBuilder::new("pgpaw").insecure_accept_declared_peer_identities(),
            TopologyConfig::parent(ParentLink::unix("worldant", &socket)),
        )
        .open()
        .await
        .unwrap();
    assert!(pgpaw.primary_dsn().unwrap().contains("/configured"));
    let (client, connection) = tokio_postgres::connect(&external_dsn, NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if parent.reachable_names() == ["pgpaw.cursor", "pgpaw.live", "pgpaw.read"] {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let read = http::Request::post(format!("/{READ_SUBJECT}"))
        .body(json!({"sql": "SELECT id, name FROM items ORDER BY id", "bearer": null}))
        .send(&parent)
        .await
        .unwrap();
    let read: serde_json::Value = serde_json::from_slice(read.body()).unwrap();
    let version = read["version"].as_u64().unwrap();
    let hash = read["hash"].as_str().unwrap();
    let cursor = http::Request::post("/pgpaw.cursor")
        .body(json!({"hash": hash, "version": version.to_string()}))
        .send(&parent)
        .await
        .unwrap();
    let cursor: serde_json::Value = serde_json::from_slice(cursor.body()).unwrap();
    assert_eq!(cursor["rows"], json!([{"id": 1, "name": "first"}]));

    let token = encode(
        &Header::new(Algorithm::HS256),
        &json!({"role": "authenticated", "owner": 7, "exp": 4_102_444_800u64}),
        &EncodingKey::from_secret(b"embedded-secret"),
    )
    .unwrap();
    let protected = http::Request::post(format!("/{READ_SUBJECT}"))
        .body(json!({"sql": "SELECT id, name FROM private_items ORDER BY id", "bearer": token}))
        .send(&parent)
        .await
        .unwrap();
    let protected: serde_json::Value = serde_json::from_slice(protected.body()).unwrap();
    assert_eq!(protected["rows"], json!([{"id": 1, "name": "allowed"}]));
    assert!(http::Request::post(format!("/{READ_SUBJECT}"))
        .body(json!({"sql": "SELECT id FROM private_items", "bearer": "invalid"}))
        .send(&parent)
        .await
        .is_err());
    assert!(http::Request::post(format!("/{READ_SUBJECT}"))
        .body(json!({"sql": "SELECT id FROM private_items", "bearer": null}))
        .send(&parent)
        .await
        .is_err());

    client
        .batch_execute("REVOKE SELECT ON items FROM PUBLIC")
        .await
        .unwrap();
    assert!(http::Request::post(format!("/{READ_SUBJECT}"))
        .body(json!({"sql": "SELECT id FROM items", "bearer": null}))
        .send(&parent)
        .await
        .is_err());
    client
        .batch_execute("GRANT SELECT ON items TO PUBLIC")
        .await
        .unwrap();
    assert!(http::Request::post(format!("/{READ_SUBJECT}"))
        .body(json!({"sql": "SELECT id FROM items", "bearer": null}))
        .send(&parent)
        .await
        .is_ok());
    client
        .batch_execute("ALTER TABLE items ENABLE ROW LEVEL SECURITY")
        .await
        .unwrap();
    assert!(http::Request::post(format!("/{READ_SUBJECT}"))
        .body(json!({"sql": "SELECT id FROM items", "bearer": null}))
        .send(&parent)
        .await
        .is_err());
    client
        .batch_execute("ALTER TABLE items DISABLE ROW LEVEL SECURITY")
        .await
        .unwrap();
    assert!(http::Request::post(format!("/{READ_SUBJECT}"))
        .body(json!({"sql": "SELECT id FROM items", "bearer": null}))
        .send(&parent)
        .await
        .is_ok());

    let mut live = parent
        .subscribe(
            LIVE_SUBJECT,
            json!({"sql": "SELECT id, name FROM items ORDER BY id", "bearer": null}),
        )
        .await
        .unwrap();
    let snapshot: LiveEvent = serde_json::from_slice(&live.next().await.unwrap().unwrap()).unwrap();
    assert!(matches!(snapshot, LiveEvent::Snapshot { .. }));
    assert_eq!(pgpaw.live_subscription_count(), 1);
    client
        .execute("INSERT INTO items VALUES ($1, $2)", &[&2i32, &"second"])
        .await
        .unwrap();
    let update = tokio::time::timeout(std::time::Duration::from_secs(5), live.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let update: LiveEvent = serde_json::from_slice(&update).unwrap();
    assert!(
        matches!(update, LiveEvent::Insert { ref row, .. } if row == &json!({"id": 2, "name": "second"}))
    );

    drop(live);
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while pgpaw.live_subscription_count() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    pgpaw.shutdown().await.unwrap();
    hosting.shutdown().await.unwrap();
}
