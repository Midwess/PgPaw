use harness::{cache_control, Server, Upstream, JWT_SECRET};

fn etag(resp: &reqwest::Response) -> String {
    resp.headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn public_query_is_a_content_addressed_redirect_cache() {
    let up = Upstream::start().await;
    up.run_sql(
        "CREATE TABLE items (id int PRIMARY KEY, name text);
         INSERT INTO items VALUES (1,'alpha'),(2,'beta');
         GRANT SELECT ON items TO PUBLIC;",
    )
    .await;

    let server = Server::start(&up, Some(JWT_SECRET)).await;
    server
        .wait_rows("select id, name from items order by id", None, 2)
        .await;

    let sql = "select id, name from items order by id";

    // 303 -> /q/{hash}/{version}
    let r1 = server.query(sql, None).await;
    assert_eq!(r1.status().as_u16(), 303);
    let loc1 = r1
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(loc1.starts_with("/q/"), "redirect target shape, got {loc1}");

    // Followed snapshot is cacheable, has an ETag, returns the rows.
    let snap1 = server.cursor(&loc1).await;
    assert_eq!(snap1.status().as_u16(), 200);
    assert!(cache_control(&snap1).contains("public, max-age=259200"));
    let etag1 = etag(&snap1);
    assert!(!etag1.is_empty(), "snapshot carries an ETag");
    let body1 = harness::as_array(snap1).await;
    assert_eq!(body1.len(), 2);

    // Idempotency: identical SQL -> identical /q/{hash}/{version}.
    let r2 = server.query(sql, None).await;
    let loc2 = r2.headers().get("location").unwrap().to_str().unwrap().to_string();
    assert_eq!(loc1, loc2, "content-addressed hash is stable for identical SQL");

    // ETag stable across identical fetches.
    let snap2 = server.cursor(&loc1).await;
    assert_eq!(etag(&snap2), etag1, "ETag stable for unchanged content");

    // Unknown cursor -> 404, not 500/hang.
    let unknown = server.cursor("/q/deadbeefdeadbeef/0").await;
    assert_eq!(unknown.status().as_u16(), 404);

    // Upstream insert bumps the version into a new snapshot URL with the new row.
    up.run_sql("INSERT INTO items VALUES (3,'gamma')").await;
    server.wait_rows(sql, None, 3).await;
    let r3 = server.query(sql, None).await;
    let loc3 = r3.headers().get("location").unwrap().to_str().unwrap().to_string();
    assert_ne!(loc1, loc3, "upstream change bumps the version");
    let body3 = harness::as_array(server.cursor(&loc3).await).await;
    assert_eq!(body3.len(), 3, "new snapshot includes the inserted row");

    // The old snapshot URL is immutable: still the old body, or evicted (404) — never mutated.
    let old = server.cursor(&loc1).await;
    let status = old.status().as_u16();
    if status == 200 {
        let body = harness::as_array(old).await;
        assert_eq!(body.len(), 2, "old snapshot must not gain the new row");
    } else {
        assert_eq!(status, 404, "old snapshot either immutable or evicted");
    }
}
