use crate::{
    App, admission,
    error::{Error, Result},
    identity::{Principal, digest},
    store::Claim,
};
use axum::{
    Json,
    body::Bytes,
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
};
use brain_protocol::{CreateSessionRequest, SessionList, SessionSummary};

pub async fn create(
    app: &App,
    p: &Principal,
    headers: &HeaderMap,
    body: Bytes,
    key: &str,
) -> Result<Response> {
    let request: CreateSessionRequest = serde_json::from_slice(&body)?;
    admission::create(app, p, &request).await?;
    let fingerprint =
        digest(&serde_jcs::to_vec(&request).map_err(|_| Error::invalid("invalid create request"))?);
    let claim = app
        .store
        .claim(p, "create", key, &fingerprint, &app.config.limits)
        .await?;
    let upstream = match claim {
        Claim::Complete(value) => {
            let summary: SessionSummary = serde_json::from_value(value)?;
            app.store
                .owned(p, summary.session_id.as_str(), false)
                .await?;
            return Ok(Json(summary).into_response());
        }
        Claim::New(upstream) => upstream,
    };
    let response = app
        .brain
        .request(
            Method::POST,
            "/v1/sessions",
            headers,
            body.clone(),
            Some(&upstream),
            None,
        )
        .await?;
    if !response.status().is_success() {
        return super::http::finite(app, response).await;
    }
    let summary: SessionSummary = serde_json::from_slice(&app.brain.bytes(response).await?)
        .map_err(|_| Error::ambiguous())?;
    app.store
        .complete_create(p, &upstream, &summary, body.len() as u64)
        .await?;
    Ok(Json(summary).into_response())
}
pub async fn list(app: &App, p: &Principal) -> Result<Response> {
    let mut sessions = Vec::new();
    for id in app.store.session_ids(&p.account).await? {
        sessions.push(app.brain.summary(&id).await?);
    }
    Ok(Json(SessionList { sessions }).into_response())
}
pub async fn delete(app: &App, id: &str, headers: &HeaderMap, key: &str) -> Result<Response> {
    if !app.store.mark_deleting(id).await? {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    app.changed.send_modify(|v| *v = v.wrapping_add(1));
    let response = app
        .brain
        .request(
            Method::DELETE,
            &format!("/v1/sessions/{id}"),
            headers,
            Bytes::new(),
            Some(key),
            None,
        )
        .await?;
    if response.status().is_success() {
        app.brain.bytes(response).await?;
        app.store.finish_delete(id).await?;
        Ok(StatusCode::NO_CONTENT.into_response())
    } else {
        super::http::finite(app, response).await
    }
}
