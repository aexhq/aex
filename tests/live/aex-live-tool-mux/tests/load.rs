//! Environment-driven private service load sanity.

#[tokio::test]
async fn readiness_remains_bounded_under_parallel_health_reads() {
    let base = aex_test_harness::required_env!(aex_live_tool_mux::URL_ENV)
        .trim_end_matches('/')
        .to_owned();
    let client = reqwest::Client::new();
    let requests = (0..64).map(|_| {
        let client = client.clone();
        let url = format!("{base}/internal/readyz");
        tokio::spawn(async move { client.get(url).send().await })
    });
    for request in requests {
        let response = request
            .await
            .expect("request task")
            .expect("private service reachable");
        assert!(response.status().is_success());
    }
}
