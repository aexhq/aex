//! GENERATED — DO NOT EDIT.
//!
//! The server traits and the total dispatch surface, one group per authoring fragment.
//!
//! Produced by `aex-contract-gen` from `api/`; contract digest
//! `sha256:b65e5107513c7e4bc4d522ce2557e476c61a8a6602270b5ea0c6e6b78dae12db`.
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
use crate::dispatch::binary_body;
use crate::dispatch::declared;
use crate::dispatch::decode_body;
use crate::dispatch::expect_no_body;
use crate::dispatch::otlp_body;
use crate::dispatch::path_param;
use crate::dispatch::path_param_registry;
use crate::dispatch::wrong_group;
use crate::error::WireResult;
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
use crate::routes::Plane;
use crate::routes::RouteId;
use crate::server::Accepted;
use crate::server::BinaryBody;
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
    /// `operations` on the central plane, served by `CentralOperationsApi`.
    CentralOperations,
    /// `files` on the regional plane, served by `FilesApi`.
    Files,
    /// `identity` on the central plane, served by `IdentityApi`.
    Identity,
    /// `observations` on the regional plane, served by `ObservationsApi`.
    Observations,
    /// `organizations` on the central plane, served by `OrganizationsApi`.
    Organizations,
    /// `otlp` on the regional plane, served by `OtlpApi`.
    Otlp,
    /// `provider-credentials` on the regional plane, served by `ProviderCredentialsApi`.
    ProviderCredentials,
    /// `operations` on the regional plane, served by `RegionalOperationsApi`.
    RegionalOperations,
    /// `registry` on the regional plane, served by `RegistryApi`.
    Registry,
    /// `sessions` on the regional plane, served by `SessionsApi`.
    Sessions,
    /// `telemetry-lifecycle` on the regional plane, served by `TelemetryLifecycleApi`.
    TelemetryLifecycle,
    /// `uploads` on the regional plane, served by `UploadsApi`.
    Uploads,
    /// `usage` on the regional plane, served by `UsageApi`.
    Usage,
    /// `workspace` on the regional plane, served by `WorkspaceApi`.
    Workspace,
    /// `workspaces` on the central plane, served by `WorkspacesApi`.
    Workspaces,
}

impl RouteGroup {
    /// Every group, in key order.
    pub const ALL: &'static [RouteGroup] = &[
        RouteGroup::ApiKeys,
        RouteGroup::Auth,
        RouteGroup::Billing,
        RouteGroup::Bootstrap,
        RouteGroup::CentralOperations,
        RouteGroup::Files,
        RouteGroup::Identity,
        RouteGroup::Observations,
        RouteGroup::Organizations,
        RouteGroup::Otlp,
        RouteGroup::ProviderCredentials,
        RouteGroup::RegionalOperations,
        RouteGroup::Registry,
        RouteGroup::Sessions,
        RouteGroup::TelemetryLifecycle,
        RouteGroup::Uploads,
        RouteGroup::Usage,
        RouteGroup::Workspace,
        RouteGroup::Workspaces,
    ];

    /// The stable `<plane>:<fragment>` key.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ApiKeys => "central:api-keys",
            Self::Auth => "central:auth",
            Self::Billing => "central:billing",
            Self::Bootstrap => "central:bootstrap",
            Self::CentralOperations => "central:operations",
            Self::Files => "regional:files",
            Self::Identity => "central:identity",
            Self::Observations => "regional:observations",
            Self::Organizations => "central:organizations",
            Self::Otlp => "regional:otlp",
            Self::ProviderCredentials => "regional:provider-credentials",
            Self::RegionalOperations => "regional:operations",
            Self::Registry => "regional:registry",
            Self::Sessions => "regional:sessions",
            Self::TelemetryLifecycle => "regional:telemetry-lifecycle",
            Self::Uploads => "regional:uploads",
            Self::Usage => "regional:usage",
            Self::Workspace => "regional:workspace",
            Self::Workspaces => "central:workspaces",
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
            Self::CentralOperations => Plane::Central,
            Self::Files => Plane::Regional,
            Self::Identity => Plane::Central,
            Self::Observations => Plane::Regional,
            Self::Organizations => Plane::Central,
            Self::Otlp => Plane::Regional,
            Self::ProviderCredentials => Plane::Regional,
            Self::RegionalOperations => Plane::Regional,
            Self::Registry => Plane::Regional,
            Self::Sessions => Plane::Regional,
            Self::TelemetryLifecycle => Plane::Regional,
            Self::Uploads => Plane::Regional,
            Self::Usage => Plane::Regional,
            Self::Workspace => Plane::Regional,
            Self::Workspaces => Plane::Central,
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
            Self::CentralOperations => "CentralOperationsApi",
            Self::Files => "FilesApi",
            Self::Identity => "IdentityApi",
            Self::Observations => "ObservationsApi",
            Self::Organizations => "OrganizationsApi",
            Self::Otlp => "OtlpApi",
            Self::ProviderCredentials => "ProviderCredentialsApi",
            Self::RegionalOperations => "RegionalOperationsApi",
            Self::Registry => "RegistryApi",
            Self::Sessions => "SessionsApi",
            Self::TelemetryLifecycle => "TelemetryLifecycleApi",
            Self::Uploads => "UploadsApi",
            Self::Usage => "UsageApi",
            Self::Workspace => "WorkspaceApi",
            Self::Workspaces => "WorkspacesApi",
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
            Self::CentralOperations => CENTRAL_OPERATIONS_ROUTES,
            Self::Files => FILES_ROUTES,
            Self::Identity => IDENTITY_ROUTES,
            Self::Observations => OBSERVATIONS_ROUTES,
            Self::Organizations => ORGANIZATIONS_ROUTES,
            Self::Otlp => OTLP_ROUTES,
            Self::ProviderCredentials => PROVIDER_CREDENTIALS_ROUTES,
            Self::RegionalOperations => REGIONAL_OPERATIONS_ROUTES,
            Self::Registry => REGISTRY_ROUTES,
            Self::Sessions => SESSIONS_ROUTES,
            Self::TelemetryLifecycle => TELEMETRY_LIFECYCLE_ROUTES,
            Self::Uploads => UPLOADS_ROUTES,
            Self::Usage => USAGE_ROUTES,
            Self::Workspace => WORKSPACE_ROUTES,
            Self::Workspaces => WORKSPACES_ROUTES,
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
    RouteId::DeviceAuthorizationCreate,
    RouteId::DeviceDecisionCreate,
    RouteId::DeviceTokenCreate,
];

/// Every route of `central:billing`, in `RouteId` order.
pub const BILLING_ROUTES: &[RouteId] = &[
    RouteId::BillingAutoTopupPolicyGet,
    RouteId::BillingAutoTopupPolicyPut,
    RouteId::BillingBalanceGet,
    RouteId::BillingPortalSessionCreate,
    RouteId::BillingStatementDownloadCreate,
    RouteId::BillingStatementGet,
    RouteId::BillingStatementsList,
    RouteId::BillingTopUpCheckoutCreate,
];

/// Every route of `central:bootstrap`, in `RouteId` order.
pub const BOOTSTRAP_ROUTES: &[RouteId] = &[RouteId::DashboardBootstrapGet];

/// Every route of `central:operations`, in `RouteId` order.
pub const CENTRAL_OPERATIONS_ROUTES: &[RouteId] = &[
    RouteId::CentralOperationCancel,
    RouteId::CentralOperationGet,
    RouteId::CentralOperationsList,
];

/// Every route of `regional:files`, in `RouteId` order.
pub const FILES_ROUTES: &[RouteId] = &[
    RouteId::SessionFilesLiveDownloadComplete,
    RouteId::SessionFilesLiveDownloadCreate,
    RouteId::SessionFilesLiveDownloadDelete,
    RouteId::SessionFilesLiveDownloadPartGet,
    RouteId::SessionFilesLiveList,
    RouteId::SessionFilesLiveStat,
    RouteId::SessionFilesLiveUploadComplete,
    RouteId::SessionFilesLiveUploadCreate,
    RouteId::SessionFilesLiveUploadDelete,
    RouteId::SessionFilesLiveUploadGet,
    RouteId::SessionFilesLiveUploadPartPut,
];

/// Every route of `central:identity`, in `RouteId` order.
pub const IDENTITY_ROUTES: &[RouteId] = &[RouteId::AccountGet];

/// Every route of `regional:observations`, in `RouteId` order.
pub const OBSERVATIONS_ROUTES: &[RouteId] = &[
    RouteId::ObservationsEventsListen,
    RouteId::ObservationsEventsQuery,
    RouteId::ObservationsEventsStream,
    RouteId::ObservationsLogsListen,
    RouteId::ObservationsLogsQuery,
    RouteId::ObservationsLogsStream,
    RouteId::ObservationsMetricsAggregate,
    RouteId::ObservationsMetricsListen,
    RouteId::ObservationsMetricsQuery,
    RouteId::ObservationsMetricsStream,
    RouteId::ObservationsSpansListen,
    RouteId::ObservationsSpansQuery,
    RouteId::ObservationsSpansStream,
    RouteId::ObservationsTelemetryListen,
    RouteId::ObservationsTelemetryQuery,
    RouteId::ObservationsTelemetryStream,
    RouteId::ObservationsTracesListen,
    RouteId::ObservationsTracesQuery,
    RouteId::ObservationsTracesStream,
    RouteId::SessionObservationsEventsListen,
    RouteId::SessionObservationsEventsQuery,
    RouteId::SessionObservationsEventsStream,
    RouteId::SessionObservationsLogsListen,
    RouteId::SessionObservationsLogsQuery,
    RouteId::SessionObservationsLogsStream,
    RouteId::SessionObservationsMetricsAggregate,
    RouteId::SessionObservationsMetricsListen,
    RouteId::SessionObservationsMetricsQuery,
    RouteId::SessionObservationsMetricsStream,
    RouteId::SessionObservationsSpansListen,
    RouteId::SessionObservationsSpansQuery,
    RouteId::SessionObservationsSpansStream,
    RouteId::SessionObservationsTelemetryListen,
    RouteId::SessionObservationsTelemetryQuery,
    RouteId::SessionObservationsTelemetryStream,
    RouteId::SessionObservationsTraceGet,
    RouteId::SessionObservationsTracesListen,
    RouteId::SessionObservationsTracesQuery,
    RouteId::SessionObservationsTracesStream,
];

/// Every route of `central:organizations`, in `RouteId` order.
pub const ORGANIZATIONS_ROUTES: &[RouteId] = &[
    RouteId::InvitationAccept,
    RouteId::InvitationCreate,
    RouteId::MembershipsList,
    RouteId::OrganizationCreate,
    RouteId::OrganizationGet,
    RouteId::OrganizationsList,
];

/// Every route of `regional:otlp`, in `RouteId` order.
pub const OTLP_ROUTES: &[RouteId] = &[
    RouteId::OtlpLogsIngest,
    RouteId::OtlpMetricsIngest,
    RouteId::OtlpTracesIngest,
];

/// Every route of `regional:provider-credentials`, in `RouteId` order.
pub const PROVIDER_CREDENTIALS_ROUTES: &[RouteId] = &[
    RouteId::ProviderCredentialGet,
    RouteId::ProviderCredentialRegister,
    RouteId::ProviderCredentialRevoke,
    RouteId::ProviderCredentialsList,
];

/// Every route of `regional:operations`, in `RouteId` order.
pub const REGIONAL_OPERATIONS_ROUTES: &[RouteId] = &[
    RouteId::RegionalOperationCancel,
    RouteId::RegionalOperationGet,
    RouteId::RegionalOperationsList,
];

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
    RouteId::SessionResume,
    RouteId::SessionSuspend,
    RouteId::SessionTerminate,
    RouteId::SessionsList,
];

/// Every route of `regional:telemetry-lifecycle`, in `RouteId` order.
pub const TELEMETRY_LIFECYCLE_ROUTES: &[RouteId] = &[
    RouteId::SessionTelemetryExportCreate,
    RouteId::SessionTelemetryExportDownloadCreate,
    RouteId::SessionTelemetryExportGet,
    RouteId::SessionTelemetryExportRevoke,
    RouteId::SessionTelemetryGapGet,
    RouteId::SessionTelemetryGapsQuery,
    RouteId::TelemetryExportCreate,
    RouteId::TelemetryExportDownloadCreate,
    RouteId::TelemetryExportGet,
    RouteId::TelemetryExportRevoke,
    RouteId::TelemetryGapGet,
    RouteId::TelemetryGapsQuery,
];

/// Every route of `regional:uploads`, in `RouteId` order.
pub const UPLOADS_ROUTES: &[RouteId] = &[
    RouteId::UploadAbort,
    RouteId::UploadComplete,
    RouteId::UploadCreate,
    RouteId::UploadPartsGrant,
];

/// Every route of `regional:usage`, in `RouteId` order.
pub const USAGE_ROUTES: &[RouteId] = &[RouteId::UsageQuery];

/// Every route of `regional:workspace`, in `RouteId` order.
pub const WORKSPACE_ROUTES: &[RouteId] = &[
    RouteId::WorkspaceCurrentGet,
    RouteId::WorkspaceLimitGet,
    RouteId::WorkspaceLimitsList,
];

/// Every route of `central:workspaces`, in `RouteId` order.
pub const WORKSPACES_ROUTES: &[RouteId] = &[
    RouteId::WorkspaceCreate,
    RouteId::WorkspaceDelete,
    RouteId::WorkspaceGet,
    RouteId::WorkspacesList,
];

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
            Self::DeviceAuthorizationCreate => RouteGroup::Auth,
            Self::DeviceDecisionCreate => RouteGroup::Auth,
            Self::DeviceTokenCreate => RouteGroup::Auth,
            Self::BillingAutoTopupPolicyGet => RouteGroup::Billing,
            Self::BillingAutoTopupPolicyPut => RouteGroup::Billing,
            Self::BillingBalanceGet => RouteGroup::Billing,
            Self::BillingPortalSessionCreate => RouteGroup::Billing,
            Self::BillingStatementDownloadCreate => RouteGroup::Billing,
            Self::BillingStatementGet => RouteGroup::Billing,
            Self::BillingStatementsList => RouteGroup::Billing,
            Self::BillingTopUpCheckoutCreate => RouteGroup::Billing,
            Self::DashboardBootstrapGet => RouteGroup::Bootstrap,
            Self::CentralOperationCancel => RouteGroup::CentralOperations,
            Self::CentralOperationGet => RouteGroup::CentralOperations,
            Self::CentralOperationsList => RouteGroup::CentralOperations,
            Self::SessionFilesLiveDownloadComplete => RouteGroup::Files,
            Self::SessionFilesLiveDownloadCreate => RouteGroup::Files,
            Self::SessionFilesLiveDownloadDelete => RouteGroup::Files,
            Self::SessionFilesLiveDownloadPartGet => RouteGroup::Files,
            Self::SessionFilesLiveList => RouteGroup::Files,
            Self::SessionFilesLiveStat => RouteGroup::Files,
            Self::SessionFilesLiveUploadComplete => RouteGroup::Files,
            Self::SessionFilesLiveUploadCreate => RouteGroup::Files,
            Self::SessionFilesLiveUploadDelete => RouteGroup::Files,
            Self::SessionFilesLiveUploadGet => RouteGroup::Files,
            Self::SessionFilesLiveUploadPartPut => RouteGroup::Files,
            Self::AccountGet => RouteGroup::Identity,
            Self::ObservationsEventsListen => RouteGroup::Observations,
            Self::ObservationsEventsQuery => RouteGroup::Observations,
            Self::ObservationsEventsStream => RouteGroup::Observations,
            Self::ObservationsLogsListen => RouteGroup::Observations,
            Self::ObservationsLogsQuery => RouteGroup::Observations,
            Self::ObservationsLogsStream => RouteGroup::Observations,
            Self::ObservationsMetricsAggregate => RouteGroup::Observations,
            Self::ObservationsMetricsListen => RouteGroup::Observations,
            Self::ObservationsMetricsQuery => RouteGroup::Observations,
            Self::ObservationsMetricsStream => RouteGroup::Observations,
            Self::ObservationsSpansListen => RouteGroup::Observations,
            Self::ObservationsSpansQuery => RouteGroup::Observations,
            Self::ObservationsSpansStream => RouteGroup::Observations,
            Self::ObservationsTelemetryListen => RouteGroup::Observations,
            Self::ObservationsTelemetryQuery => RouteGroup::Observations,
            Self::ObservationsTelemetryStream => RouteGroup::Observations,
            Self::ObservationsTracesListen => RouteGroup::Observations,
            Self::ObservationsTracesQuery => RouteGroup::Observations,
            Self::ObservationsTracesStream => RouteGroup::Observations,
            Self::SessionObservationsEventsListen => RouteGroup::Observations,
            Self::SessionObservationsEventsQuery => RouteGroup::Observations,
            Self::SessionObservationsEventsStream => RouteGroup::Observations,
            Self::SessionObservationsLogsListen => RouteGroup::Observations,
            Self::SessionObservationsLogsQuery => RouteGroup::Observations,
            Self::SessionObservationsLogsStream => RouteGroup::Observations,
            Self::SessionObservationsMetricsAggregate => RouteGroup::Observations,
            Self::SessionObservationsMetricsListen => RouteGroup::Observations,
            Self::SessionObservationsMetricsQuery => RouteGroup::Observations,
            Self::SessionObservationsMetricsStream => RouteGroup::Observations,
            Self::SessionObservationsSpansListen => RouteGroup::Observations,
            Self::SessionObservationsSpansQuery => RouteGroup::Observations,
            Self::SessionObservationsSpansStream => RouteGroup::Observations,
            Self::SessionObservationsTelemetryListen => RouteGroup::Observations,
            Self::SessionObservationsTelemetryQuery => RouteGroup::Observations,
            Self::SessionObservationsTelemetryStream => RouteGroup::Observations,
            Self::SessionObservationsTraceGet => RouteGroup::Observations,
            Self::SessionObservationsTracesListen => RouteGroup::Observations,
            Self::SessionObservationsTracesQuery => RouteGroup::Observations,
            Self::SessionObservationsTracesStream => RouteGroup::Observations,
            Self::InvitationAccept => RouteGroup::Organizations,
            Self::InvitationCreate => RouteGroup::Organizations,
            Self::MembershipsList => RouteGroup::Organizations,
            Self::OrganizationCreate => RouteGroup::Organizations,
            Self::OrganizationGet => RouteGroup::Organizations,
            Self::OrganizationsList => RouteGroup::Organizations,
            Self::OtlpLogsIngest => RouteGroup::Otlp,
            Self::OtlpMetricsIngest => RouteGroup::Otlp,
            Self::OtlpTracesIngest => RouteGroup::Otlp,
            Self::ProviderCredentialGet => RouteGroup::ProviderCredentials,
            Self::ProviderCredentialRegister => RouteGroup::ProviderCredentials,
            Self::ProviderCredentialRevoke => RouteGroup::ProviderCredentials,
            Self::ProviderCredentialsList => RouteGroup::ProviderCredentials,
            Self::RegionalOperationCancel => RouteGroup::RegionalOperations,
            Self::RegionalOperationGet => RouteGroup::RegionalOperations,
            Self::RegionalOperationsList => RouteGroup::RegionalOperations,
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
            Self::SessionResume => RouteGroup::Sessions,
            Self::SessionSuspend => RouteGroup::Sessions,
            Self::SessionTerminate => RouteGroup::Sessions,
            Self::SessionsList => RouteGroup::Sessions,
            Self::SessionTelemetryExportCreate => RouteGroup::TelemetryLifecycle,
            Self::SessionTelemetryExportDownloadCreate => RouteGroup::TelemetryLifecycle,
            Self::SessionTelemetryExportGet => RouteGroup::TelemetryLifecycle,
            Self::SessionTelemetryExportRevoke => RouteGroup::TelemetryLifecycle,
            Self::SessionTelemetryGapGet => RouteGroup::TelemetryLifecycle,
            Self::SessionTelemetryGapsQuery => RouteGroup::TelemetryLifecycle,
            Self::TelemetryExportCreate => RouteGroup::TelemetryLifecycle,
            Self::TelemetryExportDownloadCreate => RouteGroup::TelemetryLifecycle,
            Self::TelemetryExportGet => RouteGroup::TelemetryLifecycle,
            Self::TelemetryExportRevoke => RouteGroup::TelemetryLifecycle,
            Self::TelemetryGapGet => RouteGroup::TelemetryLifecycle,
            Self::TelemetryGapsQuery => RouteGroup::TelemetryLifecycle,
            Self::UploadAbort => RouteGroup::Uploads,
            Self::UploadComplete => RouteGroup::Uploads,
            Self::UploadCreate => RouteGroup::Uploads,
            Self::UploadPartsGrant => RouteGroup::Uploads,
            Self::UsageQuery => RouteGroup::Usage,
            Self::WorkspaceCurrentGet => RouteGroup::Workspace,
            Self::WorkspaceLimitGet => RouteGroup::Workspace,
            Self::WorkspaceLimitsList => RouteGroup::Workspace,
            Self::WorkspaceCreate => RouteGroup::Workspaces,
            Self::WorkspaceDelete => RouteGroup::Workspaces,
            Self::WorkspaceGet => RouteGroup::Workspaces,
            Self::WorkspacesList => RouteGroup::Workspaces,
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

from_param_enum!(OperationKind, OperationStatus, SessionStatus);

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

/// The `auth` fragment of the central plane: 5 operations.
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

    /// `POST /api/auth/device/authorizations`
    /// Begin the CLI device-authorization flow.
    fn device_authorization_create(
        &self,
        cx: &RequestContext,
        body: DeviceAuthorizationRequest,
    ) -> impl Future<Output = WireResult<Created<DeviceAuthorization>>> + Send;

    /// `POST /api/auth/device/decisions`
    /// Approve or deny a pending device authorization.
    fn device_decision_create(
        &self,
        cx: &RequestContext,
        body: DeviceDecisionRequest,
    ) -> impl Future<Output = WireResult<DeviceDecisionResult>> + Send;

    /// `POST /api/auth/device/tokens`
    /// Exchange an approved device code for an account token.
    fn device_token_create(
        &self,
        cx: &RequestContext,
        body: DeviceTokenRequest,
    ) -> impl Future<Output = WireResult<DeviceToken>> + Send;
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
        RouteId::DeviceAuthorizationCreate => {
            let body = decode_body::<DeviceAuthorizationRequest>(&raw, limits)?;
            let handled = api.device_authorization_create(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(201, &answer.0)?))
        }
        RouteId::DeviceDecisionCreate => {
            let body = decode_body::<DeviceDecisionRequest>(&raw, limits)?;
            let handled = api.device_decision_create(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::DeviceTokenCreate => {
            let body = decode_body::<DeviceTokenRequest>(&raw, limits)?;
            let handled = api.device_token_create(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        other => Err(wrong_group(other, "central:auth")),
    }
}

// --- central:billing ---------------------------------------------------------------

/// The `billing` fragment of the central plane: 8 operations.
/// Every method returns a future that is `Send`, so the composition crate can spawn it without
/// wrapping. A method never names a status: the response type it returns is the status the route
/// declares.
pub trait BillingApi: Send + Sync + 'static {
    /// `GET /api/organizations/{organizationId}/billing/auto-topup-policy`
    /// Read the automatic top-up policy.
    fn billing_auto_topup_policy_get(
        &self,
        cx: &RequestContext,
        organization_id: OrganizationId,
    ) -> impl Future<Output = WireResult<WithETag<AutoTopupPolicy>>> + Send;

    /// `PUT /api/organizations/{organizationId}/billing/auto-topup-policy`
    /// Replace the automatic top-up policy.
    fn billing_auto_topup_policy_put(
        &self,
        cx: &RequestContext,
        organization_id: OrganizationId,
        body: AutoTopupPolicyRequest,
    ) -> impl Future<Output = WireResult<WithETag<AutoTopupPolicy>>> + Send;

    /// `GET /api/billing/balance`
    /// Read the prepaid balance of an organization.
    fn billing_balance_get(
        &self,
        cx: &RequestContext,
        query: BillingBalanceGetQuery,
    ) -> impl Future<Output = WireResult<BillingBalance>> + Send;

    /// `POST /api/organizations/{organizationId}/billing/portal-sessions`
    /// Create a hosted billing portal session.
    fn billing_portal_session_create(
        &self,
        cx: &RequestContext,
        organization_id: OrganizationId,
        body: PortalSessionRequest,
    ) -> impl Future<Output = WireResult<Created<HostedSession>>> + Send;

    /// `POST /api/organizations/{organizationId}/billing/statements/{statementId}/downloads`
    /// Mint a download grant for an issued statement.
    fn billing_statement_download_create(
        &self,
        cx: &RequestContext,
        organization_id: OrganizationId,
        statement_id: StatementId,
        body: EmptyRequest,
    ) -> impl Future<Output = WireResult<Created<DownloadGrant>>> + Send;

    /// `GET /api/organizations/{organizationId}/billing/statements/{statementId}`
    /// Read one immutable issued statement.
    fn billing_statement_get(
        &self,
        cx: &RequestContext,
        organization_id: OrganizationId,
        statement_id: StatementId,
    ) -> impl Future<Output = WireResult<Statement>> + Send;

    /// `GET /api/organizations/{organizationId}/billing/statements`
    /// List issued statements, newest first.
    fn billing_statements_list(
        &self,
        cx: &RequestContext,
        organization_id: OrganizationId,
        query: BillingStatementsListQuery,
    ) -> impl Future<Output = WireResult<StatementSummaryPage>> + Send;

    /// `POST /api/organizations/{organizationId}/billing/top-up-checkouts`
    /// Create a hosted top-up checkout.
    fn billing_top_up_checkout_create(
        &self,
        cx: &RequestContext,
        organization_id: OrganizationId,
        body: TopUpCheckoutRequest,
    ) -> impl Future<Output = WireResult<Created<HostedSession>>> + Send;
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
        RouteId::BillingAutoTopupPolicyGet => {
            let organization_id = path_param::<OrganizationId>(&raw, "organizationId")?;
            expect_no_body(&raw)?;
            let handled = api.billing_auto_topup_policy_get(cx, organization_id);
            let answer = declared(raw.route, handled.await)?;
            let rendered = RawResponse::json(200, &answer.value)?;
            let rendered = rendered.with_etag(answer.etag);
            Ok(DispatchOutcome::Unary(rendered))
        }
        RouteId::BillingAutoTopupPolicyPut => {
            let organization_id = path_param::<OrganizationId>(&raw, "organizationId")?;
            let body = decode_body::<AutoTopupPolicyRequest>(&raw, limits)?;
            let handled = api.billing_auto_topup_policy_put(cx, organization_id, body);
            let answer = declared(raw.route, handled.await)?;
            let rendered = RawResponse::json(200, &answer.value)?;
            let rendered = rendered.with_etag(answer.etag);
            Ok(DispatchOutcome::Unary(rendered))
        }
        RouteId::BillingBalanceGet => {
            let query = BillingBalanceGetQuery {
                organization_id: reader.optional("organizationId")?,
            };
            expect_no_body(&raw)?;
            let handled = api.billing_balance_get(cx, query);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::BillingPortalSessionCreate => {
            let organization_id = path_param::<OrganizationId>(&raw, "organizationId")?;
            let body = decode_body::<PortalSessionRequest>(&raw, limits)?;
            let handled = api.billing_portal_session_create(cx, organization_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(201, &answer.0)?))
        }
        RouteId::BillingStatementDownloadCreate => {
            let organization_id = path_param::<OrganizationId>(&raw, "organizationId")?;
            let statement_id = path_param::<StatementId>(&raw, "statementId")?;
            let body = decode_body::<EmptyRequest>(&raw, limits)?;
            let handled =
                api.billing_statement_download_create(cx, organization_id, statement_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(201, &answer.0)?))
        }
        RouteId::BillingStatementGet => {
            let organization_id = path_param::<OrganizationId>(&raw, "organizationId")?;
            let statement_id = path_param::<StatementId>(&raw, "statementId")?;
            expect_no_body(&raw)?;
            let handled = api.billing_statement_get(cx, organization_id, statement_id);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::BillingStatementsList => {
            let organization_id = path_param::<OrganizationId>(&raw, "organizationId")?;
            let query = BillingStatementsListQuery {
                cursor: reader.optional("cursor")?,
                limit: reader.optional_bounded("limit", 1, 1000)?,
            };
            expect_no_body(&raw)?;
            let handled = api.billing_statements_list(cx, organization_id, query);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::BillingTopUpCheckoutCreate => {
            let organization_id = path_param::<OrganizationId>(&raw, "organizationId")?;
            let body = decode_body::<TopUpCheckoutRequest>(&raw, limits)?;
            let handled = api.billing_top_up_checkout_create(cx, organization_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(201, &answer.0)?))
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

// --- central:operations ---------------------------------------------------------------

/// The `operations` fragment of the central plane: 3 operations.
/// Every method returns a future that is `Send`, so the composition crate can spawn it without
/// wrapping. A method never names a status: the response type it returns is the status the route
/// declares.
pub trait CentralOperationsApi: Send + Sync + 'static {
    /// `POST /api/operations/{operationId}/cancellations`
    /// Request cancellation of a central operation.
    fn central_operation_cancel(
        &self,
        cx: &RequestContext,
        operation_id: OperationId,
        body: EmptyRequest,
    ) -> impl Future<Output = WireResult<Operation>> + Send;

    /// `GET /api/operations/{operationId}`
    /// Read one central durable operation.
    fn central_operation_get(
        &self,
        cx: &RequestContext,
        operation_id: OperationId,
    ) -> impl Future<Output = WireResult<Operation>> + Send;

    /// `GET /api/operations`
    /// List central durable operations for one organization.
    fn central_operations_list(
        &self,
        cx: &RequestContext,
        query: CentralOperationsListQuery,
    ) -> impl Future<Output = WireResult<OperationPage>> + Send;
}

/// Decodes, calls and encodes one `central:operations` request.
/// Total over `RouteId`: a route from another group is an internal error naming the mismatch, never
/// a silently wrong handler.
/// # Errors
/// Returns the handler's own declared failure, or a decode failure the route declares. A code the
/// route does not declare is refused at this boundary.
pub async fn dispatch_central_operations<A: CentralOperationsApi + ?Sized>(
    api: &A,
    cx: &RequestContext,
    raw: RawRequest<'_>,
    limits: RequestLimits,
) -> WireResult<DispatchOutcome<crate::dispatch::NoStream>> {
    let reader = QueryReader::parse(raw.route, raw.query)?;
    match raw.route {
        RouteId::CentralOperationCancel => {
            let operation_id = path_param::<OperationId>(&raw, "operationId")?;
            let body = decode_body::<EmptyRequest>(&raw, limits)?;
            let handled = api.central_operation_cancel(cx, operation_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::CentralOperationGet => {
            let operation_id = path_param::<OperationId>(&raw, "operationId")?;
            expect_no_body(&raw)?;
            let handled = api.central_operation_get(cx, operation_id);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::CentralOperationsList => {
            let query = CentralOperationsListQuery {
                cursor: reader.optional("cursor")?,
                kind: reader.optional("kind")?,
                limit: reader.optional_bounded("limit", 1, 1000)?,
                organization_id: reader.required("organizationId")?,
                status: reader.optional("status")?,
            };
            expect_no_body(&raw)?;
            let handled = api.central_operations_list(cx, query);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        other => Err(wrong_group(other, "central:operations")),
    }
}

// --- regional:files ---------------------------------------------------------------

/// The `files` fragment of the regional plane: 11 operations.
/// Every method returns a future that is `Send`, so the composition crate can spawn it without
/// wrapping. A method never names a status: the response type it returns is the status the route
/// declares.
pub trait FilesApi: Send + Sync + 'static {
    /// `POST /api/sessions/{sessionId}/files/live/downloads/{fileDownloadId}/completion`
    /// Re-stat, re-hash, and close an exact live-file download after SDK verification.
    fn session_files_live_download_complete(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        file_download_id: FileDownloadId,
        body: LiveFileDownloadCompleteRequest,
    ) -> impl Future<Output = WireResult<LiveFileDownload>> + Send;

    /// `POST /api/sessions/{sessionId}/files/live/downloads`
    /// Open one exact-version live file directly from the retained generation, auto-resuming it.
    fn session_files_live_download_create(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: LiveFileDownloadRequest,
    ) -> impl Future<Output = WireResult<Created<LiveFileDownload>>> + Send;

    /// `DELETE /api/sessions/{sessionId}/files/live/downloads/{fileDownloadId}`
    /// Close an abandoned live-file descriptor, auto-resuming the same generation.
    fn session_files_live_download_delete(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        file_download_id: FileDownloadId,
    ) -> impl Future<Output = WireResult<NoContent>> + Send;

    /// `GET /api/sessions/{sessionId}/files/live/downloads/{fileDownloadId}/parts/{partNumber}`
    /// Read one raw exact-version range directly from the retained generation.
    fn session_files_live_download_part_get(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        file_download_id: FileDownloadId,
        part_number: u32,
        query: SessionFilesLiveDownloadPartGetQuery,
    ) -> impl Future<Output = WireResult<BinaryBody>> + Send;

    /// `POST /api/sessions/{sessionId}/files/live/list`
    /// List the exact live generation, auto-resuming that same generation when suspended.
    fn session_files_live_list(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: LiveFileListRequest,
    ) -> impl Future<Output = WireResult<LiveFileEntryPage>> + Send;

    /// `POST /api/sessions/{sessionId}/files/live/stat`
    /// Stat one live path without following a symlink, auto-resuming the same generation.
    fn session_files_live_stat(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: LiveFileStatRequest,
    ) -> impl Future<Output = WireResult<LiveFileEntry>> + Send;

    /// `POST /api/sessions/{sessionId}/files/live/uploads/{fileUploadId}/completion`
    /// Verify every part and atomically publish one live file in the same generation.
    fn session_files_live_upload_complete(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        file_upload_id: FileUploadId,
    ) -> impl Future<Output = WireResult<LiveFileUpload>> + Send;

    /// `POST /api/sessions/{sessionId}/files/live/uploads`
    /// Create a resumable upload in the exact live generation, auto-resuming it when suspended.
    fn session_files_live_upload_create(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: LiveFileUploadCreateRequest,
    ) -> impl Future<Output = WireResult<Created<LiveFileUpload>>> + Send;

    /// `DELETE /api/sessions/{sessionId}/files/live/uploads/{fileUploadId}`
    /// Abort a partial upload, auto-resuming the same generation to remove its temporary bytes.
    fn session_files_live_upload_delete(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        file_upload_id: FileUploadId,
    ) -> impl Future<Output = WireResult<NoContent>> + Send;

    /// `GET /api/sessions/{sessionId}/files/live/uploads/{fileUploadId}`
    /// Read resumable live-upload state, auto-resuming the same generation.
    fn session_files_live_upload_get(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        file_upload_id: FileUploadId,
    ) -> impl Future<Output = WireResult<LiveFileUpload>> + Send;

    /// `PUT /api/sessions/{sessionId}/files/live/uploads/{fileUploadId}/parts/{partNumber}`
    /// Put one exact logical part directly into the retained generation.
    fn session_files_live_upload_part_put(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        file_upload_id: FileUploadId,
        part_number: u32,
        query: SessionFilesLiveUploadPartPutQuery,
        body: &[u8],
    ) -> impl Future<Output = WireResult<LiveFileUpload>> + Send;
}

/// Decodes, calls and encodes one `regional:files` request.
/// Total over `RouteId`: a route from another group is an internal error naming the mismatch, never
/// a silently wrong handler.
/// # Errors
/// Returns the handler's own declared failure, or a decode failure the route declares. A code the
/// route does not declare is refused at this boundary.
pub async fn dispatch_files<A: FilesApi + ?Sized>(
    api: &A,
    cx: &RequestContext,
    raw: RawRequest<'_>,
    limits: RequestLimits,
) -> WireResult<DispatchOutcome<crate::dispatch::NoStream>> {
    let reader = QueryReader::parse(raw.route, raw.query)?;
    match raw.route {
        RouteId::SessionFilesLiveDownloadComplete => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let file_download_id = path_param::<FileDownloadId>(&raw, "fileDownloadId")?;
            let body = decode_body::<LiveFileDownloadCompleteRequest>(&raw, limits)?;
            let handled =
                api.session_files_live_download_complete(cx, session_id, file_download_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::SessionFilesLiveDownloadCreate => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<LiveFileDownloadRequest>(&raw, limits)?;
            let handled = api.session_files_live_download_create(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(201, &answer.0)?))
        }
        RouteId::SessionFilesLiveDownloadDelete => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let file_download_id = path_param::<FileDownloadId>(&raw, "fileDownloadId")?;
            expect_no_body(&raw)?;
            let handled = api.session_files_live_download_delete(cx, session_id, file_download_id);
            let NoContent = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::no_content()))
        }
        RouteId::SessionFilesLiveDownloadPartGet => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let file_download_id = path_param::<FileDownloadId>(&raw, "fileDownloadId")?;
            let part_number = path_param::<u32>(&raw, "partNumber")?;
            let query = SessionFilesLiveDownloadPartGetQuery {
                version: reader.required("version")?,
            };
            expect_no_body(&raw)?;
            let handled = api.session_files_live_download_part_get(
                cx,
                session_id,
                file_download_id,
                part_number,
                query,
            );
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::binary(200, answer.0)))
        }
        RouteId::SessionFilesLiveList => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<LiveFileListRequest>(&raw, limits)?;
            let handled = api.session_files_live_list(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::SessionFilesLiveStat => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<LiveFileStatRequest>(&raw, limits)?;
            let handled = api.session_files_live_stat(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::SessionFilesLiveUploadComplete => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let file_upload_id = path_param::<FileUploadId>(&raw, "fileUploadId")?;
            expect_no_body(&raw)?;
            let handled = api.session_files_live_upload_complete(cx, session_id, file_upload_id);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::SessionFilesLiveUploadCreate => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<LiveFileUploadCreateRequest>(&raw, limits)?;
            let handled = api.session_files_live_upload_create(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(201, &answer.0)?))
        }
        RouteId::SessionFilesLiveUploadDelete => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let file_upload_id = path_param::<FileUploadId>(&raw, "fileUploadId")?;
            expect_no_body(&raw)?;
            let handled = api.session_files_live_upload_delete(cx, session_id, file_upload_id);
            let NoContent = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::no_content()))
        }
        RouteId::SessionFilesLiveUploadGet => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let file_upload_id = path_param::<FileUploadId>(&raw, "fileUploadId")?;
            expect_no_body(&raw)?;
            let handled = api.session_files_live_upload_get(cx, session_id, file_upload_id);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::SessionFilesLiveUploadPartPut => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let file_upload_id = path_param::<FileUploadId>(&raw, "fileUploadId")?;
            let part_number = path_param::<u32>(&raw, "partNumber")?;
            let query = SessionFilesLiveUploadPartPutQuery {
                sha256: reader.required("sha256")?,
            };
            let body = binary_body(&raw, limits)?;
            let handled = api.session_files_live_upload_part_put(
                cx,
                session_id,
                file_upload_id,
                part_number,
                query,
                body,
            );
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        other => Err(wrong_group(other, "regional:files")),
    }
}

// --- central:identity ---------------------------------------------------------------

/// The `identity` fragment of the central plane: 1 operation.
/// Every method returns a future that is `Send`, so the composition crate can spawn it without
/// wrapping. A method never names a status: the response type it returns is the status the route
/// declares.
pub trait IdentityApi: Send + Sync + 'static {
    /// `GET /api/account`
    /// Read the account and its operational state.
    fn account_get(
        &self,
        cx: &RequestContext,
        query: AccountGetQuery,
    ) -> impl Future<Output = WireResult<AccountOperationalState>> + Send;
}

/// Decodes, calls and encodes one `central:identity` request.
/// Total over `RouteId`: a route from another group is an internal error naming the mismatch, never
/// a silently wrong handler.
/// # Errors
/// Returns the handler's own declared failure, or a decode failure the route declares. A code the
/// route does not declare is refused at this boundary.
pub async fn dispatch_identity<A: IdentityApi + ?Sized>(
    api: &A,
    cx: &RequestContext,
    raw: RawRequest<'_>,
    _limits: RequestLimits,
) -> WireResult<DispatchOutcome<crate::dispatch::NoStream>> {
    let reader = QueryReader::parse(raw.route, raw.query)?;
    match raw.route {
        RouteId::AccountGet => {
            let query = AccountGetQuery {
                organization_id: reader.required("organizationId")?,
            };
            expect_no_body(&raw)?;
            let handled = api.account_get(cx, query);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        other => Err(wrong_group(other, "central:identity")),
    }
}

// --- regional:observations ---------------------------------------------------------------

/// The `observations` fragment of the regional plane: 39 operations.
/// Every method returns a future that is `Send`, so the composition crate can spawn it without
/// wrapping. A method never names a status: the response type it returns is the status the route
/// declares.
pub trait ObservationsApi: Send + Sync + 'static {
    /// The frame stream this implementation produces for an NDJSON route.
    /// `aex-wire` deliberately does not name `Stream`: it has no async dependency, so the
    /// composition crate supplies the concrete type and its own bound.
    type FrameStream: Send + 'static;

    /// `POST /api/streams/events/listen`
    /// Listen for new workspace events observations.
    fn observations_events_listen(
        &self,
        cx: &RequestContext,
        body: ObservationListenRequest,
    ) -> impl Future<Output = WireResult<NdjsonStream<Self::FrameStream>>> + Send;

    /// `POST /api/observations/events/query`
    /// Query workspace events observations.
    fn observations_events_query(
        &self,
        cx: &RequestContext,
        body: ObservationQuery,
    ) -> impl Future<Output = WireResult<ObservationPage>> + Send;

    /// `POST /api/streams/events/stream`
    /// Stream workspace events observations from one origin.
    fn observations_events_stream(
        &self,
        cx: &RequestContext,
        body: ObservationStreamRequest,
    ) -> impl Future<Output = WireResult<NdjsonStream<Self::FrameStream>>> + Send;

    /// `POST /api/streams/logs/listen`
    /// Listen for new workspace logs observations.
    fn observations_logs_listen(
        &self,
        cx: &RequestContext,
        body: ObservationListenRequest,
    ) -> impl Future<Output = WireResult<NdjsonStream<Self::FrameStream>>> + Send;

    /// `POST /api/observations/logs/query`
    /// Query workspace logs observations.
    fn observations_logs_query(
        &self,
        cx: &RequestContext,
        body: ObservationQuery,
    ) -> impl Future<Output = WireResult<ObservationPage>> + Send;

    /// `POST /api/streams/logs/stream`
    /// Stream workspace logs observations from one origin.
    fn observations_logs_stream(
        &self,
        cx: &RequestContext,
        body: ObservationStreamRequest,
    ) -> impl Future<Output = WireResult<NdjsonStream<Self::FrameStream>>> + Send;

    /// `POST /api/observations/metrics/aggregate`
    /// Aggregate workspace metric observations.
    fn observations_metrics_aggregate(
        &self,
        cx: &RequestContext,
        body: MetricAggregationRequest,
    ) -> impl Future<Output = WireResult<MetricAggregationPage>> + Send;

    /// `POST /api/streams/metrics/listen`
    /// Listen for new workspace metrics observations.
    fn observations_metrics_listen(
        &self,
        cx: &RequestContext,
        body: ObservationListenRequest,
    ) -> impl Future<Output = WireResult<NdjsonStream<Self::FrameStream>>> + Send;

    /// `POST /api/observations/metrics/query`
    /// Query workspace metrics observations.
    fn observations_metrics_query(
        &self,
        cx: &RequestContext,
        body: ObservationQuery,
    ) -> impl Future<Output = WireResult<ObservationPage>> + Send;

    /// `POST /api/streams/metrics/stream`
    /// Stream workspace metrics observations from one origin.
    fn observations_metrics_stream(
        &self,
        cx: &RequestContext,
        body: ObservationStreamRequest,
    ) -> impl Future<Output = WireResult<NdjsonStream<Self::FrameStream>>> + Send;

    /// `POST /api/streams/spans/listen`
    /// Listen for new workspace spans observations.
    fn observations_spans_listen(
        &self,
        cx: &RequestContext,
        body: ObservationListenRequest,
    ) -> impl Future<Output = WireResult<NdjsonStream<Self::FrameStream>>> + Send;

    /// `POST /api/observations/spans/query`
    /// Query workspace spans observations.
    fn observations_spans_query(
        &self,
        cx: &RequestContext,
        body: ObservationQuery,
    ) -> impl Future<Output = WireResult<ObservationPage>> + Send;

    /// `POST /api/streams/spans/stream`
    /// Stream workspace spans observations from one origin.
    fn observations_spans_stream(
        &self,
        cx: &RequestContext,
        body: ObservationStreamRequest,
    ) -> impl Future<Output = WireResult<NdjsonStream<Self::FrameStream>>> + Send;

    /// `POST /api/streams/telemetry/listen`
    /// Listen for new workspace telemetry observations.
    fn observations_telemetry_listen(
        &self,
        cx: &RequestContext,
        body: ObservationListenRequest,
    ) -> impl Future<Output = WireResult<NdjsonStream<Self::FrameStream>>> + Send;

    /// `POST /api/observations/telemetry/query`
    /// Query workspace telemetry observations.
    fn observations_telemetry_query(
        &self,
        cx: &RequestContext,
        body: ObservationQuery,
    ) -> impl Future<Output = WireResult<ObservationPage>> + Send;

    /// `POST /api/streams/telemetry/stream`
    /// Stream workspace telemetry observations from one origin.
    fn observations_telemetry_stream(
        &self,
        cx: &RequestContext,
        body: ObservationStreamRequest,
    ) -> impl Future<Output = WireResult<NdjsonStream<Self::FrameStream>>> + Send;

    /// `POST /api/streams/traces/listen`
    /// Listen for new workspace traces observations.
    fn observations_traces_listen(
        &self,
        cx: &RequestContext,
        body: ObservationListenRequest,
    ) -> impl Future<Output = WireResult<NdjsonStream<Self::FrameStream>>> + Send;

    /// `POST /api/observations/traces/query`
    /// Query workspace traces observations.
    fn observations_traces_query(
        &self,
        cx: &RequestContext,
        body: ObservationQuery,
    ) -> impl Future<Output = WireResult<ObservationPage>> + Send;

    /// `POST /api/streams/traces/stream`
    /// Stream workspace traces observations from one origin.
    fn observations_traces_stream(
        &self,
        cx: &RequestContext,
        body: ObservationStreamRequest,
    ) -> impl Future<Output = WireResult<NdjsonStream<Self::FrameStream>>> + Send;

    /// `POST /api/streams/{sessionId}/events/listen`
    /// Listen for new session events observations.
    fn session_observations_events_listen(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: ObservationListenRequest,
    ) -> impl Future<Output = WireResult<NdjsonStream<Self::FrameStream>>> + Send;

    /// `POST /api/observations/{sessionId}/events/query`
    /// Query session events observations.
    fn session_observations_events_query(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: ObservationQuery,
    ) -> impl Future<Output = WireResult<ObservationPage>> + Send;

    /// `POST /api/streams/{sessionId}/events/stream`
    /// Stream session events observations from one origin.
    fn session_observations_events_stream(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: ObservationStreamRequest,
    ) -> impl Future<Output = WireResult<NdjsonStream<Self::FrameStream>>> + Send;

    /// `POST /api/streams/{sessionId}/logs/listen`
    /// Listen for new session logs observations.
    fn session_observations_logs_listen(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: ObservationListenRequest,
    ) -> impl Future<Output = WireResult<NdjsonStream<Self::FrameStream>>> + Send;

    /// `POST /api/observations/{sessionId}/logs/query`
    /// Query session logs observations.
    fn session_observations_logs_query(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: ObservationQuery,
    ) -> impl Future<Output = WireResult<ObservationPage>> + Send;

    /// `POST /api/streams/{sessionId}/logs/stream`
    /// Stream session logs observations from one origin.
    fn session_observations_logs_stream(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: ObservationStreamRequest,
    ) -> impl Future<Output = WireResult<NdjsonStream<Self::FrameStream>>> + Send;

    /// `POST /api/observations/{sessionId}/metrics/aggregate`
    /// Aggregate session metric observations.
    fn session_observations_metrics_aggregate(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: MetricAggregationRequest,
    ) -> impl Future<Output = WireResult<MetricAggregationPage>> + Send;

    /// `POST /api/streams/{sessionId}/metrics/listen`
    /// Listen for new session metrics observations.
    fn session_observations_metrics_listen(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: ObservationListenRequest,
    ) -> impl Future<Output = WireResult<NdjsonStream<Self::FrameStream>>> + Send;

    /// `POST /api/observations/{sessionId}/metrics/query`
    /// Query session metrics observations.
    fn session_observations_metrics_query(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: ObservationQuery,
    ) -> impl Future<Output = WireResult<ObservationPage>> + Send;

    /// `POST /api/streams/{sessionId}/metrics/stream`
    /// Stream session metrics observations from one origin.
    fn session_observations_metrics_stream(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: ObservationStreamRequest,
    ) -> impl Future<Output = WireResult<NdjsonStream<Self::FrameStream>>> + Send;

    /// `POST /api/streams/{sessionId}/spans/listen`
    /// Listen for new session spans observations.
    fn session_observations_spans_listen(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: ObservationListenRequest,
    ) -> impl Future<Output = WireResult<NdjsonStream<Self::FrameStream>>> + Send;

    /// `POST /api/observations/{sessionId}/spans/query`
    /// Query session spans observations.
    fn session_observations_spans_query(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: ObservationQuery,
    ) -> impl Future<Output = WireResult<ObservationPage>> + Send;

    /// `POST /api/streams/{sessionId}/spans/stream`
    /// Stream session spans observations from one origin.
    fn session_observations_spans_stream(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: ObservationStreamRequest,
    ) -> impl Future<Output = WireResult<NdjsonStream<Self::FrameStream>>> + Send;

    /// `POST /api/streams/{sessionId}/telemetry/listen`
    /// Listen for new session telemetry observations.
    fn session_observations_telemetry_listen(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: ObservationListenRequest,
    ) -> impl Future<Output = WireResult<NdjsonStream<Self::FrameStream>>> + Send;

    /// `POST /api/observations/{sessionId}/telemetry/query`
    /// Query session telemetry observations.
    fn session_observations_telemetry_query(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: ObservationQuery,
    ) -> impl Future<Output = WireResult<ObservationPage>> + Send;

    /// `POST /api/streams/{sessionId}/telemetry/stream`
    /// Stream session telemetry observations from one origin.
    fn session_observations_telemetry_stream(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: ObservationStreamRequest,
    ) -> impl Future<Output = WireResult<NdjsonStream<Self::FrameStream>>> + Send;

    /// `GET /api/observations/{sessionId}/traces/{traceId}`
    /// Read one assembled trace by its W3C identifier.
    fn session_observations_trace_get(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        trace_id: TraceId,
    ) -> impl Future<Output = WireResult<TraceDetail>> + Send;

    /// `POST /api/streams/{sessionId}/traces/listen`
    /// Listen for new session traces observations.
    fn session_observations_traces_listen(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: ObservationListenRequest,
    ) -> impl Future<Output = WireResult<NdjsonStream<Self::FrameStream>>> + Send;

    /// `POST /api/observations/{sessionId}/traces/query`
    /// Query session traces observations.
    fn session_observations_traces_query(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: ObservationQuery,
    ) -> impl Future<Output = WireResult<ObservationPage>> + Send;

    /// `POST /api/streams/{sessionId}/traces/stream`
    /// Stream session traces observations from one origin.
    fn session_observations_traces_stream(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: ObservationStreamRequest,
    ) -> impl Future<Output = WireResult<NdjsonStream<Self::FrameStream>>> + Send;
}

/// Decodes, calls and encodes one `regional:observations` request.
/// Total over `RouteId`: a route from another group is an internal error naming the mismatch, never
/// a silently wrong handler.
/// # Errors
/// Returns the handler's own declared failure, or a decode failure the route declares. A code the
/// route does not declare is refused at this boundary.
pub async fn dispatch_observations<A: ObservationsApi + ?Sized>(
    api: &A,
    cx: &RequestContext,
    raw: RawRequest<'_>,
    limits: RequestLimits,
) -> WireResult<DispatchOutcome<A::FrameStream>> {
    let _reader = QueryReader::parse(raw.route, raw.query)?;
    match raw.route {
        RouteId::ObservationsEventsListen => {
            let body = decode_body::<ObservationListenRequest>(&raw, limits)?;
            let handled = api.observations_events_listen(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Ndjson(answer))
        }
        RouteId::ObservationsEventsQuery => {
            let body = decode_body::<ObservationQuery>(&raw, limits)?;
            let handled = api.observations_events_query(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::ObservationsEventsStream => {
            let body = decode_body::<ObservationStreamRequest>(&raw, limits)?;
            let handled = api.observations_events_stream(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Ndjson(answer))
        }
        RouteId::ObservationsLogsListen => {
            let body = decode_body::<ObservationListenRequest>(&raw, limits)?;
            let handled = api.observations_logs_listen(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Ndjson(answer))
        }
        RouteId::ObservationsLogsQuery => {
            let body = decode_body::<ObservationQuery>(&raw, limits)?;
            let handled = api.observations_logs_query(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::ObservationsLogsStream => {
            let body = decode_body::<ObservationStreamRequest>(&raw, limits)?;
            let handled = api.observations_logs_stream(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Ndjson(answer))
        }
        RouteId::ObservationsMetricsAggregate => {
            let body = decode_body::<MetricAggregationRequest>(&raw, limits)?;
            let handled = api.observations_metrics_aggregate(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::ObservationsMetricsListen => {
            let body = decode_body::<ObservationListenRequest>(&raw, limits)?;
            let handled = api.observations_metrics_listen(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Ndjson(answer))
        }
        RouteId::ObservationsMetricsQuery => {
            let body = decode_body::<ObservationQuery>(&raw, limits)?;
            let handled = api.observations_metrics_query(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::ObservationsMetricsStream => {
            let body = decode_body::<ObservationStreamRequest>(&raw, limits)?;
            let handled = api.observations_metrics_stream(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Ndjson(answer))
        }
        RouteId::ObservationsSpansListen => {
            let body = decode_body::<ObservationListenRequest>(&raw, limits)?;
            let handled = api.observations_spans_listen(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Ndjson(answer))
        }
        RouteId::ObservationsSpansQuery => {
            let body = decode_body::<ObservationQuery>(&raw, limits)?;
            let handled = api.observations_spans_query(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::ObservationsSpansStream => {
            let body = decode_body::<ObservationStreamRequest>(&raw, limits)?;
            let handled = api.observations_spans_stream(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Ndjson(answer))
        }
        RouteId::ObservationsTelemetryListen => {
            let body = decode_body::<ObservationListenRequest>(&raw, limits)?;
            let handled = api.observations_telemetry_listen(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Ndjson(answer))
        }
        RouteId::ObservationsTelemetryQuery => {
            let body = decode_body::<ObservationQuery>(&raw, limits)?;
            let handled = api.observations_telemetry_query(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::ObservationsTelemetryStream => {
            let body = decode_body::<ObservationStreamRequest>(&raw, limits)?;
            let handled = api.observations_telemetry_stream(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Ndjson(answer))
        }
        RouteId::ObservationsTracesListen => {
            let body = decode_body::<ObservationListenRequest>(&raw, limits)?;
            let handled = api.observations_traces_listen(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Ndjson(answer))
        }
        RouteId::ObservationsTracesQuery => {
            let body = decode_body::<ObservationQuery>(&raw, limits)?;
            let handled = api.observations_traces_query(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::ObservationsTracesStream => {
            let body = decode_body::<ObservationStreamRequest>(&raw, limits)?;
            let handled = api.observations_traces_stream(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Ndjson(answer))
        }
        RouteId::SessionObservationsEventsListen => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<ObservationListenRequest>(&raw, limits)?;
            let handled = api.session_observations_events_listen(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Ndjson(answer))
        }
        RouteId::SessionObservationsEventsQuery => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<ObservationQuery>(&raw, limits)?;
            let handled = api.session_observations_events_query(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::SessionObservationsEventsStream => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<ObservationStreamRequest>(&raw, limits)?;
            let handled = api.session_observations_events_stream(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Ndjson(answer))
        }
        RouteId::SessionObservationsLogsListen => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<ObservationListenRequest>(&raw, limits)?;
            let handled = api.session_observations_logs_listen(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Ndjson(answer))
        }
        RouteId::SessionObservationsLogsQuery => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<ObservationQuery>(&raw, limits)?;
            let handled = api.session_observations_logs_query(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::SessionObservationsLogsStream => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<ObservationStreamRequest>(&raw, limits)?;
            let handled = api.session_observations_logs_stream(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Ndjson(answer))
        }
        RouteId::SessionObservationsMetricsAggregate => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<MetricAggregationRequest>(&raw, limits)?;
            let handled = api.session_observations_metrics_aggregate(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::SessionObservationsMetricsListen => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<ObservationListenRequest>(&raw, limits)?;
            let handled = api.session_observations_metrics_listen(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Ndjson(answer))
        }
        RouteId::SessionObservationsMetricsQuery => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<ObservationQuery>(&raw, limits)?;
            let handled = api.session_observations_metrics_query(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::SessionObservationsMetricsStream => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<ObservationStreamRequest>(&raw, limits)?;
            let handled = api.session_observations_metrics_stream(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Ndjson(answer))
        }
        RouteId::SessionObservationsSpansListen => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<ObservationListenRequest>(&raw, limits)?;
            let handled = api.session_observations_spans_listen(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Ndjson(answer))
        }
        RouteId::SessionObservationsSpansQuery => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<ObservationQuery>(&raw, limits)?;
            let handled = api.session_observations_spans_query(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::SessionObservationsSpansStream => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<ObservationStreamRequest>(&raw, limits)?;
            let handled = api.session_observations_spans_stream(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Ndjson(answer))
        }
        RouteId::SessionObservationsTelemetryListen => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<ObservationListenRequest>(&raw, limits)?;
            let handled = api.session_observations_telemetry_listen(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Ndjson(answer))
        }
        RouteId::SessionObservationsTelemetryQuery => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<ObservationQuery>(&raw, limits)?;
            let handled = api.session_observations_telemetry_query(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::SessionObservationsTelemetryStream => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<ObservationStreamRequest>(&raw, limits)?;
            let handled = api.session_observations_telemetry_stream(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Ndjson(answer))
        }
        RouteId::SessionObservationsTraceGet => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let trace_id = path_param::<TraceId>(&raw, "traceId")?;
            expect_no_body(&raw)?;
            let handled = api.session_observations_trace_get(cx, session_id, trace_id);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::SessionObservationsTracesListen => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<ObservationListenRequest>(&raw, limits)?;
            let handled = api.session_observations_traces_listen(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Ndjson(answer))
        }
        RouteId::SessionObservationsTracesQuery => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<ObservationQuery>(&raw, limits)?;
            let handled = api.session_observations_traces_query(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::SessionObservationsTracesStream => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<ObservationStreamRequest>(&raw, limits)?;
            let handled = api.session_observations_traces_stream(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Ndjson(answer))
        }
        other => Err(wrong_group(other, "regional:observations")),
    }
}

// --- central:organizations ---------------------------------------------------------------

/// The `organizations` fragment of the central plane: 6 operations.
/// Every method returns a future that is `Send`, so the composition crate can spawn it without
/// wrapping. A method never names a status: the response type it returns is the status the route
/// declares.
pub trait OrganizationsApi: Send + Sync + 'static {
    /// `POST /api/invitations/acceptances`
    /// Redeem every pending invitation addressed to the caller's verified email.
    fn invitation_accept(
        &self,
        cx: &RequestContext,
        body: EmptyRequest,
    ) -> impl Future<Output = WireResult<InvitationAcceptResult>> + Send;

    /// `POST /api/organizations/{organizationId}/invitations`
    /// Invite a person to the organization.
    fn invitation_create(
        &self,
        cx: &RequestContext,
        organization_id: OrganizationId,
        body: InvitationCreateRequest,
    ) -> impl Future<Output = WireResult<Created<Invitation>>> + Send;

    /// `GET /api/organizations/{organizationId}/memberships`
    /// List the memberships of an organization.
    fn memberships_list(
        &self,
        cx: &RequestContext,
        organization_id: OrganizationId,
        query: MembershipsListQuery,
    ) -> impl Future<Output = WireResult<MembershipPage>> + Send;

    /// `POST /api/organizations`
    /// Create an organization whose creator becomes owner.
    fn organization_create(
        &self,
        cx: &RequestContext,
        body: OrganizationCreateRequest,
    ) -> impl Future<Output = WireResult<Created<Organization>>> + Send;

    /// `GET /api/organizations/{organizationId}`
    /// Read one organization.
    fn organization_get(
        &self,
        cx: &RequestContext,
        organization_id: OrganizationId,
    ) -> impl Future<Output = WireResult<Organization>> + Send;

    /// `GET /api/organizations`
    /// List organizations the caller belongs to.
    fn organizations_list(
        &self,
        cx: &RequestContext,
        query: OrganizationsListQuery,
    ) -> impl Future<Output = WireResult<OrganizationPage>> + Send;
}

/// Decodes, calls and encodes one `central:organizations` request.
/// Total over `RouteId`: a route from another group is an internal error naming the mismatch, never
/// a silently wrong handler.
/// # Errors
/// Returns the handler's own declared failure, or a decode failure the route declares. A code the
/// route does not declare is refused at this boundary.
pub async fn dispatch_organizations<A: OrganizationsApi + ?Sized>(
    api: &A,
    cx: &RequestContext,
    raw: RawRequest<'_>,
    limits: RequestLimits,
) -> WireResult<DispatchOutcome<crate::dispatch::NoStream>> {
    let reader = QueryReader::parse(raw.route, raw.query)?;
    match raw.route {
        RouteId::InvitationAccept => {
            let body = decode_body::<EmptyRequest>(&raw, limits)?;
            let handled = api.invitation_accept(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::InvitationCreate => {
            let organization_id = path_param::<OrganizationId>(&raw, "organizationId")?;
            let body = decode_body::<InvitationCreateRequest>(&raw, limits)?;
            let handled = api.invitation_create(cx, organization_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(201, &answer.0)?))
        }
        RouteId::MembershipsList => {
            let organization_id = path_param::<OrganizationId>(&raw, "organizationId")?;
            let query = MembershipsListQuery {
                cursor: reader.optional("cursor")?,
                limit: reader.optional_bounded("limit", 1, 1000)?,
            };
            expect_no_body(&raw)?;
            let handled = api.memberships_list(cx, organization_id, query);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::OrganizationCreate => {
            let body = decode_body::<OrganizationCreateRequest>(&raw, limits)?;
            let handled = api.organization_create(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(201, &answer.0)?))
        }
        RouteId::OrganizationGet => {
            let organization_id = path_param::<OrganizationId>(&raw, "organizationId")?;
            expect_no_body(&raw)?;
            let handled = api.organization_get(cx, organization_id);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::OrganizationsList => {
            let query = OrganizationsListQuery {
                cursor: reader.optional("cursor")?,
                limit: reader.optional_bounded("limit", 1, 1000)?,
            };
            expect_no_body(&raw)?;
            let handled = api.organizations_list(cx, query);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        other => Err(wrong_group(other, "central:organizations")),
    }
}

// --- regional:otlp ---------------------------------------------------------------

/// The `otlp` fragment of the regional plane: 3 operations.
/// Every method returns a future that is `Send`, so the composition crate can spawn it without
/// wrapping. A method never names a status: the response type it returns is the status the route
/// declares.
pub trait OtlpApi: Send + Sync + 'static {
    /// `POST /api/otlp/v1/logs`
    /// Admit an OTLP logs batch.
    fn otlp_logs_ingest(
        &self,
        cx: &RequestContext,
        body: &[u8],
    ) -> impl Future<Output = WireResult<TelemetryAdmissionReceipt>> + Send;

    /// `POST /api/otlp/v1/metrics`
    /// Admit an OTLP metrics batch.
    fn otlp_metrics_ingest(
        &self,
        cx: &RequestContext,
        body: &[u8],
    ) -> impl Future<Output = WireResult<TelemetryAdmissionReceipt>> + Send;

    /// `POST /api/otlp/v1/traces`
    /// Admit an OTLP traces batch.
    fn otlp_traces_ingest(
        &self,
        cx: &RequestContext,
        body: &[u8],
    ) -> impl Future<Output = WireResult<TelemetryAdmissionReceipt>> + Send;
}

/// Decodes, calls and encodes one `regional:otlp` request.
/// Total over `RouteId`: a route from another group is an internal error naming the mismatch, never
/// a silently wrong handler.
/// # Errors
/// Returns the handler's own declared failure, or a decode failure the route declares. A code the
/// route does not declare is refused at this boundary.
pub async fn dispatch_otlp<A: OtlpApi + ?Sized>(
    api: &A,
    cx: &RequestContext,
    raw: RawRequest<'_>,
    limits: RequestLimits,
) -> WireResult<DispatchOutcome<crate::dispatch::NoStream>> {
    let _reader = QueryReader::parse(raw.route, raw.query)?;
    match raw.route {
        RouteId::OtlpLogsIngest => {
            let body = otlp_body(&raw, limits)?;
            let handled = api.otlp_logs_ingest(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::OtlpMetricsIngest => {
            let body = otlp_body(&raw, limits)?;
            let handled = api.otlp_metrics_ingest(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::OtlpTracesIngest => {
            let body = otlp_body(&raw, limits)?;
            let handled = api.otlp_traces_ingest(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        other => Err(wrong_group(other, "regional:otlp")),
    }
}

// --- regional:provider-credentials ---------------------------------------------------------------

/// The `provider-credentials` fragment of the regional plane: 4 operations.
/// Every method returns a future that is `Send`, so the composition crate can spawn it without
/// wrapping. A method never names a status: the response type it returns is the status the route
/// declares.
pub trait ProviderCredentialsApi: Send + Sync + 'static {
    /// `GET /api/workspace/provider-credentials/{providerCredentialId}`
    /// Read one BYOK provider-credential binding.
    fn provider_credential_get(
        &self,
        cx: &RequestContext,
        provider_credential_id: ProviderCredentialId,
    ) -> impl Future<Output = WireResult<WithETag<ProviderCredential>>> + Send;

    /// `POST /api/workspace/provider-credentials`
    /// Register a dedicated BYOK provider credential; carries plaintext once.
    fn provider_credential_register(
        &self,
        cx: &RequestContext,
        body: ProviderCredentialRegisterRequest,
    ) -> impl Future<Output = WireResult<Created<ProviderCredential>>> + Send;

    /// `POST /api/workspace/provider-credentials/{providerCredentialId}/revocations`
    /// Revoke a BYOK provider-credential binding.
    fn provider_credential_revoke(
        &self,
        cx: &RequestContext,
        provider_credential_id: ProviderCredentialId,
        body: EmptyRequest,
    ) -> impl Future<Output = WireResult<ProviderCredential>> + Send;

    /// `GET /api/workspace/provider-credentials`
    /// List BYOK provider-credential binding metadata.
    fn provider_credentials_list(
        &self,
        cx: &RequestContext,
        query: ProviderCredentialsListQuery,
    ) -> impl Future<Output = WireResult<ProviderCredentialPage>> + Send;
}

/// Decodes, calls and encodes one `regional:provider-credentials` request.
/// Total over `RouteId`: a route from another group is an internal error naming the mismatch, never
/// a silently wrong handler.
/// # Errors
/// Returns the handler's own declared failure, or a decode failure the route declares. A code the
/// route does not declare is refused at this boundary.
pub async fn dispatch_provider_credentials<A: ProviderCredentialsApi + ?Sized>(
    api: &A,
    cx: &RequestContext,
    raw: RawRequest<'_>,
    limits: RequestLimits,
) -> WireResult<DispatchOutcome<crate::dispatch::NoStream>> {
    let reader = QueryReader::parse(raw.route, raw.query)?;
    match raw.route {
        RouteId::ProviderCredentialGet => {
            let provider_credential_id =
                path_param::<ProviderCredentialId>(&raw, "providerCredentialId")?;
            expect_no_body(&raw)?;
            let handled = api.provider_credential_get(cx, provider_credential_id);
            let answer = declared(raw.route, handled.await)?;
            let rendered = RawResponse::json(200, &answer.value)?;
            let rendered = rendered.with_etag(answer.etag);
            Ok(DispatchOutcome::Unary(rendered))
        }
        RouteId::ProviderCredentialRegister => {
            let body = decode_body::<ProviderCredentialRegisterRequest>(&raw, limits)?;
            let handled = api.provider_credential_register(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(201, &answer.0)?))
        }
        RouteId::ProviderCredentialRevoke => {
            let provider_credential_id =
                path_param::<ProviderCredentialId>(&raw, "providerCredentialId")?;
            let body = decode_body::<EmptyRequest>(&raw, limits)?;
            let handled = api.provider_credential_revoke(cx, provider_credential_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::ProviderCredentialsList => {
            let query = ProviderCredentialsListQuery {
                cursor: reader.optional("cursor")?,
                limit: reader.optional_bounded("limit", 1, 1000)?,
                provider: reader.optional("provider")?,
            };
            expect_no_body(&raw)?;
            let handled = api.provider_credentials_list(cx, query);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        other => Err(wrong_group(other, "regional:provider-credentials")),
    }
}

// --- regional:operations ---------------------------------------------------------------

/// The `operations` fragment of the regional plane: 3 operations.
/// Every method returns a future that is `Send`, so the composition crate can spawn it without
/// wrapping. A method never names a status: the response type it returns is the status the route
/// declares.
pub trait RegionalOperationsApi: Send + Sync + 'static {
    /// `POST /api/operations/{operationId}/cancellations`
    /// Request cancellation of a regional operation.
    fn regional_operation_cancel(
        &self,
        cx: &RequestContext,
        operation_id: OperationId,
        body: EmptyRequest,
    ) -> impl Future<Output = WireResult<Operation>> + Send;

    /// `GET /api/operations/{operationId}`
    /// Read one regional durable operation.
    fn regional_operation_get(
        &self,
        cx: &RequestContext,
        operation_id: OperationId,
    ) -> impl Future<Output = WireResult<Operation>> + Send;

    /// `GET /api/operations`
    /// List regional durable operations.
    fn regional_operations_list(
        &self,
        cx: &RequestContext,
        query: RegionalOperationsListQuery,
    ) -> impl Future<Output = WireResult<OperationPage>> + Send;
}

/// Decodes, calls and encodes one `regional:operations` request.
/// Total over `RouteId`: a route from another group is an internal error naming the mismatch, never
/// a silently wrong handler.
/// # Errors
/// Returns the handler's own declared failure, or a decode failure the route declares. A code the
/// route does not declare is refused at this boundary.
pub async fn dispatch_regional_operations<A: RegionalOperationsApi + ?Sized>(
    api: &A,
    cx: &RequestContext,
    raw: RawRequest<'_>,
    limits: RequestLimits,
) -> WireResult<DispatchOutcome<crate::dispatch::NoStream>> {
    let reader = QueryReader::parse(raw.route, raw.query)?;
    match raw.route {
        RouteId::RegionalOperationCancel => {
            let operation_id = path_param::<OperationId>(&raw, "operationId")?;
            let body = decode_body::<EmptyRequest>(&raw, limits)?;
            let handled = api.regional_operation_cancel(cx, operation_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::RegionalOperationGet => {
            let operation_id = path_param::<OperationId>(&raw, "operationId")?;
            expect_no_body(&raw)?;
            let handled = api.regional_operation_get(cx, operation_id);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::RegionalOperationsList => {
            let query = RegionalOperationsListQuery {
                cursor: reader.optional("cursor")?,
                kind: reader.optional("kind")?,
                limit: reader.optional_bounded("limit", 1, 1000)?,
                session_id: reader.optional("sessionId")?,
                status: reader.optional("status")?,
            };
            expect_no_body(&raw)?;
            let handled = api.regional_operations_list(cx, query);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        other => Err(wrong_group(other, "regional:operations")),
    }
}

// --- regional:registry ---------------------------------------------------------------

/// The `registry` fragment of the regional plane: 5 operations.
/// Every method returns a future that is `Send`, so the composition crate can spawn it without
/// wrapping. A method never names a status: the response type it returns is the status the route
/// declares.
pub trait RegistryApi: Send + Sync + 'static {
    /// `DELETE /api/workspace/files/{name}`
    /// Delete one registered workspace file.
    fn registry_files_delete(
        &self,
        cx: &RequestContext,
        name: ResourceName,
    ) -> impl Future<Output = WireResult<NoContent>> + Send;

    /// `POST /api/workspace/files/{name}/downloads`
    /// Mint a download grant for a registered workspace file.
    fn registry_files_download_create(
        &self,
        cx: &RequestContext,
        name: ResourceName,
        body: RegistryDownloadRequest,
    ) -> impl Future<Output = WireResult<Created<DownloadGrant>>> + Send;

    /// `GET /api/workspace/files/{name}`
    /// Read one registered workspace file.
    fn registry_files_get(
        &self,
        cx: &RequestContext,
        name: ResourceName,
    ) -> impl Future<Output = WireResult<WithETag<RegisteredFile>>> + Send;

    /// `GET /api/workspace/files`
    /// List registered workspace files.
    fn registry_files_list(
        &self,
        cx: &RequestContext,
        query: RegistryFilesListQuery,
    ) -> impl Future<Output = WireResult<RegisteredFilePage>> + Send;

    /// `PUT /api/workspace/files/{name}`
    /// Replace one registered workspace file.
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

/// The `sessions` fragment of the regional plane: 10 operations.
/// Every method returns a future that is `Send`, so the composition crate can spawn it without
/// wrapping. A method never names a status: the response type it returns is the status the route
/// declares.
pub trait SessionsApi: Send + Sync + 'static {
    /// `POST /api/sessions/{sessionId}/cancellations`
    /// Cancel current work and return the session to idle.
    fn session_cancel(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: EmptyRequest,
    ) -> impl Future<Output = WireResult<Accepted>> + Send;

    /// `POST /api/sessions`
    /// Create an eight-hour multi-turn session.
    fn session_create(
        &self,
        cx: &RequestContext,
        body: SessionCreateRequest,
    ) -> impl Future<Output = WireResult<Created<Session>>> + Send;

    /// `POST /api/sessions/{sessionId}/deletions`
    /// Irreversibly delete the session and session-scoped user content, observations, telemetry,
    /// and export objects. Independent registered workspace files remain.
    fn session_delete(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: EmptyRequest,
    ) -> impl Future<Output = WireResult<Accepted>> + Send;

    /// `GET /api/sessions/{sessionId}`
    /// Read session metadata, including automatic lifecycle state, or its minimal deletion
    /// tombstone.
    fn session_get(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
    ) -> impl Future<Output = WireResult<WithETag<Session>>> + Send;

    /// `POST /api/sessions/{sessionId}/messages`
    /// Send text and start the session's next work, automatically resuming the same suspended
    /// generation.
    fn session_message_send(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: MessageSendRequest,
    ) -> impl Future<Output = WireResult<Created<MessageSendResult>>> + Send;

    /// `GET /api/sessions/{sessionId}/messages`
    /// List complete sealed messages in immutable seal-visibility order.
    fn session_messages_list(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        query: SessionMessagesListQuery,
    ) -> impl Future<Output = WireResult<MessagePage>> + Send;

    /// `POST /api/sessions/{sessionId}/resumptions`
    /// Resume the same retained generation to idle.
    fn session_resume(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: EmptyRequest,
    ) -> impl Future<Output = WireResult<Accepted>> + Send;

    /// `POST /api/sessions/{sessionId}/suspensions`
    /// Suspend an idle session while retaining its exact generation; cancel active work first.
    fn session_suspend(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: EmptyRequest,
    ) -> impl Future<Output = WireResult<Accepted>> + Send;

    /// `POST /api/sessions/{sessionId}/terminations`
    /// Permanently destroy compute and live files while retaining metadata and sealed messages.
    fn session_terminate(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: EmptyRequest,
    ) -> impl Future<Output = WireResult<Accepted>> + Send;

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
) -> WireResult<DispatchOutcome<crate::dispatch::NoStream>> {
    let reader = QueryReader::parse(raw.route, raw.query)?;
    match raw.route {
        RouteId::SessionCancel => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<EmptyRequest>(&raw, limits)?;
            let handled = api.session_cancel(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::accepted(&answer)?))
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
            Ok(DispatchOutcome::Unary(RawResponse::accepted(&answer)?))
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
        RouteId::SessionResume => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<EmptyRequest>(&raw, limits)?;
            let handled = api.session_resume(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::accepted(&answer)?))
        }
        RouteId::SessionSuspend => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<EmptyRequest>(&raw, limits)?;
            let handled = api.session_suspend(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::accepted(&answer)?))
        }
        RouteId::SessionTerminate => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<EmptyRequest>(&raw, limits)?;
            let handled = api.session_terminate(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::accepted(&answer)?))
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

// --- regional:telemetry-lifecycle ---------------------------------------------------------------

/// The `telemetry-lifecycle` fragment of the regional plane: 12 operations.
/// Every method returns a future that is `Send`, so the composition crate can spawn it without
/// wrapping. A method never names a status: the response type it returns is the status the route
/// declares.
pub trait TelemetryLifecycleApi: Send + Sync + 'static {
    /// `POST /api/observations/{sessionId}/telemetry/exports`
    /// Admit the durable session telemetry-export operation.
    fn session_telemetry_export_create(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: TelemetryExportRequest,
    ) -> impl Future<Output = WireResult<Accepted>> + Send;

    /// `POST /api/observations/{sessionId}/telemetry/exports/{exportId}/downloads`
    /// Mint a download grant for a ready session telemetry export.
    fn session_telemetry_export_download_create(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        export_id: ExportId,
        body: EmptyRequest,
    ) -> impl Future<Output = WireResult<Created<DownloadGrant>>> + Send;

    /// `GET /api/observations/{sessionId}/telemetry/exports/{exportId}`
    /// Read one session telemetry export record.
    fn session_telemetry_export_get(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        export_id: ExportId,
    ) -> impl Future<Output = WireResult<TelemetryExport>> + Send;

    /// `POST /api/observations/{sessionId}/telemetry/exports/{exportId}/revocations`
    /// Revoke a session telemetry export and its outstanding grants.
    fn session_telemetry_export_revoke(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        export_id: ExportId,
        body: EmptyRequest,
    ) -> impl Future<Output = WireResult<TelemetryExport>> + Send;

    /// `GET /api/observations/{sessionId}/telemetry/gaps/{gapId}`
    /// Read one recorded session telemetry gap.
    fn session_telemetry_gap_get(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        gap_id: TelemetryGapId,
    ) -> impl Future<Output = WireResult<TelemetryGap>> + Send;

    /// `POST /api/observations/{sessionId}/telemetry/gaps/query`
    /// Query recorded session telemetry gaps.
    fn session_telemetry_gaps_query(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: TelemetryGapQuery,
    ) -> impl Future<Output = WireResult<TelemetryGapPage>> + Send;

    /// `POST /api/observations/telemetry/exports`
    /// Admit the durable workspace telemetry-export operation.
    fn telemetry_export_create(
        &self,
        cx: &RequestContext,
        body: TelemetryExportRequest,
    ) -> impl Future<Output = WireResult<Accepted>> + Send;

    /// `POST /api/observations/telemetry/exports/{exportId}/downloads`
    /// Mint a download grant for a ready telemetry export.
    fn telemetry_export_download_create(
        &self,
        cx: &RequestContext,
        export_id: ExportId,
        body: EmptyRequest,
    ) -> impl Future<Output = WireResult<Created<DownloadGrant>>> + Send;

    /// `GET /api/observations/telemetry/exports/{exportId}`
    /// Read one telemetry export record.
    fn telemetry_export_get(
        &self,
        cx: &RequestContext,
        export_id: ExportId,
    ) -> impl Future<Output = WireResult<TelemetryExport>> + Send;

    /// `POST /api/observations/telemetry/exports/{exportId}/revocations`
    /// Revoke a telemetry export and its outstanding grants.
    fn telemetry_export_revoke(
        &self,
        cx: &RequestContext,
        export_id: ExportId,
        body: EmptyRequest,
    ) -> impl Future<Output = WireResult<TelemetryExport>> + Send;

    /// `GET /api/observations/telemetry/gaps/{gapId}`
    /// Read one recorded telemetry gap.
    fn telemetry_gap_get(
        &self,
        cx: &RequestContext,
        gap_id: TelemetryGapId,
    ) -> impl Future<Output = WireResult<TelemetryGap>> + Send;

    /// `POST /api/observations/telemetry/gaps/query`
    /// Query recorded workspace telemetry gaps.
    fn telemetry_gaps_query(
        &self,
        cx: &RequestContext,
        body: TelemetryGapQuery,
    ) -> impl Future<Output = WireResult<TelemetryGapPage>> + Send;
}

/// Decodes, calls and encodes one `regional:telemetry-lifecycle` request.
/// Total over `RouteId`: a route from another group is an internal error naming the mismatch, never
/// a silently wrong handler.
/// # Errors
/// Returns the handler's own declared failure, or a decode failure the route declares. A code the
/// route does not declare is refused at this boundary.
pub async fn dispatch_telemetry_lifecycle<A: TelemetryLifecycleApi + ?Sized>(
    api: &A,
    cx: &RequestContext,
    raw: RawRequest<'_>,
    limits: RequestLimits,
) -> WireResult<DispatchOutcome<crate::dispatch::NoStream>> {
    let _reader = QueryReader::parse(raw.route, raw.query)?;
    match raw.route {
        RouteId::SessionTelemetryExportCreate => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<TelemetryExportRequest>(&raw, limits)?;
            let handled = api.session_telemetry_export_create(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::accepted(&answer)?))
        }
        RouteId::SessionTelemetryExportDownloadCreate => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let export_id = path_param::<ExportId>(&raw, "exportId")?;
            let body = decode_body::<EmptyRequest>(&raw, limits)?;
            let handled =
                api.session_telemetry_export_download_create(cx, session_id, export_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(201, &answer.0)?))
        }
        RouteId::SessionTelemetryExportGet => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let export_id = path_param::<ExportId>(&raw, "exportId")?;
            expect_no_body(&raw)?;
            let handled = api.session_telemetry_export_get(cx, session_id, export_id);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::SessionTelemetryExportRevoke => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let export_id = path_param::<ExportId>(&raw, "exportId")?;
            let body = decode_body::<EmptyRequest>(&raw, limits)?;
            let handled = api.session_telemetry_export_revoke(cx, session_id, export_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::SessionTelemetryGapGet => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let gap_id = path_param::<TelemetryGapId>(&raw, "gapId")?;
            expect_no_body(&raw)?;
            let handled = api.session_telemetry_gap_get(cx, session_id, gap_id);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::SessionTelemetryGapsQuery => {
            let session_id = path_param::<SessionId>(&raw, "sessionId")?;
            let body = decode_body::<TelemetryGapQuery>(&raw, limits)?;
            let handled = api.session_telemetry_gaps_query(cx, session_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::TelemetryExportCreate => {
            let body = decode_body::<TelemetryExportRequest>(&raw, limits)?;
            let handled = api.telemetry_export_create(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::accepted(&answer)?))
        }
        RouteId::TelemetryExportDownloadCreate => {
            let export_id = path_param::<ExportId>(&raw, "exportId")?;
            let body = decode_body::<EmptyRequest>(&raw, limits)?;
            let handled = api.telemetry_export_download_create(cx, export_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(201, &answer.0)?))
        }
        RouteId::TelemetryExportGet => {
            let export_id = path_param::<ExportId>(&raw, "exportId")?;
            expect_no_body(&raw)?;
            let handled = api.telemetry_export_get(cx, export_id);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::TelemetryExportRevoke => {
            let export_id = path_param::<ExportId>(&raw, "exportId")?;
            let body = decode_body::<EmptyRequest>(&raw, limits)?;
            let handled = api.telemetry_export_revoke(cx, export_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::TelemetryGapGet => {
            let gap_id = path_param::<TelemetryGapId>(&raw, "gapId")?;
            expect_no_body(&raw)?;
            let handled = api.telemetry_gap_get(cx, gap_id);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::TelemetryGapsQuery => {
            let body = decode_body::<TelemetryGapQuery>(&raw, limits)?;
            let handled = api.telemetry_gaps_query(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        other => Err(wrong_group(other, "regional:telemetry-lifecycle")),
    }
}

// --- regional:uploads ---------------------------------------------------------------

/// The `uploads` fragment of the regional plane: 4 operations.
/// Every method returns a future that is `Send`, so the composition crate can spawn it without
/// wrapping. A method never names a status: the response type it returns is the status the route
/// declares.
pub trait UploadsApi: Send + Sync + 'static {
    /// `DELETE /api/workspace/uploads/{uploadId}`
    /// Abort a staged upload.
    fn upload_abort(
        &self,
        cx: &RequestContext,
        upload_id: UploadId,
    ) -> impl Future<Output = WireResult<NoContent>> + Send;

    /// `POST /api/workspace/uploads/{uploadId}/completion`
    /// Complete a staged upload.
    fn upload_complete(
        &self,
        cx: &RequestContext,
        upload_id: UploadId,
        body: UploadCompleteRequest,
    ) -> impl Future<Output = WireResult<Upload>> + Send;

    /// `POST /api/workspace/uploads`
    /// Stage a large registered-resource value.
    fn upload_create(
        &self,
        cx: &RequestContext,
        body: UploadCreateRequest,
    ) -> impl Future<Output = WireResult<Created<Upload>>> + Send;

    /// `POST /api/workspace/uploads/{uploadId}/parts`
    /// Mint presigned PUT grants for the named parts.
    fn upload_parts_grant(
        &self,
        cx: &RequestContext,
        upload_id: UploadId,
        body: UploadPartsRequest,
    ) -> impl Future<Output = WireResult<UploadPartGrants>> + Send;
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
        RouteId::UploadAbort => {
            let upload_id = path_param::<UploadId>(&raw, "uploadId")?;
            expect_no_body(&raw)?;
            let handled = api.upload_abort(cx, upload_id);
            let NoContent = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::no_content()))
        }
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
        RouteId::UploadPartsGrant => {
            let upload_id = path_param::<UploadId>(&raw, "uploadId")?;
            let body = decode_body::<UploadPartsRequest>(&raw, limits)?;
            let handled = api.upload_parts_grant(cx, upload_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        other => Err(wrong_group(other, "regional:uploads")),
    }
}

// --- regional:usage ---------------------------------------------------------------

/// The `usage` fragment of the regional plane: 1 operation.
/// Every method returns a future that is `Send`, so the composition crate can spawn it without
/// wrapping. A method never names a status: the response type it returns is the status the route
/// declares.
pub trait UsageApi: Send + Sync + 'static {
    /// `POST /api/billing/usage/query`
    /// Query rated usage for this workspace.
    fn usage_query(
        &self,
        cx: &RequestContext,
        query: UsageQueryQuery,
        body: UsageQuery,
    ) -> impl Future<Output = WireResult<UsagePage>> + Send;
}

/// Decodes, calls and encodes one `regional:usage` request.
/// Total over `RouteId`: a route from another group is an internal error naming the mismatch, never
/// a silently wrong handler.
/// # Errors
/// Returns the handler's own declared failure, or a decode failure the route declares. A code the
/// route does not declare is refused at this boundary.
pub async fn dispatch_usage<A: UsageApi + ?Sized>(
    api: &A,
    cx: &RequestContext,
    raw: RawRequest<'_>,
    limits: RequestLimits,
) -> WireResult<DispatchOutcome<crate::dispatch::NoStream>> {
    let reader = QueryReader::parse(raw.route, raw.query)?;
    match raw.route {
        RouteId::UsageQuery => {
            let query = UsageQueryQuery {
                workspace_id: reader.optional("workspaceId")?,
            };
            let body = decode_body::<UsageQuery>(&raw, limits)?;
            let handled = api.usage_query(cx, query, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        other => Err(wrong_group(other, "regional:usage")),
    }
}

// --- regional:workspace ---------------------------------------------------------------

/// The `workspace` fragment of the regional plane: 3 operations.
/// Every method returns a future that is `Send`, so the composition crate can spawn it without
/// wrapping. A method never names a status: the response type it returns is the status the route
/// declares.
pub trait WorkspaceApi: Send + Sync + 'static {
    /// `GET /api/workspace`
    /// Read the workspace this credential is pinned to.
    fn workspace_current_get(
        &self,
        cx: &RequestContext,
    ) -> impl Future<Output = WireResult<Workspace>> + Send;

    /// `GET /api/workspace/limits/{limitId}`
    /// Read one effective workspace safety limit.
    fn workspace_limit_get(
        &self,
        cx: &RequestContext,
        limit_id: LimitId,
    ) -> impl Future<Output = WireResult<EffectiveWorkspaceLimit>> + Send;

    /// `GET /api/workspace/limits`
    /// List the effective workspace safety limits. The registry is closed and complete, so the
    /// whole set is one page and no continuation is ever minted.
    fn workspace_limits_list(
        &self,
        cx: &RequestContext,
    ) -> impl Future<Output = WireResult<EffectiveWorkspaceLimitPage>> + Send;
}

/// Decodes, calls and encodes one `regional:workspace` request.
/// Total over `RouteId`: a route from another group is an internal error naming the mismatch, never
/// a silently wrong handler.
/// # Errors
/// Returns the handler's own declared failure, or a decode failure the route declares. A code the
/// route does not declare is refused at this boundary.
pub async fn dispatch_workspace<A: WorkspaceApi + ?Sized>(
    api: &A,
    cx: &RequestContext,
    raw: RawRequest<'_>,
    _limits: RequestLimits,
) -> WireResult<DispatchOutcome<crate::dispatch::NoStream>> {
    let _reader = QueryReader::parse(raw.route, raw.query)?;
    match raw.route {
        RouteId::WorkspaceCurrentGet => {
            expect_no_body(&raw)?;
            let handled = api.workspace_current_get(cx);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::WorkspaceLimitGet => {
            let limit_id = path_param_registry::<LimitId>(&raw, "limitId")?;
            expect_no_body(&raw)?;
            let handled = api.workspace_limit_get(cx, limit_id);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::WorkspaceLimitsList => {
            expect_no_body(&raw)?;
            let handled = api.workspace_limits_list(cx);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        other => Err(wrong_group(other, "regional:workspace")),
    }
}

// --- central:workspaces ---------------------------------------------------------------

/// The `workspaces` fragment of the central plane: 4 operations.
/// Every method returns a future that is `Send`, so the composition crate can spawn it without
/// wrapping. A method never names a status: the response type it returns is the status the route
/// declares.
pub trait WorkspacesApi: Send + Sync + 'static {
    /// `POST /api/workspaces`
    /// Create a region-pinned workspace.
    fn workspace_create(
        &self,
        cx: &RequestContext,
        body: WorkspaceCreateRequest,
    ) -> impl Future<Output = WireResult<Created<Workspace>>> + Send;

    /// `POST /api/workspaces/{workspaceId}/deletions`
    /// Admit the global workspace-deletion operation.
    fn workspace_delete(
        &self,
        cx: &RequestContext,
        workspace_id: WorkspaceId,
        body: WorkspaceDeleteRequest,
    ) -> impl Future<Output = WireResult<Accepted>> + Send;

    /// `GET /api/workspaces/{workspaceId}`
    /// Read one workspace.
    fn workspace_get(
        &self,
        cx: &RequestContext,
        workspace_id: WorkspaceId,
    ) -> impl Future<Output = WireResult<Workspace>> + Send;

    /// `GET /api/workspaces`
    /// List workspaces the caller can reach.
    fn workspaces_list(
        &self,
        cx: &RequestContext,
        query: WorkspacesListQuery,
    ) -> impl Future<Output = WireResult<WorkspacePage>> + Send;
}

/// Decodes, calls and encodes one `central:workspaces` request.
/// Total over `RouteId`: a route from another group is an internal error naming the mismatch, never
/// a silently wrong handler.
/// # Errors
/// Returns the handler's own declared failure, or a decode failure the route declares. A code the
/// route does not declare is refused at this boundary.
pub async fn dispatch_workspaces<A: WorkspacesApi + ?Sized>(
    api: &A,
    cx: &RequestContext,
    raw: RawRequest<'_>,
    limits: RequestLimits,
) -> WireResult<DispatchOutcome<crate::dispatch::NoStream>> {
    let reader = QueryReader::parse(raw.route, raw.query)?;
    match raw.route {
        RouteId::WorkspaceCreate => {
            let body = decode_body::<WorkspaceCreateRequest>(&raw, limits)?;
            let handled = api.workspace_create(cx, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(201, &answer.0)?))
        }
        RouteId::WorkspaceDelete => {
            let workspace_id = path_param::<WorkspaceId>(&raw, "workspaceId")?;
            let body = decode_body::<WorkspaceDeleteRequest>(&raw, limits)?;
            let handled = api.workspace_delete(cx, workspace_id, body);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::accepted(&answer)?))
        }
        RouteId::WorkspaceGet => {
            let workspace_id = path_param::<WorkspaceId>(&raw, "workspaceId")?;
            expect_no_body(&raw)?;
            let handled = api.workspace_get(cx, workspace_id);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        RouteId::WorkspacesList => {
            let query = WorkspacesListQuery {
                cursor: reader.optional("cursor")?,
                limit: reader.optional_bounded("limit", 1, 1000)?,
                organization_id: reader.optional("organizationId")?,
            };
            expect_no_body(&raw)?;
            let handled = api.workspaces_list(cx, query);
            let answer = declared(raw.route, handled.await)?;
            Ok(DispatchOutcome::Unary(RawResponse::json(200, &answer)?))
        }
        other => Err(wrong_group(other, "central:workspaces")),
    }
}
