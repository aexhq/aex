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
