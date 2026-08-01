//! Task-only recovery for MCP 2026-07-28.

use aex_brain_domain::DispatchProof;
use aex_wire::ids::ResourceName;

/// Whether registration proved the server's Tasks extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskCapability {
    /// No Tasks extension; ordinary streams have no recovery cursor.
    Unsupported,
    /// Capability and probe round trip were qualified.
    Advertised,
}

/// Server-scoped opaque Task identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct McpTaskId {
    server: ResourceName,
    task: Box<str>,
}

impl McpTaskId {
    /// Constructs an identity with the protocol's 256-byte bound.
    ///
    /// # Errors
    ///
    /// Refuses empty, oversized, NUL-containing, or control-containing ids.
    pub fn new(server: ResourceName, task: &str) -> Result<Self, TaskIdError> {
        if task.is_empty() || task.len() > 256 {
            return Err(TaskIdError::Length { bytes: task.len() });
        }
        if task.chars().any(char::is_control) {
            return Err(TaskIdError::ControlCharacter);
        }
        Ok(Self {
            server,
            task: task.into(),
        })
    }

    /// Qualified server identity.
    #[must_use]
    pub const fn server(&self) -> &ResourceName {
        &self.server
    }

    /// Opaque server-returned id.
    #[must_use]
    pub const fn task(&self) -> &str {
        &self.task
    }
}

/// Invalid Task identity.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TaskIdError {
    /// Task identity length was outside 1..=256 bytes.
    #[error("MCP task id length {bytes} is outside 1..=256")]
    Length {
        /// Observed byte count.
        bytes: usize,
    },
    /// Task identity contained a control character.
    #[error("MCP task id contains a control character")]
    ControlCharacter,
}

/// Scheduler decision after a Streamable HTTP response drops.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropRecovery {
    /// Dispatch provably did not occur, so the same effect may be retried.
    RetrySameEffect,
    /// Reconnect only through tasks/get on this exact identity.
    QuerySameTask(McpTaskId),
    /// No durable identity exists; a second tools/call is forbidden.
    OutcomeUnknown,
}

/// Classifies a dropped response without inventing protocol resumability.
#[must_use]
pub fn after_stream_drop(
    capability: TaskCapability,
    proof: DispatchProof,
    observed_task: Option<McpTaskId>,
) -> DropRecovery {
    if proof == DispatchProof::NotSent {
        return DropRecovery::RetrySameEffect;
    }
    if capability == TaskCapability::Advertised
        && let Some(task) = observed_task
    {
        return DropRecovery::QuerySameTask(task);
    }
    DropRecovery::OutcomeUnknown
}

/// Bounded subset of a tasks/get response required by the durable scheduler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    /// Task is still running.
    Working {
        /// Server-requested poll interval.
        poll_after_ms: u64,
    },
    /// Server requires unsupported interactive input.
    InputRequired,
    /// Task completed and its already-bounded result may be committed.
    Completed,
    /// Task returned a definitive JSON-RPC error.
    Failed,
    /// Server acknowledged cancellation.
    Cancelled,
}

/// Durable action after one tasks/get response. No variant retains a socket,
/// thread, or activation while waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskTransition {
    /// Schedule a durable wake after the bounded poll interval.
    ScheduleWake {
        /// Delay clamped to 1..=30 seconds.
        after_ms: u64,
    },
    /// Issue tasks/cancel and then record a known unsupported-input failure.
    CancelThenKnownFailure,
    /// Commit the already-returned result.
    CommitCompleted,
    /// Commit the server's definitive failure.
    CommitKnownFailure,
    /// Commit cooperative cancellation.
    CommitCancelled,
    /// TTL elapsed without a terminal answer.
    OutcomeUnknownTaskTtlExpired,
}

/// Maps one tasks/get state to a durable scheduler action.
#[must_use]
pub fn task_transition(state: TaskState, ttl_expired: bool) -> TaskTransition {
    if ttl_expired {
        return TaskTransition::OutcomeUnknownTaskTtlExpired;
    }
    match state {
        TaskState::Working { poll_after_ms } => TaskTransition::ScheduleWake {
            after_ms: poll_after_ms.clamp(1_000, 30_000),
        },
        TaskState::InputRequired => TaskTransition::CancelThenKnownFailure,
        TaskState::Completed => TaskTransition::CommitCompleted,
        TaskState::Failed => TaskTransition::CommitKnownFailure,
        TaskState::Cancelled => TaskTransition::CommitCancelled,
    }
}
