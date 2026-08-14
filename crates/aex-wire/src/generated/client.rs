//! GENERATED — DO NOT EDIT.
//!
//! The low-level client: one request builder and one method per public operation.
//!
//! Produced by `aex-contract-gen` from `api/`; contract digest
//! `sha256:ec8637e9442587d0020caccfed0ab6fccef5a6d61dae1c162fe2d78e2af0dec8`.
//! Regenerate with `cargo run -p aex-contract-gen -- build`.

#![allow(clippy::large_enum_variant, reason = "a wire union is never boxed")]
#![allow(clippy::match_same_arms, reason = "one arm per row")]
#![allow(clippy::too_many_lines, reason = "one arm per row")]

use crate::client::ClientError;
use crate::client::NdjsonFrames;
use crate::client::PathWriter;
use crate::client::QueryWriter;
use crate::client::ToParam;
use crate::client::Transport;
use crate::client::WireClient;
use crate::client::WireRequest;
use crate::client::decode_ndjson;
use crate::client::decode_no_content;
use crate::client::decode_response;
use crate::client::decode_response_with_etag;
use crate::client::encode_body;
use crate::client::request_headers;
use crate::idempotency::IdempotencyKey;
use crate::ids::AccountId;
use crate::ids::AgentId;
use crate::ids::ApiKeyId;
use crate::ids::ApprovalId;
use crate::ids::BillingTransactionId;
use crate::ids::FileDownloadId;
use crate::ids::FileUploadId;
use crate::ids::GenerationId;
use crate::ids::InvitationId;
use crate::ids::MeasurementId;
use crate::ids::MembershipId;
use crate::ids::MessageId;
use crate::ids::ObservationId;
use crate::ids::OperationId;
use crate::ids::OrganizationId;
use crate::ids::PaymentMethodId;
use crate::ids::ProviderCredentialId;
use crate::ids::ResourceName;
use crate::ids::SessionId;
use crate::ids::StatementId;
use crate::ids::ToolCallId;
use crate::ids::UploadId;
use crate::ids::UserId;
use crate::ids::WorkspaceId;
use crate::models::ApiKeyCreateRequest;
use crate::models::ApiKeyPage;
use crate::models::ApiKeysListQuery;
use crate::models::BillingBalance;
use crate::models::BillingTransactionPage;
use crate::models::BillingTransactionsListQuery;
use crate::models::BillingUsageCategory;
use crate::models::BillingUsageGetQuery;
use crate::models::BillingUsagePage;
use crate::models::CliAuthConfig;
use crate::models::DashboardBootstrap;
use crate::models::DashboardSessionCredential;
use crate::models::DashboardSessionRequest;
use crate::models::DownloadGrant;
use crate::models::EmptyRequest;
use crate::models::HostedSession;
use crate::models::MessagePage;
use crate::models::MessageSendRequest;
use crate::models::MessageSendResult;
use crate::models::MessageStreamFrame;
use crate::models::NewApiKey;
use crate::models::PaymentMethodPage;
use crate::models::PaymentMethodSessionRequest;
use crate::models::RegisteredFile;
use crate::models::RegisteredFilePage;
use crate::models::RegisteredFileValue;
use crate::models::RegistryDownloadRequest;
use crate::models::RegistryFilesListQuery;
use crate::models::Session;
use crate::models::SessionCommandReceipt;
use crate::models::SessionCreateRequest;
use crate::models::SessionListPage;
use crate::models::SessionMessagesListQuery;
use crate::models::SessionMessagesStreamQuery;
use crate::models::SessionStatus;
use crate::models::SessionTelemetryReplayQuery;
use crate::models::SessionTelemetryStreamQuery;
use crate::models::SessionsListQuery;
use crate::models::TelemetryDownloadGrant;
use crate::models::TelemetryDownloadRequest;
use crate::models::TelemetryFrame;
use crate::models::TopUpCheckoutRequest;
use crate::models::UploadAdmission;
use crate::models::UploadCompleteRequest;
use crate::models::UploadCreateRequest;
use crate::routes::RouteId;
use crate::server::WithETag;
use crate::types::ETag;
use crate::types::HttpMethod;

// --- parameter encoding ---------------------------------------------------

/// Encodes a value that renders itself.
macro_rules! to_param_display {
    ($($ty:ty),* $(,)?) => {
        $(impl ToParam for $ty {
            fn to_param(&self) -> String {
                self.to_string()
            }
        })*
    };
}

to_param_display!(
    UserId,
    AccountId,
    OrganizationId,
    MembershipId,
    InvitationId,
    WorkspaceId,
    ApiKeyId,
    ProviderCredentialId,
    SessionId,
    MessageId,
    AgentId,
    ToolCallId,
    OperationId,
    ApprovalId,
    GenerationId,
    FileUploadId,
    FileDownloadId,
    ObservationId,
    UploadId,
    MeasurementId,
    StatementId,
    PaymentMethodId,
    BillingTransactionId,
);

/// Encodes a closed enumeration as its wire spelling.
macro_rules! to_param_enum {
    ($($ty:ty),* $(,)?) => {
        $(impl ToParam for $ty {
            fn to_param(&self) -> String {
                self.as_str().to_owned()
            }
        })*
    };
}

to_param_enum!(BillingUsageCategory, SessionStatus);

// --- request builders -----------------------------------------------------

/// `POST /api/api-keys`
/// Mint a workspace API key whose value is returned once.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn api_key_create_request(
    body: &ApiKeyCreateRequest,
    idempotency_key: &IdempotencyKey,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::ApiKeyCreate;
    let path = PathWriter::new(route);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, Some(idempotency_key), None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `DELETE /api/api-keys/{apiKeyId}`
/// Revoke a workspace API key.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn api_key_revoke_request(
    api_key_id: ApiKeyId,
    if_match: Option<&ETag>,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::ApiKeyRevoke;
    let mut path = PathWriter::new(route);
    path.bind(&api_key_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Delete,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, if_match),
        body: None,
    })
}

/// `GET /api/api-keys`
/// List workspace API key metadata.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn api_keys_list_request(query: &ApiKeysListQuery) -> Result<WireRequest, ClientError> {
    let route = RouteId::ApiKeysList;
    let path = PathWriter::new(route);
    let mut writer = QueryWriter::new();
    writer.put_option("cursor", query.cursor.as_ref());
    writer.put_option("limit", query.limit.as_ref());
    writer.put("workspaceId", &query.workspace_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: writer.finish(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `GET /api/auth/config`
/// Read the public Google OAuth configuration required by the native CLI.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn auth_config_get_request() -> Result<WireRequest, ClientError> {
    let route = RouteId::AuthConfigGet;
    let path = PathWriter::new(route);
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `GET /api/billing/balance`
/// Read the caller's prepaid balance and active reservations.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn billing_balance_get_request() -> Result<WireRequest, ClientError> {
    let route = RouteId::BillingBalanceGet;
    let path = PathWriter::new(route);
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `DELETE /api/billing/payment-methods/{paymentMethodId}`
/// Detach a card owned by the caller's account.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn billing_payment_method_delete_request(
    payment_method_id: PaymentMethodId,
    idempotency_key: &IdempotencyKey,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::BillingPaymentMethodDelete;
    let mut path = PathWriter::new(route);
    path.bind(&payment_method_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Delete,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, Some(idempotency_key), None, None),
        body: None,
    })
}

/// `POST /api/billing/payment-method-sessions`
/// Create a Stripe-hosted card setup session with explicit consent.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn billing_payment_method_session_create_request(
    body: &PaymentMethodSessionRequest,
    idempotency_key: &IdempotencyKey,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::BillingPaymentMethodSessionCreate;
    let path = PathWriter::new(route);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, Some(idempotency_key), None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `GET /api/billing/payment-methods`
/// List card display metadata for the caller's account.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn billing_payment_methods_list_request() -> Result<WireRequest, ClientError> {
    let route = RouteId::BillingPaymentMethodsList;
    let path = PathWriter::new(route);
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `POST /api/billing/top-up-checkouts`
/// Create a one-time Stripe-hosted prepaid top-up checkout.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn billing_top_up_checkout_create_request(
    body: &TopUpCheckoutRequest,
    idempotency_key: &IdempotencyKey,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::BillingTopUpCheckoutCreate;
    let path = PathWriter::new(route);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, Some(idempotency_key), None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `GET /api/billing/transactions`
/// List immutable prepaid ledger transactions, newest first.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn billing_transactions_list_request(
    query: &BillingTransactionsListQuery,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::BillingTransactionsList;
    let path = PathWriter::new(route);
    let mut writer = QueryWriter::new();
    writer.put_option("cursor", query.cursor.as_ref());
    writer.put_option("limit", query.limit.as_ref());
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: writer.finish(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `GET /api/billing/usage`
/// Read bounded rated usage and its settlement coverage.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn billing_usage_get_request(query: &BillingUsageGetQuery) -> Result<WireRequest, ClientError> {
    let route = RouteId::BillingUsageGet;
    let path = PathWriter::new(route);
    let mut writer = QueryWriter::new();
    writer.put_option("category", query.category.as_ref());
    writer.put_option("cursor", query.cursor.as_ref());
    writer.put_option("from", query.from.as_ref());
    writer.put_option("limit", query.limit.as_ref());
    writer.put_option("sessionId", query.session_id.as_ref());
    writer.put_option("to", query.to.as_ref());
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: writer.finish(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `GET /api/bootstrap`
/// One bounded read that fills the dashboard shell.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn dashboard_bootstrap_get_request() -> Result<WireRequest, ClientError> {
    let route = RouteId::DashboardBootstrapGet;
    let path = PathWriter::new(route);
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `POST /api/auth/sessions`
/// Exchange a provider authorization code for a first-party user session.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn dashboard_session_create_request(
    body: &DashboardSessionRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::DashboardSessionCreate;
    let path = PathWriter::new(route);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `DELETE /api/auth/sessions/current`
/// Close the first-party user session the caller presented.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn dashboard_session_delete_request() -> Result<WireRequest, ClientError> {
    let route = RouteId::DashboardSessionDelete;
    let path = PathWriter::new(route);
    Ok(WireRequest {
        route,
        method: HttpMethod::Delete,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `DELETE /api/files/{name}`
/// Delete one current workspace file.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn registry_files_delete_request(
    name: &ResourceName,
    if_match: Option<&ETag>,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::RegistryFilesDelete;
    let mut path = PathWriter::new(route);
    path.bind(name);
    Ok(WireRequest {
        route,
        method: HttpMethod::Delete,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, if_match),
        body: None,
    })
}

/// `POST /api/files/{name}/downloads`
/// Mint a download grant for a ready current workspace file.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn registry_files_download_create_request(
    name: &ResourceName,
    body: &RegistryDownloadRequest,
    idempotency_key: &IdempotencyKey,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::RegistryFilesDownloadCreate;
    let mut path = PathWriter::new(route);
    path.bind(name);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, Some(idempotency_key), None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `GET /api/files/{name}`
/// Read one current workspace file.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn registry_files_get_request(name: &ResourceName) -> Result<WireRequest, ClientError> {
    let route = RouteId::RegistryFilesGet;
    let mut path = PathWriter::new(route);
    path.bind(name);
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `GET /api/files`
/// List current workspace files.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn registry_files_list_request(
    query: &RegistryFilesListQuery,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::RegistryFilesList;
    let path = PathWriter::new(route);
    let mut writer = QueryWriter::new();
    writer.put_option("cursor", query.cursor.as_ref());
    writer.put_option("limit", query.limit.as_ref());
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: writer.finish(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `PUT /api/files/{name}`
/// Replace one current workspace file from inline bytes or an HTTPS URL.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn registry_files_put_request(
    name: &ResourceName,
    body: &RegisteredFileValue,
    idempotency_key: &IdempotencyKey,
    if_match: Option<&ETag>,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::RegistryFilesPut;
    let mut path = PathWriter::new(route);
    path.bind(name);
    Ok(WireRequest {
        route,
        method: HttpMethod::Put,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, Some(idempotency_key), None, if_match),
        body: Some(encode_body(route, body)?),
    })
}

/// `POST /api/sessions/{sessionId}/cancellations`
/// Cancel current work and return the session to idle.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_cancel_request(
    session_id: SessionId,
    body: &EmptyRequest,
    operation_id: OperationId,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionCancel;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, Some(operation_id), None),
        body: Some(encode_body(route, body)?),
    })
}

/// `POST /api/sessions`
/// Create a durable session and eagerly prepare its default-on sandbox in the background.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_create_request(
    body: &SessionCreateRequest,
    idempotency_key: &IdempotencyKey,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionCreate;
    let path = PathWriter::new(route);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, Some(idempotency_key), None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `POST /api/sessions/{sessionId}/deletions`
/// Irreversibly delete session-scoped user content; independent workspace files remain.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_delete_request(
    session_id: SessionId,
    body: &EmptyRequest,
    operation_id: OperationId,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionDelete;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, Some(operation_id), None),
        body: Some(encode_body(route, body)?),
    })
}

/// `GET /api/sessions/{sessionId}`
/// Read durable session metadata and sandbox preparation state.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_get_request(session_id: SessionId) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionGet;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `POST /api/sessions/{sessionId}/messages`
/// Admit one text message; file paths are referenced in text, never attached.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_message_send_request(
    session_id: SessionId,
    body: &MessageSendRequest,
    idempotency_key: &IdempotencyKey,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionMessageSend;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, Some(idempotency_key), None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `GET /api/sessions/{sessionId}/messages`
/// List complete committed messages in immutable seal order.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_messages_list_request(
    session_id: SessionId,
    query: &SessionMessagesListQuery,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionMessagesList;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    let mut writer = QueryWriter::new();
    writer.put_option("cursor", query.cursor.as_ref());
    writer.put_option("limit", query.limit.as_ref());
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: writer.finish(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `GET /api/sessions/{sessionId}/messages/stream`
/// Stream bounded assistant previews plus commit/reconcile/gap frames.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_messages_stream_request(
    session_id: SessionId,
    query: &SessionMessagesStreamQuery,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionMessagesStream;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    let mut writer = QueryWriter::new();
    writer.put_option("after", query.after.as_ref());
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: writer.finish(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `POST /api/sessions/{sessionId}/telemetry/downloads`
/// Mint a short-lived download for a bounded retained telemetry export.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_telemetry_download_create_request(
    session_id: SessionId,
    body: &TelemetryDownloadRequest,
    idempotency_key: &IdempotencyKey,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionTelemetryDownloadCreate;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, Some(idempotency_key), None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `GET /api/sessions/{sessionId}/telemetry/replay`
/// Replay retained telemetry from compressed immutable S3 segments.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_telemetry_replay_request(
    session_id: SessionId,
    query: &SessionTelemetryReplayQuery,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionTelemetryReplay;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    let mut writer = QueryWriter::new();
    writer.put_option("after", query.after.as_ref());
    writer.put_option("limit", query.limit.as_ref());
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: writer.finish(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `GET /api/sessions/{sessionId}/telemetry/stream`
/// Stream live trusted assistant, tool, runtime, Logs and Traces telemetry with bounded previews.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_telemetry_stream_request(
    session_id: SessionId,
    query: &SessionTelemetryStreamQuery,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionTelemetryStream;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    let mut writer = QueryWriter::new();
    writer.put_option("after", query.after.as_ref());
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: writer.finish(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `POST /api/sessions/{sessionId}/terminations`
/// Destroy sandbox compute while retaining session metadata and messages.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_terminate_request(
    session_id: SessionId,
    body: &EmptyRequest,
    operation_id: OperationId,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionTerminate;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, Some(operation_id), None),
        body: Some(encode_body(route, body)?),
    })
}

/// `GET /api/sessions`
/// List sessions in the workspace.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn sessions_list_request(query: &SessionsListQuery) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionsList;
    let path = PathWriter::new(route);
    let mut writer = QueryWriter::new();
    writer.put_option("cursor", query.cursor.as_ref());
    writer.put_option("limit", query.limit.as_ref());
    writer.put_option("status", query.status.as_ref());
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: writer.finish(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `POST /api/uploads/{uploadId}/completions`
/// Verify an admitted upload and publish it only if its private overwrite intent is still current.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn upload_complete_request(
    upload_id: UploadId,
    body: &UploadCompleteRequest,
    idempotency_key: &IdempotencyKey,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::UploadComplete;
    let mut path = PathWriter::new(route);
    path.bind(&upload_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, Some(idempotency_key), None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `POST /api/uploads`
/// Admit a direct upload for one current workspace-file name and return every bounded part grant.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn upload_create_request(
    body: &UploadCreateRequest,
    idempotency_key: &IdempotencyKey,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::UploadCreate;
    let path = PathWriter::new(route);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, Some(idempotency_key), None, None),
        body: Some(encode_body(route, body)?),
    })
}

// --- client methods -------------------------------------------------------

/// One method per public operation, over whichever transport the caller injected.
impl<T: Transport> WireClient<T> {
    /// `POST /api/api-keys`
    /// Mint a workspace API key whose value is returned once.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn api_key_create(
        &self,
        body: &ApiKeyCreateRequest,
        idempotency_key: &IdempotencyKey,
    ) -> Result<NewApiKey, ClientError> {
        let request = api_key_create_request(body, idempotency_key)?;
        let response = self.send(request).await?;
        decode_response(RouteId::ApiKeyCreate, &response)
    }

    /// `DELETE /api/api-keys/{apiKeyId}`
    /// Revoke a workspace API key.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn api_key_revoke(
        &self,
        api_key_id: ApiKeyId,
        if_match: Option<&ETag>,
    ) -> Result<(), ClientError> {
        let request = api_key_revoke_request(api_key_id, if_match)?;
        let response = self.send(request).await?;
        decode_no_content(RouteId::ApiKeyRevoke, &response)
    }

    /// `GET /api/api-keys`
    /// List workspace API key metadata.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn api_keys_list(&self, query: &ApiKeysListQuery) -> Result<ApiKeyPage, ClientError> {
        let request = api_keys_list_request(query)?;
        let response = self.send(request).await?;
        decode_response(RouteId::ApiKeysList, &response)
    }

    /// `GET /api/auth/config`
    /// Read the public Google OAuth configuration required by the native CLI.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn auth_config_get(&self) -> Result<CliAuthConfig, ClientError> {
        let request = auth_config_get_request()?;
        let response = self.send(request).await?;
        decode_response(RouteId::AuthConfigGet, &response)
    }

    /// `GET /api/billing/balance`
    /// Read the caller's prepaid balance and active reservations.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn billing_balance_get(&self) -> Result<BillingBalance, ClientError> {
        let request = billing_balance_get_request()?;
        let response = self.send(request).await?;
        decode_response(RouteId::BillingBalanceGet, &response)
    }

    /// `DELETE /api/billing/payment-methods/{paymentMethodId}`
    /// Detach a card owned by the caller's account.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn billing_payment_method_delete(
        &self,
        payment_method_id: PaymentMethodId,
        idempotency_key: &IdempotencyKey,
    ) -> Result<(), ClientError> {
        let request = billing_payment_method_delete_request(payment_method_id, idempotency_key)?;
        let response = self.send(request).await?;
        decode_no_content(RouteId::BillingPaymentMethodDelete, &response)
    }

    /// `POST /api/billing/payment-method-sessions`
    /// Create a Stripe-hosted card setup session with explicit consent.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn billing_payment_method_session_create(
        &self,
        body: &PaymentMethodSessionRequest,
        idempotency_key: &IdempotencyKey,
    ) -> Result<HostedSession, ClientError> {
        let request = billing_payment_method_session_create_request(body, idempotency_key)?;
        let response = self.send(request).await?;
        decode_response(RouteId::BillingPaymentMethodSessionCreate, &response)
    }

    /// `GET /api/billing/payment-methods`
    /// List card display metadata for the caller's account.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn billing_payment_methods_list(&self) -> Result<PaymentMethodPage, ClientError> {
        let request = billing_payment_methods_list_request()?;
        let response = self.send(request).await?;
        decode_response(RouteId::BillingPaymentMethodsList, &response)
    }

    /// `POST /api/billing/top-up-checkouts`
    /// Create a one-time Stripe-hosted prepaid top-up checkout.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn billing_top_up_checkout_create(
        &self,
        body: &TopUpCheckoutRequest,
        idempotency_key: &IdempotencyKey,
    ) -> Result<HostedSession, ClientError> {
        let request = billing_top_up_checkout_create_request(body, idempotency_key)?;
        let response = self.send(request).await?;
        decode_response(RouteId::BillingTopUpCheckoutCreate, &response)
    }

    /// `GET /api/billing/transactions`
    /// List immutable prepaid ledger transactions, newest first.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn billing_transactions_list(
        &self,
        query: &BillingTransactionsListQuery,
    ) -> Result<BillingTransactionPage, ClientError> {
        let request = billing_transactions_list_request(query)?;
        let response = self.send(request).await?;
        decode_response(RouteId::BillingTransactionsList, &response)
    }

    /// `GET /api/billing/usage`
    /// Read bounded rated usage and its settlement coverage.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn billing_usage_get(
        &self,
        query: &BillingUsageGetQuery,
    ) -> Result<BillingUsagePage, ClientError> {
        let request = billing_usage_get_request(query)?;
        let response = self.send(request).await?;
        decode_response(RouteId::BillingUsageGet, &response)
    }

    /// `GET /api/bootstrap`
    /// One bounded read that fills the dashboard shell.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn dashboard_bootstrap_get(&self) -> Result<DashboardBootstrap, ClientError> {
        let request = dashboard_bootstrap_get_request()?;
        let response = self.send(request).await?;
        decode_response(RouteId::DashboardBootstrapGet, &response)
    }

    /// `POST /api/auth/sessions`
    /// Exchange a provider authorization code for a first-party user session.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn dashboard_session_create(
        &self,
        body: &DashboardSessionRequest,
    ) -> Result<DashboardSessionCredential, ClientError> {
        let request = dashboard_session_create_request(body)?;
        let response = self.send(request).await?;
        decode_response(RouteId::DashboardSessionCreate, &response)
    }

    /// `DELETE /api/auth/sessions/current`
    /// Close the first-party user session the caller presented.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn dashboard_session_delete(&self) -> Result<(), ClientError> {
        let request = dashboard_session_delete_request()?;
        let response = self.send(request).await?;
        decode_no_content(RouteId::DashboardSessionDelete, &response)
    }

    /// `DELETE /api/files/{name}`
    /// Delete one current workspace file.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn registry_files_delete(
        &self,
        name: &ResourceName,
        if_match: Option<&ETag>,
    ) -> Result<(), ClientError> {
        let request = registry_files_delete_request(name, if_match)?;
        let response = self.send(request).await?;
        decode_no_content(RouteId::RegistryFilesDelete, &response)
    }

    /// `POST /api/files/{name}/downloads`
    /// Mint a download grant for a ready current workspace file.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn registry_files_download_create(
        &self,
        name: &ResourceName,
        body: &RegistryDownloadRequest,
        idempotency_key: &IdempotencyKey,
    ) -> Result<DownloadGrant, ClientError> {
        let request = registry_files_download_create_request(name, body, idempotency_key)?;
        let response = self.send(request).await?;
        decode_response(RouteId::RegistryFilesDownloadCreate, &response)
    }

    /// `GET /api/files/{name}`
    /// Read one current workspace file.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn registry_files_get(
        &self,
        name: &ResourceName,
    ) -> Result<WithETag<RegisteredFile>, ClientError> {
        let request = registry_files_get_request(name)?;
        let response = self.send(request).await?;
        decode_response_with_etag(RouteId::RegistryFilesGet, &response)
    }

    /// `GET /api/files`
    /// List current workspace files.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn registry_files_list(
        &self,
        query: &RegistryFilesListQuery,
    ) -> Result<RegisteredFilePage, ClientError> {
        let request = registry_files_list_request(query)?;
        let response = self.send(request).await?;
        decode_response(RouteId::RegistryFilesList, &response)
    }

    /// `PUT /api/files/{name}`
    /// Replace one current workspace file from inline bytes or an HTTPS URL.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn registry_files_put(
        &self,
        name: &ResourceName,
        body: &RegisteredFileValue,
        idempotency_key: &IdempotencyKey,
        if_match: Option<&ETag>,
    ) -> Result<WithETag<RegisteredFile>, ClientError> {
        let request = registry_files_put_request(name, body, idempotency_key, if_match)?;
        let response = self.send(request).await?;
        decode_response_with_etag(RouteId::RegistryFilesPut, &response)
    }

    /// `POST /api/sessions/{sessionId}/cancellations`
    /// Cancel current work and return the session to idle.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_cancel(
        &self,
        session_id: SessionId,
        body: &EmptyRequest,
        operation_id: OperationId,
    ) -> Result<SessionCommandReceipt, ClientError> {
        let request = session_cancel_request(session_id, body, operation_id)?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionCancel, &response)
    }

    /// `POST /api/sessions`
    /// Create a durable session and eagerly prepare its default-on sandbox in the background.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_create(
        &self,
        body: &SessionCreateRequest,
        idempotency_key: &IdempotencyKey,
    ) -> Result<Session, ClientError> {
        let request = session_create_request(body, idempotency_key)?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionCreate, &response)
    }

    /// `POST /api/sessions/{sessionId}/deletions`
    /// Irreversibly delete session-scoped user content; independent workspace files remain.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_delete(
        &self,
        session_id: SessionId,
        body: &EmptyRequest,
        operation_id: OperationId,
    ) -> Result<SessionCommandReceipt, ClientError> {
        let request = session_delete_request(session_id, body, operation_id)?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionDelete, &response)
    }

    /// `GET /api/sessions/{sessionId}`
    /// Read durable session metadata and sandbox preparation state.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_get(
        &self,
        session_id: SessionId,
    ) -> Result<WithETag<Session>, ClientError> {
        let request = session_get_request(session_id)?;
        let response = self.send(request).await?;
        decode_response_with_etag(RouteId::SessionGet, &response)
    }

    /// `POST /api/sessions/{sessionId}/messages`
    /// Admit one text message; file paths are referenced in text, never attached.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_message_send(
        &self,
        session_id: SessionId,
        body: &MessageSendRequest,
        idempotency_key: &IdempotencyKey,
    ) -> Result<MessageSendResult, ClientError> {
        let request = session_message_send_request(session_id, body, idempotency_key)?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionMessageSend, &response)
    }

    /// `GET /api/sessions/{sessionId}/messages`
    /// List complete committed messages in immutable seal order.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_messages_list(
        &self,
        session_id: SessionId,
        query: &SessionMessagesListQuery,
    ) -> Result<MessagePage, ClientError> {
        let request = session_messages_list_request(session_id, query)?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionMessagesList, &response)
    }

    /// `GET /api/sessions/{sessionId}/messages/stream`
    /// Stream bounded assistant previews plus commit/reconcile/gap frames.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_messages_stream(
        &self,
        session_id: SessionId,
        query: &SessionMessagesStreamQuery,
    ) -> Result<NdjsonFrames<MessageStreamFrame>, ClientError> {
        let request = session_messages_stream_request(session_id, query)?;
        let response = self.send(request).await?;
        decode_ndjson(RouteId::SessionMessagesStream, response)
    }

    /// `POST /api/sessions/{sessionId}/telemetry/downloads`
    /// Mint a short-lived download for a bounded retained telemetry export.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_telemetry_download_create(
        &self,
        session_id: SessionId,
        body: &TelemetryDownloadRequest,
        idempotency_key: &IdempotencyKey,
    ) -> Result<TelemetryDownloadGrant, ClientError> {
        let request = session_telemetry_download_create_request(session_id, body, idempotency_key)?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionTelemetryDownloadCreate, &response)
    }

    /// `GET /api/sessions/{sessionId}/telemetry/replay`
    /// Replay retained telemetry from compressed immutable S3 segments.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_telemetry_replay(
        &self,
        session_id: SessionId,
        query: &SessionTelemetryReplayQuery,
    ) -> Result<NdjsonFrames<TelemetryFrame>, ClientError> {
        let request = session_telemetry_replay_request(session_id, query)?;
        let response = self.send(request).await?;
        decode_ndjson(RouteId::SessionTelemetryReplay, response)
    }

    /// `GET /api/sessions/{sessionId}/telemetry/stream`
    /// Stream live trusted assistant, tool, runtime, Logs and Traces telemetry with bounded
    /// previews.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_telemetry_stream(
        &self,
        session_id: SessionId,
        query: &SessionTelemetryStreamQuery,
    ) -> Result<NdjsonFrames<TelemetryFrame>, ClientError> {
        let request = session_telemetry_stream_request(session_id, query)?;
        let response = self.send(request).await?;
        decode_ndjson(RouteId::SessionTelemetryStream, response)
    }

    /// `POST /api/sessions/{sessionId}/terminations`
    /// Destroy sandbox compute while retaining session metadata and messages.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_terminate(
        &self,
        session_id: SessionId,
        body: &EmptyRequest,
        operation_id: OperationId,
    ) -> Result<SessionCommandReceipt, ClientError> {
        let request = session_terminate_request(session_id, body, operation_id)?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionTerminate, &response)
    }

    /// `GET /api/sessions`
    /// List sessions in the workspace.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn sessions_list(
        &self,
        query: &SessionsListQuery,
    ) -> Result<SessionListPage, ClientError> {
        let request = sessions_list_request(query)?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionsList, &response)
    }

    /// `POST /api/uploads/{uploadId}/completions`
    /// Verify an admitted upload and publish it only if its private overwrite intent is still
    /// current.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn upload_complete(
        &self,
        upload_id: UploadId,
        body: &UploadCompleteRequest,
        idempotency_key: &IdempotencyKey,
    ) -> Result<RegisteredFile, ClientError> {
        let request = upload_complete_request(upload_id, body, idempotency_key)?;
        let response = self.send(request).await?;
        decode_response(RouteId::UploadComplete, &response)
    }

    /// `POST /api/uploads`
    /// Admit a direct upload for one current workspace-file name and return every bounded part
    /// grant.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn upload_create(
        &self,
        body: &UploadCreateRequest,
        idempotency_key: &IdempotencyKey,
    ) -> Result<UploadAdmission, ClientError> {
        let request = upload_create_request(body, idempotency_key)?;
        let response = self.send(request).await?;
        decode_response(RouteId::UploadCreate, &response)
    }
}
