mod common;
use aex_server::{
    App,
    billing::{self, BillingSettings, Meter, Rate, Reservation, UsageReport},
    config::Config,
    identity, payments,
    store::now,
};
use serde_json::json;
use sqlx::Row;

async fn app() -> (tempfile::TempDir, App, String) {
    let dir = tempfile::tempdir().unwrap();
    let mut config: Config =
        serde_json::from_str(include_str!("../../../examples/config.json")).unwrap();
    config.data_dir = dir.path().into();
    config.billing = Some(serde_json::from_value(json!({
        "pricebook":{"id":"test-v1","rates":{
            "turn_ms":{"micro_usd":1,"units":3}, "sandbox_ms":{"micro_usd":2,"units":1},
            "attachment_byte_secs":{"micro_usd":1,"units":1000}, "egress_bytes":{"micro_usd":1,"units":1}}},
        "max_turn_secs":120,"payments":null
    })).unwrap());
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
    (dir, app, account)
}
async fn reserve(
    app: &App,
    account: &str,
    id: &str,
    meter: Meter,
    units: i64,
    ceiling: Option<i64>,
) -> aex_server::error::Result<bool> {
    let mut tx = app.store.0.begin().await?;
    sqlx::query("SELECT id FROM accounts WHERE id=$1 FOR UPDATE")
        .bind(account)
        .execute(&mut *tx)
        .await?;
    let result = billing::reserve_in(
        &mut tx,
        account,
        Reservation {
            id,
            resource: id,
            meter,
            max_units: units,
            max_cost: ceiling,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(result)
}

#[tokio::test]
async fn concurrent_reservations_cannot_spend_the_same_credit_and_cumulative_rating_preserves_fractions()
 {
    let (_dir, app, account) = app().await;
    billing::adjust(&app.store, &account, "grant", 10, "test credit")
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        reserve(&app, &account, "one", Meter::TurnMs, 30, Some(10)),
        reserve(&app, &account, "two", Meter::TurnMs, 30, Some(10))
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let id = if a.is_ok() { "one" } else { "two" };
    assert_eq!(
        billing::wallet(&app, &account)
            .await
            .unwrap()
            .available_micro_usd,
        0
    );
    for units in [1, 2, 4] {
        billing::meter(
            &app.store,
            &UsageReport {
                id: format!("{id}:{units}"),
                reservation: id.into(),
                units,
                terminal: false,
            },
        )
        .await
        .unwrap();
    }
    let report = UsageReport {
        id: "final".into(),
        reservation: id.into(),
        units: 8,
        terminal: true,
    };
    billing::meter(&app.store, &report).await.unwrap();
    billing::meter(&app.store, &report).await.unwrap();
    let wallet = billing::wallet(&app, &account).await.unwrap();
    assert_eq!(
        (
            wallet.balance_micro_usd,
            wallet.reserved_micro_usd,
            wallet.spent_this_month_micro_usd
        ),
        (8, 0, 2)
    );
    assert!(
        billing::meter(
            &app.store,
            &UsageReport {
                id: "final".into(),
                reservation: id.into(),
                units: 9,
                terminal: true
            }
        )
        .await
        .is_err()
    );
    assert!(
        billing::meter(
            &app.store,
            &UsageReport {
                id: "late".into(),
                reservation: id.into(),
                units: 9,
                terminal: true
            }
        )
        .await
        .is_err()
    );
    let sum: i64 =
        sqlx::query_scalar("SELECT sum(delta)::bigint FROM credit_ledger WHERE account=$1")
            .bind(&account)
            .fetch_one(&app.store.0)
            .await
            .unwrap();
    assert_eq!(sum, wallet.balance_micro_usd);
    assert!(
        sqlx::query("UPDATE credit_ledger SET delta=0 WHERE account=$1")
            .bind(&account)
            .execute(&app.store.0)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM metered_usage")
            .execute(&app.store.0)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn price_acceptance_cost_ceiling_monthly_limit_and_immutable_prices_are_enforced() {
    let (_dir, app, account) = app().await;
    billing::adjust(&app.store, &account, "grant", 100, "test credit")
        .await
        .unwrap();
    assert!(
        reserve(&app, &account, "missing-ceiling", Meter::TurnMs, 3, None)
            .await
            .is_err()
    );
    assert!(
        reserve(&app, &account, "small-ceiling", Meter::TurnMs, 9, Some(2))
            .await
            .is_err()
    );
    billing::settings(
        &app,
        &account,
        BillingSettings {
            pricebook: "test-v1".into(),
            spend_limit_micro_usd: 2,
        },
    )
    .await
    .unwrap();
    assert!(
        reserve(&app, &account, "monthly-limit", Meter::TurnMs, 9, Some(10))
            .await
            .is_err()
    );
    assert!(
        billing::settings(
            &app,
            &account,
            BillingSettings {
                pricebook: "unpublished".into(),
                spend_limit_micro_usd: 10
            }
        )
        .await
        .is_err()
    );
    let mut changed = app.config.billing.clone().unwrap();
    changed
        .pricebook
        .rates
        .get_mut(&Meter::TurnMs)
        .unwrap()
        .micro_usd = 3;
    assert!(billing::initialize(&app.store, &changed).await.is_err());
    let preview = app.store.create_account(&app.config.limits).await.unwrap();
    assert!(
        !reserve(&app, &preview, "free", Meter::TurnMs, 9, None)
            .await
            .unwrap()
    );
    assert!(matches!(
        billing::wallet(&app, &preview).await.unwrap().mode,
        billing::BillingMode::Preview
    ));
    assert!(
        Rate {
            micro_usd: i64::MAX,
            units: 1
        }
        .rate(i64::MAX)
        .is_err()
    );
}

async fn pending_topup(app: &App, account: &str, id: &str) {
    sqlx::query("INSERT INTO topups(id,account,client_key,amount_cents,state,created) VALUES($1,$2,$1,1000,'creating',$3)")
        .bind(id).bind(account).bind(now()).execute(&app.store.0).await.unwrap();
}
fn checkout_event(
    account: &str,
    topup: &str,
    status: &str,
    id: &str,
    kind: &str,
) -> serde_json::Value {
    json!({"id":id,"type":kind,"data":{"object":{"id":format!("cs_{topup}"),"mode":"payment","amount_total":1000,"currency":"usd","payment_status":status,"payment_intent":format!("pi_{topup}"),"metadata":{"aex_topup":topup,"aex_account":account}}}})
}
#[tokio::test]
async fn paid_webhooks_credit_once_ignore_delayed_expiry_and_refunds_settle_only_at_terminal_state()
{
    let (_dir, app, account) = app().await;
    pending_topup(&app, &account, "payment").await;
    let unpaid = checkout_event(
        &account,
        "payment",
        "unpaid",
        "evt_unpaid",
        "checkout.session.completed",
    );
    payments::event(&app, &unpaid).await.unwrap();
    assert_eq!(
        billing::wallet(&app, &account)
            .await
            .unwrap()
            .balance_micro_usd,
        0
    );
    let paid = checkout_event(
        &account,
        "payment",
        "paid",
        "evt_paid",
        "checkout.session.async_payment_succeeded",
    );
    let (a, b) = tokio::join!(payments::event(&app, &paid), payments::event(&app, &paid));
    a.unwrap();
    b.unwrap();
    payments::event(
        &app,
        &checkout_event(
            &account,
            "payment",
            "unpaid",
            "evt_expired",
            "checkout.session.expired",
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        billing::wallet(&app, &account)
            .await
            .unwrap()
            .balance_micro_usd,
        10_000_000
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT state FROM topups WHERE id='payment'")
            .fetch_one(&app.store.0)
            .await
            .unwrap(),
        "paid"
    );
    // The persisted intent reserves funds before Stripe dispatch. Exercise reordered provider receipts.
    sqlx::query("INSERT INTO credit_refunds(id,account,topup,client_key,amount_cents,state,created) VALUES('refund_1',$1,'payment','r',500,'creating',$2)").bind(&account).bind(now()).execute(&app.store.0).await.unwrap();
    sqlx::query("UPDATE wallets SET reserved=5000000 WHERE account=$1")
        .bind(&account)
        .execute(&app.store.0)
        .await
        .unwrap();
    let event = |id: &str, status: &str| json!({"id":id,"type":"refund.updated","data":{"object":{"id":"re_1","amount":500,"currency":"usd","payment_intent":"pi_payment","status":status,"metadata":{"aex_refund":"refund_1"}}}});
    payments::event(&app, &event("refund_pending", "pending"))
        .await
        .unwrap();
    assert_eq!(
        billing::wallet(&app, &account)
            .await
            .unwrap()
            .reserved_micro_usd,
        5_000_000
    );
    payments::event(&app, &event("refund_succeeded", "succeeded"))
        .await
        .unwrap();
    payments::event(&app, &event("refund_old", "pending"))
        .await
        .unwrap();
    let wallet = billing::wallet(&app, &account).await.unwrap();
    assert_eq!(
        (wallet.balance_micro_usd, wallet.reserved_micro_usd),
        (5_000_000, 0)
    );
    let row = sqlx::query("SELECT refunded_cents FROM topups WHERE id='payment'")
        .fetch_one(&app.store.0)
        .await
        .unwrap();
    assert_eq!(row.get::<i64, _>("refunded_cents"), 500);
}

#[tokio::test]
async fn dispute_callbacks_preserve_negative_balances_suspend_new_spend_and_restore_once() {
    let (_dir, app, account) = app().await;
    pending_topup(&app, &account, "payment").await;
    payments::event(
        &app,
        &checkout_event(
            &account,
            "payment",
            "paid",
            "paid",
            "checkout.session.completed",
        ),
    )
    .await
    .unwrap();
    billing::adjust(
        &app.store,
        &account,
        "spent",
        -9_000_000,
        "test existing usage",
    )
    .await
    .unwrap();
    let dispute = |id: &str, kind: &str| json!({"id":id,"type":kind,"data":{"object":{"id":"dp_1","payment_intent":"pi_payment","amount":1000,"currency":"usd"}}});
    payments::event(
        &app,
        &dispute("withdrawn", "charge.dispute.funds_withdrawn"),
    )
    .await
    .unwrap();
    let wallet = billing::wallet(&app, &account).await.unwrap();
    assert!(wallet.suspended);
    assert_eq!(wallet.balance_micro_usd, -9_000_000);
    assert!(
        reserve(&app, &account, "denied", Meter::TurnMs, 3, Some(1))
            .await
            .is_err()
    );
    payments::event(
        &app,
        &dispute("restored", "charge.dispute.funds_reinstated"),
    )
    .await
    .unwrap();
    payments::event(
        &app,
        &dispute("withdrawn_again", "charge.dispute.funds_withdrawn"),
    )
    .await
    .unwrap();
    let wallet = billing::wallet(&app, &account).await.unwrap();
    assert!(!wallet.suspended);
    assert_eq!(wallet.balance_micro_usd, 1_000_000);
}

#[tokio::test]
async fn workload_keys_read_billing_but_cannot_accept_prices_or_create_payments() {
    use axum::{
        body::Bytes,
        http::{HeaderMap, Method},
    };
    let (_dir, app, account) = app().await;
    let (_, key) = app
        .store
        .issue_key(&account, &app.config.limits)
        .await
        .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("authorization", format!("Bearer {key}").parse().unwrap());
    assert!(
        billing::handle(
            &app,
            &Method::GET,
            "/v1/billing",
            None,
            &headers,
            Bytes::new()
        )
        .await
        .is_ok()
    );
    for path in ["/v1/billing", "/v1/billing/topups", "/v1/billing/refunds"] {
        assert!(
            billing::handle(
                &app,
                &Method::POST,
                path,
                None,
                &headers,
                Bytes::from_static(b"{}")
            )
            .await
            .is_err()
        );
    }
    let foreign = identity::random("aex_account");
    headers.insert(
        "authorization",
        format!("Bearer {foreign}").parse().unwrap(),
    );
    assert!(
        billing::handle(
            &app,
            &Method::GET,
            "/v1/billing",
            None,
            &headers,
            Bytes::new()
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn async_turns_retain_holds_through_dispatch_and_replay_without_dispatching_or_charging_twice()
 {
    use axum::{
        Json, Router,
        body::Bytes,
        http::{HeaderMap, StatusCode},
        routing::{get, post},
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let (_dir, mut app, account) = app().await;
    let phase = Arc::new(AtomicUsize::new(0));
    let calls = Arc::new(AtomicUsize::new(0));
    let received = Arc::new(tokio::sync::Notify::new());
    let accept = Arc::new(tokio::sync::Notify::new());
    let status_phase = phase.clone();
    let event_phase = phase.clone();
    let handler_phase = phase.clone();
    let handler_calls = calls.clone();
    let handler_received = received.clone();
    let handler_accept = accept.clone();
    let upstream = Router::new()
        .route("/v1/sessions/ses_billing",get(move || { let phase=status_phase.load(Ordering::SeqCst); async move { Json(json!({"session_id":"ses_billing","status":if phase == 1 {"running"} else {"idle"},"last_sequence":phase+1})) } }))
        .route("/v1/sessions/ses_billing/events",get(move || { let phase=event_phase.load(Ordering::SeqCst); async move {
            Json(json!({"events":if phase==2 {vec![json!({"sequence":2,"recorded_at_ms":1000,"event_type":"turn_started","data":{}}),json!({"sequence":3,"recorded_at_ms":1008,"event_type":"turn_ended","data":{}})]} else {vec![]},"next_cursor":phase+1}))
        }}))
        .route("/v1/sessions/ses_billing/messages",post(move |headers:HeaderMap| {
            let phase=handler_phase.clone(); let calls=handler_calls.clone(); let received=handler_received.clone(); let accept=handler_accept.clone();
            async move { assert_eq!(headers["prefer"],"respond-async"); calls.fetch_add(1,Ordering::SeqCst); received.notify_one(); accept.notified().await; phase.store(1,Ordering::SeqCst); (StatusCode::ACCEPTED,Json(json!({"session_id":"ses_billing","sequence":2}))) }
        }));
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = Arc::make_mut(&mut app.config);
    config.brain_url = format!("http://{}", socket.local_addr().unwrap());
    config.limits.minimum_free_disk_bytes = 1;
    app.brain = aex_server::brain::Brain::new(config, "b".repeat(32)).unwrap();
    let server = tokio::spawn(async move { axum::serve(socket, upstream).await.unwrap() });
    app.store
        .report_usage(now(), Default::default())
        .await
        .unwrap();
    sqlx::query("INSERT INTO sessions(id,account,created) VALUES('ses_billing',$1,$2)")
        .bind(&account)
        .bind(now())
        .execute(&app.store.0)
        .await
        .unwrap();
    billing::adjust(&app.store, &account, "grant", 100_000, "test credit")
        .await
        .unwrap();
    let (_, key) = app
        .store
        .issue_key(&account, &app.config.limits)
        .await
        .unwrap();
    let principal = app.store.principal(&key).await.unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("prefer", "respond-async".parse().unwrap());
    headers.insert("idempotency-key", "send-once".parse().unwrap());
    headers.insert("x-aex-max-cost-micro-usd", "40000".parse().unwrap());
    let input = Bytes::from_static(br#"{"input":{"message":"run"}}"#);
    let owned_app = app.clone();
    let owned_principal = principal.clone();
    let owned_headers = headers.clone();
    let owned_input = input.clone();
    let submit = tokio::spawn(async move {
        aex_server::turns::send(
            &owned_app,
            &owned_principal,
            "ses_billing",
            &owned_headers,
            owned_input,
        )
        .await
    });
    received.notified().await;
    assert!(
        aex_server::turns::reconcile(&app, "ses_billing")
            .await
            .is_err()
    );
    assert_eq!(
        billing::wallet(&app, &account)
            .await
            .unwrap()
            .reserved_micro_usd,
        40_000
    );
    accept.notify_one();
    assert_eq!(
        submit.await.unwrap().unwrap().status(),
        StatusCode::ACCEPTED
    );
    assert!(
        !aex_server::turns::reconcile(&app, "ses_billing")
            .await
            .unwrap()
    );
    let replay = aex_server::turns::send(&app, &principal, "ses_billing", &headers, input.clone())
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::ACCEPTED);
    phase.store(2, Ordering::SeqCst);
    assert!(
        aex_server::turns::reconcile(&app, "ses_billing")
            .await
            .unwrap()
    );
    assert!(
        aex_server::turns::reconcile(&app, "ses_billing")
            .await
            .unwrap()
    );
    assert_eq!(
        billing::wallet(&app, &account)
            .await
            .unwrap()
            .balance_micro_usd,
        99_998
    );
    assert_eq!(
        billing::wallet(&app, &account)
            .await
            .unwrap()
            .reserved_micro_usd,
        0
    );
    assert_eq!(
        aex_server::turns::send(&app, &principal, "ses_billing", &headers, input)
            .await
            .unwrap()
            .status(),
        StatusCode::ACCEPTED
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    server.abort();
}
