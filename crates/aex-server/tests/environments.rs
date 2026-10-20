mod common;
use aex_server::brain_contract::CreateSession;
use aex_server::{
    App,
    billing::{self, BillingSettings},
    config::Config,
    identity::Principal,
    managed::{self, Authorization, Usage},
    store::Claim,
};
use axum::{Json, extract::State, http::HeaderMap};
use serde_json::json;
use std::sync::{Arc, atomic::Ordering};

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
    Arc::make_mut(&mut app.config).managed_providers=serde_json::from_value(json!({"sandbox":{"accounts":[account],"operations":["create","exec","inspect","terminate"],"active_per_account":1,"profiles":{"python-v1":{"accounts":[account],"max_lifetime_ms":300000,"specification":{"image":"python:3.13-slim","cpu":1,"memoryMiB":1024}}}},"application":{"accounts":[account],"operations":[],"active_per_account":10}})).unwrap();
    app.config.validate().unwrap();
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
fn request() -> CreateSession {
    serde_json::from_value(json!({"harness":"pi","config":{"model":"openai/gpt-4.1-mini","tools":[],"bindings":[{"alias":"python","provider":"sandbox","operations":["create","exec"],"parameters":{"profile":"python-v1","lifetime_ms":10000}}]},"model_credential":{"api_key":"model-secret"}})).unwrap()
}
async fn reserve(
    app: &App,
    p: &Principal,
    key: &str,
    ceiling: Option<i64>,
) -> aex_server::error::Result<CreateSession> {
    let Claim::New(operation) = app
        .store
        .claim(p, "create", key, key, &app.config.limits)
        .await?
    else {
        panic!()
    };
    let mut tx = app.store.0.begin().await?;
    sqlx::query("SELECT id FROM accounts WHERE id=$1 FOR UPDATE")
        .bind(&p.account)
        .execute(&mut *tx)
        .await?;
    let mut input = request();
    managed::reserve_in(app, &mut tx, p, &operation, &mut input, ceiling).await?;
    tx.commit().await?;
    Ok(input)
}
fn headers() -> HeaderMap {
    HeaderMap::from_iter([(
        "authorization".parse().unwrap(),
        format!("Bearer {}", "e".repeat(32)).parse().unwrap(),
    )])
}
fn authorization(p: &Principal, input: &CreateSession, session: &str) -> Authorization {
    let binding = &input.config.bindings.as_ref().unwrap()[0];
    Authorization {
        tenant: p.account.clone(),
        session: session.into(),
        alias: binding.alias.clone(),
        provider: binding.provider.clone().unwrap(),
        parameters: binding.parameters.clone().unwrap(),
        action: false,
    }
}
fn usage(
    p: &Principal,
    input: &CreateSession,
    session: Option<&str>,
    units: i64,
    terminal: bool,
) -> Usage {
    Usage {
        authorization: input.config.bindings.as_ref().unwrap()[0]
            .parameters
            .as_ref()
            .unwrap()["authorization"]
            .as_str()
            .unwrap()
            .into(),
        tenant: p.account.clone(),
        session: session.map(str::to_owned),
        units_ms: units,
        terminal,
        receipt: format!("receipt:{units}:{terminal}"),
    }
}
async fn bind(app: &App, p: &Principal, input: &CreateSession, id: &str) {
    sqlx::query("INSERT INTO sessions(id,account,created) VALUES($1,$2,$3)")
        .bind(id)
        .bind(&p.account)
        .bind(aex_server::store::now())
        .execute(&app.store.0)
        .await
        .unwrap();
    let grant = &input.config.bindings.as_ref().unwrap()[0]
        .parameters
        .as_ref()
        .unwrap()["authorization"];
    sqlx::query("UPDATE tool_grants SET session=$1 WHERE id=$2")
        .bind(id)
        .bind(grant.as_str().unwrap())
        .execute(&app.store.0)
        .await
        .unwrap();
}

#[tokio::test]
async fn only_account_catalog_and_unmodified_selections_are_admitted() {
    let (_dir, mut app, p) = app().await;
    assert_eq!(
        managed::catalog(&app, &p).providers["sandbox"]
            .profiles
            .len(),
        1
    );
    let outsider = Principal {
        account: "outsider".into(),
        key: "other".into(),
    };
    assert!(managed::catalog(&app, &outsider).providers.is_empty());
    assert!(
        reserve(&app, &outsider, "foreign", Some(20000))
            .await
            .is_err()
    );
    let input = reserve(&app, &p, "selected", Some(20000)).await.unwrap();
    let value = managed::authorize(
        State(app.clone()),
        headers(),
        Json(authorization(&p, &input, "session")),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(value["profile"]["image"], "python:3.13-slim");
    Arc::make_mut(&mut app.config)
        .managed_providers
        .get_mut("sandbox")
        .unwrap()
        .profiles
        .get_mut("python-v1")
        .unwrap()
        .specification["image"] = json!("changed");
    let value = managed::authorize(
        State(app.clone()),
        headers(),
        Json(authorization(&p, &input, "session")),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(value["profile"]["image"], "python:3.13-slim");
    let mut forged = authorization(&p, &input, "session");
    forged.parameters["lifetime_ms"] = json!(20000);
    assert!(
        managed::authorize(State(app.clone()), headers(), Json(forged))
            .await
            .is_err()
    );
    let mut action = authorization(&p, &input, "session");
    action.action = true;
    assert!(
        managed::authorize(State(app), headers(), Json(action))
            .await
            .is_err()
    );
}
#[tokio::test]
async fn quota_and_money_are_reserved_before_dispatch_and_failures_leave_no_holds() {
    let (_dir, app, p) = app().await;
    for (key, ceiling) in [("no-ceiling", None), ("low", Some(19999))] {
        assert!(reserve(&app, &p, key, ceiling).await.is_err());
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
    assert_eq!(
        billing::wallet(&app, &p.account)
            .await
            .unwrap()
            .reserved_micro_usd,
        20000
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM tool_grants")
        .fetch_one(&app.store.0)
        .await
        .unwrap();
    assert_eq!(count, 1);
}
#[tokio::test]
async fn grant_binding_revocation_and_cumulative_settlement_cannot_cross_resources() {
    let (_dir, app, p) = app().await;
    let input = reserve(&app, &p, "owned", Some(20000)).await.unwrap();
    bind(&app, &p, &input, "owned").await;
    let mut action = authorization(&p, &input, "owned");
    action.action = true;
    assert!(
        managed::authorize(State(app.clone()), headers(), Json(action))
            .await
            .is_ok()
    );
    assert!(
        managed::authorize(
            State(app.clone()),
            headers(),
            Json(authorization(&p, &input, "foreign"))
        )
        .await
        .is_err()
    );
    assert!(
        managed::report(
            State(app.clone()),
            headers(),
            Json(usage(&p, &input, Some("foreign"), 1, false))
        )
        .await
        .is_err()
    );
    for units in [1000, 2000, 2000] {
        let _ = managed::report(
            State(app.clone()),
            headers(),
            Json(usage(&p, &input, Some("owned"), units, false)),
        )
        .await
        .unwrap();
    }
    assert_eq!(
        billing::wallet(&app, &p.account)
            .await
            .unwrap()
            .balance_micro_usd,
        96000
    );
    managed::revoke(&app, "owned").await.unwrap();
    assert!(!managed::settled(&app, "owned").await.unwrap());
    assert!(
        managed::authorize(
            State(app.clone()),
            headers(),
            Json(authorization(&p, &input, "owned"))
        )
        .await
        .is_err()
    );
    let _ = managed::report(
        State(app.clone()),
        headers(),
        Json(usage(&p, &input, Some("owned"), 3000, true)),
    )
    .await
    .unwrap();
    assert!(managed::settled(&app, "owned").await.unwrap());
    let wallet = billing::wallet(&app, &p.account).await.unwrap();
    assert_eq!(wallet.balance_micro_usd, 94000);
    assert_eq!(wallet.reserved_micro_usd, 0);
}
#[tokio::test]
async fn expiry_releases_only_resources_confirmed_unused_by_the_retained_service_journal() {
    let (_dir, app, p) = app().await;
    let input = reserve(&app, &p, "unbound", Some(20000)).await.unwrap();
    sqlx::query("UPDATE tool_grants SET expires_ms=0")
        .execute(&app.store.0)
        .await
        .unwrap();
    managed::maintain(&app).await.unwrap();
    assert_eq!(
        billing::wallet(&app, &p.account)
            .await
            .unwrap()
            .reserved_micro_usd,
        20000
    );
    let pending = managed::pending(State(app.clone()), headers())
        .await
        .unwrap()
        .0;
    assert_eq!(pending["grants"].as_array().unwrap().len(), 1);
    let _ = managed::report(
        State(app.clone()),
        headers(),
        Json(usage(&p, &input, None, 0, true)),
    )
    .await
    .unwrap();
    assert_eq!(
        billing::wallet(&app, &p.account)
            .await
            .unwrap()
            .reserved_micro_usd,
        0
    );
}
#[tokio::test]
async fn application_bindings_keep_their_contract_without_sandbox_money_or_lifetime() {
    let (_dir, app, p) = app().await;
    let Claim::New(operation) = app
        .store
        .claim(
            &p,
            "create",
            "application",
            "application",
            &app.config.limits,
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    let mut request:CreateSession=serde_json::from_value(json!({"harness":"pi","config":{"model":"openai/gpt-4.1-mini","tools":[],"bindings":[{"alias":"application","provider":"application","operations":["read"],"parameters":{"endpoint":"https://app.example/tools","timeout_ms":25000,"tools":[{"name":"read","description":"Read","input_schema":{"type":"object"}}]}}]},"credentials":[{"binding":"application","value":{"token":"private-application-key"}}]})).unwrap();
    let mut tx = app.store.0.begin().await.unwrap();
    managed::reserve_in(&app, &mut tx, &p, &operation, &mut request, None)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let row: String = sqlx::query_scalar("SELECT row_to_json(tool_grants)::text FROM tool_grants")
        .fetch_one(&app.store.0)
        .await
        .unwrap();
    assert!(!row.contains("private-application-key"));
    assert_eq!(
        billing::wallet(&app, &p.account)
            .await
            .unwrap()
            .reserved_micro_usd,
        0
    );
    let mut altered = request.clone();
    altered.config.bindings.as_mut().unwrap()[0]
        .parameters
        .as_mut()
        .unwrap()["authorization"] = json!("forged");
    let mut tx = app.store.0.begin().await.unwrap();
    assert!(
        managed::reserve_in(&app, &mut tx, &p, &operation, &mut altered, None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn private_tool_journal_commits_before_ack_and_cannot_resurrect_results_after_revocation() {
    use managed::journal::{Action, handle};
    let (_dir, app, p) = app().await;
    let input = reserve(&app, &p, "journal", Some(20000)).await.unwrap();
    bind(&app, &p, &input, "journal-session").await;
    let grant = input.config.bindings.as_ref().unwrap()[0]
        .parameters
        .as_ref()
        .unwrap()["authorization"]
        .as_str()
        .unwrap()
        .to_owned();
    let value = json!({"id":"job","grant":grant,"verifier":"completion-hash","result":{"content":[{"type":"text","text":"private result"}]}});
    assert!(
        handle(
            State(app.clone()),
            HeaderMap::new(),
            Json(Action::Get { id: "job".into() })
        )
        .await
        .is_err()
    );
    let _ = handle(
        State(app.clone()),
        headers(),
        Json(Action::Set {
            id: "job".into(),
            value: value.clone(),
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        handle(
            State(app.clone()),
            headers(),
            Json(Action::Completion {
                verifier: "completion-hash".into()
            })
        )
        .await
        .unwrap()
        .0,
        value
    );
    let mut tx = app.store.0.begin().await.unwrap();
    sqlx::query("SELECT id FROM tool_grants WHERE id=$1 FOR UPDATE")
        .bind(&grant)
        .execute(&mut *tx)
        .await
        .unwrap();
    let writer = app.clone();
    let late = value.clone();
    let write = tokio::spawn(async move {
        handle(
            State(writer),
            headers(),
            Json(Action::Set {
                id: "late".into(),
                value: late,
            }),
        )
        .await
    });
    sqlx::query("UPDATE tool_grants SET revoked=TRUE WHERE id=$1")
        .bind(&grant)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(write.await.unwrap().is_err());
    let _ = handle(
        State(app.clone()),
        headers(),
        Json(Action::DeleteGrant { grant }),
    )
    .await
    .unwrap();
    assert!(
        handle(
            State(app),
            headers(),
            Json(Action::Completion {
                verifier: "completion-hash".into()
            })
        )
        .await
        .unwrap()
        .0
        .is_null()
    );
}
