//! The HTTP surface: the control API (`contracts/control/v1`) plus every `session/v1` path,
//! served verbatim as an authorizing, admitting, metering proxy in front of the brain.
//!
//! Auth: the account token (`aex_at_`) for identity/billing endpoints, an API key (`aex_sk_`)
//! for session paths. Admission on session work: prepaid balance must be positive (402
//! `insufficient_balance`), the per-account concurrency and create-rate caps hold (429
//! `rate_limited`) — codes the session `ApiErrorCode` already defines, so the proxied surface
//! keeps its own error envelope. Responses of the control endpoints are built as JSON and
//! pinned to the schema by the e2e test; the generated Rust types serve the SDK side.

use std::collections::HashSet;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::Response;
use axum::routing::{any, delete, get, post};
use brain_protocol::session::ExternalToolCallRequest;
use bytes::Bytes;
use futures_util::StreamExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::admission::{
    Admission, AdmissionDecision, SessionCreateDecision, SessionCreateReservation,
};
use crate::brain::BrainClient;
use crate::customer_hand::{CustomerHandGateway, GatewayRoute};
use crate::identity::{self, bearer};
use crate::output::{self, PreparedMessage};
use crate::payments::{PaymentStatus, Payments, RefundAttempt, StripeWebhook, StripeWebhookAction};
use crate::rating::RateCard;
use crate::store::{
    AccountRow, CreditGrantRow, Db, DeletionRow, InvitedJoin, KeyRow, RefundRow, SessionRow,
    TopupRow, WaitlistRow,
};
use crate::sweep::{SweptLine, sweep_account, sweep_account_incremental};
use crate::web::{self, WebRuntime};
use crate::{Error, Result, StorageLimits, now_ms, rfc3339, usd_display};

// The neutral ceiling is the JSON data payload; a small separate allowance covers SSE fields.
const MAX_PUBLIC_SSE_FRAME_BYTES: usize = brain_protocol::MAX_PUBLIC_EVENT_BYTES + 4 * 1024;
const MAX_CUSTOMER_HAND_GRANT_BYTES: usize = 16 * 1024;
const MAX_CUSTOMER_HAND_FRAME_BYTES: usize = brain_protocol::MAX_CUSTOMER_WS_FRAME_BYTES;
const MAX_CUSTOMER_HAND_OBSERVATION_BYTES: usize = brain_protocol::MAX_CUSTOMER_OBSERVATION_BYTES;
// Ordinary inline session operations never carry compiled Tool bundles. A 1 MiB file expands to
// ~1.34 MiB in base64; 2 MiB leaves JSON overhead without giving every route create-sized memory.
const MAX_INLINE_SESSION_REQUEST_BYTES: usize = 2 * 1024 * 1024;

fn message_too_large() -> Error {
    Error::PayloadTooLarge(format!(
        "message request exceeds the {}-byte journal ceiling",
        brain_protocol::MAX_MESSAGE_REQUEST_BYTES
    ))
}

async fn bounded_message_body(body: Body) -> Result<Bytes> {
    axum::body::to_bytes(body, brain_protocol::MAX_MESSAGE_REQUEST_BYTES)
        .await
        .map_err(|_| message_too_large())
}

async fn bounded_create_body(body: Body) -> Result<Bytes> {
    axum::body::to_bytes(body, brain_protocol::MAX_CREATE_SESSION_REQUEST_BYTES)
        .await
        .map_err(|_| {
            Error::PayloadTooLarge(format!(
                "create-session request exceeds {} bytes",
                brain_protocol::MAX_CREATE_SESSION_REQUEST_BYTES
            ))
        })
}

async fn bounded_inline_session_body(body: Body) -> Result<Bytes> {
    axum::body::to_bytes(body, MAX_INLINE_SESSION_REQUEST_BYTES)
        .await
        .map_err(|_| {
            Error::PayloadTooLarge(format!(
                "session request exceeds the {MAX_INLINE_SESSION_REQUEST_BYTES}-byte inline ceiling"
            ))
        })
}

async fn bounded_customer_hand_body(body: Body, limit: usize, label: &str) -> Result<Bytes> {
    axum::body::to_bytes(body, limit).await.map_err(|_| {
        Error::PayloadTooLarge(format!(
            "customer-Hand {label} exceeds the {limit}-byte ceiling"
        ))
    })
}

async fn bounded_external_tool_body(body: Body) -> Result<Bytes> {
    axum::body::to_bytes(body, brain_protocol::MAX_EXTERNAL_TOOL_REQUEST_BYTES)
        .await
        .map_err(|_| {
            Error::PayloadTooLarge(format!(
                "external Tool request exceeds the {}-byte wire ceiling",
                brain_protocol::MAX_EXTERNAL_TOOL_REQUEST_BYTES
            ))
        })
}

fn rejects_declared_get_body(method: &Method, headers: &HeaderMap) -> bool {
    *method == Method::GET
        && headers
            .get(header::CONTENT_LENGTH)
            .is_some_and(|value| value.as_bytes() != b"0")
}

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
    /// Authenticates API Gateway WebSocket integration calls; None disables hosted customer Hands.
    pub customer_hand_gateway: Option<CustomerHandGateway>,
    /// Trusted host implementation for Aex-managed server Tools.
    pub web: WebRuntime,
    /// Fast prepaid admission and bounded in-memory unbilled reservations.
    pub admission: Admission,
    /// Bounds simultaneous create-sized buffers after authentication (24 MiB each).
    pub create_body_slots: Arc<tokio::sync::Semaphore>,
    /// Bounds simultaneous prompt/message and customer-Hand buffers after authentication
    /// (at most 192 KiB each).
    pub message_body_slots: Arc<tokio::sync::Semaphore>,
    /// Bounds other buffered session requests after authentication (2 MiB each).
    pub inline_session_body_slots: Arc<tokio::sync::Semaphore>,
    /// Defaults stamped onto new accounts: (max concurrent root sessions, root creates/hour).
    pub default_limits: (i64, i64),
    /// Account-wide roots retained in Brain, including ended/failed roots and ambiguous creates.
    /// The slot is released only after confirmed physical deletion.
    pub max_retained_root_sessions: i64,
    /// Defense-in-depth product ceilings; Brain owns the durable concurrent reservation.
    pub storage_limits: StorageLimits,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/waitlist", post(join_waitlist))
        .route("/v1/admin/waitlist", get(list_waitlist))
        .route("/v1/admin/invitations", post(create_invitation))
        .route("/v1/admin/credit-grants", post(create_credit_grant))
        .route("/v1/admin/refunds", post(create_refund))
        .route("/v1/accounts", post(create_account))
        .route("/v1/account", get(get_account))
        .route("/v1/keys", post(create_key).get(list_keys))
        .route("/v1/keys/{key_id}", delete(revoke_key))
        .route("/v1/balance", get(get_balance))
        .route("/v1/topups", post(create_topup).get(list_topups))
        .route("/v1/topups/{topup_id}", get(get_topup))
        .route(
            "/v1/topups/checkout/{checkout_session_id}",
            get(get_checkout_return),
        )
        .route("/v1/webhooks/stripe", post(stripe_webhook))
        .route("/v1/usage", get(get_usage))
        .route("/v1/rates", get(get_rates))
        .route("/v1/customer-hand/grants", post(create_customer_hand_grant))
        .route("/v1/customer-hand/gateway", post(customer_hand_gateway))
        .route(
            "/v1/customer-hand/observations/{grant_id}",
            post(customer_hand_observation),
        )
        .route("/v1/sessions", any(proxy_sessions_root))
        .route("/v1/sessions/{*rest}", any(proxy_session))
        .with_state(state)
}

/// Private Brain-to-Aex service surface. The binary serves this router on a separate loopback-only
/// listener; mounting it separately makes network isolation independent of bearer authentication.
pub fn internal_router(state: AppState) -> Router {
    Router::new()
        .route("/internal/v1/tools/call", post(execute_external_tool))
        .route("/internal/v1/status", axum::routing::get(internal_status))
        .with_state(state)
}

/// Verified Stripe paid events that matched no `(id, provider_ref, amount)` topup row —
/// "customer paid, ledger silent". Exposed on the internal status route for alerting.
pub static UNMATCHED_PAID_EVENTS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

async fn internal_status() -> Response {
    json_response(
        200,
        &json!({
            "object": "internal.status",
            "unmatched_paid_events": UNMATCHED_PAID_EVENTS.load(std::sync::atomic::Ordering::Relaxed),
        }),
    )
}

// ---- plumbing ----

fn json_response(status: u16, value: &Value) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(value.to_string()))
        .expect("static response")
}

fn deletion_status_response(job: &DeletionRow) -> Response {
    let succeeded = job.phase == "succeeded";
    let mut response = json_response(
        200,
        &json!({
            "object": "session.deletion",
            "session_id": job.session_id,
            "state": if succeeded { "succeeded" } else { "deleting" },
            "requested_at_ms": job.accepted_ms,
            "updated_at_ms": job.updated_ms,
            "completed_at_ms": job.completed_ms,
        }),
    );
    if !succeeded {
        response.headers_mut().insert(
            header::RETRY_AFTER,
            "1".parse().expect("static retry header"),
        );
    }
    response
}

fn deletion_accepted_response(session_id: &str) -> Response {
    Response::builder()
        .status(StatusCode::ACCEPTED)
        .header(
            header::LOCATION,
            format!("/v1/sessions/{session_id}/deletion"),
        )
        .header(header::RETRY_AFTER, "1")
        .body(Body::empty())
        .expect("static deletion acceptance")
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
    if !token.starts_with("aex_ad_") || !identity::secret_matches_hash(token, expected) {
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
    if !identity::secret_matches_hash(actual, expected) {
        return Err(Error::Unauthorized);
    }
    Ok(())
}

async fn execute_external_tool(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    unwrap_response(
        async {
            // This private listener is also credentialed. Authenticate before polling a body so
            // a misrouted or compromised peer cannot force a half-megabyte allocation per request.
            auth_external_executor(&state, &headers).await?;
            let body = bounded_external_tool_body(body).await?;
            let request: ExternalToolCallRequest = serde_json::from_slice(&body)
                .map_err(|error| Error::Invalid(format!("external tool request: {error}")))?;
            if !brain_protocol::contract::external_tool_request_wire_fits(&request) {
                return Err(Error::PayloadTooLarge(
                    "external Tool request exceeds Brain's canonical wire ceiling".into(),
                ));
            }
            let capability = request.context.get("brain.capability").map(String::as_str);
            let response = if capability == Some(output::OUTPUT_CAPABILITY) {
                output::execute(&state.db, request).await?
            } else if capability == Some(web::SEARCH_CAPABILITY)
                || capability == Some(web::FETCH_CAPABILITY)
            {
                let request_hash = hex::encode(Sha256::digest(
                    serde_jcs::to_vec(&request).map_err(|error| {
                        Error::Internal(format!("hosted Tool request canonicalization: {error}"))
                    })?,
                ));
                let session_id = request.session_id.to_string();
                let call_id = request.call_id.to_string();
                if let Some(cached) = state
                    .db
                    .hosted_tool_response(session_id.clone(), call_id.clone(), request_hash.clone())
                    .await?
                {
                    serde_json::from_str(&cached).map_err(|error| {
                        Error::Internal(format!("cached hosted Tool response: {error}"))
                    })?
                } else {
                    let response = state.web.execute(request).await?;
                    let stored = state
                        .db
                        .record_hosted_tool_response(
                            session_id,
                            call_id,
                            request_hash,
                            response.to_string(),
                            now_ms(),
                        )
                        .await?;
                    serde_json::from_str(&stored).map_err(|error| {
                        Error::Internal(format!("stored hosted Tool response: {error}"))
                    })?
                }
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

fn credit_grant_json(grant: &CreditGrantRow) -> Value {
    json!({
        "id": grant.id,
        "object": "credit_grant",
        "account_id": grant.account_id,
        "email": grant.email,
        "amount_cents": grant.amount_cents,
        "reason": grant.reason,
        "created_at": rfc3339(grant.created_ms),
    })
}

fn usage_line_json(line: &SweptLine) -> Result<Value> {
    let row: &SessionRow = &line.row;
    let exact_unsigned = |name: &str, value: i64| {
        if value < 0 {
            Err(Error::Internal(format!(
                "cannot expose negative {name} in an unsigned usage field"
            )))
        } else {
            Ok(value.to_string())
        }
    };
    if !(0..=134_217_728).contains(&row.fold.web_search_queries) {
        return Err(Error::Internal(
            "web_search_queries exceeds the public journal-derived ceiling".into(),
        ));
    }
    let published = u64::try_from(row.fold.session_storage_bytes)
        .ok()
        .filter(|value| *value <= crate::MAX_HOSTED_SESSION_STORAGE_BYTES)
        .ok_or_else(|| Error::Internal("invalid published session-storage gauge".into()))?;
    let reserved = u64::try_from(row.fold.upload_reserved_bytes)
        .ok()
        .filter(|value| *value <= crate::MAX_HOSTED_SESSION_STORAGE_BYTES)
        .ok_or_else(|| Error::Internal("invalid upload-reserved storage gauge".into()))?;
    if published
        .checked_add(reserved)
        .is_none_or(|value| value > crate::MAX_HOSTED_SESSION_STORAGE_BYTES)
    {
        return Err(Error::Internal(
            "session storage gauges exceed the public hosted ceiling".into(),
        ));
    }
    Ok(json!({
        "session_id": row.id,
        "shape": row.shape,
        "state": row.fold.session_state,
        "running_ms": exact_unsigned("running_ms", line.priced.running_ms)?,
        "session_storage_byte_milliseconds": line
            .priced
            .session_storage_byte_milliseconds
            .to_string(),
        "web_search_queries": row.fold.web_search_queries,
        "compute_microusd": line.priced.compute_microusd.to_string(),
        "storage_microusd": line.priced.storage_microusd.to_string(),
        "web_search_microusd": line.priced.web_search_microusd.to_string(),
        "total_microusd": line.priced.total_microusd.to_string(),
        "storage": {
            "session_storage_bytes": published,
            "upload_reserved_bytes": reserved,
        },
        "metered_to": rfc3339(row.fold.metered_to_ms),
    }))
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
            let invited = state
                .db
                .invite_waitlist(email.clone(), minted.hash.clone(), invited_ms)
                .await?;
            if invited.is_none() {
                // Joined rows refuse a plain re-invite; the escape hatch below reopens the
                // invite only when the joined account was never used (lost-token recovery).
                state
                    .db
                    .reinvite_lost_account(email.clone(), minted.hash, invited_ms)
                    .await?
                    .ok_or(Error::NotFound)?;
            }
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

fn idempotency_key(headers: &HeaderMap) -> Result<String> {
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

/// Grant non-payment-backed service credit to an existing account. This is intentionally an
/// operator-only, append-only ledger operation rather than a synthetic top-up: payment history
/// and refund eligibility continue to describe only money that actually moved through Stripe.
async fn create_credit_grant(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    unwrap_response(
        async {
            auth_operator(&state, &headers).await?;
            let request_key = idempotency_key(&headers)?;
            let req: aex_contracts::control::CreateCreditGrantRequest = parse_body(&body)?;
            let amount_cents = i64::try_from(req.amount_cents.get())
                .map_err(|_| Error::Invalid("credit grant amount is too large".into()))?;
            if amount_cents > 100_000 {
                return Err(Error::Invalid(
                    "credit grant amount must be 1 to 100,000 cents".into(),
                ));
            }
            let reason = String::from(req.reason).trim().to_owned();
            if reason.is_empty() {
                return Err(Error::Invalid("credit grant reason cannot be blank".into()));
            }

            let (grant, created) = state
                .db
                .grant_credit(CreditGrantRow {
                    id: identity::new_id("grt"),
                    request_key,
                    account_id: String::new(),
                    email: normalized_email(req.email.as_str()),
                    amount_cents,
                    reason,
                    created_ms: now_ms(),
                })
                .await?;
            state.admission.invalidate_balance(&grant.account_id);
            Ok(json_response(
                if created { 201 } else { 200 },
                &credit_grant_json(&grant),
            ))
        }
        .await,
    )
}

/// The operator support path for the public unused-credit promise. The ledger reservation is
/// committed before Stripe is called. A provider/network error leaves it pending and unspendable;
/// retrying the same request reconciles by durable Aex/Stripe metadata before creating anything.
async fn create_refund(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    unwrap_response(
        async {
            auth_operator(&state, &headers).await?;
            let request_key = idempotency_key(&headers)?;
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
                let _reconciliation = state.admission.begin_reconciliation(&topup.account_id);
                let lines =
                    sweep_account(&state.db, &state.brain, &state.card, &topup.account_id).await?;
                state
                    .admission
                    .invalidate_live_root_sessions(&topup.account_id);
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

            state.admission.invalidate_balance(&refund.account_id);

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
            state.admission.invalidate_balance(&refund.account_id);
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
            match state
                .db
                .create_invited_account(
                    row.clone(),
                    email,
                    minted.hash,
                    invite_hash,
                    row.created_ms,
                )
                .await?
            {
                InvitedJoin::Created => Ok(json_response(
                    201,
                    &json!({"account": account_json(&row), "account_token": minted.secret}),
                )),
                // A lost-response retry with the same invite: the existing account with a
                // freshly rotated token (the one from the lost response is dead).
                InvitedJoin::RotatedExisting(existing) => Ok(json_response(
                    200,
                    &json!({"account": account_json(&existing), "account_token": minted.secret}),
                )),
            }
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
            let reconciliation = state.admission.begin_reconciliation(&a.id);
            sweep_account(&state.db, &state.brain, &state.card, &a.id).await?;
            state.admission.invalidate_live_root_sessions(&a.id);
            let balance = state.db.balance(a.id.clone()).await?;
            if let Some(fence) = reconciliation.as_ref() {
                state
                    .admission
                    .mark_reconciled(&a.id, balance, now_ms(), fence.generation());
            }
            Ok(json_response(
                200,
                &json!({
                    "object": "balance",
                    "microusd": balance.to_string(),
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
            // Money creation is idempotent end to end: the required Idempotency-Key keys
            // the row (a retry after a lost response replays it), and the Stripe checkout
            // create is keyed on the topup id so no second live payment link is minted.
            let request_key = idempotency_key(&headers)?;
            if let Some(existing) = state
                .db
                .topup_by_request_key(a.id.clone(), request_key.clone())
                .await?
            {
                if existing.amount_cents != req.amount_cents {
                    return Err(Error::Conflict(
                        "this Idempotency-Key was already used with a different amount".into(),
                    ));
                }
                return Ok(json_response(200, &topup_json(&existing)));
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
            state.db.create_topup(row.clone(), request_key).await?;
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
        PaymentStatus::Paid => {
            state.db.topup_paid(t.id.clone(), now_ms()).await?;
            state.admission.invalidate_balance(&t.account_id);
        }
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
                        .stripe_topup_paid(topup_id.clone(), provider_ref, amount_cents, now_ms())
                        .await?
                    {
                        // Money moved at Stripe with no ledger credit: ack (poll remains the
                        // recovery path) but count and log at ERROR so alerting cannot miss
                        // it — a warn line was the only signal before the 2026-08-22 audit.
                        UNMATCHED_PAID_EVENTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        tracing::error!(
                            event = "stripe_paid_event_unmatched",
                            total =
                                UNMATCHED_PAID_EVENTS.load(std::sync::atomic::Ordering::Relaxed),
                            "verified Stripe paid event matched no topup row"
                        );
                    } else if let Some(topup) = state.db.operator_topup(topup_id).await? {
                        state.admission.invalidate_balance(&topup.account_id);
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

/// Return-page verification is intentionally credential-free: Stripe places the unguessable
/// Checkout Session id in the success URL. Only the payment state is disclosed, while polling
/// still drives the same idempotent ledger-credit recovery path as the authenticated endpoint.
async fn get_checkout_return(
    State(state): State<AppState>,
    Path(checkout_session_id): Path<String>,
) -> Response {
    unwrap_response(
        async {
            if !checkout_session_id.starts_with("cs_")
                || checkout_session_id.len() > 128
                || !checkout_session_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            {
                return Err(Error::NotFound);
            }
            let topup = state
                .db
                .stripe_topup_by_provider_ref(checkout_session_id)
                .await?
                .ok_or(Error::NotFound)?;
            let topup = refresh_topup(&state, topup).await?;
            let status = match topup.status.as_str() {
                "paid" => StatusCode::OK,
                "pending" => StatusCode::ACCEPTED,
                "expired" => StatusCode::GONE,
                _ => return Err(Error::Internal("invalid top-up status".into())),
            };
            Ok(Response::builder()
                .status(status)
                .header(header::CACHE_CONTROL, "no-store")
                .body(Body::empty())
                .expect("static response"))
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
            let reconciliation = state.admission.begin_reconciliation(&a.id);
            let lines = sweep_account(&state.db, &state.brain, &state.card, &a.id).await?;
            state.admission.invalidate_live_root_sessions(&a.id);
            let total = lines.iter().try_fold(0i64, |sum, line| {
                sum.checked_add(line.priced.total_microusd).ok_or_else(|| {
                    Error::Internal("account usage exceeds the exact ledger range".into())
                })
            })?;
            let balance = state.db.balance(a.id.clone()).await?;
            if let Some(fence) = reconciliation.as_ref() {
                state
                    .admission
                    .mark_reconciled(&a.id, balance, now_ms(), fence.generation());
            }
            Ok(json_response(
                200,
                &json!({
                    "object": "usage",
                    "account_id": a.id,
                    "balance_microusd": balance.to_string(),
                    "total_microusd": total.to_string(),
                    "sessions": lines
                        .iter()
                        .map(usage_line_json)
                        .collect::<Result<Vec<_>>>()?,
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

// ---- hosted customer Hand ----

async fn create_customer_hand_grant(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    unwrap_response(
        async {
            let (_key, account) = auth_key(&state, &headers).await?;
            let _body_slot = try_body_slot(&state.message_body_slots, "message")?;
            let body =
                bounded_customer_hand_body(body, MAX_CUSTOMER_HAND_GRANT_BYTES, "grant request")
                    .await?;
            let content_type = headers
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok());
            let upstream = state
                .brain
                .customer_hand_grant(&account.id, content_type, body)
                .await?;
            proxy_brain_response(upstream)
        }
        .await,
    )
}

async fn customer_hand_gateway(
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    unwrap_response(
        async {
            let gateway = state
                .customer_hand_gateway
                .as_ref()
                .ok_or(Error::NotFound)?;
            let metadata = gateway.authenticate(&headers, peer)?;
            if metadata.route == GatewayRoute::Disconnect {
                // API Gateway cannot reliably carry the connect authorizer context here and a
                // disconnect has no runner proof. Treat it as advisory: Brain expires the epoch
                // by heartbeat or a confirmed Management API Gone response.
                return Ok(Response::builder()
                    .status(StatusCode::NO_CONTENT)
                    .body(Body::empty())
                    .expect("static disconnect response"));
            }
            let _body_slot = try_body_slot(&state.message_body_slots, "message")?;
            let body =
                bounded_customer_hand_body(body, MAX_CUSTOMER_HAND_FRAME_BYTES, "frame").await?;
            let content_type = headers
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok());
            let upstream = state
                .brain
                .customer_hand_gateway(&metadata, content_type, body)
                .await?;
            proxy_brain_response(upstream)
        }
        .await,
    )
}

async fn customer_hand_observation(
    State(state): State<AppState>,
    Path(grant_id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    unwrap_response(
        async {
            if grant_id.is_empty()
                || grant_id.len() > 128
                || !grant_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
            {
                return Err(Error::Invalid(
                    "invalid customer-Hand observation grant id".into(),
                ));
            }
            let grant = bearer(&headers).ok_or(Error::Unauthorized)?;
            if grant.is_empty()
                || grant.len() > 2048
                || !grant.bytes().all(|byte| byte.is_ascii_graphic())
            {
                return Err(Error::Unauthorized);
            }
            let _body_slot = try_body_slot(&state.message_body_slots, "message")?;
            let body = bounded_customer_hand_body(
                body,
                MAX_CUSTOMER_HAND_OBSERVATION_BYTES,
                "observation",
            )
            .await?;
            let content_type = headers
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok());
            let upstream = state
                .brain
                .customer_hand_observation(&grant_id, grant, content_type, body)
                .await?;
            proxy_brain_response(upstream)
        }
        .await,
    )
}

// ---- the session authority proxy ----

/// Forward to the brain and hand the answer back as-is: status, content type, body — streaming
/// when the upstream streams (the events SSE).
async fn forward(
    state: &AppState,
    tenant_id: &str,
    method: Method,
    path_and_query: &str,
    headers: &HeaderMap,
    body: Option<Body>,
) -> Result<Response> {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok());
    let idempotency = headers.get("Idempotency-Key").and_then(|v| v.to_str().ok());
    forward_with_idempotency(
        state,
        tenant_id,
        method,
        path_and_query,
        content_type,
        idempotency,
        body.map(|body| {
            reqwest::Body::wrap_stream(
                body.into_data_stream()
                    .map(|chunk| chunk.map_err(std::io::Error::other)),
            )
        }),
    )
    .await
}

async fn forward_with_idempotency(
    state: &AppState,
    tenant_id: &str,
    method: Method,
    path_and_query: &str,
    content_type: Option<&str>,
    idempotency: Option<&str>,
    body: Option<reqwest::Body>,
) -> Result<Response> {
    let method = reqwest::Method::from_bytes(method.as_str().as_bytes())
        .map_err(|_| Error::Invalid("method".into()))?;
    let upstream = state
        .brain
        .forward(
            tenant_id,
            method,
            path_and_query,
            content_type,
            idempotency,
            body,
            // No total deadline: this is the customer proxy and may carry an SSE follow
            // stream; the customer ends it. Internal bounded ops set their own deadlines.
            None,
        )
        .await?;
    proxy_brain_response(upstream)
}

fn proxy_brain_response(upstream: reqwest::Response) -> Result<Response> {
    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    // Pass Content-Type through faithfully; a response Brain sent without one is forwarded
    // without one, never relabeled as JSON.
    let content_type = upstream
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let streaming = content_type
        .as_deref()
        .is_some_and(|value| value.starts_with("text/event-stream"));
    let mut builder = Response::builder().status(status);
    if let Some(content_type) = content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    for name in [
        header::CONTENT_DISPOSITION,
        header::CONTENT_LENGTH,
        header::ETAG,
        header::LAST_MODIFIED,
        header::ACCEPT_RANGES,
        header::CONTENT_RANGE,
        header::CACHE_CONTROL,
        header::LOCATION,
        header::RETRY_AFTER,
        header::SEC_WEBSOCKET_PROTOCOL,
    ] {
        if streaming && name == header::CONTENT_LENGTH {
            continue;
        }
        if let Some(value) = upstream.headers().get(&name) {
            builder = builder.header(name, value);
        }
    }
    if let Some(value) = upstream.headers().get("x-request-id") {
        builder = builder.header("x-request-id", value);
    }
    if streaming {
        builder = builder.header(header::CACHE_CONTROL, "no-cache");
        Ok(builder
            .body(Body::from_stream(filter_public_events(upstream)))
            .map_err(|e| Error::Internal(format!("response: {e}")))?)
    } else {
        Ok(builder
            .body(Body::from_stream(
                upstream
                    .bytes_stream()
                    .map(|chunk| chunk.map_err(std::io::Error::other)),
            ))
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
        (
            stream,
            Vec::<u8>::new(),
            Option::<(Bytes, usize)>::None,
            false,
        ),
        |(mut stream, mut buffer, mut pending, mut ended)| async move {
            loop {
                if let Some((index, delimiter_bytes)) = sse_frame_end(&buffer) {
                    if index.saturating_add(delimiter_bytes) > MAX_PUBLIC_SSE_FRAME_BYTES {
                        return Err(std::io::Error::other(format!(
                            "Brain SSE frame exceeds {MAX_PUBLIC_SSE_FRAME_BYTES} bytes"
                        )));
                    }
                    let frame: Vec<u8> = buffer.drain(..index + delimiter_bytes).collect();
                    if suppress_public_event(&frame)? {
                        continue;
                    }
                    return Ok(Some((Bytes::from(frame), (stream, buffer, pending, ended))));
                }
                if let Some((chunk, offset)) = pending.take() {
                    let available = MAX_PUBLIC_SSE_FRAME_BYTES.saturating_sub(buffer.len());
                    if available == 0 {
                        return Err(std::io::Error::other(format!(
                            "Brain SSE frame exceeds {MAX_PUBLIC_SSE_FRAME_BYTES} bytes"
                        )));
                    }
                    let end = offset.saturating_add(available).min(chunk.len());
                    buffer.extend_from_slice(&chunk[offset..end]);
                    if end < chunk.len() {
                        // Retain the reqwest Bytes and offset instead of copying an arbitrarily
                        // large transport chunk into the bounded frame accumulator.
                        pending = Some((chunk, end));
                    }
                    continue;
                }
                if ended {
                    if buffer.is_empty() {
                        return Ok(None);
                    }
                    return Err(std::io::Error::other(
                        "Brain SSE stream ended in a truncated frame",
                    ));
                }
                match stream.next().await {
                    Some(Ok(chunk)) => {
                        pending = Some((chunk, 0));
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

fn suppress_public_event(frame: &[u8]) -> std::io::Result<bool> {
    let text = std::str::from_utf8(frame)
        .map_err(|error| std::io::Error::other(format!("invalid Brain SSE UTF-8: {error}")))?;
    let lines = text
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(|line| line.strip_prefix(' ').unwrap_or(line))
        .collect::<Vec<_>>();
    if lines.is_empty() {
        // Keep-alive comments contain no model-visible data and remain byte-for-byte.
        return Ok(false);
    }
    let data = lines.join("\n");
    if data.len() > brain_protocol::MAX_PUBLIC_EVENT_BYTES {
        return Err(std::io::Error::other(format!(
            "Brain SSE data exceeds {} bytes",
            brain_protocol::MAX_PUBLIC_EVENT_BYTES
        )));
    }
    let event = serde_json::from_str::<Value>(&data)
        .map_err(|error| std::io::Error::other(format!("invalid Brain SSE event: {error}")))?;
    Ok(
        matches!(event["type"].as_str(), Some("tool.call" | "tool.result"))
            && event["name"].as_str() == Some(output::OUTPUT_TOOL_NAME),
    )
}

fn create_idempotency(headers: &HeaderMap, account_id: &str) -> Result<(String, String)> {
    let raw = required_idempotency_key(headers)?;
    let namespaced = format!("aex:{account_id}:{}", crate::identity::hash_secret(raw));
    let hash = crate::identity::hash_secret(&namespaced);
    Ok((namespaced, hash))
}

fn required_idempotency_key(headers: &HeaderMap) -> Result<&str> {
    let raw = headers
        .get("Idempotency-Key")
        .ok_or_else(|| Error::Invalid("Idempotency-Key is required for this operation".into()))?
        .to_str()
        .map_err(|_| Error::Invalid("Idempotency-Key must be valid ASCII".into()))?;
    if raw.is_empty() || raw.len() > 128 {
        return Err(Error::Invalid(
            "Idempotency-Key must contain 1 to 128 bytes".into(),
        ));
    }
    Ok(raw)
}

fn requires_idempotency_key(method: &Method, sub: &str) -> bool {
    *method == Method::POST && (sub == "/messages" || is_child_message_request(method, sub))
}

fn is_child_message_request(method: &Method, sub: &str) -> bool {
    *method == Method::POST
        && (sub == "/children"
            || (sub.starts_with("/children/")
                && (sub.ends_with("/messages") || sub.ends_with("/follow-up"))))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BufferedSessionBody {
    Message,
    Inline,
}

fn buffered_session_body(method: &Method, sub: &str) -> Option<BufferedSessionBody> {
    if *method == Method::POST && (sub == "/messages" || is_child_message_request(method, sub)) {
        return Some(BufferedSessionBody::Message);
    }
    matches!(*method, Method::POST | Method::PUT | Method::PATCH)
        .then_some(BufferedSessionBody::Inline)
}

fn try_body_slot(
    slots: &Arc<tokio::sync::Semaphore>,
    label: &str,
) -> Result<tokio::sync::OwnedSemaphorePermit> {
    slots.clone().try_acquire_owned().map_err(|_| {
        Error::RateLimited(format!(
            "the process-wide {label} request-body capacity is full; retry shortly"
        ))
    })
}

fn is_definitive_create_rejection(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::BAD_REQUEST
            | StatusCode::UNAUTHORIZED
            | StatusCode::PAYMENT_REQUIRED
            | StatusCode::FORBIDDEN
            | StatusCode::NOT_FOUND
            | StatusCode::CONFLICT
            | StatusCode::PAYLOAD_TOO_LARGE
            | StatusCode::UNSUPPORTED_MEDIA_TYPE
            | StatusCode::UNPROCESSABLE_ENTITY
            | StatusCode::TOO_MANY_REQUESTS
    )
}

fn path_and_query(uri: &Uri) -> String {
    uri.path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| uri.path().to_string())
}

/// Balance must be positive to admit new work. Usage is prepaid but rated after the fact, so it may go
/// negative mid-turn — new work is what gets refused).
async fn require_positive_balance(state: &AppState, account: &AccountRow) -> Result<()> {
    for _ in 0..2 {
        if let Some(balance) = state
            .admission
            .cached_available_balance(&account.id, now_ms())
        {
            return if balance > 0 {
                Ok(())
            } else {
                Err(Error::InsufficientBalance(format!(
                    "available balance is {}; top up at least $10 to run sessions",
                    usd_display(balance)
                )))
            };
        }
        let reconciliation = state
            .admission
            .begin_reconciliation(&account.id)
            .ok_or_else(admission_cache_full)?;
        // Admission reconciles only Brain's changed tenant-index window plus local open meters.
        // It must not issue one authoritative HEAD request per historical session on a cache
        // miss; destructive deletion owns the separate strong subtree barrier.
        sweep_account_incremental(&state.db, &state.brain, &state.card, &account.id).await?;
        state.admission.invalidate_live_root_sessions(&account.id);
        let balance = state.db.balance(account.id.clone()).await?;
        if state.admission.mark_reconciled(
            &account.id,
            balance,
            now_ms(),
            reconciliation.generation(),
        ) {
            return if balance > 0 {
                Ok(())
            } else {
                Err(Error::InsufficientBalance(format!(
                    "balance is {}; top up at least $10 to run sessions",
                    usd_display(balance)
                )))
            };
        }
    }
    Err(Error::RateLimited(
        "balance admission was fenced by concurrent ledger mutations; retry the request".into(),
    ))
}

async fn require_work_admission(state: &AppState, account: &AccountRow) -> Result<()> {
    let mut decision = state.admission.reserve_cached(&account.id, now_ms());
    // A ledger mutation racing the durable read fences the cache install. Retry the rare race
    // once; continuous concurrent mutation fails closed below instead of admitting stale credit.
    for _ in 0..2 {
        if decision != AdmissionDecision::Reconcile {
            break;
        }
        let reconciliation = state
            .admission
            .begin_reconciliation(&account.id)
            .ok_or_else(admission_cache_full)?;
        sweep_account_incremental(&state.db, &state.brain, &state.card, &account.id).await?;
        state.admission.invalidate_live_root_sessions(&account.id);
        let balance = state.db.balance(account.id.clone()).await?;
        decision = state.admission.reserve_reconciled(
            &account.id,
            balance,
            now_ms(),
            reconciliation.generation(),
        );
    }
    match decision {
        AdmissionDecision::Reserved => Ok(()),
        AdmissionDecision::Insufficient => Err(Error::InsufficientBalance(
            "available balance is below the configured maximum action exposure; top up at least $10"
                .to_owned(),
        )),
        AdmissionDecision::ExposureExhausted => Err(Error::RateLimited(
            "the account's bounded unbilled work exposure is exhausted; retry after settlement"
                .into(),
        )),
        AdmissionDecision::CacheFull => Err(admission_cache_full()),
        AdmissionDecision::Reconcile => Err(Error::RateLimited(
            "admission was fenced by concurrent ledger mutations; retry the request".into(),
        )),
    }
}

async fn reserve_session_create(
    state: &AppState,
    account: &AccountRow,
    request_id: &str,
) -> Result<SessionCreateReservation> {
    let refresh_lock = state
        .admission
        .session_refresh_lock(&account.id)
        .ok_or_else(admission_cache_full)?;
    for _ in 0..4 {
        let generation = state.admission.session_generation(&account.id);
        let created = state
            .db
            .root_creates_since(account.id.clone(), now_ms().saturating_sub(3_600_000))
            .await?;
        let mut durable_pending = state
            .db
            .uncovered_session_create_intents(account.id.clone())
            .await?;
        // This new request is process-reserved but not yet in SQLite. Include its future intent
        // so the admission calculation can discount exactly itself and count every other row.
        durable_pending = durable_pending.saturating_add(1);
        let mut decision = state.admission.reserve_session_create_cached(
            &account.id,
            request_id,
            generation,
            durable_pending,
            account.max_concurrent_sessions,
            created,
            account.session_creates_per_hour,
            now_ms(),
        );
        if matches!(decision, SessionCreateDecision::Reconcile) {
            // Only cache misses/staleness serialize. The first waiter refreshes Brain's tenant
            // index; every later waiter rechecks the now-hot atomic pending/live state.
            let _refresh = refresh_lock.lock().await;
            let generation = state.admission.session_generation(&account.id);
            let created = state
                .db
                .root_creates_since(account.id.clone(), now_ms().saturating_sub(3_600_000))
                .await?;
            let mut durable_pending = state
                .db
                .uncovered_session_create_intents(account.id.clone())
                .await?;
            durable_pending = durable_pending.saturating_add(1);
            decision = state.admission.reserve_session_create_cached(
                &account.id,
                request_id,
                generation,
                durable_pending,
                account.max_concurrent_sessions,
                created,
                account.session_creates_per_hour,
                now_ms(),
            );
            if matches!(decision, SessionCreateDecision::Reconcile) {
                let brain_live_roots = state
                    .brain
                    .live_root_session_count(&account.id, account.max_concurrent_sessions)
                    .await?;
                let local_live_roots = state.db.live_root_count(account.id.clone()).await?;
                let live_roots = brain_live_roots.max(local_live_roots);
                decision = state.admission.reserve_session_create_reconciled(
                    &account.id,
                    request_id,
                    generation,
                    live_roots,
                    durable_pending,
                    account.max_concurrent_sessions,
                    created,
                    account.session_creates_per_hour,
                    now_ms(),
                );
            }
        }
        match decision {
            SessionCreateDecision::Reserved(reservation) => return Ok(reservation),
            SessionCreateDecision::ConcurrentLimit => {
                return Err(Error::RateLimited(format!(
                    "the account has reached its {} resource-bearing-or-pending root-session limit; end failed roots and wait for ended, or await session.delete(), to release capacity",
                    account.max_concurrent_sessions
                )));
            }
            SessionCreateDecision::CreateRateLimit => {
                return Err(Error::RateLimited(format!(
                    "the account has reached its {} root-session creates per hour limit",
                    account.session_creates_per_hour
                )));
            }
            SessionCreateDecision::CacheFull => return Err(admission_cache_full()),
            SessionCreateDecision::Reconcile => continue,
        }
    }
    Err(Error::RateLimited(
        "session admission was fenced by concurrent lifecycle changes; retry the request".into(),
    ))
}

fn admission_cache_full() -> Error {
    Error::RateLimited(
        "the admission cache is at its configured safe capacity; retry after active work settles"
            .into(),
    )
}

fn is_billable_work(method: &Method, sub: &str) -> bool {
    if *method != Method::POST {
        return false;
    }
    sub == "/messages"
        || sub.ends_with("/messages")
        || sub == "/children"
        || sub.ends_with("/follow-up")
        || sub == "/sandbox"
        || sub.starts_with("/sandbox/files/")
        || matches!(
            sub,
            "/storage/copy-from-sandbox" | "/storage/copy-to-sandbox"
        )
}

fn starts_storage_write(method: &Method, sub: &str) -> bool {
    *method == Method::POST && matches!(sub, "/storage/write-inline" | "/storage/uploads")
}

fn validate_storage_upload(
    body: &[u8],
    visible_and_reserved_bytes: i64,
    limits: StorageLimits,
) -> Result<()> {
    let request: Value = serde_json::from_slice(body)
        .map_err(|error| Error::Invalid(format!("storage upload request: {error}")))?;
    let bytes = request["bytes"].as_u64().ok_or_else(|| {
        Error::Invalid("storage upload bytes must be a non-negative integer".into())
    })?;
    if bytes > limits.max_object_bytes {
        return Err(Error::PayloadTooLarge(format!(
            "storage object declares {bytes} bytes; the product limit is {} bytes",
            limits.max_object_bytes
        )));
    }
    let current = u64::try_from(visible_and_reserved_bytes.max(0)).unwrap_or(u64::MAX);
    if current.saturating_add(bytes) > limits.max_session_bytes {
        return Err(Error::StorageQuota(format!(
            "storage upload would exceed the {}-byte session limit",
            limits.max_session_bytes
        )));
    }
    Ok(())
}

async fn owned_session_row(
    state: &AppState,
    key: &KeyRow,
    account: &AccountRow,
    session_id: &str,
) -> Result<SessionRow> {
    if let Some(row) = state.db.session(session_id.to_string()).await? {
        return if row.account_id == account.id {
            Ok(row)
        } else {
            Err(Error::NotFound)
        };
    }
    let document = state
        .brain
        .get_session(&account.id, session_id)
        .await?
        .ok_or(Error::NotFound)?;
    let snapshot = crate::brain::parse_session_snapshot(document)?;
    if snapshot.id != session_id {
        return Err(Error::Upstream(
            "session HEAD identity does not match the requested resource".into(),
        ));
    }
    let row = session_row_from_snapshot(snapshot, &account.id, &key.id);
    state.db.insert_session(row.clone()).await?;
    Ok(row)
}

fn session_row_from_snapshot(
    snapshot: crate::brain::BrainSessionSnapshot,
    account_id: &str,
    key_id: &str,
) -> SessionRow {
    // HEAD gauges cannot reconstruct storage that was both created and deleted before this first
    // lookup. Persist the zero-at-create origin; the next exact journal fold installs transitions.
    let fold = crate::rating::FoldState {
        storage_transition_ms: snapshot.created_ms,
        metered_to_ms: snapshot.created_ms,
        session_state: snapshot.document["state"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        ..Default::default()
    };
    SessionRow {
        id: snapshot.id,
        account_id: account_id.to_owned(),
        key_id: key_id.to_owned(),
        parent_id: snapshot.parent_id,
        root_id: snapshot.root_id,
        depth: snapshot.depth,
        shape: snapshot.shape,
        created_ms: snapshot.created_ms,
        is_final: false,
        fold,
    }
}

/// Hydrate a selected session's immutable ancestry through strongly consistent Brain HEAD reads.
/// Brain caps depth at eight, so this rare destructive path performs at most eight requests. The
/// returned selected-to-root chain is installed in the same SQLite transaction as the deletion
/// job, making an already accepted ancestor visible before overlap alias resolution.
async fn hydrate_delete_parent_chain(
    state: &AppState,
    account: &AccountRow,
    key: &KeyRow,
    selected: &SessionRow,
) -> Result<Vec<SessionRow>> {
    const MAX_PARENT_HOPS: i64 = 8;
    if !(0..=MAX_PARENT_HOPS).contains(&selected.depth) {
        return Err(Error::Internal(format!(
            "persisted session {} has invalid depth {}",
            selected.id, selected.depth
        )));
    }
    if selected.depth == 0 {
        if selected.parent_id.is_some() || selected.root_id != selected.id {
            return Err(Error::Upstream(format!(
                "session {} has a contradictory root projection",
                selected.id
            )));
        }
        return Ok(vec![selected.clone()]);
    }
    if selected.parent_id.is_none() || selected.root_id == selected.id {
        return Err(Error::Upstream(format!(
            "session {} has a contradictory child projection",
            selected.id
        )));
    }

    let mut chain = vec![selected.clone()];
    let mut seen = HashSet::from([selected.id.clone()]);
    let mut expected_parent = selected.parent_id.clone();
    let mut child_depth = selected.depth;
    for _ in 0..selected.depth {
        let parent_id = expected_parent.take().ok_or_else(|| {
            Error::Upstream(format!(
                "session {} parent chain ended before root {}",
                selected.id, selected.root_id
            ))
        })?;
        if !seen.insert(parent_id.clone()) {
            return Err(Error::Upstream(format!(
                "session {} parent chain contains a cycle",
                selected.id
            )));
        }
        let document = state
            .brain
            .get_session(&account.id, &parent_id)
            .await?
            .ok_or_else(|| {
                Error::Upstream(format!(
                    "session {} parent {} disappeared during deletion admission",
                    selected.id, parent_id
                ))
            })?;
        let snapshot = crate::brain::parse_session_snapshot(document)?;
        if snapshot.id != parent_id
            || snapshot.root_id != selected.root_id
            || snapshot.depth + 1 != child_depth
        {
            return Err(Error::Upstream(format!(
                "session {} parent {} contradicts its child projection",
                selected.id, parent_id
            )));
        }
        if snapshot.depth == 0 {
            if snapshot.id != selected.root_id || snapshot.parent_id.is_some() {
                return Err(Error::Upstream(format!(
                    "session {} parent chain does not terminate at root {}",
                    selected.id, selected.root_id
                )));
            }
        } else if snapshot.parent_id.is_none() || snapshot.id == selected.root_id {
            return Err(Error::Upstream(format!(
                "session {} parent {} has a contradictory depth",
                selected.id, parent_id
            )));
        }
        child_depth = snapshot.depth;
        expected_parent = snapshot.parent_id.clone();
        chain.push(session_row_from_snapshot(snapshot, &account.id, &key.id));
    }
    if child_depth != 0 || expected_parent.is_some() {
        return Err(Error::Upstream(format!(
            "session {} parent chain exceeded the eight-hop limit",
            selected.id
        )));
    }
    Ok(chain)
}

/// `POST /v1/sessions` (create, with admission) and `GET /v1/sessions` (this account's list).
async fn proxy_sessions_root(
    State(state): State<AppState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Body,
) -> Response {
    unwrap_response(
        async {
            let (key, account) = auth_key(&state, &headers).await?;
            if rejects_declared_get_body(&method, &headers) {
                return Err(Error::Invalid(
                    "GET /v1/sessions requires an empty body".into(),
                ));
            }
            match method {
                Method::POST => {
                    // Authentication intentionally precedes the first body poll. Compiled Tool
                    // bundles make create the one large JSON operation, but unauthenticated peers
                    // cannot make Aex allocate that request.
                    let _body_slot = try_body_slot(&state.create_body_slots, "create")?;
                    let body = bounded_create_body(body).await?;
                    // Persist the discovery target before Brain can commit the session. If this
                    // process dies after Brain accepts but before the response, the next sweeper
                    // still finds and rates the authoritative session.
                    state
                        .db
                        .enable_session_discovery(account.id.clone())
                        .await?;
                    let idempotency = create_idempotency(&headers, &account.id)?;
                    let body = output::inject_output_tool(body)?;
                    if body.len() > brain_protocol::MAX_CREATE_SESSION_REQUEST_BYTES {
                        return Err(Error::PayloadTooLarge(format!(
                            "create-session request exceeds {} bytes after official Tool sealing",
                            brain_protocol::MAX_CREATE_SESSION_REQUEST_BYTES
                        )));
                    }
                    let request_hash = hex::encode(Sha256::digest(&body));
                    let replay = match state
                        .db
                        .session_create_request(account.id.clone(), idempotency.1.clone())
                        .await?
                    {
                        Some(existing) if existing.request_hash == request_hash => true,
                        Some(_) => {
                            return Err(Error::Conflict(
                                "Idempotency-Key was reused with a different create request".into(),
                            ));
                        }
                        None => false,
                    };
                    let existing_intent = if replay {
                        false
                    } else {
                        match state
                            .db
                            .session_create_intent(account.id.clone(), idempotency.1.clone())
                            .await?
                        {
                            Some(intent) if intent.request_hash == request_hash => true,
                            Some(_) => {
                                return Err(Error::Conflict(
                                    "Idempotency-Key was reused with a different create request"
                                        .into(),
                                ));
                            }
                            None => false,
                        }
                    };
                    let mut reservation = if replay {
                        None
                    } else {
                        let reservation = if existing_intent {
                            // The first dispatch already crossed balance admission and this exact
                            // durable identity still owns its root slot. A same-key/body retry is
                            // recovery: Brain may already have committed it, so a later zero
                            // balance must not make the outcome permanently unobservable.
                            match state.admission.resume_session_create(
                                &account.id,
                                &idempotency.1,
                                now_ms(),
                            ) {
                                SessionCreateDecision::Reserved(reservation) => reservation,
                                SessionCreateDecision::CacheFull => {
                                    return Err(admission_cache_full());
                                }
                                decision => {
                                    return Err(Error::Internal(format!(
                                        "durable create recovery returned {decision:?}"
                                    )));
                                }
                            }
                        } else {
                            require_positive_balance(&state, &account).await?;
                            reserve_session_create(&state, &account, &idempotency.1).await?
                        };
                        match state
                            .db
                            .ensure_session_create_intent(
                                account.id.clone(),
                                idempotency.1.clone(),
                                request_hash.clone(),
                                now_ms(),
                                state.max_retained_root_sessions,
                            )
                            .await?
                        {
                            crate::store::SessionCreateIntent::Created
                            | crate::store::SessionCreateIntent::Existing => Some(reservation),
                            crate::store::SessionCreateIntent::Conflict => {
                                return Err(Error::Conflict(
                                    "Idempotency-Key was reused with a different create request"
                                        .into(),
                                ));
                            }
                            crate::store::SessionCreateIntent::RetainedRootLimit => {
                                return Err(Error::RateLimited(format!(
                                    "the account has reached its {} retained root-session limit; delete finished sessions with `await session.delete()` before creating another",
                                    state.max_retained_root_sessions
                                )));
                            }
                        }
                    };
                    let content_type = headers
                        .get(header::CONTENT_TYPE)
                        .and_then(|value| value.to_str().ok());
                    let resp = match forward_with_idempotency(
                        &state,
                        &account.id,
                        method,
                        &path_and_query(&uri),
                        content_type,
                        Some(idempotency.0.as_str()),
                        Some(reqwest::Body::from(body)),
                    )
                    .await
                    {
                        Ok(resp) => resp,
                        Err(error) => {
                            // The request may have reached Brain and committed before its response
                            // was lost. Keep this unique identity charged against the root limit;
                            // an identical idempotent retry may prove and convert it later.
                            state
                                .db
                                .mark_session_create_uncertain(
                                    account.id.clone(),
                                    idempotency.1.clone(),
                                    request_hash.clone(),
                                    now_ms(),
                                )
                                .await?;
                            if let Some(reservation) = reservation.take() {
                                reservation.uncertain();
                            }
                            return Err(error);
                        }
                    };
                    if resp.status() == StatusCode::CREATED {
                        // Keep both the in-memory and durable slot until the 201 projection is
                        // validated and atomically installed. If local parsing/storage fails, the
                        // create remains uncertain across restart and an identical retry recovers.
                        let created = async {
                            let (parts, resp_body) = resp.into_parts();
                            let bytes = axum::body::to_bytes(resp_body, 16 * 1024 * 1024)
                                .await
                                .map_err(|e| Error::Internal(format!("create body: {e}")))?;
                            let doc: Value = serde_json::from_slice(&bytes)
                                .map_err(|e| Error::Upstream(format!("create body: {e}")))?;
                            let snapshot = crate::brain::parse_session_snapshot(doc)?;
                            if snapshot.parent_id.is_some() || snapshot.depth != 0 {
                                return Err(Error::Upstream(
                                    "root create returned a child session".into(),
                                ));
                            }
                            let row = session_row_from_snapshot(snapshot, &account.id, &key.id);
                            state
                                .db
                                .record_created_session(
                                    row,
                                    Some((idempotency.1.clone(), request_hash.clone(), now_ms())),
                                )
                                .await?;
                            Ok::<_, Error>(Response::from_parts(parts, Body::from(bytes)))
                        }
                        .await;
                        match created {
                            Ok(response) => {
                                if let Some(reservation) = reservation.take() {
                                    reservation.commit();
                                }
                                Ok(response)
                            }
                            Err(error) => {
                                state
                                    .db
                                    .mark_session_create_uncertain(
                                        account.id.clone(),
                                        idempotency.1.clone(),
                                        request_hash.clone(),
                                        now_ms(),
                                    )
                                    .await?;
                                if let Some(reservation) = reservation.take() {
                                    reservation.uncertain();
                                }
                                Err(error)
                            }
                        }
                    } else {
                        // Only Brain's enumerated pre-commit validation/admission statuses release
                        // the intent. Timeouts, early-data responses, redirects, 5xx, and unknown
                        // statuses are ambiguous and retain it for same-key recovery.
                        if is_definitive_create_rejection(resp.status()) {
                            state
                                .db
                                .abandon_session_create_intent(
                                    account.id.clone(),
                                    idempotency.1.clone(),
                                    request_hash.clone(),
                                )
                                .await?;
                        } else {
                            state
                                .db
                                .mark_session_create_uncertain(
                                    account.id.clone(),
                                    idempotency.1.clone(),
                                    request_hash.clone(),
                                    now_ms(),
                                )
                                .await?;
                            if let Some(reservation) = reservation.take() {
                                reservation.uncertain();
                            }
                        }
                        Ok(resp)
                    }
                }
                Method::GET => {
                    forward(
                        &state,
                        &account.id,
                        method,
                        &path_and_query(&uri),
                        &headers,
                        None,
                    )
                    .await
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
    body: Body,
) -> Response {
    unwrap_response(
        async {
            let (key, account) = auth_key(&state, &headers).await?;
            if rejects_declared_get_body(&method, &headers) {
                return Err(Error::Invalid("session GET requires an empty body".into()));
            }
            let session_id = rest.split('/').next().unwrap_or("").to_string();
            let sub = rest.strip_prefix(&session_id).unwrap_or("");
            // Unknown or foreign sessions are 404, never 403: existence is not revealed.
            let row = owned_session_row(&state, &key, &account, &session_id).await?;
            let is_message = method == Method::POST && sub == "/messages";
            let is_child_message = is_child_message_request(&method, sub);
            if requires_idempotency_key(&method, sub) {
                required_idempotency_key(&headers)?;
            }
            // Take exactly one fail-fast process slot before the first body poll and hold it for
            // this whole request lifecycle. A retry is a new authenticated request; no internal
            // forwarding path reacquires and double-counts the same buffered bytes.
            let _body_slot = match buffered_session_body(&method, sub) {
                Some(BufferedSessionBody::Message) => {
                    Some(try_body_slot(&state.message_body_slots, "message")?)
                }
                Some(BufferedSessionBody::Inline) => Some(try_body_slot(
                    &state.inline_session_body_slots,
                    "inline session",
                )?),
                None => None,
            };
            let is_work = is_billable_work(&method, sub);
            let is_storage_write = starts_storage_write(&method, sub);
            let is_storage_upload = method == Method::POST && sub == "/storage/uploads";
            let is_end = method == Method::POST && sub == "/end";
            let is_delete = method == Method::DELETE && sub.is_empty();
            let is_deletion_status = method == Method::GET && sub == "/deletion";
            if is_deletion_status && let Some(job) = state.db.deletion(session_id.clone()).await? {
                return Ok(deletion_status_response(&job));
            }
            if is_delete && row.is_final {
                // Completion-oriented deletion is idempotent at Aex's durable boundary. This is
                // also the recovery response when the original 204 reached Aex but not its caller.
                return Ok(Response::builder()
                    .status(StatusCode::NO_CONTENT)
                    .body(Body::empty())
                    .expect("static completed-deletion response"));
            }
            if is_delete {
                // D16: both public modes cross the same short, durable Aex acceptance boundary.
                // The retry worker performs the recursive end fence, strong billing settlement,
                // and Brain/S3/Hand purge. A strict SDK caller observes `/deletion`; it never
                // keeps this HTTP request open across external cleanup.
                let chain = hydrate_delete_parent_chain(&state, &account, &key, &row).await?;
                state
                    .db
                    .begin_subtree_deletion_with_chain(
                        account.id.clone(),
                        session_id.clone(),
                        chain,
                        now_ms(),
                    )
                    .await?;
                if row.parent_id.is_none() {
                    state.admission.invalidate_live_root_sessions(&account.id);
                }
                return Ok(deletion_accepted_response(&session_id));
            }
            if row.fold.session_state == "deleting" && method != Method::GET {
                return Err(Error::Conflict(
                    "session deletion has been durably accepted".into(),
                ));
            }
            if is_work && !is_message {
                require_work_admission(&state, &account).await?;
            } else if is_storage_write {
                // Storage is inexpensive and does not wake compute, but an account without credit
                // must not be able to accumulate new durable bytes indefinitely.
                require_positive_balance(&state, &account).await?;
            }
            let body = if is_storage_upload {
                let bytes = axum::body::to_bytes(body, 128 * 1024).await.map_err(|_| {
                    Error::PayloadTooLarge(
                        "storage upload request exceeds the 131072-byte ceiling".into(),
                    )
                })?;
                validate_storage_upload(
                    &bytes,
                    row.fold
                        .session_storage_bytes
                        .saturating_add(row.fold.upload_reserved_bytes),
                    state.storage_limits,
                )?;
                Body::from(bytes)
            } else if is_child_message {
                // A child prompt or message becomes the same bounded journal fact as a root
                // message. It must not inherit the larger generic inline/file request ceiling.
                Body::from(bounded_message_body(body).await?)
            } else if matches!(method, Method::POST | Method::PUT | Method::PATCH) && !is_message {
                Body::from(bounded_inline_session_body(body).await?)
            } else {
                body
            };
            let resp = if is_message {
                let body = bounded_message_body(body).await?;
                let idempotency = headers
                    .get("Idempotency-Key")
                    .and_then(|value| value.to_str().ok());
                match output::prepare_message(&state.db, &session_id, &body, idempotency).await? {
                    PreparedMessage::Plain(body) => {
                        require_work_admission(&state, &account).await?;
                        forward(
                            &state,
                            &account.id,
                            method.clone(),
                            &path_and_query(&uri),
                            &headers,
                            Some(Body::from(body)),
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
                        if let Err(error) = require_work_admission(&state, &account).await {
                            state.db.abandon_output(output_id).await?;
                            return Err(error);
                        }
                        let response = match forward(
                            &state,
                            &account.id,
                            method.clone(),
                            &path_and_query(&uri),
                            &headers,
                            Some(Body::from(body)),
                        )
                        .await
                        {
                            Ok(response) => response,
                            // A request/response transport failure does not prove that Brain
                            // missed the message. Keep the pending identity so a retry with the
                            // same idempotency key reconstructs byte-identical input.
                            Err(error) => return Err(error),
                        };
                        if response.status() != StatusCode::ACCEPTED {
                            // A client error is a definitive rejection before turn admission.
                            // Server/redirect outcomes can be post-commit and therefore retain
                            // the identity for an idempotent recovery attempt.
                            if response.status().is_client_error() {
                                state.db.abandon_output(output_id).await?;
                            }
                            return Ok(response);
                        }
                        let (mut parts, response_body) = response.into_parts();
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
                        parts.headers.remove(header::CONTENT_LENGTH);
                        Response::from_parts(parts, Body::from(accepted_json))
                    }
                }
            } else {
                let body = if matches!(method, Method::GET | Method::HEAD | Method::DELETE) {
                    None
                } else {
                    Some(body)
                };
                forward(
                    &state,
                    &account.id,
                    method,
                    &path_and_query(&uri),
                    &headers,
                    body,
                )
                .await?
            };
            if is_end && resp.status().is_success() && row.parent_id.is_none() {
                // End is a short asynchronous acceptance. Never decrement the root count or
                // claim `ended` from the returned `ending` projection; invalidate the cached
                // count so the next admission re-reads Brain's tenant index. A lagging index can
                // conservatively reject until it projects `ended`, but cannot over-admit.
                state.admission.invalidate_live_root_sessions(&account.id);
            }
            Ok(resp)
        }
        .await,
    )
}

#[cfg(test)]
mod event_filter_tests {
    use super::*;

    fn counted_body(polls: Arc<std::sync::atomic::AtomicUsize>) -> Body {
        Body::from_stream(futures_util::stream::once(async move {
            polls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok::<_, std::convert::Infallible>(Bytes::from_static(br#"{}"#))
        }))
    }

    fn external_request(padding: usize) -> ExternalToolCallRequest {
        serde_json::from_value(json!({
            "session_id": "ses_01HZZZZZZZZZZZZZZZZZZZZZZZ",
            "turn_id": "trn_01HZZZZZZZZZZZZZZZZZZZZZZZ",
            "agent_id": "root",
            "call_id": "call_01HZZZZZZZZZZZZZZZZZZZZZZZ",
            "name": "web_search",
            "input": {"query": "aex"},
            "context": {
                "brain.capability": "aex.web.search",
                "padding": "x".repeat(padding)
            }
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn external_executor_auth_precedes_its_exact_brain_wire_bound() {
        let base = external_request(0);
        let base_bytes = brain_protocol::contract::external_tool_request_wire_bytes(&base).unwrap();
        let exact = external_request(
            brain_protocol::MAX_EXTERNAL_TOOL_REQUEST_BYTES
                .checked_sub(base_bytes)
                .expect("the fixed request fits the wire ceiling"),
        );
        let exact_bytes = serde_json::to_vec(&exact).unwrap();
        assert_eq!(
            exact_bytes.len(),
            brain_protocol::MAX_EXTERNAL_TOOL_REQUEST_BYTES
        );
        assert!(brain_protocol::contract::external_tool_request_wire_fits(
            &exact
        ));
        assert_eq!(
            bounded_external_tool_body(Body::from(exact_bytes))
                .await
                .unwrap()
                .len(),
            brain_protocol::MAX_EXTERNAL_TOOL_REQUEST_BYTES
        );
        let over =
            external_request(brain_protocol::MAX_EXTERNAL_TOOL_REQUEST_BYTES - base_bytes + 1);
        assert!(!brain_protocol::contract::external_tool_request_wire_fits(
            &over
        ));
        assert!(matches!(
            bounded_external_tool_body(Body::from(serde_json::to_vec(&over).unwrap())).await,
            Err(Error::PayloadTooLarge(_))
        ));

        let polls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let state = AppState {
            db: Db::open_memory().unwrap(),
            brain: BrainClient::new("http://127.0.0.1:1", "test-operator"),
            payments: Arc::new(crate::payments::FakePayments),
            stripe_webhook: None,
            card: RateCard::default(),
            operator_token_hash: None,
            external_executor_token_hash: Some(identity::hash_secret("executor-secret")),
            customer_hand_gateway: None,
            web: WebRuntime::hosted(None),
            admission: Admission::new(crate::admission::AdmissionConfig::default()).unwrap(),
            create_body_slots: Arc::new(tokio::sync::Semaphore::new(4)),
            message_body_slots: Arc::new(tokio::sync::Semaphore::new(256)),
            inline_session_body_slots: Arc::new(tokio::sync::Semaphore::new(64)),
            default_limits: (10, 30),
            max_retained_root_sessions: 100,
            storage_limits: StorageLimits::default(),
        };
        let response =
            execute_external_tool(State(state), HeaderMap::new(), counted_body(polls.clone()))
                .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            polls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "an unauthenticated private executor request must not poll its body"
        );
    }

    #[test]
    fn hosted_effectful_admission_requires_bounded_idempotency_keys() {
        for sub in [
            "/messages",
            "/children",
            "/children/ses_child/messages",
            "/children/ses_child/follow-up",
        ] {
            assert!(requires_idempotency_key(&Method::POST, sub), "{sub}");
        }
        for sub in [
            "/children",
            "/children/ses_child/messages",
            "/children/ses_child/follow-up",
        ] {
            assert!(is_child_message_request(&Method::POST, sub), "{sub}");
        }
        assert!(!is_child_message_request(
            &Method::GET,
            "/children/ses_child/messages"
        ));
        for sub in [
            "/end",
            "/cancel",
            "/sandbox",
            "/storage/uploads/transfer_1/complete",
            "/storage/delete",
            "/children/ses_child/end",
            "/children/ses_child/interrupt",
            "/children/ses_child/wait",
        ] {
            assert!(!requires_idempotency_key(&Method::POST, sub), "{sub}");
        }

        let empty = HeaderMap::new();
        assert!(required_idempotency_key(&empty).is_err());
        let mut valid = HeaderMap::new();
        valid.insert("Idempotency-Key", "stable-operation".parse().unwrap());
        assert_eq!(
            required_idempotency_key(&valid).unwrap(),
            "stable-operation"
        );
        assert!(is_definitive_create_rejection(StatusCode::BAD_REQUEST));
        assert!(is_definitive_create_rejection(StatusCode::CONFLICT));
        assert!(!is_definitive_create_rejection(StatusCode::REQUEST_TIMEOUT));
        assert!(!is_definitive_create_rejection(StatusCode::TOO_EARLY));
        assert!(!is_definitive_create_rejection(
            StatusCode::INTERNAL_SERVER_ERROR
        ));
    }

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
        assert!(suppress_public_event(output_call.as_bytes()).unwrap());
        assert!(
            suppress_public_event(
                format!(
                    "data: {{\"type\":\"tool.result\",\"name\":\"{}\"}}\r\n\r\n",
                    output::OUTPUT_TOOL_NAME
                )
                .as_bytes()
            )
            .unwrap()
        );
        assert!(
            !suppress_public_event(b"data: {\"type\":\"tool.call\",\"name\":\"web_search\"}\n\n")
                .unwrap()
        );
        assert!(
            !suppress_public_event(
                format!(
                    "data: {{\"type\":\"turn.completed\",\"result\":{{\"name\":\"{}\"}}}}\n\n",
                    output::OUTPUT_TOOL_NAME
                )
                .as_bytes()
            )
            .unwrap()
        );
        assert!(suppress_public_event(b"data: not-json\n\n").is_err());
    }

    #[test]
    fn public_event_data_cannot_consume_the_framing_allowance() {
        let fixed = json!({"type":"model.usage","padding":""}).to_string();
        let exact = json!({
            "type":"model.usage",
            "padding":"x".repeat(brain_protocol::MAX_PUBLIC_EVENT_BYTES - fixed.len())
        })
        .to_string();
        assert_eq!(exact.len(), brain_protocol::MAX_PUBLIC_EVENT_BYTES);
        assert!(!suppress_public_event(format!("data:{exact}\n\n").as_bytes()).unwrap());

        let over = json!({
            "type":"model.usage",
            "padding":"x".repeat(brain_protocol::MAX_PUBLIC_EVENT_BYTES + 1 - fixed.len())
        })
        .to_string();
        assert_eq!(over.len(), brain_protocol::MAX_PUBLIC_EVENT_BYTES + 1);
        assert!(suppress_public_event(format!("data:{over}\n\n").as_bytes()).is_err());
    }

    #[test]
    fn prepaid_admission_covers_every_operation_that_can_start_compute() {
        for path in [
            "/messages",
            "/children",
            "/children/ses_child/follow-up",
            "/sandbox",
            "/sandbox/files/read-inline",
            "/sandbox/files/uploads",
            "/storage/copy-from-sandbox",
            "/storage/copy-to-sandbox",
        ] {
            assert!(is_billable_work(&Method::POST, path), "{path}");
        }
        for path in [
            "/children/ses_child/wait",
            "/children/ses_child/interrupt",
            "/children/ses_child/end",
            "/storage/write-inline",
            "/storage/delete",
            "/end",
        ] {
            assert!(!is_billable_work(&Method::POST, path), "{path}");
        }
        assert!(is_billable_work(
            &Method::POST,
            "/children/ses_child/messages"
        ));
        assert!(!is_billable_work(&Method::GET, "/sandbox"));
        assert!(starts_storage_write(&Method::POST, "/storage/write-inline"));
        assert!(starts_storage_write(&Method::POST, "/storage/uploads"));
        assert!(!starts_storage_write(
            &Method::POST,
            "/storage/uploads/x/complete"
        ));
        assert!(!starts_storage_write(&Method::POST, "/storage/delete"));

        for path in [
            "/messages",
            "/children",
            "/children/ses_child/messages",
            "/children/ses_child/follow-up",
        ] {
            assert_eq!(
                buffered_session_body(&Method::POST, path),
                Some(BufferedSessionBody::Message),
                "{path}"
            );
        }
        for path in [
            "/storage/uploads",
            "/storage/write-inline",
            "/sandbox/files/write-inline",
            "/end",
        ] {
            assert_eq!(
                buffered_session_body(&Method::POST, path),
                Some(BufferedSessionBody::Inline),
                "{path}"
            );
        }
        assert_eq!(buffered_session_body(&Method::GET, "/events"), None);
        assert_eq!(buffered_session_body(&Method::DELETE, ""), None);
    }

    #[test]
    fn session_body_capacity_is_fail_fast_and_released_with_the_request() {
        let message = Arc::new(tokio::sync::Semaphore::new(1));
        let first = try_body_slot(&message, "message").unwrap();
        assert!(matches!(
            try_body_slot(&message, "message"),
            Err(Error::RateLimited(_))
        ));
        drop(first);
        assert!(try_body_slot(&message, "message").is_ok());

        let inline = Arc::new(tokio::sync::Semaphore::new(1));
        let first = try_body_slot(&inline, "inline session").unwrap();
        assert!(matches!(
            try_body_slot(&inline, "inline session"),
            Err(Error::RateLimited(_))
        ));
        drop(first);
        assert!(try_body_slot(&inline, "inline session").is_ok());
    }

    #[test]
    fn storage_uploads_are_bounded_before_a_ticket_reaches_brain() {
        let limits = StorageLimits {
            max_object_bytes: 512,
            max_session_bytes: 1_024,
        };
        assert!(validate_storage_upload(br#"{"bytes":512}"#, 512, limits).is_ok());
        assert!(matches!(
            validate_storage_upload(br#"{"bytes":513}"#, 0, limits),
            Err(Error::PayloadTooLarge(_))
        ));
        assert!(matches!(
            validate_storage_upload(br#"{"bytes":2}"#, 1_023, limits),
            Err(Error::StorageQuota(_))
        ));
        assert!(matches!(
            validate_storage_upload(br#"{"bytes":-1}"#, 0, limits),
            Err(Error::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn public_message_body_uses_the_exact_brain_journal_ceiling() {
        let exact = bounded_message_body(Body::from(vec![
            b'x';
            brain_protocol::MAX_MESSAGE_REQUEST_BYTES
        ]))
        .await
        .expect("the exact ceiling is accepted");
        assert_eq!(exact.len(), brain_protocol::MAX_MESSAGE_REQUEST_BYTES);

        assert!(matches!(
            bounded_message_body(Body::from(vec![
                b'x';
                brain_protocol::MAX_MESSAGE_REQUEST_BYTES
                    + 1
            ]))
            .await,
            Err(Error::PayloadTooLarge(_))
        ));
    }

    #[tokio::test]
    async fn saturated_message_capacity_returns_429_before_body_poll() {
        let db = Db::open_memory().unwrap();
        let api_key = "aex_sk_message_capacity_test";
        db.create_account(
            AccountRow {
                id: "acc_capacity".into(),
                email: "capacity@example.com".into(),
                created_ms: 1,
                max_concurrent_sessions: 10,
                session_creates_per_hour: 30,
            },
            "capacity@example.com".into(),
            identity::hash_secret("aex_at_unused"),
        )
        .await
        .unwrap();
        db.create_key(
            KeyRow {
                id: "key_capacity".into(),
                account_id: "acc_capacity".into(),
                name: "test".into(),
                prefix: "aex_sk_message".into(),
                created_ms: 1,
                last_used_ms: None,
                revoked_ms: None,
            },
            identity::hash_secret(api_key),
        )
        .await
        .unwrap();
        db.insert_session(SessionRow {
            id: "ses_capacity".into(),
            account_id: "acc_capacity".into(),
            key_id: "key_capacity".into(),
            parent_id: None,
            root_id: "ses_capacity".into(),
            depth: 0,
            shape: "1gb".into(),
            created_ms: 1,
            is_final: false,
            fold: crate::rating::FoldState {
                session_state: "open".into(),
                ..Default::default()
            },
        })
        .await
        .unwrap();
        let message_body_slots = Arc::new(tokio::sync::Semaphore::new(1));
        let inline_body_slots = Arc::new(tokio::sync::Semaphore::new(1));
        let state = AppState {
            db,
            brain: BrainClient::new("http://127.0.0.1:1", "test-operator"),
            payments: Arc::new(crate::payments::FakePayments),
            stripe_webhook: None,
            card: RateCard::default(),
            operator_token_hash: None,
            external_executor_token_hash: None,
            customer_hand_gateway: None,
            web: WebRuntime::hosted(None),
            admission: Admission::new(crate::admission::AdmissionConfig::default()).unwrap(),
            create_body_slots: Arc::new(tokio::sync::Semaphore::new(4)),
            message_body_slots: message_body_slots.clone(),
            inline_session_body_slots: inline_body_slots.clone(),
            default_limits: (10, 30),
            max_retained_root_sessions: 100,
            storage_limits: StorageLimits::default(),
        };
        let held = message_body_slots.acquire_owned().await.unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {api_key}").parse().unwrap(),
        );
        headers.insert("Idempotency-Key", "capacity-operation".parse().unwrap());

        let polls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        for rest in [
            "ses_capacity/messages",
            "ses_capacity/children/ses_child/messages",
            "ses_capacity/children/ses_child/follow-up",
        ] {
            let saturated = proxy_session(
                State(state.clone()),
                Method::POST,
                format!("/v1/sessions/{rest}").parse().unwrap(),
                Path(rest.to_owned()),
                headers.clone(),
                counted_body(polls.clone()),
            )
            .await;
            assert_eq!(saturated.status(), StatusCode::TOO_MANY_REQUESTS, "{rest}");
        }
        assert_eq!(
            polls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "root message, child message, and child follow-up must all fail before polling",
        );
        drop(held);

        let held = inline_body_slots.acquire_owned().await.unwrap();
        let inline_polls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let saturated = proxy_session(
            State(state),
            Method::POST,
            Uri::from_static("/v1/sessions/ses_capacity/storage/write-inline"),
            Path("ses_capacity/storage/write-inline".to_owned()),
            headers,
            counted_body(inline_polls.clone()),
        )
        .await;
        assert_eq!(saturated.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            inline_polls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "generic inline session mutations must fail before polling",
        );
        drop(held);
    }

    #[tokio::test]
    async fn customer_hand_auth_and_capacity_precede_every_body_poll() {
        let db = Db::open_memory().unwrap();
        let api_key = "aex_sk_customer_hand_body_test";
        db.create_account(
            AccountRow {
                id: "acc_customer_hand_body".into(),
                email: "customer-hand-body@example.com".into(),
                created_ms: 1,
                max_concurrent_sessions: 10,
                session_creates_per_hour: 30,
            },
            "customer-hand-body@example.com".into(),
            identity::hash_secret("aex_at_unused"),
        )
        .await
        .unwrap();
        db.create_key(
            KeyRow {
                id: "key_customer_hand_body".into(),
                account_id: "acc_customer_hand_body".into(),
                name: "test".into(),
                prefix: "aex_sk_customer".into(),
                created_ms: 1,
                last_used_ms: None,
                revoked_ms: None,
            },
            identity::hash_secret(api_key),
        )
        .await
        .unwrap();
        let body_slots = Arc::new(tokio::sync::Semaphore::new(1));
        let gateway_token = "gateway-secret-with-at-least-thirty-two-bytes";
        let state = AppState {
            db,
            brain: BrainClient::new("http://127.0.0.1:1", "test-operator"),
            payments: Arc::new(crate::payments::FakePayments),
            stripe_webhook: None,
            card: RateCard::default(),
            operator_token_hash: None,
            external_executor_token_hash: None,
            customer_hand_gateway: Some(
                CustomerHandGateway::new(
                    gateway_token,
                    vec!["127.0.0.0/8".parse().unwrap()],
                    vec!["198.51.100.0/24".parse().unwrap()],
                )
                .unwrap(),
            ),
            web: WebRuntime::hosted(None),
            admission: Admission::new(crate::admission::AdmissionConfig::default()).unwrap(),
            create_body_slots: Arc::new(tokio::sync::Semaphore::new(4)),
            message_body_slots: body_slots.clone(),
            inline_session_body_slots: Arc::new(tokio::sync::Semaphore::new(64)),
            default_limits: (10, 30),
            max_retained_root_sessions: 100,
            storage_limits: StorageLimits::default(),
        };
        let mut gateway_headers = HeaderMap::new();
        for (name, value) in [
            ("x-aex-apigateway-token", gateway_token),
            ("x-aex-connection-id", "connection-1"),
            ("x-aex-route-key", "$connect"),
            ("x-aex-request-id", "request-1"),
            ("x-aex-source-ip", "203.0.113.7"),
            ("sec-websocket-protocol", "aex.grant.test"),
            ("x-forwarded-for", "203.0.113.7"),
        ] {
            gateway_headers.insert(name, value.parse().unwrap());
        }

        let polls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        assert_eq!(
            create_customer_hand_grant(
                State(state.clone()),
                HeaderMap::new(),
                counted_body(polls.clone()),
            )
            .await
            .status(),
            StatusCode::UNAUTHORIZED,
        );
        let mut gateway_without_token = gateway_headers.clone();
        gateway_without_token.remove("x-aex-apigateway-token");
        assert_eq!(
            customer_hand_gateway(
                ConnectInfo("127.0.0.1:1234".parse().unwrap()),
                State(state.clone()),
                gateway_without_token,
                counted_body(polls.clone()),
            )
            .await
            .status(),
            StatusCode::UNAUTHORIZED,
        );
        assert_eq!(
            customer_hand_observation(
                State(state.clone()),
                Path("chg_test".into()),
                HeaderMap::new(),
                counted_body(polls.clone()),
            )
            .await
            .status(),
            StatusCode::UNAUTHORIZED,
        );
        assert_eq!(polls.load(std::sync::atomic::Ordering::SeqCst), 0);

        let held = body_slots.acquire_owned().await.unwrap();
        let mut api_headers = HeaderMap::new();
        api_headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {api_key}").parse().unwrap(),
        );
        assert_eq!(
            create_customer_hand_grant(
                State(state.clone()),
                api_headers,
                counted_body(polls.clone()),
            )
            .await
            .status(),
            StatusCode::TOO_MANY_REQUESTS,
        );
        assert_eq!(
            customer_hand_gateway(
                ConnectInfo("127.0.0.1:1234".parse().unwrap()),
                State(state.clone()),
                gateway_headers,
                counted_body(polls.clone()),
            )
            .await
            .status(),
            StatusCode::TOO_MANY_REQUESTS,
        );
        let mut observation_headers = HeaderMap::new();
        observation_headers.insert(
            header::AUTHORIZATION,
            "Bearer observation-grant".parse().unwrap(),
        );
        assert_eq!(
            customer_hand_observation(
                State(state),
                Path("chg_test".into()),
                observation_headers,
                counted_body(polls.clone()),
            )
            .await
            .status(),
            StatusCode::TOO_MANY_REQUESTS,
        );
        assert_eq!(
            polls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "customer-Hand auth failures and saturated requests must not poll their bodies",
        );
        drop(held);
    }

    #[tokio::test]
    async fn public_authentication_precedes_body_polling_and_create_uses_brains_exact_ceiling() {
        assert_eq!(
            brain_protocol::MAX_CREATE_SESSION_REQUEST_BYTES,
            24 * 1024 * 1024
        );
        let exact = bounded_create_body(Body::from(vec![
            b'x';
            brain_protocol::MAX_CREATE_SESSION_REQUEST_BYTES
        ]))
        .await
        .expect("the exact create ceiling is accepted");
        assert_eq!(
            exact.len(),
            brain_protocol::MAX_CREATE_SESSION_REQUEST_BYTES
        );
        drop(exact);
        assert!(matches!(
            bounded_create_body(Body::from(vec![
                b'x';
                brain_protocol::MAX_CREATE_SESSION_REQUEST_BYTES
                    + 1
            ]))
            .await,
            Err(Error::PayloadTooLarge(_))
        ));

        let polls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = polls.clone();
        let oversized = Bytes::from(vec![
            b'x';
            brain_protocol::MAX_CREATE_SESSION_REQUEST_BYTES + 1
        ]);
        let body = Body::from_stream(futures_util::stream::once(async move {
            observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok::<_, std::convert::Infallible>(oversized)
        }));
        let db = Db::open_memory().unwrap();
        let api_key = "aex_sk_create_body_test";
        db.create_account(
            AccountRow {
                id: "acc_create_body".into(),
                email: "create-body@example.com".into(),
                created_ms: 1,
                max_concurrent_sessions: 10,
                session_creates_per_hour: 30,
            },
            "create-body@example.com".into(),
            identity::hash_secret("aex_at_unused"),
        )
        .await
        .unwrap();
        db.create_key(
            KeyRow {
                id: "key_create_body".into(),
                account_id: "acc_create_body".into(),
                name: "test".into(),
                prefix: "aex_sk_create".into(),
                created_ms: 1,
                last_used_ms: None,
                revoked_ms: None,
            },
            identity::hash_secret(api_key),
        )
        .await
        .unwrap();
        let create_body_slots = Arc::new(tokio::sync::Semaphore::new(1));
        let state = AppState {
            db,
            brain: BrainClient::new("http://127.0.0.1:1", "test-operator"),
            payments: Arc::new(crate::payments::FakePayments),
            stripe_webhook: None,
            card: RateCard::default(),
            operator_token_hash: None,
            external_executor_token_hash: None,
            customer_hand_gateway: None,
            web: WebRuntime::hosted(None),
            admission: Admission::new(crate::admission::AdmissionConfig::default()).unwrap(),
            create_body_slots: create_body_slots.clone(),
            message_body_slots: Arc::new(tokio::sync::Semaphore::new(256)),
            inline_session_body_slots: Arc::new(tokio::sync::Semaphore::new(64)),
            default_limits: (10, 30),
            max_retained_root_sessions: 100,
            storage_limits: StorageLimits::default(),
        };
        let response = proxy_sessions_root(
            State(state.clone()),
            Method::POST,
            Uri::from_static("/v1/sessions"),
            HeaderMap::new(),
            body,
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            polls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "an unauthenticated create must not poll or allocate its body"
        );

        let held = create_body_slots.acquire_owned().await.unwrap();
        let saturated_polls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut create_headers = HeaderMap::new();
        create_headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {api_key}").parse().unwrap(),
        );
        create_headers.insert("Idempotency-Key", "create-capacity".parse().unwrap());
        let response = proxy_sessions_root(
            State(state.clone()),
            Method::POST,
            Uri::from_static("/v1/sessions"),
            create_headers,
            counted_body(saturated_polls.clone()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            saturated_polls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "a saturated authenticated create must not poll or allocate its body"
        );
        drop(held);

        let message_polls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        for rest in [
            "ses_missing/messages",
            "ses_missing/children/ses_child/messages",
            "ses_missing/children/ses_child/follow-up",
            "ses_missing/storage/write-inline",
        ] {
            let response = proxy_session(
                State(state.clone()),
                Method::POST,
                format!("/v1/sessions/{rest}").parse().unwrap(),
                Path(rest.to_owned()),
                HeaderMap::new(),
                counted_body(message_polls.clone()),
            )
            .await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{rest}");
        }
        assert_eq!(
            message_polls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "unauthenticated message and inline session mutations must not poll their bodies"
        );
    }
}
