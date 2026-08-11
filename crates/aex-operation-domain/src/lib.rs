//! `aex-operation-domain` owns the pure durable-operation, lease and fence model shared by
//! every regional worker.
//!
//! # Invariants
//!
//! - a stale fence is always rejected, whatever the lease timing;
//! - claim, renew, steal, complete and cancel are total transitions over the recorded state;
//! - a due scan is idempotent: rescanning the same due set produces no extra effect;
//! - `committed_at` is orthogonal to `status`, so an operation can be running and
//!   already non-cancelable.
//!
//! # Not this crate's job
//!
//! - queues, tables or schedulers (`aex-work-dynamodb`);
//! - the work a specific operation performs;
//! - wall-clock reads: the current instant is always a parameter.

pub mod admission;
pub mod cursor;
pub mod deletion;
pub mod due;
pub mod lease;
pub mod operation;
pub mod redact;

pub use admission::{AdmissionOutcome, AdmitRequest, ConflictCode, admit};
pub use cursor::{
    CURSOR_MAX_ENCODED_BYTES, CURSOR_VERSION, ContinuationCursor, CursorError, CursorKind,
    CursorPosition, ExportMember, PAGE_DEFAULT, PageError, PageToken, PurgeStage, page_limit,
};
pub use deletion::{DeletionEpoch, DeletionGuard, DeletionState};
pub use due::{
    DueScanPlan, DueShard, PageBudget, ShardScan, backoff, backoff_step, plan_due_scan, shard_of,
};
pub use lease::{
    ClaimDenial, ClaimOutcome, DedupIdentity, Fence, FenceRejection, Lease, OwnerId, StepOutcome,
    WorkCommit, WorkId, WorkItem, WorkState, claim, complete, renew,
};
pub use operation::{
    CancelRejection, Execution, FailureClass, Operation, OperationCommit, OperationFailure,
    OperationKind, OperationResult, OperationScope, OperationStatus, Progress, ProgressError,
    PublicProjectionError, TransitionError, cancel, commit_point, fail, progress,
    revoke_telemetry_export, start, succeed,
};
pub use redact::redact_for_session_delete;
