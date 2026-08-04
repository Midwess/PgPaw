use pgpaw::{recover_primary, EmbeddedPrimarySource, LifecycleErrorKind, PgPaw, PgSource};
use tokio_postgres::NoTls;

#[cfg(feature = "unb")]
use unb::{http, Node, ParentLink, SendExt, TopologyConfig};
#[cfg(feature = "unb")]
use futures_util::StreamExt;
#[cfg(feature = "unb")]
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
#[cfg(feature = "unb")]
use pgpaw::protocol::payload::LiveEvent;
#[cfg(feature = "unb")]
use pgpaw::protocol::subjects::{LIVE_SUBJECT, SQL_SUBJECT};
#[cfg(feature = "unb")]
use pgpaw::AuthConfig;
#[cfg(feature = "unb")]
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
        .source(PgSource::primary(EmbeddedPrimarySource {
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
        .source(PgSource::primary(EmbeddedPrimarySource {
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
        .source(PgSource::primary(EmbeddedPrimarySource::embedded(
            dir.path(),
        )))
        .open()
        .await
        .unwrap();

    let error = match PgPaw::builder()
        .source(PgSource::primary(EmbeddedPrimarySource::embedded(
            dir.path(),
        )))
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

#[cfg(all(feature = "unb", unix))]
#[tokio::test]
#[serial_test::serial]
async fn child_startup_failure_is_topology_and_rolls_back_the_primary() {
    ensure_runtime_dir();
    let dir = tempfile::tempdir().unwrap();
    let error = PgPaw::builder()
        .source(PgSource::primary(EmbeddedPrimarySource::embedded(
            dir.path().join("primary"),
        )))
        .unb(
            unb::NodeBuilder::new("pgpaw").insecure_accept_declared_peer_identities(),
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
        .source(PgSource::primary(EmbeddedPrimarySource::embedded(
            dir.path().join("primary"),
        )))
        .open()
        .await
        .expect("failed child startup must release the primary data directory");
    reopened.shutdown().await.unwrap();
}

#[cfg(all(feature = "unb", unix))]
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
    let hosting = parent.host_unix(unb_transport::unix::UnixListener::bind(&socket).unwrap());
    let mut pgpaw = PgPaw::builder()
        .source(PgSource::primary(EmbeddedPrimarySource::embedded(
            dir.path().join("primary"),
        )))
        .unb(
            unb::NodeBuilder::new("pgpaw").insecure_accept_declared_peer_identities(),
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

#[cfg(all(feature = "unb", unix))]
fn configured_primary(data_dir: std::path::PathBuf, port: u16) -> EmbeddedPrimarySource {
    EmbeddedPrimarySource {
        data_dir,
        database: "configured".into(),
        listen_addresses: "127.0.0.1".into(),
        port,
        min_connections: 0,
        max_connections: 5,
    }
}

#[cfg(all(feature = "unb", unix))]
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn embedded_child_reads_configured_database_and_observes_external_commits() {
    ensure_runtime_dir();
    let port: u16 = 35000 + (std::process::id() % 15000) as u16;
    let dir = tempfile::tempdir().unwrap();
    let primary_dir = dir.path().join("primary");
    let external_dsn = format!("postgres://postgres@127.0.0.1:{port}/configured");

    let bootstrap = PgPaw::builder()
        .source(PgSource::primary(configured_primary(
            primary_dir.clone(),
            port,
        )))
        .open()
        .await
        .unwrap();
    {
        let (client, connection) = tokio_postgres::connect(&external_dsn, NoTls).await.unwrap();
        let connection = tokio::spawn(async move {
            let _ = connection.await;
        });
        client.batch_execute("CREATE TABLE items (id int primary key, name text); GRANT SELECT ON items TO PUBLIC; INSERT INTO items VALUES (1, 'first')").await.unwrap();
        client.batch_execute("GRANT SELECT ON items TO pgpaw_public; CREATE TABLE notes (id int primary key, body text); GRANT SELECT, INSERT, UPDATE, DELETE ON notes TO pgpaw_public").await.unwrap();
        client.batch_execute("CREATE ROLE authenticated; CREATE TABLE private_items (id int primary key, owner int, name text); ALTER TABLE private_items ENABLE ROW LEVEL SECURITY; GRANT SELECT ON private_items TO authenticated; CREATE POLICY private_owner ON private_items FOR SELECT TO authenticated USING (owner = ((current_setting('request.jwt.claims', true))::jsonb ->> 'owner')::int); INSERT INTO private_items VALUES (1, 7, 'allowed'), (2, 8, 'denied')").await.unwrap();
        client.batch_execute("CREATE TABLE header_items (id int primary key, owner text not null, name text); ALTER TABLE header_items ENABLE ROW LEVEL SECURITY; GRANT SELECT ON header_items TO pgpaw_public; CREATE POLICY header_owner ON header_items FOR SELECT TO pgpaw_public USING (owner = current_setting('request.headers', true)::jsonb ->> 'authorization'); INSERT INTO header_items VALUES (1, 'u1', 'mine'), (2, 'u2', 'theirs')").await.unwrap();
        drop(client);
        connection.abort();
    }
    bootstrap.shutdown().await.unwrap();

    let socket = dir.path().join("parent.sock");
    let parent = Node::builder("worldant")
        .insecure_accept_declared_peer_identities()
        .build()
        .unwrap();
    let hosting = parent.host_unix(unb_transport::unix::UnixListener::bind(&socket).unwrap());
    let pgpaw = PgPaw::builder()
        .source(PgSource::primary(configured_primary(primary_dir, port)))
        .auth(AuthConfig::jwt_secret("embedded-secret"))
        .unb(
            unb::NodeBuilder::new("pgpaw").insecure_accept_declared_peer_identities(),
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
            if parent.reachable_names() == ["pgpaw.live", "pgpaw.sql"] {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let read = http::Request::post(format!("/{SQL_SUBJECT}"))
        .body(json!({"sql": "SELECT id, name FROM items ORDER BY id", "bearer": null}))
        .send(&parent)
        .await
        .unwrap();
    let read: serde_json::Value = serde_json::from_slice(read.body()).unwrap();
    assert_eq!(read["command"], "SELECT");
    assert_eq!(read["rows"], json!([{"id": 1, "name": "first"}]));
    assert_eq!(read["rowsAffected"], 0);

    let insert = http::Request::post(format!("/{SQL_SUBJECT}"))
        .body(json!({"sql": "INSERT INTO notes (id, body) VALUES ($1, $2)", "params": [1, "hello"], "bearer": null}))
        .send(&parent)
        .await
        .unwrap();
    let insert: serde_json::Value = serde_json::from_slice(insert.body()).unwrap();
    assert_eq!(insert["command"], "INSERT");
    assert_eq!(insert["rows"], json!([]));
    assert_eq!(insert["rowsAffected"], 1);

    let update = http::Request::post(format!("/{SQL_SUBJECT}"))
        .body(json!({"sql": "UPDATE notes SET body = $1 WHERE id = $2 RETURNING id, body", "params": ["updated", 1], "bearer": null}))
        .send(&parent)
        .await
        .unwrap();
    let update: serde_json::Value = serde_json::from_slice(update.body()).unwrap();
    assert_eq!(update["command"], "UPDATE");
    assert_eq!(update["rows"], json!([{"id": 1, "body": "updated"}]));
    assert_eq!(update["rowsAffected"], 1);

    assert!(
        http::Request::post(format!("/{SQL_SUBJECT}"))
            .body(json!({"sql": "CREATE TABLE anon_ddl (id int)", "bearer": null}))
            .send(&parent)
            .await
            .is_err(),
        "the neutral public role cannot run DDL"
    );
    assert!(
        http::Request::post(format!("/{SQL_SUBJECT}"))
            .body(json!({"sql": "SELECT 1; SELECT 2", "bearer": null}))
            .send(&parent)
            .await
            .is_err(),
        "multi-statement text is rejected"
    );
    assert!(
        http::Request::post(format!("/{SQL_SUBJECT}"))
            .body(json!({"sql": "BEGIN", "bearer": null}))
            .send(&parent)
            .await
            .is_err(),
        "transaction control is rejected"
    );
    assert!(
        http::Request::post(format!("/{SQL_SUBJECT}"))
            .body(json!({"sql": "SELECT n.id, m.id FROM notes n JOIN notes m ON m.id = n.id", "bearer": null}))
            .send(&parent)
            .await
            .is_err(),
        "duplicate output columns are rejected"
    );
    assert!(
        http::Request::post(format!("/{SQL_SUBJECT}"))
            .body(json!({"sql": "DELETE FROM notes WHERE id = 1 RETURNING id, body AS id", "bearer": null}))
            .send(&parent)
            .await
            .is_err(),
        "duplicate RETURNING names are rejected statically"
    );

    let token = encode(
        &Header::new(Algorithm::HS256),
        &json!({"role": "authenticated", "owner": 7, "exp": 4_102_444_800u64}),
        &EncodingKey::from_secret(b"embedded-secret"),
    )
    .unwrap();
    let protected = http::Request::post(format!("/{SQL_SUBJECT}"))
        .body(json!({"sql": "SELECT id, name FROM private_items ORDER BY id", "bearer": token}))
        .send(&parent)
        .await
        .unwrap();
    let protected: serde_json::Value = serde_json::from_slice(protected.body()).unwrap();
    assert_eq!(protected["rows"], json!([{"id": 1, "name": "allowed"}]));
    assert!(http::Request::post(format!("/{SQL_SUBJECT}"))
        .body(json!({"sql": "SELECT id FROM private_items", "bearer": "invalid"}))
        .send(&parent)
        .await
        .is_err());
    let anon_private = http::Request::post(format!("/{SQL_SUBJECT}"))
        .body(json!({"sql": "SELECT id FROM private_items", "bearer": null}))
        .send(&parent)
        .await
        .unwrap();
    let anon_private: serde_json::Value = serde_json::from_slice(anon_private.body()).unwrap();
    assert_eq!(
        anon_private["rows"],
        json!([]),
        "row security hides every private row from the neutral public role"
    );

    let default_headers: String = client
        .query_one("select current_setting('request.headers', true)", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        default_headers, "{}",
        "the database default keeps request.headers parse-safe on every connection"
    );

    let headered = http::Request::post(format!("/{SQL_SUBJECT}"))
        .header("Authorization", "u1")
        .body(json!({"sql": "SELECT id, name FROM header_items ORDER BY id", "bearer": null}))
        .send(&parent)
        .await
        .unwrap();
    let headered: serde_json::Value = serde_json::from_slice(headered.body()).unwrap();
    assert_eq!(
        headered["rows"],
        json!([{"id": 1, "name": "mine"}]),
        "an RLS policy keyed on request.headers selects only the header owner's rows"
    );
    let unheadered = http::Request::post(format!("/{SQL_SUBJECT}"))
        .body(json!({"sql": "SELECT id, name FROM header_items ORDER BY id", "bearer": null}))
        .send(&parent)
        .await
        .unwrap();
    let unheadered: serde_json::Value = serde_json::from_slice(unheadered.body()).unwrap();
    assert_eq!(
        unheadered["rows"],
        json!([]),
        "a request without headers satisfies no header-derived policy"
    );
    let projected = http::Request::post(format!("/{SQL_SUBJECT}"))
        .header("Authorization", "u1")
        .body(json!({"sql": "SELECT current_setting('request.headers', true) AS headers", "bearer": null}))
        .send(&parent)
        .await
        .unwrap();
    let projected: serde_json::Value = serde_json::from_slice(projected.body()).unwrap();
    let projected: serde_json::Value =
        serde_json::from_str(projected["rows"][0]["headers"].as_str().unwrap()).unwrap();
    assert_eq!(projected["authorization"], "u1");
    assert!(
        projected
            .as_object()
            .unwrap()
            .keys()
            .all(|name| !name.starts_with("unb-")),
        "reserved transport headers never reach request.headers: {projected}"
    );

    client
        .batch_execute("REVOKE SELECT ON items FROM pgpaw_public, PUBLIC")
        .await
        .unwrap();
    assert!(
        http::Request::post(format!("/{SQL_SUBJECT}"))
            .body(json!({"sql": "SELECT id FROM items", "bearer": null}))
            .send(&parent)
            .await
            .is_err(),
        "revoked privilege denies the public role"
    );
    client
        .batch_execute("GRANT SELECT ON items TO pgpaw_public, PUBLIC")
        .await
        .unwrap();
    assert!(http::Request::post(format!("/{SQL_SUBJECT}"))
        .body(json!({"sql": "SELECT id FROM items", "bearer": null}))
        .send(&parent)
        .await
        .is_ok());

    let empty = http::Request::post(format!("/{SQL_SUBJECT}"))
        .body(json!({"sql": "SELECT id, body FROM notes WHERE id = 999", "bearer": null}))
        .send(&parent)
        .await
        .unwrap();
    let empty: serde_json::Value = serde_json::from_slice(empty.body()).unwrap();
    assert_eq!(empty["rows"], json!([]));
    assert_eq!(empty["rowsAffected"], 0);

    let nulls = http::Request::post(format!("/{SQL_SUBJECT}"))
        .body(json!({"sql": "SELECT NULL::text AS missing, 42 AS n", "bearer": null}))
        .send(&parent)
        .await
        .unwrap();
    let nulls: serde_json::Value = serde_json::from_slice(nulls.body()).unwrap();
    assert_eq!(nulls["rows"], json!([{"missing": null, "n": 42}]));

    let owner_token = encode(
        &Header::new(Algorithm::HS256),
        &json!({"role": "postgres", "exp": 4_102_444_800u64}),
        &EncodingKey::from_secret(b"embedded-secret"),
    )
    .unwrap();
    assert!(
        http::Request::post(format!("/{SQL_SUBJECT}"))
            .body(json!({"sql": "SELECT 1", "bearer": owner_token}))
            .send(&parent)
            .await
            .is_err(),
        "the engine owner role never serves public SQL"
    );

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
    let marker: LiveEvent = serde_json::from_slice(&live.next().await.unwrap().unwrap()).unwrap();
    assert!(
        matches!(marker, LiveEvent::UpToDate { .. }),
        "each commit batch ends with up-to-date: {marker:?}"
    );

    let chain = pgpaw::schema::AppChain {
        app: "items_app".into(),
        rel_dir: std::path::PathBuf::from("apps/items_app/migrations"),
        files: vec![pgpaw::schema::MigrationFile {
            filename: "0001_rename.sql".into(),
            ordinal: 1,
            sql: "UPDATE items SET name = 'renamed' WHERE id = 2".into(),
            checksum: pgpaw::schema::content_hash("UPDATE items SET name = 'renamed' WHERE id = 2"),
        }],
    };
    pgpaw
        .schema_ops()
        .apply_migrations("w", &[chain])
        .await
        .unwrap();
    let migrated = tokio::time::timeout(std::time::Duration::from_secs(5), live.next())
        .await
        .expect("a schema-ops migration commit wakes the live subscription")
        .unwrap()
        .unwrap();
    let migrated: LiveEvent = serde_json::from_slice(&migrated).unwrap();
    assert!(
        matches!(migrated, LiveEvent::Update { ref row, .. } if row == &json!({"id": 2, "name": "renamed"})),
        "{migrated:?}"
    );

    let mut headered_live = parent
        .subscribe_with(
            LIVE_SUBJECT,
            json!({"sql": "SELECT id, name FROM header_items ORDER BY id", "bearer": null}),
            serde_json::Map::from_iter([("authorization".to_string(), json!("u1"))]),
        )
        .await
        .unwrap();
    let snapshot: LiveEvent =
        serde_json::from_slice(&headered_live.next().await.unwrap().unwrap()).unwrap();
    let LiveEvent::Snapshot { rows, .. } = snapshot else {
        panic!("expected a snapshot: {snapshot:?}");
    };
    assert_eq!(
        rows.unwrap(),
        json!([{"id": 1, "name": "mine"}]),
        "the initial live snapshot sees request.headers"
    );
    client
        .batch_execute(
            "INSERT INTO header_items VALUES (3, 'u1', 'mine too'), (4, 'u2', 'not mine')",
        )
        .await
        .unwrap();
    let delta = tokio::time::timeout(std::time::Duration::from_secs(5), headered_live.next())
        .await
        .expect("a commit wakes the headered subscription")
        .unwrap()
        .unwrap();
    let delta: LiveEvent = serde_json::from_slice(&delta).unwrap();
    assert!(
        matches!(delta, LiveEvent::Insert { ref row, .. } if row == &json!({"id": 3, "name": "mine too"})),
        "the delta re-query sees the subscribe-time request.headers: {delta:?}"
    );
    drop(headered_live);

    let mut live_one = parent
        .subscribe(
            LIVE_SUBJECT,
            json!({"sql": "SELECT id, name FROM items WHERE id = $1", "params": [1], "bearer": null}),
        )
        .await
        .unwrap();
    let one: LiveEvent = serde_json::from_slice(&live_one.next().await.unwrap().unwrap()).unwrap();
    let mut live_two = parent
        .subscribe(
            LIVE_SUBJECT,
            json!({"sql": "SELECT id, name FROM items WHERE id = $1", "params": [2], "bearer": null}),
        )
        .await
        .unwrap();
    let two: LiveEvent = serde_json::from_slice(&live_two.next().await.unwrap().unwrap()).unwrap();
    let (LiveEvent::Snapshot { hash: hash_one, .. }, LiveEvent::Snapshot { hash: hash_two, .. }) =
        (one, two)
    else {
        panic!("expected two snapshots");
    };
    assert_ne!(
        hash_one, hash_two,
        "identical SQL with different params occupies distinct cache keys"
    );
    drop(live_one);
    drop(live_two);

    let mut filtered = parent
        .subscribe_with(
            LIVE_SUBJECT,
            json!({"sql": "SELECT id, name FROM header_items WHERE id = $1", "params": [3], "bearer": null}),
            serde_json::Map::from_iter([("authorization".to_string(), json!("u1"))]),
        )
        .await
        .unwrap();
    let filtered_snapshot: LiveEvent =
        serde_json::from_slice(&filtered.next().await.unwrap().unwrap()).unwrap();
    let LiveEvent::Snapshot { rows, .. } = filtered_snapshot else {
        panic!("expected a snapshot: {filtered_snapshot:?}");
    };
    assert_eq!(
        rows.unwrap(),
        json!([{"id": 3, "name": "mine too"}]),
        "the live snapshot binds $1 under the subscribe-time headers"
    );
    drop(filtered);

    let joined = parent
        .subscribe(
            LIVE_SUBJECT,
            json!({"sql": "SELECT i.id AS item_id, n.id AS note_id FROM items i JOIN notes n ON n.id = i.id WHERE i.id = $1", "params": [1], "bearer": null}),
        )
        .await
        .unwrap();
    drop(joined);

    let mut fragile = parent
        .subscribe(
            LIVE_SUBJECT,
            json!({"sql": "SELECT id, name FROM items ORDER BY id", "bearer": null}),
        )
        .await
        .unwrap();
    let fragile_snapshot: LiveEvent =
        serde_json::from_slice(&fragile.next().await.unwrap().unwrap()).unwrap();
    assert!(matches!(fragile_snapshot, LiveEvent::Snapshot { .. }));
    client
        .batch_execute(
            "ALTER TABLE items RENAME COLUMN name TO label; \
             INSERT INTO items (id, label) VALUES (9, 'broken')",
        )
        .await
        .unwrap();
    let failed = tokio::time::timeout(std::time::Duration::from_secs(5), fragile.next())
        .await
        .expect("a failed delta re-query answers, not silence")
        .unwrap()
        .unwrap();
    let failed: LiveEvent = serde_json::from_slice(&failed).unwrap();
    assert!(
        matches!(failed, LiveEvent::Reset),
        "a failed re-query resets the subscription instead of emitting a phantom delete: {failed:?}"
    );
    drop(fragile);

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
