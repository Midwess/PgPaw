use harness::{mint, Server, Upstream, JWT_SECRET};
use serde_json::json;

const FAR_EXP: i64 = 4_070_908_800;

fn ids(rows: &[serde_json::Value]) -> Vec<i64> {
    let mut out: Vec<i64> = rows.iter().map(|r| r["id"].as_i64().unwrap()).collect();
    out.sort_unstable();
    out
}

#[tokio::test]
async fn security_ddl_after_launch_propagates_and_reclassifies() {
    let up = Upstream::start().await;

    // docs starts PUBLIC: RLS off, granted PUBLIC.
    up.run_sql(
        "CREATE TABLE docs (id int PRIMARY KEY, org_id int, title text);
         INSERT INTO docs VALUES
           (101,1,'A-doc-one'),(102,1,'A-doc-two'),
           (201,2,'B-doc-one'),(202,2,'B-doc-two'),(203,2,'B-doc-three');
         GRANT SELECT ON docs TO PUBLIC;",
    )
    .await;
    // The DDL event trigger makes later schema changes replicate online.
    up.install_ddl_trigger().await;

    let server = Server::start(&up, Some(JWT_SECRET)).await;
    server
        .wait_rows("select * from docs order by id", None, 5)
        .await;

    // Baseline: docs is public, served token-free as a redirect.
    let baseline = server.query("select * from docs order by id", None).await;
    assert_eq!(baseline.status().as_u16(), 303, "docs starts public");

    // Apply security DDL AFTER the server is already running.
    up.run_sql(
        "CREATE ROLE member LOGIN;
         GRANT SELECT ON docs TO member;
         ALTER TABLE docs ENABLE ROW LEVEL SECURITY;
         ALTER TABLE docs FORCE ROW LEVEL SECURITY;
         CREATE POLICY docs_by_org ON docs FOR SELECT TO member
           USING ( org_id = ((select current_setting('request.jwt.claims', true))::json->>'org_id')::int );",
    )
    .await;

    // Propagation: the no-token query flips public(303) -> private(401) with no restart.
    server
        .wait_status("select * from docs order by id", None, 401, 90)
        .await;

    let token_a = mint(JWT_SECRET, json!({"role":"member","org_id":1,"exp":FAR_EXP}));
    let token_b = mint(JWT_SECRET, json!({"role":"member","org_id":2,"exp":FAR_EXP}));

    server
        .wait_rows("select * from docs order by id", Some(&token_a), 2)
        .await;
    let a_rows = server
        .rows("select * from docs order by id", Some(&token_a))
        .await;
    assert_eq!(ids(&a_rows), vec![101, 102], "A sees only org 1");

    server
        .wait_rows("select * from docs order by id", Some(&token_b), 3)
        .await;
    let b_rows = server
        .rows("select * from docs order by id", Some(&token_b))
        .await;
    assert_eq!(ids(&b_rows), vec![201, 202, 203], "B sees only org 2");

    // Bidirectional: disabling RLS upstream re-widens the query back to public.
    up.run_sql("ALTER TABLE docs DISABLE ROW LEVEL SECURITY")
        .await;
    server
        .wait_status("select * from docs order by id", None, 303, 90)
        .await;
}
