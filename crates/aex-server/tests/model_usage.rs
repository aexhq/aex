mod common;

use aex_server::{
    App,
    billing::{self, BillingSettings},
    config::Config,
    model_usage,
};
use brain_protocol::Event;
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
    sqlx::query("INSERT INTO sessions(id,account,created,model_observed_at) VALUES($1,$2,0,$3)")
        .bind(id)
        .bind(account)
        .bind(model_usage::now_ms())
        .execute(&app.store.0)
        .await
        .unwrap();
}

fn event(sequence: u64, kind: &str, data: serde_json::Value) -> Event {
    Event {
        sequence,
        recorded_at_ms: model_usage::now_ms() as u64,
        event_type: kind.into(),
        data,
        origin: None,
    }
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
    use axum::{
        Router,
        response::{Sse, sse::Event as SseEvent},
        routing::{get, post},
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let (_dir, mut app, account) = app(4).await;
    session(&app, &account, "ses_stream").await;
    let cancelled = Arc::new(tokio::sync::Notify::new());
    let cancel_count = Arc::new(AtomicUsize::new(0));
    let receiver = cancelled.clone();
    let sender = cancelled.clone();
    let count = cancel_count.clone();
    let upstream = Router::new().route("/v1/sessions/ses_stream/events",get(move || {
        let receiver = receiver.clone();
        async move {
            Sse::new(async_stream::stream! {
                let start = event(1,"model_call_started",json!({}));
                yield Ok::<_,std::convert::Infallible>(SseEvent::default().id("1").event("model_call_started").json_data(start).unwrap());
                yield Ok(SseEvent::default().event("model_request").json_data(json!({"model_call_sequence":1,"input_bytes":6000,"media_inputs":0})).unwrap());
                receiver.notified().await;
                let ended = event(2,"model_call_failed",json!({"sequence":1,"response":{"usage":{"total_input_tokens":30,"output_tokens":0},"usage_complete":false}}));
                yield Ok(SseEvent::default().id("2").event("model_call_failed").json_data(ended).unwrap());
                yield Ok(SseEvent::default().id("3").event("session_ended").json_data(event(3,"session_ended",json!({}))).unwrap());
            })
        }
    })).route("/v1/sessions/ses_stream/cancel",post(move || {
        count.fetch_add(1,Ordering::SeqCst);
        sender.notify_one();
        async { axum::Json(json!({})) }
    }));
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
    assert_eq!(cancel_count.load(Ordering::SeqCst), 1);
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
        model_usage::record(&app, id, &event(1, "model_call_started", json!({})))
            .await
            .unwrap();
    }
    let receipt = event(
        2,
        "model_call_ended",
        json!({"sequence":1,"result":{"usage":{
            "total_input_tokens":4,"input_tokens":4,"cache_read_input_tokens":3,"output_tokens":2,"reasoning_tokens":1
        }}}),
    );
    let (a, b, c) = tokio::join!(
        model_usage::record(&app, "ses_one", &receipt),
        model_usage::record(&app, "ses_two", &receipt),
        model_usage::record(&app, "ses_three", &receipt)
    );
    a.unwrap();
    b.unwrap();
    c.unwrap();
    model_usage::record(&app, "ses_one", &receipt)
        .await
        .unwrap();
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
    model_usage::record(&app, "ses_background", &event(1, "turn_ended", json!({})))
        .await
        .unwrap();
    for start in [2, 4, 6] {
        model_usage::record(
            &app,
            "ses_background",
            &event(start, "model_call_started", json!({})),
        )
        .await
        .unwrap();
        model_usage::record(&app,"ses_background",&event(start+1,"model_call_ended",json!({"sequence":start,"result":{"usage":{"total_input_tokens":1,"output_tokens":0}}}))).await.unwrap();
    }
    model_usage::record(
        &app,
        "ses_background",
        &event(8, "model_call_started", json!({})),
    )
    .await
    .unwrap();
    model_usage::record(
        &app,
        "ses_background",
        &event(
            9,
            "model_call_failed",
            json!({"sequence":8,"response":{"usage":{"output_tokens":2},"usage_complete":false}}),
        ),
    )
    .await
    .unwrap();
    let mut spoof = event(10, "model_call_started", json!({}));
    spoof.origin = Some(serde_json::from_value(json!({"kind":"tool","sequence":2})).unwrap());
    model_usage::record(&app, "ses_background", &spoof)
        .await
        .unwrap();
    let totals = model_usage::totals(&app.store, &account).await.unwrap();
    assert_eq!(totals.reported_input_tokens, 3);
    assert_eq!(totals.reported_output_tokens, 2);
    assert_eq!(totals.unmeasured_calls, 1);
    assert_eq!(totals.charged_micro_usd, 1);
    let rows = sqlx::query("SELECT input_tokens,terminal FROM model_usage WHERE session='ses_background' ORDER BY sequence").fetch_all(&app.store.0).await.unwrap();
    assert_eq!(rows.len(), 4);
    assert!(rows[3].get::<Option<i64>, _>("input_tokens").is_none());
}

#[tokio::test]
async fn delayed_observation_uses_the_price_at_call_start_and_retains_receipts_after_deletion() {
    let (_dir, app, account) = app(100).await;
    session(&app, &account, "ses_prices").await;
    let start = event(1, "model_call_started", json!({}));
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
    model_usage::record(&app, "ses_prices", &start)
        .await
        .unwrap();
    model_usage::record(
        &app,
        "ses_prices",
        &event(
            2,
            "model_call_ended",
            json!({"sequence":1,"result":{"usage":{"total_input_tokens":3,"output_tokens":0}}}),
        ),
    )
    .await
    .unwrap();
    model_usage::record(&app, "ses_prices", &event(3, "session_ended", json!({})))
        .await
        .unwrap();
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
