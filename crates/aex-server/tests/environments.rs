mod common;
use aex_server::{
    App,
    billing::{self, BillingSettings},
    config::Config,
    environments::{self, Authorization, AuthorizedSelection, Usage},
    identity::Principal,
    store::Claim,
};
use axum::{Json, extract::State, http::HeaderMap};
use brain_protocol::CreateSessionRequest;
use serde_json::json;
use std::{
    sync::{Arc, atomic::Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

async fn app() -> (tempfile::TempDir, App, Principal) {
    let dir = tempfile::tempdir().unwrap();
    let mut config: Config =
        serde_json::from_str(include_str!("../../../examples/config.json")).unwrap();
    config.data_dir = dir.path().into();
    config.limits.minimum_free_disk_bytes = 1;
    config.billing = Some(
        serde_json::from_value(json!({"pricebook":{"id":"test-v1","rates":{
        "turn_ms":{"micro_usd":1,"units":1},"sandbox_ms":{"micro_usd":2,"units":1},
        "attachment_byte_secs":{"micro_usd":0,"units":1},"egress_bytes":{"micro_usd":0,"units":1}}},
        "max_turn_secs":120,"payments":null}))
        .unwrap(),
    );
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
    let (key, _) = app
        .store
        .issue_key(&account, &app.config.limits)
        .await
        .unwrap();
    let managed = serde_json::from_value(json!({"url":"http://127.0.0.1:8083","public_url":"https://api.aex.dev/environments/modal",
        "configuration":{"appName":"test"},"active_per_account":1,"profiles":{"python-v1":{"accounts":[account],"specification":{
            "image":"im-test","commands":{"calculate":["python","/app/calculate.py"]},"cpu":1,"memoryMiB":1024,
            "maxLifetimeMs":300000,"workdir":"/workspace","region":"us","outboundDomains":[],"maxOutputBytes":4096}}}})).unwrap();
    Arc::make_mut(&mut app.config).environments = Some(managed);
    app.config.validate().unwrap();
    environments::initialize(&app.store, app.config.environments.as_ref().unwrap())
        .await
        .unwrap();
    app.environment_token = Some("e".repeat(32));
    app.accepting.store(true, Ordering::SeqCst);
    app.store
        .report_usage(aex_server::store::now(), Default::default())
        .await
        .unwrap();
    billing::settings(
        &app,
        &account,
        BillingSettings {
            pricebook: "test-v1".into(),
            spend_limit_micro_usd: 1_000_000,
        },
    )
    .await
    .unwrap();
    billing::adjust(&app.store, &account, "grant", 100_000, "fixture credit")
        .await
        .unwrap();
    (dir, app, Principal { account, key })
}
fn request() -> CreateSessionRequest {
    serde_json::from_value(json!({"agentloop":{"implementation":{"type":"brain_component","entrypoint":"turn","id":"0".repeat(64)},"configuration":{},"environment":"brain"},
        "model":{"provider":"openai","name":"gpt-4.1-mini","api_key":"model-secret"},"tools":[],
        "environments":[{"name":"brain","driver":"brain"},{"name":"python","driver":"http","url":"https://api.aex.dev/environments/modal","configuration":{"profile":"python-v1","lifetimeMs":10000}}]})).unwrap()
}
async fn reserve(
    app: &App,
    p: &Principal,
    key: &str,
    maximum: Option<i64>,
) -> aex_server::error::Result<CreateSessionRequest> {
    let Claim::New(operation) = app
        .store
        .claim(p, "create", key, key, &app.config.limits)
        .await?
    else {
        unreachable!()
    };
    let mut tx = app.store.0.begin().await?;
    sqlx::query("SELECT id FROM accounts WHERE id=$1 FOR UPDATE")
        .bind(&p.account)
        .execute(&mut *tx)
        .await?;
    let mut input = request();
    environments::reserve_in(app, &mut tx, p, &operation, &mut input, maximum).await?;
    tx.commit().await?;
    Ok(input)
}
fn headers() -> HeaderMap {
    let mut result = HeaderMap::new();
    result.insert(
        "authorization",
        format!("Bearer {}", "e".repeat(32)).parse().unwrap(),
    );
    result
}
fn authorization(input: &CreateSessionRequest, session: &str) -> Authorization {
    Authorization {
        session_id: session.into(),
        environment: "python".into(),
        configuration: serde_json::from_value(input.environments[1].configuration.clone()).unwrap(),
    }
}
fn usage(id: &str, units: i64, terminal: bool) -> Usage {
    Usage {
        session_id: "session-one".into(),
        environment: "python".into(),
        authorization: id.into(),
        profile: "python-v1".into(),
        resource_id: Some("provider/resource:one".into()),
        units_ms: units,
        terminal,
    }
}

#[tokio::test]
async fn only_account_catalog_and_unmodified_selections_are_admitted() {
    let (_dir, app, p) = app().await;
    assert_eq!(environments::catalog(&app, &p).unwrap().profiles.len(), 1);
    let outsider = Principal {
        account: "outsider".into(),
        key: "other".into(),
    };
    assert!(
        environments::catalog(&app, &outsider)
            .unwrap()
            .profiles
            .is_empty()
    );
    let mut input = request();
    assert!(environments::selection(&app, &outsider, &input.environments[1]).is_err());
    assert!(environments::selection(&app, &p, &input.environments[1]).is_ok());
    input.environments[1].configuration["authorization"] = json!("forged");
    assert!(environments::selection(&app, &p, &input.environments[1]).is_err());
    input = request();
    input.environments[1].driver = brain_protocol::Driver::Http {
        url: "https://customer.example/driver".into(),
        credential: None,
    };
    assert!(environments::selection(&app, &p, &input.environments[1]).is_err());
    let mut changed = app.config.environments.clone().unwrap();
    changed
        .profiles
        .get_mut("python-v1")
        .unwrap()
        .specification
        .configuration
        .insert("image".into(), json!("im-other"));
    assert!(
        environments::initialize(&app.store, &changed)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn quota_and_money_are_reserved_before_sealing_and_failures_leave_no_holds() {
    let (_dir, app, p) = app().await;
    for (key, maximum) in [("no-ceiling", None), ("low-ceiling", Some(19999))] {
        assert!(reserve(&app, &p, key, maximum).await.is_err());
    }
    assert_eq!(
        billing::wallet(&app, &p.account)
            .await
            .unwrap()
            .reserved_micro_usd,
        0
    );
    let (first, second) = tokio::join!(
        reserve(&app, &p, "first", Some(20000)),
        reserve(&app, &p, "second", Some(20000))
    );
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    let input = first.or(second).unwrap();
    assert!(
        matches!(&input.environments[1].driver,brain_protocol::Driver::Http{url,credential} if url == "http://127.0.0.1:8083" && credential.as_deref() == app.environment_token.as_deref())
    );
    assert_eq!(
        billing::wallet(&app, &p.account)
            .await
            .unwrap()
            .reserved_micro_usd,
        20000
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM environment_grants")
        .fetch_one(&app.store.0)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn grant_binding_revocation_and_cumulative_settlement_cannot_cross_resources() {
    let (_dir, app, p) = app().await;
    let input = reserve(&app, &p, "run", Some(20000)).await.unwrap();
    assert!(
        environments::authorize(
            State(app.clone()),
            HeaderMap::new(),
            Json(authorization(&input, "session-one"))
        )
        .await
        .is_err()
    );
    let first = environments::authorize(
        State(app.clone()),
        headers(),
        Json(authorization(&input, "session-one")),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(
        first,
        environments::authorize(
            State(app.clone()),
            headers(),
            Json(authorization(&input, "session-one"))
        )
        .await
        .unwrap()
        .0
    );
    assert!(
        environments::authorize(
            State(app.clone()),
            headers(),
            Json(authorization(&input, "session-two"))
        )
        .await
        .is_err()
    );
    let AuthorizedSelection {
        authorization: id, ..
    } = authorization(&input, "session-one").configuration;
    for units in [1000, 1000, 2000, 500] {
        let _ = environments::report(
            State(app.clone()),
            headers(),
            Json(usage(&id, units, false)),
        )
        .await
        .unwrap();
    }
    let mut wrong = usage(&id, 3000, false);
    wrong.session_id = "session-two".into();
    assert!(
        environments::report(State(app.clone()), headers(), Json(wrong))
            .await
            .is_err()
    );
    let mut wrong = usage(&id, 3000, false);
    wrong.resource_id = Some("sb-other".into());
    assert!(
        environments::report(State(app.clone()), headers(), Json(wrong))
            .await
            .is_err()
    );
    assert!(
        environments::report(State(app.clone()), headers(), Json(usage(&id, 500, true)))
            .await
            .is_err()
    );
    app.store.revoke_key(&p.key).await.unwrap();
    assert!(
        environments::authorize(
            State(app.clone()),
            headers(),
            Json(authorization(&input, "session-one"))
        )
        .await
        .is_err()
    );
    app.accepting.store(false, Ordering::SeqCst);
    for _ in 0..2 {
        let _ = environments::report(State(app.clone()), headers(), Json(usage(&id, 3000, true)))
            .await
            .unwrap();
    }
    assert!(
        environments::report(State(app.clone()), headers(), Json(usage(&id, 4000, true)))
            .await
            .is_err()
    );
    let wallet = billing::wallet(&app, &p.account).await.unwrap();
    assert_eq!(
        (wallet.balance_micro_usd, wallet.reserved_micro_usd),
        (94000, 0)
    );
}

#[tokio::test]
async fn expiry_releases_only_proven_unbound_resources() {
    let (_dir, mut app, p) = app().await;
    Arc::make_mut(&mut app.config)
        .environments
        .as_mut()
        .unwrap()
        .active_per_account = 2;
    reserve(&app, &p, "unbound", Some(20000)).await.unwrap();
    let bound = reserve(&app, &p, "bound", Some(20000)).await.unwrap();
    let _ = environments::authorize(
        State(app.clone()),
        headers(),
        Json(authorization(&bound, "session-one")),
    )
    .await
    .unwrap();
    let expired = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
        - 1;
    sqlx::query("UPDATE environment_grants SET expires_at=$1")
        .bind(expired)
        .execute(&app.store.0)
        .await
        .unwrap();
    environments::maintain(&app).await.unwrap();
    environments::maintain(&app).await.unwrap();
    let wallet = billing::wallet(&app, &p.account).await.unwrap();
    assert_eq!(
        (wallet.balance_micro_usd, wallet.reserved_micro_usd),
        (100000, 20000)
    );
    assert!(
        environments::authorize(
            State(app.clone()),
            headers(),
            Json(authorization(&bound, "session-one"))
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn create_admission_commits_funding_before_dispatch_and_never_repeats_an_unknown_effect() {
    use axum::{Router, body::Bytes, http::StatusCode, routing::post};
    use std::sync::atomic::AtomicUsize;
    let (_dir, mut app, p) = app().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let funded = app.clone();
    let account = p.account.clone();
    let upstream = Router::new().route(
        "/v1/sessions",
        post(move |Json(input): Json<serde_json::Value>| {
            let (calls, app, account) = (observed.clone(), funded.clone(), account.clone());
            async move {
                assert_eq!(input["environments"][1]["url"], "http://127.0.0.1:8083");
                assert_eq!(input["environments"][1]["credential"], "e".repeat(32));
                assert!(
                    input["environments"][1]["configuration"]["authorization"]
                        .as_str()
                        .unwrap()
                        .starts_with("env_")
                );
                assert_eq!(
                    billing::wallet(&app, &account)
                        .await
                        .unwrap()
                        .reserved_micro_usd,
                    20000
                );
                calls.fetch_add(1, Ordering::SeqCst);
                StatusCode::BAD_GATEWAY
            }
        }),
    );
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    Arc::make_mut(&mut app.config).brain_url = format!("http://{}", socket.local_addr().unwrap());
    app.brain = aex_server::brain::Brain::new(&app.config, "b".repeat(32)).unwrap();
    let server = tokio::spawn(async move {
        axum::serve(socket, upstream).await.unwrap();
    });
    let body = Bytes::from(serde_json::to_vec(&request()).unwrap());
    let mut headers = HeaderMap::new();
    headers.insert("content-type", "application/json".parse().unwrap());
    headers.insert("x-aex-max-cost-micro-usd", "19999".parse().unwrap());
    assert!(
        aex_server::sessions::create(&app, &p, &headers, body.clone(), "once")
            .await
            .is_err()
    );
    let claims: i64 = sqlx::query_scalar("SELECT count(*) FROM claims")
        .fetch_one(&app.store.0)
        .await
        .unwrap();
    assert_eq!((claims, calls.load(Ordering::SeqCst)), (0, 0));
    headers.insert("x-aex-max-cost-micro-usd", "20000".parse().unwrap());
    assert_eq!(
        aex_server::sessions::create(&app, &p, &headers, body.clone(), "once")
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_GATEWAY
    );
    assert!(
        aex_server::sessions::create(&app, &p, &headers, body, "once")
            .await
            .is_err()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        billing::wallet(&app, &p.account)
            .await
            .unwrap()
            .reserved_micro_usd,
        20000
    );
    server.abort();
}
