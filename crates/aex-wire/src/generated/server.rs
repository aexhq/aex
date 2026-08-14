//! GENERATED — DO NOT EDIT.
//!
//! The server traits and the total dispatch surface, one group per authoring fragment.
//!
//! Produced by `aex-contract-gen` from `api/`; contract digest
//! `sha256:e7f95cd7fc830a0871b1276cde9ae11e4245d0b843cd3dba567e133e551e051f`.
//! Regenerate with `cargo run -p aex-contract-gen -- build`.

#![allow(clippy::large_enum_variant, reason = "a wire union is never boxed")]
#![allow(clippy::match_same_arms, reason = "one arm per row")]
#![allow(clippy::too_many_lines, reason = "one arm per row")]

use core::future::Future;

use crate::dispatch::DispatchOutcome;
use crate::dispatch::FromParam;
use crate::dispatch::ParamError;
use crate::dispatch::QueryReader;
use crate::dispatch::RawRequest;
use crate::dispatch::RawResponse;
use crate::dispatch::RequestLimits;
use crate::dispatch::declared;
use crate::dispatch::decode_body;
use crate::dispatch::expect_no_body;
use crate::dispatch::path_param;
use crate::dispatch::wrong_group;
use crate::error::WireResult;
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
use crate::models::DashboardBootstrap;
use crate::models::DashboardSessionCredential;
use crate::models::DashboardSessionRequest;
use crate::models::DownloadGrant;
use crate::models::EmptyRequest;
use crate::models::HostedSession;
use crate::models::MessagePage;
use crate::models::MessageSendRequest;
use crate::models::MessageSendResult;
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
use crate::models::TopUpCheckoutRequest;
use crate::models::UploadAdmission;
use crate::models::UploadCompleteRequest;
use crate::models::UploadCreateRequest;
use crate::routes::Plane;
use crate::routes::RouteId;
use crate::server::Created;
use crate::server::NdjsonStream;
use crate::server::NoContent;
use crate::server::RequestContext;
use crate::server::WithETag;

/// A mountable group of operations: exactly one authoring fragment on exactly one plane.
/// This is a projection of `ROUTES`, not a second table. Every route belongs to exactly one group
/// and every group's slice is a subset of `RouteId::ALL`, which `route_groups_partition_the_table`
/// asserts.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum RouteGroup {
    /// `api-keys` on the central plane, served by `ApiKeysApi`.
    ApiKeys,
    /// `auth` on the central plane, served by `AuthApi`.
    Auth,
    /// `billing` on the central plane, served by `BillingApi`.
    Billing,
    /// `bootstrap` on the central plane, served by `BootstrapApi`.
    Bootstrap,
    /// `registry` on the regional plane, served by `RegistryApi`.
    Registry,
    /// `sessions` on the regional plane, served by `SessionsApi`.
    Sessions,
    /// `uploads` on the regional plane, served by `UploadsApi`.
    Uploads,
}

impl RouteGroup {
    /// Every group, in key order.
    pub const ALL: &'static [RouteGroup] = &[
        RouteGroup::ApiKeys,
        RouteGroup::Auth,
        RouteGroup::Billing,
        RouteGroup::Bootstrap,
        RouteGroup::Registry,
        RouteGroup::Sessions,
        RouteGroup::Uploads,
    ];

    /// The stable `<plane>:<fragment>` key.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ApiKeys => "central:api-keys",
            Self::Auth => "central:auth",
            Self::Billing => "central:billing",
            Self::Bootstrap => "central:bootstrap",
            Self::Registry => "regional:registry",
            Self::Sessions => "regional:sessions",
            Self::Uploads => "regional:uploads",
        }
    }

    /// Which plane serves the group.
    #[must_use]
    pub const fn plane(self) -> Plane {
        match self {
            Self::ApiKeys => Plane::Central,
            Self::Auth => Plane::Central,
            Self::Billing => Plane::Central,
            Self::Bootstrap => Plane::Central,
            Self::Registry => Plane::Regional,
            Self::Sessions => Plane::Regional,
            Self::Uploads => Plane::Regional,
        }
    }

    /// The generated trait a composition crate mounts.
    #[must_use]
    pub const fn trait_name(self) -> &'static str {
        match self {
            Self::ApiKeys => "ApiKeysApi",
            Self::Auth => "AuthApi",
            Self::Billing => "BillingApi",
            Self::Bootstrap => "BootstrapApi",
            Self::Registry => "RegistryApi",
            Self::Sessions => "SessionsApi",
            Self::Uploads => "UploadsApi",
        }
    }

    /// Every route in the group, in `RouteId` order. Mounting is a loop over this slice.
    #[must_use]
    pub const fn routes(self) -> &'static [RouteId] {
        match self {
            Self::ApiKeys => API_KEYS_ROUTES,
            Self::Auth => AUTH_ROUTES,
            Self::Billing => BILLING_ROUTES,
            Self::Bootstrap => BOOTSTRAP_ROUTES,
            Self::Registry => REGISTRY_ROUTES,
            Self::Sessions => SESSIONS_ROUTES,
            Self::Uploads => UPLOADS_ROUTES,
        }
    }
}

/// Every route of `central:api-keys`, in `RouteId` order.
pub const API_KEYS_ROUTES: &[RouteId] = &[
    RouteId::ApiKeyCreate,
    RouteId::ApiKeyRevoke,
    RouteId::ApiKeysList,
];

/// Every route of `central:auth`, in `RouteId` order.
pub const AUTH_ROUTES: &[RouteId] = &[
    RouteId::DashboardSessionCreate,
    RouteId::DashboardSessionDelete,
];

/// Every route of `central:billing`, in `RouteId` order.
pub const BILLING_ROUTES: &[RouteId] = &[
    RouteId::BillingBalanceGet,
    RouteId::BillingPaymentMethodDelete,
    RouteId::BillingPaymentMethodSessionCreate,
    RouteId::BillingPaymentMethodsList,
    RouteId::BillingTopUpCheckoutCreate,
    RouteId::BillingTransactionsList,
    RouteId::BillingUsageGet,
];

/// Every route of `central:bootstrap`, in `RouteId` order.
pub const BOOTSTRAP_ROUTES: &[RouteId] = &[RouteId::DashboardBootstrapGet];

/// Every route of `regional:registry`, in `RouteId` order.
pub const REGISTRY_ROUTES: &[RouteId] = &[
    RouteId::RegistryFilesDelete,
    RouteId::RegistryFilesDownloadCreate,
    RouteId::RegistryFilesGet,
    RouteId::RegistryFilesList,
    RouteId::RegistryFilesPut,
];

/// Every route of `regional:sessions`, in `RouteId` order.
pub const SESSIONS_ROUTES: &[RouteId] = &[
    RouteId::SessionCancel,
    RouteId::SessionCreate,
    RouteId::SessionDelete,
    RouteId::SessionGet,
    RouteId::SessionMessageSend,
    RouteId::SessionMessagesList,
    RouteId::SessionMessagesStream,
    RouteId::SessionTelemetryDownloadCreate,
    RouteId::SessionTelemetryReplay,
    RouteId::SessionTelemetryStream,
    RouteId::SessionTerminate,
    RouteId::SessionsList,
];

/// Every route of `regional:uploads`, in `RouteId` order.
pub const UPLOADS_ROUTES: &[RouteId] = &[RouteId::UploadComplete, RouteId::UploadCreate];

impl RouteId {
    /// Which group serves this route.
    #[must_use]
    pub const fn group(self) -> RouteGroup {
        match self {
            Self::ApiKeyCreate => RouteGroup::ApiKeys,
            Self::ApiKeyRevoke => RouteGroup::ApiKeys,
            Self::ApiKeysList => RouteGroup::ApiKeys,
            Self::DashboardSessionCreate => RouteGroup::Auth,
            Self::DashboardSessionDelete => RouteGroup::Auth,
            Self::BillingBalanceGet => RouteGroup::Billing,
            Self::BillingPaymentMethodDelete => RouteGroup::Billing,
            Self::BillingPaymentMethodSessionCreate => RouteGroup::Billing,
            Self::BillingPaymentMethodsList => RouteGroup::Billing,
            Self::BillingTopUpCheckoutCreate => RouteGroup::Billing,
            Self::BillingTransactionsList => RouteGroup::Billing,
            Self::BillingUsageGet => RouteGroup::Billing,
            Self::DashboardBootstrapGet => RouteGroup::Bootstrap,
            Self::RegistryFilesDelete => RouteGroup::Registry,
            Self::RegistryFilesDownloadCreate => RouteGroup::Registry,
            Self::RegistryFilesGet => RouteGroup::Registry,
            Self::RegistryFilesList => RouteGroup::Registry,
            Self::RegistryFilesPut => RouteGroup::Registry,
            Self::SessionCancel => RouteGroup::Sessions,
            Self::SessionCreate => RouteGroup::Sessions,
            Self::SessionDelete => RouteGroup::Sessions,
            Self::SessionGet => RouteGroup::Sessions,
            Self::SessionMessageSend => RouteGroup::Sessions,
            Self::SessionMessagesList => RouteGroup::Sessions,
            Self::SessionMessagesStream => RouteGroup::Sessions,
            Self::SessionTelemetryDownloadCreate => RouteGroup::Sessions,
            Self::SessionTelemetryReplay => RouteGroup::Sessions,
            Self::SessionTelemetryStream => RouteGroup::Sessions,
            Self::SessionTerminate => RouteGroup::Sessions,
            Self::SessionsList => RouteGroup::Sessions,
            Self::UploadComplete => RouteGroup::Uploads,
            Self::UploadCreate => RouteGroup::Uploads,
        }
    }
}

// --- parameter decoding ---------------------------------------------------

/// Decodes a prefixed identifier from a path segment or a query value.
macro_rules! from_param_id {
    ($($ty:ty),* $(,)?) => {
        $(impl FromParam for $ty {
            fn from_param(text: &str) -> Result<Self, ParamError> {
                <$ty as crate::ids::PrefixedId>::parse(text)
                    .map_err(|reason| ParamError::new(reason.to_string()))
            }
        })*
    };
}

from_param_id!(
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

/// Decodes a closed enumeration from a query value.
macro_rules! from_param_enum {
    ($($ty:ty),* $(,)?) => {
        $(impl FromParam for $ty {
            fn from_param(text: &str) -> Result<Self, ParamError> {
                Self::ALL
                    .iter()
                    .copied()
                    .find(|candidate| candidate.as_str() == text)
                    .ok_or_else(|| {
                        ParamError::new(format!("`{text}` is not a value of `{}`", stringify!($ty)))
                    })
            }
        })*
    };
}

from_param_enum!(BillingUsageCategory, SessionStatus);

// --- central:api-keys ---------------------------------------------------------------

/// The `api-keys` fragment of the central plane: 3 operations.
/// Every method returns a future that is `Send`, so the composition crate can spawn it without
/// wrapping. A method never names a status: the response type it returns is the status the route
/// declares.
pub trait ApiKeysApi: Send + Sync + 'static {
    /// `POST /api/api-keys`
    /// Mint a workspace API key whose value is returned once.
    fn api_key_create(
        &self,
        cx: &RequestContext,
        body: ApiKeyCreateRequest,
    ) -> impl Future<Output = WireResult<Created<NewApiKey>>> + Send;

    /// `DELETE /api/api-keys/{apiKeyId}`
    /// Revoke a workspace API key.
    fn api_key_revoke(
        &self,
        cx: &RequestContext,
        api_key_id: ApiKeyId,
    ) -> impl Future<Output = WireResult<NoContent>> + Send;

    /// `GET /api/api-keys`
    /// List workspace API key metadata.
    fn api_keys_list(
        &self,
        cx: &RequestContext,
        query: ApiKeysListQuery,
    ) -> impl Future<Output = WireResult<ApiKeyPage>> + Send;
}

/// Decodes, calls and encodes one `central:api-keys` request.
/// Total over `RouteId`: a route from another group is an internal error naming the mismatch, never
/// a silently wrong handler.
/// # Errors
/// Returns the handler's own declared failure, or a decode failure the route declares. A code the
/// route does not declare is refused at this boundary.
pub async fn dispatch_api_keys<A: ApiKeysApi + ?Sized>(
    api: &A,
    cx: &RequestContext,
    raw: RawRequest<'_>,
    limits: RequestLimits,
) -> WireResult<DispatchOutcome<crate::dispatch::NoStream>> {
    let reader = QueryReader::parse(raw.route, raw.query)?;
    match raw.route {
        RouteId::ApiKeyCreate => {
            let body = decode_body::<ApiKeyCreateRequest>(&raw, limits)?;
            let handled = api.api_key_create(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(201, &answer.0)?))
        }
        RouteId::ApiKeyRevoke => {
            let api_key_id = path_param::<ApiKeyId>(&raw, "apiKeyId")?;
            expect_no_body(&raw)?;
            let handled = api.api_key_revoke(cx, api_key_id);
            let NoContent = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::no_content()))
        }
        RouteId::ApiKeysList => {
            let query = ApiKeysListQuery {
                cursor: reader.optional("cursor")?,
                limit: reader.optional_bounded("limit", 1, 1000)?,
                workspace_id: reader.required("workspaceId")?,
            };
            expect_no_body(&raw)?;
            let handled = api.api_keys_list(cx, query);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        other => Err(wrong_group(other, "central:api-keys")),
    }
}

// --- central:auth ---------------------------------------------------------------

/// The `auth` fragment of the central plane: 2 operations.
/// Every method returns a future that is `Send`, so the composition crate can spawn it without
/// wrapping. A method never names a status: the response type it returns is the status the route
/// declares.
pub trait AuthApi: Send + Sync + 'static {
    /// `POST /api/auth/sessions`
    /// Exchange a provider authorization code for a browser session.
    fn dashboard_session_create(
        &self,
        cx: &RequestContext,
        body: DashboardSessionRequest,
    ) -> impl Future<Output = WireResult<Created<DashboardSessionCredential>>> + Send;

    /// `DELETE /api/auth/sessions/current`
    /// Close the browser session the caller presented.
    fn dashboard_session_delete(
        &self,
        cx: &RequestContext,
    ) -> impl Future<Output = WireResult<NoContent>> + Send;
}

/// Decodes, calls and encodes one `central:auth` request.
/// Total over `RouteId`: a route from another group is an internal error naming the mismatch, never
/// a silently wrong handler.
/// # Errors
/// Returns the handler's own declared failure, or a decode failure the route declares. A code the
/// route does not declare is refused at this boundary.
pub async fn dispatch_auth<A: AuthApi + ?Sized>(
    api: &A,
    cx: &RequestContext,
    raw: RawRequest<'_>,
    limits: RequestLimits,
) -> WireResult<DispatchOutcome<crate::dispatch::NoStream>> {
    let _reader = QueryReader::parse(raw.route, raw.query)?;
    match raw.route {
        RouteId::DashboardSessionCreate => {
            let body = decode_body::<DashboardSessionRequest>(&raw, limits)?;
            let handled = api.dashboard_session_create(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(201, &answer.0)?))
        }
        RouteId::DashboardSessionDelete => {
            expect_no_body(&raw)?;
            let handled = api.dashboard_session_delete(cx);
            let NoContent = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::no_content()))
        }
        other => Err(wrong_group(other, "central:auth")),
    }
}

// --- central:billing ---------------------------------------------------------------

/// The `billing` fragment of the central plane: 7 operations.
/// Every method returns a future that is `Send`, so the composition crate can spawn it without
/// wrapping. A method never names a status: the response type it returns is the status the route
/// declares.
pub trait BillingApi: Send + Sync + 'static {
    /// `GET /api/billing/balance`
    /// Read the caller's prepaid balance and active reservations.
    fn billing_balance_get(
        &self,
        cx: &RequestContext,
    ) -> impl Future<Output = WireResult<BillingBalance>> + Send;

    /// `DELETE /api/billing/payment-methods/{paymentMethodId}`
    /// Detach a card owned by the caller's account.
    fn billing_payment_method_delete(
        &self,
        cx: &RequestContext,
        payment_method_id: PaymentMethodId,
    ) -> impl Future<Output = WireResult<NoContent>> + Send;

    /// `POST /api/billing/payment-method-sessions`
    /// Create a Stripe-hosted card setup session with explicit consent.
    fn billing_payment_method_session_create(
        &self,
        cx: &RequestContext,
        body: PaymentMethodSessionRequest,
    ) -> impl Future<Output = WireResult<Created<HostedSession>>> + Send;

    /// `GET /api/billing/payment-methods`
    /// List card display metadata for the caller's account.
    fn billing_payment_methods_list(
        &self,
        cx: &RequestContext,
    ) -> impl Future<Output = WireResult<PaymentMethodPage>> + Send;

    /// `POST /api/billing/top-up-checkouts`
    /// Create a one-time Stripe-hosted prepaid top-up checkout.
    fn billing_top_up_checkout_create(
        &self,
        cx: &RequestContext,
        body: TopUpCheckoutRequest,
    ) -> impl Future<Output = WireResult<Created<HostedSession>>> + Send;

    /// `GET /api/billing/transactions`
    /// List immutable prepaid ledger transactions, newest first.
    fn billing_transactions_list(
        &self,
        cx: &RequestContext,
        query: BillingTransactionsListQuery,
    ) -> impl Future<Output = WireResult<BillingTransactionPage>> + Send;

    /// `GET /api/billing/usage`
    /// Read bounded rated usage and its settlement coverage.
    fn billing_usage_get(
        &self,
        cx: &RequestContext,
        query: BillingUsageGetQuery,
    ) -> impl Future<Output = WireResult<BillingUsagePage>> + Send;
}

/// Decodes, calls and encodes one `central:billing` request.
/// Total over `RouteId`: a route from another group is an internal error naming the mismatch, never
/// a silently wrong handler.
/// # Errors
/// Returns the handler's own declared failure, or a decode failure the route declares. A code the
/// route does not declare is refused at this boundary.
pub async fn dispatch_billing<A: BillingApi + ?Sized>(
    api: &A,
    cx: &RequestContext,
    raw: RawRequest<'_>,
    limits: RequestLimits,
) -> WireResult<DispatchOutcome<crate::dispatch::NoStream>> {
    let reader = QueryReader::parse(raw.route, raw.query)?;
    match raw.route {
        RouteId::BillingBalanceGet => {
            expect_no_body(&raw)?;
            let handled = api.billing_balance_get(cx);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::BillingPaymentMethodDelete => {
            let payment_method_id = path_param::<PaymentMethodId>(&raw, "paymentMethodId")?;
            expect_no_body(&raw)?;
            let handled = api.billing_payment_method_delete(cx, payment_method_id);
            let NoContent = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::no_content()))
        }
        RouteId::BillingPaymentMethodSessionCreate => {
            let body = decode_body::<PaymentMethodSessionRequest>(&raw, limits)?;
            let handled = api.billing_payment_method_session_create(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(201, &answer.0)?))
        }
        RouteId::BillingPaymentMethodsList => {
            expect_no_body(&raw)?;
            let handled = api.billing_payment_methods_list(cx);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::BillingTopUpCheckoutCreate => {
            let body = decode_body::<TopUpCheckoutRequest>(&raw, limits)?;
            let handled = api.billing_top_up_checkout_create(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(201, &answer.0)?))
        }
        RouteId::BillingTransactionsList => {
            let query = BillingTransactionsListQuery {
                cursor: reader.optional("cursor")?,
                limit: reader.optional_bounded("limit", 1, 100)?,
            };
            expect_no_body(&raw)?;
            let handled = api.billing_transactions_list(cx, query);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::BillingUsageGet => {
            let query = BillingUsageGetQuery {
                category: reader.optional("category")?,
                cursor: reader.optional("cursor")?,
                from: reader.optional("from")?,
                limit: reader.optional_bounded("limit", 1, 100)?,
                session_id: reader.optional("sessionId")?,
                to: reader.optional("to")?,
            };
            expect_no_body(&raw)?;
            let handled = api.billing_usage_get(cx, query);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        other => Err(wrong_group(other, "central:billing")),
    }
}

// --- central:bootstrap ---------------------------------------------------------------

/// The `bootstrap` fragment of the central plane: 1 operation.
/// Every method returns a future that is `Send`, so the composition crate can spawn it without
/// wrapping. A method never names a status: the response type it returns is the status the route
/// declares.
pub trait BootstrapApi: Send + Sync + 'static {
    /// `GET /api/bootstrap`
    /// One bounded read that fills the dashboard shell.
    fn dashboard_bootstrap_get(
        &self,
        cx: &RequestContext,
    ) -> impl Future<Output = WireResult<DashboardBootstrap>> + Send;
}

/// Decodes, calls and encodes one `central:bootstrap` request.
/// Total over `RouteId`: a route from another group is an internal error naming the mismatch, never
/// a silently wrong handler.
/// # Errors
/// Returns the handler's own declared failure, or a decode failure the route declares. A code the
/// route does not declare is refused at this boundary.
pub async fn dispatch_bootstrap<A: BootstrapApi + ?Sized>(
    api: &A,
    cx: &RequestContext,
    raw: RawRequest<'_>,
    _limits: RequestLimits,
) -> WireResult<DispatchOutcome<crate::dispatch::NoStream>> {
    let _reader = QueryReader::parse(raw.route, raw.query)?;
    match raw.route {
        RouteId::DashboardBootstrapGet => {
            expect_no_body(&raw)?;
            let handled = api.dashboard_bootstrap_get(cx);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        other => Err(wrong_group(other, "central:bootstrap")),
    }
}

// --- regional:registry ---------------------------------------------------------------

/// The `registry` fragment of the regional plane: 5 operations.
/// Every method returns a future that is `Send`, so the composition crate can spawn it without
/// wrapping. A method never names a status: the response type it returns is the status the route
/// declares.
pub trait RegistryApi: Send + Sync + 'static {
    /// `DELETE /api/files/{name}`
    /// Delete one current workspace file.
    fn registry_files_delete(
        &self,
        cx: &RequestContext,
        name: ResourceName,
    ) -> impl Future<Output = WireResult<NoContent>> + Send;

    /// `POST /api/files/{name}/downloads`
    /// Mint a download grant for a ready current workspace file.
    fn registry_files_download_create(
        &self,
        cx: &RequestContext,
        name: ResourceName,
        body: RegistryDownloadRequest,
    ) -> impl Future<Output = WireResult<Created<DownloadGrant>>> + Send;

    /// `GET /api/files/{name}`
    /// Read one current workspace file.
    fn registry_files_get(
        &self,
        cx: &RequestContext,
        name: ResourceName,
    ) -> impl Future<Output = WireResult<WithETag<RegisteredFile>>> + Send;

    /// `GET /api/files`
    /// List current workspace files.
    fn registry_files_list(
        &self,
        cx: &RequestContext,
        query: RegistryFilesListQuery,
    ) -> impl Future<Output = WireResult<RegisteredFilePage>> + Send;

    /// `PUT /api/files/{name}`
    /// Replace one current workspace file from inline bytes or an HTTPS URL.
    fn registry_files_put(
        &self,
        cx: &RequestContext,
        name: ResourceName,
        body: RegisteredFileValue,
    ) -> impl Future<Output = WireResult<WithETag<RegisteredFile>>> + Send;
}

/// Decodes, calls and encodes one `regional:registry` request.
/// Total over `RouteId`: a route from another group is an internal error naming the mismatch, never
/// a silently wrong handler.
/// # Errors
/// Returns the handler's own declared failure, or a decode failure the route declares. A code the
/// route does not declare is refused at this boundary.
pub async fn dispatch_registry<A: RegistryApi + ?Sized>(
    api: &A,
    cx: &RequestContext,
    raw: RawRequest<'_>,
    limits: RequestLimits,
) -> WireResult<DispatchOutcome<crate::dispatch::NoStream>> {
    let reader = QueryReader::parse(raw.route, raw.query)?;
    match raw.route {
        RouteId::RegistryFilesDelete => {
            let name = path_param::<ResourceName>(&raw, "name")?;
            expect_no_body(&raw)?;
            let handled = api.registry_files_delete(cx, name);
            let NoContent = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::no_content()))
        }
        RouteId::RegistryFilesDownloadCreate => {
            let name = path_param::<ResourceName>(&raw, "name")?;
            let body = decode_body::<RegistryDownloadRequest>(&raw, limits)?;
            let handled = api.registry_files_download_create(cx, name, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(201, &answer.0)?))
        }
        RouteId::RegistryFilesGet => {
            let name = path_param::<ResourceName>(&raw, "name")?;
            expect_no_body(&raw)?;
            let handled = api.registry_files_get(cx, name);
            let answer = declared(raw.route, handled.await)?;
            let rendered = RawResponse::json(200, &answer.value)?;
            let rendered = rendered.with_etag(answer.etag);
            Ok(DispatchOutcome::Unary(rendered))
        }
        RouteId::RegistryFilesList => {
            let query = RegistryFilesListQuery {
                cursor: reader.optional("cursor")?,
                limit: reader.optional_bounded("limit", 1, 1000)?,
            };
            expect_no_body(&raw)?;
            let handled = api.registry_files_list(cx, query);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::RegistryFilesPut => {
            let name = path_param::<ResourceName>(&raw, "name")?;
            let body = decode_body::<RegisteredFileValue>(&raw, limits)?;
            let handled = api.registry_files_put(cx, name, body);
            let answer = declared(raw.route, handled.await)?;
            let rendered = RawResponse::json(200, &answer.value)?;
            let rendered = rendered.with_etag(answer.etag);
            Ok(DispatchOutcome::Unary(rendered))
        }
        other => Err(wrong_group(other, "regional:registry")),
    }
}

// --- regional:sessions ---------------------------------------------------------------

/// The `sessions` fragment of the regional plane: 12 operations.
/// Every method returns a future that is `Send`, so the composition crate can spawn it without
/// wrapping. A method never names a status: the response type it returns is the status the route
/// declares.
pub trait SessionsApi: Send + Sync + 'static {
    /// The frame stream this implementation produces for an NDJSON route.
    /// `aex-wire` deliberately does not name `Stream`: it has no async dependency, so the
    /// composition crate supplies the concrete type and its own bound.
    type FrameStream: Send + 'static;

    /// `POST /api/sessions/{sessionId}/cancellations`
    /// Cancel current work and return the session to idle.
    fn session_cancel(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: EmptyRequest,
    ) -> impl Future<Output = WireResult<SessionCommandReceipt>> + Send;

    /// `POST /api/sessions`
    /// Create a durable session and eagerly prepare its default-on sandbox in the background.
    fn session_create(
        &self,
        cx: &RequestContext,
        body: SessionCreateRequest,
    ) -> impl Future<Output = WireResult<Created<Session>>> + Send;

    /// `POST /api/sessions/{sessionId}/deletions`
    /// Irreversibly delete session-scoped user content; independent workspace files remain.
    fn session_delete(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: EmptyRequest,
    ) -> impl Future<Output = WireResult<SessionCommandReceipt>> + Send;

    /// `GET /api/sessions/{sessionId}`
    /// Read durable session metadata and sandbox preparation state.
    fn session_get(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
    ) -> impl Future<Output = WireResult<WithETag<Session>>> + Send;

    /// `POST /api/sessions/{sessionId}/messages`
    /// Admit one text message; file paths are referenced in text, never attached.
    fn session_message_send(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: MessageSendRequest,
    ) -> impl Future<Output = WireResult<Created<MessageSendResult>>> + Send;

    /// `GET /api/sessions/{sessionId}/messages`
    /// List complete committed messages in immutable seal order.
    fn session_messages_list(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        query: SessionMessagesListQuery,
    ) -> impl Future<Output = WireResult<MessagePage>> + Send;

    /// `GET /api/sessions/{sessionId}/messages/stream`
    /// Stream bounded assistant previews plus commit/reconcile/gap frames.
    fn session_messages_stream(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        query: SessionMessagesStreamQuery,
    ) -> impl Future<Output = WireResult<NdjsonStream<Self::FrameStream>>> + Send;

    /// `POST /api/sessions/{sessionId}/telemetry/downloads`
    /// Mint a short-lived download for a bounded retained telemetry export.
    fn session_telemetry_download_create(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: TelemetryDownloadRequest,
    ) -> impl Future<Output = WireResult<Created<TelemetryDownloadGrant>>> + Send;

    /// `GET /api/sessions/{sessionId}/telemetry/replay`
    /// Replay retained telemetry from compressed immutable S3 segments.
    fn session_telemetry_replay(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        query: SessionTelemetryReplayQuery,
    ) -> impl Future<Output = WireResult<NdjsonStream<Self::FrameStream>>> + Send;

    /// `GET /api/sessions/{sessionId}/telemetry/stream`
    /// Stream live trusted assistant, tool, runtime, Logs and Traces telemetry with bounded
    /// previews.
    fn session_telemetry_stream(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        query: SessionTelemetryStreamQuery,
    ) -> impl Future<Output = WireResult<NdjsonStream<Self::FrameStream>>> + Send;

    /// `POST /api/sessions/{sessionId}/terminations`
    /// Destroy sandbox compute while retaining session metadata and messages.
    fn session_terminate(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: EmptyRequest,
    ) -> impl Future<Output = WireResult<SessionCommandReceipt>> + Send;

    /// `GET /api/sessions`
    /// List sessions in the workspace.
    fn sessions_list(
        &self,
        cx: &RequestContext,
        query: SessionsListQuery,
    ) -> impl Future<Output = WireResult<SessionListPage>> + Send;
}

/// Decodes, calls and encodes one `regional:sessions` request.
/// Total over `RouteId`: a route from another group is an internal error naming the mismatch, never
/// a silently wrong handler.
/// # Errors
/// Returns the handler's own declared failure, or a decode failure the route declares. A code the
/// route does not declare is refused at this boundary.
pub async fn dispatch_sessions<A: SessionsApi + ?Sized>(
    api: &A,
    cx: &RequestContext,
    raw: RawRequest<'_>,
    limits: RequestLimits,
) -> WireResult<DispatchOutcome<A::FrameStream>> {
    let reader = QueryReader::parse(raw.route, raw.query)?;
    match raw.route {
        RouteId::SessionCancel => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<EmptyRequest>(&raw, limits)?;
            let handled = api.session_cancel(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(202, &answer)?))
        }
        RouteId::SessionCreate => {
            let body = decode_body::<SessionCreateRequest>(&raw, limits)?;
            let handled = api.session_create(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(201, &answer.0)?))
        }
        RouteId::SessionDelete => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<EmptyRequest>(&raw, limits)?;
            let handled = api.session_delete(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(202, &answer)?))
        }
        RouteId::SessionGet => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            expect_no_body(&raw)?;
            let handled = api.session_get(cx, session_id);
            let answer = declared(raw.route, handled.await)?;
            let rendered = RawResponse::json(200, &answer.value)?;
            let rendered = rendered.with_etag(answer.etag);
            Ok(DispatchOutcome::Unary(rendered))
        }
        RouteId::SessionMessageSend => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<MessageSendRequest>(&raw, limits)?;
            let handled = api.session_message_send(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(201, &answer.0)?))
        }
        RouteId::SessionMessagesList => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let query = SessionMessagesListQuery {
                cursor: reader.optional("cursor")?,
                limit: reader.optional_bounded("limit", 1, 100)?,
            };
            expect_no_body(&raw)?;
            let handled = api.session_messages_list(cx, session_id, query);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::SessionMessagesStream => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let query = SessionMessagesStreamQuery {
                after: reader.optional("after")?,
            };
            expect_no_body(&raw)?;
            let handled = api.session_messages_stream(cx, session_id, query);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Ndjson(answer))
        }
        RouteId::SessionTelemetryDownloadCreate => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<TelemetryDownloadRequest>(&raw, limits)?;
            let handled = api.session_telemetry_download_create(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(201, &answer.0)?))
        }
        RouteId::SessionTelemetryReplay => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let query = SessionTelemetryReplayQuery {
                after: reader.optional("after")?,
                limit: reader.optional_bounded("limit", 1, 10000)?,
            };
            expect_no_body(&raw)?;
            let handled = api.session_telemetry_replay(cx, session_id, query);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Ndjson(answer))
        }
        RouteId::SessionTelemetryStream => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let query = SessionTelemetryStreamQuery {
                after: reader.optional("after")?,
            };
            expect_no_body(&raw)?;
            let handled = api.session_telemetry_stream(cx, session_id, query);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Ndjson(answer))
        }
        RouteId::SessionTerminate => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<EmptyRequest>(&raw, limits)?;
            let handled = api.session_terminate(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(202, &answer)?))
        }
        RouteId::SessionsList => {
            let query = SessionsListQuery {
                cursor: reader.optional("cursor")?,
                limit: reader.optional_bounded("limit", 1, 100)?,
                status: reader.optional("status")?,
            };
            expect_no_body(&raw)?;
            let handled = api.sessions_list(cx, query);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        other => Err(wrong_group(other, "regional:sessions")),
    }
}

// --- regional:uploads ---------------------------------------------------------------

/// The `uploads` fragment of the regional plane: 2 operations.
/// Every method returns a future that is `Send`, so the composition crate can spawn it without
/// wrapping. A method never names a status: the response type it returns is the status the route
/// declares.
pub trait UploadsApi: Send + Sync + 'static {
    /// `POST /api/uploads/{uploadId}/completions`
    /// Verify an admitted upload and publish it only if its private overwrite intent is still
    /// current.
    fn upload_complete(
        &self,
        cx: &RequestContext,
        upload_id: UploadId,
        body: UploadCompleteRequest,
    ) -> impl Future<Output = WireResult<RegisteredFile>> + Send;

    /// `POST /api/uploads`
    /// Admit a direct upload for one current workspace-file name and return every bounded part
    /// grant.
    fn upload_create(
        &self,
        cx: &RequestContext,
        body: UploadCreateRequest,
    ) -> impl Future<Output = WireResult<Created<UploadAdmission>>> + Send;
}

/// Decodes, calls and encodes one `regional:uploads` request.
/// Total over `RouteId`: a route from another group is an internal error naming the mismatch, never
/// a silently wrong handler.
/// # Errors
/// Returns the handler's own declared failure, or a decode failure the route declares. A code the
/// route does not declare is refused at this boundary.
pub async fn dispatch_uploads<A: UploadsApi + ?Sized>(
    api: &A,
    cx: &RequestContext,
    raw: RawRequest<'_>,
    limits: RequestLimits,
) -> WireResult<DispatchOutcome<crate::dispatch::NoStream>> {
    let _reader = QueryReader::parse(raw.route, raw.query)?;
    match raw.route {
        RouteId::UploadComplete => {
            let upload_id = path_param::<UploadId>(&raw, "uploadId")?;
            let body = decode_body::<UploadCompleteRequest>(&raw, limits)?;
            let handled = api.upload_complete(cx, upload_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::UploadCreate => {
            let body = decode_body::<UploadCreateRequest>(&raw, limits)?;
            let handled = api.upload_create(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(201, &answer.0)?))
        }
        other => Err(wrong_group(other, "regional:uploads")),
    }
}
