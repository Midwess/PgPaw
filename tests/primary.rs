use pgpaw::{open_primary, PrimaryConfig};
use tokio_postgres::NoTls;

#[cfg(feature = "az-wire")]
use az_wire::{Node, ParentLink, TopologyConfig};
#[cfg(feature = "az-wire")]
use futures_util::StreamExt;
#[cfg(feature = "az-wire")]
use pgpaw::wire::{LiveEvent, LIVE_SUBJECT, READ_SUBJECT};
#[cfg(feature = "az-wire")]
use serde_json::json;
#[cfg(feature = "az-wire")]
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
#[cfg(feature = "az-wire")]
use pgpaw::EmbeddedVerifierConfig;

#[tokio::test]
#[serial_test::serial]
async fn primary_serves_writable_postgres_over_tcp() {
    if std::env::var_os("PGLITE_RUNTIME_DIR").is_none() {
        std::env::set_var(
            "PGLITE_RUNTIME_DIR",
            concat!(env!("CARGO_MANIFEST_DIR"), "/target/pglite-rt"),
        );
    }
    let port: u16 = 50000 + (std::process::id() % 15000) as u16;
    let dir = std::env::temp_dir().join(format!("pgpaw-primary-{}-{port}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    let database = "worldant_test";
    let db = open_primary(&PrimaryConfig {
        data_dir: dir,
        database: database.into(),
        listen_addresses: "127.0.0.1".into(),
        port,
        min_connections: 0,
        max_connections: 5,
        #[cfg(feature = "az-wire")]
        verifier: None,
    })
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

    db.shutdown().await.unwrap();
}

#[tokio::test]
#[serial_test::serial]
async fn primary_rejects_invalid_connection_bounds_before_starting() {
    let dir = std::env::temp_dir().join(format!(
        "pgpaw-primary-invalid-bounds-{}",
        std::process::id()
    ));
    let result = open_primary(&PrimaryConfig {
        data_dir: dir.clone(),
        database: "postgres".into(),
        listen_addresses: String::new(),
        port: 0,
        min_connections: 2,
        max_connections: 1,
        #[cfg(feature = "az-wire")]
        verifier: None,
    })
    .await;
    let error = match result {
        Ok(handle) => {
            handle.shutdown().await.unwrap();
            panic!("invalid connection bounds must fail");
        }
        Err(error) => error,
    };

    assert!(error.to_string().contains("primary connections require"));
    assert!(!dir.exists());
}

#[cfg(all(feature = "az-wire", unix))]
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn embedded_child_reads_configured_database_and_observes_external_commits() {
    if std::env::var_os("PGLITE_RUNTIME_DIR").is_none() {
        std::env::set_var(
            "PGLITE_RUNTIME_DIR",
            concat!(env!("CARGO_MANIFEST_DIR"), "/target/pglite-rt"),
        );
    }
    let port: u16 = 35000 + (std::process::id() % 15000) as u16;
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("parent.sock");
    let parent = Node::builder("worldant")
        .insecure_accept_declared_peer_identities()
        .build()
        .unwrap();
    let hosting = parent.host_unix(az_wire_transport::unix::UnixListener::bind(&socket).unwrap());
    let mut primary = open_primary(&PrimaryConfig {
        data_dir: dir.path().join("primary"),
        database: "configured".into(),
        listen_addresses: "127.0.0.1".into(),
        port,
        min_connections: 0,
        max_connections: 5,
        verifier: Some(EmbeddedVerifierConfig {
            jwt_secret: "embedded-secret".into(),
            role_claim: "role".into(),
        }),
    })
    .await
    .unwrap();
    assert!(primary.dsn().contains("/configured"));
    let external_dsn = format!("postgres://postgres@127.0.0.1:{port}/configured");
    let (client, connection) = tokio_postgres::connect(&external_dsn, NoTls).await.unwrap();
    tokio::spawn(async move { let _ = connection.await; });
    client.batch_execute("CREATE TABLE items (id int primary key, name text); GRANT SELECT ON items TO PUBLIC; INSERT INTO items VALUES (1, 'first')").await.unwrap();
    client.batch_execute("CREATE ROLE authenticated; CREATE TABLE private_items (id int primary key, owner int, name text); ALTER TABLE private_items ENABLE ROW LEVEL SECURITY; GRANT SELECT ON private_items TO authenticated; CREATE POLICY private_owner ON private_items FOR SELECT TO authenticated USING (owner = ((current_setting('request.jwt.claims', true))::jsonb ->> 'owner')::int); INSERT INTO private_items VALUES (1, 7, 'allowed'), (2, 8, 'denied')").await.unwrap();

    primary
        .attach_child(
            "pgpaw",
            TopologyConfig::parent(ParentLink::unix("worldant", &socket)),
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if parent.reachable_names() == ["pgpaw.cursor", "pgpaw.live", "pgpaw.read"] {
                break;
            }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();

    let read = parent.request(READ_SUBJECT, json!({"sql": "SELECT id, name FROM items ORDER BY id", "bearer": null})).await.unwrap();
    let version = read["version"].as_u64().unwrap();
    let hash = read["hash"].as_str().unwrap();
    let cursor = parent.request("pgpaw.cursor", json!({"hash": hash, "version": version.to_string()})).await.unwrap();
    assert_eq!(cursor["rows"], json!([{"id": 1, "name": "first"}]));

    let token = encode(
        &Header::new(Algorithm::HS256),
        &json!({"role": "authenticated", "owner": 7, "exp": 4_102_444_800u64}),
        &EncodingKey::from_secret(b"embedded-secret"),
    ).unwrap();
    let protected = parent.request(READ_SUBJECT, json!({"sql": "SELECT id, name FROM private_items ORDER BY id", "bearer": token})).await.unwrap();
    assert_eq!(protected["rows"], json!([{"id": 1, "name": "allowed"}]));
    assert!(parent.request(READ_SUBJECT, json!({"sql": "SELECT id FROM private_items", "bearer": "invalid"})).await.is_err());
    assert!(parent.request(READ_SUBJECT, json!({"sql": "SELECT id FROM private_items", "bearer": null})).await.is_err());

    let mut live = parent.subscribe(LIVE_SUBJECT, json!({"sql": "SELECT id, name FROM items ORDER BY id", "bearer": null})).await.unwrap();
    let snapshot: LiveEvent = serde_json::from_slice(&live.next().await.unwrap().unwrap()).unwrap();
    assert!(matches!(snapshot, LiveEvent::Snapshot { .. }));
    assert_eq!(primary.live_subscription_count(), 1);
    client.execute("INSERT INTO items VALUES ($1, $2)", &[&2i32, &"second"]).await.unwrap();
    let update = tokio::time::timeout(std::time::Duration::from_secs(5), live.next()).await.unwrap().unwrap().unwrap();
    let update: LiveEvent = serde_json::from_slice(&update).unwrap();
    assert!(matches!(update, LiveEvent::Insert { ref row, .. } if row == &json!({"id": 2, "name": "second"})));

    drop(live);
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while primary.live_subscription_count() != 0 {
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    primary.shutdown().await.unwrap();
    hosting.shutdown().await.unwrap();
}
