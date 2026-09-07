pub async fn database(path: &std::path::Path) -> String {
    let origin = std::env::var("AEX_TEST_DATABASE_URL")
        .expect("AEX_TEST_DATABASE_URL is required; tests use real PostgreSQL");
    let schema = format!(
        "test_{}",
        aex_server::identity::digest(path.to_string_lossy().as_bytes())
    );
    let pool = sqlx::PgPool::connect(&origin).await.unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE SCHEMA IF NOT EXISTS {schema}"
    )))
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;
    let mut url = url::Url::parse(&origin).unwrap();
    url.query_pairs_mut()
        .append_pair("options", &format!("-csearch_path={schema}"));
    url.into()
}
