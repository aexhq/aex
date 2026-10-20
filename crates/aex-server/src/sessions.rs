use crate::{
    App, admission,
    brain_contract::{CreateSession, Session, SessionList},
    error::{Error, Result},
    identity::{Principal, digest},
    store::{Claim, Store},
};
use axum::{
    Json,
    body::Bytes,
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
};
use sqlx::Row;

pub async fn create(
    app: &App,
    p: &Principal,
    headers: &HeaderMap,
    body: Bytes,
    key: &str,
) -> Result<Response> {
    let mut request: CreateSession = serde_json::from_slice(&body)?;
    admission::create(app, p, &request).await?;
    let ceiling = crate::billing::maximum(headers)?;
    let fingerprint = digest(
        &serde_jcs::to_vec(&serde_json::json!({"request":request,"maximum":ceiling}))
            .map_err(|_| Error::invalid("invalid create request"))?,
    );
    let mut tx = app.store.0.begin().await?;
    let operation =
        match Store::claim_in(&mut tx, p, "create", key, &fingerprint, &app.config.limits).await? {
            Claim::Complete(reply) => {
                tx.commit().await?;
                if reply["status"]
                    .as_u64()
                    .is_some_and(|status| (200..300).contains(&status))
                {
                    let session: Session = serde_json::from_value(reply["body"].clone())?;
                    app.store.owned(p, &session.id, false).await?;
                }
                return crate::http::saved(reply);
            }
            Claim::New(operation) => operation,
        };
    crate::managed::reserve_in(app, &mut tx, p, &operation, &mut request, ceiling).await?;
    crate::model_usage::admit_in(&mut tx, &p.account).await?;
    tx.commit().await?;
    let response = app
        .brain
        .request(
            Method::POST,
            "/sessions",
            headers,
            Bytes::from(serde_json::to_vec(&request)?),
            &p.account,
            None,
        )
        .await?;
    let status = response.status();
    if !status.is_success() {
        let response = crate::http::finite(app, response).await?;
        if status.is_client_error() {
            let bytes =
                axum::body::to_bytes(response.into_body(), app.config.limits.response_bytes)
                    .await
                    .map_err(|_| Error::ambiguous())?;
            let reply = serde_json::json!({"status":status.as_u16(),"body":serde_json::from_slice::<serde_json::Value>(&bytes)?});
            let mut tx = app.store.0.begin().await?;
            Store::lock_account(&mut tx, &p.account).await?;
            for grant in
                sqlx::query("SELECT id FROM tool_grants WHERE operation=$1 AND provider='sandbox'")
                    .bind(&operation)
                    .fetch_all(&mut *tx)
                    .await?
            {
                crate::billing::meter_in(
                    &mut tx,
                    &crate::billing::UsageReport {
                        id: format!("create-rejected:{}", grant.get::<String, _>("id")),
                        reservation: grant.get("id"),
                        units: 0,
                        terminal: true,
                    },
                )
                .await?;
            }
            sqlx::query("UPDATE tool_grants SET revoked=TRUE,settled=TRUE WHERE operation=$1")
                .bind(&operation)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE claims SET state='complete',result=$1 WHERE upstream_key=$2")
                .bind(serde_json::to_string(&reply)?)
                .bind(&operation)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            return crate::http::saved(reply);
        }
        return Ok(response);
    }
    let session: Session = serde_json::from_slice(&app.brain.bytes(response).await?)
        .map_err(|_| Error::ambiguous())?;
    app.store
        .complete_create(p, &operation, &session, body.len() as u64)
        .await?;
    app.changed
        .send_modify(|value| *value = value.wrapping_add(1));
    Ok((StatusCode::CREATED, Json(session)).into_response())
}
pub async fn list(app: &App, p: &Principal, query: Option<&str>) -> Result<Response> {
    let parameters: std::collections::BTreeMap<_, _> =
        url::form_urlencoded::parse(query.unwrap_or("").as_bytes())
            .into_owned()
            .collect();
    let after = parameters.get("after").map(String::as_str).unwrap_or("");
    let limit = parameters
        .get("limit")
        .map(|value| value.parse::<usize>())
        .transpose()
        .map_err(|_| Error::invalid("invalid list limit"))?
        .unwrap_or(100);
    if !(1..=1000).contains(&limit) {
        return Err(Error::invalid("invalid list limit"));
    }
    let mut ids = app.store.session_ids(&p.account).await?;
    ids.sort();
    let mut sessions = Vec::new();
    for id in ids.into_iter().filter(|id| id.as_str() > after).take(limit) {
        sessions.push(app.brain.summary(&p.account, &id).await?);
    }
    Ok(Json(SessionList { sessions }).into_response())
}
pub async fn delete(app: &App, id: &str, headers: &HeaderMap) -> Result<Response> {
    if !app.store.mark_deleting(id).await? {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    let account = app.store.session_account(id).await?;
    let response = app
        .brain
        .request(
            Method::POST,
            &format!("/sessions/{id}/close"),
            headers,
            Bytes::new(),
            &account,
            None,
        )
        .await?;
    if !response.status().is_success() && response.status() != StatusCode::NOT_FOUND {
        return crate::http::finite(app, response).await;
    }
    app.brain.bytes(response).await?;
    app.changed
        .send_modify(|value| *value = value.wrapping_add(1));
    if !crate::turns::reconcile(app, id).await? {
        return Err(Error::conflict("session execution is still settling"));
    }
    crate::model_usage::reconcile(app, id).await?;
    crate::managed::revoke(app, id).await?;
    if !crate::managed::settled(app, id).await? {
        return Err(Error::conflict("managed resource cleanup is pending"));
    }
    if !crate::attachments::delete_session(app, id).await? {
        return Err(Error::conflict("attachment cleanup is pending"));
    }
    let response = app
        .brain
        .request(
            Method::DELETE,
            &format!("/sessions/{id}"),
            headers,
            Bytes::new(),
            &account,
            None,
        )
        .await?;
    if !response.status().is_success() {
        return crate::http::finite(app, response).await;
    }
    app.brain.bytes(response).await?;
    app.store.finish_delete(id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
pub async fn retire(app: &App, id: &str) -> Result<Response> {
    delete(app, id, &HeaderMap::new()).await
}
