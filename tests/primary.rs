use pgpaw::{open_primary, PrimaryConfig};
use tokio_postgres::NoTls;

#[tokio::test]
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
