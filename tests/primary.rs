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

    let db = open_primary(&PrimaryConfig {
        data_dir: dir,
        listen_addresses: "127.0.0.1".into(),
        port,
        max_connections: 5,
    })
    .await
    .expect("open primary over TCP");

    let dsn = format!("postgres://postgres@127.0.0.1:{port}/postgres");
    let (client, connection) = tokio_postgres::connect(&dsn, NoTls)
        .await
        .expect("tcp connect to primary");
    tokio::spawn(async move {
        let _ = connection.await;
    });

    client
        .batch_execute("CREATE TABLE t (id int primary key); INSERT INTO t VALUES (1)")
        .await
        .unwrap();
    let rows = client.query("SELECT id FROM t", &[]).await.unwrap();
    assert_eq!(rows[0].get::<_, i32>(0), 1);

    db.close().await.unwrap();
}
