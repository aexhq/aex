//! Pure preparation for the five Brain-control tools.
//!
//! This module owns no filesystem path and performs no I/O. Parking, approval
//! persistence, content writes, and the absorbing terminal commit stay in the
//! Brain application layer; the values prepared here are total functions of
//! their committed inputs.

use aex_wire::CanonicalJson;
use aex_wire::ids::ContentHash;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One folded todo item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TodoItem {
    /// Stable task wording.
    pub content: String,
    /// Folded task status.
    pub status: TodoStatus,
    /// Present-progress wording.
    pub active_form: String,
}

/// The closed todo status vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    /// Work not started.
    Pending,
    /// The active item.
    InProgress,
    /// Finished work.
    Completed,
}

/// Folded todo counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoCounts {
    /// Pending items.
    pub pending: u32,
    /// Active items.
    pub in_progress: u32,
    /// Completed items.
    pub completed: u32,
}

/// Canonical `todo_read` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoReadResult {
    /// Full replacement list.
    pub todos: Vec<TodoItem>,
    /// Counts derived from that list.
    pub counts: TodoCounts,
}

/// Canonical `todo_write` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoWriteResult {
    /// Number of accepted rows.
    pub accepted: u32,
    /// Counts derived from the replacement.
    pub counts: TodoCounts,
    /// Stable human-readable summary.
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TodoWriteArgs {
    todos: Vec<TodoItem>,
}

/// Applies a full todo replacement.
///
/// The returned [`CanonicalJson`] is the fold state consumed by
/// [`render_todo_read`]; it is never written to a workspace file.
///
/// # Errors
///
/// Returns a typed invalid-argument error for schema or item-bound failures.
pub fn apply_todo_write(
    arguments: &CanonicalJson,
) -> Result<(CanonicalJson, TodoWriteResult), ControlFailure> {
    let args: TodoWriteArgs = serde_json::from_value(arguments.to_value())
        .map_err(|error| ControlFailure::InvalidDocument(error.to_string()))?;
    if args.todos.len() > 200 {
        return Err(ControlFailure::InvalidArgument(
            "todos must contain at most 200 items",
        ));
    }
    for item in &args.todos {
        if item.content.is_empty()
            || item.content.len() > 1_024
            || item.active_form.is_empty()
            || item.active_form.len() > 1_024
        {
            return Err(ControlFailure::InvalidArgument(
                "todo content and activeForm must each contain 1..=1024 bytes",
            ));
        }
    }
    let counts = count(&args.todos);
    let accepted = u32::try_from(args.todos.len())
        .map_err(|_| ControlFailure::InvalidArgument("todos must contain at most 200 items"))?;
    let state = CanonicalJson::from_value(
        &serde_json::to_value(&args)
            .map_err(|error| ControlFailure::InvalidDocument(error.to_string()))?,
    )
    .map_err(|error| ControlFailure::InvalidDocument(error.to_string()))?;
    let result = TodoWriteResult {
        accepted,
        counts,
        summary: format!(
            "accepted {accepted} todos: {} pending, {} in progress, {} completed",
            counts.pending, counts.in_progress, counts.completed
        ),
    };
    Ok((state, result))
}

/// Renders `todo_read` from the folded last successful write.
///
/// # Errors
///
/// Returns an invalid-document error if a corrupt fold view is supplied.
pub fn render_todo_read(
    last_todo_write: Option<&CanonicalJson>,
) -> Result<TodoReadResult, ControlFailure> {
    let todos = match last_todo_write {
        Some(state) => {
            let args: TodoWriteArgs = serde_json::from_value(state.to_value())
                .map_err(|error| ControlFailure::InvalidDocument(error.to_string()))?;
            args.todos
        }
        None => Vec::new(),
    };
    let counts = count(&todos);
    Ok(TodoReadResult { todos, counts })
}

fn count(todos: &[TodoItem]) -> TodoCounts {
    let mut counts = TodoCounts::default();
    for todo in todos {
        match todo.status {
            TodoStatus::Pending => counts.pending += 1,
            TodoStatus::InProgress => counts.in_progress += 1,
            TodoStatus::Completed => counts.completed += 1,
        }
    }
    counts
}

/// A validated durable wait decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaitPlan {
    /// Identity derived from the effect identity.
    pub wait_id: ContentHash,
    /// Exact requested delay; never clamped.
    pub delay_ms: u64,
}

/// Validates and identities a durable timer park.
///
/// # Errors
///
/// Refuses values outside 1..=86400 seconds and values beyond the run's exact
/// remaining wall-clock budget.
pub fn prepare_wait(
    seconds: u64,
    remaining_ms: u64,
    effect_id: &ContentHash,
) -> Result<WaitPlan, ControlFailure> {
    if !(1..=86_400).contains(&seconds) {
        return Err(ControlFailure::InvalidArgument(
            "seconds must be in 1..=86400",
        ));
    }
    let requested_ms = seconds * 1_000;
    if requested_ms > remaining_ms {
        return Err(ControlFailure::WaitExceedsRemainingBudget {
            requested_ms,
            remaining_ms,
        });
    }
    let mut identity = Vec::with_capacity(12 + 32);
    identity.extend_from_slice(b"aex.wait.v1\0");
    identity.extend_from_slice(effect_id.as_bytes());
    Ok(WaitPlan {
        wait_id: ContentHash::of(&identity),
        delay_ms: requested_ms,
    })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SubmitResultArgs {
    status: SubmitStatus,
    summary: String,
    #[serde(default)]
    data: Option<Value>,
    #[serde(default)]
    files: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SubmitStatus {
    Success,
    Failure,
}

/// Pure result preparation before content persistence and the terminal commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreparedSubmitResult {
    /// Digest used by `resultRef`.
    pub result_ref: ContentHash,
    /// Whether the absorbing terminal state is `Completed` rather than `Failed`.
    pub success: bool,
    /// Canonical result bytes.
    pub bytes: u64,
    /// Whether the content authority must be written before the referencing commit.
    pub store_external: bool,
    /// Always false: terminal deliverables are refused, never truncated.
    pub truncated: bool,
}

/// Validates, hashes, and budgets an explicit terminal deliverable.
///
/// # Errors
///
/// Returns [`ControlFailure::ResultBytesExhausted`] instead of truncating when
/// the retained-result budget is insufficient.
pub fn prepare_submit_result(
    arguments: &CanonicalJson,
    retained_result_bytes_remaining: u64,
) -> Result<PreparedSubmitResult, ControlFailure> {
    let args: SubmitResultArgs = serde_json::from_value(arguments.to_value())
        .map_err(|error| ControlFailure::InvalidDocument(error.to_string()))?;
    if args.summary.is_empty() || args.summary.len() > 8_192 {
        return Err(ControlFailure::InvalidArgument(
            "summary must contain 1..=8192 bytes",
        ));
    }
    if args.files.len() > 256 || args.files.iter().any(|path| path.len() > 4_096) {
        return Err(ControlFailure::InvalidArgument(
            "files must contain at most 256 paths of at most 4096 bytes",
        ));
    }
    if args.data.as_ref().is_some_and(|data| !data.is_object()) {
        return Err(ControlFailure::InvalidArgument("data must be an object"));
    }
    let success = matches!(args.status, SubmitStatus::Success);
    let bytes = arguments.as_bytes().len() as u64;
    if bytes > retained_result_bytes_remaining {
        return Err(ControlFailure::ResultBytesExhausted {
            required: bytes,
            remaining: retained_result_bytes_remaining,
        });
    }
    Ok(PreparedSubmitResult {
        result_ref: ContentHash::of(arguments.as_bytes()),
        success,
        bytes,
        store_external: bytes > 32_768,
        truncated: false,
    })
}

/// A Brain-control tool could not be prepared.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ControlFailure {
    /// A stable argument-bound violation.
    #[error("invalid control tool argument: {0}")]
    InvalidArgument(&'static str),
    /// JSON shape did not match the control contract.
    #[error("invalid control tool document: {0}")]
    InvalidDocument(String),
    /// The requested wait cannot complete before the run's wall bound.
    #[error("wait requests {requested_ms} ms but only {remaining_ms} ms remain")]
    WaitExceedsRemainingBudget {
        /// Exact requested duration.
        requested_ms: u64,
        /// Exact remaining duration.
        remaining_ms: u64,
    },
    /// The terminal result would exceed retained-result dimension B.
    #[error("terminal result needs {required} bytes but {remaining} remain")]
    ResultBytesExhausted {
        /// Canonical bytes needed.
        required: u64,
        /// Budget still available.
        remaining: u64,
    },
}
