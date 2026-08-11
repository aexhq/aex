//! GENERATED — DO NOT EDIT.
//!
//! The identifier registry and its newtypes.
//!
//! Produced by `aex-contract-gen` from `api/`; contract digest
//! `sha256:baac0d51775c29553d4772dc56103c1c178a1312a14f00ea68ef2aa53741c21d`.
//! Regenerate with `cargo run -p aex-contract-gen -- build`.

#![allow(clippy::large_enum_variant, reason = "a wire union is never boxed")]
#![allow(clippy::match_same_arms, reason = "one arm per row")]
#![allow(clippy::too_many_lines, reason = "one arm per row")]

use crate::ids::prefixed_id;

/// Every resource an AEX identifier can name.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum IdKind {
    /// An authenticated person.
    User,
    /// The billing and ownership boundary.
    Organization,
    /// A person's role inside one organization.
    Membership,
    /// A pending invitation to join an organization.
    Invitation,
    /// A region-pinned execution and content boundary.
    Workspace,
    /// Workspace API key metadata; never the secret.
    ApiKey,
    /// A BYOK provider-credential binding.
    ProviderCredential,
    /// A durable conversation and its workspace.
    Session,
    /// One message in a session.
    Message,
    /// One internal execution agent, root or subagent.
    Agent,
    /// One bound tool invocation.
    ToolCall,
    /// A durable operation record.
    Operation,
    /// A pending or resolved tool approval.
    Approval,
    /// One exact `MicroVM` generation.
    Generation,
    /// One ephemeral exact-generation live-file upload.
    FileUpload,
    /// One ephemeral exact-generation live-file download.
    FileDownload,
    /// One admitted telemetry observation.
    Observation,
    /// One admitted OTLP batch.
    TelemetryBatch,
    /// One recorded telemetry gap.
    TelemetryGap,
    /// One telemetry export artifact.
    Export,
    /// One staged multipart upload.
    Upload,
    /// One metered download measurement.
    Measurement,
    /// One immutable issued billing statement.
    Statement,
}

impl IdKind {
    /// Every kind, in registry order.
    pub const ALL: &'static [IdKind] = &[
        IdKind::User,
        IdKind::Organization,
        IdKind::Membership,
        IdKind::Invitation,
        IdKind::Workspace,
        IdKind::ApiKey,
        IdKind::ProviderCredential,
        IdKind::Session,
        IdKind::Message,
        IdKind::Agent,
        IdKind::ToolCall,
        IdKind::Operation,
        IdKind::Approval,
        IdKind::Generation,
        IdKind::FileUpload,
        IdKind::FileDownload,
        IdKind::Observation,
        IdKind::TelemetryBatch,
        IdKind::TelemetryGap,
        IdKind::Export,
        IdKind::Upload,
        IdKind::Measurement,
        IdKind::Statement,
    ];

    /// The wire prefix, without the trailing underscore.
    #[must_use]
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::User => "usr",
            Self::Organization => "org",
            Self::Membership => "mem",
            Self::Invitation => "inv",
            Self::Workspace => "wsp",
            Self::ApiKey => "key",
            Self::ProviderCredential => "pcr",
            Self::Session => "ses",
            Self::Message => "msg",
            Self::Agent => "agt",
            Self::ToolCall => "tcl",
            Self::Operation => "op",
            Self::Approval => "apr",
            Self::Generation => "gen",
            Self::FileUpload => "ful",
            Self::FileDownload => "fdl",
            Self::Observation => "obs",
            Self::TelemetryBatch => "bch",
            Self::TelemetryGap => "gap",
            Self::Export => "exp",
            Self::Upload => "upl",
            Self::Measurement => "msr",
            Self::Statement => "stm",
        }
    }

    /// The anchored pattern the registry publishes for this kind.
    #[must_use]
    pub const fn pattern(self) -> &'static str {
        match self {
            Self::User => "^usr_[0-9a-hjkmnp-tv-z]{26}$",
            Self::Organization => "^org_[0-9a-hjkmnp-tv-z]{26}$",
            Self::Membership => "^mem_[0-9a-hjkmnp-tv-z]{26}$",
            Self::Invitation => "^inv_[0-9a-hjkmnp-tv-z]{26}$",
            Self::Workspace => "^wsp_[0-9a-hjkmnp-tv-z]{26}$",
            Self::ApiKey => "^key_[0-9a-hjkmnp-tv-z]{26}$",
            Self::ProviderCredential => "^pcr_[0-9a-hjkmnp-tv-z]{26}$",
            Self::Session => "^ses_[0-9a-hjkmnp-tv-z]{26}$",
            Self::Message => "^msg_[0-9a-hjkmnp-tv-z]{26}$",
            Self::Agent => "^agt_[0-9a-hjkmnp-tv-z]{26}$",
            Self::ToolCall => "^tcl_[0-9a-hjkmnp-tv-z]{26}$",
            Self::Operation => "^op_[0-9a-hjkmnp-tv-z]{26}$",
            Self::Approval => "^apr_[0-9a-hjkmnp-tv-z]{26}$",
            Self::Generation => "^gen_[0-9a-hjkmnp-tv-z]{26}$",
            Self::FileUpload => "^ful_[0-9a-hjkmnp-tv-z]{26}$",
            Self::FileDownload => "^fdl_[0-9a-hjkmnp-tv-z]{26}$",
            Self::Observation => "^obs_[0-9a-hjkmnp-tv-z]{26}$",
            Self::TelemetryBatch => "^bch_[0-9a-hjkmnp-tv-z]{26}$",
            Self::TelemetryGap => "^gap_[0-9a-hjkmnp-tv-z]{26}$",
            Self::Export => "^exp_[0-9a-hjkmnp-tv-z]{26}$",
            Self::Upload => "^upl_[0-9a-hjkmnp-tv-z]{26}$",
            Self::Measurement => "^msr_[0-9a-hjkmnp-tv-z]{26}$",
            Self::Statement => "^stm_[0-9a-hjkmnp-tv-z]{26}$",
        }
    }

    /// The `snake_case` registry key.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Organization => "organization",
            Self::Membership => "membership",
            Self::Invitation => "invitation",
            Self::Workspace => "workspace",
            Self::ApiKey => "api_key",
            Self::ProviderCredential => "provider_credential",
            Self::Session => "session",
            Self::Message => "message",
            Self::Agent => "agent",
            Self::ToolCall => "tool_call",
            Self::Operation => "operation",
            Self::Approval => "approval",
            Self::Generation => "generation",
            Self::FileUpload => "file_upload",
            Self::FileDownload => "file_download",
            Self::Observation => "observation",
            Self::TelemetryBatch => "telemetry_batch",
            Self::TelemetryGap => "telemetry_gap",
            Self::Export => "export",
            Self::Upload => "upload",
            Self::Measurement => "measurement",
            Self::Statement => "statement",
        }
    }
}

prefixed_id!(
    /// An authenticated person.
    UserId,
    User
);

prefixed_id!(
    /// The billing and ownership boundary.
    OrganizationId,
    Organization
);

prefixed_id!(
    /// A person's role inside one organization.
    MembershipId,
    Membership
);

prefixed_id!(
    /// A pending invitation to join an organization.
    InvitationId,
    Invitation
);

prefixed_id!(
    /// A region-pinned execution and content boundary.
    WorkspaceId,
    Workspace
);

prefixed_id!(
    /// Workspace API key metadata; never the secret.
    ApiKeyId,
    ApiKey
);

prefixed_id!(
    /// A BYOK provider-credential binding.
    ProviderCredentialId,
    ProviderCredential
);

prefixed_id!(
    /// A durable conversation and its workspace.
    SessionId,
    Session
);

prefixed_id!(
    /// One message in a session.
    MessageId,
    Message
);

prefixed_id!(
    /// One internal execution agent, root or subagent.
    AgentId,
    Agent
);

prefixed_id!(
    /// One bound tool invocation.
    ToolCallId,
    ToolCall
);

prefixed_id!(
    /// A durable operation record.
    OperationId,
    Operation
);

prefixed_id!(
    /// A pending or resolved tool approval.
    ApprovalId,
    Approval
);

prefixed_id!(
    /// One exact `MicroVM` generation.
    GenerationId,
    Generation
);

prefixed_id!(
    /// One ephemeral exact-generation live-file upload.
    FileUploadId,
    FileUpload
);

prefixed_id!(
    /// One ephemeral exact-generation live-file download.
    FileDownloadId,
    FileDownload
);

prefixed_id!(
    /// One admitted telemetry observation.
    ObservationId,
    Observation
);

prefixed_id!(
    /// One admitted OTLP batch.
    TelemetryBatchId,
    TelemetryBatch
);

prefixed_id!(
    /// One recorded telemetry gap.
    TelemetryGapId,
    TelemetryGap
);

prefixed_id!(
    /// One telemetry export artifact.
    ExportId,
    Export
);

prefixed_id!(
    /// One staged multipart upload.
    UploadId,
    Upload
);

prefixed_id!(
    /// One metered download measurement.
    MeasurementId,
    Measurement
);

prefixed_id!(
    /// One immutable issued billing statement.
    StatementId,
    Statement
);
