use crate::{
    App,
    error::{Error, Result},
    store::{Store, now},
};
use axum::{
    Json,
    body::Bytes,
    http::{HeaderMap, Method},
    response::{IntoResponse, Response},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Row, Transaction};
use std::collections::BTreeMap;

#[derive(
    Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum Meter {
    TurnMs,
    SandboxMs,
    AttachmentByteSecs,
    EgressBytes,
}
impl Meter {
    pub fn name(self) -> &'static str {
        match self {
            Self::TurnMs => "turn_ms",
            Self::SandboxMs => "sandbox_ms",
            Self::AttachmentByteSecs => "attachment_byte_secs",
            Self::EgressBytes => "egress_bytes",
        }
    }
}

/// A rational price in micro-USD. Rating rounds once over cumulative resource usage.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Rate {
    pub micro_usd: i64,
    pub units: i64,
}
impl Rate {
    pub fn rate(&self, units: i64) -> Result<i64> {
        if units < 0 || self.units <= 0 || self.micro_usd < 0 {
            return Err(Error::invalid("invalid price or quantity"));
        }
        i64::try_from(i128::from(units) * i128::from(self.micro_usd) / i128::from(self.units))
            .map_err(|_| Error::invalid("rated cost exceeds supported amount"))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Pricebook {
    pub id: String,
    pub rates: BTreeMap<Meter, Rate>,
}

#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub pricebook: Pricebook,
    /// Must bound the paired Brain server's maximum turn duration.
    pub max_turn_secs: u32,
    pub default_spend_limit_micro_usd: i64,
    pub payments: Option<crate::payments::Config>,
}
impl Config {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.pricebook.id.is_empty()
                && self.pricebook.id.len() <= 80
                && self
                    .pricebook
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
            "invalid pricebook id"
        );
        anyhow::ensure!(
            self.max_turn_secs > 0 && self.default_spend_limit_micro_usd >= 0,
            "invalid billing limits"
        );
        for meter in [
            Meter::TurnMs,
            Meter::SandboxMs,
            Meter::AttachmentByteSecs,
            Meter::EgressBytes,
        ] {
            let rate = self
                .pricebook
                .rates
                .get(&meter)
                .ok_or_else(|| anyhow::anyhow!("missing {} price", meter.name()))?;
            anyhow::ensure!(rate.units > 0 && rate.micro_usd >= 0, "invalid rate");
        }
        if let Some(config) = &self.payments {
            config.validate()?;
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BillingMode {
    Preview,
    Prepaid,
}
#[derive(Serialize, JsonSchema)]
pub struct Wallet {
    pub mode: BillingMode,
    pub currency: String,
    pub balance_micro_usd: i64,
    pub reserved_micro_usd: i64,
    pub available_micro_usd: i64,
    pub spent_this_month_micro_usd: i64,
    pub spend_limit_micro_usd: Option<i64>,
    pub suspended: bool,
    pub accepted_pricebook: Option<String>,
    pub offered_pricebook: Option<Pricebook>,
    pub topup_amounts_cents: Vec<i64>,
    pub payment_mode: Option<crate::payments::Mode>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BillingSettings {
    pub pricebook: String,
    pub spend_limit_micro_usd: i64,
}
#[derive(Serialize, JsonSchema)]
pub struct LedgerEntry {
    pub id: i64,
    pub kind: String,
    pub reference: String,
    pub delta_micro_usd: i64,
    pub description: String,
    pub created: i64,
}
#[derive(Serialize, JsonSchema)]
pub struct LedgerPage {
    pub entries: Vec<LedgerEntry>,
    pub next_before: Option<i64>,
}

pub async fn initialize(store: &Store, config: &Config) -> anyhow::Result<()> {
    let document = serde_json::to_string(&config.pricebook)?;
    sqlx::query("INSERT INTO pricebooks VALUES ($1,$2,$3) ON CONFLICT (id) DO NOTHING")
        .bind(&config.pricebook.id)
        .bind(&document)
        .bind(now())
        .execute(&store.0)
        .await?;
    let saved: String = sqlx::query_scalar("SELECT document FROM pricebooks WHERE id=$1")
        .bind(&config.pricebook.id)
        .fetch_one(&store.0)
        .await?;
    anyhow::ensure!(
        serde_json::from_str::<Pricebook>(&saved)? == config.pricebook,
        "a published pricebook is immutable; publish a new id"
    );
    Ok(())
}

// Accounts are the existing serialization boundary for admission, money and ownership.
pub(crate) async fn entry(
    tx: &mut Transaction<'_, Postgres>,
    account: &str,
    kind: &str,
    reference: &str,
    delta: i64,
    description: &str,
) -> Result<()> {
    let inserted = sqlx::query("INSERT INTO credit_ledger(account,kind,reference,delta,description,created) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(account,kind,reference) DO NOTHING")
        .bind(account).bind(kind).bind(reference).bind(delta).bind(description).bind(now()).execute(&mut **tx).await?;
    if inserted.rows_affected() == 0 {
        let saved = sqlx::query("SELECT delta,description FROM credit_ledger WHERE account=$1 AND kind=$2 AND reference=$3")
            .bind(account).bind(kind).bind(reference).fetch_one(&mut **tx).await?;
        if saved.get::<i64, _>("delta") != delta
            || saved.get::<String, _>("description") != description
        {
            return Err(Error::conflict(
                "ledger reference reused with different value",
            ));
        }
    } else {
        sqlx::query("UPDATE wallets SET balance=balance+$1 WHERE account=$2")
            .bind(delta)
            .bind(account)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

async fn spent(tx: &mut Transaction<'_, Postgres>, account: &str) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT -coalesce(sum(delta),0)::bigint FROM credit_ledger WHERE account=$1 AND kind='usage' AND created >= extract(epoch FROM (date_trunc('month',to_timestamp($2) AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'))::bigint")
        .bind(account).bind(now() as f64).fetch_one(&mut **tx).await?)
}

pub async fn wallet(app: &App, account: &str) -> Result<Wallet> {
    let config = app.config.billing.as_ref();
    let payments = config.and_then(|c| c.payments.as_ref());
    let mut result = Wallet {
        mode: BillingMode::Preview,
        currency: "usd".into(),
        balance_micro_usd: 0,
        reserved_micro_usd: 0,
        available_micro_usd: 0,
        spent_this_month_micro_usd: 0,
        spend_limit_micro_usd: None,
        suspended: false,
        accepted_pricebook: None,
        offered_pricebook: config.map(|c| c.pricebook.clone()),
        topup_amounts_cents: payments
            .map(|p| p.topup_amounts_cents.clone())
            .unwrap_or_default(),
        payment_mode: payments.map(|p| p.mode),
    };
    let mut tx = app.store.0.begin().await?;
    Store::lock_account(&mut tx, account).await?;
    if let Some(row) = sqlx::query("SELECT * FROM wallets WHERE account=$1")
        .bind(account)
        .fetch_optional(&mut *tx)
        .await?
    {
        result.mode = BillingMode::Prepaid;
        result.balance_micro_usd = row.get("balance");
        result.reserved_micro_usd = row.get("reserved");
        result.available_micro_usd = result.balance_micro_usd - result.reserved_micro_usd;
        result.spent_this_month_micro_usd = spent(&mut tx, account).await?;
        result.spend_limit_micro_usd = Some(row.get("spend_limit"));
        result.suspended = row.get("suspended");
        result.accepted_pricebook = Some(row.get("pricebook"));
    }
    tx.commit().await?;
    Ok(result)
}

pub async fn settings(app: &App, account: &str, input: BillingSettings) -> Result<Wallet> {
    let config = app
        .config
        .billing
        .as_ref()
        .ok_or_else(|| Error::invalid("prepaid billing is not configured"))?;
    if input.pricebook != config.pricebook.id || input.spend_limit_micro_usd < 0 {
        return Err(Error::invalid(
            "accept the offered pricebook and a nonnegative monthly spend limit",
        ));
    }
    let mut tx = app.store.0.begin().await?;
    Store::lock_account(&mut tx, account).await?;
    sqlx::query("INSERT INTO wallets(account,pricebook,accepted_at,spend_limit) VALUES($1,$2,$3,$4) ON CONFLICT(account) DO UPDATE SET pricebook=$2,accepted_at=$3,spend_limit=$4")
        .bind(account).bind(&input.pricebook).bind(now()).bind(input.spend_limit_micro_usd).execute(&mut *tx).await?;
    entry(
        &mut tx,
        account,
        "price_acceptance",
        &input.pricebook,
        0,
        "Accepted published pricebook",
    )
    .await?;
    tx.commit().await?;
    wallet(app, account).await
}

pub fn maximum(headers: &HeaderMap) -> Result<Option<i64>> {
    headers
        .get("x-aex-max-cost-micro-usd")
        .map(|v| {
            v.to_str()
                .ok()
                .and_then(|s| s.parse::<i64>().ok())
                .filter(|n| *n >= 0)
                .ok_or_else(|| {
                    Error::invalid("x-aex-max-cost-micro-usd must be a nonnegative integer")
                })
        })
        .transpose()
}

pub struct Reservation<'a> {
    pub id: &'a str,
    pub resource: &'a str,
    pub meter: Meter,
    pub max_units: i64,
    pub max_cost: Option<i64>,
}

/// The caller holds the account lock and commits this alongside its durable dispatch intent.
pub async fn reserve_in(
    tx: &mut Transaction<'_, Postgres>,
    account: &str,
    input: Reservation<'_>,
) -> Result<bool> {
    let Some(wallet) = sqlx::query("SELECT w.*,p.document FROM wallets w JOIN pricebooks p ON p.id=w.pricebook WHERE account=$1")
        .bind(account).fetch_optional(&mut **tx).await? else { return Ok(false); };
    if let Some(saved) =
        sqlx::query("SELECT account,resource,meter,max_units FROM credit_reservations WHERE id=$1")
            .bind(input.id)
            .fetch_optional(&mut **tx)
            .await?
    {
        if saved.get::<String, _>("account") != account
            || saved.get::<String, _>("resource") != input.resource
            || saved.get::<String, _>("meter") != input.meter.name()
            || saved.get::<i64, _>("max_units") != input.max_units
        {
            return Err(Error::conflict(
                "reservation id reused with different resource",
            ));
        }
        return Ok(true);
    }
    let book: Pricebook = serde_json::from_str(wallet.get("document"))?;
    let amount = book
        .rates
        .get(&input.meter)
        .ok_or_else(Error::internal)?
        .rate(input.max_units)?;
    let max_cost = input
        .max_cost
        .ok_or_else(|| Error::invalid("prepaid work requires x-aex-max-cost-micro-usd"))?;
    let reserved: i64 = wallet.get("reserved");
    let balance: i64 = wallet.get("balance");
    let monthly = spent(tx, account).await?;
    if wallet.get::<bool, _>("suspended")
        || amount > max_cost
        || i128::from(amount) + i128::from(reserved) > i128::from(balance)
        || i128::from(monthly) + i128::from(reserved) + i128::from(amount)
            > i128::from(wallet.get::<i64, _>("spend_limit"))
    {
        return Err(Error::credits(
            "insufficient available credits, spend limit, or request cost ceiling",
        ));
    }
    sqlx::query("INSERT INTO credit_reservations(id,account,resource,meter,pricebook,max_units,remaining,state,created) VALUES($1,$2,$3,$4,$5,$6,$7,'open',$8)")
        .bind(input.id).bind(account).bind(input.resource).bind(input.meter.name()).bind(&book.id).bind(input.max_units).bind(amount).bind(now()).execute(&mut **tx).await?;
    sqlx::query("UPDATE wallets SET reserved=reserved+$1 WHERE account=$2")
        .bind(amount)
        .bind(account)
        .execute(&mut **tx)
        .await?;
    Ok(true)
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UsageReport {
    pub id: String,
    pub reservation: String,
    pub units: i64,
    pub terminal: bool,
}

/// Trusted server/provider observations only. A report is never accepted from a workload key.
pub async fn meter(store: &Store, report: &UsageReport) -> Result<()> {
    let account: String = sqlx::query_scalar("SELECT account FROM credit_reservations WHERE id=$1")
        .bind(&report.reservation)
        .fetch_optional(&store.0)
        .await?
        .ok_or_else(Error::missing)?;
    let mut tx = store.0.begin().await?;
    Store::lock_account(&mut tx, &account).await?;
    meter_in(&mut tx, report).await?;
    tx.commit().await?;
    Ok(())
}
pub(crate) async fn meter_in(
    tx: &mut Transaction<'_, Postgres>,
    report: &UsageReport,
) -> Result<()> {
    if report.id.is_empty() || report.id.len() > 200 || report.units < 0 {
        return Err(Error::invalid("invalid usage report"));
    }
    if let Some(saved) =
        sqlx::query("SELECT reservation,units,terminal FROM metered_usage WHERE id=$1")
            .bind(&report.id)
            .fetch_optional(&mut **tx)
            .await?
    {
        if saved.get::<String, _>("reservation") != report.reservation
            || saved.get::<i64, _>("units") != report.units
            || saved.get::<bool, _>("terminal") != report.terminal
        {
            return Err(Error::conflict("meter event id reused"));
        }
        return Ok(());
    }
    let row = sqlx::query("SELECT r.*,p.document FROM credit_reservations r JOIN pricebooks p ON p.id=r.pricebook WHERE r.id=$1").bind(&report.reservation).fetch_one(&mut **tx).await?;
    if row.get::<String, _>("state") != "open"
        || report.units < row.get::<i64, _>("units")
        || report.units > row.get::<i64, _>("max_units")
    {
        return Err(Error::conflict("usage is outside the open reservation"));
    }
    let book: Pricebook = serde_json::from_str(row.get("document"))?;
    let meter: Meter = serde_json::from_value(serde_json::Value::String(row.get("meter")))?;
    let amount = book
        .rates
        .get(&meter)
        .ok_or_else(Error::internal)?
        .rate(report.units)?;
    let delta = amount - row.get::<i64, _>("rated");
    let remaining: i64 = row.get("remaining");
    let account: String = row.get("account");
    entry(
        tx,
        &account,
        "usage",
        &report.id,
        -delta,
        &format!("{}: {} cumulative units", meter.name(), report.units),
    )
    .await?;
    sqlx::query("UPDATE wallets SET reserved=reserved-$1 WHERE account=$2")
        .bind(if report.terminal { remaining } else { delta })
        .bind(&account)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "UPDATE credit_reservations SET units=$1,rated=$2,remaining=$3,state=$4 WHERE id=$5",
    )
    .bind(report.units)
    .bind(amount)
    .bind(if report.terminal {
        0
    } else {
        remaining - delta
    })
    .bind(if report.terminal { "closed" } else { "open" })
    .bind(&report.reservation)
    .execute(&mut **tx)
    .await?;
    sqlx::query("INSERT INTO metered_usage VALUES($1,$2,$3,$4,$5,$6)")
        .bind(&report.id)
        .bind(&report.reservation)
        .bind(report.units)
        .bind(report.terminal)
        .bind(delta)
        .bind(now())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub async fn adjust(
    store: &Store,
    account: &str,
    reference: &str,
    delta: i64,
    reason: &str,
) -> Result<()> {
    if reference.is_empty()
        || reference.len() > 200
        || reason.trim().is_empty()
        || reason.len() > 1000
        || delta == 0
    {
        return Err(Error::invalid(
            "adjustments require a reference, nonzero delta and reason",
        ));
    }
    let mut tx = store.0.begin().await?;
    Store::lock_account(&mut tx, account).await?;
    entry(&mut tx, account, "adjustment", reference, delta, reason).await?;
    tx.commit().await?;
    Ok(())
}

pub fn route(method: &Method, path: &str) -> bool {
    if method == Method::POST && path == "/v1/billing/sync" {
        return true;
    }
    matches!(
        (method.as_str(), path),
        ("GET" | "PUT", "/v1/billing")
            | ("GET", "/v1/billing/ledger")
            | ("GET" | "POST", "/v1/billing/topups")
            | ("GET" | "POST", "/v1/billing/refunds")
    )
}
pub async fn handle(
    app: &App,
    method: &Method,
    path: &str,
    query: Option<&str>,
    headers: &HeaderMap,
    body: Bytes,
) -> Result<Response> {
    let token = crate::identity::bearer(headers)?;
    let account = if method == Method::GET && !token.starts_with("aex_account_") {
        app.store.principal(token).await?.account
    } else {
        crate::account::dashboard_account(app, token).await?
    };
    if path == "/v1/billing" {
        return Ok(Json(if method == Method::PUT {
            settings(app, &account, serde_json::from_slice(&body)?).await?
        } else {
            wallet(app, &account).await?
        })
        .into_response());
    }
    if path == "/v1/billing/ledger" {
        let before = match query {
            None => i64::MAX,
            Some(q) => q
                .strip_prefix("before=")
                .and_then(|s| s.parse::<i64>().ok())
                .filter(|n| *n > 0)
                .ok_or_else(|| Error::invalid("expected before ledger cursor"))?,
        };
        let rows = sqlx::query(
            "SELECT * FROM credit_ledger WHERE account=$1 AND id<$2 ORDER BY id DESC LIMIT 101",
        )
        .bind(&account)
        .bind(before)
        .fetch_all(&app.store.0)
        .await?;
        let entries: Vec<_> = rows
            .iter()
            .take(100)
            .map(|r| LedgerEntry {
                id: r.get("id"),
                kind: r.get("kind"),
                reference: r.get("reference"),
                delta_micro_usd: r.get("delta"),
                description: r.get("description"),
                created: r.get("created"),
            })
            .collect();
        let next_before = if rows.len() > 100 {
            entries.last().map(|r| r.id)
        } else {
            None
        };
        return Ok(Json(LedgerPage {
            entries,
            next_before,
        })
        .into_response());
    }
    crate::payments::handle(app, &account, method, path, headers, body).await
}
