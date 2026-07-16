#![cfg(feature = "az-wire")]

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;

use harness::{Upstream, PUBLICATION};

fn free_address() -> SocketAddr {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind ephemeral port");
    let port = listener.local_addr().expect("local addr").port();
    drop(listener);
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
}

#[tokio::test(flavor = "multi_thread")]
async fn replica_with_az_wire_host_binds_serves_and_releases() {
    let up = Upstream::start().await;
    up.run_sql("CREATE TABLE items (id int PRIMARY KEY); INSERT INTO items VALUES (1)")
        .await;
    up.run_sql(&format!("CREATE PUBLICATION {PUBLICATION} FOR ALL TABLES"))
        .await;
    let data = tempfile::tempdir().expect("tempdir");
    let address = free_address();
    let mut host = az_wire::HostConfig::new(address);
    host.webtransport = false;

    let pgpaw = pgpaw::PgPaw::builder()
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
        .az_wire(
            az_wire::NodeBuilder::new("pgpaw").insecure_accept_declared_peer_identities(),
            az_wire::TopologyConfig::host(host),
        )
        .open()
        .await
        .expect("open replica with az-wire host binding");

    assert!(
        TcpListener::bind(address).is_err(),
        "az-wire host must hold its listener while running"
    );

    pgpaw.shutdown().await.expect("shutdown");

    TcpListener::bind(address).expect("az-wire listener must be released after shutdown");
}
