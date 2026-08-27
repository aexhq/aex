use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use aex_control::{
    api::{AppState, router},
    brain::BrainClient,
    identity,
    payments::FakePayments,
    store::{AccountRow, CreditGrantRow, Db, KeyRow},
};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::State,
    http::{Method, Request, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
};
use brain_protocol::{JournalId, Session, SessionId, SessionStatus};
use serde_json::{Value, json};
use tower::ServiceExt as _;

#[tokio::test]
async fn authenticated_session_api_tracks_ownership_and_forwards_the_brain_contract() {
    let deleted = Arc::new(AtomicBool::new(false));
    let brain = Router::new()
        .route("/health/ready", any(|| async { StatusCode::NO_CONTENT }))
        .route("/v1/agentloops", any(fake_brain))
        .route("/v1/agentloops/{digest}", any(fake_brain))
        .route("/v1/sessions", any(fake_brain))
        .route("/v1/sessions/{*rest}", any(fake_brain))
        .with_state(deleted.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, brain).await.unwrap() });

    let db = Db::open_memory().unwrap();
    let account_secret = identity::mint_secret("at");
    let account = AccountRow {
        id: identity::new_id("acc"),
        email: "owner@example.com".into(),
        created_ms: 1,
        max_concurrent_sessions: 10,
        session_creates_per_hour: 30,
    };
    db.create_account(account.clone(), account.email.clone(), account_secret.hash)
        .await
        .unwrap();
    let key_secret = identity::mint_secret("sk");
    db.create_key(
        KeyRow {
            id: identity::new_id("key"),
            account_id: account.id.clone(),
            name: "test".into(),
            prefix: key_secret.prefix,
            created_ms: 1,
            last_used_ms: None,
            revoked_ms: None,
        },
        key_secret.hash,
    )
    .await
    .unwrap();
    db.grant_credit(CreditGrantRow {
        id: identity::new_id("grt"),
        request_key: "test-credit".into(),
        account_id: account.id.clone(),
        email: account.email.clone(),
        amount_cents: 100,
        reason: "test".into(),
        created_ms: 1,
    })
    .await
    .unwrap();
    let app = router(AppState {
        db,
        brain: BrainClient::new(format!("http://{address}"), "operator").unwrap(),
        payments: Arc::new(FakePayments),
        stripe_webhook: None,
        operator_token_hash: None,
        default_limits: (10, 30),
    });

    let unauthorized = call(&app, Method::GET, "/v1/sessions", None, None).await;
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let oversized_control = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/waitlist")
                .header("content-type", "application/json")
                .body(Body::from(vec![b'x'; 64 * 1024 + 1]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(oversized_control.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let created = call(
        &app,
        Method::POST,
        "/v1/sessions",
        Some(&key_secret.secret),
        Some((
            "create-one",
            json!({
                "agentloop_digest":"a".repeat(64),
                "brain_configuration":{},
                "model":{"provider":"vercel-ai-gateway","name":"openai/test","api_key":"test-key"},
                "presentation":{"system":"test","tools":[]},
                "environments":[],
                "tool_bindings":[]
            }),
        )),
    )
    .await;
    assert_eq!(created.status(), StatusCode::OK);
    assert_eq!(
        json_body(created).await["session_id"],
        session().session_id.to_string()
    );

    let listed = call(
        &app,
        Method::GET,
        "/v1/sessions",
        Some(&key_secret.secret),
        None,
    )
    .await;
    assert_eq!(
        json_body(listed).await["sessions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let missing_key = call(
        &app,
        Method::POST,
        &format!("/v1/sessions/{}/messages", session().session_id),
        Some(&key_secret.secret),
        None,
    )
    .await;
    assert_eq!(missing_key.status(), StatusCode::BAD_REQUEST);

    for (method, suffix, body) in [
        (Method::GET, "", None),
        (
            Method::POST,
            "/messages",
            Some(("message-one", json!({"content":"hello"}))),
        ),
        (Method::GET, "/events", None),
        (Method::POST, "/cancel", Some(("cancel-one", json!({})))),
        (Method::POST, "/end", Some(("end-one", json!({})))),
    ] {
        let response = call(
            &app,
            method,
            &format!("/v1/sessions/{}{suffix}", session().session_id),
            Some(&key_secret.secret),
            body,
        )
        .await;
        assert!(
            response.status().is_success(),
            "{suffix}: {}",
            response.status()
        );
    }

    let deleted_response = call(
        &app,
        Method::DELETE,
        &format!("/v1/sessions/{}", session().session_id),
        Some(&key_secret.secret),
        Some(("delete-one", json!({}))),
    )
    .await;
    assert_eq!(deleted_response.status(), StatusCode::NO_CONTENT);
    assert!(deleted.load(Ordering::Acquire));
    let absent = call(
        &app,
        Method::GET,
        &format!("/v1/sessions/{}", session().session_id),
        Some(&key_secret.secret),
        None,
    )
    .await;
    assert_eq!(absent.status(), StatusCode::NOT_FOUND);
}

async fn fake_brain(State(deleted): State<Arc<AtomicBool>>, request: Request<Body>) -> Response {
    let path = request.uri().path();
    if path == "/v1/agentloops" {
        return axum::Json(json!({"digest":"a".repeat(64),"status":"admitted"})).into_response();
    }
    if path.contains("/events") {
        return axum::Json(json!({"events":[],"next_cursor":0})).into_response();
    }
    if request.method() == Method::DELETE {
        deleted.store(true, Ordering::Release);
        return StatusCode::NO_CONTENT.into_response();
    }
    if deleted.load(Ordering::Acquire) && path != "/v1/sessions" {
        return StatusCode::NOT_FOUND.into_response();
    }
    if path == "/v1/sessions" && request.method() == Method::GET {
        return axum::Json(json!({"sessions":[session()]})).into_response();
    }
    axum::Json(session()).into_response()
}

fn session() -> Session {
    Session {
        session_id: SessionId::new("ses_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        journal_id: JournalId::new("jrn_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        status: SessionStatus::Idle,
        through_sequence: 1,
        presentation_digest: "a".repeat(64),
    }
}

async fn call(
    app: &Router,
    method: Method,
    path: &str,
    bearer: Option<&str>,
    body: Option<(&str, Value)>,
) -> Response {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(bearer) = bearer {
        request = request.header("authorization", format!("Bearer {bearer}"));
    }
    let body = if let Some((key, body)) = body {
        request = request
            .header("idempotency-key", key)
            .header("content-type", "application/json");
        Body::from(body.to_string())
    } else {
        Body::empty()
    };
    app.clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap()
}

async fn json_body(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}
