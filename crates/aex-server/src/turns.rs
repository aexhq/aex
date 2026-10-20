use crate::{
    App,
    billing::{self, Meter, Reservation, UsageReport},
    brain_contract::{Event, Input},
    error::{Error, Result},
    identity::{self, Principal},
    store::{Claim, Store, now},
};
use axum::{
    body::Bytes,
    http::{HeaderMap, Method},
    response::Response,
};
use serde_json::json;
use sqlx::Row;

pub async fn send(
    app: &App,
    p: &Principal,
    session: &str,
    headers: &HeaderMap,
    body: Bytes,
) -> Result<Response> {
    let public: serde_json::Value = serde_json::from_slice(&body)?;
    let mut public_input = public.clone();
    let media = public_input
        .as_object_mut()
        .ok_or_else(|| Error::invalid("message must be an object"))?
        .remove("media");
    let mut request: Input = serde_json::from_value(public_input)?;
    if request.r#type != "user.message"
        || (request
            .text
            .as_deref()
            .is_none_or(|text| text.trim().is_empty())
            && media
                .as_ref()
                .is_none_or(|media| media.as_array().is_none_or(Vec::is_empty)))
    {
        return Err(Error::invalid("a user message needs text or media"));
    }
    let key = request
        .key
        .clone()
        .ok_or_else(|| Error::invalid("messages require an idempotency key"))?;
    if key.is_empty() || key.len() > 128 {
        return Err(Error::invalid("message key must contain 1 to 128 bytes"));
    }
    let maximum = billing::maximum(headers)?;
    let fingerprint = identity::digest(
        &serde_jcs::to_vec(&json!({"input":public,"maximum":maximum}))
            .map_err(|_| Error::invalid("invalid input"))?,
    );
    if let Some(saved)=sqlx::query("SELECT fingerprint,result FROM claims WHERE account=$1 AND operation=$2 AND client_key=$3 AND state='complete'").bind(&p.account).bind(format!("message:{session}")).bind(&key).fetch_optional(&app.store.0).await? {
        if saved.get::<String,_>("fingerprint")!=fingerprint {return Err(Error::conflict("operation key reused with different request"));}
        return crate::http::saved(serde_json::from_str(saved.get("result"))?);
    }
    crate::admission::disk(app).await?;
    reconcile(app, session).await?;
    let mut tx = app.store.0.begin().await?;
    let operation = match Store::claim_in(
        &mut tx,
        p,
        &format!("message:{session}"),
        &key,
        &fingerprint,
        &app.config.limits,
    )
    .await?
    {
        Claim::Complete(reply) => return crate::http::saved(reply),
        Claim::New(operation) => operation,
    };
    if app.brain.summary(&p.account, session).await?.status != "idle" {
        return Err(Error::conflict("session must be idle before sending"));
    }
    Store::reserve_turn_in(&mut tx, p, session, &operation, &app.config.limits).await?;
    let prepaid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM wallets WHERE account=$1)")
        .bind(&p.account)
        .fetch_one(&mut *tx)
        .await?;
    if prepaid && !crate::model_usage::admit_in(&mut tx, &p.account).await? {
        let config = app
            .config
            .billing
            .as_ref()
            .ok_or_else(|| Error::conflict("prepaid execution is not configured"))?;
        billing::reserve_in(
            &mut tx,
            &p.account,
            Reservation {
                id: &operation,
                resource: session,
                meter: Meter::TurnMs,
                max_units: i64::from(config.max_turn_secs) * 1000,
                max_cost: maximum,
            },
        )
        .await?;
    }
    let cursor: String = sqlx::query_scalar("SELECT brain_cursor FROM sessions WHERE id=$1")
        .bind(session)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO brain_turns(operation,session,since_cursor,started_ms) VALUES($1,$2,$3,$4)",
    )
    .bind(&operation)
    .bind(session)
    .bind(cursor)
    .bind(crate::model_usage::now_ms())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    request.key = Some(operation.clone());
    request.media = match crate::attachments::resolve_media(app, p, session, media).await {
        Ok(media) => media,
        Err(error) => {
            reject(app,session,&operation,&p.account,json!({"status":error.0.as_u16(),"body":{"error":{"code":error.1.code,"message":error.1.message}}})).await?;
            return Err(error);
        }
    };
    let response = app
        .brain
        .request(
            Method::POST,
            &format!("/sessions/{session}/events"),
            headers,
            Bytes::from(serde_json::to_vec(&request)?),
            &p.account,
            None,
        )
        .await?;
    let status = response.status();
    let response = crate::http::finite(app, response).await?;
    let bytes = axum::body::to_bytes(response.into_body(), app.config.limits.response_bytes)
        .await
        .map_err(|_| Error::ambiguous())?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| Error::ambiguous())?;
    let reply = json!({"status":status.as_u16(),"body":value});
    if status.is_client_error() {
        reject(app, session, &operation, &p.account, reply.clone()).await?;
    } else if status.is_success() {
        let accepted: Event = serde_json::from_value(value)?;
        let mut tx = app.store.0.begin().await?;
        Store::lock_account(&mut tx, &p.account).await?;
        sqlx::query("UPDATE brain_turns SET input_id=$1,accepted=$2 WHERE operation=$3")
            .bind(&accepted.id)
            .bind(serde_json::to_string(&accepted)?)
            .bind(&operation)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE claims SET state='complete',result=$1 WHERE upstream_key=$2")
            .bind(serde_json::to_string(&reply)?)
            .bind(&operation)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        app.changed
            .send_modify(|value| *value = value.wrapping_add(1));
    }
    crate::http::saved(reply)
}
async fn reject(
    app: &App,
    session: &str,
    operation: &str,
    account: &str,
    reply: serde_json::Value,
) -> Result<()> {
    let mut tx = app.store.0.begin().await?;
    Store::lock_account(&mut tx, account).await?;
    let reserved: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM credit_reservations WHERE id=$1 AND state='open')",
    )
    .bind(operation)
    .fetch_one(&mut *tx)
    .await?;
    if reserved {
        billing::meter_in(
            &mut tx,
            &UsageReport {
                id: format!("turn-rejected:{operation}"),
                reservation: operation.into(),
                units: 0,
                terminal: true,
            },
        )
        .await?;
    }
    sqlx::query("UPDATE sessions SET active_key=NULL,active_since=NULL,retained_bytes=greatest(0,retained_bytes-$1) WHERE id=$2 AND active_key=$3").bind(app.config.limits.turn_reserve_bytes as i64).bind(session).bind(operation).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM brain_turns WHERE operation=$1")
        .bind(operation)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE claims SET state='rejected',result=$1 WHERE upstream_key=$2")
        .bind(serde_json::to_string(&reply)?)
        .bind(operation)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}
pub async fn reconcile(app: &App, session: &str) -> Result<bool> {
    let Some(turn)=sqlx::query("SELECT s.account,s.active_key,t.since_cursor,t.input_id,t.accepted,r.max_units FROM sessions s JOIN brain_turns t ON t.operation=s.active_key LEFT JOIN credit_reservations r ON r.id=s.active_key WHERE s.id=$1 AND s.runtime='brain' AND s.active_key IS NOT NULL").bind(session).fetch_optional(&app.store.0).await? else {return Ok(true)};
    let account: String = turn.get("account");
    let operation: String = turn.get("active_key");
    let mut input = turn.get::<Option<String>, _>("input_id");
    let mut accepted = turn.get::<Option<String>, _>("accepted");
    let mut cursor: String = turn.get("since_cursor");
    let mut started = None;
    let mut ended = None;
    loop {
        let page = app.brain.history(&account, session, &cursor).await?;
        if page.events.is_empty() {
            break;
        }
        for event in page.events {
            cursor = event.cursor.clone().ok_or_else(Error::internal)?;
            if event.r#type == "user.message" && event.data["key"] == operation {
                input = Some(event.id.clone());
                accepted = Some(serde_json::to_string(&event)?);
            }
            if event.input_id.as_deref() != input.as_deref() || input.is_none() {
                continue;
            }
            if event.r#type == "session.status"
                && event.data["status"] == "running"
                && started.is_none()
            {
                started = Some(crate::model_usage::timestamp(&event.time)?);
            }
            if event.r#type == "session.status"
                && event.data["status"].as_str().is_some_and(|status| {
                    ["idle", "interrupted", "failed", "closed"].contains(&status)
                })
            {
                ended = Some(crate::model_usage::timestamp(&event.time)?);
            }
        }
        if ended.is_some() {
            break;
        }
    }
    let status = app.brain.summary(&account, session).await?.status;
    if input.is_none() {
        return Err(Error::ambiguous());
    }
    if ended.is_none() && status == "running" {
        return Ok(false);
    }
    if ended.is_none() && status == "idle" {
        return Err(Error::ambiguous());
    }
    let known = started.zip(ended);
    let units = known.map(|(started, ended)| ended.saturating_sub(started).max(0));
    let mut tx = app.store.0.begin().await?;
    Store::lock_account(&mut tx, &account).await?;
    let current: Option<String> = sqlx::query_scalar("SELECT active_key FROM sessions WHERE id=$1")
        .bind(session)
        .fetch_one(&mut *tx)
        .await?;
    if current.as_deref() != Some(&operation) {
        return Ok(true);
    }
    if let Some(maximum) = turn.get::<Option<i64>, _>("max_units") {
        // Unknown duration stays unmeasured; release the reservation without inventing a charge.
        billing::meter_in(
            &mut tx,
            &UsageReport {
                id: format!("turn-terminal:{operation}"),
                reservation: operation.clone(),
                units: units.unwrap_or(0).min(maximum),
                terminal: true,
            },
        )
        .await?;
    }
    sqlx::query("UPDATE brain_turns SET input_id=$1,accepted=$2,outcome=$3 WHERE operation=$4")
        .bind(input)
        .bind(&accepted)
        .bind(if known.is_some() {
            "settled"
        } else {
            "unknown"
        })
        .bind(&operation)
        .execute(&mut *tx)
        .await?;
    if let Some(accepted) = accepted {
        let event: Event = serde_json::from_str(&accepted)?;
        sqlx::query("UPDATE claims SET state='complete',result=$1 WHERE upstream_key=$2 AND state='pending'").bind(serde_json::to_string(&json!({"status":202,"body":event}))?).bind(&operation).execute(&mut *tx).await?;
    }
    sqlx::query("UPDATE sessions SET active_key=NULL,active_since=NULL,changed_at=$1 WHERE id=$2")
        .bind(now())
        .bind(session)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}
pub async fn maintain(app: &App) -> Result<usize> {
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM sessions WHERE runtime='brain' AND active_key IS NOT NULL",
    )
    .fetch_all(&app.store.0)
    .await?;
    let mut finished = 0;
    for id in ids {
        match reconcile(app, &id).await {
            Ok(true) => finished += 1,
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(session=%id,code=%error.1.code,"turn reconciliation pending")
            }
        }
    }
    Ok(finished)
}
