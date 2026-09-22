mod observer;
pub use observer::run;

use crate::{
    App,
    billing::{self, Meter, Pricebook, Rate},
    error::{Error, Result},
    store::Store,
};
use axum::{
    body::Bytes,
    http::{HeaderMap, Method},
};
use brain_protocol::{Event, EventPage, Usage};
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
    let row = sqlx::query("SELECT coalesce(sum(input_tokens),0)::bigint AS input,coalesce(sum(output_tokens),0)::bigint AS output,count(*) FILTER (WHERE terminal AND (NOT complete OR input_tokens IS NULL OR output_tokens IS NULL)) AS unknown,coalesce(sum(rated),0)::bigint AS rated,coalesce(sum(charged),0)::bigint AS charged FROM model_usage WHERE account=$1 AND ($2::text IS NULL OR session=$2)")
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
    let stale: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sessions WHERE account=$1 AND NOT model_complete AND state='owned' AND model_observed_at<$2)")
        .bind(account).bind(now_ms()-5_000).fetch_one(&mut **tx).await?;
    if stale || pending_in(tx, account).await? >= headroom_in(tx, account).await? {
        return Err(Error::credits(
            "token spending limit reached or usage observation unavailable",
        ));
    }
    Ok(true)
}

fn quantity(value: Option<u64>) -> Result<Option<i64>> {
    value
        .map(|value| {
            i64::try_from(value)
                .map_err(|_| Error::invalid("token quantity exceeds supported amount"))
        })
        .transpose()
}

/// Only authenticated Brain journal records reach this path. Tool-origin emissions cannot meter usage.
pub async fn record(app: &App, session: &str, event: &Event) -> Result<()> {
    let account: String = sqlx::query_scalar("SELECT account FROM sessions WHERE id=$1")
        .bind(session)
        .fetch_one(&app.store.0)
        .await?;
    let mut tx = app.store.0.begin().await?;
    Store::lock_account(&mut tx, &account).await?;
    let cursor: i64 = sqlx::query_scalar("SELECT model_cursor FROM sessions WHERE id=$1")
        .bind(session)
        .fetch_one(&mut *tx)
        .await?;
    let sequence = quantity(Some(event.sequence))?.unwrap();
    if sequence <= cursor {
        return Ok(());
    }
    if event.origin.is_none() {
        match event.event_type.as_str() {
            "model_call_started" => {
                let started = quantity(Some(event.recorded_at_ms))?.unwrap();
                sqlx::query("INSERT INTO model_usage(session,sequence,account,pricebook,started_ms) VALUES($1,$2,$3,(SELECT h.pricebook FROM model_price_history h JOIN pricebooks p ON p.id=h.pricebook WHERE h.account=$3 AND h.effective_ms<=$4 AND p.document::jsonb->'rates' ? 'model_tokens' AND h.id=(SELECT id FROM model_price_history WHERE account=$3 AND effective_ms<=$4 ORDER BY effective_ms DESC,id DESC LIMIT 1) LIMIT 1),$4)")
                    .bind(session).bind(sequence).bind(&account).bind(started).execute(&mut *tx).await?;
            }
            "model_call_ended" | "model_call_failed" => {
                let start = event.data["sequence"]
                    .as_u64()
                    .ok_or_else(Error::internal)?;
                let usage = event
                    .data
                    .get("result")
                    .or_else(|| event.data.get("response"))
                    .and_then(|result| result.get("usage"))
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                let usage: Usage = serde_json::from_value(usage)?;
                let complete = event.event_type == "model_call_ended"
                    || event.data["response"]["usage_complete"] == true;
                settle_in(
                    &mut tx,
                    session,
                    quantity(Some(start))?.unwrap(),
                    &usage,
                    complete,
                )
                .await?;
            }
            "turn_failed" | "session_ended" | "session_creation_failed" => {
                let pending = sqlx::query(
                    "SELECT sequence,usage FROM model_usage WHERE session=$1 AND NOT terminal",
                )
                .bind(session)
                .fetch_all(&mut *tx)
                .await?;
                for row in pending {
                    settle_in(
                        &mut tx,
                        session,
                        row.get("sequence"),
                        &serde_json::from_str::<Usage>(row.get("usage"))?,
                        false,
                    )
                    .await?;
                }
            }
            _ => {}
        }
    }
    let complete = event.origin.is_none()
        && matches!(
            event.event_type.as_str(),
            "session_ended" | "session_creation_failed"
        );
    sqlx::query("UPDATE sessions SET model_cursor=$1,model_observed_at=$2,model_complete=model_complete OR $3,model_pending=CASE WHEN $3 THEN 0 ELSE model_pending END WHERE id=$4")
        .bind(sequence).bind(now_ms()).bind(complete).bind(session).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

async fn settle_in(
    tx: &mut Transaction<'_, Postgres>,
    session: &str,
    sequence: i64,
    usage: &Usage,
    complete: bool,
) -> Result<()> {
    let row = sqlx::query(
        "SELECT account,pricebook,terminal,usage FROM model_usage WHERE session=$1 AND sequence=$2",
    )
    .bind(session)
    .bind(sequence)
    .fetch_one(&mut **tx)
    .await?;
    if row.get::<bool, _>("terminal") {
        return Ok(());
    }
    let mut observed: Usage = serde_json::from_str(row.get("usage"))?;
    observed.observe(usage).map_err(|_| Error::ambiguous())?;
    let usage = &observed;
    let input = quantity(usage.total_input_tokens)?;
    let output = quantity(usage.output_tokens)?;
    let tokens = input
        .unwrap_or(0)
        .checked_add(output.unwrap_or(0))
        .ok_or_else(|| Error::invalid("token total exceeds supported amount"))?;
    let account: String = row.get("account");
    let mut rated = 0;
    let mut charged = 0;
    if let Some(pricebook) = row.get::<Option<String>, _>("pricebook") {
        let document: String = sqlx::query_scalar("SELECT document FROM pricebooks WHERE id=$1")
            .bind(&pricebook)
            .fetch_one(&mut **tx)
            .await?;
        let book: Pricebook = serde_json::from_str(&document)?;
        sqlx::query("INSERT INTO model_usage_totals(account,pricebook) VALUES($1,$2) ON CONFLICT DO NOTHING").bind(&account).bind(&pricebook).execute(&mut **tx).await?;
        let total = sqlx::query(
            "SELECT tokens,rated FROM model_usage_totals WHERE account=$1 AND pricebook=$2",
        )
        .bind(&account)
        .bind(&pricebook)
        .fetch_one(&mut **tx)
        .await?;
        let cumulative = total
            .get::<i64, _>("tokens")
            .checked_add(tokens)
            .ok_or_else(|| Error::invalid("token total exceeds supported amount"))?;
        let cost = book.rates[&Meter::ModelTokens].rate(cumulative)?;
        rated = cost - total.get::<i64, _>("rated");
        charged = rated.min(headroom_in(tx, &account).await?);
        billing::entry(
            tx,
            &account,
            "usage",
            &format!("model:{session}:{sequence}"),
            -charged,
            &format!(
                "{tokens} reported model tokens; {} micro-USD absorbed",
                rated - charged
            ),
        )
        .await?;
        sqlx::query(
            "UPDATE model_usage_totals SET tokens=$1,rated=$2 WHERE account=$3 AND pricebook=$4",
        )
        .bind(cumulative)
        .bind(cost)
        .bind(&account)
        .bind(&pricebook)
        .execute(&mut **tx)
        .await?;
    }
    sqlx::query("UPDATE model_usage SET terminal=TRUE,usage=$1,input_tokens=$2,output_tokens=$3,rated=$4,charged=$5,complete=$8 WHERE session=$6 AND sequence=$7")
        .bind(serde_json::to_string(usage)?).bind(input).bind(output).bind(rated).bind(charged).bind(session).bind(sequence).bind(complete).execute(&mut **tx).await?;
    Ok(())
}

pub async fn reconcile(app: &App, session: &str) -> Result<()> {
    let complete: bool = sqlx::query_scalar("SELECT model_complete FROM sessions WHERE id=$1")
        .bind(session)
        .fetch_one(&app.store.0)
        .await?;
    if complete {
        return Ok(());
    }
    loop {
        let after: i64 = sqlx::query_scalar("SELECT model_cursor FROM sessions WHERE id=$1")
            .bind(session)
            .fetch_one(&app.store.0)
            .await?;
        let response = app
            .brain
            .request(
                Method::GET,
                &format!("/v1/sessions/{session}/events?after={after}"),
                &HeaderMap::new(),
                Bytes::new(),
                None,
                None,
            )
            .await?;
        if !response.status().is_success() {
            return Err(Error::ambiguous());
        }
        let page: EventPage = serde_json::from_slice(&app.brain.bytes(response).await?)?;
        if page.events.is_empty() {
            return Ok(());
        }
        for event in page.events {
            record(app, session, &event).await?;
        }
    }
}

#[derive(Default)]
struct Estimate {
    input_bytes: u64,
    media_inputs: u64,
    output_bytes: u64,
    started_ms: i64,
    usage: Usage,
    previous_input: Option<(u64, u64)>,
}
impl Estimate {
    fn tokens(&self, now: i64) -> u64 {
        let input = self.usage.total_input_tokens.unwrap_or_else(|| {
            let text = if self.input_bytes == 0 {
                1024
            } else {
                self.input_bytes.div_ceil(4)
            };
            let calibrated = self
                .previous_input
                .filter(|(_, bytes)| *bytes > 0)
                .map(|(tokens, bytes)| {
                    (u128::from(tokens) * u128::from(self.input_bytes) / u128::from(bytes))
                        .min(u128::from(u64::MAX)) as u64
                })
                .unwrap_or(0);
            text.max(calibrated)
                .saturating_add(self.media_inputs.saturating_mul(8192))
        });
        let elapsed = now.saturating_sub(self.started_ms).max(0) as u64;
        let output = self
            .usage
            .output_tokens
            .unwrap_or(0)
            .max(self.output_bytes.div_ceil(2))
            .max(elapsed.saturating_mul(256) / 1000);
        input
            .saturating_add(output)
            .saturating_add(input / 10)
            .saturating_add(256)
    }
    fn cost(&self, now: i64, rate: &Rate) -> Result<i64> {
        rate.rate(i64::try_from(self.tokens(now)).map_err(|_| Error::capacity())?)
            .map(|cost| cost.saturating_add(i64::from(rate.micro_usd > 0)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimates_replace_input_predictions_and_account_for_media_and_silent_output() {
        let mut estimate = Estimate {
            input_bytes: 12000,
            started_ms: 1000,
            ..Default::default()
        };
        let text = estimate.tokens(1000);
        estimate.media_inputs = 2;
        assert!(estimate.tokens(1000) > text);
        estimate.usage.total_input_tokens = Some(100);
        assert!(estimate.tokens(1000) < text);
        assert!(estimate.tokens(3000) > estimate.tokens(1000));
        estimate.usage.output_tokens = Some(2000);
        assert!(estimate.tokens(3000) >= 2100);
    }

    #[test]
    fn prior_receipts_calibrate_request_estimates_without_becoming_settled_usage() {
        let plain = Estimate {
            input_bytes: 3000,
            ..Default::default()
        };
        let calibrated = Estimate {
            input_bytes: 3000,
            previous_input: Some((9000, 3000)),
            ..Default::default()
        };
        assert!(calibrated.tokens(0) > plain.tokens(0));
        assert_eq!(calibrated.usage, Usage::default());
        assert_eq!(
            calibrated
                .cost(
                    0,
                    &Rate {
                        micro_usd: 0,
                        units: 1
                    }
                )
                .unwrap(),
            0
        );
    }
}
