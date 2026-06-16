use harness::{run_and_capture_error, Upstream};

#[tokio::test]
async fn jwks_url_fails_loud_at_startup() {
    let up = Upstream::start().await;
    up.run_sql("CREATE TABLE t (id int PRIMARY KEY); GRANT SELECT ON t TO PUBLIC")
        .await;
    let error = run_and_capture_error(&up, None, Some("https://example.test/jwks")).await;
    assert!(
        matches!(error, pgpaw::CacheError::Config(_)),
        "jwks-url must be a Config error, got {error:?}"
    );
    assert!(
        error.to_string().to_lowercase().contains("jwks"),
        "error should mention JWKS, got: {error}"
    );
}
