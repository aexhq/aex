//! Wake hints.
//!
//! A hint is never truth. It says "there may be work"; the authority says what
//! the work is. That is why a lost hint is a latency problem and never a
//! correctness problem, and why every worker re-reads its authority after waking.

use aex_wire::ids::{OperationId, SessionId, WorkspaceId};
use aex_wire::types::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{DedupeKey, SchemaVersion};

/// Somewhere that may have work waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(
    tag = "hint",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum WakeHint {
    /// A session may have a runnable turn.
    SessionWork {
        /// Which session.
        session: SessionId,
    },
    /// A durable operation may be due.
    OperationDue {
        /// Which operation.
        operation: OperationId,
    },
    /// A runtime generation may need a control action.
    RuntimeControl {
        /// Which session owns the generation.
        session: SessionId,
    },
    /// Content lifecycle work may be due in a workspace.
    ContentLifecycle {
        /// Which workspace.
        workspace: WorkspaceId,
    },
    /// A telemetry spool may need draining.
    ObservationSpool {
        /// Which workspace.
        workspace: WorkspaceId,
    },
}

/// One wake hint, ready to enqueue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WakeEnvelope {
    /// Which envelope version this is.
    pub schema_version: SchemaVersion,
    /// Where there may be work.
    pub hint: WakeHint,
    /// When the hint was emitted.
    pub emitted_at: Timestamp,
    /// The deduplication key; a hint that arrives twice is still one wake.
    pub dedupe: DedupeKey,
}
