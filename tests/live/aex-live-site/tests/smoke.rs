//! Deployed static-site smoke selected only by the live feature lane.

#[tokio::test]
async fn deployed_site_reports_the_expected_release_identity() {
    let base = aex_test_harness::required_env!("AEX_SITE_URL");
    let expected = aex_test_harness::required_env!("AEX_EXPECTED_BUILD_ID");
    let response = reqwest::get(format!("{}/", base.trim_end_matches('/')))
        .await
        .expect("deployed site must be reachable");
    assert!(
        response.status().is_success(),
        "site returned {}",
        response.status()
    );
    let body = response.text().await.expect("site body must be readable");
    assert!(
        body.contains(&expected),
        "site does not carry expected build identity"
    );
}
