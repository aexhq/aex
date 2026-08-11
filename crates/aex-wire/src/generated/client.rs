//! GENERATED — DO NOT EDIT.
//!
//! The low-level client: one request builder and one method per public operation.
//!
//! Produced by `aex-contract-gen` from `api/`; contract digest
//! `sha256:dddf87f6715de38e24d77e348f34f1d066254b797719a09a995d660373335a66`.
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
use crate::client::decode_binary;
use crate::client::decode_ndjson;
use crate::client::decode_no_content;
use crate::client::decode_response;
use crate::client::decode_response_with_etag;
use crate::client::encode_body;
use crate::client::request_headers;
use crate::idempotency::IdempotencyKey;
use crate::ids::AgentId;
use crate::ids::ApiKeyId;
use crate::ids::ApprovalId;
use crate::ids::ExportId;
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
use crate::ids::ProviderCredentialId;
use crate::ids::ResourceName;
use crate::ids::SessionId;
use crate::ids::StatementId;
use crate::ids::TelemetryBatchId;
use crate::ids::TelemetryGapId;
use crate::ids::ToolCallId;
use crate::ids::TraceId;
use crate::ids::UploadId;
use crate::ids::UserId;
use crate::ids::WorkspaceId;
use crate::limits::LimitId;
use crate::models::AccountGetQuery;
use crate::models::AccountOperationalState;
use crate::models::ApiKeyCreateRequest;
use crate::models::ApiKeyPage;
use crate::models::ApiKeysListQuery;
use crate::models::AutoTopupPolicy;
use crate::models::AutoTopupPolicyRequest;
use crate::models::BillingBalance;
use crate::models::BillingBalanceGetQuery;
use crate::models::BillingStatementsListQuery;
use crate::models::CentralOperationsListQuery;
use crate::models::DashboardBootstrap;
use crate::models::DashboardSessionCredential;
use crate::models::DashboardSessionRequest;
use crate::models::DeviceAuthorization;
use crate::models::DeviceAuthorizationRequest;
use crate::models::DeviceDecisionRequest;
use crate::models::DeviceDecisionResult;
use crate::models::DeviceToken;
use crate::models::DeviceTokenRequest;
use crate::models::DownloadGrant;
use crate::models::EffectiveWorkspaceLimit;
use crate::models::EffectiveWorkspaceLimitPage;
use crate::models::EmptyRequest;
use crate::models::HostedSession;
use crate::models::Invitation;
use crate::models::InvitationAcceptResult;
use crate::models::InvitationCreateRequest;
use crate::models::LiveFileDownload;
use crate::models::LiveFileDownloadCompleteRequest;
use crate::models::LiveFileDownloadRequest;
use crate::models::LiveFileEntry;
use crate::models::LiveFileEntryPage;
use crate::models::LiveFileListRequest;
use crate::models::LiveFileStatRequest;
use crate::models::LiveFileUpload;
use crate::models::LiveFileUploadCreateRequest;
use crate::models::MembershipPage;
use crate::models::MembershipsListQuery;
use crate::models::MessagePage;
use crate::models::MessageSendRequest;
use crate::models::MessageSendResult;
use crate::models::MetricAggregationPage;
use crate::models::MetricAggregationRequest;
use crate::models::NewApiKey;
use crate::models::ObservationFrame;
use crate::models::ObservationListenRequest;
use crate::models::ObservationPage;
use crate::models::ObservationQuery;
use crate::models::ObservationStreamRequest;
use crate::models::Operation;
use crate::models::OperationKind;
use crate::models::OperationPage;
use crate::models::OperationStatus;
use crate::models::Organization;
use crate::models::OrganizationCreateRequest;
use crate::models::OrganizationPage;
use crate::models::OrganizationsListQuery;
use crate::models::PortalSessionRequest;
use crate::models::ProviderCredential;
use crate::models::ProviderCredentialPage;
use crate::models::ProviderCredentialRegisterRequest;
use crate::models::ProviderCredentialsListQuery;
use crate::models::RegionalOperationsListQuery;
use crate::models::RegisteredFile;
use crate::models::RegisteredFilePage;
use crate::models::RegisteredFileValue;
use crate::models::RegistryDownloadRequest;
use crate::models::RegistryFilesListQuery;
use crate::models::Session;
use crate::models::SessionCreateRequest;
use crate::models::SessionFilesLiveDownloadPartGetQuery;
use crate::models::SessionFilesLiveUploadPartPutQuery;
use crate::models::SessionListPage;
use crate::models::SessionMessagesListQuery;
use crate::models::SessionStatus;
use crate::models::SessionsListQuery;
use crate::models::Statement;
use crate::models::StatementSummaryPage;
use crate::models::TelemetryAdmissionReceipt;
use crate::models::TelemetryExport;
use crate::models::TelemetryExportRequest;
use crate::models::TelemetryGap;
use crate::models::TelemetryGapPage;
use crate::models::TelemetryGapQuery;
use crate::models::TopUpCheckoutRequest;
use crate::models::TraceDetail;
use crate::models::Upload;
use crate::models::UploadCompleteRequest;
use crate::models::UploadCreateRequest;
use crate::models::UploadPartGrants;
use crate::models::UploadPartsRequest;
use crate::models::UsagePage;
use crate::models::UsageQuery;
use crate::models::UsageQueryQuery;
use crate::models::Workspace;
use crate::models::WorkspaceCreateRequest;
use crate::models::WorkspaceDeleteRequest;
use crate::models::WorkspacePage;
use crate::models::WorkspacesListQuery;
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
    TelemetryBatchId,
    TelemetryGapId,
    ExportId,
    UploadId,
    MeasurementId,
    StatementId,
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

to_param_enum!(OperationKind, OperationStatus, SessionStatus);

// --- request builders -----------------------------------------------------

/// `GET /api/account`
/// Read the account and its operational state.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn account_get_request(query: &AccountGetQuery) -> Result<WireRequest, ClientError> {
    let route = RouteId::AccountGet;
    let path = PathWriter::new(route);
    let mut writer = QueryWriter::new();
    writer.put("organizationId", &query.organization_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: writer.finish(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

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

/// `GET /api/organizations/{organizationId}/billing/auto-topup-policy`
/// Read the automatic top-up policy.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn billing_auto_topup_policy_get_request(
    organization_id: OrganizationId,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::BillingAutoTopupPolicyGet;
    let mut path = PathWriter::new(route);
    path.bind(&organization_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `PUT /api/organizations/{organizationId}/billing/auto-topup-policy`
/// Replace the automatic top-up policy.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn billing_auto_topup_policy_put_request(
    organization_id: OrganizationId,
    body: &AutoTopupPolicyRequest,
    idempotency_key: &IdempotencyKey,
    if_match: &ETag,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::BillingAutoTopupPolicyPut;
    let mut path = PathWriter::new(route);
    path.bind(&organization_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Put,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, Some(idempotency_key), None, Some(if_match)),
        body: Some(encode_body(route, body)?),
    })
}

/// `GET /api/billing/balance`
/// Read the prepaid balance of an organization.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn billing_balance_get_request(
    query: &BillingBalanceGetQuery,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::BillingBalanceGet;
    let path = PathWriter::new(route);
    let mut writer = QueryWriter::new();
    writer.put_option("organizationId", query.organization_id.as_ref());
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: writer.finish(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `POST /api/organizations/{organizationId}/billing/portal-sessions`
/// Create a hosted billing portal session.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn billing_portal_session_create_request(
    organization_id: OrganizationId,
    body: &PortalSessionRequest,
    idempotency_key: &IdempotencyKey,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::BillingPortalSessionCreate;
    let mut path = PathWriter::new(route);
    path.bind(&organization_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, Some(idempotency_key), None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `POST /api/organizations/{organizationId}/billing/statements/{statementId}/downloads`
/// Mint a download grant for an issued statement.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn billing_statement_download_create_request(
    organization_id: OrganizationId,
    statement_id: StatementId,
    body: &EmptyRequest,
    idempotency_key: &IdempotencyKey,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::BillingStatementDownloadCreate;
    let mut path = PathWriter::new(route);
    path.bind(&organization_id);
    path.bind(&statement_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, Some(idempotency_key), None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `GET /api/organizations/{organizationId}/billing/statements/{statementId}`
/// Read one immutable issued statement.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn billing_statement_get_request(
    organization_id: OrganizationId,
    statement_id: StatementId,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::BillingStatementGet;
    let mut path = PathWriter::new(route);
    path.bind(&organization_id);
    path.bind(&statement_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `GET /api/organizations/{organizationId}/billing/statements`
/// List issued statements, newest first.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn billing_statements_list_request(
    organization_id: OrganizationId,
    query: &BillingStatementsListQuery,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::BillingStatementsList;
    let mut path = PathWriter::new(route);
    path.bind(&organization_id);
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

/// `POST /api/organizations/{organizationId}/billing/top-up-checkouts`
/// Create a hosted top-up checkout.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn billing_top_up_checkout_create_request(
    organization_id: OrganizationId,
    body: &TopUpCheckoutRequest,
    idempotency_key: &IdempotencyKey,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::BillingTopUpCheckoutCreate;
    let mut path = PathWriter::new(route);
    path.bind(&organization_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, Some(idempotency_key), None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `POST /api/operations/{operationId}/cancellations`
/// Request cancellation of a central operation.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn central_operation_cancel_request(
    operation_id: OperationId,
    body: &EmptyRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::CentralOperationCancel;
    let mut path = PathWriter::new(route);
    path.bind(&operation_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `GET /api/operations/{operationId}`
/// Read one central durable operation.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn central_operation_get_request(
    operation_id: OperationId,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::CentralOperationGet;
    let mut path = PathWriter::new(route);
    path.bind(&operation_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `GET /api/operations`
/// List central durable operations for one organization.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn central_operations_list_request(
    query: &CentralOperationsListQuery,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::CentralOperationsList;
    let path = PathWriter::new(route);
    let mut writer = QueryWriter::new();
    writer.put_option("cursor", query.cursor.as_ref());
    writer.put_option("kind", query.kind.as_ref());
    writer.put_option("limit", query.limit.as_ref());
    writer.put("organizationId", &query.organization_id);
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
/// Exchange a provider authorization code for a browser session.
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
/// Close the browser session the caller presented.
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

/// `POST /api/auth/device/authorizations`
/// Begin the CLI device-authorization flow.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn device_authorization_create_request(
    body: &DeviceAuthorizationRequest,
    idempotency_key: &IdempotencyKey,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::DeviceAuthorizationCreate;
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

/// `POST /api/auth/device/decisions`
/// Approve or deny a pending device authorization.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn device_decision_create_request(
    body: &DeviceDecisionRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::DeviceDecisionCreate;
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

/// `POST /api/auth/device/tokens`
/// Exchange an approved device code for an account token.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn device_token_create_request(body: &DeviceTokenRequest) -> Result<WireRequest, ClientError> {
    let route = RouteId::DeviceTokenCreate;
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

/// `POST /api/invitations/acceptances`
/// Redeem every pending invitation addressed to the caller's verified email.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn invitation_accept_request(body: &EmptyRequest) -> Result<WireRequest, ClientError> {
    let route = RouteId::InvitationAccept;
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

/// `POST /api/organizations/{organizationId}/invitations`
/// Invite a person to the organization.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn invitation_create_request(
    organization_id: OrganizationId,
    body: &InvitationCreateRequest,
    idempotency_key: &IdempotencyKey,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::InvitationCreate;
    let mut path = PathWriter::new(route);
    path.bind(&organization_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, Some(idempotency_key), None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `GET /api/organizations/{organizationId}/memberships`
/// List the memberships of an organization.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn memberships_list_request(
    organization_id: OrganizationId,
    query: &MembershipsListQuery,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::MembershipsList;
    let mut path = PathWriter::new(route);
    path.bind(&organization_id);
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

/// `POST /api/streams/events/listen`
/// Listen for new workspace events observations.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn observations_events_listen_request(
    body: &ObservationListenRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::ObservationsEventsListen;
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

/// `POST /api/observations/events/query`
/// Query workspace events observations.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn observations_events_query_request(
    body: &ObservationQuery,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::ObservationsEventsQuery;
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

/// `POST /api/streams/events/stream`
/// Stream workspace events observations from one origin.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn observations_events_stream_request(
    body: &ObservationStreamRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::ObservationsEventsStream;
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

/// `POST /api/streams/logs/listen`
/// Listen for new workspace logs observations.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn observations_logs_listen_request(
    body: &ObservationListenRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::ObservationsLogsListen;
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

/// `POST /api/observations/logs/query`
/// Query workspace logs observations.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn observations_logs_query_request(
    body: &ObservationQuery,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::ObservationsLogsQuery;
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

/// `POST /api/streams/logs/stream`
/// Stream workspace logs observations from one origin.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn observations_logs_stream_request(
    body: &ObservationStreamRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::ObservationsLogsStream;
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

/// `POST /api/observations/metrics/aggregate`
/// Aggregate workspace metric observations.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn observations_metrics_aggregate_request(
    body: &MetricAggregationRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::ObservationsMetricsAggregate;
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

/// `POST /api/streams/metrics/listen`
/// Listen for new workspace metrics observations.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn observations_metrics_listen_request(
    body: &ObservationListenRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::ObservationsMetricsListen;
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

/// `POST /api/observations/metrics/query`
/// Query workspace metrics observations.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn observations_metrics_query_request(
    body: &ObservationQuery,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::ObservationsMetricsQuery;
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

/// `POST /api/streams/metrics/stream`
/// Stream workspace metrics observations from one origin.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn observations_metrics_stream_request(
    body: &ObservationStreamRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::ObservationsMetricsStream;
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

/// `POST /api/streams/spans/listen`
/// Listen for new workspace spans observations.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn observations_spans_listen_request(
    body: &ObservationListenRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::ObservationsSpansListen;
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

/// `POST /api/observations/spans/query`
/// Query workspace spans observations.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn observations_spans_query_request(
    body: &ObservationQuery,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::ObservationsSpansQuery;
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

/// `POST /api/streams/spans/stream`
/// Stream workspace spans observations from one origin.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn observations_spans_stream_request(
    body: &ObservationStreamRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::ObservationsSpansStream;
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

/// `POST /api/streams/telemetry/listen`
/// Listen for new workspace telemetry observations.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn observations_telemetry_listen_request(
    body: &ObservationListenRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::ObservationsTelemetryListen;
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

/// `POST /api/observations/telemetry/query`
/// Query workspace telemetry observations.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn observations_telemetry_query_request(
    body: &ObservationQuery,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::ObservationsTelemetryQuery;
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

/// `POST /api/streams/telemetry/stream`
/// Stream workspace telemetry observations from one origin.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn observations_telemetry_stream_request(
    body: &ObservationStreamRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::ObservationsTelemetryStream;
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

/// `POST /api/streams/traces/listen`
/// Listen for new workspace traces observations.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn observations_traces_listen_request(
    body: &ObservationListenRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::ObservationsTracesListen;
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

/// `POST /api/observations/traces/query`
/// Query workspace traces observations.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn observations_traces_query_request(
    body: &ObservationQuery,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::ObservationsTracesQuery;
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

/// `POST /api/streams/traces/stream`
/// Stream workspace traces observations from one origin.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn observations_traces_stream_request(
    body: &ObservationStreamRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::ObservationsTracesStream;
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

/// `POST /api/organizations`
/// Create an organization whose creator becomes owner.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn organization_create_request(
    body: &OrganizationCreateRequest,
    idempotency_key: &IdempotencyKey,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::OrganizationCreate;
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

/// `GET /api/organizations/{organizationId}`
/// Read one organization.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn organization_get_request(
    organization_id: OrganizationId,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::OrganizationGet;
    let mut path = PathWriter::new(route);
    path.bind(&organization_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `GET /api/organizations`
/// List organizations the caller belongs to.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn organizations_list_request(
    query: &OrganizationsListQuery,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::OrganizationsList;
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

/// `POST /api/otlp/v1/logs`
/// Admit an OTLP logs batch.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn otlp_logs_ingest_request(
    body: &[u8],
    idempotency_key: &IdempotencyKey,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::OtlpLogsIngest;
    let path = PathWriter::new(route);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, Some(idempotency_key), None, None),
        body: Some(body.to_vec()),
    })
}

/// `POST /api/otlp/v1/metrics`
/// Admit an OTLP metrics batch.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn otlp_metrics_ingest_request(
    body: &[u8],
    idempotency_key: &IdempotencyKey,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::OtlpMetricsIngest;
    let path = PathWriter::new(route);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, Some(idempotency_key), None, None),
        body: Some(body.to_vec()),
    })
}

/// `POST /api/otlp/v1/traces`
/// Admit an OTLP traces batch.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn otlp_traces_ingest_request(
    body: &[u8],
    idempotency_key: &IdempotencyKey,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::OtlpTracesIngest;
    let path = PathWriter::new(route);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, Some(idempotency_key), None, None),
        body: Some(body.to_vec()),
    })
}

/// `GET /api/workspace/provider-credentials/{providerCredentialId}`
/// Read one BYOK provider-credential binding.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn provider_credential_get_request(
    provider_credential_id: ProviderCredentialId,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::ProviderCredentialGet;
    let mut path = PathWriter::new(route);
    path.bind(&provider_credential_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `POST /api/workspace/provider-credentials`
/// Register a dedicated BYOK provider credential; carries plaintext once.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn provider_credential_register_request(
    body: &ProviderCredentialRegisterRequest,
    idempotency_key: &IdempotencyKey,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::ProviderCredentialRegister;
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

/// `POST /api/workspace/provider-credentials/{providerCredentialId}/revocations`
/// Revoke a BYOK provider-credential binding.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn provider_credential_revoke_request(
    provider_credential_id: ProviderCredentialId,
    body: &EmptyRequest,
    idempotency_key: &IdempotencyKey,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::ProviderCredentialRevoke;
    let mut path = PathWriter::new(route);
    path.bind(&provider_credential_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, Some(idempotency_key), None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `GET /api/workspace/provider-credentials`
/// List BYOK provider-credential binding metadata.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn provider_credentials_list_request(
    query: &ProviderCredentialsListQuery,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::ProviderCredentialsList;
    let path = PathWriter::new(route);
    let mut writer = QueryWriter::new();
    writer.put_option("cursor", query.cursor.as_ref());
    writer.put_option("limit", query.limit.as_ref());
    writer.put_option("provider", query.provider.as_ref());
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: writer.finish(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `POST /api/operations/{operationId}/cancellations`
/// Request cancellation of a regional operation.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn regional_operation_cancel_request(
    operation_id: OperationId,
    body: &EmptyRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::RegionalOperationCancel;
    let mut path = PathWriter::new(route);
    path.bind(&operation_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `GET /api/operations/{operationId}`
/// Read one regional durable operation.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn regional_operation_get_request(
    operation_id: OperationId,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::RegionalOperationGet;
    let mut path = PathWriter::new(route);
    path.bind(&operation_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `GET /api/operations`
/// List regional durable operations.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn regional_operations_list_request(
    query: &RegionalOperationsListQuery,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::RegionalOperationsList;
    let path = PathWriter::new(route);
    let mut writer = QueryWriter::new();
    writer.put_option("cursor", query.cursor.as_ref());
    writer.put_option("kind", query.kind.as_ref());
    writer.put_option("limit", query.limit.as_ref());
    writer.put_option("sessionId", query.session_id.as_ref());
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

/// `DELETE /api/workspace/files/{name}`
/// Delete one registered workspace file.
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

/// `POST /api/workspace/files/{name}/downloads`
/// Mint a download grant for a registered workspace file.
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

/// `GET /api/workspace/files/{name}`
/// Read one registered workspace file.
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

/// `GET /api/workspace/files`
/// List registered workspace files.
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

/// `PUT /api/workspace/files/{name}`
/// Replace one registered workspace file.
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
/// Create an eight-hour multi-turn session.
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
/// Irreversibly delete the session and session-scoped user content, observations, telemetry, and
/// export objects. Independent registered workspace files remain.
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

/// `POST /api/sessions/{sessionId}/files/live/downloads/{fileDownloadId}/completion`
/// Re-stat, re-hash, and close an exact live-file download after SDK verification.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_files_live_download_complete_request(
    session_id: SessionId,
    file_download_id: FileDownloadId,
    body: &LiveFileDownloadCompleteRequest,
    idempotency_key: &IdempotencyKey,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionFilesLiveDownloadComplete;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    path.bind(&file_download_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, Some(idempotency_key), None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `POST /api/sessions/{sessionId}/files/live/downloads`
/// Open one exact-version live file directly from the retained generation, auto-resuming it.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_files_live_download_create_request(
    session_id: SessionId,
    body: &LiveFileDownloadRequest,
    idempotency_key: &IdempotencyKey,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionFilesLiveDownloadCreate;
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

/// `DELETE /api/sessions/{sessionId}/files/live/downloads/{fileDownloadId}`
/// Close an abandoned live-file descriptor, auto-resuming the same generation.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_files_live_download_delete_request(
    session_id: SessionId,
    file_download_id: FileDownloadId,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionFilesLiveDownloadDelete;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    path.bind(&file_download_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Delete,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `GET /api/sessions/{sessionId}/files/live/downloads/{fileDownloadId}/parts/{partNumber}`
/// Read one raw exact-version range directly from the retained generation.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_files_live_download_part_get_request(
    session_id: SessionId,
    file_download_id: FileDownloadId,
    part_number: u32,
    query: &SessionFilesLiveDownloadPartGetQuery,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionFilesLiveDownloadPartGet;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    path.bind(&file_download_id);
    path.bind(&part_number);
    let mut writer = QueryWriter::new();
    writer.put("version", &query.version);
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: writer.finish(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `POST /api/sessions/{sessionId}/files/live/list`
/// List the exact live generation, auto-resuming that same generation when suspended.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_files_live_list_request(
    session_id: SessionId,
    body: &LiveFileListRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionFilesLiveList;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `POST /api/sessions/{sessionId}/files/live/stat`
/// Stat one live path without following a symlink, auto-resuming the same generation.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_files_live_stat_request(
    session_id: SessionId,
    body: &LiveFileStatRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionFilesLiveStat;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `POST /api/sessions/{sessionId}/files/live/uploads/{fileUploadId}/completion`
/// Verify every part and atomically publish one live file in the same generation.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_files_live_upload_complete_request(
    session_id: SessionId,
    file_upload_id: FileUploadId,
    idempotency_key: &IdempotencyKey,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionFilesLiveUploadComplete;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    path.bind(&file_upload_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, Some(idempotency_key), None, None),
        body: None,
    })
}

/// `POST /api/sessions/{sessionId}/files/live/uploads`
/// Create a resumable upload in the exact live generation, auto-resuming it when suspended.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_files_live_upload_create_request(
    session_id: SessionId,
    body: &LiveFileUploadCreateRequest,
    idempotency_key: &IdempotencyKey,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionFilesLiveUploadCreate;
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

/// `DELETE /api/sessions/{sessionId}/files/live/uploads/{fileUploadId}`
/// Abort a partial upload, auto-resuming the same generation to remove its temporary bytes.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_files_live_upload_delete_request(
    session_id: SessionId,
    file_upload_id: FileUploadId,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionFilesLiveUploadDelete;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    path.bind(&file_upload_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Delete,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `GET /api/sessions/{sessionId}/files/live/uploads/{fileUploadId}`
/// Read resumable live-upload state, auto-resuming the same generation.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_files_live_upload_get_request(
    session_id: SessionId,
    file_upload_id: FileUploadId,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionFilesLiveUploadGet;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    path.bind(&file_upload_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `PUT /api/sessions/{sessionId}/files/live/uploads/{fileUploadId}/parts/{partNumber}`
/// Put one exact logical part directly into the retained generation.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_files_live_upload_part_put_request(
    session_id: SessionId,
    file_upload_id: FileUploadId,
    part_number: u32,
    query: &SessionFilesLiveUploadPartPutQuery,
    body: &[u8],
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionFilesLiveUploadPartPut;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    path.bind(&file_upload_id);
    path.bind(&part_number);
    let mut writer = QueryWriter::new();
    writer.put("sha256", &query.sha256);
    Ok(WireRequest {
        route,
        method: HttpMethod::Put,
        path: path.finish()?,
        query: writer.finish(),
        headers: request_headers(route, None, None, None),
        body: Some(body.to_vec()),
    })
}

/// `GET /api/sessions/{sessionId}`
/// Read session metadata, including automatic lifecycle state, or its minimal deletion tombstone.
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
/// Send text and start the session's next work, automatically resuming the same suspended
/// generation.
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
/// List complete sealed messages in immutable seal-visibility order.
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

/// `POST /api/streams/{sessionId}/events/listen`
/// Listen for new session events observations.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_observations_events_listen_request(
    session_id: SessionId,
    body: &ObservationListenRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionObservationsEventsListen;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `POST /api/observations/{sessionId}/events/query`
/// Query session events observations.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_observations_events_query_request(
    session_id: SessionId,
    body: &ObservationQuery,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionObservationsEventsQuery;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `POST /api/streams/{sessionId}/events/stream`
/// Stream session events observations from one origin.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_observations_events_stream_request(
    session_id: SessionId,
    body: &ObservationStreamRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionObservationsEventsStream;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `POST /api/streams/{sessionId}/logs/listen`
/// Listen for new session logs observations.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_observations_logs_listen_request(
    session_id: SessionId,
    body: &ObservationListenRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionObservationsLogsListen;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `POST /api/observations/{sessionId}/logs/query`
/// Query session logs observations.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_observations_logs_query_request(
    session_id: SessionId,
    body: &ObservationQuery,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionObservationsLogsQuery;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `POST /api/streams/{sessionId}/logs/stream`
/// Stream session logs observations from one origin.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_observations_logs_stream_request(
    session_id: SessionId,
    body: &ObservationStreamRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionObservationsLogsStream;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `POST /api/observations/{sessionId}/metrics/aggregate`
/// Aggregate session metric observations.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_observations_metrics_aggregate_request(
    session_id: SessionId,
    body: &MetricAggregationRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionObservationsMetricsAggregate;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `POST /api/streams/{sessionId}/metrics/listen`
/// Listen for new session metrics observations.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_observations_metrics_listen_request(
    session_id: SessionId,
    body: &ObservationListenRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionObservationsMetricsListen;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `POST /api/observations/{sessionId}/metrics/query`
/// Query session metrics observations.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_observations_metrics_query_request(
    session_id: SessionId,
    body: &ObservationQuery,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionObservationsMetricsQuery;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `POST /api/streams/{sessionId}/metrics/stream`
/// Stream session metrics observations from one origin.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_observations_metrics_stream_request(
    session_id: SessionId,
    body: &ObservationStreamRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionObservationsMetricsStream;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `POST /api/streams/{sessionId}/spans/listen`
/// Listen for new session spans observations.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_observations_spans_listen_request(
    session_id: SessionId,
    body: &ObservationListenRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionObservationsSpansListen;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `POST /api/observations/{sessionId}/spans/query`
/// Query session spans observations.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_observations_spans_query_request(
    session_id: SessionId,
    body: &ObservationQuery,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionObservationsSpansQuery;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `POST /api/streams/{sessionId}/spans/stream`
/// Stream session spans observations from one origin.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_observations_spans_stream_request(
    session_id: SessionId,
    body: &ObservationStreamRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionObservationsSpansStream;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `POST /api/streams/{sessionId}/telemetry/listen`
/// Listen for new session telemetry observations.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_observations_telemetry_listen_request(
    session_id: SessionId,
    body: &ObservationListenRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionObservationsTelemetryListen;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `POST /api/observations/{sessionId}/telemetry/query`
/// Query session telemetry observations.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_observations_telemetry_query_request(
    session_id: SessionId,
    body: &ObservationQuery,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionObservationsTelemetryQuery;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `POST /api/streams/{sessionId}/telemetry/stream`
/// Stream session telemetry observations from one origin.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_observations_telemetry_stream_request(
    session_id: SessionId,
    body: &ObservationStreamRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionObservationsTelemetryStream;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `GET /api/observations/{sessionId}/traces/{traceId}`
/// Read one assembled trace by its W3C identifier.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_observations_trace_get_request(
    session_id: SessionId,
    trace_id: TraceId,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionObservationsTraceGet;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    path.bind(&trace_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `POST /api/streams/{sessionId}/traces/listen`
/// Listen for new session traces observations.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_observations_traces_listen_request(
    session_id: SessionId,
    body: &ObservationListenRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionObservationsTracesListen;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `POST /api/observations/{sessionId}/traces/query`
/// Query session traces observations.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_observations_traces_query_request(
    session_id: SessionId,
    body: &ObservationQuery,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionObservationsTracesQuery;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `POST /api/streams/{sessionId}/traces/stream`
/// Stream session traces observations from one origin.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_observations_traces_stream_request(
    session_id: SessionId,
    body: &ObservationStreamRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionObservationsTracesStream;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `POST /api/sessions/{sessionId}/resumptions`
/// Resume the same retained generation to idle.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_resume_request(
    session_id: SessionId,
    body: &EmptyRequest,
    operation_id: OperationId,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionResume;
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

/// `POST /api/sessions/{sessionId}/suspensions`
/// Suspend an idle session while retaining its exact generation; cancel active work first.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_suspend_request(
    session_id: SessionId,
    body: &EmptyRequest,
    operation_id: OperationId,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionSuspend;
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

/// `POST /api/observations/{sessionId}/telemetry/exports`
/// Admit the durable session telemetry-export operation.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_telemetry_export_create_request(
    session_id: SessionId,
    body: &TelemetryExportRequest,
    operation_id: OperationId,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionTelemetryExportCreate;
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

/// `POST /api/observations/{sessionId}/telemetry/exports/{exportId}/downloads`
/// Mint a download grant for a ready session telemetry export.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_telemetry_export_download_create_request(
    session_id: SessionId,
    export_id: ExportId,
    body: &EmptyRequest,
    idempotency_key: &IdempotencyKey,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionTelemetryExportDownloadCreate;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    path.bind(&export_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, Some(idempotency_key), None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `GET /api/observations/{sessionId}/telemetry/exports/{exportId}`
/// Read one session telemetry export record.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_telemetry_export_get_request(
    session_id: SessionId,
    export_id: ExportId,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionTelemetryExportGet;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    path.bind(&export_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `POST /api/observations/{sessionId}/telemetry/exports/{exportId}/revocations`
/// Revoke a session telemetry export and its outstanding grants.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_telemetry_export_revoke_request(
    session_id: SessionId,
    export_id: ExportId,
    body: &EmptyRequest,
    idempotency_key: &IdempotencyKey,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionTelemetryExportRevoke;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    path.bind(&export_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, Some(idempotency_key), None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `GET /api/observations/{sessionId}/telemetry/gaps/{gapId}`
/// Read one recorded session telemetry gap.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_telemetry_gap_get_request(
    session_id: SessionId,
    gap_id: TelemetryGapId,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionTelemetryGapGet;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    path.bind(&gap_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `POST /api/observations/{sessionId}/telemetry/gaps/query`
/// Query recorded session telemetry gaps.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn session_telemetry_gaps_query_request(
    session_id: SessionId,
    body: &TelemetryGapQuery,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::SessionTelemetryGapsQuery;
    let mut path = PathWriter::new(route);
    path.bind(&session_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `POST /api/sessions/{sessionId}/terminations`
/// Permanently destroy compute and live files while retaining metadata and sealed messages.
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

/// `POST /api/observations/telemetry/exports`
/// Admit the durable workspace telemetry-export operation.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn telemetry_export_create_request(
    body: &TelemetryExportRequest,
    operation_id: OperationId,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::TelemetryExportCreate;
    let path = PathWriter::new(route);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, Some(operation_id), None),
        body: Some(encode_body(route, body)?),
    })
}

/// `POST /api/observations/telemetry/exports/{exportId}/downloads`
/// Mint a download grant for a ready telemetry export.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn telemetry_export_download_create_request(
    export_id: ExportId,
    body: &EmptyRequest,
    idempotency_key: &IdempotencyKey,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::TelemetryExportDownloadCreate;
    let mut path = PathWriter::new(route);
    path.bind(&export_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, Some(idempotency_key), None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `GET /api/observations/telemetry/exports/{exportId}`
/// Read one telemetry export record.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn telemetry_export_get_request(export_id: ExportId) -> Result<WireRequest, ClientError> {
    let route = RouteId::TelemetryExportGet;
    let mut path = PathWriter::new(route);
    path.bind(&export_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `POST /api/observations/telemetry/exports/{exportId}/revocations`
/// Revoke a telemetry export and its outstanding grants.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn telemetry_export_revoke_request(
    export_id: ExportId,
    body: &EmptyRequest,
    idempotency_key: &IdempotencyKey,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::TelemetryExportRevoke;
    let mut path = PathWriter::new(route);
    path.bind(&export_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, Some(idempotency_key), None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `GET /api/observations/telemetry/gaps/{gapId}`
/// Read one recorded telemetry gap.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn telemetry_gap_get_request(gap_id: TelemetryGapId) -> Result<WireRequest, ClientError> {
    let route = RouteId::TelemetryGapGet;
    let mut path = PathWriter::new(route);
    path.bind(&gap_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `POST /api/observations/telemetry/gaps/query`
/// Query recorded workspace telemetry gaps.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn telemetry_gaps_query_request(body: &TelemetryGapQuery) -> Result<WireRequest, ClientError> {
    let route = RouteId::TelemetryGapsQuery;
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

/// `DELETE /api/workspace/uploads/{uploadId}`
/// Abort a staged upload.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn upload_abort_request(upload_id: UploadId) -> Result<WireRequest, ClientError> {
    let route = RouteId::UploadAbort;
    let mut path = PathWriter::new(route);
    path.bind(&upload_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Delete,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `POST /api/workspace/uploads/{uploadId}/completion`
/// Complete a staged upload.
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

/// `POST /api/workspace/uploads`
/// Stage a large registered-resource value.
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

/// `POST /api/workspace/uploads/{uploadId}/parts`
/// Mint presigned PUT grants for the named parts.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn upload_parts_grant_request(
    upload_id: UploadId,
    body: &UploadPartsRequest,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::UploadPartsGrant;
    let mut path = PathWriter::new(route);
    path.bind(&upload_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `POST /api/billing/usage/query`
/// Query rated usage for this workspace.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn usage_query_request(
    query: &UsageQueryQuery,
    body: &UsageQuery,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::UsageQuery;
    let path = PathWriter::new(route);
    let mut writer = QueryWriter::new();
    writer.put_option("workspaceId", query.workspace_id.as_ref());
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: writer.finish(),
        headers: request_headers(route, None, None, None),
        body: Some(encode_body(route, body)?),
    })
}

/// `POST /api/workspaces`
/// Create a region-pinned workspace.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn workspace_create_request(
    body: &WorkspaceCreateRequest,
    idempotency_key: &IdempotencyKey,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::WorkspaceCreate;
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

/// `GET /api/workspace`
/// Read the workspace this credential is pinned to.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn workspace_current_get_request() -> Result<WireRequest, ClientError> {
    let route = RouteId::WorkspaceCurrentGet;
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

/// `POST /api/workspaces/{workspaceId}/deletions`
/// Admit the global workspace-deletion operation.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn workspace_delete_request(
    workspace_id: WorkspaceId,
    body: &WorkspaceDeleteRequest,
    operation_id: OperationId,
) -> Result<WireRequest, ClientError> {
    let route = RouteId::WorkspaceDelete;
    let mut path = PathWriter::new(route);
    path.bind(&workspace_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Post,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, Some(operation_id), None),
        body: Some(encode_body(route, body)?),
    })
}

/// `GET /api/workspaces/{workspaceId}`
/// Read one workspace.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn workspace_get_request(workspace_id: WorkspaceId) -> Result<WireRequest, ClientError> {
    let route = RouteId::WorkspaceGet;
    let mut path = PathWriter::new(route);
    path.bind(&workspace_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `GET /api/workspace/limits/{limitId}`
/// Read one effective workspace safety limit.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn workspace_limit_get_request(limit_id: LimitId) -> Result<WireRequest, ClientError> {
    let route = RouteId::WorkspaceLimitGet;
    let mut path = PathWriter::new(route);
    path.bind(&limit_id);
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: String::new(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

/// `GET /api/workspace/limits`
/// List the effective workspace safety limits. The registry is closed and complete, so the whole
/// set is one page and no continuation is ever minted.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn workspace_limits_list_request() -> Result<WireRequest, ClientError> {
    let route = RouteId::WorkspaceLimitsList;
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

/// `GET /api/workspaces`
/// List workspaces the caller can reach.
///
/// Built without executing it, so a caller that needs its own transport — a frame stream, a proxy,
/// a recorded fixture — can take the request and run it.
///
/// # Errors
/// Returns [`ClientError::Encode`] when the request cannot be rendered.
pub fn workspaces_list_request(query: &WorkspacesListQuery) -> Result<WireRequest, ClientError> {
    let route = RouteId::WorkspacesList;
    let path = PathWriter::new(route);
    let mut writer = QueryWriter::new();
    writer.put_option("cursor", query.cursor.as_ref());
    writer.put_option("limit", query.limit.as_ref());
    writer.put_option("organizationId", query.organization_id.as_ref());
    Ok(WireRequest {
        route,
        method: HttpMethod::Get,
        path: path.finish()?,
        query: writer.finish(),
        headers: request_headers(route, None, None, None),
        body: None,
    })
}

// --- client methods -------------------------------------------------------

/// One method per public operation, over whichever transport the caller injected.
impl<T: Transport> WireClient<T> {
    /// `GET /api/account`
    /// Read the account and its operational state.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn account_get(
        &self,
        query: &AccountGetQuery,
    ) -> Result<AccountOperationalState, ClientError> {
        let request = account_get_request(query)?;
        let response = self.send(request).await?;
        decode_response(RouteId::AccountGet, &response)
    }

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

    /// `GET /api/organizations/{organizationId}/billing/auto-topup-policy`
    /// Read the automatic top-up policy.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn billing_auto_topup_policy_get(
        &self,
        organization_id: OrganizationId,
    ) -> Result<WithETag<AutoTopupPolicy>, ClientError> {
        let request = billing_auto_topup_policy_get_request(organization_id)?;
        let response = self.send(request).await?;
        decode_response_with_etag(RouteId::BillingAutoTopupPolicyGet, &response)
    }

    /// `PUT /api/organizations/{organizationId}/billing/auto-topup-policy`
    /// Replace the automatic top-up policy.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn billing_auto_topup_policy_put(
        &self,
        organization_id: OrganizationId,
        body: &AutoTopupPolicyRequest,
        idempotency_key: &IdempotencyKey,
        if_match: &ETag,
    ) -> Result<WithETag<AutoTopupPolicy>, ClientError> {
        let request = billing_auto_topup_policy_put_request(
            organization_id,
            body,
            idempotency_key,
            if_match,
        )?;
        let response = self.send(request).await?;
        decode_response_with_etag(RouteId::BillingAutoTopupPolicyPut, &response)
    }

    /// `GET /api/billing/balance`
    /// Read the prepaid balance of an organization.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn billing_balance_get(
        &self,
        query: &BillingBalanceGetQuery,
    ) -> Result<BillingBalance, ClientError> {
        let request = billing_balance_get_request(query)?;
        let response = self.send(request).await?;
        decode_response(RouteId::BillingBalanceGet, &response)
    }

    /// `POST /api/organizations/{organizationId}/billing/portal-sessions`
    /// Create a hosted billing portal session.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn billing_portal_session_create(
        &self,
        organization_id: OrganizationId,
        body: &PortalSessionRequest,
        idempotency_key: &IdempotencyKey,
    ) -> Result<HostedSession, ClientError> {
        let request =
            billing_portal_session_create_request(organization_id, body, idempotency_key)?;
        let response = self.send(request).await?;
        decode_response(RouteId::BillingPortalSessionCreate, &response)
    }

    /// `POST /api/organizations/{organizationId}/billing/statements/{statementId}/downloads`
    /// Mint a download grant for an issued statement.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn billing_statement_download_create(
        &self,
        organization_id: OrganizationId,
        statement_id: StatementId,
        body: &EmptyRequest,
        idempotency_key: &IdempotencyKey,
    ) -> Result<DownloadGrant, ClientError> {
        let request = billing_statement_download_create_request(
            organization_id,
            statement_id,
            body,
            idempotency_key,
        )?;
        let response = self.send(request).await?;
        decode_response(RouteId::BillingStatementDownloadCreate, &response)
    }

    /// `GET /api/organizations/{organizationId}/billing/statements/{statementId}`
    /// Read one immutable issued statement.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn billing_statement_get(
        &self,
        organization_id: OrganizationId,
        statement_id: StatementId,
    ) -> Result<Statement, ClientError> {
        let request = billing_statement_get_request(organization_id, statement_id)?;
        let response = self.send(request).await?;
        decode_response(RouteId::BillingStatementGet, &response)
    }

    /// `GET /api/organizations/{organizationId}/billing/statements`
    /// List issued statements, newest first.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn billing_statements_list(
        &self,
        organization_id: OrganizationId,
        query: &BillingStatementsListQuery,
    ) -> Result<StatementSummaryPage, ClientError> {
        let request = billing_statements_list_request(organization_id, query)?;
        let response = self.send(request).await?;
        decode_response(RouteId::BillingStatementsList, &response)
    }

    /// `POST /api/organizations/{organizationId}/billing/top-up-checkouts`
    /// Create a hosted top-up checkout.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn billing_top_up_checkout_create(
        &self,
        organization_id: OrganizationId,
        body: &TopUpCheckoutRequest,
        idempotency_key: &IdempotencyKey,
    ) -> Result<HostedSession, ClientError> {
        let request =
            billing_top_up_checkout_create_request(organization_id, body, idempotency_key)?;
        let response = self.send(request).await?;
        decode_response(RouteId::BillingTopUpCheckoutCreate, &response)
    }

    /// `POST /api/operations/{operationId}/cancellations`
    /// Request cancellation of a central operation.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn central_operation_cancel(
        &self,
        operation_id: OperationId,
        body: &EmptyRequest,
    ) -> Result<Operation, ClientError> {
        let request = central_operation_cancel_request(operation_id, body)?;
        let response = self.send(request).await?;
        decode_response(RouteId::CentralOperationCancel, &response)
    }

    /// `GET /api/operations/{operationId}`
    /// Read one central durable operation.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn central_operation_get(
        &self,
        operation_id: OperationId,
    ) -> Result<Operation, ClientError> {
        let request = central_operation_get_request(operation_id)?;
        let response = self.send(request).await?;
        decode_response(RouteId::CentralOperationGet, &response)
    }

    /// `GET /api/operations`
    /// List central durable operations for one organization.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn central_operations_list(
        &self,
        query: &CentralOperationsListQuery,
    ) -> Result<OperationPage, ClientError> {
        let request = central_operations_list_request(query)?;
        let response = self.send(request).await?;
        decode_response(RouteId::CentralOperationsList, &response)
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
    /// Exchange a provider authorization code for a browser session.
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
    /// Close the browser session the caller presented.
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

    /// `POST /api/auth/device/authorizations`
    /// Begin the CLI device-authorization flow.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn device_authorization_create(
        &self,
        body: &DeviceAuthorizationRequest,
        idempotency_key: &IdempotencyKey,
    ) -> Result<DeviceAuthorization, ClientError> {
        let request = device_authorization_create_request(body, idempotency_key)?;
        let response = self.send(request).await?;
        decode_response(RouteId::DeviceAuthorizationCreate, &response)
    }

    /// `POST /api/auth/device/decisions`
    /// Approve or deny a pending device authorization.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn device_decision_create(
        &self,
        body: &DeviceDecisionRequest,
    ) -> Result<DeviceDecisionResult, ClientError> {
        let request = device_decision_create_request(body)?;
        let response = self.send(request).await?;
        decode_response(RouteId::DeviceDecisionCreate, &response)
    }

    /// `POST /api/auth/device/tokens`
    /// Exchange an approved device code for an account token.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn device_token_create(
        &self,
        body: &DeviceTokenRequest,
    ) -> Result<DeviceToken, ClientError> {
        let request = device_token_create_request(body)?;
        let response = self.send(request).await?;
        decode_response(RouteId::DeviceTokenCreate, &response)
    }

    /// `POST /api/invitations/acceptances`
    /// Redeem every pending invitation addressed to the caller's verified email.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn invitation_accept(
        &self,
        body: &EmptyRequest,
    ) -> Result<InvitationAcceptResult, ClientError> {
        let request = invitation_accept_request(body)?;
        let response = self.send(request).await?;
        decode_response(RouteId::InvitationAccept, &response)
    }

    /// `POST /api/organizations/{organizationId}/invitations`
    /// Invite a person to the organization.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn invitation_create(
        &self,
        organization_id: OrganizationId,
        body: &InvitationCreateRequest,
        idempotency_key: &IdempotencyKey,
    ) -> Result<Invitation, ClientError> {
        let request = invitation_create_request(organization_id, body, idempotency_key)?;
        let response = self.send(request).await?;
        decode_response(RouteId::InvitationCreate, &response)
    }

    /// `GET /api/organizations/{organizationId}/memberships`
    /// List the memberships of an organization.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn memberships_list(
        &self,
        organization_id: OrganizationId,
        query: &MembershipsListQuery,
    ) -> Result<MembershipPage, ClientError> {
        let request = memberships_list_request(organization_id, query)?;
        let response = self.send(request).await?;
        decode_response(RouteId::MembershipsList, &response)
    }

    /// `POST /api/streams/events/listen`
    /// Listen for new workspace events observations.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn observations_events_listen(
        &self,
        body: &ObservationListenRequest,
    ) -> Result<NdjsonFrames<ObservationFrame>, ClientError> {
        let request = observations_events_listen_request(body)?;
        let response = self.send(request).await?;
        decode_ndjson(RouteId::ObservationsEventsListen, response)
    }

    /// `POST /api/observations/events/query`
    /// Query workspace events observations.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn observations_events_query(
        &self,
        body: &ObservationQuery,
    ) -> Result<ObservationPage, ClientError> {
        let request = observations_events_query_request(body)?;
        let response = self.send(request).await?;
        decode_response(RouteId::ObservationsEventsQuery, &response)
    }

    /// `POST /api/streams/events/stream`
    /// Stream workspace events observations from one origin.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn observations_events_stream(
        &self,
        body: &ObservationStreamRequest,
    ) -> Result<NdjsonFrames<ObservationFrame>, ClientError> {
        let request = observations_events_stream_request(body)?;
        let response = self.send(request).await?;
        decode_ndjson(RouteId::ObservationsEventsStream, response)
    }

    /// `POST /api/streams/logs/listen`
    /// Listen for new workspace logs observations.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn observations_logs_listen(
        &self,
        body: &ObservationListenRequest,
    ) -> Result<NdjsonFrames<ObservationFrame>, ClientError> {
        let request = observations_logs_listen_request(body)?;
        let response = self.send(request).await?;
        decode_ndjson(RouteId::ObservationsLogsListen, response)
    }

    /// `POST /api/observations/logs/query`
    /// Query workspace logs observations.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn observations_logs_query(
        &self,
        body: &ObservationQuery,
    ) -> Result<ObservationPage, ClientError> {
        let request = observations_logs_query_request(body)?;
        let response = self.send(request).await?;
        decode_response(RouteId::ObservationsLogsQuery, &response)
    }

    /// `POST /api/streams/logs/stream`
    /// Stream workspace logs observations from one origin.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn observations_logs_stream(
        &self,
        body: &ObservationStreamRequest,
    ) -> Result<NdjsonFrames<ObservationFrame>, ClientError> {
        let request = observations_logs_stream_request(body)?;
        let response = self.send(request).await?;
        decode_ndjson(RouteId::ObservationsLogsStream, response)
    }

    /// `POST /api/observations/metrics/aggregate`
    /// Aggregate workspace metric observations.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn observations_metrics_aggregate(
        &self,
        body: &MetricAggregationRequest,
    ) -> Result<MetricAggregationPage, ClientError> {
        let request = observations_metrics_aggregate_request(body)?;
        let response = self.send(request).await?;
        decode_response(RouteId::ObservationsMetricsAggregate, &response)
    }

    /// `POST /api/streams/metrics/listen`
    /// Listen for new workspace metrics observations.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn observations_metrics_listen(
        &self,
        body: &ObservationListenRequest,
    ) -> Result<NdjsonFrames<ObservationFrame>, ClientError> {
        let request = observations_metrics_listen_request(body)?;
        let response = self.send(request).await?;
        decode_ndjson(RouteId::ObservationsMetricsListen, response)
    }

    /// `POST /api/observations/metrics/query`
    /// Query workspace metrics observations.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn observations_metrics_query(
        &self,
        body: &ObservationQuery,
    ) -> Result<ObservationPage, ClientError> {
        let request = observations_metrics_query_request(body)?;
        let response = self.send(request).await?;
        decode_response(RouteId::ObservationsMetricsQuery, &response)
    }

    /// `POST /api/streams/metrics/stream`
    /// Stream workspace metrics observations from one origin.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn observations_metrics_stream(
        &self,
        body: &ObservationStreamRequest,
    ) -> Result<NdjsonFrames<ObservationFrame>, ClientError> {
        let request = observations_metrics_stream_request(body)?;
        let response = self.send(request).await?;
        decode_ndjson(RouteId::ObservationsMetricsStream, response)
    }

    /// `POST /api/streams/spans/listen`
    /// Listen for new workspace spans observations.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn observations_spans_listen(
        &self,
        body: &ObservationListenRequest,
    ) -> Result<NdjsonFrames<ObservationFrame>, ClientError> {
        let request = observations_spans_listen_request(body)?;
        let response = self.send(request).await?;
        decode_ndjson(RouteId::ObservationsSpansListen, response)
    }

    /// `POST /api/observations/spans/query`
    /// Query workspace spans observations.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn observations_spans_query(
        &self,
        body: &ObservationQuery,
    ) -> Result<ObservationPage, ClientError> {
        let request = observations_spans_query_request(body)?;
        let response = self.send(request).await?;
        decode_response(RouteId::ObservationsSpansQuery, &response)
    }

    /// `POST /api/streams/spans/stream`
    /// Stream workspace spans observations from one origin.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn observations_spans_stream(
        &self,
        body: &ObservationStreamRequest,
    ) -> Result<NdjsonFrames<ObservationFrame>, ClientError> {
        let request = observations_spans_stream_request(body)?;
        let response = self.send(request).await?;
        decode_ndjson(RouteId::ObservationsSpansStream, response)
    }

    /// `POST /api/streams/telemetry/listen`
    /// Listen for new workspace telemetry observations.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn observations_telemetry_listen(
        &self,
        body: &ObservationListenRequest,
    ) -> Result<NdjsonFrames<ObservationFrame>, ClientError> {
        let request = observations_telemetry_listen_request(body)?;
        let response = self.send(request).await?;
        decode_ndjson(RouteId::ObservationsTelemetryListen, response)
    }

    /// `POST /api/observations/telemetry/query`
    /// Query workspace telemetry observations.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn observations_telemetry_query(
        &self,
        body: &ObservationQuery,
    ) -> Result<ObservationPage, ClientError> {
        let request = observations_telemetry_query_request(body)?;
        let response = self.send(request).await?;
        decode_response(RouteId::ObservationsTelemetryQuery, &response)
    }

    /// `POST /api/streams/telemetry/stream`
    /// Stream workspace telemetry observations from one origin.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn observations_telemetry_stream(
        &self,
        body: &ObservationStreamRequest,
    ) -> Result<NdjsonFrames<ObservationFrame>, ClientError> {
        let request = observations_telemetry_stream_request(body)?;
        let response = self.send(request).await?;
        decode_ndjson(RouteId::ObservationsTelemetryStream, response)
    }

    /// `POST /api/streams/traces/listen`
    /// Listen for new workspace traces observations.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn observations_traces_listen(
        &self,
        body: &ObservationListenRequest,
    ) -> Result<NdjsonFrames<ObservationFrame>, ClientError> {
        let request = observations_traces_listen_request(body)?;
        let response = self.send(request).await?;
        decode_ndjson(RouteId::ObservationsTracesListen, response)
    }

    /// `POST /api/observations/traces/query`
    /// Query workspace traces observations.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn observations_traces_query(
        &self,
        body: &ObservationQuery,
    ) -> Result<ObservationPage, ClientError> {
        let request = observations_traces_query_request(body)?;
        let response = self.send(request).await?;
        decode_response(RouteId::ObservationsTracesQuery, &response)
    }

    /// `POST /api/streams/traces/stream`
    /// Stream workspace traces observations from one origin.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn observations_traces_stream(
        &self,
        body: &ObservationStreamRequest,
    ) -> Result<NdjsonFrames<ObservationFrame>, ClientError> {
        let request = observations_traces_stream_request(body)?;
        let response = self.send(request).await?;
        decode_ndjson(RouteId::ObservationsTracesStream, response)
    }

    /// `POST /api/organizations`
    /// Create an organization whose creator becomes owner.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn organization_create(
        &self,
        body: &OrganizationCreateRequest,
        idempotency_key: &IdempotencyKey,
    ) -> Result<Organization, ClientError> {
        let request = organization_create_request(body, idempotency_key)?;
        let response = self.send(request).await?;
        decode_response(RouteId::OrganizationCreate, &response)
    }

    /// `GET /api/organizations/{organizationId}`
    /// Read one organization.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn organization_get(
        &self,
        organization_id: OrganizationId,
    ) -> Result<Organization, ClientError> {
        let request = organization_get_request(organization_id)?;
        let response = self.send(request).await?;
        decode_response(RouteId::OrganizationGet, &response)
    }

    /// `GET /api/organizations`
    /// List organizations the caller belongs to.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn organizations_list(
        &self,
        query: &OrganizationsListQuery,
    ) -> Result<OrganizationPage, ClientError> {
        let request = organizations_list_request(query)?;
        let response = self.send(request).await?;
        decode_response(RouteId::OrganizationsList, &response)
    }

    /// `POST /api/otlp/v1/logs`
    /// Admit an OTLP logs batch.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn otlp_logs_ingest(
        &self,
        body: &[u8],
        idempotency_key: &IdempotencyKey,
    ) -> Result<TelemetryAdmissionReceipt, ClientError> {
        let request = otlp_logs_ingest_request(body, idempotency_key)?;
        let response = self.send(request).await?;
        decode_response(RouteId::OtlpLogsIngest, &response)
    }

    /// `POST /api/otlp/v1/metrics`
    /// Admit an OTLP metrics batch.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn otlp_metrics_ingest(
        &self,
        body: &[u8],
        idempotency_key: &IdempotencyKey,
    ) -> Result<TelemetryAdmissionReceipt, ClientError> {
        let request = otlp_metrics_ingest_request(body, idempotency_key)?;
        let response = self.send(request).await?;
        decode_response(RouteId::OtlpMetricsIngest, &response)
    }

    /// `POST /api/otlp/v1/traces`
    /// Admit an OTLP traces batch.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn otlp_traces_ingest(
        &self,
        body: &[u8],
        idempotency_key: &IdempotencyKey,
    ) -> Result<TelemetryAdmissionReceipt, ClientError> {
        let request = otlp_traces_ingest_request(body, idempotency_key)?;
        let response = self.send(request).await?;
        decode_response(RouteId::OtlpTracesIngest, &response)
    }

    /// `GET /api/workspace/provider-credentials/{providerCredentialId}`
    /// Read one BYOK provider-credential binding.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn provider_credential_get(
        &self,
        provider_credential_id: ProviderCredentialId,
    ) -> Result<WithETag<ProviderCredential>, ClientError> {
        let request = provider_credential_get_request(provider_credential_id)?;
        let response = self.send(request).await?;
        decode_response_with_etag(RouteId::ProviderCredentialGet, &response)
    }

    /// `POST /api/workspace/provider-credentials`
    /// Register a dedicated BYOK provider credential; carries plaintext once.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn provider_credential_register(
        &self,
        body: &ProviderCredentialRegisterRequest,
        idempotency_key: &IdempotencyKey,
    ) -> Result<ProviderCredential, ClientError> {
        let request = provider_credential_register_request(body, idempotency_key)?;
        let response = self.send(request).await?;
        decode_response(RouteId::ProviderCredentialRegister, &response)
    }

    /// `POST /api/workspace/provider-credentials/{providerCredentialId}/revocations`
    /// Revoke a BYOK provider-credential binding.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn provider_credential_revoke(
        &self,
        provider_credential_id: ProviderCredentialId,
        body: &EmptyRequest,
        idempotency_key: &IdempotencyKey,
    ) -> Result<ProviderCredential, ClientError> {
        let request =
            provider_credential_revoke_request(provider_credential_id, body, idempotency_key)?;
        let response = self.send(request).await?;
        decode_response(RouteId::ProviderCredentialRevoke, &response)
    }

    /// `GET /api/workspace/provider-credentials`
    /// List BYOK provider-credential binding metadata.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn provider_credentials_list(
        &self,
        query: &ProviderCredentialsListQuery,
    ) -> Result<ProviderCredentialPage, ClientError> {
        let request = provider_credentials_list_request(query)?;
        let response = self.send(request).await?;
        decode_response(RouteId::ProviderCredentialsList, &response)
    }

    /// `POST /api/operations/{operationId}/cancellations`
    /// Request cancellation of a regional operation.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn regional_operation_cancel(
        &self,
        operation_id: OperationId,
        body: &EmptyRequest,
    ) -> Result<Operation, ClientError> {
        let request = regional_operation_cancel_request(operation_id, body)?;
        let response = self.send(request).await?;
        decode_response(RouteId::RegionalOperationCancel, &response)
    }

    /// `GET /api/operations/{operationId}`
    /// Read one regional durable operation.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn regional_operation_get(
        &self,
        operation_id: OperationId,
    ) -> Result<Operation, ClientError> {
        let request = regional_operation_get_request(operation_id)?;
        let response = self.send(request).await?;
        decode_response(RouteId::RegionalOperationGet, &response)
    }

    /// `GET /api/operations`
    /// List regional durable operations.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn regional_operations_list(
        &self,
        query: &RegionalOperationsListQuery,
    ) -> Result<OperationPage, ClientError> {
        let request = regional_operations_list_request(query)?;
        let response = self.send(request).await?;
        decode_response(RouteId::RegionalOperationsList, &response)
    }

    /// `DELETE /api/workspace/files/{name}`
    /// Delete one registered workspace file.
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

    /// `POST /api/workspace/files/{name}/downloads`
    /// Mint a download grant for a registered workspace file.
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

    /// `GET /api/workspace/files/{name}`
    /// Read one registered workspace file.
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

    /// `GET /api/workspace/files`
    /// List registered workspace files.
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

    /// `PUT /api/workspace/files/{name}`
    /// Replace one registered workspace file.
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
    ) -> Result<Operation, ClientError> {
        let request = session_cancel_request(session_id, body, operation_id)?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionCancel, &response)
    }

    /// `POST /api/sessions`
    /// Create an eight-hour multi-turn session.
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
    /// Irreversibly delete the session and session-scoped user content, observations, telemetry,
    /// and export objects. Independent registered workspace files remain.
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
    ) -> Result<Operation, ClientError> {
        let request = session_delete_request(session_id, body, operation_id)?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionDelete, &response)
    }

    /// `POST /api/sessions/{sessionId}/files/live/downloads/{fileDownloadId}/completion`
    /// Re-stat, re-hash, and close an exact live-file download after SDK verification.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_files_live_download_complete(
        &self,
        session_id: SessionId,
        file_download_id: FileDownloadId,
        body: &LiveFileDownloadCompleteRequest,
        idempotency_key: &IdempotencyKey,
    ) -> Result<LiveFileDownload, ClientError> {
        let request = session_files_live_download_complete_request(
            session_id,
            file_download_id,
            body,
            idempotency_key,
        )?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionFilesLiveDownloadComplete, &response)
    }

    /// `POST /api/sessions/{sessionId}/files/live/downloads`
    /// Open one exact-version live file directly from the retained generation, auto-resuming it.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_files_live_download_create(
        &self,
        session_id: SessionId,
        body: &LiveFileDownloadRequest,
        idempotency_key: &IdempotencyKey,
    ) -> Result<LiveFileDownload, ClientError> {
        let request =
            session_files_live_download_create_request(session_id, body, idempotency_key)?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionFilesLiveDownloadCreate, &response)
    }

    /// `DELETE /api/sessions/{sessionId}/files/live/downloads/{fileDownloadId}`
    /// Close an abandoned live-file descriptor, auto-resuming the same generation.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_files_live_download_delete(
        &self,
        session_id: SessionId,
        file_download_id: FileDownloadId,
    ) -> Result<(), ClientError> {
        let request = session_files_live_download_delete_request(session_id, file_download_id)?;
        let response = self.send(request).await?;
        decode_no_content(RouteId::SessionFilesLiveDownloadDelete, &response)
    }

    /// `GET /api/sessions/{sessionId}/files/live/downloads/{fileDownloadId}/parts/{partNumber}`
    /// Read one raw exact-version range directly from the retained generation.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_files_live_download_part_get(
        &self,
        session_id: SessionId,
        file_download_id: FileDownloadId,
        part_number: u32,
        query: &SessionFilesLiveDownloadPartGetQuery,
    ) -> Result<Vec<u8>, ClientError> {
        let request = session_files_live_download_part_get_request(
            session_id,
            file_download_id,
            part_number,
            query,
        )?;
        let response = self.send(request).await?;
        decode_binary(RouteId::SessionFilesLiveDownloadPartGet, response)
    }

    /// `POST /api/sessions/{sessionId}/files/live/list`
    /// List the exact live generation, auto-resuming that same generation when suspended.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_files_live_list(
        &self,
        session_id: SessionId,
        body: &LiveFileListRequest,
    ) -> Result<LiveFileEntryPage, ClientError> {
        let request = session_files_live_list_request(session_id, body)?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionFilesLiveList, &response)
    }

    /// `POST /api/sessions/{sessionId}/files/live/stat`
    /// Stat one live path without following a symlink, auto-resuming the same generation.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_files_live_stat(
        &self,
        session_id: SessionId,
        body: &LiveFileStatRequest,
    ) -> Result<LiveFileEntry, ClientError> {
        let request = session_files_live_stat_request(session_id, body)?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionFilesLiveStat, &response)
    }

    /// `POST /api/sessions/{sessionId}/files/live/uploads/{fileUploadId}/completion`
    /// Verify every part and atomically publish one live file in the same generation.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_files_live_upload_complete(
        &self,
        session_id: SessionId,
        file_upload_id: FileUploadId,
        idempotency_key: &IdempotencyKey,
    ) -> Result<LiveFileUpload, ClientError> {
        let request = session_files_live_upload_complete_request(
            session_id,
            file_upload_id,
            idempotency_key,
        )?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionFilesLiveUploadComplete, &response)
    }

    /// `POST /api/sessions/{sessionId}/files/live/uploads`
    /// Create a resumable upload in the exact live generation, auto-resuming it when suspended.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_files_live_upload_create(
        &self,
        session_id: SessionId,
        body: &LiveFileUploadCreateRequest,
        idempotency_key: &IdempotencyKey,
    ) -> Result<LiveFileUpload, ClientError> {
        let request = session_files_live_upload_create_request(session_id, body, idempotency_key)?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionFilesLiveUploadCreate, &response)
    }

    /// `DELETE /api/sessions/{sessionId}/files/live/uploads/{fileUploadId}`
    /// Abort a partial upload, auto-resuming the same generation to remove its temporary bytes.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_files_live_upload_delete(
        &self,
        session_id: SessionId,
        file_upload_id: FileUploadId,
    ) -> Result<(), ClientError> {
        let request = session_files_live_upload_delete_request(session_id, file_upload_id)?;
        let response = self.send(request).await?;
        decode_no_content(RouteId::SessionFilesLiveUploadDelete, &response)
    }

    /// `GET /api/sessions/{sessionId}/files/live/uploads/{fileUploadId}`
    /// Read resumable live-upload state, auto-resuming the same generation.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_files_live_upload_get(
        &self,
        session_id: SessionId,
        file_upload_id: FileUploadId,
    ) -> Result<LiveFileUpload, ClientError> {
        let request = session_files_live_upload_get_request(session_id, file_upload_id)?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionFilesLiveUploadGet, &response)
    }

    /// `PUT /api/sessions/{sessionId}/files/live/uploads/{fileUploadId}/parts/{partNumber}`
    /// Put one exact logical part directly into the retained generation.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_files_live_upload_part_put(
        &self,
        session_id: SessionId,
        file_upload_id: FileUploadId,
        part_number: u32,
        query: &SessionFilesLiveUploadPartPutQuery,
        body: &[u8],
    ) -> Result<LiveFileUpload, ClientError> {
        let request = session_files_live_upload_part_put_request(
            session_id,
            file_upload_id,
            part_number,
            query,
            body,
        )?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionFilesLiveUploadPartPut, &response)
    }

    /// `GET /api/sessions/{sessionId}`
    /// Read session metadata, including automatic lifecycle state, or its minimal deletion
    /// tombstone.
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
    /// Send text and start the session's next work, automatically resuming the same suspended
    /// generation.
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
    /// List complete sealed messages in immutable seal-visibility order.
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

    /// `POST /api/streams/{sessionId}/events/listen`
    /// Listen for new session events observations.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_observations_events_listen(
        &self,
        session_id: SessionId,
        body: &ObservationListenRequest,
    ) -> Result<NdjsonFrames<ObservationFrame>, ClientError> {
        let request = session_observations_events_listen_request(session_id, body)?;
        let response = self.send(request).await?;
        decode_ndjson(RouteId::SessionObservationsEventsListen, response)
    }

    /// `POST /api/observations/{sessionId}/events/query`
    /// Query session events observations.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_observations_events_query(
        &self,
        session_id: SessionId,
        body: &ObservationQuery,
    ) -> Result<ObservationPage, ClientError> {
        let request = session_observations_events_query_request(session_id, body)?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionObservationsEventsQuery, &response)
    }

    /// `POST /api/streams/{sessionId}/events/stream`
    /// Stream session events observations from one origin.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_observations_events_stream(
        &self,
        session_id: SessionId,
        body: &ObservationStreamRequest,
    ) -> Result<NdjsonFrames<ObservationFrame>, ClientError> {
        let request = session_observations_events_stream_request(session_id, body)?;
        let response = self.send(request).await?;
        decode_ndjson(RouteId::SessionObservationsEventsStream, response)
    }

    /// `POST /api/streams/{sessionId}/logs/listen`
    /// Listen for new session logs observations.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_observations_logs_listen(
        &self,
        session_id: SessionId,
        body: &ObservationListenRequest,
    ) -> Result<NdjsonFrames<ObservationFrame>, ClientError> {
        let request = session_observations_logs_listen_request(session_id, body)?;
        let response = self.send(request).await?;
        decode_ndjson(RouteId::SessionObservationsLogsListen, response)
    }

    /// `POST /api/observations/{sessionId}/logs/query`
    /// Query session logs observations.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_observations_logs_query(
        &self,
        session_id: SessionId,
        body: &ObservationQuery,
    ) -> Result<ObservationPage, ClientError> {
        let request = session_observations_logs_query_request(session_id, body)?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionObservationsLogsQuery, &response)
    }

    /// `POST /api/streams/{sessionId}/logs/stream`
    /// Stream session logs observations from one origin.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_observations_logs_stream(
        &self,
        session_id: SessionId,
        body: &ObservationStreamRequest,
    ) -> Result<NdjsonFrames<ObservationFrame>, ClientError> {
        let request = session_observations_logs_stream_request(session_id, body)?;
        let response = self.send(request).await?;
        decode_ndjson(RouteId::SessionObservationsLogsStream, response)
    }

    /// `POST /api/observations/{sessionId}/metrics/aggregate`
    /// Aggregate session metric observations.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_observations_metrics_aggregate(
        &self,
        session_id: SessionId,
        body: &MetricAggregationRequest,
    ) -> Result<MetricAggregationPage, ClientError> {
        let request = session_observations_metrics_aggregate_request(session_id, body)?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionObservationsMetricsAggregate, &response)
    }

    /// `POST /api/streams/{sessionId}/metrics/listen`
    /// Listen for new session metrics observations.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_observations_metrics_listen(
        &self,
        session_id: SessionId,
        body: &ObservationListenRequest,
    ) -> Result<NdjsonFrames<ObservationFrame>, ClientError> {
        let request = session_observations_metrics_listen_request(session_id, body)?;
        let response = self.send(request).await?;
        decode_ndjson(RouteId::SessionObservationsMetricsListen, response)
    }

    /// `POST /api/observations/{sessionId}/metrics/query`
    /// Query session metrics observations.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_observations_metrics_query(
        &self,
        session_id: SessionId,
        body: &ObservationQuery,
    ) -> Result<ObservationPage, ClientError> {
        let request = session_observations_metrics_query_request(session_id, body)?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionObservationsMetricsQuery, &response)
    }

    /// `POST /api/streams/{sessionId}/metrics/stream`
    /// Stream session metrics observations from one origin.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_observations_metrics_stream(
        &self,
        session_id: SessionId,
        body: &ObservationStreamRequest,
    ) -> Result<NdjsonFrames<ObservationFrame>, ClientError> {
        let request = session_observations_metrics_stream_request(session_id, body)?;
        let response = self.send(request).await?;
        decode_ndjson(RouteId::SessionObservationsMetricsStream, response)
    }

    /// `POST /api/streams/{sessionId}/spans/listen`
    /// Listen for new session spans observations.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_observations_spans_listen(
        &self,
        session_id: SessionId,
        body: &ObservationListenRequest,
    ) -> Result<NdjsonFrames<ObservationFrame>, ClientError> {
        let request = session_observations_spans_listen_request(session_id, body)?;
        let response = self.send(request).await?;
        decode_ndjson(RouteId::SessionObservationsSpansListen, response)
    }

    /// `POST /api/observations/{sessionId}/spans/query`
    /// Query session spans observations.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_observations_spans_query(
        &self,
        session_id: SessionId,
        body: &ObservationQuery,
    ) -> Result<ObservationPage, ClientError> {
        let request = session_observations_spans_query_request(session_id, body)?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionObservationsSpansQuery, &response)
    }

    /// `POST /api/streams/{sessionId}/spans/stream`
    /// Stream session spans observations from one origin.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_observations_spans_stream(
        &self,
        session_id: SessionId,
        body: &ObservationStreamRequest,
    ) -> Result<NdjsonFrames<ObservationFrame>, ClientError> {
        let request = session_observations_spans_stream_request(session_id, body)?;
        let response = self.send(request).await?;
        decode_ndjson(RouteId::SessionObservationsSpansStream, response)
    }

    /// `POST /api/streams/{sessionId}/telemetry/listen`
    /// Listen for new session telemetry observations.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_observations_telemetry_listen(
        &self,
        session_id: SessionId,
        body: &ObservationListenRequest,
    ) -> Result<NdjsonFrames<ObservationFrame>, ClientError> {
        let request = session_observations_telemetry_listen_request(session_id, body)?;
        let response = self.send(request).await?;
        decode_ndjson(RouteId::SessionObservationsTelemetryListen, response)
    }

    /// `POST /api/observations/{sessionId}/telemetry/query`
    /// Query session telemetry observations.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_observations_telemetry_query(
        &self,
        session_id: SessionId,
        body: &ObservationQuery,
    ) -> Result<ObservationPage, ClientError> {
        let request = session_observations_telemetry_query_request(session_id, body)?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionObservationsTelemetryQuery, &response)
    }

    /// `POST /api/streams/{sessionId}/telemetry/stream`
    /// Stream session telemetry observations from one origin.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_observations_telemetry_stream(
        &self,
        session_id: SessionId,
        body: &ObservationStreamRequest,
    ) -> Result<NdjsonFrames<ObservationFrame>, ClientError> {
        let request = session_observations_telemetry_stream_request(session_id, body)?;
        let response = self.send(request).await?;
        decode_ndjson(RouteId::SessionObservationsTelemetryStream, response)
    }

    /// `GET /api/observations/{sessionId}/traces/{traceId}`
    /// Read one assembled trace by its W3C identifier.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_observations_trace_get(
        &self,
        session_id: SessionId,
        trace_id: TraceId,
    ) -> Result<TraceDetail, ClientError> {
        let request = session_observations_trace_get_request(session_id, trace_id)?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionObservationsTraceGet, &response)
    }

    /// `POST /api/streams/{sessionId}/traces/listen`
    /// Listen for new session traces observations.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_observations_traces_listen(
        &self,
        session_id: SessionId,
        body: &ObservationListenRequest,
    ) -> Result<NdjsonFrames<ObservationFrame>, ClientError> {
        let request = session_observations_traces_listen_request(session_id, body)?;
        let response = self.send(request).await?;
        decode_ndjson(RouteId::SessionObservationsTracesListen, response)
    }

    /// `POST /api/observations/{sessionId}/traces/query`
    /// Query session traces observations.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_observations_traces_query(
        &self,
        session_id: SessionId,
        body: &ObservationQuery,
    ) -> Result<ObservationPage, ClientError> {
        let request = session_observations_traces_query_request(session_id, body)?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionObservationsTracesQuery, &response)
    }

    /// `POST /api/streams/{sessionId}/traces/stream`
    /// Stream session traces observations from one origin.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_observations_traces_stream(
        &self,
        session_id: SessionId,
        body: &ObservationStreamRequest,
    ) -> Result<NdjsonFrames<ObservationFrame>, ClientError> {
        let request = session_observations_traces_stream_request(session_id, body)?;
        let response = self.send(request).await?;
        decode_ndjson(RouteId::SessionObservationsTracesStream, response)
    }

    /// `POST /api/sessions/{sessionId}/resumptions`
    /// Resume the same retained generation to idle.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_resume(
        &self,
        session_id: SessionId,
        body: &EmptyRequest,
        operation_id: OperationId,
    ) -> Result<Operation, ClientError> {
        let request = session_resume_request(session_id, body, operation_id)?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionResume, &response)
    }

    /// `POST /api/sessions/{sessionId}/suspensions`
    /// Suspend an idle session while retaining its exact generation; cancel active work first.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_suspend(
        &self,
        session_id: SessionId,
        body: &EmptyRequest,
        operation_id: OperationId,
    ) -> Result<Operation, ClientError> {
        let request = session_suspend_request(session_id, body, operation_id)?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionSuspend, &response)
    }

    /// `POST /api/observations/{sessionId}/telemetry/exports`
    /// Admit the durable session telemetry-export operation.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_telemetry_export_create(
        &self,
        session_id: SessionId,
        body: &TelemetryExportRequest,
        operation_id: OperationId,
    ) -> Result<Operation, ClientError> {
        let request = session_telemetry_export_create_request(session_id, body, operation_id)?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionTelemetryExportCreate, &response)
    }

    /// `POST /api/observations/{sessionId}/telemetry/exports/{exportId}/downloads`
    /// Mint a download grant for a ready session telemetry export.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_telemetry_export_download_create(
        &self,
        session_id: SessionId,
        export_id: ExportId,
        body: &EmptyRequest,
        idempotency_key: &IdempotencyKey,
    ) -> Result<DownloadGrant, ClientError> {
        let request = session_telemetry_export_download_create_request(
            session_id,
            export_id,
            body,
            idempotency_key,
        )?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionTelemetryExportDownloadCreate, &response)
    }

    /// `GET /api/observations/{sessionId}/telemetry/exports/{exportId}`
    /// Read one session telemetry export record.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_telemetry_export_get(
        &self,
        session_id: SessionId,
        export_id: ExportId,
    ) -> Result<TelemetryExport, ClientError> {
        let request = session_telemetry_export_get_request(session_id, export_id)?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionTelemetryExportGet, &response)
    }

    /// `POST /api/observations/{sessionId}/telemetry/exports/{exportId}/revocations`
    /// Revoke a session telemetry export and its outstanding grants.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_telemetry_export_revoke(
        &self,
        session_id: SessionId,
        export_id: ExportId,
        body: &EmptyRequest,
        idempotency_key: &IdempotencyKey,
    ) -> Result<TelemetryExport, ClientError> {
        let request =
            session_telemetry_export_revoke_request(session_id, export_id, body, idempotency_key)?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionTelemetryExportRevoke, &response)
    }

    /// `GET /api/observations/{sessionId}/telemetry/gaps/{gapId}`
    /// Read one recorded session telemetry gap.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_telemetry_gap_get(
        &self,
        session_id: SessionId,
        gap_id: TelemetryGapId,
    ) -> Result<TelemetryGap, ClientError> {
        let request = session_telemetry_gap_get_request(session_id, gap_id)?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionTelemetryGapGet, &response)
    }

    /// `POST /api/observations/{sessionId}/telemetry/gaps/query`
    /// Query recorded session telemetry gaps.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn session_telemetry_gaps_query(
        &self,
        session_id: SessionId,
        body: &TelemetryGapQuery,
    ) -> Result<TelemetryGapPage, ClientError> {
        let request = session_telemetry_gaps_query_request(session_id, body)?;
        let response = self.send(request).await?;
        decode_response(RouteId::SessionTelemetryGapsQuery, &response)
    }

    /// `POST /api/sessions/{sessionId}/terminations`
    /// Permanently destroy compute and live files while retaining metadata and sealed messages.
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
    ) -> Result<Operation, ClientError> {
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

    /// `POST /api/observations/telemetry/exports`
    /// Admit the durable workspace telemetry-export operation.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn telemetry_export_create(
        &self,
        body: &TelemetryExportRequest,
        operation_id: OperationId,
    ) -> Result<Operation, ClientError> {
        let request = telemetry_export_create_request(body, operation_id)?;
        let response = self.send(request).await?;
        decode_response(RouteId::TelemetryExportCreate, &response)
    }

    /// `POST /api/observations/telemetry/exports/{exportId}/downloads`
    /// Mint a download grant for a ready telemetry export.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn telemetry_export_download_create(
        &self,
        export_id: ExportId,
        body: &EmptyRequest,
        idempotency_key: &IdempotencyKey,
    ) -> Result<DownloadGrant, ClientError> {
        let request = telemetry_export_download_create_request(export_id, body, idempotency_key)?;
        let response = self.send(request).await?;
        decode_response(RouteId::TelemetryExportDownloadCreate, &response)
    }

    /// `GET /api/observations/telemetry/exports/{exportId}`
    /// Read one telemetry export record.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn telemetry_export_get(
        &self,
        export_id: ExportId,
    ) -> Result<TelemetryExport, ClientError> {
        let request = telemetry_export_get_request(export_id)?;
        let response = self.send(request).await?;
        decode_response(RouteId::TelemetryExportGet, &response)
    }

    /// `POST /api/observations/telemetry/exports/{exportId}/revocations`
    /// Revoke a telemetry export and its outstanding grants.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn telemetry_export_revoke(
        &self,
        export_id: ExportId,
        body: &EmptyRequest,
        idempotency_key: &IdempotencyKey,
    ) -> Result<TelemetryExport, ClientError> {
        let request = telemetry_export_revoke_request(export_id, body, idempotency_key)?;
        let response = self.send(request).await?;
        decode_response(RouteId::TelemetryExportRevoke, &response)
    }

    /// `GET /api/observations/telemetry/gaps/{gapId}`
    /// Read one recorded telemetry gap.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn telemetry_gap_get(
        &self,
        gap_id: TelemetryGapId,
    ) -> Result<TelemetryGap, ClientError> {
        let request = telemetry_gap_get_request(gap_id)?;
        let response = self.send(request).await?;
        decode_response(RouteId::TelemetryGapGet, &response)
    }

    /// `POST /api/observations/telemetry/gaps/query`
    /// Query recorded workspace telemetry gaps.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn telemetry_gaps_query(
        &self,
        body: &TelemetryGapQuery,
    ) -> Result<TelemetryGapPage, ClientError> {
        let request = telemetry_gaps_query_request(body)?;
        let response = self.send(request).await?;
        decode_response(RouteId::TelemetryGapsQuery, &response)
    }

    /// `DELETE /api/workspace/uploads/{uploadId}`
    /// Abort a staged upload.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn upload_abort(&self, upload_id: UploadId) -> Result<(), ClientError> {
        let request = upload_abort_request(upload_id)?;
        let response = self.send(request).await?;
        decode_no_content(RouteId::UploadAbort, &response)
    }

    /// `POST /api/workspace/uploads/{uploadId}/completion`
    /// Complete a staged upload.
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
    ) -> Result<Upload, ClientError> {
        let request = upload_complete_request(upload_id, body, idempotency_key)?;
        let response = self.send(request).await?;
        decode_response(RouteId::UploadComplete, &response)
    }

    /// `POST /api/workspace/uploads`
    /// Stage a large registered-resource value.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn upload_create(
        &self,
        body: &UploadCreateRequest,
        idempotency_key: &IdempotencyKey,
    ) -> Result<Upload, ClientError> {
        let request = upload_create_request(body, idempotency_key)?;
        let response = self.send(request).await?;
        decode_response(RouteId::UploadCreate, &response)
    }

    /// `POST /api/workspace/uploads/{uploadId}/parts`
    /// Mint presigned PUT grants for the named parts.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn upload_parts_grant(
        &self,
        upload_id: UploadId,
        body: &UploadPartsRequest,
    ) -> Result<UploadPartGrants, ClientError> {
        let request = upload_parts_grant_request(upload_id, body)?;
        let response = self.send(request).await?;
        decode_response(RouteId::UploadPartsGrant, &response)
    }

    /// `POST /api/billing/usage/query`
    /// Query rated usage for this workspace.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn usage_query(
        &self,
        query: &UsageQueryQuery,
        body: &UsageQuery,
    ) -> Result<UsagePage, ClientError> {
        let request = usage_query_request(query, body)?;
        let response = self.send(request).await?;
        decode_response(RouteId::UsageQuery, &response)
    }

    /// `POST /api/workspaces`
    /// Create a region-pinned workspace.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn workspace_create(
        &self,
        body: &WorkspaceCreateRequest,
        idempotency_key: &IdempotencyKey,
    ) -> Result<Workspace, ClientError> {
        let request = workspace_create_request(body, idempotency_key)?;
        let response = self.send(request).await?;
        decode_response(RouteId::WorkspaceCreate, &response)
    }

    /// `GET /api/workspace`
    /// Read the workspace this credential is pinned to.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn workspace_current_get(&self) -> Result<Workspace, ClientError> {
        let request = workspace_current_get_request()?;
        let response = self.send(request).await?;
        decode_response(RouteId::WorkspaceCurrentGet, &response)
    }

    /// `POST /api/workspaces/{workspaceId}/deletions`
    /// Admit the global workspace-deletion operation.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn workspace_delete(
        &self,
        workspace_id: WorkspaceId,
        body: &WorkspaceDeleteRequest,
        operation_id: OperationId,
    ) -> Result<Operation, ClientError> {
        let request = workspace_delete_request(workspace_id, body, operation_id)?;
        let response = self.send(request).await?;
        decode_response(RouteId::WorkspaceDelete, &response)
    }

    /// `GET /api/workspaces/{workspaceId}`
    /// Read one workspace.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn workspace_get(&self, workspace_id: WorkspaceId) -> Result<Workspace, ClientError> {
        let request = workspace_get_request(workspace_id)?;
        let response = self.send(request).await?;
        decode_response(RouteId::WorkspaceGet, &response)
    }

    /// `GET /api/workspace/limits/{limitId}`
    /// Read one effective workspace safety limit.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn workspace_limit_get(
        &self,
        limit_id: LimitId,
    ) -> Result<EffectiveWorkspaceLimit, ClientError> {
        let request = workspace_limit_get_request(limit_id)?;
        let response = self.send(request).await?;
        decode_response(RouteId::WorkspaceLimitGet, &response)
    }

    /// `GET /api/workspace/limits`
    /// List the effective workspace safety limits. The registry is closed and complete, so the
    /// whole set is one page and no continuation is ever minted.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn workspace_limits_list(&self) -> Result<EffectiveWorkspaceLimitPage, ClientError> {
        let request = workspace_limits_list_request()?;
        let response = self.send(request).await?;
        decode_response(RouteId::WorkspaceLimitsList, &response)
    }

    /// `GET /api/workspaces`
    /// List workspaces the caller can reach.
    ///
    /// # Errors
    /// Returns [`ClientError::Api`] for the published error envelope, [`ClientError::Transport`]
    /// when the request never reached a status, and [`ClientError::Decode`] when the answer does
    /// not match the contract.
    pub async fn workspaces_list(
        &self,
        query: &WorkspacesListQuery,
    ) -> Result<WorkspacePage, ClientError> {
        let request = workspaces_list_request(query)?;
        let response = self.send(request).await?;
        decode_response(RouteId::WorkspacesList, &response)
    }
}
