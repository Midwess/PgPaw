use harness::{Server, Upstream, JWT_SECRET};

#[tokio::test]
async fn quoted_pascalcase_table_is_queryable() {
    let up = Upstream::start().await;
    up.run_sql(
        "CREATE TABLE \"Workspace\" (id int PRIMARY KEY, name text);
         INSERT INTO \"Workspace\" VALUES (1,'alpha');
         GRANT SELECT ON \"Workspace\" TO PUBLIC;",
    )
    .await;

    let server = Server::start(&up, Some(JWT_SECRET)).await;
    server
        .wait_rows("select id from \"Workspace\"", None, 1)
        .await;

    let resp = server.query("select * from \"Workspace\"", None).await;
    assert_eq!(
        resp.status().as_u16(),
        303,
        "a quoted PascalCase table must be recognised as replicated, not rejected"
    );

    let rows = server.rows("select * from \"Workspace\"", None).await;
    assert_eq!(rows.len(), 1, "the quoted table's row is queryable");
}
