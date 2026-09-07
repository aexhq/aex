use crate::{
    App,
    error::{Error, Result},
    identity::Principal,
};
use axum::{
    Json,
    body::Bytes,
    http::{HeaderMap, Method},
    response::{IntoResponse, Response},
};
use brain_protocol::HostRegistration;

pub async fn register(app: &App, p: &Principal, headers: &HeaderMap) -> Result<Response> {
    // Serialize registration through ownership commit so simultaneous requests cannot exceed the host quota.
    let _guard = app
        .host_registration
        .try_lock()
        .map_err(|_| Error::capacity())?;
    let count = app.store.host_count(&p.account).await?;
    if count >= i64::from(app.config.limits.hosts_per_account) {
        return Err(Error::capacity());
    }
    let response = app
        .brain
        .request(Method::POST, "/v1/hosts", headers, Bytes::new(), None, None)
        .await?;
    if !response.status().is_success() {
        return super::http::finite(app, response).await;
    }
    let host: HostRegistration = serde_json::from_slice(&app.brain.bytes(response).await?)
        .map_err(|_| Error::ambiguous())?;
    app.store.register_host(p, &host).await?;
    Ok(Json(host).into_response())
}
