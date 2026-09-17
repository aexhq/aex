use crate::{
    App,
    billing::{self, Meter, Reservation, UsageReport},
    error::{Error, Result},
    identity::{self, Principal},
    store::{Claim, Store, now},
};
use axum::{
    Json,
    body::Bytes,
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
};
use brain_protocol::{EventPage, MessageRequest, SessionStatus};
use serde::{Deserialize, Serialize};
use sqlx::Row;

#[derive(Serialize, Deserialize)]
struct Reply {
    status: u16,
    body: serde_json::Value,
}
impl Reply {
    fn response(self) -> Result<Response> {
        let status = StatusCode::from_u16(self.status).map_err(|_| Error::internal())?;
        let mut response = (status, Json(self.body)).into_response();
        response
            .headers_mut()
            .insert("cache-control", "no-store".parse().unwrap());
        if status == StatusCode::ACCEPTED {
            response
                .headers_mut()
                .insert("preference-applied", "respond-async".parse().unwrap());
        }
        Ok(response)
    }
}

pub async fn send(
    app: &App,
    principal: &Principal,
    session: &str,
    headers: &HeaderMap,
    body: Bytes,
) -> Result<Response> {
    let request: MessageRequest = serde_json::from_slice(&body)?;
    if request.input.message.is_empty() {
        return Err(Error::invalid("message must not be empty"));
    }
    let asynchronous = match headers.get("prefer") {
        None => false,
        Some(value) if value == "respond-async" => true,
        _ => return Err(Error::invalid("supported preference is respond-async")),
    };
    let maximum = billing::maximum(headers)?;
    crate::admission::disk(app).await?;
    let fingerprint = identity::digest(
        &serde_jcs::to_vec(&(request, asynchronous, maximum))
            .map_err(|_| Error::invalid("invalid message"))?,
    );
    let mut tx = app.store.0.begin().await?;
    let operation = match Store::claim_in(
        &mut tx,
        principal,
        &format!("message:{session}"),
        identity::operation_key(headers)?,
        &fingerprint,
        &app.config.limits,
    )
    .await?
    {
        Claim::Complete(saved) => return serde_json::from_value::<Reply>(saved)?.response(),
        Claim::New(operation) => operation,
    };
    // Capacity, usage ceiling and dispatch intent commit together before the upstream effect.
    let summary = app.brain.summary(session).await?;
    if !matches!(summary.status, SessionStatus::Idle) {
        return Err(Error::conflict("session is not idle"));
    }
    Store::reserve_turn_in(&mut tx, principal, session, &operation, &app.config.limits).await?;
    let prepaid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM wallets WHERE account=$1)")
        .bind(&principal.account)
        .fetch_one(&mut *tx)
        .await?;
    if prepaid {
        let config = app
            .config
            .billing
            .as_ref()
            .ok_or_else(|| Error::conflict("prepaid execution is not configured"))?;
        billing::reserve_in(
            &mut tx,
            &principal.account,
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
    sqlx::query("INSERT INTO session_turns VALUES($1,$2,$3)")
        .bind(&operation)
        .bind(session)
        .bind(i64::try_from(summary.last_sequence).map_err(|_| Error::internal())?)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let response = app
        .brain
        .request(
            Method::POST,
            &format!("/v1/sessions/{session}/messages"),
            headers,
            body,
            Some(&operation),
            None,
        )
        .await?;
    // Sanitize upstream errors through the same boundary as all other Aex routes.
    let response = crate::http::finite(app, response).await?;
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), app.config.limits.response_bytes)
        .await
        .map_err(|_| Error::ambiguous())?;
    let reply = Reply {
        status: status.as_u16(),
        body: serde_json::from_slice(&bytes).map_err(|_| Error::ambiguous())?,
    };
    sqlx::query(
        "UPDATE claims SET state='complete',result=$1 WHERE upstream_key=$2 AND state='pending'",
    )
    .bind(serde_json::to_string(&reply)?)
    .bind(&operation)
    .execute(&app.store.0)
    .await?;
    // An accepted turn keeps its reservation. Maintenance reconciles after disconnects/restarts.
    if let Err(error) = reconcile(app, session).await {
        tracing::warn!(session,code=%error.1.code,"turn reservation awaiting reconciliation");
    }
    reply.response()
}

pub async fn reconcile(app: &App, session: &str) -> Result<bool> {
    let Some(turn) = sqlx::query("SELECT s.account,s.active_key,t.since_sequence,r.max_units,c.result FROM sessions s LEFT JOIN session_turns t ON t.operation=s.active_key LEFT JOIN credit_reservations r ON r.id=s.active_key LEFT JOIN claims c ON c.upstream_key=s.active_key WHERE s.id=$1 AND s.active_key IS NOT NULL")
        .bind(session).fetch_optional(&app.store.0).await? else { return Ok(true); };
    let summary = app.brain.summary(session).await?;
    if matches!(
        summary.status,
        SessionStatus::Running | SessionStatus::Creating | SessionStatus::Ending
    ) {
        return Ok(false);
    }
    let operation: String = turn.get("active_key");
    let account: String = turn.get("account");
    let max_units: Option<i64> = turn.get("max_units");
    let mut units = 0;
    if let Some(since) = turn.get::<Option<i64>, _>("since_sequence") {
        let mut after = since as u64;
        let mut started = None;
        let mut ended = None;
        while after < summary.last_sequence {
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
            let previous = after;
            for event in page
                .events
                .into_iter()
                .take_while(|e| e.sequence <= summary.last_sequence)
            {
                after = event.sequence;
                if event.event_type == "turn_started" && event.origin.is_none() && started.is_none()
                {
                    started = Some(event.recorded_at_ms);
                }
                if matches!(event.event_type.as_str(), "turn_ended" | "turn_failed")
                    && event.origin.is_none()
                    && started.is_some()
                {
                    ended = Some(event.recorded_at_ms);
                    break;
                }
            }
            if ended.is_some() {
                break;
            }
            if after == previous {
                return Err(Error::ambiguous());
            }
        }
        units = match (started, ended) {
            (None, None) => {
                let reply = turn
                    .get::<Option<String>, _>("result")
                    .map(|s| serde_json::from_str::<Reply>(&s))
                    .transpose()?;
                if !reply.is_some_and(|r| (400..500).contains(&r.status)) {
                    return Err(Error::ambiguous());
                }
                0
            }
            (Some(start), Some(end)) => {
                i64::try_from(end.checked_sub(start).ok_or_else(Error::ambiguous)?)
                    .map_err(|_| Error::ambiguous())?
            }
            _ => return Err(Error::ambiguous()),
        };
    }
    let mut tx = app.store.0.begin().await?;
    Store::lock_account(&mut tx, &account).await?;
    let current: Option<String> = sqlx::query_scalar("SELECT active_key FROM sessions WHERE id=$1")
        .bind(session)
        .fetch_one(&mut *tx)
        .await?;
    if current.as_deref() != Some(&operation) {
        return Ok(true);
    }
    if let Some(maximum) = max_units {
        // Cancellation/recovery can finish after the approved billing window. The customer pays at most the accepted ceiling.
        billing::meter_in(
            &mut tx,
            &UsageReport {
                id: format!("turn-terminal:{operation}"),
                reservation: operation,
                units: units.min(maximum),
                terminal: true,
            },
        )
        .await?;
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
    let ids: Vec<String> =
        sqlx::query_scalar("SELECT id FROM sessions WHERE active_key IS NOT NULL")
            .fetch_all(&app.store.0)
            .await?;
    let mut finished = 0;
    for id in ids {
        match reconcile(app, &id).await {
            Ok(true) => finished += 1,
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(session=%id,code=%error.1.code,"turn reconciliation deferred")
            }
        }
    }
    Ok(finished)
}
