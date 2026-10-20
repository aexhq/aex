mod common;

use aex_server::brain_contract::{Event, ModelRequest, ModelUsage};
use aex_server::{
    App,
    billing::{self, BillingSettings},
    config::Config,
    model_usage,
};
use serde_json::json;
use sqlx::Row;

async fn app(limit: i64) -> (tempfile::TempDir, App, String) {
    let directory = tempfile::tempdir().unwrap();
    let mut config: Config =
        serde_json::from_str(include_str!("../../../examples/config.json")).unwrap();
    config.data_dir = directory.path().into();
    config.billing = Some(serde_json::from_value(json!({"pricebook":{"id":"tokens-v1","rates":{
        "model_tokens":{"micro_usd":1,"units":3}, "sandbox_ms":{"micro_usd":1,"units":1},
        "attachment_byte_secs":{"micro_usd":1,"units":1000}, "egress_bytes":{"micro_usd":1,"units":1}
    }},"max_turn_secs":120,"payments":null})).unwrap());
    let app = App::open(
        config,
        "b".repeat(32),
        "o".repeat(32),
        common::database(directory.path()).await,
        "s".repeat(32),
    )
    .await
    .unwrap();
    let account = app.store.create_account(&app.config.limits).await.unwrap();
    billing::settings(
        &app,
        &account,
        BillingSettings {
            pricebook: "tokens-v1".into(),
            spend_limit_micro_usd: limit,
        },
    )
    .await
    .unwrap();
    billing::adjust(&app.store, &account, "fund", 100, "test funding")
        .await
        .unwrap();
    (directory, app, account)
}

async fn session(app: &App, account: &str, id: &str) {
    sqlx::query("INSERT INTO sessions(id,account,created,model_observed_at,runtime) VALUES($1,$2,0,$3,'brain')")
        .bind(id)
        .bind(account)
        .bind(model_usage::now_ms())
        .execute(&app.store.0)
        .await
        .unwrap();
}

fn event(sequence: u64, kind: &str, data: serde_json::Value) -> Event {
    Event {
        id: format!("{sequence:032x}"),
        cursor: Some(format!("cursor:{sequence}")),
        time: time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap(),
        r#type: kind.into(),
        data,
        input_id: None,
    }
}
fn usage(input: Option<i64>, output: Option<i64>) -> ModelUsage {
    ModelUsage {
        availability: if input.is_some() && output.is_some() {
            "reported"
        } else {
            "partial"
        }
        .into(),
        input_tokens: input,
        output_tokens: output,
        input_cache: "included".into(),
        output_reasoning: "included".into(),
        semantics: "provider".into(),
        ..Default::default()
    }
}
fn request(sequence: u64, id: &str, state: &str, usage: ModelUsage) -> Event {
    event(
        sequence,
        "model.request",
        serde_json::to_value(ModelRequest {
            request_id: id.into(),
            state: state.into(),
            usage,
            harness: "pi".into(),
            provider: "openai".into(),
            model: "gpt-4.1-mini".into(),
            purpose: "inference".into(),
            boundary: "logical".into(),
            attempts: "unobservable".into(),
            ..Default::default()
        })
        .unwrap(),
    )
}
async fn record(app: &App, id: &str, event: &Event) -> bool {
    let after: String = sqlx::query_scalar("SELECT brain_cursor FROM sessions WHERE id=$1")
        .bind(id)
        .fetch_one(&app.store.0)
        .await
        .unwrap();
    model_usage::record(app, id, &after, event).await.unwrap()
}

#[tokio::test]
async fn switching_execution_meters_waits_for_legacy_holds_and_usage_is_account_scoped() {
    let (_dir, app, account) = app(100).await;
    let mut config = (*app.config).clone();
    let billing = config.billing.as_mut().unwrap();
    billing.pricebook.id = "legacy".into();
    let rate = billing
        .pricebook
        .rates
        .remove(&billing::Meter::ModelTokens)
        .unwrap();
    billing.pricebook.rates.insert(billing::Meter::TurnMs, rate);
    billing::initialize(&app.store, billing).await.unwrap();
    let mut legacy = app.clone();
    legacy.config = std::sync::Arc::new(config);
    billing::settings(
        &legacy,
        &account,
        BillingSettings {
            pricebook: "legacy".into(),
            spend_limit_micro_usd: 100,
        },
    )
    .await
    .unwrap();
    let mut tx = app.store.0.begin().await.unwrap();
    sqlx::query("SELECT id FROM accounts WHERE id=$1 FOR UPDATE")
        .bind(&account)
        .execute(&mut *tx)
        .await
        .unwrap();
    billing::reserve_in(
        &mut tx,
        &account,
        billing::Reservation {
            id: "legacy-turn",
            resource: "ses_legacy",
            meter: billing::Meter::TurnMs,
            max_units: 3,
            max_cost: Some(1),
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert!(
        billing::settings(
            &app,
            &account,
            BillingSettings {
                pricebook: "tokens-v1".into(),
                spend_limit_micro_usd: 100
            }
        )
        .await
        .is_err()
    );
    billing::meter(
        &app.store,
        &billing::UsageReport {
            id: "legacy-ended".into(),
            reservation: "legacy-turn".into(),
            units: 0,
            terminal: true,
        },
    )
    .await
    .unwrap();
    billing::settings(
        &app,
        &account,
        BillingSettings {
            pricebook: "tokens-v1".into(),
            spend_limit_micro_usd: 100,
        },
    )
    .await
    .unwrap();
    session(&app, &account, "ses_owned").await;
    let other = app.store.create_account(&app.config.limits).await.unwrap();
    let (_, own_key) = app
        .store
        .issue_key(&account, &app.config.limits)
        .await
        .unwrap();
    let (_, other_key) = app
        .store
        .issue_key(&other, &app.config.limits)
        .await
        .unwrap();
    let own = aex_server::account::handle(
        &app,
        &axum::http::Method::GET,
        "/v1/usage/ses_owned",
        &own_key,
        axum::body::Bytes::new(),
    )
    .await
    .unwrap();
    assert_eq!(own.status(), 200);
    let denied = aex_server::account::handle(
        &app,
        &axum::http::Method::GET,
        "/v1/usage/ses_owned",
        &other_key,
        axum::body::Bytes::new(),
    )
    .await
    .err()
    .unwrap();
    assert_eq!(denied.0, 404);
}

#[tokio::test]
async fn background_observation_stops_work_and_settles_actual_usage_without_a_submitted_turn() {
    use axum::{Json, Router, extract::Query, routing::get};
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_test_writer()
        .try_init();
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    let (_dir, mut app, account) = app(4).await;
    session(&app, &account, "ses_stream").await;
    let cancelled = Arc::new(AtomicBool::new(false));
    let count = Arc::new(AtomicUsize::new(0));
    let read = cancelled.clone();
    let metadata = cancelled.clone();
    let write = cancelled.clone();
    let calls = count.clone();
    let started = request(1, "background", "started", usage(None, None));
    let terminal = request(2, "background", "failed", usage(Some(30), Some(0)));
    let closed = event(3, "session.status", json!({"status":"closed"}));
    let upstream=Router::new().route("/sessions/ses_stream/events",get(move |Query(query):Query<std::collections::BTreeMap<String,String>>| {
        let mut events=vec![started.clone()];if read.load(Ordering::SeqCst){let mut terminal=terminal.clone();terminal.data["unmeasured"]=json!(["retry count unavailable"]);events.extend([terminal,closed.clone()]);}
        let after=query.get("after").cloned().unwrap_or_default();
        let position=if after.is_empty(){0}else{events.iter().position(|event|event.cursor.as_ref()==Some(&after)).unwrap()+1};
        let next=events.last().and_then(|event|event.cursor.clone()).unwrap_or(after);
        async move {Json(json!({"events":events.into_iter().skip(position).collect::<Vec<_>>(),"next_cursor":next}))}
    }).post(move |Json(input):Json<serde_json::Value>| {assert_eq!(input["type"],"user.interrupt");calls.fetch_add(1,Ordering::SeqCst);write.store(true,Ordering::SeqCst);async {Json(json!({}))}}))
    .route("/sessions/ses_stream",get(move || {let closed=metadata.load(Ordering::SeqCst);async move {Json(json!({"id":"ses_stream","status":if closed{"closed"}else{"running"},"harness":"pi","config":{"model":"openai/gpt-4.1-mini","tools":[]},"descriptors":[],"created_at":"2026-10-20T00:00:00Z","updated_at":"2026-10-20T00:00:00Z"}))}}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = (*app.config).clone();
    config.brain_url = format!("http://{}", listener.local_addr().unwrap());
    app.brain = aex_server::brain::Brain::new(&config, "b".repeat(32)).unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let stop = tokio_util::sync::CancellationToken::new();
    let observer = tokio::spawn(model_usage::run(app.clone(), stop.clone()));
    tokio::time::timeout(std::time::Duration::from_secs(8), async {
        loop {
            let complete: bool =
                sqlx::query_scalar("SELECT model_complete FROM sessions WHERE id='ses_stream'")
                    .fetch_one(&app.store.0)
                    .await
                    .unwrap();
            if complete {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    stop.cancel();
    observer.await.unwrap();
    server.abort();
    let usage = model_usage::totals(&app.store, &account).await.unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(usage.reported_input_tokens, 30);
    assert_eq!(usage.rated_micro_usd, 10);
    assert_eq!(usage.charged_micro_usd, 4);
    assert_eq!(usage.pending_estimate_micro_usd, 0);
    assert_eq!(usage.unmeasured_calls, 1);
}

#[tokio::test]
async fn concurrent_receipts_replay_once_and_absorb_excess_without_double_counting_subsets() {
    let (_dir, app, account) = app(4).await;
    for id in ["ses_one", "ses_two", "ses_three"] {
        session(&app, &account, id).await;
        record(
            &app,
            id,
            &request(1, "request", "started", usage(None, None)),
        )
        .await;
    }
    let mut reported = usage(Some(4), Some(2));
    reported.cache_read_tokens = Some(3);
    reported.reasoning_tokens = Some(1);
    let receipt = request(2, "request", "completed", reported);
    let (a, b, c) = tokio::join!(
        model_usage::record(&app, "ses_one", "cursor:1", &receipt),
        model_usage::record(&app, "ses_two", "cursor:1", &receipt),
        model_usage::record(&app, "ses_three", "cursor:1", &receipt)
    );
    assert!(a.unwrap() && b.unwrap() && c.unwrap());
    assert!(
        !model_usage::record(&app, "ses_one", "cursor:1", &receipt)
            .await
            .unwrap()
    );
    let totals = model_usage::totals(&app.store, &account).await.unwrap();
    assert_eq!(totals.reported_input_tokens, 12);
    assert_eq!(totals.reported_output_tokens, 6);
    assert_eq!(totals.rated_micro_usd, 6);
    assert_eq!(totals.charged_micro_usd, 4);
    let wallet = billing::wallet(&app, &account).await.unwrap();
    assert_eq!(wallet.balance_micro_usd, 96);
    assert_eq!(wallet.spent_this_month_micro_usd, 4);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM credit_ledger WHERE account=$1 AND kind='usage'")
            .bind(&account)
            .fetch_one(&app.store.0)
            .await
            .unwrap();
    assert_eq!(count, 3);
}

#[tokio::test]
async fn cumulative_rating_covers_background_calls_and_retains_unknown_usage() {
    let (_dir, app, account) = app(100).await;
    session(&app, &account, "ses_background").await;
    record(
        &app,
        "ses_background",
        &event(1, "session.status", json!({"status":"idle"})),
    )
    .await;
    for start in [2, 4, 6] {
        let id = format!("request-{start}");
        record(
            &app,
            "ses_background",
            &request(start, &id, "started", usage(None, None)),
        )
        .await;
        record(
            &app,
            "ses_background",
            &request(start + 1, &id, "completed", usage(Some(1), Some(0))),
        )
        .await;
    }
    record(
        &app,
        "ses_background",
        &request(8, "partial", "started", usage(None, None)),
    )
    .await;
    record(
        &app,
        "ses_background",
        &request(9, "partial", "failed", usage(None, Some(2))),
    )
    .await;
    let mut compaction = request(10, "compaction", "started", usage(None, None));
    compaction.data["purpose"] = json!("compaction");
    record(&app, "ses_background", &compaction).await;
    record(
        &app,
        "ses_background",
        &event(11, "session.status", json!({"status":"interrupted"})),
    )
    .await;
    let mut end = request(12, "compaction", "completed", usage(Some(7), Some(3)));
    end.data["purpose"] = json!("compaction");
    record(&app, "ses_background", &end).await;
    let totals = model_usage::totals(&app.store, &account).await.unwrap();
    assert_eq!(totals.reported_input_tokens, 10);
    assert_eq!(totals.reported_output_tokens, 5);
    assert_eq!(totals.unmeasured_calls, 1);
    assert_eq!(totals.charged_micro_usd, 5);
    let rows=sqlx::query("SELECT input_tokens,terminal FROM brain_model_usage WHERE session='ses_background' ORDER BY request_id").fetch_all(&app.store.0).await.unwrap();
    assert_eq!(rows.len(), 5);
    assert!(
        rows.iter()
            .any(|row| row.get::<Option<i64>, _>("input_tokens").is_none())
    );
}

#[tokio::test]
async fn delayed_observation_uses_the_price_at_call_start_and_retains_receipts_after_deletion() {
    let (_dir, app, account) = app(100).await;
    session(&app, &account, "ses_prices").await;
    let start = request(1, "priced", "started", usage(None, None));
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    let mut config = (*app.config).clone();
    let billing = config.billing.as_mut().unwrap();
    billing.pricebook.id = "tokens-v2".into();
    billing
        .pricebook
        .rates
        .get_mut(&billing::Meter::ModelTokens)
        .unwrap()
        .micro_usd = 30;
    billing::initialize(&app.store, billing).await.unwrap();
    let mut next = app.clone();
    next.config = std::sync::Arc::new(config);
    billing::settings(
        &next,
        &account,
        BillingSettings {
            pricebook: "tokens-v2".into(),
            spend_limit_micro_usd: 100,
        },
    )
    .await
    .unwrap();
    record(&app, "ses_prices", &start).await;
    record(
        &app,
        "ses_prices",
        &request(2, "priced", "completed", usage(Some(3), Some(0))),
    )
    .await;
    record(
        &app,
        "ses_prices",
        &event(3, "session.status", json!({"status":"closed"})),
    )
    .await;
    app.store.finish_delete("ses_prices").await.unwrap();
    model_usage::reconcile(&app, "ses_prices").await.unwrap();
    assert_eq!(
        model_usage::totals(&app.store, &account)
            .await
            .unwrap()
            .charged_micro_usd,
        1
    );
}

#[tokio::test]
async fn physical_retries_replace_logical_totals_and_concurrent_cursor_replay_never_charges_twice()
{
    let (_dir, app, account) = app(100).await;
    session(&app, &account, "retries").await;
    let logical = request(1, "r", "started", usage(None, None));
    record(&app, "retries", &logical).await;
    for (seq, attempt, tokens) in [(2, "first", 3), (3, "retry", 6)] {
        let mut receipt = request(seq, "r", "completed", usage(Some(tokens), Some(0)));
        receipt.data["boundary"] = json!("physical");
        receipt.data["attempt_id"] = json!(attempt);
        receipt.data["attempts"] = json!("observed");
        record(&app, "retries", &receipt).await;
    }
    record(
        &app,
        "retries",
        &request(4, "r", "completed", usage(Some(6), Some(0))),
    )
    .await;
    let totals = model_usage::totals(&app.store, &account).await.unwrap();
    assert_eq!(totals.reported_input_tokens, 9);
    assert_eq!(totals.rated_micro_usd, 3);
    let closed = event(5, "session.status", json!({"status":"closed"}));
    let (a, b) = tokio::join!(
        model_usage::record(&app, "retries", "cursor:4", &closed),
        model_usage::record(&app, "retries", "cursor:4", &closed)
    );
    assert_ne!(a.unwrap(), b.unwrap());
}
