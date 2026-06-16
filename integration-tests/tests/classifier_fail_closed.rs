use harness::{cache_control, mint, Server, Upstream, JWT_SECRET};
use serde_json::json;

const FAR_EXP: i64 = 4_070_908_800;

#[tokio::test]
async fn classifier_fails_closed_on_every_edge() {
    let up = Upstream::start().await;

    up.run_sql(
        "CREATE TABLE open_t (id int PRIMARY KEY, v text);
         INSERT INTO open_t VALUES (1,'o-one'),(2,'o-two');
         GRANT SELECT ON open_t TO PUBLIC;

         CREATE TABLE pub_t2 (id int PRIMARY KEY, v text);
         INSERT INTO pub_t2 VALUES (1,'q-one'),(2,'q-two');
         GRANT SELECT ON pub_t2 TO PUBLIC;

         CREATE ROLE member LOGIN;

         CREATE TABLE rls_nopolicy (id int PRIMARY KEY, v text);
         INSERT INTO rls_nopolicy VALUES (1,'secret-a'),(2,'secret-b');
         ALTER TABLE rls_nopolicy ENABLE ROW LEVEL SECURITY;
         GRANT SELECT ON rls_nopolicy TO member;

         CREATE TABLE secret_t (id int PRIMARY KEY, org_id int, v text);
         INSERT INTO secret_t VALUES (1,1,'s-one'),(2,1,'s-two');
         ALTER TABLE secret_t ENABLE ROW LEVEL SECURITY;
         GRANT SELECT ON secret_t TO member;
         CREATE POLICY secret_by_org ON secret_t FOR SELECT TO member
           USING ( org_id = ((select current_setting('request.jwt.claims', true))::json->>'org_id')::int );

         CREATE TABLE dup_t (id int PRIMARY KEY, v text);
         INSERT INTO dup_t VALUES (1,'public-dup');
         GRANT SELECT ON dup_t TO PUBLIC;
         CREATE SCHEMA s2;
         CREATE TABLE s2.dup_t (id int PRIMARY KEY, v text);
         ALTER TABLE s2.dup_t ENABLE ROW LEVEL SECURITY;",
    )
    .await;

    up.install_ddl_trigger().await;

    let server = Server::start(&up, Some(JWT_SECRET)).await;
    server
        .wait_rows("select * from open_t order by id", None, 2)
        .await;

    let token_a = mint(JWT_SECRET, json!({"role":"member","org_id":1,"exp":FAR_EXP}));

    let control = server.query("select * from open_t order by id", None).await;
    assert_eq!(control.status().as_u16(), 303, "open_t is genuinely public");

    let nopolicy_anon = server
        .query("select * from rls_nopolicy order by id", None)
        .await;
    assert_eq!(nopolicy_anon.status().as_u16(), 401, "rls table needs a token");
    let nopolicy_auth = server
        .query("select * from rls_nopolicy order by id", Some(&token_a))
        .await;
    assert_eq!(nopolicy_auth.status().as_u16(), 200);
    assert_eq!(cache_control(&nopolicy_auth), "private, no-store");
    let nopolicy_rows = harness::as_array(nopolicy_auth).await;
    assert_eq!(nopolicy_rows.len(), 0, "no policy = deny-all, not a leak");

    let mixed = server
        .query(
            "select o.id from open_t o join secret_t s on s.id = o.id",
            None,
        )
        .await;
    assert_eq!(mixed.status().as_u16(), 401, "private table taints the join");

    let dup = server.query("select * from dup_t", None).await;
    assert_eq!(
        dup.status().as_u16(),
        401,
        "colliding relname resolves private"
    );

    let unknown = server.query("select * from does_not_exist", None).await;
    assert_eq!(unknown.status().as_u16(), 400, "unknown table is rejected");

    let before = server.query("select * from pub_t2 order by id", None).await;
    assert_eq!(before.status().as_u16(), 303, "pub_t2 starts public");
    up.run_sql("REVOKE SELECT ON pub_t2 FROM PUBLIC").await;
    server
        .wait_status("select * from pub_t2 order by id", None, 401, 90)
        .await;
}
