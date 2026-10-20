use crate::{
    App,
    error::{Error, Result},
    identity, sessions,
};
use axum::{
    Json, Router,
    body::{Body, Bytes, to_bytes},
    extract::{Request, State},
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;

pub fn router(app: App) -> Router {
    Router::new().fallback(handle).with_state(app)
}
#[derive(Debug, PartialEq)]
pub enum Route<'a> {
    Account,
    Billing,
    PaymentWebhook,
    ToolCompletion(&'a str),
    Models,
    Tools,
    Clients,
    ClientRevoke(&'a str),
    Composition,
    AttachmentLimits,
    AttachmentGrant(&'a str),
    AttachmentPut(&'a str),
    AttachmentMetadata(&'a str, &'a str),
    AttachmentUpload(&'a str),
    AttachmentDelete(&'a str, &'a str),
    AttachmentContent(&'a str),
    Health(bool),
    Create,
    List,
    Register,
    Executor(&'a str, &'a str),
    Session(&'a str, &'a str),
}
fn id(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
pub fn route<'a>(method: &Method, path: &'a str) -> Result<Route<'a>> {
    if path.len() > 512
        || path
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || b"/_.:-".contains(&byte)))
    {
        return Err(Error::missing());
    }
    if crate::account::route(method, path) {
        return Ok(Route::Account);
    }
    if crate::billing::route(method, path) {
        return Ok(Route::Billing);
    }
    let parts: Vec<_> = path.split('/').collect();
    match (method.as_str(), parts.as_slice()) {
        ("POST", ["", "v1", "clients"]) => Ok(Route::Clients),
        ("DELETE", ["", "v1", "clients", id]) => Ok(Route::ClientRevoke(id)),
        ("POST", ["", "v1", "webhooks", "stripe"]) => Ok(Route::PaymentWebhook),
        ("POST", ["", "v1", "tool-completions", token])
            if token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit()) =>
        {
            Ok(Route::ToolCompletion(token))
        }
        ("GET", ["", "v1", "models"]) => Ok(Route::Models),
        ("GET", ["", "tools"]) => Ok(Route::Tools),
        ("GET", ["", "composition"]) => Ok(Route::Composition),
        ("GET", ["", "v1", "attachments", "limits"]) => Ok(Route::AttachmentLimits),
        ("POST", ["", "v1", "sessions", session, "attachment-grants"]) if id(session) => {
            Ok(Route::AttachmentGrant(session))
        }
        ("PUT", ["", "v1", "attachments", attachment, "upload"]) => {
            Ok(Route::AttachmentPut(attachment))
        }
        ("GET", ["", "v1", "sessions", session, "attachments", attachment]) if id(session) => {
            Ok(Route::AttachmentMetadata(session, attachment))
        }
        ("POST", ["", "v1", "sessions", session, "attachments"]) if id(session) => {
            Ok(Route::AttachmentUpload(session))
        }
        ("DELETE", ["", "v1", "sessions", session, "attachments", attachment]) if id(session) => {
            Ok(Route::AttachmentDelete(session, attachment))
        }
        ("GET" | "HEAD", ["", "v1", "attachments", attachment, "content"]) => {
            Ok(Route::AttachmentContent(attachment))
        }
        ("GET", ["", "health", "live"]) => Ok(Route::Health(false)),
        ("GET", ["", "health", "ready"]) => Ok(Route::Health(true)),
        ("POST", ["", "sessions"]) => Ok(Route::Create),
        ("GET", ["", "sessions"]) => Ok(Route::List),
        ("POST", ["", "executors"]) => Ok(Route::Register),
        ("GET", ["", "executors", executor, "commands"]) if id(executor) => {
            Ok(Route::Executor(executor, "commands"))
        }
        (
            "POST",
            [
                "",
                "executors",
                executor,
                operation @ ("responses" | "close"),
            ],
        ) if id(executor) => Ok(Route::Executor(executor, operation)),
        ("GET" | "DELETE", ["", "sessions", session]) if id(session) => {
            Ok(Route::Session(session, ""))
        }
        ("GET", ["", "sessions", session, operation @ "events"]) if id(session) => {
            Ok(Route::Session(session, operation))
        }
        (
            "POST",
            [
                "",
                "sessions",
                session,
                operation @ ("events" | "resume" | "close"),
            ],
        ) if id(session) => Ok(Route::Session(session, operation)),
        _ => Err(Error::missing()),
    }
}
async fn handle(State(app): State<App>, request: Request) -> Response {
    let origin = request
        .headers()
        .get("origin")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let request_id = identity::random("req");
    let upload = request.uri().path().starts_with("/v1/attachments/")
        && request.uri().path().ends_with("/upload");
    let allowed = if let Some(origin) = origin.as_deref() {
        if upload {
            app.config
                .attachments
                .as_ref()
                .is_some_and(|config| config.upload_origins.contains(origin))
        } else {
            // Known applications must also receive readable auth failures after
            // expiry so an attached executor can finish its local cleanup.
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM browser_grants WHERE origin=$1)",
            )
            .bind(origin)
            .fetch_one(&app.store.0)
            .await
            .unwrap_or(false)
        }
    } else {
        true
    };
    let result = if !allowed {
        Err(Error::denied())
    } else if request.method() == Method::OPTIONS {
        let method = request
            .headers()
            .get("access-control-request-method")
            .and_then(|value| value.to_str().ok())
            .and_then(|method| Method::from_bytes(method.as_bytes()).ok());
        if origin.is_some()
            && method.is_some_and(|method| route(&method, request.uri().path()).is_ok())
        {
            Ok(StatusCode::NO_CONTENT.into_response())
        } else {
            Err(Error::denied())
        }
    } else {
        dispatch(app, request).await
    };
    let mut response = match result {
        Ok(response) => response,
        Err(error) => error.into_response(),
    };
    response
        .headers_mut()
        .insert("x-aex-request-id", request_id.parse().unwrap());
    response
        .headers_mut()
        .insert("x-content-type-options", "nosniff".parse().unwrap());
    if !response.headers().contains_key("cache-control") {
        response
            .headers_mut()
            .insert("cache-control", "no-store".parse().unwrap());
    }
    if allowed && let Some(origin) = origin {
        response
            .headers_mut()
            .insert("access-control-allow-origin", origin.parse().unwrap());
        response
            .headers_mut()
            .insert("vary", "Origin".parse().unwrap());
        response.headers_mut().insert(
            "access-control-allow-methods",
            "GET,POST,DELETE,PUT".parse().unwrap(),
        );
        response.headers_mut().insert("access-control-allow-headers","authorization,content-type,idempotency-key,x-aex-max-cost-micro-usd,x-aex-download-budget-bytes,x-aex-expires-at".parse().unwrap());
    }
    response
}
async fn dispatch(app: App, request: Request) -> Result<Response> {
    let (parts, body) = request.into_parts();
    let path = parts.uri.path();
    let route = route(&parts.method, path)?;
    if parts.headers.keys().any(|name| {
        matches!(
            name.as_str(),
            "x-aex-tenant" | "x-aex-account" | "x-aex-session"
        ) || name.as_str().starts_with("x-brain-")
    }) {
        return Err(Error::denied());
    }
    let _permit = crate::admission::request(
        if matches!(
            route,
            Route::Executor(_, "responses" | "close") | Route::ToolCompletion(_)
        ) {
            app.completions.clone()
        } else {
            app.requests.clone()
        },
        app.config.attachments.is_some()
            && !matches!(
                route,
                Route::AttachmentContent(_) | Route::Executor(_, _) | Route::ToolCompletion(_)
            ),
    )?;
    if let Route::Health(ready) = route {
        return Ok(if !ready
            || app.accepting.load(std::sync::atomic::Ordering::SeqCst)
                && app.brain.ready().await
                && app.store.ready().await
        {
            StatusCode::NO_CONTENT
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        }
        .into_response());
    }
    if matches!(route, Route::PaymentWebhook) {
        if parts.uri.query().is_some() {
            return Err(Error::invalid("unsupported query"));
        }
        let bytes = to_bytes(body, 1 << 20)
            .await
            .map_err(|_| Error::capacity())?;
        return crate::payments::webhook(&app, &parts.headers, bytes).await;
    }
    if let Route::ToolCompletion(token) = route {
        if parts.uri.query().is_some() {
            return Err(Error::invalid("unsupported query"));
        }
        let origin = app
            .config
            .application_tool_url
            .as_ref()
            .ok_or_else(Error::missing)?;
        let body =
            tokio::time::timeout(std::time::Duration::from_secs(30), to_bytes(body, 2 << 20))
                .await
                .map_err(|_| Error::capacity())?
                .map_err(|_| Error::capacity())?;
        let response = app
            .application_client
            .post(format!("{}/complete/{token}", origin.trim_end_matches('/')))
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|_| Error::internal())?;
        return finite(&app, response).await;
    }
    if !app.accepting.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(Error::capacity());
    }
    if let Route::AttachmentContent(attachment) = route {
        return crate::attachments::download(
            &app,
            attachment,
            parts.uri.query(),
            parts.method == Method::HEAD,
            _permit,
        )
        .await;
    }
    if parts.uri.query().is_some()
        && !matches!(
            route,
            Route::Session(_, "events") | Route::List | Route::Billing
        )
    {
        return Err(Error::invalid("unsupported query"));
    }
    if let Some(query) = parts.uri.query()
        && matches!(route, Route::Session(_, "events") | Route::List)
        && url::form_urlencoded::parse(query.as_bytes())
            .any(|(name, _)| !["after", "limit"].contains(&name.as_ref()))
    {
        return Err(Error::invalid("unsupported query"));
    }
    let bytes = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        to_bytes(body, app.config.limits.request_bytes),
    )
    .await
    .map_err(|_| Error::invalid("request body timed out"))?
    .map_err(|_| Error::invalid("request body exceeds limit"))?;
    if matches!(route, Route::Account) {
        let token = if path == "/v1/auth/exchange" {
            ""
        } else {
            identity::bearer(&parts.headers)?
        };
        return crate::account::handle(&app, &parts.method, path, token, bytes).await;
    }
    if matches!(route, Route::Billing) {
        return crate::billing::handle(
            &app,
            &parts.method,
            path,
            parts.uri.query(),
            &parts.headers,
            bytes,
        )
        .await;
    }
    let token = identity::bearer(&parts.headers)?;
    let access = if let Route::Executor(executor, _) = route {
        crate::clients::executor_access(&app, &parts.headers, executor).await?
    } else {
        crate::clients::access(&app, &parts.headers, token).await?
    };
    if let Some(access) = &access {
        access.authorize(&route)?;
    }
    if parts.headers.contains_key("origin")
        && access.is_none()
        && !matches!(route, Route::AttachmentPut(_))
    {
        return Err(Error::denied());
    }
    let principal = if let Route::Executor(executor, _) = route {
        app.store.executor(executor, token).await?
    } else if let Route::AttachmentPut(attachment) = route {
        crate::attachments::grants::principal(&app, attachment, token).await?
    } else if let Some(access) = &access {
        access.principal.clone()
    } else {
        app.store.principal(token).await?
    };
    app.store.active(&principal).await?;
    let semaphore = app
        .account_requests
        .lock()
        .await
        .entry(principal.account.clone())
        .or_insert_with(|| {
            std::sync::Arc::new(tokio::sync::Semaphore::new(
                app.config.limits.requests_per_account,
            ))
        })
        .clone();
    let _account = crate::admission::request(semaphore, app.config.attachments.is_some())?;
    let mut stream_session = None;
    match route {
        Route::Clients => {
            return crate::clients::grant(
                &app,
                &principal,
                &parts.headers,
                serde_json::from_slice(&bytes)?,
            )
            .await;
        }
        Route::ClientRevoke(id) => return crate::clients::revoke(&app, &principal, id).await,
        Route::Composition => {
            return Ok(
                Json(access.as_ref().ok_or_else(Error::denied)?.composition()?).into_response(),
            );
        }
        Route::Models => {
            return Ok(
                Json(json!({"models":app.config.models,"harnesses":app.config.harnesses}))
                    .into_response(),
            );
        }
        Route::Tools => return Ok(Json(crate::managed::catalog(&app, &principal)).into_response()),
        Route::AttachmentLimits => {
            return Ok(Json(crate::attachments::grants::limits(&app)?).into_response());
        }
        Route::AttachmentGrant(session) => {
            return crate::attachments::grants::create(
                &app,
                &principal,
                session,
                &parts.headers,
                serde_json::from_slice(&bytes)?,
            )
            .await;
        }
        Route::AttachmentPut(attachment) => {
            return crate::attachments::grants::receive(
                &app,
                &principal,
                attachment,
                &parts.headers,
                bytes,
            )
            .await;
        }
        Route::AttachmentMetadata(session, attachment) => {
            return crate::attachments::grants::metadata(&app, &principal, session, attachment)
                .await;
        }
        Route::AttachmentUpload(session) => {
            return crate::attachments::upload(&app, &principal, session, &parts.headers, bytes)
                .await;
        }
        Route::AttachmentDelete(session, attachment) => {
            return crate::attachments::revoke(&app, &principal, session, attachment).await;
        }
        Route::Create => {
            if let Some(access) = &access {
                return crate::clients::create(&app, access, &parts.headers, bytes).await;
            }
            return sessions::create(
                &app,
                &principal,
                &parts.headers,
                bytes,
                identity::operation_key(&parts.headers)?,
            )
            .await;
        }
        Route::List => return sessions::list(&app, &principal, parts.uri.query()).await,
        Route::Register => {
            return crate::hosts::register(
                &app,
                &principal,
                &parts.headers,
                bytes,
                access.as_ref(),
            )
            .await;
        }
        Route::Session(session, operation) => {
            app.store
                .owned(&principal, session, parts.method == Method::DELETE)
                .await?;
            stream_session = Some(session.to_owned());
            if parts.method == Method::DELETE {
                return sessions::delete(&app, session, &parts.headers).await;
            }
            if parts.method == Method::POST && operation == "events" {
                let input: serde_json::Value = serde_json::from_slice(&bytes)?;
                if input["type"] != "user.interrupt" {
                    return crate::turns::send(&app, &principal, session, &parts.headers, bytes)
                        .await;
                }
            }
        }
        Route::Executor(_, _) => {}
        Route::Account
        | Route::Billing
        | Route::PaymentWebhook
        | Route::ToolCompletion(_)
        | Route::Health(_)
        | Route::AttachmentContent(_) => unreachable!(),
    }
    let stream = matches!(route, Route::Executor(_, "commands"))
        || matches!(route, Route::Session(_, "events"))
            && parts
                .headers
                .get("accept")
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.contains("text/event-stream"));
    let stream_permit = if stream {
        let semaphore = app
            .streams
            .lock()
            .await
            .entry(principal.account.clone())
            .or_insert_with(|| {
                std::sync::Arc::new(tokio::sync::Semaphore::new(
                    app.config.limits.streams_per_account,
                ))
            })
            .clone();
        Some(
            semaphore
                .try_acquire_owned()
                .map_err(|_| Error::capacity())?,
        )
    } else {
        None
    };
    let mut headers = parts.headers.clone();
    if let Some(session) = access.as_ref().and_then(|access| access.session.as_deref()) {
        headers.insert(
            "x-aex-session",
            session.parse().map_err(|_| Error::internal())?,
        );
    }
    let executor_token = if matches!(route, Route::Executor(_, _)) {
        Some(token)
    } else {
        None
    };
    let response = app
        .brain
        .request(
            parts.method,
            parts.uri.path_and_query().unwrap().as_str(),
            &headers,
            bytes,
            &principal.account,
            executor_token,
        )
        .await?;
    if let Route::Executor(executor, "close") = route
        && (response.status().is_success() || response.status() == StatusCode::UNAUTHORIZED)
    {
        sqlx::query("DELETE FROM executors WHERE id=$1 AND account=$2")
            .bind(executor)
            .bind(&principal.account)
            .execute(&app.store.0)
            .await?;
    }
    if !stream || !response.status().is_success() {
        return finite(&app, response).await;
    }
    let executor = if let Route::Executor(executor, _) = route {
        Some(executor.to_owned())
    } else {
        None
    };
    let mut upstream = response.bytes_stream();
    let mut changes = app.changed.subscribe();
    let stream = async_stream::stream! {
        let _stream_permit=stream_permit;
        let mut check=tokio::time::interval(std::time::Duration::from_secs(1));
        let mut tail=Vec::new();let mut boundary=true;let mut ticks=0;
        let expiry=access.as_ref().map(|access|access.expires_at.saturating_sub(crate::store::now()).max(0) as u64).unwrap_or(0);
        let expiry=tokio::time::sleep(std::time::Duration::from_secs(expiry));tokio::pin!(expiry);
        loop {tokio::select!{
            biased;
            _=&mut expiry,if access.is_some()=>break,
            changed=changes.changed()=>{
                if changed.is_err() || !authorized_stream(&app,&principal,stream_session.as_deref(),executor.as_deref(),access.as_ref()).await {break;}
            },
            _=check.tick()=>{
                if !authorized_stream(&app,&principal,stream_session.as_deref(),executor.as_deref(),access.as_ref()).await {break;}
                ticks+=1;if boundary && ticks%15==0 {yield Ok::<Bytes,std::io::Error>(Bytes::from_static(b": keepalive\n\n"));}
            },
            chunk=upstream.next()=>match chunk {
                Some(Ok(bytes))=>{tail.extend_from_slice(&bytes[bytes.len().saturating_sub(4)..]);if tail.len()>4 {tail.drain(..tail.len()-4);} boundary=tail.ends_with(b"\n\n") || tail.ends_with(b"\r\n\r\n");yield Ok(bytes);},
                Some(Err(_))=>{yield Err(std::io::Error::other("upstream stream interrupted"));break;},None=>break,
            }
        }}
    };
    Ok(Response::builder()
        .status(200)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-store")
        .body(Body::from_stream(stream))
        .unwrap())
}
async fn authorized_stream(
    app: &App,
    p: &identity::Principal,
    session: Option<&str>,
    executor: Option<&str>,
    access: Option<&crate::clients::Access>,
) -> bool {
    if !app.accepting.load(std::sync::atomic::Ordering::SeqCst)
        || app.store.active(p).await.is_err()
    {
        return false;
    }
    if let Some(session) = session
        && app.store.owned(p, session, false).await.is_err()
    {
        return false;
    }
    if let Some(executor) = executor
        && app.store.own_executor(p, executor).await.is_err()
    {
        return false;
    }
    if let Some(access) = access
        && !crate::clients::active(app, &access.verifier).await
    {
        return false;
    }
    true
}
#[derive(Serialize, Deserialize)]
struct Reply {
    status: u16,
    body: serde_json::Value,
}
pub fn saved(value: serde_json::Value) -> Result<Response> {
    let reply: Reply = serde_json::from_value(value)?;
    Ok((
        StatusCode::from_u16(reply.status).map_err(|_| Error::internal())?,
        Json(reply.body),
    )
        .into_response())
}
pub async fn finite(app: &App, response: reqwest::Response) -> Result<Response> {
    let status = response.status();
    let request_id = response.headers().get("x-request-id").cloned();
    let bytes = app.brain.bytes(response).await?;
    if status.is_server_error() {
        return Ok((status,Json(json!({"error":{"code":"brain_error","message":"Brain operation failed; inspect session events or contact the operator"}}))).into_response());
    }
    let mut response = Response::builder()
        .status(status)
        .header("content-type", "application/json");
    if let Some(id) = request_id {
        response = response.header("x-request-id", id);
    }
    Ok(response.body(Body::from(bytes)).unwrap())
}
