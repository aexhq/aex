//! Central control commands.
//!
//! Every command is FIFO-ordered by the account it affects, because two
//! provisioning commands for one workspace must not race, and a revocation must
//! never be applied before the change that caused it.

use aex_wire::ids::{ApiKeyId, OrganizationId, UserId, WorkspaceId};
use aex_wire::types::{Region, Timestamp};
use serde::{Deserialize, Serialize};

use crate::{Epoch, FifoGroup, SchemaVersion};

/// Which email the control worker was asked to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmailKind {
    /// An organization invitation.
    Invitation,
    /// The account was paused for lack of funds.
    AccountPaused,
    /// The account was restored.
    AccountRestored,
    /// A statement was issued.
    StatementIssued,
}

/// Everything the central control worker may be asked to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "command",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ControlCommand {
    /// Create the empty regional workspace behind a central placement.
    ProvisionRegionalWorkspace {
        /// The workspace to create.
        workspace: WorkspaceId,
        /// Its owning organization.
        organization: OrganizationId,
        /// Where it lives, forever.
        region: Region,
    },
    /// Delete a regional workspace as part of the global lifecycle operation.
    DeleteRegionalWorkspace {
        /// The workspace to delete.
        workspace: WorkspaceId,
        /// Its owning organization.
        organization: OrganizationId,
        /// Where it lives.
        region: Region,
    },
    /// Send one transactional email.
    DispatchEmail {
        /// Which email.
        kind: EmailKind,
        /// Who it is about.
        organization: OrganizationId,
        /// Who receives it.
        recipient: UserId,
    },
    /// Push a new revocation epoch out to a region.
    PropagateRevocationEpoch {
        /// Which organization.
        organization: OrganizationId,
        /// The key that was revoked, when one was.
        api_key: Option<ApiKeyId>,
        /// Which region to inform.
        region: Region,
        /// The new epoch.
        epoch: Epoch,
    },
}

/// One control command, ready to enqueue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ControlEnvelope<T> {
    /// Which envelope version this is.
    pub schema_version: SchemaVersion,
    /// The command identity, stable across retries.
    pub command_id: aex_wire::Uuid7,
    /// The FIFO group; always the account the command affects.
    pub group: FifoGroup,
    /// Which delivery attempt this is.
    pub attempt: u32,
    /// When it was first issued.
    pub issued_at: Timestamp,
    /// The command itself.
    pub payload: T,
}

/// Why a region refused a fenced control request.
///
/// A closed vocabulary rather than a free-text code. The caller turns each arm
/// into a retry decision, and a code it has never seen is a decision it cannot
/// make: an open vocabulary would leave the central plane guessing whether an
/// unfamiliar refusal is worth retrying, and guessing wrong in either direction
/// either wedges a workspace or provisions it twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegionalRefusal {
    /// The workspace exists in the region under a different organization.
    OrganizationMismatch,
    /// A newer fence already ran; this attempt is stale.
    FenceSuperseded,
    /// The region recognised the workspace but not this intent.
    IntentConflict,
    /// The region is admitting nothing right now.
    RegionUnavailable,
    /// The region failed in a way it could not classify.
    Internal,
}

impl RegionalRefusal {
    /// Every arm, for the totality test.
    pub const ALL: [Self; 5] = [
        Self::OrganizationMismatch,
        Self::FenceSuperseded,
        Self::IntentConflict,
        Self::RegionUnavailable,
        Self::Internal,
    ];

    /// The stable machine code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::OrganizationMismatch => "regional_organization_mismatch",
            Self::FenceSuperseded => "regional_fence_superseded",
            Self::IntentConflict => "regional_intent_conflict",
            Self::RegionUnavailable => "regional_region_unavailable",
            Self::Internal => "regional_internal",
        }
    }

    /// Whether the identical request may be sent again.
    ///
    /// A superseded fence is **not** retryable under the same fence: a newer
    /// attempt already owns the workspace, and repeating this one would race it.
    #[must_use]
    pub const fn retryable(self) -> bool {
        matches!(self, Self::RegionUnavailable | Self::Internal)
    }
}

/// One ordered request the central control plane makes of a region.
///
/// Workspace lifecycle arms carry their operation fence. Account pause carries
/// the exact account epoch and every per-session transaction repeats that
/// placement predicate, so a delayed pause cannot cross a later resume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "request",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum RegionalControlRequest {
    /// Create the regional half of a workspace.
    ProvisionWorkspace {
        /// Which workspace.
        workspace: WorkspaceId,
        /// Its owning organization.
        organization: OrganizationId,
        /// Where it lives, forever.
        region: Region,
        /// The fence this attempt runs under.
        fence: u64,
        /// The canonical intent, hex-encoded, so a replay is recognisable.
        intent_hash: String,
    },
    /// Remove the regional half of a workspace.
    DeleteWorkspace {
        /// Which workspace.
        workspace: WorkspaceId,
        /// Where it lives.
        region: Region,
        /// The fence this attempt runs under.
        fence: u64,
    },
    /// Interrupt the sessions that were active when an account pause became visible.
    ApplyAccountPause {
        /// Workspace whose placement is paused.
        workspace: WorkspaceId,
        /// Owning organization.
        organization: OrganizationId,
        /// Region holding the session authority.
        region: Region,
        /// Exact account epoch of this pause.
        account_epoch: u64,
    },
}

/// What a region answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "outcome",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum RegionalControlOutcome {
    /// The regional half exists.
    WorkspaceProvisioned {
        /// Which workspace, echoed so a mismatched answer is detectable.
        workspace: WorkspaceId,
        /// Whether this call created it.
        created: bool,
    },
    /// The regional half is gone.
    WorkspaceDeleted {
        /// Which workspace, echoed for the same reason.
        workspace: WorkspaceId,
        /// Whether the regional half is gone.
        removed: bool,
    },
    /// One bounded account-pause page was applied.
    AccountPauseApplied {
        /// Workspace, echoed for subject validation.
        workspace: WorkspaceId,
        /// Whether the durable scan reached its end.
        complete: bool,
        /// Sessions newly interrupted by this page.
        interrupted: u32,
    },
    /// The region refused, and said why.
    Refused {
        /// Which refusal.
        reason: RegionalRefusal,
    },
}

/// One regional control request or answer, versioned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegionalControlEnvelope<T> {
    /// Which envelope version this is.
    pub schema_version: SchemaVersion,
    /// The request identity, stable across retries of the same fence.
    pub request_id: aex_wire::Uuid7,
    /// The request or the answer.
    pub payload: T,
}
