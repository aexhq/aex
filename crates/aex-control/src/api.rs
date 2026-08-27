use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, Method, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::{any, delete, get, post},
};
use brain_protocol::{CreateSessionRequest, Session, SessionList};
use serde_json::json;
use sha2::{Digest as _, Sha256};

use crate::{
    Error, Result, billing,
    brain::BrainClient,
    identity, now_ms,
    payments::{PaymentStatus, Payments, RefundAttempt, StripeWebhook, StripeWebhookAction},
    rating::FoldState,
    rfc3339,
    store::{
        AccountRow, CreditGrantRow, Db, InvitedJoin, KeyRow, RefundRow, SessionCreateIntent,
        SessionCreateLimits, SessionRow, TopupRow, WaitlistRow,
    },
    usd_display,
};

const MAX_BRAIN_BODY_BYTES: usize = 32 * 1024 * 1024;
const MAX_CONTROL_BODY_BYTES: usize = 64 * 1024;
static UNMATCHED_PAID_EVENTS: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub brain: BrainClient,
    pub payments: Arc<dyn Payments>,
    pub stripe_webhook: Option<StripeWebhook>,
    pub operator_token_hash: Option<String>,
    pub default_limits: (i64, i64),
}

pub fn router(state: AppState) -> Router {
    let control = Router::new()
        .route("/v1/waitlist", post(join_waitlist))
        .route("/v1/admin/waitlist", get(list_waitlist))
        .route("/v1/admin/invitations", post(create_invitation))
        .route("/v1/admin/credit-grants", post(create_credit_grant))
        .route("/v1/admin/refunds", post(create_refund))
        .route("/v1/accounts", post(create_account))
        .route("/v1/account", get(account))
        .route("/v1/keys", post(create_key).get(list_keys))
        .route("/v1/keys/{key_id}", delete(revoke_key))
        .route("/v1/balance", get(balance))
        .route("/v1/topups", post(create_topup).get(list_topups))
        .route("/v1/topups/{topup_id}", get(get_topup))
        .route(
            "/v1/topups/checkout/{checkout_session_id}",
            get(get_checkout_return),
        )
        .route("/v1/webhooks/stripe", post(stripe_webhook))
        .route("/v1/usage", get(usage))
        .route("/v1/rates", get(rates))
        .layer(DefaultBodyLimit::max(MAX_CONTROL_BODY_BYTES));
    let brain = Router::new()
        .route("/v1/agentloops", any(proxy_agentloop_root))
        .route("/v1/agentloops/{digest}", any(proxy_agentloop))
        .route("/v1/sessions", any(proxy_sessions))
        .route("/v1/sessions/{*rest}", any(proxy_session))
        .layer(DefaultBodyLimit::max(MAX_BRAIN_BODY_BYTES));
    Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .merge(control)
        .merge(brain)
        .with_state(state)
}

async fn live() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn ready(State(state): State<AppState>) -> StatusCode {
    match state
        .brain
        .forward(Method::GET, "/health/ready", None, None, None)
        .await
    {
        Ok(response) if response.status().is_success() => StatusCode::NO_CONTENT,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    }
}

async fn proxy_agentloop_root(
    State(state): State<AppState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Body,
) -> Response {
    respond(
        async {
            auth_key(&state, &headers).await?;
            if method != Method::POST {
                return Err(Error::Invalid(
                    "only POST is supported on /v1/agentloops".into(),
                ));
            }
            forward(&state, method, &uri, &headers, Some(body)).await
        }
        .await,
    )
}

async fn proxy_agentloop(
    State(state): State<AppState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    respond(
        async {
            auth_key(&state, &headers).await?;
            if method != Method::GET {
                return Err(Error::Invalid(
                    "only GET is supported on an Agentloop".into(),
                ));
            }
            forward(&state, method, &uri, &headers, None).await
        }
        .await,
    )
}

async fn proxy_sessions(
    State(state): State<AppState>,
    method: Method,
    _uri: Uri,
    headers: HeaderMap,
    body: Body,
) -> Response {
    respond(
        async {
            let (key, account) = auth_key(&state, &headers).await?;
            match method {
                Method::POST => {
                    let idempotency_key = required_idempotency_key(&headers)?;
                    let bytes = bounded(body).await?;
                    serde_json::from_slice::<CreateSessionRequest>(&bytes)
                        .map_err(|error| Error::Invalid(format!("create session: {error}")))?;
                    let request_hash = hex::encode(Sha256::digest(&bytes));
                    let request_key_hash = identity::hash_secret(idempotency_key);
                    let completed = state
                        .db
                        .session_create_request(account.id.clone(), request_key_hash.clone())
                        .await?;
                    if completed
                        .as_ref()
                        .is_some_and(|request| request.request_hash != request_hash)
                    {
                        return Err(Error::Conflict(
                            "Idempotency-Key was already used with a different request".into(),
                        ));
                    }
                    let pending = state
                        .db
                        .session_create_intent(account.id.clone(), request_key_hash.clone())
                        .await?;
                    if pending
                        .as_ref()
                        .is_some_and(|intent| intent.request_hash != request_hash)
                    {
                        return Err(Error::Conflict(
                            "Idempotency-Key was already used with a different request".into(),
                        ));
                    }
                    if completed.is_none() && pending.is_none() {
                        require_funded_account(&state, &account.id).await?;
                    }
                    if completed.is_none() {
                        match state
                            .db
                            .ensure_session_create_admission(
                                account.id.clone(),
                                request_key_hash.clone(),
                                request_hash.clone(),
                                SessionCreateLimits {
                                    now_ms: now_ms(),
                                    max_retained_roots: account.max_concurrent_sessions,
                                    create_window_start_ms: now_ms().saturating_sub(3_600_000),
                                    max_creates_in_window: account.session_creates_per_hour,
                                },
                            )
                            .await?
                        {
                            SessionCreateIntent::Created | SessionCreateIntent::Existing => {}
                            SessionCreateIntent::Conflict => {
                                return Err(Error::Conflict(
                                    "Idempotency-Key was already used with a different request"
                                        .into(),
                                ));
                            }
                            SessionCreateIntent::RetainedRootLimit => {
                                return Err(Error::RateLimited(format!(
                                    "account is limited to {} retained sessions",
                                    account.max_concurrent_sessions
                                )));
                            }
                            SessionCreateIntent::CreateRateLimit => {
                                return Err(Error::RateLimited(format!(
                                    "account is limited to {} session creates per hour",
                                    account.session_creates_per_hour
                                )));
                            }
                        }
                    }
                    let response = match state
                        .brain
                        .forward(
                            Method::POST,
                            "/v1/sessions",
                            Some("application/json"),
                            Some(idempotency_key),
                            Some(bytes),
                        )
                        .await
                    {
                        Ok(response) => response,
                        Err(error) => {
                            if completed.is_none() {
                                state
                                    .db
                                    .mark_session_create_uncertain(
                                        account.id,
                                        request_key_hash,
                                        request_hash,
                                        now_ms(),
                                    )
                                    .await?;
                            }
                            return Err(error);
                        }
                    };
                    let (status, response_headers, bytes) = match read_upstream(response).await {
                        Ok(response) => response,
                        Err(error) => {
                            if completed.is_none() {
                                state
                                    .db
                                    .mark_session_create_uncertain(
                                        account.id,
                                        request_key_hash,
                                        request_hash,
                                        now_ms(),
                                    )
                                    .await?;
                            }
                            return Err(error);
                        }
                    };
                    if status.is_success() {
                        let session: Session = serde_json::from_slice(&bytes).map_err(|error| {
                            Error::Upstream(format!("Brain create response: {error}"))
                        })?;
                        let id = session.session_id.to_string();
                        let created_ms = now_ms();
                        state
                            .db
                            .record_created_session(
                                SessionRow {
                                    id: id.clone(),
                                    account_id: account.id,
                                    key_id: key.id,
                                    parent_id: None,
                                    root_id: id,
                                    depth: 0,
                                    shape: "brain".into(),
                                    created_ms,
                                    is_final: false,
                                    fold: FoldState {
                                        storage_transition_ms: created_ms,
                                        metered_to_ms: created_ms,
                                        session_state: format!("{:?}", session.status)
                                            .to_lowercase(),
                                        ..Default::default()
                                    },
                                },
                                Some((request_key_hash, request_hash, created_ms)),
                            )
                            .await?;
                    } else if completed.is_none() {
                        if status.is_client_error() {
                            state
                                .db
                                .abandon_session_create_intent(
                                    account.id,
                                    request_key_hash,
                                    request_hash,
                                )
                                .await?;
                        } else {
                            state
                                .db
                                .mark_session_create_uncertain(
                                    account.id,
                                    request_key_hash,
                                    request_hash,
                                    now_ms(),
                                )
                                .await?;
                        }
                    }
                    Ok(upstream_response(status, &response_headers, bytes))
                }
                Method::GET => {
                    let mut sessions = Vec::new();
                    for row in state.db.sessions_of(account.id).await? {
                        if let Some(session) = state.brain.session(&row.id).await? {
                            sessions.push(session);
                        }
                    }
                    Ok(Json(SessionList { sessions }).into_response())
                }
                _ => Err(Error::Invalid("unsupported sessions operation".into())),
            }
        }
        .await,
    )
}

async fn proxy_session(
    State(state): State<AppState>,
    Path(rest): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Body,
) -> Response {
    respond(
        async {
            let (_, account) = auth_key(&state, &headers).await?;
            let session_id = rest.split('/').next().unwrap_or_default();
            let row = state
                .db
                .session(session_id.to_owned())
                .await?
                .filter(|row| row.account_id == account.id)
                .ok_or(Error::NotFound)?;
            let suffix = rest.strip_prefix(session_id).unwrap_or_default();
            let body = match (method.clone(), suffix) {
                (Method::GET, "") | (Method::GET, "/events") => None,
                (Method::POST, "/messages") => {
                    required_idempotency_key(&headers)?;
                    billing::reconcile_session(&state.db, &state.brain, &row).await?;
                    require_positive_balance(&state, &account.id).await?;
                    Some(body)
                }
                (Method::POST, "/cancel") | (Method::POST, "/end") => {
                    required_idempotency_key(&headers)?;
                    None
                }
                (Method::DELETE, "") => {
                    required_idempotency_key(&headers)?;
                    None
                }
                _ => return Err(Error::NotFound),
            };
            let response = forward(&state, method.clone(), &uri, &headers, body).await?;
            if method == Method::DELETE && response.status() == StatusCode::NO_CONTENT {
                state.db.forget_session(row.account_id, row.id).await?;
            }
            Ok(response)
        }
        .await,
    )
}

async fn forward(
    state: &AppState,
    method: Method,
    uri: &Uri,
    headers: &HeaderMap,
    body: Option<Body>,
) -> Result<Response> {
    let body = match body {
        Some(body) => Some(bounded(body).await?),
        None => None,
    };
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok());
    let idempotency_key = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok());
    let response = state
        .brain
        .forward(
            method,
            uri.path_and_query()
                .map_or(uri.path(), |value| value.as_str()),
            content_type,
            idempotency_key,
            body,
        )
        .await?;
    proxy_response(response).await
}

async fn proxy_response(response: reqwest::Response) -> Result<Response> {
    let (status, headers, body) = read_upstream(response).await?;
    Ok(upstream_response(status, &headers, body))
}

async fn read_upstream(
    response: reqwest::Response,
) -> Result<(StatusCode, reqwest::header::HeaderMap, Bytes)> {
    let status = response.status();
    let headers = response.headers().clone();
    let body = response
        .bytes()
        .await
        .map_err(|error| Error::Upstream(format!("Brain response: {error}")))?;
    if body.len() > MAX_BRAIN_BODY_BYTES {
        return Err(Error::Upstream("Brain response exceeds 32 MiB".into()));
    }
    Ok((status, headers, body))
}

fn upstream_response(
    status: StatusCode,
    upstream_headers: &reqwest::header::HeaderMap,
    body: Bytes,
) -> Response {
    let mut response = Response::builder().status(status);
    for name in [header::CONTENT_TYPE, header::LOCATION, header::RETRY_AFTER] {
        if let Some(value) = upstream_headers.get(&name) {
            response = response.header(name, value);
        }
    }
    response
        .body(Body::from(body))
        .expect("valid upstream response")
}

async fn bounded(body: Body) -> Result<Bytes> {
    axum::body::to_bytes(body, MAX_BRAIN_BODY_BYTES)
        .await
        .map_err(|_| Error::PayloadTooLarge("request exceeds 32 MiB".into()))
}

fn required_idempotency_key(headers: &HeaderMap) -> Result<&str> {
    headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty() && value.len() <= 256)
        .ok_or_else(|| Error::Invalid("Idempotency-Key must be between 1 and 256 bytes".into()))
}

async fn auth_key(state: &AppState, headers: &HeaderMap) -> Result<(KeyRow, AccountRow)> {
    let token = identity::bearer(headers).ok_or(Error::Unauthorized)?;
    if !token.starts_with("aex_sk_") {
        return Err(Error::Unauthorized);
    }
    state
        .db
        .key_auth(identity::hash_secret(token), now_ms())
        .await?
        .ok_or(Error::Unauthorized)
}

async fn require_funded_account(state: &AppState, account_id: &str) -> Result<()> {
    billing::reconcile_account(&state.db, &state.brain, account_id).await?;
    require_positive_balance(state, account_id).await
}

async fn require_positive_balance(state: &AppState, account_id: &str) -> Result<()> {
    let balance = state.db.balance(account_id.to_owned()).await?;
    if balance <= 0 {
        return Err(Error::InsufficientBalance(
            "add credit before starting model work".into(),
        ));
    }
    Ok(())
}

async fn auth_account(state: &AppState, headers: &HeaderMap) -> Result<AccountRow> {
    let token = identity::bearer(headers).ok_or(Error::Unauthorized)?;
    if !token.starts_with("aex_at_") {
        return Err(Error::Unauthorized);
    }
    state
        .db
        .account_by_token_hash(identity::hash_secret(token))
        .await?
        .ok_or(Error::Unauthorized)
}

async fn account(State(state): State<AppState>, headers: HeaderMap) -> Response {
    respond(
        auth_account(&state, &headers)
            .await
            .map(|account| Json(account_json(&account)).into_response()),
    )
}

async fn create_key(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    respond(
        async {
            let account = auth_account(&state, &headers).await?;
            let request: aex_contracts::control::CreateApiKeyRequest = parse_body(&body)?;
            let minted = identity::mint_secret("sk");
            let row = KeyRow {
                id: identity::new_id("key"),
                account_id: account.id,
                name: request.name.to_string(),
                prefix: minted.prefix,
                created_ms: now_ms(),
                last_used_ms: None,
                revoked_ms: None,
            };
            state.db.create_key(row.clone(), minted.hash).await?;
            Ok(json_response(
                StatusCode::CREATED,
                json!({"key": key_json(&row), "secret": minted.secret}),
            ))
        }
        .await,
    )
}

async fn list_keys(State(state): State<AppState>, headers: HeaderMap) -> Response {
    respond(
        async {
            let account = auth_account(&state, &headers).await?;
            let keys: Vec<_> = state
                .db
                .list_keys(account.id)
                .await?
                .iter()
                .map(key_json)
                .collect();
            Ok(json_response(
                StatusCode::OK,
                json!({"object":"list","data":keys}),
            ))
        }
        .await,
    )
}

async fn revoke_key(
    State(state): State<AppState>,
    Path(key_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    respond(
        async {
            let account = auth_account(&state, &headers).await?;
            if !state.db.revoke_key(account.id, key_id, now_ms()).await? {
                return Err(Error::NotFound);
            }
            Ok(StatusCode::NO_CONTENT.into_response())
        }
        .await,
    )
}

async fn balance(State(state): State<AppState>, headers: HeaderMap) -> Response {
    respond(
        async {
            let account = auth_account(&state, &headers).await?;
            billing::reconcile_account(&state.db, &state.brain, &account.id).await?;
            let value = state.db.balance(account.id).await?;
            Ok(json_response(
                StatusCode::OK,
                json!({
                    "object":"balance",
                    "microusd":value.to_string(),
                    "usd":usd_display(value),
                    "metered_to":rfc3339(now_ms())
                }),
            ))
        }
        .await,
    )
}

async fn rates() -> Response {
    json_response(StatusCode::OK, rate_card_json())
}

fn account_json(account: &AccountRow) -> serde_json::Value {
    json!({
        "id":account.id,
        "object":"account",
        "email":account.email,
        "created_at":rfc3339(account.created_ms),
        "limits":{
            "max_concurrent_sessions":account.max_concurrent_sessions,
            "session_creates_per_hour":account.session_creates_per_hour
        }
    })
}

fn json_response(status: StatusCode, value: serde_json::Value) -> Response {
    (status, Json(value)).into_response()
}

fn parse_body<T: serde::de::DeserializeOwned>(body: &Bytes) -> Result<T> {
    if body.len() > MAX_CONTROL_BODY_BYTES {
        return Err(Error::PayloadTooLarge(
            "control request exceeds 64 KiB".into(),
        ));
    }
    serde_json::from_slice(body).map_err(|error| Error::Invalid(error.to_string()))
}

async fn auth_operator(state: &AppState, headers: &HeaderMap) -> Result<()> {
    let expected = state.operator_token_hash.as_ref().ok_or(Error::NotFound)?;
    let token = identity::bearer(headers).ok_or(Error::Unauthorized)?;
    if !token.starts_with("aex_ad_") || identity::hash_secret(token) != *expected {
        return Err(Error::Unauthorized);
    }
    Ok(())
}

fn normalized_email(email: &str) -> String {
    email.trim().to_ascii_lowercase()
}

fn key_json(key: &KeyRow) -> serde_json::Value {
    json!({
        "id":key.id,
        "object":"api_key",
        "name":key.name,
        "prefix":key.prefix,
        "created_at":rfc3339(key.created_ms),
        "last_used_at":key.last_used_ms.map(rfc3339),
        "revoked_at":key.revoked_ms.map(rfc3339)
    })
}

fn waitlist_entry_json(row: &WaitlistRow) -> serde_json::Value {
    json!({
        "object":"waitlist_entry",
        "email":row.email,
        "status":row.status,
        "created_at":rfc3339(row.created_ms),
        "invited_at":row.invited_ms.map(rfc3339),
        "joined_at":row.joined_ms.map(rfc3339)
    })
}

fn topup_json(row: &TopupRow) -> serde_json::Value {
    json!({
        "id":row.id,
        "object":"topup",
        "amount_cents":row.amount_cents,
        "status":row.status,
        "checkout_url":row.checkout_url,
        "created_at":rfc3339(row.created_ms),
        "paid_at":row.paid_ms.map(rfc3339)
    })
}

fn credit_grant_json(row: &CreditGrantRow) -> serde_json::Value {
    json!({
        "id":row.id,
        "object":"credit_grant",
        "email":row.email,
        "amount_cents":row.amount_cents,
        "reason":row.reason,
        "created_at":rfc3339(row.created_ms)
    })
}

fn refund_json(row: &RefundRow) -> serde_json::Value {
    json!({
        "id":row.id,
        "object":"refund",
        "topup_id":row.topup_id,
        "amount_cents":row.amount_cents,
        "status":row.status,
        "provider_ref":row.provider_ref,
        "failure_reason":row.failure_reason,
        "created_at":rfc3339(row.created_ms),
        "updated_at":rfc3339(row.updated_ms)
    })
}

fn rate_card_json() -> serde_json::Value {
    json!({"object":"rate_card","model_gateway":"pass_through"})
}

async fn join_waitlist(State(state): State<AppState>, body: Bytes) -> Response {
    respond(
        async {
            let request: aex_contracts::control::JoinWaitlistRequest = parse_body(&body)?;
            let email = normalized_email(request.email.as_str());
            let received_ms = now_ms();
            state.db.join_waitlist(email.clone(), received_ms).await?;
            Ok(json_response(
                StatusCode::ACCEPTED,
                json!({
                    "object":"waitlist_submission",
                    "email":email,
                    "status":"received",
                    "received_at":rfc3339(received_ms)
                }),
            ))
        }
        .await,
    )
}

async fn list_waitlist(State(state): State<AppState>, headers: HeaderMap) -> Response {
    respond(
        async {
            auth_operator(&state, &headers).await?;
            let data = state
                .db
                .list_waitlist()
                .await?
                .iter()
                .map(waitlist_entry_json)
                .collect::<Vec<_>>();
            Ok(json_response(
                StatusCode::OK,
                json!({"object":"list","data":data}),
            ))
        }
        .await,
    )
}

async fn create_invitation(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    respond(
        async {
            auth_operator(&state, &headers).await?;
            let request: aex_contracts::control::CreateInvitationRequest = parse_body(&body)?;
            let email = normalized_email(request.email.as_str());
            let minted = identity::mint_secret("iv");
            let invited_ms = now_ms();
            if state
                .db
                .invite_waitlist(email.clone(), minted.hash.clone(), invited_ms)
                .await?
                .is_none()
            {
                state
                    .db
                    .reinvite_lost_account(email.clone(), minted.hash, invited_ms)
                    .await?
                    .ok_or(Error::NotFound)?;
            }
            Ok(json_response(
                StatusCode::CREATED,
                json!({
                    "object":"invitation",
                    "email":email,
                    "invite_token":minted.secret,
                    "invited_at":rfc3339(invited_ms)
                }),
            ))
        }
        .await,
    )
}

async fn create_account(State(state): State<AppState>, body: Bytes) -> Response {
    respond(
        async {
            let request: aex_contracts::control::CreateAccountRequest = parse_body(&body)?;
            let minted = identity::mint_secret("at");
            let email = normalized_email(request.email.as_str());
            let row = AccountRow {
                id: identity::new_id("acc"),
                email: email.clone(),
                created_ms: now_ms(),
                max_concurrent_sessions: state.default_limits.0,
                session_creates_per_hour: state.default_limits.1,
            };
            let (account, created) = match state
                .db
                .create_invited_account(
                    row.clone(),
                    email,
                    minted.hash,
                    identity::hash_secret(request.invite_token.as_str()),
                    row.created_ms,
                )
                .await?
            {
                InvitedJoin::Created => (row, true),
                InvitedJoin::RotatedExisting(existing) => (existing, false),
            };
            Ok(json_response(
                if created {
                    StatusCode::CREATED
                } else {
                    StatusCode::OK
                },
                json!({"account":account_json(&account),"account_token":minted.secret}),
            ))
        }
        .await,
    )
}

fn control_idempotency_key(headers: &HeaderMap) -> Result<String> {
    let key = headers
        .get("idempotency-key")
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

async fn create_credit_grant(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    respond(
        async {
            auth_operator(&state, &headers).await?;
            let request_key = control_idempotency_key(&headers)?;
            let request: aex_contracts::control::CreateCreditGrantRequest = parse_body(&body)?;
            let amount_cents = i64::try_from(request.amount_cents.get())
                .map_err(|_| Error::Invalid("credit amount is too large".into()))?;
            if amount_cents > 100_000 {
                return Err(Error::Invalid(
                    "credit amount must be at most 100,000 cents".into(),
                ));
            }
            let reason = request.reason.trim().to_owned();
            if reason.is_empty() {
                return Err(Error::Invalid("credit reason cannot be blank".into()));
            }
            let (grant, created) = state
                .db
                .grant_credit(CreditGrantRow {
                    id: identity::new_id("grt"),
                    request_key,
                    account_id: String::new(),
                    email: normalized_email(request.email.as_str()),
                    amount_cents,
                    reason,
                    created_ms: now_ms(),
                })
                .await?;
            Ok(json_response(
                if created {
                    StatusCode::CREATED
                } else {
                    StatusCode::OK
                },
                credit_grant_json(&grant),
            ))
        }
        .await,
    )
}

async fn create_topup(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    respond(
        async {
            let account = auth_account(&state, &headers).await?;
            let request: aex_contracts::control::CreateTopupRequest = parse_body(&body)?;
            if !(1_000..=100_000).contains(&request.amount_cents) {
                return Err(Error::Invalid(
                    "top-up amount must be between 1,000 and 100,000 cents".into(),
                ));
            }
            let request_key = control_idempotency_key(&headers)?;
            if let Some(existing) = state
                .db
                .topup_by_request_key(account.id.clone(), request_key.clone())
                .await?
            {
                if existing.amount_cents != request.amount_cents {
                    return Err(Error::Conflict(
                        "Idempotency-Key was used with a different top-up amount".into(),
                    ));
                }
                return Ok(json_response(StatusCode::OK, topup_json(&existing)));
            }
            let id = identity::new_id("top");
            let checkout = state
                .payments
                .create_checkout(&id, request.amount_cents)
                .await?;
            let row = TopupRow {
                id,
                account_id: account.id,
                amount_cents: request.amount_cents,
                status: "pending".into(),
                provider: state.payments.name().into(),
                provider_ref: checkout.provider_ref,
                checkout_url: Some(checkout.url),
                created_ms: now_ms(),
                paid_ms: None,
            };
            state.db.create_topup(row.clone(), request_key).await?;
            Ok(json_response(StatusCode::CREATED, topup_json(&row)))
        }
        .await,
    )
}

async fn refresh_topup(state: &AppState, row: TopupRow) -> Result<TopupRow> {
    if row.status != "pending" {
        return Ok(row);
    }
    match state.payments.check(&row.provider_ref).await? {
        PaymentStatus::Paid => state.db.topup_paid(row.id.clone(), now_ms()).await?,
        PaymentStatus::Expired => state.db.topup_expired(row.id.clone()).await?,
        PaymentStatus::Pending => return Ok(row),
    }
    state
        .db
        .topup(row.account_id.clone(), row.id.clone())
        .await?
        .ok_or(Error::NotFound)
}

async fn get_topup(
    State(state): State<AppState>,
    Path(topup_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    respond(
        async {
            let account = auth_account(&state, &headers).await?;
            let row = state
                .db
                .topup(account.id, topup_id)
                .await?
                .ok_or(Error::NotFound)?;
            Ok(json_response(
                StatusCode::OK,
                topup_json(&refresh_topup(&state, row).await?),
            ))
        }
        .await,
    )
}

async fn list_topups(State(state): State<AppState>, headers: HeaderMap) -> Response {
    respond(
        async {
            let account = auth_account(&state, &headers).await?;
            let mut data = Vec::new();
            for row in state.db.list_topups(account.id.clone()).await? {
                data.push(topup_json(&refresh_topup(&state, row).await?));
            }
            Ok(json_response(
                StatusCode::OK,
                json!({"object":"list","data":data}),
            ))
        }
        .await,
    )
}

async fn get_checkout_return(
    State(state): State<AppState>,
    Path(checkout_session_id): Path<String>,
) -> Response {
    respond(
        async {
            if !checkout_session_id.starts_with("cs_")
                || checkout_session_id.len() > 128
                || !checkout_session_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            {
                return Err(Error::NotFound);
            }
            let row = state
                .db
                .stripe_topup_by_provider_ref(checkout_session_id)
                .await?
                .ok_or(Error::NotFound)?;
            let status = match refresh_topup(&state, row).await?.status.as_str() {
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

async fn stripe_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    respond(
        async {
            let webhook = state.stripe_webhook.as_ref().ok_or(Error::NotFound)?;
            let signature = headers
                .get("stripe-signature")
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| Error::Invalid("missing Stripe-Signature".into()))?;
            match webhook.verify(signature, &body, chrono::Utc::now().timestamp())? {
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
                        let count = UNMATCHED_PAID_EVENTS.fetch_add(1, Ordering::Relaxed) + 1;
                        tracing::error!(count, "verified Stripe payment matched no top-up");
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
                        tracing::warn!("verified Stripe expiry matched no pending top-up");
                    }
                }
                None => {}
            }
            Ok(json_response(StatusCode::OK, json!({"received":true})))
        }
        .await,
    )
}

async fn create_refund(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    respond(
        async {
            auth_operator(&state, &headers).await?;
            let request_key = control_idempotency_key(&headers)?;
            let request: aex_contracts::control::CreateRefundRequest = parse_body(&body)?;
            let amount_cents = i64::try_from(request.amount_cents.get())
                .map_err(|_| Error::Invalid("refund amount is too large".into()))?;
            let topup_id: String = request.topup_id.into();
            let existing = state.db.refund_by_request_key(request_key.clone()).await?;
            let mut refund = if let Some(existing) = existing {
                if existing.topup_id != topup_id || existing.amount_cents != amount_cents {
                    return Err(Error::Conflict(
                        "Idempotency-Key was used with different refund fields".into(),
                    ));
                }
                existing
            } else {
                let topup = state
                    .db
                    .operator_topup(topup_id.clone())
                    .await?
                    .ok_or(Error::NotFound)?;
                billing::reconcile_account(&state.db, &state.brain, &topup.account_id).await?;
                for session in state.db.sessions_of(topup.account_id.clone()).await? {
                    if state
                        .brain
                        .session(&session.id)
                        .await?
                        .is_some_and(|value| {
                            matches!(value.status, brain_protocol::SessionStatus::Running)
                        })
                    {
                        return Err(Error::Conflict(
                            "wait for running sessions before refunding credit".into(),
                        ));
                    }
                }
                let created_ms = now_ms();
                state
                    .db
                    .begin_refund(
                        RefundRow {
                            id: identity::new_id("rfd"),
                            request_key,
                            topup_id,
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
            if refund.status == "pending" {
                let topup = state
                    .db
                    .operator_topup(refund.topup_id.clone())
                    .await?
                    .ok_or(Error::NotFound)?;
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
            }
            Ok(json_response(StatusCode::OK, refund_json(&refund)))
        }
        .await,
    )
}

async fn usage(State(state): State<AppState>, headers: HeaderMap) -> Response {
    respond(
        async {
            let account = auth_account(&state, &headers).await?;
            let metered_to = now_ms();
            let reconciled =
                billing::reconcile_account(&state.db, &state.brain, &account.id).await?;
            let sessions = reconciled
                .iter()
                .map(|(session, usage)| {
                    let model_microusd = usage.gateway_cost_nano_usd / 1_000;
                    json!({
                        "session_id":session.id,
                        "state":session.fold.session_state,
                        "model_calls":usage.model_calls,
                        "input_tokens":usage.input_tokens.to_string(),
                        "output_tokens":usage.output_tokens.to_string(),
                        "model_microusd":model_microusd.to_string(),
                        "total_microusd":model_microusd.to_string(),
                        "metered_to":rfc3339(metered_to)
                    })
                })
                .collect::<Vec<_>>();
            let total = reconciled.iter().try_fold(0i64, |sum, (_, usage)| {
                sum.checked_add(usage.gateway_cost_nano_usd / 1_000)
                    .ok_or_else(|| Error::Internal("usage total exceeds the billing range".into()))
            })?;
            let balance = state.db.balance(account.id.clone()).await?;
            Ok(json_response(
                StatusCode::OK,
                json!({
                    "object":"usage",
                    "account_id":account.id,
                    "balance_microusd":balance.to_string(),
                    "total_microusd":total.to_string(),
                    "sessions":sessions,
                    "rates":rate_card_json(),
                    "metered_to":rfc3339(metered_to)
                }),
            ))
        }
        .await,
    )
}

fn respond(result: Result<Response>) -> Response {
    result.unwrap_or_else(|error| {
        (
            StatusCode::from_u16(error.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(json!({"error":{"code":error.code(),"message":error.to_string()}})),
        )
            .into_response()
    })
}
