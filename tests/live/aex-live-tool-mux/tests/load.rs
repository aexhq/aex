//! Environment-driven private service load sanity.

#[tokio::test]
async fn readiness_remains_bounded_under_parallel_health_reads() {
    let Ok(base) = std::env::var(aex_live_tool_mux::URL_ENV) else {
        return;
    };
    let client = reqwest::Client::new();
    let requests = (0..64).map(|_| {
        let client = client.clone();
        let url = format!("{}/internal/readyz", base.trim_end_matches('/'));
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
