use crate::{
    App,
    brain_contract::{CreateSession, ExecutorRegistration, Tool, ToolBinding},
    error::{Error, Result},
    identity::{self, Principal},
    store,
};
use axum::{
    Json,
    body::{Bytes, to_bytes},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClientGrantInput {
    pub origin: String,
    pub expires_at: i64,
    pub session: GrantSession,
    #[serde(default)]
    pub tools: Vec<Tool>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum GrantSession {
    Existing(String),
    Create(CreateSession),
}
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ClientAccess {
    pub id: String,
    pub token: String,
    pub expires_at: i64,
    pub session: CreateSession,
    pub tools: Vec<Tool>,
}
#[derive(Serialize, JsonSchema)]
pub struct Composition {
    pub create: CreateSession,
    pub session: Option<String>,
    pub tools: Vec<Tool>,
}
#[derive(Clone)]
pub struct Pending {
    pub request: CreateSession,
    pub until: i64,
    pub ceiling: Option<axum::http::HeaderValue>,
}
pub struct Access {
    pub principal: Principal,
    pub id: String,
    pub verifier: String,
    pub session: Option<String>,
    pub expires_at: i64,
    composition: Value,
    tools: Vec<Tool>,
}
fn public_request(request: &CreateSession) -> Result<Value> {
    let mut value = serde_json::to_value(request)?;
    let object = value.as_object_mut().ok_or_else(Error::internal)?;
    object.remove("model_credential");
    object.remove("credentials");
    Ok(value)
}
fn descriptors_equal(a: &[Tool], b: &[Tool]) -> Result<bool> {
    Ok(serde_json::to_value(a)? == serde_json::to_value(b)?)
}
pub async fn grant(
    app: &App,
    p: &Principal,
    headers: &HeaderMap,
    input: ClientGrantInput,
) -> Result<Response> {
    let origin =
        url::Url::parse(&input.origin).map_err(|_| Error::invalid("invalid browser origin"))?;
    if origin.origin().ascii_serialization() != input.origin
        || !origin.username().is_empty()
        || origin.password().is_some()
        || !(origin.scheme() == "https"
            || origin.scheme() == "http"
                && origin.host_str().is_some_and(|host| {
                    host == "localhost"
                        || host
                            .parse::<std::net::IpAddr>()
                            .is_ok_and(|ip| ip.is_loopback())
                }))
        || input.expires_at <= store::now()
        || input.expires_at > store::now() + 86400
    {
        return Err(Error::invalid(
            "browser grants require an exact application origin and expiry within 24 hours",
        ));
    }
    let (request, session) = match input.session {
        GrantSession::Existing(id) => {
            app.store.owned(p, &id, false).await?;
            let saved = app.brain.summary(&p.account, &id).await?;
            let binding = saved
                .config
                .bindings
                .as_deref()
                .unwrap_or_default()
                .iter()
                .find(|binding| binding.alias == "runtime")
                .ok_or_else(Error::denied)?;
            if !descriptors_equal(binding.runtime.as_deref().unwrap_or_default(), &input.tools)? {
                return Err(Error::denied());
            }
            (
                CreateSession {
                    harness: saved.harness,
                    config: saved.config,
                    ..Default::default()
                },
                Some(id.to_owned()),
            )
        }
        GrantSession::Create(request) => {
            if request
                .config
                .bindings
                .as_deref()
                .unwrap_or_default()
                .iter()
                .any(|binding| binding.executor.is_some())
            {
                return Err(Error::invalid(
                    "browser callbacks must register in their declaring tab",
                ));
            }
            crate::admission::create(app, p, &request).await?;
            (request, None)
        }
    };
    let public = public_request(&request)?;
    let mut pending = app.pending_clients.lock().await;
    pending.retain(|_, value| value.until > store::now());
    if pending.len() >= app.config.limits.requests {
        return Err(Error::capacity());
    }
    let id = identity::random("browser");
    let token = identity::random("client");
    let verifier = identity::digest(token.as_bytes());
    sqlx::query("INSERT INTO browser_grants(id,verifier,account,issuing_key,origin,expires_at,session,composition,tools) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
        .bind(&id).bind(&verifier).bind(&p.account).bind(&p.key).bind(&input.origin).bind(input.expires_at).bind(&session).bind(serde_json::to_string(&public)?).bind(serde_json::to_string(&input.tools)?).execute(&app.store.0).await?;
    if session.is_none() {
        pending.insert(
            verifier.clone(),
            Pending {
                request,
                until: input.expires_at.min(store::now() + 300),
                ceiling: headers.get("x-aex-max-cost-micro-usd").cloned(),
            },
        );
    }
    Ok((
        StatusCode::CREATED,
        [("cache-control", "no-store")],
        Json(ClientAccess {
            id,
            token,
            expires_at: input.expires_at,
            session: serde_json::from_value(public)?,
            tools: input.tools,
        }),
    )
        .into_response())
}
pub async fn access(app: &App, headers: &HeaderMap, token: &str) -> Result<Option<Access>> {
    let verifier = identity::digest(token.as_bytes());
    let Some(row) = sqlx::query("SELECT * FROM browser_grants WHERE verifier=$1")
        .bind(&verifier)
        .fetch_optional(&app.store.0)
        .await?
    else {
        return Ok(None);
    };
    from_row(headers, row, verifier)
}
pub async fn executor_access(app: &App, headers: &HeaderMap, id: &str) -> Result<Option<Access>> {
    let Some(row) = sqlx::query(
        "SELECT g.* FROM browser_grants g JOIN executors e ON e.browser_grant=g.id WHERE e.id=$1",
    )
    .bind(id)
    .fetch_optional(&app.store.0)
    .await?
    else {
        return Ok(None);
    };
    let verifier: String = row.get("verifier");
    from_row(headers, row, verifier)
}
fn from_row(
    headers: &HeaderMap,
    row: sqlx::postgres::PgRow,
    verifier: String,
) -> Result<Option<Access>> {
    let origin = headers.get("origin").and_then(|value| value.to_str().ok());
    if !row.get::<bool, _>("active")
        || row.get::<i64, _>("expires_at") <= store::now()
        || !(origin == Some(row.get::<&str, _>("origin"))
            || origin.is_none()
                && headers
                    .get("sec-fetch-site")
                    .is_some_and(|value| value == "same-origin"))
    {
        return Err(Error::denied());
    }
    Ok(Some(Access {
        principal: Principal {
            account: row.get("account"),
            key: row.get("issuing_key"),
        },
        id: row.get("id"),
        verifier,
        session: row.get("session"),
        expires_at: row.get("expires_at"),
        composition: serde_json::from_str(row.get("composition"))?,
        tools: serde_json::from_str(row.get("tools"))?,
    }))
}
impl Access {
    pub fn authorize(&self, route: &crate::http::Route<'_>) -> Result<()> {
        use crate::http::Route;
        match route {
            Route::Composition | Route::Register | Route::Create => Ok(()),
            Route::Session(id, op)
                if self.session.as_deref() == Some(*id)
                    && ["", "events", "resume", "close"].contains(op) =>
            {
                Ok(())
            }
            Route::Executor(_, _) => Ok(()),
            _ => Err(Error::denied()),
        }
    }
    pub fn executor(&self, input: &ExecutorRegistration) -> Result<()> {
        if input.binding != "runtime"
            || input.session != self.session
            || !descriptors_equal(&input.tools, &self.tools)?
        {
            return Err(Error::denied());
        }
        Ok(())
    }
    pub fn composition(&self) -> Result<Composition> {
        Ok(Composition {
            create: serde_json::from_value(self.composition.clone())?,
            session: self.session.clone(),
            tools: self.tools.clone(),
        })
    }
}
pub async fn create(
    app: &App,
    access: &Access,
    headers: &HeaderMap,
    body: Bytes,
) -> Result<Response> {
    if access.session.is_some() {
        return Err(Error::conflict(
            "browser grant is already bound; read its composition and rebind the tab",
        ));
    }
    let mut request: CreateSession = serde_json::from_slice(&body)?;
    let mut expected: CreateSession = serde_json::from_value(access.composition.clone())?;
    let runtime: Vec<_> = request
        .config
        .bindings
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter(|binding| binding.executor.is_some())
        .cloned()
        .collect();
    if access.tools.is_empty() {
        if !runtime.is_empty() {
            return Err(Error::denied());
        }
    } else {
        if runtime.len() != 1 {
            return Err(Error::denied());
        }
        let binding = &runtime[0];
        if binding.alias != "runtime"
            || !descriptors_equal(
                binding.runtime.as_deref().unwrap_or_default(),
                &access.tools,
            )?
        {
            return Err(Error::denied());
        }
        app.store
            .own_executor(
                &access.principal,
                binding.executor.as_deref().ok_or_else(Error::denied)?,
            )
            .await?;
        let issuer: Option<String> =
            sqlx::query_scalar("SELECT browser_grant FROM executors WHERE id=$1")
                .bind(binding.executor.as_deref())
                .fetch_one(&app.store.0)
                .await?;
        if issuer.as_deref() != Some(&access.id) {
            return Err(Error::denied());
        }
        expected
            .config
            .bindings
            .get_or_insert_with(Vec::new)
            .push(ToolBinding {
                alias: "runtime".into(),
                executor: binding.executor.clone(),
                runtime: Some(access.tools.clone()),
                ..Default::default()
            });
    }
    if serde_json::to_value(&request)? != serde_json::to_value(expected)? {
        return Err(Error::denied());
    }
    let pending = app
        .pending_clients
        .lock()
        .await
        .get(&access.verifier)
        .filter(|pending| pending.until > store::now())
        .cloned()
        .ok_or_else(|| Error::conflict("private browser composition expired; issue a new grant"))?;
    request.model_credential = pending.request.model_credential;
    request.credentials = pending.request.credentials;
    let mut headers = headers.clone();
    if let Some(ceiling) = pending.ceiling {
        headers.insert("x-aex-max-cost-micro-usd", ceiling);
    }
    let response = crate::sessions::create(
        app,
        &access.principal,
        &headers,
        Bytes::from(serde_json::to_vec(&request)?),
        &access.id,
    )
    .await?;
    if !response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let bytes = to_bytes(response.into_body(), app.config.limits.response_bytes)
        .await
        .map_err(|_| Error::ambiguous())?;
    let session: crate::brain_contract::Session = serde_json::from_slice(&bytes)?;
    let bound = sqlx::query(
        "UPDATE browser_grants SET session=$1 WHERE id=$2 AND active AND expires_at>$3",
    )
    .bind(&session.id)
    .bind(&access.id)
    .bind(store::now())
    .execute(&app.store.0)
    .await?
    .rows_affected();
    if bound != 1 {
        return Err(Error::denied());
    }
    app.pending_clients.lock().await.remove(&access.verifier);
    Ok((status, Json(session)).into_response())
}
pub async fn revoke(app: &App, p: &Principal, id: &str) -> Result<Response> {
    let row = sqlx::query(
        "UPDATE browser_grants SET active=FALSE WHERE id=$1 AND account=$2 RETURNING verifier",
    )
    .bind(id)
    .bind(&p.account)
    .fetch_optional(&app.store.0)
    .await?
    .ok_or_else(Error::missing)?;
    app.pending_clients
        .lock()
        .await
        .remove(row.get::<&str, _>("verifier"));
    app.changed
        .send_modify(|value| *value = value.wrapping_add(1));
    Ok(StatusCode::NO_CONTENT.into_response())
}
pub async fn active(app: &App, verifier: &str) -> bool {
    sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM browser_grants g JOIN api_keys k ON k.id=g.issuing_key JOIN accounts a ON a.id=g.account WHERE g.verifier=$1 AND g.active AND g.expires_at>$2 AND k.active=1 AND a.active=1)").bind(verifier).bind(store::now()).fetch_one(&app.store.0).await.unwrap_or(false)
}
