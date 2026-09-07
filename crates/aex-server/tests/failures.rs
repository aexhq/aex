mod common;
use aex_server::{
    App,
    config::Config,
    operator::{Operation, execute},
};
use axum::{Router, body::Body, http::Request, routing::post};
use http_body_util::BodyExt;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tower::ServiceExt;

#[tokio::test]
async fn lost_upstream_create_response_does_not_create_a_second_resource() {
    let created = Arc::new(AtomicUsize::new(0));
    let calls = created.clone();
    let upstream = Router::new().route(
        "/v1/sessions",
        post(move || {
            let calls = calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                (
                    axum::http::StatusCode::BAD_GATEWAY,
                    "response lost after creation",
                )
            }
        }),
    );
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", socket.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(socket, upstream).await.unwrap() });
    let directory = tempfile::tempdir().unwrap();
    let mut config: Config =
        serde_json::from_str(include_str!("../../../examples/config.json")).unwrap();
    config.data_dir = directory.path().to_path_buf();
    config.brain_url = url;
    config.limits.minimum_free_disk_bytes = 1;
    let app = App::open(
        config,
        "b".repeat(32),
        "o".repeat(32),
        common::database(directory.path()).await,
        "s".repeat(32),
    )
    .await
    .unwrap();
    let account = execute(&app, Operation::CreateAccount).await.unwrap()["account"]
        .as_str()
        .unwrap()
        .to_string();
    let token = execute(&app, Operation::IssueKey { account })
        .await
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    execute(
        &app,
        Operation::ReportUsage {
            observed_at: aex_server::store::now(),
            sessions: Default::default(),
        },
    )
    .await
    .unwrap();
    app.accepting.store(true, Ordering::SeqCst);
    let body = serde_json::json!({"agentloop":{"implementation":{"type":"brain_component","entrypoint":"turn","id":"0".repeat(64)},"configuration":{},"environment":"brain"},"model":{"provider":"openai","name":"gpt-4.1-mini","api_key":"model-secret"},"tools":[],"environments":[{"name":"brain","driver":"brain"}]});
    for _ in 0..2 {
        let response = aex_server::http::router(app.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/sessions")
                    .header("authorization", format!("Bearer {token}"))
                    .header("idempotency-key", "create-once")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response.status().is_server_error());
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert!(!String::from_utf8_lossy(&bytes).contains("model-secret"));
    }
    assert_eq!(created.load(Ordering::SeqCst), 1);
    let owned: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
        .fetch_one(&app.store.0)
        .await
        .unwrap();
    assert_eq!(owned, 0);
    let claim: String = sqlx::query_scalar("SELECT fingerprint FROM claims")
        .fetch_one(&app.store.0)
        .await
        .unwrap();
    assert_eq!(claim.len(), 64);
    server.abort();
}
