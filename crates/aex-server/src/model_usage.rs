mod observer;
pub use observer::run;

use crate::brain_contract::{Event, ModelRequest, ModelUsage};
use crate::{
    App,
    billing::{self, Meter, Pricebook},
    error::{Error, Result},
    store::Store,
};
use axum::{
    body::Bytes,
    http::{HeaderMap, Method},
};
use schemars::JsonSchema;
use serde::Serialize;
use sqlx::{Postgres, Row, Transaction};

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock before epoch")
        .as_millis() as i64
}

#[derive(Default, Serialize, JsonSchema)]
pub struct TokenUsage {
    pub reported_input_tokens: i64,
    pub reported_output_tokens: i64,
    pub unmeasured_calls: i64,
    pub rated_micro_usd: i64,
    pub charged_micro_usd: i64,
    pub pending_estimate_micro_usd: i64,
}

pub async fn totals(store: &Store, account: &str) -> Result<TokenUsage> {
    totals_for(store, account, None).await
}

pub async fn totals_for(store: &Store, account: &str, session: Option<&str>) -> Result<TokenUsage> {
    let row = sqlx::query("SELECT coalesce(sum(input_tokens),0)::bigint AS input,coalesce(sum(output_tokens),0)::bigint AS output,count(*) FILTER (WHERE terminal AND (NOT complete OR input_tokens IS NULL OR output_tokens IS NULL)) AS unknown,coalesce(sum(rated),0)::bigint AS rated,coalesce(sum(charged),0)::bigint AS charged FROM (SELECT account,session,input_tokens,output_tokens,terminal,complete,rated,charged FROM model_usage UNION ALL SELECT u.account,u.session,u.input_tokens,u.output_tokens,u.terminal,u.complete,u.rated,u.charged FROM brain_model_usage u WHERE u.attempt_id<>'' OR NOT EXISTS(SELECT 1 FROM brain_model_usage a WHERE a.session=u.session AND a.request_id=u.request_id AND a.attempt_id<>'')) evidence WHERE account=$1 AND ($2::text IS NULL OR session=$2)")
        .bind(account).bind(session).fetch_one(&store.0).await?;
    let pending = sqlx::query_scalar("SELECT coalesce(sum(model_pending),0)::bigint FROM sessions WHERE account=$1 AND NOT model_complete AND state!='deleted' AND ($2::text IS NULL OR id=$2)")
        .bind(account).bind(session).fetch_one(&store.0).await?;
    Ok(TokenUsage {
        reported_input_tokens: row.get("input"),
        reported_output_tokens: row.get("output"),
        unmeasured_calls: row.get("unknown"),
        rated_micro_usd: row.get("rated"),
        charged_micro_usd: row.get("charged"),
        pending_estimate_micro_usd: pending,
    })
}

pub(crate) async fn pending_in(tx: &mut Transaction<'_, Postgres>, account: &str) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT coalesce(sum(model_pending),0)::bigint FROM sessions WHERE account=$1 AND NOT model_complete AND state!='deleted'")
        .bind(account).fetch_one(&mut **tx).await?)
}

pub(crate) async fn headroom_in(tx: &mut Transaction<'_, Postgres>, account: &str) -> Result<i64> {
    let row =
        sqlx::query("SELECT balance,reserved,spend_limit,suspended FROM wallets WHERE account=$1")
            .bind(account)
            .fetch_one(&mut **tx)
            .await?;
    if row.get::<bool, _>("suspended") {
        return Ok(0);
    }
    let reserved: i64 = row.get("reserved");
    let available = row.get::<i64, _>("balance").saturating_sub(reserved);
    let monthly = row
        .get::<i64, _>("spend_limit")
        .saturating_sub(billing::spent(tx, account).await?)
        .saturating_sub(reserved);
    Ok(available.min(monthly).max(0))
}

/// Returns whether this account uses tokens; legacy duration offers keep their own admission.
pub(crate) async fn admit_in(tx: &mut Transaction<'_, Postgres>, account: &str) -> Result<bool> {
    let Some(document): Option<String> = sqlx::query_scalar(
        "SELECT p.document FROM wallets w JOIN pricebooks p ON p.id=w.pricebook WHERE account=$1",
    )
    .bind(account)
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Ok(false);
    };
    let book: Pricebook = serde_json::from_str(&document)?;
    if !book.rates.contains_key(&Meter::ModelTokens) {
        return Ok(false);
    }
    let stale: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sessions WHERE account=$1 AND NOT model_complete AND state='owned' AND runtime='brain' AND model_observed_at<$2)")
        .bind(account).bind(now_ms()-5_000).fetch_one(&mut **tx).await?;
    if stale || pending_in(tx, account).await? >= headroom_in(tx, account).await? {
        return Err(Error::credits(
            "token spending limit reached or usage observation unavailable",
        ));
    }
    Ok(true)
}

pub(crate) fn timestamp(value: &str) -> Result<i64> {
    let parsed = time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
        .map_err(|_| Error::internal())?;
    i64::try_from(parsed.unix_timestamp_nanos() / 1_000_000).map_err(|_| Error::internal())
}
fn inclusive(
    base: Option<i64>,
    first: Option<i64>,
    second: Option<i64>,
    semantics: &str,
) -> Result<(Option<i64>, bool)> {
    let Some(base) = base else {
        return Ok((None, false));
    };
    if semantics == "included" {
        return Ok((Some(base), true));
    }
    if semantics == "excluded" {
        let value = base
            .checked_add(first.unwrap_or(0))
            .and_then(|value| value.checked_add(second.unwrap_or(0)))
            .ok_or_else(Error::capacity)?;
        return Ok((Some(value), first.is_some() && second.is_some()));
    }
    Ok((Some(base), false))
}
fn normalize(usage: &ModelUsage) -> Result<(Option<i64>, Option<i64>, bool)> {
    let (input, input_complete) = inclusive(
        usage.input_tokens,
        usage.cache_read_tokens,
        usage.cache_write_tokens,
        &usage.input_cache,
    )?;
    let (output, output_complete) = inclusive(
        usage.output_tokens,
        usage.reasoning_tokens,
        Some(0),
        &usage.output_reasoning,
    )?;
    Ok((
        input,
        output,
        usage.availability == "reported" && input_complete && output_complete,
    ))
}

// The opaque cursor and receipt commit together. A competing reader restarts
// from the stored cursor instead of ordering cursors or replaying charges.
pub async fn record(app: &App, session: &str, after: &str, event: &Event) -> Result<bool> {
    let cursor = event.cursor.as_deref().ok_or_else(Error::internal)?;
    let account = app.store.session_account(session).await?;
    let mut tx = app.store.0.begin().await?;
    Store::lock_account(&mut tx, &account).await?;
    let cursor_before: String =
        sqlx::query_scalar("SELECT brain_cursor FROM sessions WHERE id=$1 FOR UPDATE")
            .bind(session)
            .fetch_one(&mut *tx)
            .await?;
    if cursor_before != after {
        return Ok(false);
    }
    if event.r#type == "model.request" {
        let request: ModelRequest = serde_json::from_value(event.data.clone())?;
        let attempt = request.attempt_id.as_deref().unwrap_or("");
        let started = timestamp(&event.time)?;
        sqlx::query("INSERT INTO brain_model_usage(session,request_id,attempt_id,account,pricebook,started_ms,evidence) VALUES($1,$2,$3,$4,(SELECT h.pricebook FROM model_price_history h JOIN pricebooks p ON p.id=h.pricebook WHERE h.account=$4 AND h.effective_ms<=$5 AND p.document::jsonb->'rates' ? 'model_tokens' ORDER BY h.effective_ms DESC,h.id DESC LIMIT 1),$5,$6) ON CONFLICT DO NOTHING")
            .bind(session).bind(&request.request_id).bind(attempt).bind(&account).bind(started).bind(serde_json::to_string(&request)?).execute(&mut *tx).await?;
        let terminal = request.state != "started";
        let row=sqlx::query("SELECT terminal,pricebook FROM brain_model_usage WHERE session=$1 AND request_id=$2 AND attempt_id=$3").bind(session).bind(&request.request_id).bind(attempt).fetch_one(&mut *tx).await?;
        if !row.get::<bool, _>("terminal") {
            let (input, output, measured) = normalize(&request.usage)?;
            let complete = measured
                && request
                    .unmeasured
                    .as_deref()
                    .is_none_or(<[String]>::is_empty);
            let mut rated = 0;
            let mut charged = 0;
            let physical:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM brain_model_usage WHERE session=$1 AND request_id=$2 AND attempt_id<>'')").bind(session).bind(&request.request_id).fetch_one(&mut *tx).await?;
            if terminal
                && (!attempt.is_empty() || !physical)
                && let Some(pricebook) = row.get::<Option<String>, _>("pricebook")
            {
                let tokens = input
                    .unwrap_or(0)
                    .checked_add(output.unwrap_or(0))
                    .ok_or_else(Error::capacity)?;
                let document: String =
                    sqlx::query_scalar("SELECT document FROM pricebooks WHERE id=$1")
                        .bind(&pricebook)
                        .fetch_one(&mut *tx)
                        .await?;
                let book: Pricebook = serde_json::from_str(&document)?;
                sqlx::query("INSERT INTO model_usage_totals(account,pricebook) VALUES($1,$2) ON CONFLICT DO NOTHING").bind(&account).bind(&pricebook).execute(&mut *tx).await?;
                let total = sqlx::query(
                    "SELECT tokens,rated FROM model_usage_totals WHERE account=$1 AND pricebook=$2",
                )
                .bind(&account)
                .bind(&pricebook)
                .fetch_one(&mut *tx)
                .await?;
                let cumulative = total
                    .get::<i64, _>("tokens")
                    .checked_add(tokens)
                    .ok_or_else(Error::capacity)?;
                let cost = book.rates[&Meter::ModelTokens].rate(cumulative)?;
                rated = cost - total.get::<i64, _>("rated");
                charged = rated.min(headroom_in(&mut tx, &account).await?);
                billing::entry(
                    &mut tx,
                    &account,
                    "usage",
                    &format!("brain:{session}:{}:{attempt}", request.request_id),
                    -charged,
                    &format!(
                        "{tokens} reported tokens; {} micro-USD absorbed; complete={complete}",
                        rated - charged
                    ),
                )
                .await?;
                sqlx::query("UPDATE model_usage_totals SET tokens=$1,rated=$2 WHERE account=$3 AND pricebook=$4").bind(cumulative).bind(cost).bind(&account).bind(&pricebook).execute(&mut *tx).await?;
            }
            sqlx::query("UPDATE brain_model_usage SET evidence=$1,terminal=$2,complete=$3,input_tokens=$4,output_tokens=$5,rated=$6,charged=$7 WHERE session=$8 AND request_id=$9 AND attempt_id=$10")
                .bind(serde_json::to_string(&request)?).bind(terminal).bind(complete).bind(input).bind(output).bind(rated).bind(charged).bind(session).bind(&request.request_id).bind(attempt).execute(&mut *tx).await?;
        }
    }
    let closed = event.r#type == "session.status" && event.data["status"] == "closed";
    sqlx::query("UPDATE sessions SET brain_cursor=$1,model_observed_at=$2,model_complete=model_complete OR $3 WHERE id=$4").bind(cursor).bind(now_ms()).bind(closed).bind(session).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(true)
}
pub async fn reconcile(app: &App, session: &str) -> Result<()> {
    let complete: bool = sqlx::query_scalar("SELECT model_complete FROM sessions WHERE id=$1")
        .bind(session)
        .fetch_one(&app.store.0)
        .await?;
    if complete {
        return Ok(());
    }
    let account = app.store.session_account(session).await?;
    loop {
        let after: String = sqlx::query_scalar("SELECT brain_cursor FROM sessions WHERE id=$1")
            .bind(session)
            .fetch_one(&app.store.0)
            .await?;
        let page = app.brain.history(&account, session, &after).await?;
        if page.events.is_empty() {
            break;
        }
        let mut cursor = after;
        for event in page.events {
            if !record(app, session, &cursor, &event).await? {
                break;
            }
            cursor = event.cursor.ok_or_else(Error::internal)?;
        }
    }
    estimate(app, session).await
}
async fn estimate(app: &App, session: &str) -> Result<()> {
    let mut tx = app.store.0.begin().await?;
    let account = app.store.session_account(session).await?;
    Store::lock_account(&mut tx, &account).await?;
    let calls=sqlx::query("SELECT u.started_ms,u.pricebook,u.evidence FROM brain_model_usage u WHERE u.session=$1 AND NOT u.terminal AND (u.attempt_id<>'' OR NOT EXISTS(SELECT 1 FROM brain_model_usage a WHERE a.session=u.session AND a.request_id=u.request_id AND a.attempt_id<>''))")
        .bind(session).fetch_all(&mut *tx).await?;
    let mut pending = 0i64;
    for row in calls {
        if let Some(pricebook) = row.get::<Option<String>, _>("pricebook") {
            let document: String =
                sqlx::query_scalar("SELECT document FROM pricebooks WHERE id=$1")
                    .bind(pricebook)
                    .fetch_one(&mut *tx)
                    .await?;
            let book: Pricebook = serde_json::from_str(&document)?;
            let request: ModelRequest = serde_json::from_str(row.get("evidence"))?;
            let (input, output, _) = normalize(&request.usage)?;
            let elapsed = now_ms().saturating_sub(row.get("started_ms")).max(0);
            let tokens = input
                .unwrap_or(1024)
                .checked_add(output.unwrap_or(0).max(elapsed.saturating_mul(256) / 1000))
                .and_then(|value| value.checked_add(256))
                .ok_or_else(Error::capacity)?;
            pending = pending
                .checked_add(book.rates[&Meter::ModelTokens].rate(tokens)?)
                .ok_or_else(Error::capacity)?;
        }
    }
    sqlx::query("UPDATE sessions SET model_pending=$1,model_observed_at=$2 WHERE id=$3")
        .bind(pending)
        .bind(now_ms())
        .bind(session)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}
