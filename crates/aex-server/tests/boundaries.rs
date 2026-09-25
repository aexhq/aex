mod common;
use aex_server::{
    App,
    config::Config,
    identity::{Principal, digest},
    operator::{Operation, execute},
    store::{Claim, Store},
};
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
};
use tower::ServiceExt;

fn config(path: &std::path::Path) -> Config {
    let mut config: Config =
        serde_json::from_str(include_str!("../../../examples/config.json")).unwrap();
    config.data_dir = path.to_path_buf();
    config.limits.minimum_free_disk_bytes = 1;
    config
}
async fn app() -> (tempfile::TempDir, App, Principal, String) {
    let directory = tempfile::tempdir().unwrap();
    let app = App::open(
        config(directory.path()),
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
    let key = execute(
        &app,
        Operation::IssueKey {
            account: account.clone(),
        },
    )
    .await
    .unwrap();
    let token = key["token"].as_str().unwrap().to_string();
    let p = app.store.principal(&token).await.unwrap();
    execute(
        &app,
        Operation::ReportUsage {
            observed_at: aex_server::store::now(),
            sessions: Default::default(),
        },
    )
    .await
    .unwrap();
    app.accepting
        .store(true, std::sync::atomic::Ordering::SeqCst);
    (directory, app, p, token)
}
fn summary(id: &str) -> brain_protocol::SessionSummary {
    brain_protocol::SessionSummary {
        session_id: brain_protocol::SessionId::new(id),
        status: brain_protocol::SessionStatus::Idle,
        last_sequence: 1,
    }
}

#[tokio::test]
async fn rejected_turn_keys_retry_after_restart_but_unknown_dispatches_do_not() {
    use axum::{
        Json, Router,
        routing::{get, post},
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let (dir, mut app, p, _) = app().await;
    sqlx::query(
        "INSERT INTO sessions(id,account,created,active_key) VALUES('session',$1,$2,'busy')",
    )
    .bind(&p.account)
    .bind(aex_server::store::now())
    .execute(&app.store.0)
    .await
    .unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let calls = count.clone();
    let upstream = Router::new()
        .route("/v1/sessions/session", get(|| async { Json(summary("session")) }))
        .route("/v1/sessions/session/messages", post(move || {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                match n {
                    0 => (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({"code":"overloaded","message":"busy","retryable":true,"details":{"admission":"rejected"}}))),
                    1 => (StatusCode::ACCEPTED, Json(serde_json::json!({"sequence":2}))),
                    _ => (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"code":"ambiguous","message":"lost response","retryable":false}))),
                }
            }
        }));
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    Arc::make_mut(&mut app.config).brain_url = format!("http://{}", socket.local_addr().unwrap());
    app.brain = aex_server::brain::Brain::new(&app.config, "b".repeat(32)).unwrap();
    let server = tokio::spawn(async move { axum::serve(socket, upstream).await.unwrap() });
    let headers = |key: &str| {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("prefer", "respond-async".parse().unwrap());
        headers.insert("idempotency-key", key.parse().unwrap());
        headers
    };
    let body = axum::body::Bytes::from_static(br#"{"input":{"message":"read"}}"#);
    assert!(
        aex_server::turns::send(&app, &p, "session", &headers("retry"), body.clone())
            .await
            .is_err()
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
    app.store.finish_turn("session").await.unwrap();
    let rejected = aex_server::turns::send(&app, &p, "session", &headers("retry"), body.clone())
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT state FROM claims WHERE client_key='retry'")
            .fetch_one(&app.store.0)
            .await
            .unwrap(),
        "rejected"
    );
    app.store = Store::open(&common::database(dir.path()).await)
        .await
        .unwrap();
    let changed = axum::body::Bytes::from_static(br#"{"input":{"message":"write"}}"#);
    assert!(
        aex_server::turns::send(&app, &p, "session", &headers("retry"), changed)
            .await
            .is_err()
    );
    for _ in 0..2 {
        assert_eq!(
            aex_server::turns::send(&app, &p, "session", &headers("retry"), body.clone())
                .await
                .unwrap()
                .status(),
            StatusCode::ACCEPTED
        );
    }
    assert_eq!(count.load(Ordering::SeqCst), 2);
    app.store.finish_turn("session").await.unwrap();
    for _ in 0..2 {
        assert_eq!(
            aex_server::turns::send(&app, &p, "session", &headers("uncertain"), body.clone())
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_GATEWAY
        );
    }
    assert_eq!(count.load(Ordering::SeqCst), 3);
    server.abort();
}

#[tokio::test]
async fn operation_claims_are_account_scoped_durable_and_never_resend_pending_work() {
    let (dir, app, p, _) = app().await;
    let a2 = execute(&app, Operation::CreateAccount).await.unwrap()["account"]
        .as_str()
        .unwrap()
        .to_string();
    let q = Principal {
        account: a2,
        key: p.key.clone(),
    };
    let Claim::New(first) = app
        .store
        .claim(&p, "create", "same", "request", &app.config.limits)
        .await
        .unwrap()
    else {
        panic!()
    };
    let Claim::New(second) = app
        .store
        .claim(&q, "create", "same", "request", &app.config.limits)
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_ne!(first, second);
    assert_eq!(
        app.store
            .claim(&p, "create", "same", "different", &app.config.limits)
            .await
            .err()
            .unwrap()
            .0,
        StatusCode::CONFLICT
    );
    let reopened = Store::open(&common::database(dir.path()).await)
        .await
        .unwrap();
    assert_eq!(
        reopened
            .claim(&p, "create", "same", "request", &app.config.limits)
            .await
            .err()
            .unwrap()
            .1
            .code,
        "ambiguous"
    );
    app.store
        .complete_create(&p, &first, &summary("ses_test"), 100)
        .await
        .unwrap();
    assert!(matches!(
        reopened
            .claim(&p, "create", "same", "request", &app.config.limits)
            .await
            .unwrap(),
        Claim::Complete(_)
    ));
    assert!(app.store.owned(&q, "ses_test", false).await.is_err());
}

#[tokio::test]
async fn concurrent_create_reservations_cannot_exceed_account_limit() {
    let (_dir, app, p, _) = app().await;
    let mut limits = app.config.limits.clone();
    limits.sessions_per_account = 1;
    let (first, second) = tokio::join!(
        app.store.claim(&p, "create", "a", "a", &limits),
        app.store.claim(&p, "create", "b", "b", &limits)
    );
    assert_ne!(first.is_ok(), second.is_ok());
}

#[tokio::test]
async fn keys_and_hosts_follow_revocation_and_suspension_after_restart() {
    let (dir, app, p, token) = app().await;
    let host = brain_protocol::HostRegistration {
        host_id: brain_protocol::HostId::new("host_test"),
        token: "host-secret".into(),
    };
    app.store.register_host(&p, &host).await.unwrap();
    assert!(app.store.host("host_test", "wrong").await.is_err());
    assert!(app.store.host("host_test", "host-secret").await.is_ok());
    execute(&app, Operation::RevokeKey { key: p.key })
        .await
        .unwrap();
    let reopened = Store::open(&common::database(dir.path()).await)
        .await
        .unwrap();
    assert!(reopened.principal(&token).await.is_err());
    assert!(reopened.host("host_test", "host-secret").await.is_err());
    let values: Vec<String> = sqlx::query_scalar("SELECT verifier FROM api_keys")
        .fetch_all(&reopened.0)
        .await
        .unwrap();
    assert_eq!(values, [digest(token.as_bytes())]);
}

#[tokio::test]
async fn every_session_route_denies_another_account_before_contacting_brain() {
    let (_dir, app, p, token) = app().await;
    let Claim::New(key) = app
        .store
        .claim(&p, "create", "x", "x", &app.config.limits)
        .await
        .unwrap()
    else {
        panic!()
    };
    app.store
        .complete_create(&p, &key, &summary("ses_owned"), 10)
        .await
        .unwrap();
    let account = execute(&app, Operation::CreateAccount).await.unwrap()["account"]
        .as_str()
        .unwrap()
        .to_string();
    let other = execute(&app, Operation::IssueKey { account })
        .await
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    for (method, suffix) in [
        ("GET", ""),
        ("GET", "/transcript"),
        ("GET", "/events"),
        ("POST", "/messages"),
        ("POST", "/cancel"),
        ("POST", "/end"),
        ("POST", "/environments"),
        ("DELETE", ""),
    ] {
        let response = aex_server::http::router(app.clone())
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(format!("/v1/sessions/ses_owned{suffix}"))
                    .header("authorization", format!("Bearer {other}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 404, "{method} {suffix}");
    }
    for path in [
        "/operate",
        "/v1/sessions/ses_owned/executions/1/call",
        "/v1/sessions/%2E%2E/hosts",
    ] {
        let response = aex_server::http::router(app.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 404, "{path}");
    }
}

#[tokio::test]
async fn deleting_retains_ownership_and_denies_new_work_until_confirmed() {
    let (_dir, app, p, _) = app().await;
    let Claim::New(key) = app
        .store
        .claim(&p, "create", "x", "x", &app.config.limits)
        .await
        .unwrap()
    else {
        panic!()
    };
    app.store
        .complete_create(&p, &key, &summary("ses_owned"), 10)
        .await
        .unwrap();
    app.store.mark_deleting("ses_owned").await.unwrap();
    assert!(app.store.owned(&p, "ses_owned", false).await.is_err());
    assert!(app.store.owned(&p, "ses_owned", true).await.is_ok());
    app.store.finish_delete("ses_owned").await.unwrap();
    assert!(!app.store.mark_deleting("ses_owned").await.unwrap());
}

#[tokio::test]
async fn stale_meter_and_disk_pressure_fail_admission_without_resetting_reservations() {
    let (_dir, app, p, _) = app().await;
    assert!(aex_server::admission::disk(&app).await.is_ok());
    sqlx::query("UPDATE storage_report SET received=0")
        .execute(&app.store.0)
        .await
        .unwrap();
    assert_eq!(
        aex_server::admission::disk(&app).await.err().unwrap().0,
        503
    );
    let Claim::New(key) = app
        .store
        .claim(&p, "create", "x", "x", &app.config.limits)
        .await
        .unwrap()
    else {
        panic!()
    };
    app.store
        .complete_create(&p, &key, &summary("ses_owned"), 10)
        .await
        .unwrap();
    app.store
        .reserve_turn(&p, "ses_owned", "turn", &app.config.limits)
        .await
        .unwrap();
    assert!(
        app.store
            .reserve_turn(&p, "ses_owned", "turn", &app.config.limits)
            .await
            .is_err()
    );
    assert!(
        app.store
            .reserve_turn(&p, "ses_owned", "different", &app.config.limits)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn operator_interface_requires_its_own_credential() {
    let (_dir, app, _, token) = app().await;
    for token in [token, "wrong".into()] {
        let response = aex_server::operator::router(app.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/operate")
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::from(r#"{"action":"create_account"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 401);
    }
}

#[test]
fn configuration_and_routes_fail_closed() {
    let mut config = config(std::path::Path::new("data"));
    assert!(config.validate().is_ok());
    config.operator_listen = "0.0.0.0:8082".parse().unwrap();
    assert!(config.validate().is_err());
    for path in [
        "/v1/sessions/x/../hosts",
        "/v1/sessions/x?admin=true",
        "//v1/sessions",
        "/v1/sessions/x/unknown",
    ] {
        assert!(aex_server::http::route(&Method::GET, path).is_err());
    }
}

#[tokio::test]
async fn hosted_policy_delegates_models_and_rejects_customer_code_endpoints_and_ambient_grants() {
    let (_dir, app, p, _) = app().await;
    let base = serde_json::json!({"agentloop":{"implementation":{"type":"brain_component","entrypoint":"turn","id":"0".repeat(64)},"configuration":{},"environment":"brain"},"model":{"provider":"openai","name":"gpt-4.1-mini","api_key":"model-secret"},"tools":[],"environments":[{"name":"brain","lifecycle":"automatic", "driver":"brain"}]});
    for provider in ["openai", "deepseek", "customer-provider"] {
        let mut request = base.clone();
        request["model"]["provider"] = serde_json::json!(provider);
        let request = serde_json::from_value(request).unwrap();
        assert!(
            aex_server::admission::create(&app, &p, &request)
                .await
                .is_ok(),
            "Brain validates {provider}"
        );
    }
    for (pointer, value) in [
        (
            "/environments",
            serde_json::json!([{"name":"brain","driver":"brain","lifecycle":"automatic","template":{"max_instances":2,"configuration_schema":true}}]),
        ),
        (
            "/environments",
            serde_json::json!([{"name":"brain","driver":"brain","lifecycle":"automatic","methods":{"inspect":{"effect":"replace","description":"Inspect","input_schema":true}}}]),
        ),
        (
            "/environments",
            serde_json::json!([{"name":"brain","driver":"brain"}]),
        ),
        (
            "/environments",
            serde_json::json!([{"name":"brain","lifecycle":"automatic", "driver":"brain","configuration":{"network":["http://169.254.169.254"]}}]),
        ),
        (
            "/agentloop/implementation/id",
            serde_json::json!("f".repeat(64)),
        ),
        (
            "/environments",
            serde_json::json!([{"name":"brain","lifecycle":"automatic", "driver":"brain","configuration":{"filesystem":{"workspace":"write"}}}]),
        ),
        (
            "/environments",
            serde_json::json!([{"name":"brain","lifecycle":"automatic", "driver":"brain","configuration":{"secrets":["AEX_OPERATOR_TOKEN"]}}]),
        ),
        (
            "/environments",
            serde_json::json!([{"name":"brain","lifecycle":"automatic", "driver":"http","url":"http://169.254.169.254"}]),
        ),
        (
            "/environments",
            serde_json::json!([{"name":"brain","lifecycle":"automatic", "driver":"host","host_id":"host_another_account"}]),
        ),
    ] {
        let mut value_request = base.clone();
        *value_request.pointer_mut(pointer).unwrap() = value;
        let request = serde_json::from_value(value_request).unwrap();
        assert!(
            aex_server::admission::create(&app, &p, &request)
                .await
                .is_err(),
            "{pointer}"
        );
    }
}

#[tokio::test]
async fn a_storage_scan_cannot_release_an_active_or_newer_turn_reservation() {
    let (_dir, app, p, _) = app().await;
    let Claim::New(key) = app
        .store
        .claim(&p, "create", "x", "x", &app.config.limits)
        .await
        .unwrap()
    else {
        panic!()
    };
    app.store
        .complete_create(&p, &key, &summary("ses_owned"), 10)
        .await
        .unwrap();
    app.store
        .reserve_turn(&p, "ses_owned", "turn", &app.config.limits)
        .await
        .unwrap();
    let reserved: i64 =
        sqlx::query_scalar("SELECT retained_bytes FROM sessions WHERE id='ses_owned'")
            .fetch_one(&app.store.0)
            .await
            .unwrap();
    execute(
        &app,
        Operation::ReportUsage {
            observed_at: aex_server::store::now(),
            sessions: std::collections::BTreeMap::from([("ses_owned".into(), 0)]),
        },
    )
    .await
    .unwrap();
    let after: i64 = sqlx::query_scalar("SELECT retained_bytes FROM sessions WHERE id='ses_owned'")
        .fetch_one(&app.store.0)
        .await
        .unwrap();
    assert_eq!(after, reserved);
    assert!(after >= app.config.limits.turn_reserve_bytes as i64);
}
