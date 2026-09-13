use crate::{
    App, admission,
    error::{Error, Result},
    hosts, identity, sessions,
};
use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
    extract::{Request, State},
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use tracing::Instrument;

pub fn router(app: App) -> Router {
    Router::new().fallback(handle).with_state(app)
}

#[derive(Debug, PartialEq)]
pub enum Route<'a> {
    Account,
    Models,
    AttachmentUpload(&'a str),
    AttachmentDelete(&'a str, &'a str),
    AttachmentContent(&'a str),
    Health(bool),
    Create,
    List,
    Register,
    Artifact(&'a str, Option<&'a str>),
    Host(&'a str, &'a str),
    Session(&'a str, &'a str),
}
pub fn route<'a>(method: &Method, path: &'a str) -> Result<Route<'a>> {
    if path.len() > 512
        || path
            .bytes()
            .any(|c| !(c.is_ascii_alphanumeric() || b"/_.:-".contains(&c)))
    {
        return Err(Error::missing());
    }
    let parts: Vec<_> = path.split('/').collect();
    if crate::account::route(method, path) {
        return Ok(Route::Account);
    }
    match (method.as_str(), parts.as_slice()) {
        ("GET", ["", "v1", "models"]) => Ok(Route::Models),
        ("POST", ["", "v1", "sessions", session, "attachments"]) => {
            Ok(Route::AttachmentUpload(session))
        }
        ("DELETE", ["", "v1", "sessions", session, "attachments", id]) => {
            Ok(Route::AttachmentDelete(session, id))
        }
        ("GET" | "HEAD", ["", "v1", "attachments", id, "content"]) => {
            Ok(Route::AttachmentContent(id))
        }
        ("GET", ["", "health", "live"]) => Ok(Route::Health(false)),
        ("GET", ["", "health", "ready"]) => Ok(Route::Health(true)),
        ("POST", ["", "v1", "sessions"]) => Ok(Route::Create),
        ("GET", ["", "v1", "sessions"]) => Ok(Route::List),
        ("POST", ["", "v1", "hosts"]) => Ok(Route::Register),
        ("POST", ["", "v1", kind @ ("agentloops" | "tools")]) => Ok(Route::Artifact(kind, None)),
        ("GET", ["", "v1", kind @ ("agentloops" | "tools"), id]) => {
            Ok(Route::Artifact(kind, Some(id)))
        }
        ("GET", ["", "v1", "hosts", id, "commands"]) => Ok(Route::Host(id, "commands")),
        ("POST", ["", "v1", "hosts", id, op @ ("results" | "events")]) => Ok(Route::Host(id, op)),
        ("GET" | "DELETE", ["", "v1", "sessions", id]) => Ok(Route::Session(id, "")),
        ("GET", ["", "v1", "sessions", id, op @ ("events" | "transcript")]) => {
            Ok(Route::Session(id, op))
        }
        (
            "POST",
            [
                "",
                "v1",
                "sessions",
                id,
                op @ ("messages" | "cancel" | "end"),
            ],
        ) => Ok(Route::Session(id, op)),
        _ => Err(Error::missing()),
    }
}

async fn handle(State(app): State<App>, request: Request) -> Response {
    let request_id = identity::random("req");
    let span = tracing::info_span!("request", request_id, method = %request.method(), route = tracing::field::Empty, account = tracing::field::Empty, session = tracing::field::Empty);
    async move {
        let started = std::time::Instant::now();
        let mut response = match dispatch(app, request).await {
            Ok(response) => response,
            Err(error) => error.into_response(),
        };
        tracing::info!(
            status = response.status().as_u16(),
            header_ms = started.elapsed().as_secs_f64() * 1000.0,
            "request completed"
        );
        response.headers_mut().insert(
            "x-aex-request-id",
            request_id.parse().expect("generated request identifier"),
        );
        response
    }
    .instrument(span)
    .await
}

async fn dispatch(app: App, request: Request) -> Result<Response> {
    let (parts, body) = request.into_parts();
    let path = parts.uri.path();
    let route = route(&parts.method, path)?;
    let reserve_download = app.config.attachments.is_some();
    let _permit = crate::admission::request(
        app.requests.clone(),
        reserve_download && !matches!(route, Route::AttachmentContent(_)),
    )?;
    tracing::Span::current().record(
        "route",
        match route {
            Route::Account => "account",
            Route::Models => "models",
            Route::AttachmentUpload(_)
            | Route::AttachmentDelete(_, _)
            | Route::AttachmentContent(_) => "attachments",
            Route::Health(_) => "health",
            Route::Create => "create",
            Route::List => "list",
            Route::Register => "register",
            Route::Artifact(kind, _) => kind,
            Route::Host(_, op) | Route::Session(_, op) => op,
        },
    );
    if let Route::Health(ready) = route {
        return Ok(if !ready
            || (app.accepting.load(std::sync::atomic::Ordering::SeqCst)
                && app.brain.ready().await
                && app.store.ready().await)
        {
            StatusCode::NO_CONTENT
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        }
        .into_response());
    }
    if !app.accepting.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(Error::capacity());
    }
    if let Route::AttachmentContent(id) = route {
        return crate::attachments::download(
            &app,
            id,
            parts.uri.query(),
            parts.method == Method::HEAD,
            _permit,
        )
        .await;
    }
    if let Some(query) = parts.uri.query() {
        let valid = match route {
            Route::Session(_, "events") => query
                .strip_prefix("after=")
                .is_some_and(|v| v.parse::<u64>().is_ok()),
            Route::Models => true,
            _ => false,
        };
        if !valid {
            return Err(Error::invalid("unsupported query parameters"));
        }
    }
    let token = if path == "/v1/auth/exchange" {
        ""
    } else {
        identity::bearer(&parts.headers)?
    };
    if matches!(route, Route::Account) {
        let body = tokio::time::timeout(std::time::Duration::from_secs(10), to_bytes(body, 4096))
            .await
            .map_err(|_| Error::invalid("request body timed out"))?
            .map_err(|_| Error::invalid("request body exceeds limit"))?;
        let mut response = crate::account::handle(&app, &parts.method, path, token, body).await?;
        response
            .headers_mut()
            .insert("cache-control", "no-store".parse().unwrap());
        return Ok(response);
    }
    let principal = match route {
        Route::Host(id, _) => app.store.host(id, token).await?,
        _ => app.store.principal(token).await?,
    };
    tracing::Span::current().record("account", &principal.account);
    let _account_permit = {
        let mut requests = app.account_requests.lock().await;
        let semaphore = requests
            .entry(principal.account.clone())
            .or_insert_with(|| {
                std::sync::Arc::new(tokio::sync::Semaphore::new(
                    app.config.limits.requests_per_account,
                ))
            })
            .clone();
        crate::admission::request(semaphore, reserve_download)?
    };
    let mut changes = app.changed.subscribe();
    app.store.active(&principal).await?;
    let body = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        to_bytes(body, app.config.limits.request_bytes),
    )
    .await
    .map_err(|_| Error::invalid("request body timed out"))?
    .map_err(|_| Error::invalid("request body exceeds limit"))?;
    let mut upstream_key = None;
    let mut host_token = None;
    let mut stream_session = None;
    let mut turn_session = None;
    match route {
        Route::AttachmentUpload(session) => {
            return crate::attachments::upload(&app, &principal, session, &parts.headers, body)
                .await;
        }
        Route::AttachmentDelete(session, id) => {
            return crate::attachments::revoke(&app, &principal, session, id).await;
        }
        Route::AttachmentContent(_) => unreachable!(),
        Route::Models => {
            let response = app
                .brain
                .request(
                    Method::GET,
                    &parts.uri.to_string(),
                    &parts.headers,
                    Bytes::new(),
                    None,
                    None,
                )
                .await?;
            return finite(&app, response).await;
        }
        Route::Create => {
            return sessions::create(
                &app,
                &principal,
                &parts.headers,
                body,
                identity::operation_key(&parts.headers)?,
            )
            .await;
        }
        Route::List => return sessions::list(&app, &principal).await,
        Route::Register => return hosts::register(&app, &principal, &parts.headers).await,
        Route::Artifact(kind, id) => {
            return crate::artifacts::handle(&app, &principal, kind, id, &parts.headers, body)
                .await;
        }
        Route::Host(_, op) => {
            host_token = Some(token);
            if op == "results" {
                let result: brain_protocol::HostResult = serde_json::from_slice(&body)?;
                app.store
                    .owned(&principal, result.session_id.as_str(), false)
                    .await?;
            } else if op == "events" {
                let event: brain_protocol::HostEvent = serde_json::from_slice(&body)?;
                app.store
                    .owned(&principal, event.session_id.as_str(), false)
                    .await?;
            }
        }
        Route::Session(id, op) => {
            app.store
                .owned(&principal, id, parts.method == Method::DELETE)
                .await?;
            tracing::Span::current().record("session", id);
            stream_session = Some(id.to_string());
            if parts.method != Method::GET {
                let key = identity::scoped_key(
                    &principal,
                    path,
                    identity::operation_key(&parts.headers)?,
                );
                if parts.method == Method::DELETE {
                    return sessions::delete(&app, id, &parts.headers, &key).await;
                }
                if op == "messages" {
                    let _: brain_protocol::MessageRequest = serde_json::from_slice(&body)?;
                    admission::disk(&app).await?;
                    app.store
                        .reserve_turn(&principal, id, &key, &app.config.limits)
                        .await?;
                    turn_session = Some(id.to_string());
                }
                upstream_key = Some(key);
            }
        }
        Route::Health(_) | Route::Account => unreachable!(),
    }
    let wants_stream = matches!(route, Route::Host(_, "commands"))
        || (matches!(route, Route::Session(_, "events"))
            && parts
                .headers
                .get("accept")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.contains("text/event-stream")));
    let stream_permit = if wants_stream {
        let mut streams = app.streams.lock().await;
        let semaphore = streams
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
    let response = app
        .brain
        .request(
            parts.method,
            parts.uri.path_and_query().unwrap().as_str(),
            &parts.headers,
            body,
            upstream_key.as_deref(),
            host_token,
        )
        .await?;
    if let Some(id) = turn_session {
        // The response can acknowledge a running turn. Only a known terminal state releases capacity.
        if let Ok(summary) = app.brain.summary(&id).await
            && !matches!(
                summary.status,
                brain_protocol::SessionStatus::Running
                    | brain_protocol::SessionStatus::Creating
                    | brain_protocol::SessionStatus::Ending
            )
        {
            app.store.finish_turn(&id).await?;
        }
    }
    if !wants_stream || !response.status().is_success() {
        return finite(&app, response).await;
    }
    let mut upstream = response.bytes_stream();
    let stream_host = match route {
        Route::Host(id, _) => Some(id.to_string()),
        _ => None,
    };
    let stream = async_stream::stream! {
        let _permit = stream_permit;
        let mut keepalive = tokio::time::interval(std::time::Duration::from_secs(15));
        let mut tail = Vec::new();
        let mut frame_boundary = true;
        loop {
            tokio::select! {
                biased;
                changed = changes.changed() => {
                    if changed.is_err() || !app.accepting.load(std::sync::atomic::Ordering::SeqCst) || app.store.active(&principal).await.is_err() { break; }
                    if let Some(id) = &stream_session && app.store.owned(&principal, id, false).await.is_err() { break; }
                    if let Some(id) = &stream_host && app.store.own_host(&principal, id).await.is_err() { break; }
                }
                chunk = upstream.next() => match chunk {
                    Some(Ok(bytes)) => {
                        tail.extend_from_slice(&bytes[bytes.len().saturating_sub(4)..]);
                        if tail.len() > 4 { tail.drain(..tail.len()-4); }
                        frame_boundary = tail.ends_with(b"\n\n") || tail.ends_with(b"\r\n\r\n");
                        yield Ok::<Bytes, std::io::Error>(bytes);
                    },
                    Some(Err(_)) => { yield Err(std::io::Error::other("upstream stream interrupted")); break; },
                    None => break,
                },
                _ = keepalive.tick(), if frame_boundary => yield Ok(Bytes::from_static(b": keepalive\n\n")),
            }
        }
    };
    Ok(Response::builder()
        .status(200)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-store")
        .body(Body::from_stream(stream))
        .unwrap())
}

pub async fn finite(app: &App, response: reqwest::Response) -> Result<Response> {
    let status = response.status();
    let content_type = response.headers().get("content-type").cloned();
    let body = app.brain.bytes(response).await?;
    if status.is_server_error() {
        let mut error: brain_protocol::ApiError = serde_json::from_slice(&body)
            .unwrap_or_else(|_| brain_protocol::ApiError::internal("upstream failed"));
        error.message =
            "Brain operation failed; inspect committed session events or contact the operator"
                .into();
        error.details = None;
        return Ok((status, axum::Json(error)).into_response());
    }
    let mut response = Response::builder()
        .status(status)
        .header("cache-control", "no-store");
    if let Some(content_type) = content_type {
        response = response.header("content-type", content_type);
    }
    Ok(response.body(Body::from(body)).unwrap())
}
