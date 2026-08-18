//! The HTTP surface: the control API (`contracts/control/v1`) plus every `session/v1` path,
//! served verbatim as an authorizing, admitting, metering proxy in front of the brain.
//!
//! Auth: the account token (`aex_at_`) for identity/billing endpoints, an API key (`aex_sk_`)
//! for session paths. Admission on session work: prepaid balance must be positive (402
//! `insufficient_balance`), the per-account concurrency and create-rate caps hold (429
//! `rate_limited`) — codes the session `ApiErrorCode` already defines, so the proxied surface
//! keeps its own error envelope. Responses of the control endpoints are built as JSON and
//! pinned to the schema by the e2e test; the generated Rust types serve the SDK side.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::Response;
use axum::routing::{any, delete, get, post};
use bytes::Bytes;
use serde_json::{Value, json};

use crate::brain::BrainClient;
use crate::identity::{self, bearer};
use crate::payments::{PaymentStatus, Payments};
use crate::rating::RateCard;
use crate::store::{AccountRow, Db, KeyRow, SessionRow, TopupRow};
use crate::sweep::{SweptLine, sweep_account, sweep_session};
use crate::{Error, Result, now_ms, rfc3339, usd_display};

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub brain: BrainClient,
    pub payments: Arc<dyn Payments>,
    pub card: RateCard,
    /// Defaults stamped onto new accounts: (max_concurrent_sessions, session_creates_per_hour).
    pub default_limits: (i64, i64),
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/accounts", post(create_account))
        .route("/v1/account", get(get_account))
        .route("/v1/keys", post(create_key).get(list_keys))
        .route("/v1/keys/{key_id}", delete(revoke_key))
        .route("/v1/balance", get(get_balance))
        .route("/v1/topups", post(create_topup).get(list_topups))
        .route("/v1/topups/{topup_id}", get(get_topup))
        .route("/v1/usage", get(get_usage))
        .route("/v1/rates", get(get_rates))
        .route("/v1/sessions", any(proxy_sessions_root))
        .route("/v1/sessions/{*rest}", any(proxy_session))
        .with_state(state)
}

// ---- plumbing ----

fn json_response(status: u16, value: &Value) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(value.to_string()))
        .expect("static response")
}

fn error_response(e: &Error) -> Response {
    json_response(
        e.status(),
        &json!({"error": {"code": e.code(), "message": e.to_string()}}),
    )
}

/// One `Result<Response>` -> `Response` seam so handlers can use `?`.
fn unwrap_response(r: Result<Response>) -> Response {
    r.unwrap_or_else(|e| error_response(&e))
}

fn parse_body<T: serde::de::DeserializeOwned>(body: &Bytes) -> Result<T> {
    serde_json::from_slice(body).map_err(|e| Error::Invalid(format!("request body: {e}")))
}

async fn auth_account(state: &AppState, headers: &HeaderMap) -> Result<AccountRow> {
    let token = bearer(headers).ok_or(Error::Unauthorized)?;
    if !token.starts_with("aex_at_") {
        return Err(Error::Unauthorized);
    }
    state
        .db
        .account_by_token_hash(identity::hash_secret(token))
        .await?
        .ok_or(Error::Unauthorized)
}

async fn auth_key(state: &AppState, headers: &HeaderMap) -> Result<(KeyRow, AccountRow)> {
    let token = bearer(headers).ok_or(Error::Unauthorized)?;
    if !token.starts_with("aex_sk_") {
        return Err(Error::Unauthorized);
    }
    state
        .db
        .key_auth(identity::hash_secret(token), now_ms())
        .await?
        .ok_or(Error::Unauthorized)
}

// ---- JSON shapes (pinned to contracts/control/v1 by the e2e schema validation) ----

fn account_json(a: &AccountRow) -> Value {
    json!({
        "id": a.id,
        "object": "account",
        "email": a.email,
        "created_at": rfc3339(a.created_ms),
        "limits": {
            "max_concurrent_sessions": a.max_concurrent_sessions,
            "session_creates_per_hour": a.session_creates_per_hour,
        },
    })
}

fn key_json(k: &KeyRow) -> Value {
    let mut v = json!({
        "id": k.id,
        "object": "api_key",
        "name": k.name,
        "prefix": k.prefix,
        "created_at": rfc3339(k.created_ms),
    });
    if let Some(ms) = k.last_used_ms {
        v["last_used_at"] = json!(rfc3339(ms));
    }
    if let Some(ms) = k.revoked_ms {
        v["revoked_at"] = json!(rfc3339(ms));
    }
    v
}

fn topup_json(t: &TopupRow) -> Value {
    let mut v = json!({
        "id": t.id,
        "object": "topup",
        "amount_cents": t.amount_cents,
        "status": t.status,
        "created_at": rfc3339(t.created_ms),
    });
    if t.status == "pending"
        && let Some(url) = &t.checkout_url
    {
        v["checkout_url"] = json!(url);
    }
    if let Some(ms) = t.paid_ms {
        v["paid_at"] = json!(rfc3339(ms));
    }
    v
}

fn usage_line_json(line: &SweptLine) -> Value {
    let row: &SessionRow = &line.row;
    json!({
        "session_id": row.id,
        "shape": row.shape,
        "state": row.fold.session_state,
        "running_ms": line.priced.running_ms,
        "suspended_byte_seconds": row.fold.suspended_byte_seconds,
        "workspace_byte_seconds": row.fold.workspace_byte_seconds,
        "artifact_byte_seconds": row.fold.artifact_byte_seconds,
        "compute_microusd": line.priced.compute_microusd,
        "storage_microusd": line.priced.storage_microusd,
        "total_microusd": line.priced.total_microusd,
        "storage": {
            "workspace_bytes": row.fold.workspace_bytes,
            "suspended_bytes": row.fold.suspended_bytes,
            "artifact_bytes": row.fold.artifact_bytes,
        },
        "metered_to": rfc3339(row.fold.metered_to_ms),
    })
}

// ---- identity ----

async fn create_account(State(state): State<AppState>, body: Bytes) -> Response {
    unwrap_response(
        async {
            let req: aex_contracts::control::CreateAccountRequest = parse_body(&body)?;
            let minted = identity::mint_secret("at");
            let row = AccountRow {
                id: identity::new_id("acc"),
                email: req.email.to_string(),
                created_ms: now_ms(),
                max_concurrent_sessions: state.default_limits.0,
                session_creates_per_hour: state.default_limits.1,
            };
            state
                .db
                .create_account(row.clone(), row.email.clone(), minted.hash)
                .await?;
            Ok(json_response(
                201,
                &json!({"account": account_json(&row), "account_token": minted.secret}),
            ))
        }
        .await,
    )
}

async fn get_account(State(state): State<AppState>, headers: HeaderMap) -> Response {
    unwrap_response(
        async {
            let a = auth_account(&state, &headers).await?;
            Ok(json_response(200, &account_json(&a)))
        }
        .await,
    )
}

async fn create_key(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    unwrap_response(
        async {
            let a = auth_account(&state, &headers).await?;
            let req: aex_contracts::control::CreateApiKeyRequest = parse_body(&body)?;
            let minted = identity::mint_secret("sk");
            let row = KeyRow {
                id: identity::new_id("key"),
                account_id: a.id,
                name: req.name.to_string(),
                prefix: minted.prefix.clone(),
                created_ms: now_ms(),
                last_used_ms: None,
                revoked_ms: None,
            };
            state.db.create_key(row.clone(), minted.hash).await?;
            Ok(json_response(
                201,
                &json!({"key": key_json(&row), "secret": minted.secret}),
            ))
        }
        .await,
    )
}

async fn list_keys(State(state): State<AppState>, headers: HeaderMap) -> Response {
    unwrap_response(
        async {
            let a = auth_account(&state, &headers).await?;
            let keys: Vec<Value> = state
                .db
                .list_keys(a.id)
                .await?
                .iter()
                .map(key_json)
                .collect();
            Ok(json_response(200, &json!({"object": "list", "data": keys})))
        }
        .await,
    )
}

async fn revoke_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key_id): Path<String>,
) -> Response {
    unwrap_response(
        async {
            let a = auth_account(&state, &headers).await?;
            if state.db.revoke_key(a.id, key_id, now_ms()).await? {
                Ok(Response::builder()
                    .status(204)
                    .body(Body::empty())
                    .expect("static response"))
            } else {
                Err(Error::NotFound)
            }
        }
        .await,
    )
}

// ---- billing ----

async fn get_balance(State(state): State<AppState>, headers: HeaderMap) -> Response {
    unwrap_response(
        async {
            let a = auth_account(&state, &headers).await?;
            sweep_account(&state.db, &state.brain, &state.card, &a.id).await?;
            let balance = state.db.balance(a.id).await?;
            Ok(json_response(
                200,
                &json!({
                    "object": "balance",
                    "microusd": balance,
                    "usd": usd_display(balance),
                    "metered_to": rfc3339(now_ms()),
                }),
            ))
        }
        .await,
    )
}

async fn create_topup(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    unwrap_response(
        async {
            let a = auth_account(&state, &headers).await?;
            let req: aex_contracts::control::CreateTopupRequest = parse_body(&body)?;
            if req.amount_cents < 1000 {
                return Err(Error::Invalid(
                    "minimum top-up is $10.00 (1000 cents)".into(),
                ));
            }
            if req.amount_cents > 100_000_000 {
                return Err(Error::Invalid("maximum top-up is $1,000,000.00".into()));
            }
            let id = identity::new_id("top");
            let checkout = state
                .payments
                .create_checkout(&id, req.amount_cents)
                .await?;
            let row = TopupRow {
                id: id.clone(),
                account_id: a.id.clone(),
                amount_cents: req.amount_cents,
                status: "pending".into(),
                provider: state.payments.name().into(),
                provider_ref: checkout.provider_ref,
                checkout_url: Some(checkout.url),
                created_ms: now_ms(),
                paid_ms: None,
            };
            state.db.create_topup(row.clone()).await?;
            Ok(json_response(201, &topup_json(&row)))
        }
        .await,
    )
}

/// While pending, ask the provider; on Paid, credit idempotently. Webhookless MVP: polling IS
/// the settlement path, and a future webhook endpoint drives the same `topup_paid`.
async fn refresh_topup(state: &AppState, t: TopupRow) -> Result<TopupRow> {
    if t.status != "pending" {
        return Ok(t);
    }
    match state.payments.check(&t.provider_ref).await? {
        PaymentStatus::Paid => state.db.topup_paid(t.id.clone(), now_ms()).await?,
        PaymentStatus::Expired => state.db.topup_expired(t.id.clone()).await?,
        PaymentStatus::Pending => return Ok(t),
    }
    state
        .db
        .topup(t.account_id.clone(), t.id.clone())
        .await?
        .ok_or(Error::NotFound)
}

async fn get_topup(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(topup_id): Path<String>,
) -> Response {
    unwrap_response(
        async {
            let a = auth_account(&state, &headers).await?;
            let t = state
                .db
                .topup(a.id.clone(), topup_id)
                .await?
                .ok_or(Error::NotFound)?;
            let t = refresh_topup(&state, t).await?;
            Ok(json_response(200, &topup_json(&t)))
        }
        .await,
    )
}

async fn list_topups(State(state): State<AppState>, headers: HeaderMap) -> Response {
    unwrap_response(
        async {
            let a = auth_account(&state, &headers).await?;
            let mut out = Vec::new();
            for t in state.db.list_topups(a.id.clone()).await? {
                out.push(topup_json(&refresh_topup(&state, t).await?));
            }
            Ok(json_response(200, &json!({"object": "list", "data": out})))
        }
        .await,
    )
}

async fn get_usage(State(state): State<AppState>, headers: HeaderMap) -> Response {
    unwrap_response(
        async {
            let a = auth_account(&state, &headers).await?;
            let lines = sweep_account(&state.db, &state.brain, &state.card, &a.id).await?;
            let total: i64 = lines.iter().map(|l| l.priced.total_microusd).sum();
            let balance = state.db.balance(a.id.clone()).await?;
            Ok(json_response(
                200,
                &json!({
                    "object": "usage",
                    "account_id": a.id,
                    "balance_microusd": balance,
                    "total_microusd": total,
                    "sessions": lines.iter().map(usage_line_json).collect::<Vec<_>>(),
                    "rates": state.card.to_json(),
                    "metered_to": rfc3339(now_ms()),
                }),
            ))
        }
        .await,
    )
}

async fn get_rates(State(state): State<AppState>) -> Response {
    json_response(200, &state.card.to_json())
}

// ---- the session authority proxy ----

/// Forward to the brain and hand the answer back as-is: status, content type, body — streaming
/// when the upstream streams (the events SSE).
async fn forward(
    state: &AppState,
    method: Method,
    path_and_query: &str,
    headers: &HeaderMap,
    body: Option<Bytes>,
) -> Result<Response> {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok());
    let idempotency = headers.get("Idempotency-Key").and_then(|v| v.to_str().ok());
    let method = reqwest::Method::from_bytes(method.as_str().as_bytes())
        .map_err(|_| Error::Invalid("method".into()))?;
    let upstream = state
        .brain
        .forward(method, path_and_query, content_type, idempotency, body)
        .await?;
    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = upstream
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    let streaming = content_type.starts_with("text/event-stream");
    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type);
    if streaming {
        builder = builder.header(header::CACHE_CONTROL, "no-cache");
        Ok(builder
            .body(Body::from_stream(upstream.bytes_stream()))
            .map_err(|e| Error::Internal(format!("response: {e}")))?)
    } else {
        let bytes = upstream
            .bytes()
            .await
            .map_err(|e| Error::Upstream(format!("body: {e}")))?;
        Ok(builder
            .body(Body::from(bytes))
            .map_err(|e| Error::Internal(format!("response: {e}")))?)
    }
}

fn path_and_query(uri: &Uri) -> String {
    uri.path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| uri.path().to_string())
}

/// Balance must be positive to admit new work (D4: prepaid; rated after the fact, so it may go
/// negative mid-turn — new work is what gets refused).
async fn require_positive_balance(state: &AppState, account: &AccountRow) -> Result<()> {
    let balance = state.db.balance(account.id.clone()).await?;
    if balance <= 0 {
        return Err(Error::InsufficientBalance(format!(
            "balance is {}; top up at least $10 to run sessions",
            usd_display(balance)
        )));
    }
    Ok(())
}

/// `POST /v1/sessions` (create, with admission) and `GET /v1/sessions` (this account's list).
async fn proxy_sessions_root(
    State(state): State<AppState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    unwrap_response(
        async {
            let (key, account) = auth_key(&state, &headers).await?;
            match method {
                Method::POST => {
                    sweep_account(&state.db, &state.brain, &state.card, &account.id).await?;
                    require_positive_balance(&state, &account).await?;
                    let live = state.db.live_count(account.id.clone()).await?;
                    if live >= account.max_concurrent_sessions {
                        return Err(Error::RateLimited(format!(
                            "{live} live sessions; the account limit is {}",
                            account.max_concurrent_sessions
                        )));
                    }
                    let hour_ago = now_ms() - 3_600_000;
                    let created = state.db.creates_since(account.id.clone(), hour_ago).await?;
                    if created >= account.session_creates_per_hour {
                        return Err(Error::RateLimited(format!(
                            "{created} sessions created in the last hour; the account limit is {}",
                            account.session_creates_per_hour
                        )));
                    }
                    let resp = forward(&state, method, &path_and_query(&uri), &headers, Some(body))
                        .await?;
                    if resp.status() == StatusCode::CREATED {
                        let (parts, resp_body) = resp.into_parts();
                        let bytes = axum::body::to_bytes(resp_body, 16 * 1024 * 1024)
                            .await
                            .map_err(|e| Error::Internal(format!("create body: {e}")))?;
                        let doc: Value = serde_json::from_slice(&bytes)
                            .map_err(|e| Error::Upstream(format!("create body: {e}")))?;
                        let id = doc["id"]
                            .as_str()
                            .ok_or_else(|| Error::Upstream("created session has no id".into()))?;
                        let shape = doc["hand"]["shape"].as_str().unwrap_or("1gb");
                        let mut fold = crate::rating::FoldState {
                            metered_to_ms: now_ms(),
                            ..Default::default()
                        };
                        fold.session_state = doc["state"].as_str().unwrap_or("idle").into();
                        fold.hand_state =
                            doc["hand"]["state"].as_str().unwrap_or("preparing").into();
                        state
                            .db
                            .insert_session(SessionRow {
                                id: id.to_string(),
                                account_id: account.id.clone(),
                                key_id: key.id.clone(),
                                shape: shape.to_string(),
                                created_ms: now_ms(),
                                is_final: false,
                                fold,
                            })
                            .await?;
                        Ok(Response::from_parts(parts, Body::from(bytes)))
                    } else {
                        Ok(resp)
                    }
                }
                Method::GET => {
                    let mut sessions = Vec::new();
                    for row in state.db.sessions_of(account.id.clone()).await? {
                        if row.is_final {
                            continue;
                        }
                        if let Some(doc) = state.brain.get_session(&row.id).await? {
                            sessions.push(doc);
                        }
                    }
                    Ok(json_response(
                        200,
                        &json!({"object": "list", "data": sessions, "has_more": false}),
                    ))
                }
                _ => Err(Error::Invalid(format!(
                    "{method} not supported on /v1/sessions"
                ))),
            }
        }
        .await,
    )
}

/// Every `/v1/sessions/{id}...` path: ownership, per-op admission, verbatim forwarding.
async fn proxy_session(
    State(state): State<AppState>,
    method: Method,
    uri: Uri,
    Path(rest): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    unwrap_response(
        async {
            let (_key, account) = auth_key(&state, &headers).await?;
            let session_id = rest.split('/').next().unwrap_or("").to_string();
            let sub = rest.strip_prefix(&session_id).unwrap_or("");
            // Unknown or foreign sessions are 404, never 403: existence is not revealed.
            let row = state
                .db
                .session(session_id.clone())
                .await?
                .filter(|r| r.account_id == account.id)
                .ok_or(Error::NotFound)?;

            let is_message = method == Method::POST && sub == "/messages";
            let is_delete = method == Method::DELETE && sub.is_empty();
            if is_message {
                sweep_session(&state.db, &state.brain, &state.card, row.clone()).await?;
                require_positive_balance(&state, &account).await?;
            } else if is_delete && !row.is_final {
                // Capture the tail of the journal while it still exists.
                sweep_session(&state.db, &state.brain, &state.card, row).await?;
            }
            let resp = forward(&state, method, &path_and_query(&uri), &headers, Some(body)).await?;
            if is_delete && resp.status() == StatusCode::NO_CONTENT {
                // The brain now 404s; the sweep marks the session final and stops its meters.
                if let Some(row) = state.db.session(session_id.clone()).await? {
                    sweep_session(&state.db, &state.brain, &state.card, row).await?;
                }
            }
            Ok(resp)
        }
        .await,
    )
}
