//! The session journal envelope.
//!
//! Ownership splits deliberately: `aex-session-domain` owns the envelope, the
//! ordering algebra and the authority fold; `aex-brain-domain` owns payload
//! interpretation and effect receipts and consumes these types. This crate owns
//! only the kind vocabulary, because both sides have to agree on it and neither
//! may extend it unilaterally.
//!
//! [`JournalEntryKind`] is **closed**. A catch-all arm would let an unknown
//! entry pass through the authority fold as "something else", and an authority
//! that folds over values it does not understand is not an authority.

use aex_wire::ids::{AgentId, GenerationId, MessageId, SessionId, ToolCallId};

use crate::RunId;
use aex_wire::types::{DecimalU128, Timestamp};
use serde::{Deserialize, Serialize};

use crate::SchemaVersion;

/// Every kind of journal entry the session authority folds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalEntryKind {
    /// A run was admitted.
    RunAdmitted,
    /// A run started executing.
    RunStarted,
    /// A run reached a terminal status.
    RunTerminal,
    /// A user message was recorded.
    UserMessage,
    /// An assistant message was recorded.
    AssistantMessage,
    /// A tool call was bound and persisted before dispatch.
    ToolCallBound,
    /// A tool call produced a terminal result.
    ToolCallTerminal,
    /// A subagent was admitted.
    SubagentAdmitted,
    /// A subagent reached a terminal status.
    SubagentTerminal,
    /// An approval was raised.
    ApprovalRaised,
    /// An approval was decided.
    ApprovalResolved,
    /// A generation was launched.
    GenerationLaunched,
    /// A generation was suspended.
    GenerationSuspended,
    /// A generation was resumed.
    GenerationResumed,
    /// A generation was terminated.
    GenerationTerminated,
    /// The live workspace was persisted.
    WorkspacePersisted,
    /// The live workspace was discarded.
    WorkspaceDiscarded,
    /// Credential custody was rebound.
    CustodyRebound,
    /// A durable operation was admitted.
    OperationAdmitted,
    /// A durable operation committed.
    OperationCommitted,
    /// The session was forked.
    SessionForked,
    /// The session was tombstoned.
    SessionDeleted,
}

impl JournalEntryKind {
    /// Every kind, in fold order.
    pub const ALL: [Self; 22] = [
        Self::RunAdmitted,
        Self::RunStarted,
        Self::RunTerminal,
        Self::UserMessage,
        Self::AssistantMessage,
        Self::ToolCallBound,
        Self::ToolCallTerminal,
        Self::SubagentAdmitted,
        Self::SubagentTerminal,
        Self::ApprovalRaised,
        Self::ApprovalResolved,
        Self::GenerationLaunched,
        Self::GenerationSuspended,
        Self::GenerationResumed,
        Self::GenerationTerminated,
        Self::WorkspacePersisted,
        Self::WorkspaceDiscarded,
        Self::CustodyRebound,
        Self::OperationAdmitted,
        Self::OperationCommitted,
        Self::SessionForked,
        Self::SessionDeleted,
    ];

    /// Whether the entry can only be written by the session authority itself.
    ///
    /// Brain writes execution entries; lifecycle entries are the authority's, so
    /// a compromised Brain cannot forge a deletion.
    #[must_use]
    pub const fn is_authority_only(self) -> bool {
        matches!(
            self,
            Self::OperationAdmitted
                | Self::OperationCommitted
                | Self::SessionForked
                | Self::SessionDeleted
                | Self::WorkspacePersisted
                | Self::WorkspaceDiscarded
                | Self::CustodyRebound
        )
    }
}

/// What one journal entry is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct JournalSubject {
    /// The run, when the entry belongs to one.
    pub run: Option<RunId>,
    /// The agent, when the entry belongs to one.
    pub agent: Option<AgentId>,
    /// The message, when the entry records one.
    pub message: Option<MessageId>,
    /// The tool call, when the entry records one.
    pub tool_call: Option<ToolCallId>,
    /// The generation, when the entry records one.
    pub generation: Option<GenerationId>,
}

/// The envelope every journal entry shares.
///
/// The payload stays opaque here: this crate fixes ordering and identity, and
/// `aex-brain-domain` interprets what is inside.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct JournalEnvelope {
    /// Which envelope version this is.
    pub schema_version: SchemaVersion,
    /// The owning session.
    pub session: SessionId,
    /// The contiguous position in the session's ordered journal.
    pub sequence: DecimalU128,
    /// Which kind of entry this is.
    pub kind: JournalEntryKind,
    /// What the entry is about.
    pub subject: JournalSubject,
    /// When the writer recorded it.
    pub recorded_at: Timestamp,
    /// The canonical payload, interpreted by the Brain domain.
    pub payload: aex_wire::CanonicalJson,
}
