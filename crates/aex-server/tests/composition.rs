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
const SESSION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const EXECUTOR: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
fn session() -> Value {
    json!({"harness":"pi","config":{"model":"openai/gpt-4.1-mini","tools":[]},"model_credential":{"api_key":"private-model-key"}})
}
fn tools() -> Value {
    json!([{"name":"lookup","description":"Look up","input_schema":{"type":"object"}}])
}
async fn fixture() -> (tempfile::TempDir, App, Principal, String, Arc<AtomicUsize>) {
    let dir = tempfile::tempdir().unwrap();
    let mut config: Config =
        serde_json::from_str(include_str!("../../../examples/config.json")).unwrap();
    config.data_dir = dir.path().into();
    config.limits.minimum_free_disk_bytes = 1;
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let saved = Arc::new(tokio::sync::Mutex::new(Value::Null));
    let read = saved.clone();
    let upstream=Router::new().route("/executors",post(||async {Json(json!({"id":EXECUTOR,"token":"h".repeat(32)}))}))
        .route("/sessions",post(move |headers:HeaderMap,Json(value):Json<Value>|{let saved=saved.clone();count.fetch_add(1,Ordering::SeqCst);async move {assert!(headers.contains_key("x-aex-tenant"));assert_eq!(value["model_credential"]["api_key"],"private-model-key");let result=json!({"id":SESSION,"harness":value["harness"],"config":value["config"],"status":"idle","descriptors":[],"created_at":"2026-10-20T00:00:00Z","updated_at":"2026-10-20T00:00:00Z"});*saved.lock().await=result.clone();Json(result)}}))
        .route(&format!("/sessions/{SESSION}"),get(move ||{let read=read.clone();async move {Json(read.lock().await.clone())}}));
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    config.brain_url = format!("http://{}", socket.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(socket, upstream).await.unwrap() });
    let app = App::open(
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
    (dir, app, Principal { account, key }, token, calls)
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
async fn retired_program_preparation_and_legacy_routes_are_rejected_before_dispatch() {
    let (_dir, app, _p, token, calls) = fixture().await;
    for path in [
        "/v1/programs",
        "/v1/artifacts",
        "/v1/brain-env/prepare",
        "/v1/hosts",
        "/v1/sessions",
    ] {
        assert_eq!(
            call(&app, Method::POST, path, &token, None, json!({}))
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn browser_access_fixes_one_composition_origin_executor_and_session_without_disclosing_model_keys()
 {
    let (_dir, app, p, token, calls) = fixture().await;
    let origin = "https://customer.example";
    let (status, grant) = call(
        &app,
        Method::POST,
        "/v1/clients",
        &token,
        None,
        json!({"origin":origin,"expires_at":store::now()+3600,"session":session(),"tools":tools()}),
    )
    .await;
    assert!(status.is_success(), "{grant}");
    assert!(!grant.to_string().contains("private-model-key"));
    assert!(grant.get("credentials").is_none());
    let access = grant["token"].as_str().unwrap();
    let row: String =
        sqlx::query_scalar("SELECT row_to_json(browser_grants)::text FROM browser_grants")
            .fetch_one(&app.store.0)
            .await
            .unwrap();
    assert!(!row.contains("private-model-key"));
    assert!(!row.contains(access));
    for wrong in [None, Some("https://attacker.example")] {
        assert!(
            !call(
                &app,
                Method::POST,
                "/executors",
                access,
                wrong,
                json!({"binding":"runtime","tools":tools()})
            )
            .await
            .0
            .is_success()
        );
    }
    let mut altered = tools();
    altered[0]["name"] = json!("other");
    assert!(
        !call(
            &app,
            Method::POST,
            "/executors",
            access,
            Some(origin),
            json!({"binding":"runtime","tools":altered})
        )
        .await
        .0
        .is_success()
    );
    let (status, registered) = call(
        &app,
        Method::POST,
        "/executors",
        access,
        Some(origin),
        json!({"binding":"runtime","tools":tools()}),
    )
    .await;
    assert!(status.is_success(), "{registered}");
    let mut create = grant["session"].clone();
    create["config"]["bindings"] =
        json!([{"alias":"runtime","executor":EXECUTOR,"runtime":tools()}]);
    let mut changed = create.clone();
    changed["config"]["model"] = json!("other");
    assert!(
        !call(
            &app,
            Method::POST,
            "/sessions",
            access,
            Some(origin),
            changed
        )
        .await
        .0
        .is_success()
    );
    let (status, result) = call(
        &app,
        Method::POST,
        "/sessions",
        access,
        Some(origin),
        create.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{result}");
    assert!(app.pending_clients.lock().await.is_empty());
    assert_eq!(
        call(
            &app,
            Method::POST,
            "/sessions",
            access,
            Some(origin),
            create
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let (status, composition) = call(
        &app,
        Method::GET,
        "/composition",
        access,
        Some(origin),
        Value::Null,
    )
    .await;
    assert!(status.is_success());
    assert_eq!(composition["session"], SESSION);
    assert!(
        call(
            &app,
            Method::GET,
            &format!("/sessions/{SESSION}"),
            access,
            Some(origin),
            Value::Null
        )
        .await
        .0
        .is_success()
    );
    for path in [
        "/sessions",
        "/sessions/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "/v1/models",
        "/tools",
        "/v1/account",
    ] {
        assert!(
            !call(&app, Method::GET, path, access, Some(origin), Value::Null)
                .await
                .0
                .is_success(),
            "{path}"
        );
    }
    let registered = call(
        &app,
        Method::POST,
        "/executors",
        access,
        Some(origin),
        json!({"binding":"runtime","session":SESSION,"tools":tools()}),
    )
    .await;
    assert!(registered.0.is_success(), "{}", registered.1);
    aex_server::clients::revoke(&app, &p, grant["id"].as_str().unwrap())
        .await
        .unwrap();
    let headers = HeaderMap::from_iter([("origin".parse().unwrap(), origin.parse().unwrap())]);
    assert!(
        aex_server::clients::access(&app, &headers, access)
            .await
            .is_err()
    );
    assert!(app.store.executor(EXECUTOR, &"h".repeat(32)).await.is_err());
}
#[tokio::test]
async fn browser_pending_credentials_are_not_recreated_after_restart() {
    let (_dir, app, _p, token, calls) = fixture().await;
    let origin = "https://customer.example";
    let (status, grant) = call(
        &app,
        Method::POST,
        "/v1/clients",
        &token,
        None,
        json!({"origin":origin,"expires_at":store::now()+60,"session":session()}),
    )
    .await;
    assert!(status.is_success(), "{grant}");
    app.pending_clients.lock().await.clear();
    let result = call(
        &app,
        Method::POST,
        "/sessions",
        grant["token"].as_str().unwrap(),
        Some(origin),
        grant["session"].clone(),
    )
    .await;
    assert_eq!(result.0, StatusCode::CONFLICT, "{}", result.1);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn bound_browser_grant_reconnects_explicitly_after_restart_and_obeys_expiry() {
    let (dir, app, _p, token, _calls) = fixture().await;
    let origin = "https://customer.example";
    let (_, grant) = call(
        &app,
        Method::POST,
        "/v1/clients",
        &token,
        None,
        json!({"origin":origin,"expires_at":store::now()+60,"session":session()}),
    )
    .await;
    let access = grant["token"].as_str().unwrap();
    assert_eq!(
        call(
            &app,
            Method::POST,
            "/sessions",
            access,
            Some(origin),
            grant["session"].clone()
        )
        .await
        .0,
        StatusCode::CREATED
    );
    let mut reopened = app.clone();
    reopened.store = store::Store::open(&common::database(dir.path()).await)
        .await
        .unwrap();
    reopened.pending_clients = Arc::default();
    let (status, composition) = call(
        &reopened,
        Method::GET,
        "/composition",
        access,
        Some(origin),
        Value::Null,
    )
    .await;
    assert!(status.is_success());
    assert_eq!(composition["session"], SESSION);
    sqlx::query("UPDATE browser_grants SET expires_at=0")
        .execute(&app.store.0)
        .await
        .unwrap();
    assert!(
        !call(
            &reopened,
            Method::GET,
            "/composition",
            access,
            Some(origin),
            Value::Null
        )
        .await
        .0
        .is_success()
    );
}
