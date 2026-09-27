mod common;
use aex_server::{App, config::Config, identity::Principal, store};
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    http::{HeaderMap, Method, Request, StatusCode},
    routing::{get, post},
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tower::ServiceExt;

async fn fixture() -> (tempfile::TempDir, App, Principal, String, Arc<AtomicUsize>) {
    let dir = tempfile::tempdir().unwrap();
    let mut config: Config =
        serde_json::from_str(include_str!("../../../examples/config.json")).unwrap();
    config.data_dir = dir.path().into();
    config.limits.minimum_free_disk_bytes = 1;
    config.agentloops.insert("0".repeat(64));
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let hosts = Arc::new(AtomicUsize::new(0));
    let upstream = Router::new()
        .route(
            "/v1/hosts",
            post(move || {
                let id = hosts.fetch_add(1, Ordering::SeqCst);
                async move { Json(json!({"host_id":format!("host_{id}"),"token":"h".repeat(32)})) }
            }),
        )
        .route(
            "/v1/sessions",
            post(move |Json(value): Json<Value>| {
                counter.fetch_add(1, Ordering::SeqCst);
                async move {
                    assert_eq!(value["model"]["api_key"], "private-model-key");
                    Json(json!({"session_id":"session","status":"idle","last_sequence":1}))
                }
            }),
        )
        .route(
            "/v1/sessions/session",
            get(|| async {
                Json(json!({"session_id":"session","status":"idle","last_sequence":1}))
            }),
        )
        .route(
            "/v1/callback",
            post(|headers: HeaderMap, Json(value): Json<Value>| async move {
                assert_eq!(headers["authorization"], "Bearer completion-capability");
                assert_eq!(value["method"], "finish");
                Json(42)
            }),
        );
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    config.brain_url = format!("http://{}", socket.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(socket, upstream).await.unwrap() });
    let mut app = App::open(
        config,
        "b".repeat(32),
        "o".repeat(32),
        common::database(dir.path()).await,
        "s".repeat(32),
    )
    .await
    .unwrap();
    let account = app.store.create_account(&app.config.limits).await.unwrap();
    let (key, token) = app
        .store
        .issue_key(&account, &app.config.limits)
        .await
        .unwrap();
    app.accepting.store(true, Ordering::SeqCst);
    app.store
        .report_usage(store::now(), Default::default())
        .await
        .unwrap();
    app.environment_token = Some("e".repeat(32));
    Arc::make_mut(&mut app.config).http_environments = Some(serde_json::from_value(json!({"url":app.config.brain_url,"public_url":"https://api.aex.dev/environments/http","bindings":{}})).unwrap());
    // The runtime owns the short-lived fixture task; each test uses a separate listener and schema.
    drop(task);
    (dir, app, Principal { account, key }, token, calls)
}
fn session() -> Value {
    json!({"agentloop":{"implementation":{"type":"brain_component","entrypoint":"turn","id":"0".repeat(64)},"configuration":{},"environment":"brain"},
        "model":{"provider":"openai","name":"gpt-4.1-mini","api_key":"private-model-key"},"tools":[],
        "environments":[{"name":"brain","lifecycle":"automatic","driver":"brain"},
            {"name":"tab","lifecycle":"automatic","driver":"host","host_id":"placeholder","configuration":{}}]})
}
async fn call(
    app: &App,
    method: Method,
    path: &str,
    token: &str,
    origin: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .header("idempotency-key", "fixture");
    if let Some(origin) = origin {
        req = req.header("origin", origin);
    }
    let response = aex_server::http::router(app.clone())
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1_048_576).await.unwrap();
    (
        status,
        if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        },
    )
}

#[tokio::test]
async fn browser_access_fixes_one_composition_origin_host_and_session_without_disclosing_model_keys()
 {
    let (_dir, app, p, token, calls) = fixture().await;
    let origin = "https://customer.example";
    let (status, grant) = call(
        &app,
        Method::POST,
        "/v1/clients",
        &token,
        None,
        json!({"origin":origin,"expires_at":store::now()+3600,"session":session()}),
    )
    .await;
    assert!(status.is_success(), "{grant}");
    assert!(!grant.to_string().contains("private-model-key"));
    let access = grant["token"].as_str().unwrap();
    let row: String =
        sqlx::query_scalar("SELECT row_to_json(client_grants)::text FROM client_grants")
            .fetch_one(&app.store.0)
            .await
            .unwrap();
    assert!(!row.contains("private-model-key"));
    assert!(!row.contains(access));
    let mut request = session();
    request["model"]["api_key"] = json!(access);
    request["environments"][1]["host_id"] = grant["credentials"]["hostId"].clone();
    for wrong in [None, Some("https://attacker.example")] {
        assert!(
            !call(
                &app,
                Method::POST,
                "/v1/sessions",
                access,
                wrong,
                request.clone()
            )
            .await
            .0
            .is_success()
        );
    }
    let mut altered = request.clone();
    altered["model"]["name"] = json!("other-model");
    assert!(
        !call(
            &app,
            Method::POST,
            "/v1/sessions",
            access,
            Some(origin),
            altered
        )
        .await
        .0
        .is_success()
    );
    let (status, created) = call(
        &app,
        Method::POST,
        "/v1/sessions",
        access,
        Some(origin),
        request.clone(),
    )
    .await;
    assert!(status.is_success(), "{created}");
    assert!(app.pending_clients.lock().await.is_empty());
    let (status, replayed) = call(
        &app,
        Method::POST,
        "/v1/sessions",
        access,
        Some(origin),
        request,
    )
    .await;
    assert!(status.is_success(), "{replayed}");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        call(
            &app,
            Method::GET,
            "/v1/sessions/session",
            access,
            Some(origin),
            Value::Null
        )
        .await
        .0
        .is_success()
    );
    for path in [
        "/v1/sessions",
        "/v1/sessions/other",
        "/v1/models",
        "/v1/environments",
    ] {
        assert!(
            !call(&app, Method::GET, path, access, Some(origin), Value::Null)
                .await
                .0
                .is_success(),
            "{path}"
        );
    }
    assert!(
        !call(
            &app,
            Method::POST,
            "/v1/tools",
            access,
            Some(origin),
            json!({})
        )
        .await
        .0
        .is_success()
    );
    let host = grant["credentials"]["hostId"].as_str().unwrap();
    assert!(
        !call(
            &app,
            Method::POST,
            &format!("/v1/hosts/{host}/results"),
            &"h".repeat(32),
            Some(origin),
            json!({"session_id":"another"})
        )
        .await
        .0
        .is_success()
    );
    let headers = HeaderMap::from_iter([("origin".parse().unwrap(), origin.parse().unwrap())]);
    aex_server::clients::revoke(&app, &p, grant["id"].as_str().unwrap())
        .await
        .unwrap();
    assert!(
        aex_server::clients::access(&app, Some(access), None, &headers)
            .await
            .is_err()
    );
    assert!(
        aex_server::clients::access(&app, None, Some(host), &headers)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn browser_pending_credentials_are_not_recreated_after_restart() {
    let (_dir, app, _p, token, calls) = fixture().await;
    let (status, grant) = call(&app, Method::POST, "/v1/clients", &token, None,
        json!({"origin":"https://customer.example","expires_at":store::now()+60,"session":session()})).await;
    assert!(status.is_success(), "{grant}");
    app.pending_clients.lock().await.clear();
    let mut request = session();
    request["model"]["api_key"] = grant["token"].clone();
    request["environments"][1]["host_id"] = grant["credentials"]["hostId"].clone();
    assert_eq!(
        call(
            &app,
            Method::POST,
            "/v1/sessions",
            grant["token"].as_str().unwrap(),
            Some("https://customer.example"),
            request
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn application_declarations_replace_operator_bindings_and_keep_secrets_out_of_product_rows() {
    use aex_server::environments::{application, http};
    use axum::extract::State;
    let (_dir, app, p, _token, _) = fixture().await;
    let mut value = session();
    value["environments"][1] = json!({"name":"application","lifecycle":"automatic","driver":"http", "url":"https://api.aex.dev/environments/application",
        "credential":"private-application-credential-value", "configuration":{"type":"application","endpoint":"https://customer.example/tools","timeoutMs":30000}});
    let mut request: brain_protocol::CreateSessionRequest = serde_json::from_value(value).unwrap();
    aex_server::admission::create(&app, &p, &request)
        .await
        .unwrap();
    let mut tx = app.store.0.begin().await.unwrap();
    let claim = app
        .store
        .claim(&p, "create", "create", "fixed", &app.config.limits)
        .await
        .unwrap();
    let store::Claim::New(operation) = claim else {
        panic!()
    };
    aex_server::environments::reserve_in(&app, &mut tx, &p, &operation, &mut request, None)
        .await
        .unwrap();
    http::reserve_in(&app, &mut tx, &p, &operation, &mut request)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let row: String = sqlx::query_scalar("SELECT row_to_json(http_grants)::text FROM http_grants")
        .fetch_one(&app.store.0)
        .await
        .unwrap();
    assert!(!row.contains("private-application-credential-value"));
    assert!(row.contains("customer.example"));
    let env = &request.environments[1];
    let id = env.configuration["authorization"].as_str().unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        format!("Bearer {}", "e".repeat(32)).parse().unwrap(),
    );
    let input = json!({"sessionId":"session","environment":"application", "configuration":env.configuration, "authorization":id});
    let admitted = http::authorize(
        State(app.clone()),
        headers.clone(),
        Json(serde_json::from_value(input.clone()).unwrap()),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(admitted["url"], "https://customer.example/tools");
    assert_eq!(
        admitted["callbackUrl"],
        "https://api.aex.dev/v1/environments/application/callback"
    );
    assert!(admitted.get("token").is_none());
    let mut substituted = input;
    substituted["authorization"] = json!("another-grant");
    assert!(
        http::authorize(
            State(app.clone()),
            headers,
            Json(serde_json::from_value(substituted).unwrap())
        )
        .await
        .is_err()
    );
    let mut invalid = request.environments[1].clone();
    invalid.configuration =
        json!({"type":"application","endpoint":"http://private.example/tools","timeoutMs":30000});
    assert!(application::selection(&invalid).is_err());
    // Completions remain available while ordinary request capacity is occupied and the service drains.
    let _held = app
        .requests
        .clone()
        .acquire_many_owned(app.config.limits.requests as u32)
        .await
        .unwrap();
    app.accepting.store(false, Ordering::SeqCst);
    let (status, sequence) = call(
        &app,
        Method::POST,
        "/v1/environments/application/callback",
        "completion-capability",
        None,
        json!({"method":"finish","input":null}),
    )
    .await;
    assert!(status.is_success());
    assert_eq!(sequence, 42);
}
