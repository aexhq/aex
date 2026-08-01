//! GENERATED — DO NOT EDIT.
//!
//! The closed v1 public error vocabulary.
//!
//! Produced by `aex-contract-gen` from `api/`; contract digest
//! `sha256:fec7f531dec7025944bcb6ef8da610d99ce5a3b4efcbcb27ecc1d467e730c37d`.
//! Regenerate with `cargo run -p aex-contract-gen -- build`.

#![allow(clippy::large_enum_variant, reason = "a wire union is never boxed")]
#![allow(clippy::match_same_arms, reason = "one arm per row")]
#![allow(clippy::too_many_lines, reason = "one arm per row")]

use serde::{Deserialize, Serialize};

use crate::error::{ErrorClass, PrecedenceStage};

/// Every error the platform emits. Adding a variant is a compile error at every exhaustive match,
/// which is the point.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// `unauthenticated` — no valid credential was presented
    Unauthenticated,
    /// `forbidden` — the principal may not act on this resource
    Forbidden,
    /// `insufficient_scope` — the credential lacks the required scope
    InsufficientScope,
    /// `token_invalid` — the credential did not verify
    TokenInvalid,
    /// `token_revoked` — the credential has been revoked
    TokenRevoked,
    /// `token_expired` — the credential has expired
    TokenExpired,
    /// `malformed_token` — the credential is not a well-formed AEX credential
    MalformedToken,
    /// `authorization_pending` — the device authorization has not been approved yet
    AuthorizationPending,
    /// `slow_down` — poll less often
    SlowDown,
    /// `not_found` — no such resource
    NotFound,
    /// `gone` — the resource was deleted
    Gone,
    /// `idempotency_conflict` — the idempotency key was reused with a different intent
    IdempotencyConflict,
    /// `operation_idempotency_conflict` — the operation id was reused with a different intent
    OperationIdempotencyConflict,
    /// `invalid_request` — the request body failed strict validation
    InvalidRequest,
    /// `invalid_cursor` — the cursor is malformed, expired, or bound to a different query
    InvalidCursor,
    /// `invalid_file_selection` — the include or exclude selection is not a valid file selection
    InvalidFileSelection,
    /// `invalid_range` — the byte range is absent, empty, or larger than one grant may sign
    InvalidRange,
    /// `payload_too_large` — the encoded body exceeded the effective limit
    PayloadTooLarge,
    /// `limit_exceeded` — an effective workspace limit was exceeded
    LimitExceeded,
    /// `session_not_idle` — the session must be idle for this operation
    SessionNotIdle,
    /// `workspace_activation_required` — the regional workspace is not activated yet
    WorkspaceActivationRequired,
    /// `workspace_not_live` — the workspace is not live
    WorkspaceNotLive,
    /// `session_deleting` — the session is being deleted
    SessionDeleting,
    /// `session_deleted` — the session was deleted
    SessionDeleted,
    /// `deletion_in_progress` — a deletion of this resource is already running
    DeletionInProgress,
    /// `operation_not_cancelable` — the operation has passed its cancellation point
    OperationNotCancelable,
    /// `approval_not_found` — no such approval
    ApprovalNotFound,
    /// `approval_already_resolved` — the approval already has a decision
    ApprovalAlreadyResolved,
    /// `file_not_found` — no such file entry
    FileNotFound,
    /// `content_missing` — the referenced content is not present
    ContentMissing,
    /// `export_not_found` — no such export
    ExportNotFound,
    /// `export_not_ready` — the export is still preparing
    ExportNotReady,
    /// `export_expired` — the export expired
    ExportExpired,
    /// `export_revoked` — the export was revoked
    ExportRevoked,
    /// `download_grant_expired` — the download grant expired
    DownloadGrantExpired,
    /// `telemetry_payload_too_large` — the OTLP payload exceeded its effective limit
    TelemetryPayloadTooLarge,
    /// `invalid_telemetry` — the telemetry payload is not the pinned OTLP revision
    InvalidTelemetry,
    /// `telemetry_quota_exceeded` — the telemetry admission quota was exceeded
    TelemetryQuotaExceeded,
    /// `telemetry_incomplete` — the requested range has recorded gaps and completeness was required
    TelemetryIncomplete,
    /// `unsupported_export_signal` — that signal cannot be exported in the requested format
    UnsupportedExportSignal,
    /// `invalid_query` — the observation query exceeded its structural bounds
    InvalidQuery,
    /// `invalid_metric_aggregation` — the metric aggregation is not computable over the selection
    InvalidMetricAggregation,
    /// `invalid_network_policy` — the requested network policy is not permitted
    InvalidNetworkPolicy,
    /// `unsupported_package_ecosystem` — that package ecosystem is not supported
    UnsupportedPackageEcosystem,
    /// `package_resolution_failed` — a requested package could not be resolved
    PackageResolutionFailed,
    /// `package_artifact_unavailable` — a resolved package artifact could not be fetched
    PackageArtifactUnavailable,
    /// `package_integrity_mismatch` — a package artifact failed its integrity check
    PackageIntegrityMismatch,
    /// `unknown_provider` — no such provider
    UnknownProvider,
    /// `unknown_model` — no such model for that provider
    UnknownModel,
    /// `unqualified_provider_model` — the provider and model pair has no live conformance receipt
    UnqualifiedProviderModel,
    /// `provider_credential_not_found` — no such provider credential binding
    ProviderCredentialNotFound,
    /// `provider_credential_revoked` — the provider credential binding was revoked
    ProviderCredentialRevoked,
    /// `invalid_auto_topup_policy` — the auto top-up policy is not internally consistent
    InvalidAutoTopupPolicy,
    /// `payment_method_required` — the organization has no usable saved payment method
    PaymentMethodRequired,
    /// `authentication_unavailable` — the authorization authority could not be reached
    AuthenticationUnavailable,
    /// `account_paused` — the account is paused and this route is not pause-exempt
    AccountPaused,
    /// `account_state_unavailable` — the account state could not be established
    AccountStateUnavailable,
    /// `precondition_failed` — the `If-Match` or generation precondition did not hold
    PreconditionFailed,
    /// `wrong_workspace_region` — the workspace is placed in a different region
    WrongWorkspaceRegion,
    /// `observability_unavailable` — the observation authority could not be reached
    ObservabilityUnavailable,
    /// `rate_limited` — too many requests
    RateLimited,
    /// `upstream_error` — a dependency failed
    UpstreamError,
    /// `internal_error` — an unexpected condition occurred
    InternalError,
}

impl ErrorCode {
    /// Every code, in registry order.
    pub const ALL: &'static [ErrorCode] = &[
        ErrorCode::Unauthenticated,
        ErrorCode::Forbidden,
        ErrorCode::InsufficientScope,
        ErrorCode::TokenInvalid,
        ErrorCode::TokenRevoked,
        ErrorCode::TokenExpired,
        ErrorCode::MalformedToken,
        ErrorCode::AuthorizationPending,
        ErrorCode::SlowDown,
        ErrorCode::NotFound,
        ErrorCode::Gone,
        ErrorCode::IdempotencyConflict,
        ErrorCode::OperationIdempotencyConflict,
        ErrorCode::InvalidRequest,
        ErrorCode::InvalidCursor,
        ErrorCode::InvalidFileSelection,
        ErrorCode::InvalidRange,
        ErrorCode::PayloadTooLarge,
        ErrorCode::LimitExceeded,
        ErrorCode::SessionNotIdle,
        ErrorCode::WorkspaceActivationRequired,
        ErrorCode::WorkspaceNotLive,
        ErrorCode::SessionDeleting,
        ErrorCode::SessionDeleted,
        ErrorCode::DeletionInProgress,
        ErrorCode::OperationNotCancelable,
        ErrorCode::ApprovalNotFound,
        ErrorCode::ApprovalAlreadyResolved,
        ErrorCode::FileNotFound,
        ErrorCode::ContentMissing,
        ErrorCode::ExportNotFound,
        ErrorCode::ExportNotReady,
        ErrorCode::ExportExpired,
        ErrorCode::ExportRevoked,
        ErrorCode::DownloadGrantExpired,
        ErrorCode::TelemetryPayloadTooLarge,
        ErrorCode::InvalidTelemetry,
        ErrorCode::TelemetryQuotaExceeded,
        ErrorCode::TelemetryIncomplete,
        ErrorCode::UnsupportedExportSignal,
        ErrorCode::InvalidQuery,
        ErrorCode::InvalidMetricAggregation,
        ErrorCode::InvalidNetworkPolicy,
        ErrorCode::UnsupportedPackageEcosystem,
        ErrorCode::PackageResolutionFailed,
        ErrorCode::PackageArtifactUnavailable,
        ErrorCode::PackageIntegrityMismatch,
        ErrorCode::UnknownProvider,
        ErrorCode::UnknownModel,
        ErrorCode::UnqualifiedProviderModel,
        ErrorCode::ProviderCredentialNotFound,
        ErrorCode::ProviderCredentialRevoked,
        ErrorCode::InvalidAutoTopupPolicy,
        ErrorCode::PaymentMethodRequired,
        ErrorCode::AuthenticationUnavailable,
        ErrorCode::AccountPaused,
        ErrorCode::AccountStateUnavailable,
        ErrorCode::PreconditionFailed,
        ErrorCode::WrongWorkspaceRegion,
        ErrorCode::ObservabilityUnavailable,
        ErrorCode::RateLimited,
        ErrorCode::UpstreamError,
        ErrorCode::InternalError,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unauthenticated => "unauthenticated",
            Self::Forbidden => "forbidden",
            Self::InsufficientScope => "insufficient_scope",
            Self::TokenInvalid => "token_invalid",
            Self::TokenRevoked => "token_revoked",
            Self::TokenExpired => "token_expired",
            Self::MalformedToken => "malformed_token",
            Self::AuthorizationPending => "authorization_pending",
            Self::SlowDown => "slow_down",
            Self::NotFound => "not_found",
            Self::Gone => "gone",
            Self::IdempotencyConflict => "idempotency_conflict",
            Self::OperationIdempotencyConflict => "operation_idempotency_conflict",
            Self::InvalidRequest => "invalid_request",
            Self::InvalidCursor => "invalid_cursor",
            Self::InvalidFileSelection => "invalid_file_selection",
            Self::InvalidRange => "invalid_range",
            Self::PayloadTooLarge => "payload_too_large",
            Self::LimitExceeded => "limit_exceeded",
            Self::SessionNotIdle => "session_not_idle",
            Self::WorkspaceActivationRequired => "workspace_activation_required",
            Self::WorkspaceNotLive => "workspace_not_live",
            Self::SessionDeleting => "session_deleting",
            Self::SessionDeleted => "session_deleted",
            Self::DeletionInProgress => "deletion_in_progress",
            Self::OperationNotCancelable => "operation_not_cancelable",
            Self::ApprovalNotFound => "approval_not_found",
            Self::ApprovalAlreadyResolved => "approval_already_resolved",
            Self::FileNotFound => "file_not_found",
            Self::ContentMissing => "content_missing",
            Self::ExportNotFound => "export_not_found",
            Self::ExportNotReady => "export_not_ready",
            Self::ExportExpired => "export_expired",
            Self::ExportRevoked => "export_revoked",
            Self::DownloadGrantExpired => "download_grant_expired",
            Self::TelemetryPayloadTooLarge => "telemetry_payload_too_large",
            Self::InvalidTelemetry => "invalid_telemetry",
            Self::TelemetryQuotaExceeded => "telemetry_quota_exceeded",
            Self::TelemetryIncomplete => "telemetry_incomplete",
            Self::UnsupportedExportSignal => "unsupported_export_signal",
            Self::InvalidQuery => "invalid_query",
            Self::InvalidMetricAggregation => "invalid_metric_aggregation",
            Self::InvalidNetworkPolicy => "invalid_network_policy",
            Self::UnsupportedPackageEcosystem => "unsupported_package_ecosystem",
            Self::PackageResolutionFailed => "package_resolution_failed",
            Self::PackageArtifactUnavailable => "package_artifact_unavailable",
            Self::PackageIntegrityMismatch => "package_integrity_mismatch",
            Self::UnknownProvider => "unknown_provider",
            Self::UnknownModel => "unknown_model",
            Self::UnqualifiedProviderModel => "unqualified_provider_model",
            Self::ProviderCredentialNotFound => "provider_credential_not_found",
            Self::ProviderCredentialRevoked => "provider_credential_revoked",
            Self::InvalidAutoTopupPolicy => "invalid_auto_topup_policy",
            Self::PaymentMethodRequired => "payment_method_required",
            Self::AuthenticationUnavailable => "authentication_unavailable",
            Self::AccountPaused => "account_paused",
            Self::AccountStateUnavailable => "account_state_unavailable",
            Self::PreconditionFailed => "precondition_failed",
            Self::WrongWorkspaceRegion => "wrong_workspace_region",
            Self::ObservabilityUnavailable => "observability_unavailable",
            Self::RateLimited => "rate_limited",
            Self::UpstreamError => "upstream_error",
            Self::InternalError => "internal_error",
        }
    }

    /// The HTTP status this code renders at.
    #[must_use]
    pub const fn http_status(self) -> u16 {
        match self {
            Self::Unauthenticated => 401,
            Self::Forbidden => 403,
            Self::InsufficientScope => 403,
            Self::TokenInvalid => 401,
            Self::TokenRevoked => 401,
            Self::TokenExpired => 401,
            Self::MalformedToken => 401,
            Self::AuthorizationPending => 400,
            Self::SlowDown => 429,
            Self::NotFound => 404,
            Self::Gone => 410,
            Self::IdempotencyConflict => 409,
            Self::OperationIdempotencyConflict => 409,
            Self::InvalidRequest => 400,
            Self::InvalidCursor => 400,
            Self::InvalidFileSelection => 400,
            Self::InvalidRange => 400,
            Self::PayloadTooLarge => 413,
            Self::LimitExceeded => 429,
            Self::SessionNotIdle => 409,
            Self::WorkspaceActivationRequired => 409,
            Self::WorkspaceNotLive => 409,
            Self::SessionDeleting => 409,
            Self::SessionDeleted => 410,
            Self::DeletionInProgress => 409,
            Self::OperationNotCancelable => 409,
            Self::ApprovalNotFound => 404,
            Self::ApprovalAlreadyResolved => 409,
            Self::FileNotFound => 404,
            Self::ContentMissing => 409,
            Self::ExportNotFound => 404,
            Self::ExportNotReady => 409,
            Self::ExportExpired => 410,
            Self::ExportRevoked => 410,
            Self::DownloadGrantExpired => 410,
            Self::TelemetryPayloadTooLarge => 413,
            Self::InvalidTelemetry => 400,
            Self::TelemetryQuotaExceeded => 429,
            Self::TelemetryIncomplete => 409,
            Self::UnsupportedExportSignal => 400,
            Self::InvalidQuery => 400,
            Self::InvalidMetricAggregation => 400,
            Self::InvalidNetworkPolicy => 400,
            Self::UnsupportedPackageEcosystem => 400,
            Self::PackageResolutionFailed => 409,
            Self::PackageArtifactUnavailable => 409,
            Self::PackageIntegrityMismatch => 409,
            Self::UnknownProvider => 400,
            Self::UnknownModel => 400,
            Self::UnqualifiedProviderModel => 409,
            Self::ProviderCredentialNotFound => 404,
            Self::ProviderCredentialRevoked => 409,
            Self::InvalidAutoTopupPolicy => 400,
            Self::PaymentMethodRequired => 402,
            Self::AuthenticationUnavailable => 503,
            Self::AccountPaused => 402,
            Self::AccountStateUnavailable => 503,
            Self::PreconditionFailed => 412,
            Self::WrongWorkspaceRegion => 409,
            Self::ObservabilityUnavailable => 503,
            Self::RateLimited => 429,
            Self::UpstreamError => 502,
            Self::InternalError => 500,
        }
    }

    /// Whether an identical retry can succeed.
    #[must_use]
    pub const fn retryable(self) -> bool {
        match self {
            Self::Unauthenticated => false,
            Self::Forbidden => false,
            Self::InsufficientScope => false,
            Self::TokenInvalid => false,
            Self::TokenRevoked => false,
            Self::TokenExpired => false,
            Self::MalformedToken => false,
            Self::AuthorizationPending => true,
            Self::SlowDown => true,
            Self::NotFound => false,
            Self::Gone => false,
            Self::IdempotencyConflict => false,
            Self::OperationIdempotencyConflict => false,
            Self::InvalidRequest => false,
            Self::InvalidCursor => false,
            Self::InvalidFileSelection => false,
            Self::InvalidRange => false,
            Self::PayloadTooLarge => false,
            Self::LimitExceeded => true,
            Self::SessionNotIdle => true,
            Self::WorkspaceActivationRequired => true,
            Self::WorkspaceNotLive => false,
            Self::SessionDeleting => false,
            Self::SessionDeleted => false,
            Self::DeletionInProgress => false,
            Self::OperationNotCancelable => false,
            Self::ApprovalNotFound => false,
            Self::ApprovalAlreadyResolved => false,
            Self::FileNotFound => false,
            Self::ContentMissing => false,
            Self::ExportNotFound => false,
            Self::ExportNotReady => true,
            Self::ExportExpired => false,
            Self::ExportRevoked => false,
            Self::DownloadGrantExpired => false,
            Self::TelemetryPayloadTooLarge => false,
            Self::InvalidTelemetry => false,
            Self::TelemetryQuotaExceeded => true,
            Self::TelemetryIncomplete => true,
            Self::UnsupportedExportSignal => false,
            Self::InvalidQuery => false,
            Self::InvalidMetricAggregation => false,
            Self::InvalidNetworkPolicy => false,
            Self::UnsupportedPackageEcosystem => false,
            Self::PackageResolutionFailed => false,
            Self::PackageArtifactUnavailable => true,
            Self::PackageIntegrityMismatch => false,
            Self::UnknownProvider => false,
            Self::UnknownModel => false,
            Self::UnqualifiedProviderModel => false,
            Self::ProviderCredentialNotFound => false,
            Self::ProviderCredentialRevoked => false,
            Self::InvalidAutoTopupPolicy => false,
            Self::PaymentMethodRequired => false,
            Self::AuthenticationUnavailable => true,
            Self::AccountPaused => false,
            Self::AccountStateUnavailable => true,
            Self::PreconditionFailed => false,
            Self::WrongWorkspaceRegion => false,
            Self::ObservabilityUnavailable => true,
            Self::RateLimited => true,
            Self::UpstreamError => true,
            Self::InternalError => false,
        }
    }

    /// Which failure family the code belongs to.
    #[must_use]
    pub const fn class(self) -> ErrorClass {
        match self {
            Self::Unauthenticated => ErrorClass::Auth,
            Self::Forbidden => ErrorClass::Auth,
            Self::InsufficientScope => ErrorClass::Auth,
            Self::TokenInvalid => ErrorClass::Auth,
            Self::TokenRevoked => ErrorClass::Auth,
            Self::TokenExpired => ErrorClass::Auth,
            Self::MalformedToken => ErrorClass::Auth,
            Self::AuthorizationPending => ErrorClass::State,
            Self::SlowDown => ErrorClass::Quota,
            Self::NotFound => ErrorClass::NotFound,
            Self::Gone => ErrorClass::NotFound,
            Self::IdempotencyConflict => ErrorClass::Conflict,
            Self::OperationIdempotencyConflict => ErrorClass::Conflict,
            Self::InvalidRequest => ErrorClass::Validation,
            Self::InvalidCursor => ErrorClass::Validation,
            Self::InvalidFileSelection => ErrorClass::Validation,
            Self::InvalidRange => ErrorClass::Validation,
            Self::PayloadTooLarge => ErrorClass::Validation,
            Self::LimitExceeded => ErrorClass::Quota,
            Self::SessionNotIdle => ErrorClass::State,
            Self::WorkspaceActivationRequired => ErrorClass::State,
            Self::WorkspaceNotLive => ErrorClass::State,
            Self::SessionDeleting => ErrorClass::State,
            Self::SessionDeleted => ErrorClass::NotFound,
            Self::DeletionInProgress => ErrorClass::Conflict,
            Self::OperationNotCancelable => ErrorClass::State,
            Self::ApprovalNotFound => ErrorClass::NotFound,
            Self::ApprovalAlreadyResolved => ErrorClass::Conflict,
            Self::FileNotFound => ErrorClass::NotFound,
            Self::ContentMissing => ErrorClass::State,
            Self::ExportNotFound => ErrorClass::NotFound,
            Self::ExportNotReady => ErrorClass::State,
            Self::ExportExpired => ErrorClass::NotFound,
            Self::ExportRevoked => ErrorClass::NotFound,
            Self::DownloadGrantExpired => ErrorClass::NotFound,
            Self::TelemetryPayloadTooLarge => ErrorClass::Validation,
            Self::InvalidTelemetry => ErrorClass::Validation,
            Self::TelemetryQuotaExceeded => ErrorClass::Quota,
            Self::TelemetryIncomplete => ErrorClass::State,
            Self::UnsupportedExportSignal => ErrorClass::Validation,
            Self::InvalidQuery => ErrorClass::Validation,
            Self::InvalidMetricAggregation => ErrorClass::Validation,
            Self::InvalidNetworkPolicy => ErrorClass::Validation,
            Self::UnsupportedPackageEcosystem => ErrorClass::Validation,
            Self::PackageResolutionFailed => ErrorClass::State,
            Self::PackageArtifactUnavailable => ErrorClass::State,
            Self::PackageIntegrityMismatch => ErrorClass::State,
            Self::UnknownProvider => ErrorClass::Validation,
            Self::UnknownModel => ErrorClass::Validation,
            Self::UnqualifiedProviderModel => ErrorClass::State,
            Self::ProviderCredentialNotFound => ErrorClass::NotFound,
            Self::ProviderCredentialRevoked => ErrorClass::State,
            Self::InvalidAutoTopupPolicy => ErrorClass::Validation,
            Self::PaymentMethodRequired => ErrorClass::State,
            Self::AuthenticationUnavailable => ErrorClass::Unavailable,
            Self::AccountPaused => ErrorClass::State,
            Self::AccountStateUnavailable => ErrorClass::Unavailable,
            Self::PreconditionFailed => ErrorClass::Precondition,
            Self::WrongWorkspaceRegion => ErrorClass::Conflict,
            Self::ObservabilityUnavailable => ErrorClass::Unavailable,
            Self::RateLimited => ErrorClass::Quota,
            Self::UpstreamError => ErrorClass::Unavailable,
            Self::InternalError => ErrorClass::Internal,
        }
    }

    /// Which precedence stage may emit the code.
    #[must_use]
    pub const fn precedence_stage(self) -> PrecedenceStage {
        match self {
            Self::Unauthenticated => PrecedenceStage::Authentication,
            Self::Forbidden => PrecedenceStage::Authorization,
            Self::InsufficientScope => PrecedenceStage::Scope,
            Self::TokenInvalid => PrecedenceStage::Authentication,
            Self::TokenRevoked => PrecedenceStage::Authentication,
            Self::TokenExpired => PrecedenceStage::Authentication,
            Self::MalformedToken => PrecedenceStage::Authentication,
            Self::AuthorizationPending => PrecedenceStage::DomainState,
            Self::SlowDown => PrecedenceStage::DomainState,
            Self::NotFound => PrecedenceStage::TombstoneAndParent,
            Self::Gone => PrecedenceStage::TombstoneAndParent,
            Self::IdempotencyConflict => PrecedenceStage::IdempotencyIdentity,
            Self::OperationIdempotencyConflict => PrecedenceStage::OperationIdentity,
            Self::InvalidRequest => PrecedenceStage::BodyLimitAndParse,
            Self::InvalidCursor => PrecedenceStage::BodyLimitAndParse,
            Self::InvalidFileSelection => PrecedenceStage::BodyLimitAndParse,
            Self::InvalidRange => PrecedenceStage::BodyLimitAndParse,
            Self::PayloadTooLarge => PrecedenceStage::TransportEnvelope,
            Self::LimitExceeded => PrecedenceStage::DomainState,
            Self::SessionNotIdle => PrecedenceStage::DomainState,
            Self::WorkspaceActivationRequired => PrecedenceStage::DomainState,
            Self::WorkspaceNotLive => PrecedenceStage::DomainState,
            Self::SessionDeleting => PrecedenceStage::TombstoneAndParent,
            Self::SessionDeleted => PrecedenceStage::TombstoneAndParent,
            Self::DeletionInProgress => PrecedenceStage::TombstoneAndParent,
            Self::OperationNotCancelable => PrecedenceStage::DomainState,
            Self::ApprovalNotFound => PrecedenceStage::TombstoneAndParent,
            Self::ApprovalAlreadyResolved => PrecedenceStage::DomainState,
            Self::FileNotFound => PrecedenceStage::DomainState,
            Self::ContentMissing => PrecedenceStage::DomainState,
            Self::ExportNotFound => PrecedenceStage::TombstoneAndParent,
            Self::ExportNotReady => PrecedenceStage::DomainState,
            Self::ExportExpired => PrecedenceStage::DomainState,
            Self::ExportRevoked => PrecedenceStage::DomainState,
            Self::DownloadGrantExpired => PrecedenceStage::DomainState,
            Self::TelemetryPayloadTooLarge => PrecedenceStage::TransportEnvelope,
            Self::InvalidTelemetry => PrecedenceStage::BodyLimitAndParse,
            Self::TelemetryQuotaExceeded => PrecedenceStage::DomainState,
            Self::TelemetryIncomplete => PrecedenceStage::DomainState,
            Self::UnsupportedExportSignal => PrecedenceStage::BodyLimitAndParse,
            Self::InvalidQuery => PrecedenceStage::BodyLimitAndParse,
            Self::InvalidMetricAggregation => PrecedenceStage::BodyLimitAndParse,
            Self::InvalidNetworkPolicy => PrecedenceStage::BodyLimitAndParse,
            Self::UnsupportedPackageEcosystem => PrecedenceStage::BodyLimitAndParse,
            Self::PackageResolutionFailed => PrecedenceStage::DomainState,
            Self::PackageArtifactUnavailable => PrecedenceStage::DomainState,
            Self::PackageIntegrityMismatch => PrecedenceStage::DomainState,
            Self::UnknownProvider => PrecedenceStage::BodyLimitAndParse,
            Self::UnknownModel => PrecedenceStage::BodyLimitAndParse,
            Self::UnqualifiedProviderModel => PrecedenceStage::DomainState,
            Self::ProviderCredentialNotFound => PrecedenceStage::TombstoneAndParent,
            Self::ProviderCredentialRevoked => PrecedenceStage::DomainState,
            Self::InvalidAutoTopupPolicy => PrecedenceStage::BodyLimitAndParse,
            Self::PaymentMethodRequired => PrecedenceStage::DomainState,
            Self::AuthenticationUnavailable => PrecedenceStage::Authentication,
            Self::AccountPaused => PrecedenceStage::AccountState,
            Self::AccountStateUnavailable => PrecedenceStage::AccountState,
            Self::PreconditionFailed => PrecedenceStage::Precondition,
            Self::WrongWorkspaceRegion => PrecedenceStage::Placement,
            Self::ObservabilityUnavailable => PrecedenceStage::DomainState,
            Self::RateLimited => PrecedenceStage::DomainState,
            Self::UpstreamError => PrecedenceStage::Commit,
            Self::InternalError => PrecedenceStage::Commit,
        }
    }

    /// The default human message.
    #[must_use]
    pub const fn default_message(self) -> &'static str {
        match self {
            Self::Unauthenticated => "no valid credential was presented",
            Self::Forbidden => "the principal may not act on this resource",
            Self::InsufficientScope => "the credential lacks the required scope",
            Self::TokenInvalid => "the credential did not verify",
            Self::TokenRevoked => "the credential has been revoked",
            Self::TokenExpired => "the credential has expired",
            Self::MalformedToken => "the credential is not a well-formed AEX credential",
            Self::AuthorizationPending => "the device authorization has not been approved yet",
            Self::SlowDown => "poll less often",
            Self::NotFound => "no such resource",
            Self::Gone => "the resource was deleted",
            Self::IdempotencyConflict => "the idempotency key was reused with a different intent",
            Self::OperationIdempotencyConflict => {
                "the operation id was reused with a different intent"
            }
            Self::InvalidRequest => "the request body failed strict validation",
            Self::InvalidCursor => {
                "the cursor is malformed, expired, or bound to a different query"
            }
            Self::InvalidFileSelection => {
                "the include or exclude selection is not a valid file selection"
            }
            Self::InvalidRange => {
                "the byte range is absent, empty, or larger than one grant may sign"
            }
            Self::PayloadTooLarge => "the encoded body exceeded the effective limit",
            Self::LimitExceeded => "an effective workspace limit was exceeded",
            Self::SessionNotIdle => "the session must be idle for this operation",
            Self::WorkspaceActivationRequired => "the regional workspace is not activated yet",
            Self::WorkspaceNotLive => "the workspace is not live",
            Self::SessionDeleting => "the session is being deleted",
            Self::SessionDeleted => "the session was deleted",
            Self::DeletionInProgress => "a deletion of this resource is already running",
            Self::OperationNotCancelable => "the operation has passed its cancellation point",
            Self::ApprovalNotFound => "no such approval",
            Self::ApprovalAlreadyResolved => "the approval already has a decision",
            Self::FileNotFound => "no such file entry",
            Self::ContentMissing => "the referenced content is not present",
            Self::ExportNotFound => "no such export",
            Self::ExportNotReady => "the export is still preparing",
            Self::ExportExpired => "the export expired",
            Self::ExportRevoked => "the export was revoked",
            Self::DownloadGrantExpired => "the download grant expired",
            Self::TelemetryPayloadTooLarge => "the OTLP payload exceeded its effective limit",
            Self::InvalidTelemetry => "the telemetry payload is not the pinned OTLP revision",
            Self::TelemetryQuotaExceeded => "the telemetry admission quota was exceeded",
            Self::TelemetryIncomplete => {
                "the requested range has recorded gaps and completeness was required"
            }
            Self::UnsupportedExportSignal => {
                "that signal cannot be exported in the requested format"
            }
            Self::InvalidQuery => "the observation query exceeded its structural bounds",
            Self::InvalidMetricAggregation => {
                "the metric aggregation is not computable over the selection"
            }
            Self::InvalidNetworkPolicy => "the requested network policy is not permitted",
            Self::UnsupportedPackageEcosystem => "that package ecosystem is not supported",
            Self::PackageResolutionFailed => "a requested package could not be resolved",
            Self::PackageArtifactUnavailable => "a resolved package artifact could not be fetched",
            Self::PackageIntegrityMismatch => "a package artifact failed its integrity check",
            Self::UnknownProvider => "no such provider",
            Self::UnknownModel => "no such model for that provider",
            Self::UnqualifiedProviderModel => {
                "the provider and model pair has no live conformance receipt"
            }
            Self::ProviderCredentialNotFound => "no such provider credential binding",
            Self::ProviderCredentialRevoked => "the provider credential binding was revoked",
            Self::InvalidAutoTopupPolicy => "the auto top-up policy is not internally consistent",
            Self::PaymentMethodRequired => "the organization has no usable saved payment method",
            Self::AuthenticationUnavailable => "the authorization authority could not be reached",
            Self::AccountPaused => "the account is paused and this route is not pause-exempt",
            Self::AccountStateUnavailable => "the account state could not be established",
            Self::PreconditionFailed => "the `If-Match` or generation precondition did not hold",
            Self::WrongWorkspaceRegion => "the workspace is placed in a different region",
            Self::ObservabilityUnavailable => "the observation authority could not be reached",
            Self::RateLimited => "too many requests",
            Self::UpstreamError => "a dependency failed",
            Self::InternalError => "an unexpected condition occurred",
        }
    }

    /// How a caller fixes the request, when that is knowable.
    #[must_use]
    pub const fn remedy(self) -> Option<&'static str> {
        match self {
            Self::Unauthenticated => Some("present a valid account token or workspace API key"),
            Self::Forbidden => None,
            Self::InsufficientScope => Some("mint a key that carries the scope named in `details`"),
            Self::TokenInvalid => None,
            Self::TokenRevoked => None,
            Self::TokenExpired => None,
            Self::MalformedToken => None,
            Self::AuthorizationPending => None,
            Self::SlowDown => None,
            Self::NotFound => None,
            Self::Gone => None,
            Self::IdempotencyConflict => None,
            Self::OperationIdempotencyConflict => None,
            Self::InvalidRequest => None,
            Self::InvalidCursor => None,
            Self::InvalidFileSelection => None,
            Self::InvalidRange => None,
            Self::PayloadTooLarge => None,
            Self::LimitExceeded => None,
            Self::SessionNotIdle => None,
            Self::WorkspaceActivationRequired => None,
            Self::WorkspaceNotLive => None,
            Self::SessionDeleting => None,
            Self::SessionDeleted => None,
            Self::DeletionInProgress => None,
            Self::OperationNotCancelable => None,
            Self::ApprovalNotFound => None,
            Self::ApprovalAlreadyResolved => None,
            Self::FileNotFound => None,
            Self::ContentMissing => None,
            Self::ExportNotFound => None,
            Self::ExportNotReady => None,
            Self::ExportExpired => None,
            Self::ExportRevoked => None,
            Self::DownloadGrantExpired => None,
            Self::TelemetryPayloadTooLarge => None,
            Self::InvalidTelemetry => None,
            Self::TelemetryQuotaExceeded => None,
            Self::TelemetryIncomplete => None,
            Self::UnsupportedExportSignal => None,
            Self::InvalidQuery => None,
            Self::InvalidMetricAggregation => None,
            Self::InvalidNetworkPolicy => None,
            Self::UnsupportedPackageEcosystem => None,
            Self::PackageResolutionFailed => None,
            Self::PackageArtifactUnavailable => None,
            Self::PackageIntegrityMismatch => None,
            Self::UnknownProvider => None,
            Self::UnknownModel => None,
            Self::UnqualifiedProviderModel => None,
            Self::ProviderCredentialNotFound => None,
            Self::ProviderCredentialRevoked => None,
            Self::InvalidAutoTopupPolicy => None,
            Self::PaymentMethodRequired => None,
            Self::AuthenticationUnavailable => None,
            Self::AccountPaused => None,
            Self::AccountStateUnavailable => None,
            Self::PreconditionFailed => None,
            Self::WrongWorkspaceRegion => {
                Some("reissue the request against the `apiUrl` in `details`")
            }
            Self::ObservabilityUnavailable => None,
            Self::RateLimited => None,
            Self::UpstreamError => None,
            Self::InternalError => None,
        }
    }

    /// Resolves a wire spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|it| it.as_str() == text)
    }
}
