use std::time::{Duration, Instant};

use harness::{Server, Upstream, JWT_SECRET};

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
async fn join_select_star_is_rejected_and_join_delete_carries_the_row() {
    let up = Upstream::start().await;
    up.run_sql(
        "CREATE TABLE workspaces (id int PRIMARY KEY, name text);
         CREATE TABLE sandbox (id int PRIMARY KEY, workspace_id int, status text);
         INSERT INTO workspaces VALUES (1,'alpha');
         INSERT INTO sandbox VALUES (1,1,'running'),(2,1,'doomed-marker');
         GRANT SELECT ON workspaces TO PUBLIC;
         GRANT SELECT ON sandbox TO PUBLIC;",
    )
    .await;

    let server = Server::start(&up, Some(JWT_SECRET)).await;
    server.wait_rows("select id from sandbox", None, 2).await;

    let rejected = server
        .query(
            "select * from sandbox s join workspaces w on s.workspace_id = w.id",
            None,
        )
        .await;
    assert_eq!(
        rejected.status().as_u16(),
        400,
        "SELECT * across a join yields two `id` columns and must be rejected, not silently collapsed"
    );
    let body = rejected.text().await.unwrap();
    assert!(
        body.contains("more than one column named"),
        "rejection should name the duplicate column; got {body}"
    );

    let mut stream = server
        .live(
            "select s.id as id, s.status as status, w.name as name \
             from sandbox s join workspaces w on s.workspace_id = w.id order by s.id",
            None,
        )
        .await;
    assert_eq!(stream.status().as_u16(), 200);
    assert!(
        stream_contains(&mut stream, "snapshot", 10).await,
        "live join must open with a snapshot frame"
    );

    up.run_sql("DELETE FROM sandbox WHERE id = 2").await;
    assert!(
        stream_contains(&mut stream, "doomed-marker", 15).await,
        "a join delete keys by row_hash, so the delete delta must carry the removed row \
         for the client to match it by getKey"
    );
}
