//! Dashboard authorization-negative checks selected only by the live feature lane.

#[tokio::test]
async fn an_off_allowlist_passthrough_path_is_not_found() {
    let base = aex_test_harness::required_env!("AEX_DASHBOARD_URL");
    let response = reqwest::get(format!(
        "{}/api/v1/central/not-allowlisted",
        base.trim_end_matches('/')
    ))
    .await
    .expect("dashboard must return an authorization-negative response");
    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
}
