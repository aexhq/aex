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
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::Response;
use axum::routing::{any, delete, get, post};
use bytes::Bytes;
use brain_protocol::session::ExternalToolCallRequest;
use futures_util::StreamExt;
use serde_json::{Value, json};

use crate::brain::BrainClient;
use crate::identity::{self, bearer};
use crate::output::{self, PreparedMessage};
use crate::payments::{PaymentStatus, Payments, RefundAttempt, StripeWebhook, StripeWebhookAction};
use crate::rating::RateCard;
use crate::store::{AccountRow, Db, KeyRow, RefundRow, SessionRow, TopupRow, WaitlistRow};
use crate::sweep::{SweptLine, sweep_account, sweep_session};
use crate::web::{self, WebRuntime};
use crate::{Error, Result, now_ms, rfc3339, usd_display};

const MAX_PUBLIC_SSE_FRAME_BYTES: usize = 512 * 1024;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub brain: BrainClient,
    pub payments: Arc<dyn Payments>,
    pub stripe_webhook: Option<StripeWebhook>,
    pub card: RateCard,
    /// SHA-256 of the separately configured operator token. `None` disables admin routes.
    pub operator_token_hash: Option<String>,
    /// SHA-256 of the Brain-to-control executor bearer. None disables the internal route.
    pub external_executor_token_hash: Option<String>,
    /// Trusted host implementation for Aex-managed server Tools.
    pub web: WebRuntime,
    /// Defaults stamped onto new accounts: (max_concurrent_sessions, session_creates_per_hour).
    pub default_limits: (i64, i64),
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/waitlist", post(join_waitlist))
        .route("/v1/admin/waitlist", get(list_waitlist))
        .route("/v1/admin/invitations", post(create_invitation))
        .route("/v1/admin/refunds", post(create_refund))
        .route("/v1/accounts", post(create_account))
        .route("/v1/account", get(get_account))
        .route("/v1/keys", post(create_key).get(list_keys))
        .route("/v1/keys/{key_id}", delete(revoke_key))
        .route("/v1/balance", get(get_balance))
        .route("/v1/topups", post(create_topup).get(list_topups))
        .route("/v1/topups/{topup_id}", get(get_topup))
        .route("/v1/webhooks/stripe", post(stripe_webhook))
        .route("/v1/usage", get(get_usage))
        .route("/v1/rates", get(get_rates))
        .route("/v1/sessions", any(proxy_sessions_root))
        .route("/v1/sessions/{*rest}", any(proxy_session))
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .with_state(state)
}

/// Private Brain-to-Aex service surface. The binary serves this router on a separate loopback-only
/// listener; mounting it separately makes network isolation independent of bearer authentication.
pub fn internal_router(state: AppState) -> Router {
    Router::new()
        .route("/internal/v1/tools/call", post(execute_external_tool))
        .layer(DefaultBodyLimit::max(128 * 1024))
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

async fn auth_operator(state: &AppState, headers: &HeaderMap) -> Result<()> {
    let expected = state.operator_token_hash.as_ref().ok_or(Error::NotFound)?;
    let token = bearer(headers).ok_or(Error::Unauthorized)?;
    if !token.starts_with("aex_ad_") || identity::hash_secret(token) != *expected {
        return Err(Error::Unauthorized);
    }
    Ok(())
}

async fn auth_external_executor(state: &AppState, headers: &HeaderMap) -> Result<()> {
    let expected = state
        .external_executor_token_hash
        .as_ref()
        .ok_or(Error::NotFound)?;
    let actual = bearer(headers).ok_or(Error::Unauthorized)?;
    if identity::hash_secret(actual) != *expected {
        return Err(Error::Unauthorized);
    }
    Ok(())
}

async fn execute_external_tool(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    unwrap_response(
        async {
            auth_external_executor(&state, &headers).await?;
            let request: ExternalToolCallRequest = serde_json::from_slice(&body)
                .map_err(|error| Error::Invalid(format!("external tool request: {error}")))?;
            let capability = request.context.get("brain.capability").map(String::as_str);
            let response = if capability == Some(output::OUTPUT_CAPABILITY) {
                output::execute(&state.db, request).await?
            } else if capability == Some(web::SEARCH_CAPABILITY)
                || capability == Some(web::FETCH_CAPABILITY)
            {
                state.web.execute(request).await?
            } else {
                return Err(Error::Invalid(format!(
                    "unknown hosted server capability {capability:?}"
                )));
            };
            Ok(json_response(200, &response))
        }
        .await,
    )
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

fn waitlist_entry_json(row: &WaitlistRow) -> Value {
    let mut value = json!({
        "object": "waitlist_entry",
        "email": row.email,
        "status": row.status,
        "created_at": rfc3339(row.created_ms),
    });
    if let Some(ms) = row.invited_ms {
        value["invited_at"] = json!(rfc3339(ms));
    }
    if let Some(ms) = row.joined_ms {
        value["joined_at"] = json!(rfc3339(ms));
    }
    value
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

fn refund_json(r: &RefundRow) -> Value {
    let mut value = json!({
        "id": r.id,
        "object": "refund",
        "topup_id": r.topup_id,
        "amount_cents": r.amount_cents,
        "status": r.status,
        "created_at": rfc3339(r.created_ms),
        "updated_at": rfc3339(r.updated_ms),
    });
    if r.status == "failed"
        && let Some(reason) = &r.failure_reason
    {
        value["failure_reason"] = json!(reason);
    }
    value
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
        "web_search_queries": row.fold.web_search_queries,
        "compute_microusd": line.priced.compute_microusd,
        "storage_microusd": line.priced.storage_microusd,
        "web_search_microusd": line.priced.web_search_microusd,
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

fn normalized_email(email: &str) -> String {
    email.trim().to_ascii_lowercase()
}

async fn join_waitlist(State(state): State<AppState>, body: Bytes) -> Response {
    unwrap_response(
        async {
            let req: aex_contracts::control::JoinWaitlistRequest = parse_body(&body)?;
            let email = normalized_email(req.email.as_str());
            let received_ms = now_ms();
            state.db.join_waitlist(email.clone(), received_ms).await?;
            Ok(json_response(
                202,
                &json!({
                    "object": "waitlist_submission",
                    "email": email,
                    "status": "received",
                    "received_at": rfc3339(received_ms),
                }),
            ))
        }
        .await,
    )
}

async fn list_waitlist(State(state): State<AppState>, headers: HeaderMap) -> Response {
    unwrap_response(
        async {
            auth_operator(&state, &headers).await?;
            let data = state
                .db
                .list_waitlist()
                .await?
                .iter()
                .map(waitlist_entry_json)
                .collect::<Vec<_>>();
            Ok(json_response(200, &json!({"object": "list", "data": data})))
        }
        .await,
    )
}

async fn create_invitation(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    unwrap_response(
        async {
            auth_operator(&state, &headers).await?;
            let req: aex_contracts::control::CreateInvitationRequest = parse_body(&body)?;
            let email = normalized_email(req.email.as_str());
            let minted = identity::mint_secret("iv");
            let invited_ms = now_ms();
            state
                .db
                .invite_waitlist(email.clone(), minted.hash, invited_ms)
                .await?
                .ok_or(Error::NotFound)?;
            Ok(json_response(
                201,
                &json!({
                    "object": "invitation",
                    "email": email,
                    "invite_token": minted.secret,
                    "invited_at": rfc3339(invited_ms),
                }),
            ))
        }
        .await,
    )
}

fn refund_idempotency_key(headers: &HeaderMap) -> Result<String> {
    let key = headers
        .get("Idempotency-Key")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| Error::Invalid("missing Idempotency-Key header".into()))?;
    if !(8..=128).contains(&key.len())
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
    {
        return Err(Error::Invalid(
            "Idempotency-Key must be 8-128 letters, digits, '.', '_', ':', or '-'".into(),
        ));
    }
    Ok(key.to_owned())
}

/// The operator support path for the public unused-credit promise. The ledger reservation is
/// committed before Stripe is called. A provider/network error leaves it pending and unspendable;
/// retrying the same request reconciles by durable Aex/Stripe metadata before creating anything.
async fn create_refund(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    unwrap_response(
        async {
            auth_operator(&state, &headers).await?;
            let request_key = refund_idempotency_key(&headers)?;
            let req: aex_contracts::control::CreateRefundRequest = parse_body(&body)?;
            let amount_cents = i64::try_from(req.amount_cents.get())
                .map_err(|_| Error::Invalid("refund amount is too large".into()))?;
            let topup_id: String = req.topup_id.into();
            if amount_cents > 100_000 {
                return Err(Error::Invalid(
                    "refund amount must be 1 to 100,000 cents".into(),
                ));
            }

            let existing = state.db.refund_by_request_key(request_key.clone()).await?;
            let mut refund = if let Some(existing) = existing {
                if existing.topup_id != topup_id || existing.amount_cents != amount_cents {
                    return Err(Error::Conflict(
                        "Idempotency-Key was already used with different refund fields".into(),
                    ));
                }
                existing
            } else {
                let topup = state
                    .db
                    .operator_topup(topup_id.clone())
                    .await?
                    .ok_or(Error::NotFound)?;
                if topup.status != "paid" {
                    return Err(Error::Conflict("only a paid top-up can be refunded".into()));
                }
                if topup.provider != state.payments.name() {
                    return Err(Error::Conflict(
                        "top-up payment provider is not active".into(),
                    ));
                }
                let lines =
                    sweep_account(&state.db, &state.brain, &state.card, &topup.account_id).await?;
                if lines
                    .iter()
                    .any(|line| line.row.fold.turn_open_ms.is_some())
                {
                    return Err(Error::Conflict(
                        "wait for running session turns to finish before refunding credit".into(),
                    ));
                }
                let created_ms = now_ms();
                state
                    .db
                    .begin_refund(
                        RefundRow {
                            id: identity::new_id("rfd"),
                            request_key,
                            topup_id: topup_id.clone(),
                            account_id: String::new(),
                            amount_cents,
                            status: "pending".into(),
                            provider_ref: None,
                            failure_reason: None,
                            created_ms,
                            updated_ms: created_ms,
                        },
                        state.payments.name().into(),
                    )
                    .await?
            };

            if refund.status != "pending" {
                return Ok(json_response(200, &refund_json(&refund)));
            }
            let topup = state
                .db
                .operator_topup(refund.topup_id.clone())
                .await?
                .ok_or(Error::NotFound)?;
            if topup.provider != state.payments.name() {
                return Err(Error::Conflict(
                    "top-up payment provider is not active".into(),
                ));
            }
            refund = match state
                .payments
                .refund(
                    &topup.id,
                    &topup.provider_ref,
                    &refund.id,
                    refund.amount_cents,
                )
                .await?
            {
                RefundAttempt::Pending { provider_ref } => {
                    state
                        .db
                        .refund_pending(refund.id, provider_ref, now_ms())
                        .await?
                }
                RefundAttempt::Succeeded { provider_ref } => {
                    state
                        .db
                        .refund_succeeded(refund.id, provider_ref, now_ms())
                        .await?
                }
                RefundAttempt::Failed {
                    provider_ref,
                    reason,
                } => {
                    state
                        .db
                        .refund_failed(refund.id, provider_ref, reason, now_ms())
                        .await?
                }
            };
            Ok(json_response(200, &refund_json(&refund)))
        }
        .await,
    )
}

async fn create_account(State(state): State<AppState>, body: Bytes) -> Response {
    unwrap_response(
        async {
            let req: aex_contracts::control::CreateAccountRequest = parse_body(&body)?;
            let minted = identity::mint_secret("at");
            let email = normalized_email(req.email.as_str());
            let invite_hash = identity::hash_secret(req.invite_token.as_str());
            let row = AccountRow {
                id: identity::new_id("acc"),
                email: email.clone(),
                created_ms: now_ms(),
                max_concurrent_sessions: state.default_limits.0,
                session_creates_per_hour: state.default_limits.1,
            };
            state
                .db
                .create_invited_account(
                    row.clone(),
                    email,
                    minted.hash,
                    invite_hash,
                    row.created_ms,
                )
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
            if req.amount_cents > 100_000 {
                return Err(Error::Invalid("maximum top-up is $1,000.00".into()));
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

/// While pending, ask the provider; on Paid, credit idempotently. Polling remains the
/// correctness/recovery path if webhook delivery is late or unavailable.
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

async fn stripe_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    unwrap_response(
        async {
            let webhook = state.stripe_webhook.as_ref().ok_or(Error::NotFound)?;
            let signature = headers
                .get("stripe-signature")
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| Error::Invalid("missing stripe webhook signature".into()))?;
            let now_seconds = chrono::Utc::now().timestamp();
            match webhook.verify(signature, &body, now_seconds)? {
                Some(StripeWebhookAction::Paid {
                    topup_id,
                    provider_ref,
                    amount_cents,
                }) => {
                    if !state
                        .db
                        .stripe_topup_paid(topup_id, provider_ref, amount_cents, now_ms())
                        .await?
                    {
                        tracing::warn!("ignored unmatched Stripe paid event");
                    }
                }
                Some(StripeWebhookAction::Expired {
                    topup_id,
                    provider_ref,
                }) => {
                    if !state
                        .db
                        .stripe_topup_expired(topup_id, provider_ref)
                        .await?
                    {
                        tracing::warn!("ignored unmatched Stripe expired event");
                    }
                }
                None => {}
            }
            Ok(json_response(200, &json!({"received": true})))
        }
        .await,
    )
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
            .body(Body::from_stream(filter_public_events(upstream)))
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

/// Keep Aex's reserved protocol call private while preserving Brain sequence numbers and every
/// ordinary event byte-for-byte. The successful validated value remains on `turn.completed`.
fn filter_public_events(
    upstream: reqwest::Response,
) -> impl futures_util::Stream<Item = std::io::Result<Bytes>> {
    let stream = Box::pin(upstream.bytes_stream());
    futures_util::stream::try_unfold(
        (stream, Vec::<u8>::new(), false),
        |(mut stream, mut buffer, mut ended)| async move {
            loop {
                if let Some((index, delimiter_bytes)) = sse_frame_end(&buffer) {
                    if index.saturating_add(delimiter_bytes) > MAX_PUBLIC_SSE_FRAME_BYTES {
                        return Err(std::io::Error::other(format!(
                            "Brain SSE frame exceeds {MAX_PUBLIC_SSE_FRAME_BYTES} bytes"
                        )));
                    }
                    let frame: Vec<u8> = buffer.drain(..index + delimiter_bytes).collect();
                    if suppress_public_event(&frame) {
                        continue;
                    }
                    return Ok(Some((Bytes::from(frame), (stream, buffer, ended))));
                }
                if ended {
                    if buffer.is_empty() {
                        return Ok(None);
                    }
                    if buffer.len() > MAX_PUBLIC_SSE_FRAME_BYTES {
                        return Err(std::io::Error::other(format!(
                            "Brain SSE frame exceeds {MAX_PUBLIC_SSE_FRAME_BYTES} bytes"
                        )));
                    }
                    let frame = std::mem::take(&mut buffer);
                    if suppress_public_event(&frame) {
                        return Ok(None);
                    }
                    return Ok(Some((Bytes::from(frame), (stream, buffer, ended))));
                }
                match stream.next().await {
                    Some(Ok(chunk)) => {
                        buffer.extend_from_slice(&chunk);
                        if sse_frame_end(&buffer).is_none()
                            && buffer.len() > MAX_PUBLIC_SSE_FRAME_BYTES
                        {
                            return Err(std::io::Error::other(format!(
                                "Brain SSE frame exceeds {MAX_PUBLIC_SSE_FRAME_BYTES} bytes"
                            )));
                        }
                    }
                    Some(Err(error)) => {
                        return Err(std::io::Error::other(format!("Brain SSE stream: {error}")));
                    }
                    None => ended = true,
                }
            }
        },
    )
}

fn sse_frame_end(buffer: &[u8]) -> Option<(usize, usize)> {
    let lf = buffer.windows(2).position(|window| window == b"\n\n");
    let crlf = buffer.windows(4).position(|window| window == b"\r\n\r\n");
    match (lf, crlf) {
        (Some(left), Some(right)) if left <= right => Some((left, 2)),
        (Some(_), Some(right)) => Some((right, 4)),
        (Some(index), None) => Some((index, 2)),
        (None, Some(index)) => Some((index, 4)),
        (None, None) => None,
    }
}

fn suppress_public_event(frame: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(frame) else {
        return false;
    };
    let data = text
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(|line| line.strip_prefix(' ').unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n");
    let Ok(event) = serde_json::from_str::<Value>(&data) else {
        return false;
    };
    matches!(event["type"].as_str(), Some("tool.call" | "tool.result"))
        && event["name"].as_str() == Some(output::OUTPUT_TOOL_NAME)
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
                    let body = output::inject_output_tool(&body)?;
                    let resp = forward(
                        &state,
                        method,
                        &path_and_query(&uri),
                        &headers,
                        Some(Bytes::from(body)),
                    )
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
                                output_capable: true,
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
            let output_capable = row.output_capable;

            let is_message = method == Method::POST && sub == "/messages";
            let is_work = is_message;
            let is_delete = method == Method::DELETE && sub.is_empty();
            if is_work {
                sweep_session(&state.db, &state.brain, &state.card, row.clone()).await?;
                require_positive_balance(&state, &account).await?;
            } else if is_delete && !row.is_final {
                // Capture the tail of the journal while it still exists.
                sweep_session(&state.db, &state.brain, &state.card, row).await?;
            }
            let resp = if is_message {
                let idempotency = headers
                    .get("Idempotency-Key")
                    .and_then(|value| value.to_str().ok());
                match output::prepare_message(
                    &state.db,
                    &session_id,
                    output_capable,
                    &body,
                    idempotency,
                )
                .await?
                {
                    PreparedMessage::Plain(body) => {
                        forward(
                            &state,
                            method.clone(),
                            &path_and_query(&uri),
                            &headers,
                            Some(Bytes::from(body)),
                        )
                        .await?
                    }
                    PreparedMessage::Replay { accepted_json } => {
                        let accepted: Value =
                            serde_json::from_str(&accepted_json).map_err(|error| {
                                Error::Internal(format!("stored message acceptance: {error}"))
                            })?;
                        return Ok(json_response(202, &accepted));
                    }
                    PreparedMessage::New {
                        body,
                        output_id,
                        schema_hash,
                    } => {
                        let response = match forward(
                            &state,
                            method.clone(),
                            &path_and_query(&uri),
                            &headers,
                            Some(Bytes::from(body)),
                        )
                        .await
                        {
                            Ok(response) => response,
                            Err(error) => {
                                state.db.abandon_output(output_id).await?;
                                return Err(error);
                            }
                        };
                        if response.status() != StatusCode::ACCEPTED {
                            state.db.abandon_output(output_id).await?;
                            return Ok(response);
                        }
                        let (parts, response_body) = response.into_parts();
                        let bytes = axum::body::to_bytes(response_body, 1024 * 1024)
                            .await
                            .map_err(|error| {
                                Error::Upstream(format!("message accepted body: {error}"))
                            })?;
                        let (turn_id, accepted_json) =
                            output::augment_accepted(&bytes, &output_id, &schema_hash)?;
                        state
                            .db
                            .accept_output(output_id, turn_id, accepted_json.clone())
                            .await?;
                        Response::from_parts(parts, Body::from(accepted_json))
                    }
                }
            } else {
                forward(&state, method, &path_and_query(&uri), &headers, Some(body)).await?
            };
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

#[cfg(test)]
mod event_filter_tests {
    use super::*;

    #[test]
    fn frame_boundaries_support_lf_and_crlf() {
        assert_eq!(sse_frame_end(b"data: {}\n\nnext"), Some((8, 2)));
        assert_eq!(sse_frame_end(b"data: {}\r\n\r\nnext"), Some((8, 4)));
        assert_eq!(sse_frame_end(b"data: {}"), None);
    }

    #[test]
    fn only_reserved_protocol_call_and_result_events_are_hidden() {
        let output_call = format!(
            "event: tool.call\ndata: {{\"type\":\"tool.call\",\"name\":\"{}\",\"input\":{{\"secret\":true}}}}\n\n",
            output::OUTPUT_TOOL_NAME
        );
        assert!(suppress_public_event(output_call.as_bytes()));
        assert!(suppress_public_event(
            format!(
                "data: {{\"type\":\"tool.result\",\"name\":\"{}\"}}\r\n\r\n",
                output::OUTPUT_TOOL_NAME
            )
            .as_bytes()
        ));
        assert!(!suppress_public_event(
            b"data: {\"type\":\"tool.call\",\"name\":\"web_search\"}\n\n"
        ));
        assert!(!suppress_public_event(
            format!(
                "data: {{\"type\":\"turn.completed\",\"result\":{{\"name\":\"{}\"}}}}\n\n",
                output::OUTPUT_TOOL_NAME
            )
            .as_bytes()
        ));
    }
}
