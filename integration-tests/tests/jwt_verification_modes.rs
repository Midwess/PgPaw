use harness::{mint, Server, Upstream, JWT_SECRET};
use serde_json::json;

const FAR_EXP: i64 = 4_070_908_800;
const PAST_EXP: i64 = 100;
const ALG_NONE: &str = "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJyb2xlIjoibWVtYmVyIiwib3JnX2lkIjoxLCJleHAiOjQwNzA5MDg4MDB9.";

async fn seed(up: &Upstream) {
    up.run_sql(
        "CREATE TABLE pub_t (id int PRIMARY KEY, v text);
         INSERT INTO pub_t VALUES (1,'pub-one'),(2,'pub-two');
         GRANT SELECT ON pub_t TO PUBLIC;
         CREATE ROLE member LOGIN;
         CREATE TABLE secret_t (id int PRIMARY KEY, org_id int, v text);
         INSERT INTO secret_t VALUES (11,1,'org1-a'),(12,1,'org1-b');
         ALTER TABLE secret_t ENABLE ROW LEVEL SECURITY;
         GRANT SELECT ON secret_t TO member;
         CREATE POLICY secret_by_org ON secret_t FOR SELECT TO member
           USING ( org_id = ((select current_setting('request.jwt.claims', true))::json->>'org_id')::int );",
    )
    .await;
}

#[tokio::test]
async fn verifier_accepts_only_well_formed_tokens() {
    let up = Upstream::start().await;
    seed(&up).await;
    let server = Server::start(&up, Some(JWT_SECRET)).await;

    let valid = mint(JWT_SECRET, json!({"role":"member","org_id":1,"exp":FAR_EXP}));
    let expired = mint(JWT_SECRET, json!({"role":"member","org_id":1,"exp":PAST_EXP}));
    let bad_sig = mint("wrong-secret", json!({"role":"member","org_id":1,"exp":FAR_EXP}));
    let missing_role = mint(JWT_SECRET, json!({"org_id":1,"exp":FAR_EXP}));

    server
        .wait_rows("select * from secret_t order by id", Some(&valid), 2)
        .await;

    let q = "select * from secret_t order by id";
    assert_eq!(
        server.query(q, Some(&valid)).await.status().as_u16(),
        200,
        "valid HS256 accepted"
    );
    assert_eq!(server.query(q, Some(&expired)).await.status().as_u16(), 401);
    assert_eq!(server.query(q, Some(&bad_sig)).await.status().as_u16(), 401);
    assert_eq!(
        server.query(q, Some(ALG_NONE)).await.status().as_u16(),
        401,
        "alg:none downgrade rejected"
    );
    assert_eq!(
        server.query(q, Some(&missing_role)).await.status().as_u16(),
        401,
        "missing role claim rejected"
    );

    assert_eq!(
        server.query_auth(q, &valid).await.status().as_u16(),
        401,
        "missing Bearer prefix is malformed"
    );
    assert_eq!(
        server
            .query_auth(q, "Basic dXNlcjpwYXNz")
            .await
            .status()
            .as_u16(),
        401,
        "non-Bearer scheme is malformed"
    );

    assert_eq!(
        server
            .query("select * from pub_t order by id", None)
            .await
            .status()
            .as_u16(),
        303,
        "public query needs no token even on a jwt server"
    );
}
