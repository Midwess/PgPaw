use pgpaw::{EmbeddedPrimarySource, PgPaw, PgSource};
use tokio_postgres::NoTls;

fn ensure_runtime_dir() {
    if std::env::var_os("PGLITE_RUNTIME_DIR").is_none() {
        std::env::set_var(
            "PGLITE_RUNTIME_DIR",
            concat!(env!("CARGO_MANIFEST_DIR"), "/target/pglite-rt"),
        );
    }
}

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

async fn open_primary(dir: &std::path::Path, port: u16) -> PgPaw {
    PgPaw::builder()
        .source(PgSource::primary(EmbeddedPrimarySource {
            data_dir: dir.to_path_buf(),
            database: "schema_test".into(),
            listen_addresses: "127.0.0.1".into(),
            port,
            min_connections: 0,
            max_connections: 5,
        }))
        .open()
        .await
        .expect("open primary")
}

#[tokio::test]
#[serial_test::serial]
async fn migrations_apply_once_reflect_and_stay_immutable() {
    ensure_runtime_dir();
    let port: u16 = 41000 + (std::process::id() % 15000) as u16;
    let dir = tempfile::tempdir().unwrap();
    let world = tempfile::tempdir().unwrap();
    write(
        world.path(),
        "apps/todo/migrations/0001_init.sql",
        "CREATE TABLE todos (id text primary key, body text not null)",
    );
    write(
        world.path(),
        "apps/todo/migrations/0002_flag.sql",
        "ALTER TABLE todos ADD COLUMN done boolean not null default false",
    );

    let pgpaw = open_primary(&dir.path().join("data"), port).await;
    let schema = pgpaw.schema_ops();
    let (chains, warnings) = pgpaw::schema::discover_migrations(world.path(), "w").unwrap();
    assert!(warnings.is_empty());
    assert_eq!(chains.len(), 1);
    assert_eq!(chains[0].files.len(), 2);

    let report = schema.apply_migrations("w", &chains).await.unwrap();
    assert_eq!(report.applied.len(), 2);
    assert_eq!(report.already_applied, 0);

    let rerun = schema.apply_migrations("w", &chains).await.unwrap();
    assert_eq!(rerun.applied.len(), 0);
    assert_eq!(rerun.already_applied, 2);

    let tables = schema
        .reflect_tables(&["todos".to_string()])
        .await
        .unwrap();
    assert_eq!(tables[0].columns.len(), 3);
    assert_eq!(tables[0].primary_key, vec!["id"]);

    write(
        world.path(),
        "apps/todo/migrations/0001_init.sql",
        "CREATE TABLE todos (id text primary key)",
    );
    let (mutated, _) = pgpaw::schema::discover_migrations(world.path(), "w").unwrap();
    let error = schema.apply_migrations("w", &mutated).await.unwrap_err();
    assert!(
        error.to_string().contains("immutable"),
        "mutated applied migration must be refused: {error}"
    );

    pgpaw.shutdown().await.unwrap();
}

#[tokio::test]
#[serial_test::serial]
async fn legacy_ledger_handoff_is_atomic_and_rerun_safe() {
    ensure_runtime_dir();
    let port: u16 = 42000 + (std::process::id() % 15000) as u16;
    let dir = tempfile::tempdir().unwrap();

    let pgpaw = open_primary(&dir.path().join("data"), port).await;
    let dsn = format!("postgres://postgres@127.0.0.1:{port}/schema_test");
    let (client, connection) = tokio_postgres::connect(&dsn, NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
        .batch_execute(
            "CREATE TABLE applied_app_migrations ( \
               world_id text NOT NULL DEFAULT '', app text NOT NULL, filename text NOT NULL, \
               checksum text NOT NULL, applied_at timestamptz NOT NULL DEFAULT now(), \
               PRIMARY KEY (world_id, app, filename)); \
             INSERT INTO applied_app_migrations (world_id, app, filename, checksum) \
             VALUES ('w', 'todo', '0001_init.sql', 'abc123'); \
             CREATE TABLE app_schemas ( \
               world_id text NOT NULL DEFAULT '', app text NOT NULL, \
               tables jsonb NOT NULL DEFAULT '[]'::jsonb, \
               applied_at timestamptz NOT NULL DEFAULT now(), PRIMARY KEY (world_id, app)); \
             INSERT INTO app_schemas (world_id, app, tables) \
             VALUES ('w', 'todo', '[\"todos\"]'::jsonb)",
        )
        .await
        .unwrap();

    let schema = pgpaw.schema_ops();
    schema.handoff_legacy_ledgers().await.unwrap();

    let migrated = client
        .query_one(
            "SELECT checksum FROM pgpaw_applied_migrations \
             WHERE world_id = 'w' AND app = 'todo' AND filename = '0001_init.sql'",
            &[],
        )
        .await
        .unwrap();
    let checksum: String = migrated.get(0);
    assert_eq!(checksum, "abc123");
    assert!(
        client
            .query_one("SELECT 1 FROM applied_app_migrations LIMIT 1", &[])
            .await
            .is_err(),
        "legacy migration ledger is dropped in the same transaction"
    );
    assert!(
        client
            .query_one("SELECT 1 FROM app_schemas LIMIT 1", &[])
            .await
            .is_err(),
        "legacy schema ledger is dropped in the same transaction"
    );

    schema.handoff_legacy_ledgers().await.unwrap();

    client
        .batch_execute(
            "CREATE TABLE applied_app_migrations ( \
               world_id text NOT NULL DEFAULT '', app text NOT NULL, filename text NOT NULL, \
               checksum text NOT NULL, applied_at timestamptz NOT NULL DEFAULT now(), \
               PRIMARY KEY (world_id, app, filename)); \
             INSERT INTO applied_app_migrations (world_id, app, filename, checksum) \
             VALUES ('w', 'todo', '0001_init.sql', 'DIFFERENT')",
        )
        .await
        .unwrap();
    let error = schema.handoff_legacy_ledgers().await.unwrap_err();
    assert!(
        error.to_string().contains("checksums"),
        "conflicting checksum must roll the handoff back: {error}"
    );
    let survived = client
        .query_one("SELECT count(*)::int8 FROM applied_app_migrations", &[])
        .await
        .unwrap();
    let survived: i64 = survived.get(0);
    assert_eq!(
        survived, 1,
        "a failed handoff leaves the legacy ledger untouched"
    );

    pgpaw.shutdown().await.unwrap();
}
