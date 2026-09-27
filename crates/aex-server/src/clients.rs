use crate::{
    App,
    error::{Error, Result},
    identity::{self, Principal},
    store,
};
use axum::{
    Json,
    body::{Bytes, to_bytes},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use brain_protocol::{CreateSessionRequest, Driver, HostRegistration};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClientGrantInput {
    pub origin: String,
    pub expires_at: i64,
    /// The prepared Brain request. Credentials remain on the server.
    pub session: Value,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ClientAccess {
    pub id: String,
    pub token: String,
    pub expires_at: i64,
    pub credentials: ClientHost,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClientHost {
    pub host_id: String,
    pub token: String,
}

pub struct Pending {
    pub request: CreateSessionRequest,
    pub until: i64,
    pub ceiling: Option<axum::http::HeaderValue>,
}
pub struct Access {
    pub principal: Principal,
    pub verifier: String,
    pub host: String,
    pub session: Option<String>,
    pub expires_at: i64,
    fingerprint: String,
    components: Vec<(String, String)>,
}

fn public_request(request: &CreateSessionRequest, token: &str) -> Result<Value> {
    let mut value = serde_json::to_value(request)?;
    value["model"]["api_key"] = json!(token);
    if let Some(envs) = value["environments"].as_array_mut() {
        for env in envs {
            if env.get("credential").is_some() {
                env["credential"] = json!(token);
            }
        }
    }
    Ok(value)
}
fn fingerprint(value: &Value) -> Result<String> {
    Ok(identity::digest(&serde_jcs::to_vec(value).map_err(
        |_| Error::invalid("invalid client composition"),
    )?))
}
pub async fn grant(
    app: &App,
    p: &Principal,
    headers: &HeaderMap,
    input: ClientGrantInput,
) -> Result<Response> {
    let origin =
        url::Url::parse(&input.origin).map_err(|_| Error::invalid("invalid client origin"))?;
    if origin.origin().ascii_serialization() != input.origin
        || !origin.username().is_empty()
        || origin.password().is_some()
        || (origin.scheme() != "https"
            && !(origin.scheme() == "http"
                && origin.host_str().is_some_and(|h| {
                    h == "localhost"
                        || h.parse::<std::net::IpAddr>()
                            .is_ok_and(|ip| ip.is_loopback())
                })))
        || input.expires_at <= store::now()
        || input.expires_at > store::now() + 86_400
    {
        return Err(Error::invalid(
            "client access needs an application origin and an expiry within 24 hours",
        ));
    }
    let mut request: CreateSessionRequest = serde_json::from_value(input.session)?;
    let mut pending = app.pending_clients.lock().await;
    pending.retain(|_, value| value.until > store::now());
    if pending.len() >= app.config.limits.requests {
        return Err(Error::capacity());
    }
    let response = crate::hosts::register(app, p, headers).await?;
    if !response.status().is_success() {
        return Ok(response);
    }
    let host: HostRegistration = serde_json::from_slice(
        &to_bytes(response.into_body(), 4096)
            .await
            .map_err(|_| Error::internal())?,
    )?;
    for env in &mut request.environments {
        if let Driver::Host { host_id } = &mut env.driver {
            *host_id = host.host_id.clone();
        }
    }
    crate::admission::create(app, p, &request).await?;
    let id = identity::random("clientgrant");
    let token = identity::random("client");
    let verifier = identity::digest(token.as_bytes());
    let public = public_request(&request, &token)?;
    let fingerprint = fingerprint(&public)?;
    let mut components = Vec::new();
    if let Some(id) = request
        .agentloop
        .implementation
        .get("id")
        .and_then(Value::as_str)
    {
        components.push(("agentloops", id.to_owned()));
    }
    for tool in &request.tools {
        for placement in tool.placements.values() {
            if placement.implementation.get("type").and_then(Value::as_str)
                == Some("brain_component")
                && let Some(id) = placement.implementation.get("id").and_then(Value::as_str)
            {
                components.push(("tools", id.to_owned()));
            }
        }
    }
    sqlx::query("INSERT INTO client_grants(id,verifier,account,issuing_key,host,origin,expires_at,fingerprint,components) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
        .bind(&id).bind(&verifier).bind(&p.account).bind(&p.key).bind(host.host_id.as_str()).bind(input.origin)
        .bind(input.expires_at).bind(fingerprint).bind(serde_json::to_string(&components)?).execute(&app.store.0).await?;
    let until = input.expires_at.min(store::now() + 300);
    pending.insert(
        verifier.clone(),
        Pending {
            request,
            until,
            ceiling: headers.get("x-aex-max-cost-micro-usd").cloned(),
        },
    );
    let pending_clients = app.pending_clients.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(
            until.saturating_sub(store::now()).max(0) as u64,
        ))
        .await;
        pending_clients.lock().await.remove(&verifier);
    });
    Ok((
        [("cache-control", "no-store")],
        Json(ClientAccess {
            id,
            token,
            expires_at: input.expires_at,
            credentials: ClientHost {
                host_id: host.host_id.to_string(),
                token: host.token,
            },
        }),
    )
        .into_response())
}

pub async fn access(
    app: &App,
    token: Option<&str>,
    host: Option<&str>,
    headers: &HeaderMap,
) -> Result<Option<Access>> {
    let row = if let Some(token) = token {
        sqlx::query("SELECT * FROM client_grants WHERE verifier=$1")
            .bind(identity::digest(token.as_bytes()))
            .fetch_optional(&app.store.0)
            .await?
    } else {
        sqlx::query("SELECT * FROM client_grants WHERE host=$1")
            .bind(host)
            .fetch_optional(&app.store.0)
            .await?
    };
    let Some(row) = row else {
        return if token.is_some() {
            Err(Error::denied())
        } else {
            Ok(None)
        };
    };
    if row.get::<i64, _>("expires_at") <= store::now()
        || headers.get("origin").and_then(|v| v.to_str().ok()) != Some(row.get::<&str, _>("origin"))
    {
        return Err(Error::denied());
    }
    let principal = Principal {
        account: row.get("account"),
        key: row.get("issuing_key"),
    };
    app.store.active(&principal).await?;
    Ok(Some(Access {
        principal,
        verifier: row.get("verifier"),
        host: row.get("host"),
        session: row.get("session"),
        expires_at: row.get("expires_at"),
        fingerprint: row.get("fingerprint"),
        components: serde_json::from_str(row.get("components"))?,
    }))
}
impl Access {
    pub fn authorize(&self, route: &crate::http::Route<'_>) -> Result<()> {
        use crate::http::Route;
        match route {
            Route::Create | Route::Artifact(_, _) => Ok(()),
            Route::Host(id, _) if *id == self.host => Ok(()),
            Route::Session(id, op)
                if self.session.as_deref() == Some(*id) && *op != "environments" =>
            {
                Ok(())
            }
            _ => Err(Error::denied()),
        }
    }
    pub fn artifact(&self, kind: &str, id: &str) -> Result<()> {
        if self.components.iter().any(|(k, i)| k == kind && i == id) {
            Ok(())
        } else {
            Err(Error::denied())
        }
    }
}

pub async fn create(
    app: &App,
    access: &Access,
    headers: &HeaderMap,
    body: Bytes,
) -> Result<Response> {
    let requested: CreateSessionRequest = serde_json::from_slice(&body)?;
    if fingerprint(&serde_json::to_value(&requested)?)? != access.fingerprint {
        return Err(Error::denied());
    }
    if let Some(id) = &access.session {
        app.store.owned(&access.principal, id, false).await?;
        return Ok((
            [("cache-control", "no-store")],
            Json(app.brain.summary(id).await?),
        )
            .into_response());
    }
    let mut headers = headers.clone();
    headers.remove("x-aex-max-cost-micro-usd");
    let body = {
        let pending = app.pending_clients.lock().await;
        let saved = pending.get(&access.verifier).filter(|value| value.until > store::now())
            .ok_or_else(|| Error::conflict("client session authorization expired or was interrupted; request fresh access from the application"))?;
        if let Some(ceiling) = &saved.ceiling {
            headers.insert("x-aex-max-cost-micro-usd", ceiling.clone());
        }
        Bytes::from(serde_json::to_vec(&saved.request)?)
    };
    let response =
        crate::sessions::create(app, &access.principal, &headers, body, &access.verifier).await?;
    if response.status().is_success() {
        app.pending_clients.lock().await.remove(&access.verifier);
    }
    Ok(response)
}

pub async fn revoke(app: &App, p: &Principal, id: &str) -> Result<Response> {
    let verifier: String = sqlx::query_scalar(
        "UPDATE client_grants SET expires_at=0 WHERE id=$1 AND account=$2 RETURNING verifier",
    )
    .bind(id)
    .bind(&p.account)
    .fetch_optional(&app.store.0)
    .await?
    .ok_or_else(Error::missing)?;
    app.pending_clients.lock().await.remove(&verifier);
    app.changed.send_modify(|v| *v = v.wrapping_add(1));
    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
}
pub async fn active(app: &App, verifier: &str) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT expires_at FROM client_grants WHERE verifier=$1")
        .bind(verifier)
        .fetch_optional(&app.store.0)
        .await
        .is_ok_and(|value| value.is_some_and(|at| at > store::now()))
}
