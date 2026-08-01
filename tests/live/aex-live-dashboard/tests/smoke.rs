//! Deployed dashboard smoke selected only by the live feature lane.

#[tokio::test]
async fn health_is_reachable_without_a_database_probe() {
    let base = aex_test_harness::required_env!("AEX_DASHBOARD_URL");
    let expected = aex_test_harness::required_env!("AEX_EXPECTED_BUILD_ID");
    let response = reqwest::get(format!("{}/api/health", base.trim_end_matches('/')))
        .await
        .expect("dashboard health must be reachable");
    assert!(
        response.status().is_success(),
        "health returned {}",
        response.status()
    );
    let body = response.text().await.expect("health body must be readable");
    assert!(
        body.contains(&expected),
        "health does not carry expected build identity"
    );
}
