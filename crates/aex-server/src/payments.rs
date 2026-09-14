use crate::{
    App, billing,
    error::{Error, Result},
    identity,
    store::{Store, now},
};
use axum::{
    Json,
    body::Bytes,
    http::{HeaderMap, Method},
    response::{IntoResponse, Response},
};
use hmac::{Hmac, KeyInit, Mac};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Sha256;
use sqlx::{Postgres, Row, Transaction};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Test,
    Live,
}
#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub mode: Mode,
    pub return_url: String,
    pub topup_amounts_cents: Vec<i64>,
}
impl Config {
    pub fn validate(&self) -> anyhow::Result<()> {
        let url = url::Url::parse(&self.return_url)?;
        anyhow::ensure!(
            url.scheme() == "https"
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "billing return_url must be a fixed HTTPS page"
        );
        anyhow::ensure!(
            !self.topup_amounts_cents.is_empty()
                && self
                    .topup_amounts_cents
                    .iter()
                    .all(|n| (100..=100_000).contains(n)),
            "topup amounts must be between $1 and $1,000"
        );
        Ok(())
    }
}

/// Stripe credentials remain in the trusted service and are never exposed to workloads.
pub struct Stripe {
    client: reqwest::Client,
    secret: String,
    webhook: String,
    mode: Mode,
}
impl Stripe {
    pub fn from_env(config: &Config) -> anyhow::Result<Self> {
        let secret = std::env::var("STRIPE_SECRET_KEY")?;
        let webhook = std::env::var("STRIPE_WEBHOOK_SECRET")?;
        let prefix = if config.mode == Mode::Live {
            "sk_live_"
        } else {
            "sk_test_"
        };
        anyhow::ensure!(
            secret.starts_with(prefix)
                && secret.len() > prefix.len()
                && webhook.starts_with("whsec_")
                && webhook.len() > 6,
            "Stripe credentials must match configured payment mode"
        );
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(20))
                .build()?,
            secret,
            webhook,
            mode: config.mode,
        })
    }
    async fn request(
        &self,
        method: Method,
        path: &str,
        key: Option<&str>,
        form: &[(String, String)],
    ) -> Result<Value> {
        let mut request = self
            .client
            .request(method, format!("https://api.stripe.com/v1/{path}"))
            .bearer_auth(&self.secret)
            .header("stripe-version", "2025-02-24.acacia");
        if let Some(key) = key {
            request = request.header("idempotency-key", key);
        }
        if !form.is_empty() {
            request = request.form(form);
        }
        let response = request.send().await.map_err(|_| Error::ambiguous())?;
        if response.status().is_client_error() && !matches!(response.status().as_u16(), 409 | 429) {
            let mut error = Error::invalid("payment provider rejected the request");
            error.1.code = "payment_rejected".into();
            return Err(error);
        }
        if !response.status().is_success() {
            tracing::warn!(
                status = response.status().as_u16(),
                "Stripe request failed; durable payment intent retained"
            );
            return Err(Error::ambiguous());
        }
        response.json().await.map_err(|_| Error::ambiguous())
    }
    pub fn verify(&self, body: &[u8], signature: &str) -> Result<Value> {
        verify(body, signature, &self.webhook, self.mode, now())
    }
}

fn verify(body: &[u8], signature: &str, secret: &str, mode: Mode, at: i64) -> Result<Value> {
    let parts: Vec<_> = signature
        .split(',')
        .filter_map(|p| p.split_once('='))
        .collect();
    let timestamps: Vec<_> = parts.iter().filter(|(k, _)| *k == "t").collect();
    if timestamps.len() != 1 {
        return Err(Error::denied());
    }
    let timestamp = timestamps[0]
        .1
        .parse::<i64>()
        .map_err(|_| Error::denied())?;
    if at.abs_diff(timestamp) > 300 {
        return Err(Error::denied());
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_err(|_| Error::denied())?;
    mac.update(timestamps[0].1.as_bytes());
    mac.update(b".");
    mac.update(body);
    if !parts
        .iter()
        .filter(|(k, _)| *k == "v1")
        .any(|(_, v)| hex::decode(v).is_ok_and(|bytes| mac.clone().verify_slice(&bytes).is_ok()))
    {
        return Err(Error::denied());
    }
    let event: Value = serde_json::from_slice(body)?;
    if event["livemode"].as_bool() != Some(mode == Mode::Live) {
        return Err(Error::denied());
    }
    Ok(event)
}
fn string<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value[field]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 1000)
        .ok_or_else(|| Error::invalid("invalid payment event"))
}
fn amount(value: &Value, field: &str) -> Result<i64> {
    value[field]
        .as_i64()
        .filter(|n| *n > 0 && *n <= 100_000)
        .ok_or_else(|| Error::invalid("invalid payment amount"))
}
fn configured(app: &App) -> Result<(&Config, &Stripe)> {
    match (
        app.config
            .billing
            .as_ref()
            .and_then(|b| b.payments.as_ref()),
        app.payments.as_deref(),
    ) {
        (Some(config), Some(stripe)) => Ok((config, stripe)),
        _ => Err(Error::invalid("payments are not configured")),
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TopupInput {
    pub amount_cents: i64,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RefundInput {
    pub topup: String,
    pub amount_cents: i64,
}
#[derive(Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SyncPayment {
    Topup {
        id: String,
        checkout_id: Option<String>,
    },
    Refund {
        id: String,
        refund_id: Option<String>,
    },
}
#[derive(Serialize, JsonSchema)]
pub struct Topup {
    pub id: String,
    pub amount_cents: i64,
    pub state: String,
    pub checkout_url: Option<String>,
    pub receipt_url: Option<String>,
    pub refunded_cents: i64,
    pub created: i64,
}
#[derive(Serialize, JsonSchema)]
pub struct Refund {
    pub id: String,
    pub topup: String,
    pub amount_cents: i64,
    pub state: String,
    pub created: i64,
}
fn topup(row: &sqlx::postgres::PgRow) -> Topup {
    Topup {
        id: row.get("id"),
        amount_cents: row.get("amount_cents"),
        state: row.get("state"),
        checkout_url: row.get("checkout_url"),
        receipt_url: row.get("receipt_url"),
        refunded_cents: row.get("refunded_cents"),
        created: row.get("created"),
    }
}
fn refund(row: &sqlx::postgres::PgRow) -> Refund {
    Refund {
        id: row.get("id"),
        topup: row.get("topup"),
        amount_cents: row.get("amount_cents"),
        state: row.get("state"),
        created: row.get("created"),
    }
}

async fn create_topup(app: &App, account: &str, key: &str, input: TopupInput) -> Result<Topup> {
    let (config, stripe) = configured(app)?;
    if !config.topup_amounts_cents.contains(&input.amount_cents) {
        return Err(Error::invalid("choose an offered topup amount"));
    }
    let mut tx = app.store.0.begin().await?;
    Store::lock_account(&mut tx, account).await?;
    if let Some(saved) = sqlx::query("SELECT * FROM topups WHERE account=$1 AND client_key=$2")
        .bind(account)
        .bind(key)
        .fetch_optional(&mut *tx)
        .await?
    {
        if saved.get::<i64, _>("amount_cents") != input.amount_cents {
            return Err(Error::conflict("topup key reused with different amount"));
        }
        if saved.get::<String, _>("state") == "creating" {
            return Err(Error::ambiguous());
        }
        return Ok(topup(&saved));
    }
    let wallet = sqlx::query("SELECT suspended FROM wallets WHERE account=$1")
        .bind(account)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| Error::invalid("accept the pricebook before topping up"))?;
    if wallet.get::<bool, _>("suspended") {
        return Err(Error::credits("billing account is suspended"));
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM topups WHERE account=$1 AND state IN ('creating','open')",
    )
    .bind(account)
    .fetch_one(&mut *tx)
    .await?;
    if count >= 3 {
        return Err(Error::conflict(
            "finish or expire an existing checkout first",
        ));
    }
    let id = identity::random("topup");
    sqlx::query("INSERT INTO topups(id,account,client_key,amount_cents,state,created) VALUES($1,$2,$3,$4,'creating',$5)").bind(&id).bind(account).bind(key).bind(input.amount_cents).bind(now()).execute(&mut *tx).await?;
    tx.commit().await?;
    let form = vec![
        ("mode".into(), "payment".into()),
        ("currency".into(), "usd".into()),
        ("payment_method_types[0]".into(), "card".into()),
        ("client_reference_id".into(), id.clone()),
        ("metadata[aex_topup]".into(), id.clone()),
        ("metadata[aex_account]".into(), account.into()),
        (
            "payment_intent_data[metadata][aex_topup]".into(),
            id.clone(),
        ),
        (
            "payment_intent_data[metadata][aex_account]".into(),
            account.into(),
        ),
        ("line_items[0][price_data][currency]".into(), "usd".into()),
        (
            "line_items[0][price_data][unit_amount]".into(),
            input.amount_cents.to_string(),
        ),
        (
            "line_items[0][price_data][product_data][name]".into(),
            "Aex prepaid credits".into(),
        ),
        ("line_items[0][quantity]".into(), "1".into()),
        (
            "success_url".into(),
            format!("{}?topup={id}", config.return_url),
        ),
        ("cancel_url".into(), config.return_url.clone()),
    ];
    let checkout = stripe
        .request(Method::POST, "checkout/sessions", Some(&id), &form)
        .await;
    if let Err(error) = &checkout
        && error.1.code == "payment_rejected"
    {
        sqlx::query("UPDATE topups SET state='failed' WHERE id=$1 AND state='creating'")
            .bind(&id)
            .execute(&app.store.0)
            .await?;
    }
    let checkout = checkout?;
    let checkout_id = string(&checkout, "id")?;
    let checkout_url = string(&checkout, "url")?;
    // A webhook can arrive before this request finishes. Never overwrite its paid state.
    let row = sqlx::query("UPDATE topups SET checkout_id=$1,checkout_url=$2,state=CASE WHEN state='creating' THEN 'open' ELSE state END WHERE id=$3 AND (checkout_id IS NULL OR checkout_id=$1) RETURNING *")
        .bind(checkout_id).bind(checkout_url).bind(&id).fetch_optional(&app.store.0).await?.ok_or_else(Error::ambiguous)?;
    Ok(topup(&row))
}

async fn create_refund(app: &App, account: &str, key: &str, input: RefundInput) -> Result<Refund> {
    let (_, stripe) = configured(app)?;
    if input.amount_cents <= 0 || input.amount_cents > 100_000 {
        return Err(Error::invalid("invalid refund amount"));
    }
    let mut tx = app.store.0.begin().await?;
    Store::lock_account(&mut tx, account).await?;
    if let Some(saved) =
        sqlx::query("SELECT * FROM credit_refunds WHERE account=$1 AND client_key=$2")
            .bind(account)
            .bind(key)
            .fetch_optional(&mut *tx)
            .await?
    {
        if saved.get::<String, _>("topup") != input.topup
            || saved.get::<i64, _>("amount_cents") != input.amount_cents
        {
            return Err(Error::conflict("refund key reused with different request"));
        }
        return Ok(refund(&saved));
    }
    let wallet = sqlx::query("SELECT balance,reserved,suspended FROM wallets WHERE account=$1")
        .bind(account)
        .fetch_one(&mut *tx)
        .await?;
    let credit = input.amount_cents * 10_000;
    if wallet.get::<bool, _>("suspended")
        || wallet.get::<i64, _>("balance") - wallet.get::<i64, _>("reserved") < credit
    {
        return Err(Error::credits("refund exceeds available credits"));
    }
    let payment = sqlx::query("SELECT * FROM topups WHERE id=$1 AND account=$2 AND state='paid'")
        .bind(&input.topup)
        .bind(account)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(Error::missing)?;
    let pending: i64 = sqlx::query_scalar("SELECT coalesce(sum(amount_cents),0)::bigint FROM credit_refunds WHERE topup=$1 AND state IN ('creating','pending')").bind(&input.topup).fetch_one(&mut *tx).await?;
    if payment.get::<i64, _>("refunded_cents") + pending + input.amount_cents
        > payment.get::<i64, _>("amount_cents")
    {
        return Err(Error::invalid("refund exceeds the original payment"));
    }
    let payment_intent: String = payment.get("payment_intent");
    let id = identity::random("refund");
    sqlx::query("INSERT INTO credit_refunds(id,account,topup,client_key,amount_cents,state,created) VALUES($1,$2,$3,$4,$5,'creating',$6)")
        .bind(&id).bind(account).bind(&input.topup).bind(key).bind(input.amount_cents).bind(now()).execute(&mut *tx).await?;
    sqlx::query("UPDATE wallets SET reserved=reserved+$1 WHERE account=$2")
        .bind(credit)
        .bind(account)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let value = stripe
        .request(
            Method::POST,
            "refunds",
            Some(&id),
            &[
                ("payment_intent".into(), payment_intent),
                ("amount".into(), input.amount_cents.to_string()),
                ("metadata[aex_refund]".into(), id.clone()),
            ],
        )
        .await;
    if let Err(error) = &value
        && error.1.code == "payment_rejected"
    {
        let mut tx = app.store.0.begin().await?;
        Store::lock_account(&mut tx, account).await?;
        let changed = sqlx::query(
            "UPDATE credit_refunds SET state='failed' WHERE id=$1 AND state='creating'",
        )
        .bind(&id)
        .execute(&mut *tx)
        .await?;
        if changed.rows_affected() == 1 {
            sqlx::query("UPDATE wallets SET reserved=reserved-$1 WHERE account=$2")
                .bind(credit)
                .bind(account)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
    }
    apply_refund(&app.store, &value?).await?;
    let row = sqlx::query("SELECT * FROM credit_refunds WHERE id=$1")
        .bind(id)
        .fetch_one(&app.store.0)
        .await?;
    Ok(refund(&row))
}

pub async fn handle(
    app: &App,
    account: &str,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body: Bytes,
) -> Result<Response> {
    if path == "/v1/billing/sync" {
        sync(app, account, serde_json::from_slice(&body)?).await?;
        return Ok(Json(billing::wallet(app, account).await?).into_response());
    }
    if method == Method::POST {
        let key = identity::operation_key(headers)?;
        return if path.ends_with("/topups") {
            Ok(
                Json(create_topup(app, account, key, serde_json::from_slice(&body)?).await?)
                    .into_response(),
            )
        } else {
            Ok(
                Json(create_refund(app, account, key, serde_json::from_slice(&body)?).await?)
                    .into_response(),
            )
        };
    }
    if path.ends_with("/topups") {
        let rows = sqlx::query(
            "SELECT * FROM topups WHERE account=$1 ORDER BY created DESC,id DESC LIMIT 100",
        )
        .bind(account)
        .fetch_all(&app.store.0)
        .await?;
        Ok(Json(rows.iter().map(topup).collect::<Vec<_>>()).into_response())
    } else {
        let rows = sqlx::query(
            "SELECT * FROM credit_refunds WHERE account=$1 ORDER BY created DESC,id DESC LIMIT 100",
        )
        .bind(account)
        .fetch_all(&app.store.0)
        .await?;
        Ok(Json(rows.iter().map(refund).collect::<Vec<_>>()).into_response())
    }
}

async fn checkout(store: &Store, object: &Value, kind: &str) -> Result<()> {
    let Some(id) = object["metadata"]["aex_topup"].as_str() else {
        return Ok(());
    };
    let account = string(&object["metadata"], "aex_account")?;
    let mut tx = store.0.begin().await?;
    Store::lock_account(&mut tx, account).await?;
    let row = sqlx::query("SELECT * FROM topups WHERE id=$1 AND account=$2")
        .bind(id)
        .bind(account)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(Error::missing)?;
    let checkout_id = string(object, "id")?;
    if object["currency"] != "usd"
        || object["mode"] != "payment"
        || amount(object, "amount_total")? != row.get::<i64, _>("amount_cents")
        || row
            .get::<Option<String>, _>("checkout_id")
            .is_some_and(|saved| saved != checkout_id)
    {
        return Err(Error::conflict("payment does not match topup"));
    }
    if object["payment_status"] == "paid" {
        let intent = string(object, "payment_intent")?;
        billing::entry(
            &mut tx,
            account,
            "topup",
            id,
            row.get::<i64, _>("amount_cents") * 10_000,
            "Stripe prepaid topup",
        )
        .await?;
        sqlx::query("UPDATE topups SET state='paid',checkout_id=$1,payment_intent=$2 WHERE id=$3")
            .bind(checkout_id)
            .bind(intent)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    } else if row.get::<String, _>("state") != "paid" {
        let state = if kind == "checkout.session.expired" {
            "expired"
        } else if kind == "checkout.session.async_payment_failed" {
            "failed"
        } else {
            "open"
        };
        sqlx::query("UPDATE topups SET state=$1,checkout_id=$2 WHERE id=$3")
            .bind(state)
            .bind(checkout_id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

async fn refund_in(
    tx: &mut Transaction<'_, Postgres>,
    row: &sqlx::postgres::PgRow,
    object: &Value,
) -> Result<()> {
    let intent: String = sqlx::query_scalar("SELECT payment_intent FROM topups WHERE id=$1")
        .bind(row.get::<String, _>("topup"))
        .fetch_one(&mut **tx)
        .await?;
    if object["payment_intent"].as_str() != Some(intent.as_str()) {
        return Err(Error::conflict("refund belongs to another payment"));
    }
    let cents = amount(object, "amount")?;
    let provider = string(object, "id")?;
    let state = string(object, "status")?;
    if object["currency"] != "usd"
        || cents != row.get::<i64, _>("amount_cents")
        || !matches!(
            state,
            "pending" | "requires_action" | "succeeded" | "failed" | "canceled"
        )
        || row
            .get::<Option<String>, _>("provider_id")
            .is_some_and(|id| id != provider)
    {
        return Err(Error::conflict("refund does not match its intent"));
    }
    let previous: String = row.get("state");
    if matches!(previous.as_str(), "succeeded" | "failed" | "canceled") {
        return Ok(());
    }
    let account: String = row.get("account");
    if matches!(state, "succeeded" | "failed" | "canceled") {
        sqlx::query("UPDATE wallets SET reserved=reserved-$1 WHERE account=$2")
            .bind(cents * 10_000)
            .bind(&account)
            .execute(&mut **tx)
            .await?;
        if state == "succeeded" {
            billing::entry(
                tx,
                &account,
                "refund",
                provider,
                -cents * 10_000,
                "Stripe credit refund",
            )
            .await?;
            sqlx::query("UPDATE topups SET refunded_cents=refunded_cents+$1 WHERE id=$2")
                .bind(cents)
                .bind(row.get::<String, _>("topup"))
                .execute(&mut **tx)
                .await?;
        }
    }
    sqlx::query("UPDATE credit_refunds SET state=$1,provider_id=$2 WHERE id=$3")
        .bind(if state == "requires_action" {
            "pending"
        } else {
            state
        })
        .bind(provider)
        .bind(row.get::<String, _>("id"))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn apply_refund(store: &Store, object: &Value) -> Result<()> {
    if let Some(id) = object["metadata"]["aex_refund"].as_str() {
        let account: String = sqlx::query_scalar("SELECT account FROM credit_refunds WHERE id=$1")
            .bind(id)
            .fetch_optional(&store.0)
            .await?
            .ok_or_else(Error::missing)?;
        let mut tx = store.0.begin().await?;
        Store::lock_account(&mut tx, &account).await?;
        let row = sqlx::query("SELECT * FROM credit_refunds WHERE id=$1")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
        refund_in(&mut tx, &row, object).await?;
        tx.commit().await?;
    } else if object["status"] == "succeeded" {
        let intent = string(object, "payment_intent")?;
        let Some(payment) = sqlx::query("SELECT id,account FROM topups WHERE payment_intent=$1")
            .bind(intent)
            .fetch_optional(&store.0)
            .await?
        else {
            return Err(Error::ambiguous());
        };
        let account: String = payment.get("account");
        let mut tx = store.0.begin().await?;
        Store::lock_account(&mut tx, &account).await?;
        let cents = amount(object, "amount")?;
        if object["currency"] != "usd" {
            return Err(Error::conflict("refund currency mismatch"));
        }
        let provider = string(object, "id")?;
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM credit_ledger WHERE account=$1 AND kind='refund' AND reference=$2)").bind(&account).bind(provider).fetch_one(&mut *tx).await?;
        billing::entry(
            &mut tx,
            &account,
            "refund",
            provider,
            -cents * 10_000,
            "Stripe credit refund",
        )
        .await?;
        if !exists {
            sqlx::query("UPDATE topups SET refunded_cents=refunded_cents+$1 WHERE id=$2")
                .bind(cents)
                .bind(payment.get::<String, _>("id"))
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
    }
    Ok(())
}

async fn dispute(store: &Store, object: &Value, restored: bool) -> Result<()> {
    let intent = string(object, "payment_intent")?;
    let Some(account) =
        sqlx::query_scalar::<_, String>("SELECT account FROM topups WHERE payment_intent=$1")
            .bind(intent)
            .fetch_optional(&store.0)
            .await?
    else {
        return Err(Error::ambiguous());
    };
    let id = string(object, "id")?;
    let credit = amount(object, "amount")? * 10_000;
    if object["currency"] != "usd" {
        return Err(Error::conflict("dispute currency mismatch"));
    }
    let mut tx = store.0.begin().await?;
    Store::lock_account(&mut tx, &account).await?;
    sqlx::query("INSERT INTO payment_disputes(id,account,amount) VALUES($1,$2,$3) ON CONFLICT(id) DO NOTHING").bind(id).bind(&account).bind(credit).execute(&mut *tx).await?;
    let row = sqlx::query("UPDATE payment_disputes SET withdrawn=withdrawn OR $1,restored=restored OR $2 WHERE id=$3 AND account=$4 AND amount=$5 RETURNING *")
        .bind(!restored).bind(restored).bind(id).bind(&account).bind(credit).fetch_optional(&mut *tx).await?.ok_or_else(|| Error::conflict("dispute changed amount"))?;
    if row.get::<bool, _>("withdrawn") {
        billing::entry(
            &mut tx,
            &account,
            "reversal",
            id,
            -credit,
            "Stripe disputed funds withdrawn",
        )
        .await?;
        if row.get::<bool, _>("restored") {
            billing::entry(
                &mut tx,
                &account,
                "adjustment",
                &format!("dispute-restored:{id}"),
                credit,
                "Stripe disputed funds restored",
            )
            .await?;
        }
    }
    sqlx::query("UPDATE wallets SET suspended=EXISTS(SELECT 1 FROM payment_disputes WHERE account=$1 AND withdrawn AND NOT restored) WHERE account=$1").bind(account).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn event(app: &App, event: &Value) -> Result<()> {
    let id = string(event, "id")?;
    let kind = string(event, "type")?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM payment_events WHERE id=$1)")
            .bind(id)
            .fetch_one(&app.store.0)
            .await?;
    if exists {
        return Ok(());
    }
    let object = &event["data"]["object"];
    match kind {
        "checkout.session.completed"
        | "checkout.session.async_payment_succeeded"
        | "checkout.session.async_payment_failed"
        | "checkout.session.expired" => checkout(&app.store, object, kind).await?,
        "refund.created" | "refund.updated" | "refund.failed" => {
            apply_refund(&app.store, object).await?
        }
        "charge.dispute.funds_withdrawn" => dispute(&app.store, object, false).await?,
        "charge.dispute.funds_reinstated" => dispute(&app.store, object, true).await?,
        _ => {}
    }
    // Money mutations above carry their own durable deduplication identities. A crash here is safe to replay.
    sqlx::query("INSERT INTO payment_events VALUES($1,$2,$3) ON CONFLICT(id) DO NOTHING")
        .bind(id)
        .bind(kind)
        .bind(now())
        .execute(&app.store.0)
        .await?;
    Ok(())
}
pub async fn webhook(app: &App, headers: &HeaderMap, body: Bytes) -> Result<Response> {
    let (_, stripe) = configured(app)?;
    let signature = headers
        .get("stripe-signature")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(Error::denied)?;
    event(app, &stripe.verify(&body, signature)?).await?;
    Ok(Json(json!({"received":true})).into_response())
}

async fn sync(app: &App, account: &str, input: SyncPayment) -> Result<()> {
    let (_, stripe) = configured(app)?;
    let provider_id =
        |saved: Option<String>, supplied: Option<String>, prefix: &str| -> Result<String> {
            if saved.is_some() && supplied.is_some() && saved != supplied {
                return Err(Error::conflict(
                    "provider reference does not match the saved payment",
                ));
            }
            saved
                .or(supplied)
                .filter(|id| {
                    id.starts_with(prefix)
                        && id.len() <= 200
                        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                })
                .ok_or_else(Error::ambiguous)
        };
    match input {
        SyncPayment::Topup { id, checkout_id } => {
            let row = sqlx::query("SELECT checkout_id FROM topups WHERE id=$1 AND account=$2")
                .bind(&id)
                .bind(account)
                .fetch_optional(&app.store.0)
                .await?
                .ok_or_else(Error::missing)?;
            let provider = provider_id(row.get("checkout_id"), checkout_id, "cs_")?;
            let object = stripe
                .request(
                    Method::GET,
                    &format!("checkout/sessions/{provider}"),
                    None,
                    &[],
                )
                .await?;
            if object["metadata"]["aex_topup"].as_str() != Some(id.as_str())
                || object["metadata"]["aex_account"].as_str() != Some(account)
            {
                return Err(Error::conflict("checkout belongs to another topup"));
            }
            let kind = if object["status"] == "expired" {
                "checkout.session.expired"
            } else {
                "checkout.session.completed"
            };
            checkout(&app.store, &object, kind).await?;
            if object["payment_status"] == "paid" {
                let intent = string(&object, "payment_intent")?;
                let payment = stripe
                    .request(
                        Method::GET,
                        &format!("payment_intents/{intent}?expand[]=latest_charge"),
                        None,
                        &[],
                    )
                    .await?;
                if let Some(url) = payment["latest_charge"]["receipt_url"].as_str() {
                    sqlx::query("UPDATE topups SET receipt_url=$1 WHERE id=$2")
                        .bind(url)
                        .bind(id)
                        .execute(&app.store.0)
                        .await?;
                }
            }
        }
        SyncPayment::Refund { id, refund_id } => {
            let row =
                sqlx::query("SELECT provider_id FROM credit_refunds WHERE id=$1 AND account=$2")
                    .bind(&id)
                    .bind(account)
                    .fetch_optional(&app.store.0)
                    .await?
                    .ok_or_else(Error::missing)?;
            let provider = provider_id(row.get("provider_id"), refund_id, "re_")?;
            let object = stripe
                .request(Method::GET, &format!("refunds/{provider}"), None, &[])
                .await?;
            if object["metadata"]["aex_refund"].as_str() != Some(id.as_str()) {
                return Err(Error::conflict("refund belongs to another intent"));
            }
            apply_refund(&app.store, &object).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signed_raw_events_require_recent_timestamp_matching_mode_and_untouched_bytes() {
        let body = br#"{"livemode":false,"id":"evt_1"}"#;
        let mut mac = Hmac::<Sha256>::new_from_slice(b"whsec_test").unwrap();
        mac.update(b"1000.");
        mac.update(body);
        let signature = format!("t=1000,v1={}", hex::encode(mac.finalize().into_bytes()));
        assert!(verify(body, &signature, "whsec_test", Mode::Test, 1001).is_ok());
        assert!(verify(body, &signature, "whsec_test", Mode::Test, 1301).is_err());
        assert!(verify(body, &signature, "whsec_test", Mode::Live, 1001).is_err());
        assert!(verify(b"{}", &signature, "whsec_test", Mode::Test, 1001).is_err());
        assert!(
            verify(
                body,
                &format!("t=1000,{signature}"),
                "whsec_test",
                Mode::Test,
                1001
            )
            .is_err()
        );
    }
}
