//! Static-reference availability selected only by the live feature lane.

#[tokio::test]
async fn generated_docs_remain_available_without_a_product_api() {
    let base = aex_test_harness::required_env!("AEX_SITE_URL");
    let response = reqwest::get(format!("{}/docs", base.trim_end_matches('/')))
        .await
        .expect("deployed docs must be reachable");
    assert!(
        response.status().is_success(),
        "docs returned {}",
        response.status()
    );
    let body = response.text().await.expect("docs body must be readable");
    assert!(body.contains("AEX API reference"));
}
