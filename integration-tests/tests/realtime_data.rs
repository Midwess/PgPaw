use std::time::{Duration, Instant};

use harness::{Server, Upstream, JWT_SECRET};

fn version_of(location: &str) -> String {
    location.rsplit('/').next().unwrap().to_string()
}

async fn stream_contains(resp: &mut reqwest::Response, needle: &str, secs: u64) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut buf = String::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        match tokio::time::timeout(remaining, resp.chunk()).await {
            Ok(Ok(Some(bytes))) => {
                buf.push_str(&String::from_utf8_lossy(&bytes));
                if buf.contains(needle) {
                    return true;
                }
            }
            _ => return false,
        }
    }
}

#[tokio::test]
async fn live_stream_pushes_delta_on_upstream_insert() {
    let up = Upstream::start().await;
    up.run_sql(
        "CREATE TABLE events (id int PRIMARY KEY, label text);
         INSERT INTO events VALUES (1,'one'),(2,'two');
         GRANT SELECT ON events TO PUBLIC;",
    )
    .await;

    let server = Server::start(&up, Some(JWT_SECRET)).await;
    server
        .wait_rows("select id, label from events order by id", None, 2)
        .await;

    // Pre-insert version baseline.
    let baseline = server.query("select * from events order by id", None).await;
    assert_eq!(baseline.status().as_u16(), 303);
    let base_version = version_of(
        baseline
            .headers()
            .get("location")
            .unwrap()
            .to_str()
            .unwrap(),
    );

    // Open the live stream.
    let mut stream = server
        .live("select id, label from events order by id", None)
        .await;
    assert_eq!(stream.status().as_u16(), 200);
    let ctype = stream
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(
        ctype.starts_with("text/event-stream"),
        "live opens an SSE stream, got {ctype}"
    );

    // Mutate upstream while the stream is open, then observe the delta.
    up.run_sql("INSERT INTO events VALUES (3,'live-three')")
        .await;
    assert!(
        stream_contains(&mut stream, "live-three", 15).await,
        "live stream did not surface the inserted row within 15s"
    );

    // Non-live read reflects the new row under a bumped version.
    server
        .wait_rows("select * from events order by id", None, 3)
        .await;
    let after = server.query("select * from events order by id", None).await;
    assert_eq!(after.status().as_u16(), 303);
    let after_location = after.headers().get("location").unwrap().to_str().unwrap();
    assert_ne!(
        version_of(after_location),
        base_version,
        "an upstream insert must bump the snapshot version"
    );
    let rows = server.rows("select * from events order by id", None).await;
    assert_eq!(rows.len(), 3, "the inserted row is now queryable");
}
