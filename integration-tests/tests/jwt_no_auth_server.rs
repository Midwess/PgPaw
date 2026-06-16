use harness::{mint, Server, Upstream, JWT_SECRET};
use serde_json::json;

const FAR_EXP: i64 = 4_070_908_800;

#[tokio::test]
async fn server_without_jwt_config_fails_closed_but_serves_public() {
    let up = Upstream::start().await;
    up.run_sql(
        "CREATE TABLE pub_t (id int PRIMARY KEY, v text);
         INSERT INTO pub_t VALUES (1,'pub-one'),(2,'pub-two');
         GRANT SELECT ON pub_t TO PUBLIC;
         CREATE ROLE member LOGIN;
         CREATE TABLE secret_t (id int PRIMARY KEY, org_id int, v text);
         INSERT INTO secret_t VALUES (11,1,'org1-a');
         ALTER TABLE secret_t ENABLE ROW LEVEL SECURITY;
         GRANT SELECT ON secret_t TO member;
         CREATE POLICY secret_by_org ON secret_t FOR SELECT TO member
           USING ( org_id = ((select current_setting('request.jwt.claims', true))::json->>'org_id')::int );",
    )
    .await;

    let server = Server::start(&up, None).await;
    server
        .wait_rows("select * from pub_t order by id", None, 2)
        .await;

    let token = mint(JWT_SECRET, json!({"role":"member","org_id":1,"exp":FAR_EXP}));

    let presented = server
        .query("select * from secret_t order by id", Some(&token))
        .await;
    assert_eq!(
        presented.status().as_u16(),
        401,
        "token to unconfigured verifier is rejected"
    );

    let public = server.query("select * from pub_t order by id", None).await;
    assert_eq!(public.status().as_u16(), 303, "public path needs no auth");
}
