#![cfg(all(feature = "az-wire", unix))]

use std::sync::Arc;
use std::time::{Duration, Instant};

use az_wire::{
    handler, http, Handler, HandlerError, Node, ParentLink, Reply, Request, SendExt, TopologyConfig,
};
use futures_util::future::try_join_all;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const WARMUP: usize = 200;
const SEQUENTIAL: usize = 2_000;
const CONCURRENCY: usize = 32;
const CONCURRENT: usize = 4_096;

#[derive(Deserialize, Serialize, JsonSchema)]
struct ReadRequest {
    sql: String,
}

#[handler]
async fn read(request: Request<ReadRequest>) -> Result<Reply<Value>, HandlerError> {
    Ok(Reply::new(json!({
        "rows": [{"id": 1, "name": "benchmark"}],
        "sql": request.into_payload().sql,
        "version": 1
    })))
}

struct Measurement {
    elapsed: Duration,
    latencies: Vec<Duration>,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "manual topology performance check"]
async fn native_topologies_have_no_gross_adapter_regression() {
    let direct_service = service("direct-service");
    let direct = caller("direct-caller");
    direct.link(&direct_service).await.unwrap();
    let direct_result = measure(&direct).await;

    let public_service = service("public-service");
    let hosting = public_service
        .host(([127, 0, 0, 1], 0))
        .without_webtransport()
        .start()
        .await
        .unwrap();
    let public = caller("public-caller");
    let transport =
        az_wire_client::dial_transport(&format!("ws://{}", hosting.websocket_addr().unwrap()))
            .await
            .unwrap();
    public
        .connect_transport("public-service", transport)
        .await
        .unwrap();
    let public_result = measure(&public).await;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("parent.sock");
    let unix_service = service("unix-service");
    let unix_hosting =
        unix_service.host_unix(az_wire_transport::unix::UnixListener::bind(&path).unwrap());
    let unix = caller("unix-caller");
    let topology = unix
        .start_topology(TopologyConfig::parent(ParentLink::unix(
            "unix-service",
            &path,
        )))
        .await
        .unwrap();
    let unix_result = measure(&unix).await;

    report("direct in-process", &direct_result);
    report("public native websocket", &public_result);
    report("unix parent routing", &unix_result);

    assert_gross_regression("public native websocket", &direct_result, &public_result);
    assert_gross_regression("unix parent routing", &direct_result, &unix_result);

    topology.shutdown().await.unwrap();
    unix_hosting.shutdown().await.unwrap();
    hosting.shutdown().await.unwrap();
}

fn service(name: &str) -> std::sync::Arc<Node> {
    Node::builder(name)
        .service(read.at_subject("pgpaw.read"))
        .insecure_accept_declared_peer_identities()
        .build()
        .unwrap()
}

fn caller(name: &str) -> std::sync::Arc<Node> {
    Node::builder(name)
        .insecure_accept_declared_peer_identities()
        .build()
        .unwrap()
}

async fn measure(node: &Arc<Node>) -> Measurement {
    for _ in 0..WARMUP {
        request(node).await;
    }

    let started = Instant::now();
    let mut latencies = Vec::with_capacity(SEQUENTIAL);
    for _ in 0..SEQUENTIAL {
        let request_started = Instant::now();
        request(node).await;
        latencies.push(request_started.elapsed());
    }

    for _ in 0..CONCURRENT / CONCURRENCY {
        try_join_all((0..CONCURRENCY).map(|_| async {
            request(node).await;
            Ok::<(), ()>(())
        }))
        .await
        .unwrap();
    }

    Measurement {
        elapsed: started.elapsed(),
        latencies,
    }
}

async fn request(node: &Arc<Node>) {
    let payload = serde_json::to_value(ReadRequest {
        sql: "benchmark-read".into(),
    })
    .unwrap();
    let response = http::Request::post("/pgpaw.read")
        .body(payload)
        .send(node)
        .await
        .unwrap();
    let response: Value = serde_json::from_slice(response.body()).unwrap();
    assert_eq!(response["version"], 1);
}

fn report(name: &str, result: &Measurement) {
    let mut samples = result.latencies.clone();
    samples.sort_unstable();
    let percentile = |percent: usize| samples[(samples.len() - 1) * percent / 100];
    let operations = SEQUENTIAL + CONCURRENT;
    println!(
        "{name:<24} {:>9.0} req/s p50={:>8.1}us p95={:>8.1}us p99={:>8.1}us",
        operations as f64 / result.elapsed.as_secs_f64(),
        percentile(50).as_secs_f64() * 1_000_000.0,
        percentile(95).as_secs_f64() * 1_000_000.0,
        percentile(99).as_secs_f64() * 1_000_000.0,
    );
}

fn assert_gross_regression(name: &str, direct: &Measurement, candidate: &Measurement) {
    let direct_mean = direct.latencies.iter().sum::<Duration>() / direct.latencies.len() as u32;
    let candidate_mean =
        candidate.latencies.iter().sum::<Duration>() / candidate.latencies.len() as u32;
    assert!(
        candidate_mean < direct_mean * 100,
        "{name} mean latency {candidate_mean:?} exceeds the gross-regression limit relative to {direct_mean:?}"
    );
}
