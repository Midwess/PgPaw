use harness::{cache_control, mint, Server, Upstream, JWT_SECRET};
use serde_json::json;

const FAR_EXP: i64 = 4_070_908_800;
const PAST_EXP: i64 = 100;

fn ids(rows: &[serde_json::Value]) -> Vec<i64> {
    let mut out: Vec<i64> = rows.iter().map(|r| r["id"].as_i64().unwrap()).collect();
    out.sort_unstable();
    out
}

#[tokio::test]
async fn jwt_scoped_query_enforces_upstream_rls() {
    let up = Upstream::start().await;
    assert_eq!(up.setting("wal_level").await, "logical");

    up.run_sql(
        "CREATE TABLE orgs (id int PRIMARY KEY, name text);
         INSERT INTO orgs VALUES (1,'Acme'),(2,'Globex');
         CREATE TABLE documents (id int PRIMARY KEY, org_id int REFERENCES orgs(id), title text);
         INSERT INTO documents VALUES
           (101,1,'A-doc-one'),(102,1,'A-doc-two'),
           (201,2,'B-doc-one'),(202,2,'B-doc-two'),(203,2,'B-doc-three');
         CREATE ROLE member LOGIN;
         GRANT SELECT ON orgs TO PUBLIC;
         GRANT SELECT ON documents TO member;
         ALTER TABLE documents ENABLE ROW LEVEL SECURITY;
         ALTER TABLE documents FORCE ROW LEVEL SECURITY;
         CREATE POLICY documents_by_org ON documents FOR SELECT TO member
           USING ( org_id = ((select current_setting('request.jwt.claims', true))::json->>'org_id')::int );",
    )
    .await;

    let server = Server::start(&up, Some(JWT_SECRET)).await;

    let token_a = mint(
        JWT_SECRET,
        json!({"role":"member","org_id":1,"exp":FAR_EXP}),
    );
    let token_b = mint(
        JWT_SECRET,
        json!({"role":"member","org_id":2,"exp":FAR_EXP}),
    );
    let token_expired = mint(
        JWT_SECRET,
        json!({"role":"member","org_id":1,"exp":PAST_EXP}),
    );

    server
        .wait_rows("select * from documents order by id", Some(&token_a), 2)
        .await;

    let public = server
        .query("select id, name from orgs order by id", None)
        .await;
    assert_eq!(
        public.status().as_u16(),
        303,
        "public orgs query must redirect"
    );
    let location = public
        .headers()
        .get("location")
        .expect("303 carries Location")
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        location.starts_with("/q/"),
        "Location must be /q/{{hash}}/{{version}}, got {location}"
    );
    let snapshot = server.cursor(&location).await;
    assert_eq!(snapshot.status().as_u16(), 200);
    assert!(
        cache_control(&snapshot).contains("public, max-age=259200"),
        "followed snapshot must be CDN-cacheable"
    );
    let orgs = harness::as_array(snapshot).await;
    assert_eq!(orgs.len(), 2, "both orgs visible publicly");

    let a_resp = server
        .query("select * from documents order by id", Some(&token_a))
        .await;
    assert_eq!(a_resp.status().as_u16(), 200, "private query is inline 200");
    assert_eq!(cache_control(&a_resp), "private, no-store");
    let a_rows = harness::as_array(a_resp).await;
    assert_eq!(ids(&a_rows), vec![101, 102], "A sees only its rows");

    let a_join = server
        .rows(
            "select d.id, d.title, o.name from documents d join orgs o on o.id = d.org_id order by d.id",
            Some(&token_a),
        )
        .await;
    assert_eq!(ids(&a_join), vec![101, 102], "JOIN does not leak B's rows");
    assert!(
        a_join.iter().all(|r| r["name"] == "Acme"),
        "A only joins to its own org"
    );

    let b_rows = server
        .rows("select * from documents order by id", Some(&token_b))
        .await;
    assert_eq!(ids(&b_rows), vec![201, 202, 203], "B sees only its rows");

    let no_token = server
        .query("select * from documents order by id", None)
        .await;
    assert_eq!(
        no_token.status().as_u16(),
        401,
        "private query needs a token"
    );

    let expired = server
        .query("select * from documents order by id", Some(&token_expired))
        .await;
    assert_eq!(expired.status().as_u16(), 401, "expired token rejected");

    let private_live = server
        .live("select * from documents order by id", Some(&token_a))
        .await;
    assert_eq!(
        private_live.status().as_u16(),
        200,
        "access-controlled queries stream live under the token's role"
    );
    let private_ctype = private_live
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(
        private_ctype.starts_with("text/event-stream"),
        "private live opens an SSE stream, got {private_ctype}"
    );

    let public_live = server
        .live("select id, name from orgs order by id", None)
        .await;
    assert_eq!(public_live.status().as_u16(), 200);
    let ctype = public_live
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(
        ctype.starts_with("text/event-stream"),
        "public live opens an SSE stream, got {ctype}"
    );
}
