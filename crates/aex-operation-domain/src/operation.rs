//! The durable operation envelope and its transitions.
//!
//! Two things here are load bearing. `committed_at` is a **latch orthogonal to
//! `status`**: once it is set the operation is non-cancelable even while it is
//! still `Running`, and `fail` after it is a typed error rather than a silent
//! overwrite. And every inline operation still creates a durable envelope,
//! committed already `Succeeded` (D-07), so one `GET /operations/{id}` answers
//! for both execution shapes without a workflow engine.

use aex_wire::CanonicalJson;
use aex_wire::error::ErrorCode;
use aex_wire::idempotency::IntentDigest;
use aex_wire::ids::{MeasurementId, OperationId, SessionId, WorkspaceId};
use aex_wire::types::Timestamp;

use crate::cursor::ContinuationCursor;

/// What a durable operation does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OperationKind {
    /// Stop the session's work.
    SessionStop,
    /// Persist the live workspace into the durable tree.
    SessionPersist,
    /// Clone the session.
    SessionClone,
    /// Discard the live workspace.
    WorkspaceDiscard,
    /// Rebind session credential custody.
    CredentialRebind,
    /// Move the session into the recovery window.
    SessionTrash,
    /// Bring the session back out of the recovery window.
    SessionRestore,
    /// Destroy the session.
    SessionPurge,
    /// Destroy a whole workspace.
    WorkspacePurge,
    /// Export telemetry.
    TelemetryExport,
    /// Collect unreferenced content.
    ContentGc,
}

/// How an operation runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Execution {
    /// It completes inside the admission transaction.
    Inline,
    /// It runs as a fenced sequence of bounded steps.
    Continued,
}

impl OperationKind {
    /// Every kind, in canonical order.
    pub const ALL: [Self; 11] = [
        Self::SessionStop,
        Self::SessionPersist,
        Self::SessionClone,
        Self::WorkspaceDiscard,
        Self::CredentialRebind,
        Self::SessionTrash,
        Self::SessionRestore,
        Self::SessionPurge,
        Self::WorkspacePurge,
        Self::TelemetryExport,
        Self::ContentGc,
    ];

    /// The stable wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SessionStop => "session_stop",
            Self::SessionPersist => "session_persist",
            Self::SessionClone => "session_clone",
            Self::WorkspaceDiscard => "workspace_discard",
            Self::CredentialRebind => "credential_rebind",
            Self::SessionTrash => "session_trash",
            Self::SessionRestore => "session_restore",
            Self::SessionPurge => "session_purge",
            Self::WorkspacePurge => "workspace_purge",
            Self::TelemetryExport => "telemetry_export",
            Self::ContentGc => "content_gc",
        }
    }

    /// How the kind runs by default. A `SessionPersist` may still escalate; see
    /// [`classify_persist`].
    #[must_use]
    pub const fn execution(self) -> Execution {
        match self {
            Self::SessionStop
            | Self::SessionPersist
            | Self::SessionClone
            | Self::WorkspaceDiscard
            | Self::CredentialRebind
            | Self::SessionTrash
            | Self::SessionRestore => Execution::Inline,
            Self::SessionPurge | Self::WorkspacePurge | Self::TelemetryExport | Self::ContentGc => {
                Execution::Continued
            }
        }
    }

    /// Whether a caller may still cancel the operation the instant it is
    /// accepted.
    #[must_use]
    pub const fn cancelable_on_accept(self) -> bool {
        !matches!(
            self,
            Self::SessionStop | Self::SessionTrash | Self::SessionPurge | Self::WorkspacePurge
        )
    }

    /// Whether the kind claims the session's deletion guard.
    #[must_use]
    pub const fn claims_session_deletion(self) -> bool {
        matches!(self, Self::SessionTrash | Self::SessionPurge)
    }

    /// Whether the kind's result is content free and therefore survives a purge
    /// redaction intact.
    #[must_use]
    pub const fn result_survives_purge(self) -> bool {
        matches!(
            self,
            Self::SessionStop | Self::SessionTrash | Self::SessionPurge
        )
    }
}

/// How much a persist would move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PersistShape {
    /// How many leaves change.
    pub changed_leaves: u64,
    /// How many pages change.
    pub changed_pages: u64,
    /// How many body bytes move.
    pub bytes_moved: u64,
}

/// The largest shape that still fits one admission transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InlineBudget {
    /// Largest inline `changed_leaves`.
    pub max_changed_leaves: u64,
    /// Largest inline `changed_pages`.
    pub max_changed_pages: u64,
    /// Largest inline `bytes_moved`.
    pub max_bytes_moved: u64,
}

impl InlineBudget {
    /// The launch budget. A persist above any dimension escalates rather than
    /// timing out (D-09).
    pub const DEFAULT: Self = Self {
        max_changed_leaves: 1_000,
        max_changed_pages: 64,
        max_bytes_moved: 64 * 1024 * 1024,
    };
}

/// Whether a persist of this shape can run inline.
///
/// Monotone in every dimension: growing a shape never turns a continued persist
/// back into an inline one.
#[must_use]
pub const fn classify_persist(shape: &PersistShape, budget: &InlineBudget) -> Execution {
    if shape.changed_leaves > budget.max_changed_leaves
        || shape.changed_pages > budget.max_changed_pages
        || shape.bytes_moved > budget.max_bytes_moved
    {
        Execution::Continued
    } else {
        Execution::Inline
    }
}

/// Where an operation is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OperationStatus {
    /// Accepted, not started.
    Queued,
    /// Started, not terminal.
    Running,
    /// Terminal, successful.
    Succeeded,
    /// Terminal, failed.
    Failed,
    /// Terminal, cancelled.
    Cancelled,
}

impl OperationStatus {
    /// Every status, in lifecycle order.
    pub const ALL: [Self; 5] = [
        Self::Queued,
        Self::Running,
        Self::Succeeded,
        Self::Failed,
        Self::Cancelled,
    ];

    /// Whether no transition leaves this status.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

/// How a failure should be treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FailureClass {
    /// The step may be retried.
    Retryable,
    /// The operation is over.
    Terminal,
    /// The item exhausted its attempts and is never redriven again (D-10).
    PoisonManualReview,
}

/// How far a continued operation has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Progress {
    /// How many units are done.
    pub processed: u64,
    /// How many units are expected, when that is known.
    pub total_hint: Option<u64>,
}

impl Progress {
    /// The progress a fresh operation reports.
    pub const START: Self = Self {
        processed: 0,
        total_hint: None,
    };

    /// Whether `next` is a legal successor of `self`.
    ///
    /// # Errors
    ///
    /// Returns [`ProgressError`] when `processed` regresses or exceeds a
    /// declared total.
    pub const fn check_successor(self, next: Self) -> Result<(), ProgressError> {
        if next.processed < self.processed {
            return Err(ProgressError::Regressed {
                from: self.processed,
                to: next.processed,
            });
        }
        if let Some(total) = next.total_hint
            && next.processed > total
        {
            return Err(ProgressError::AboveTotal {
                processed: next.processed,
                total,
            });
        }
        Ok(())
    }
}

/// Why a progress report was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProgressError {
    /// `processed` went backwards.
    #[error("progress regressed from {from} to {to}")]
    Regressed {
        /// The previous count.
        from: u64,
        /// The reported count.
        to: u64,
    },
    /// `processed` exceeded the declared total.
    #[error("progress {processed} exceeds the declared total {total}")]
    AboveTotal {
        /// The reported count.
        processed: u64,
        /// The declared total.
        total: u64,
    },
}

/// What an operation produced.
///
/// The measurement identity is an envelope fact and survives purge redaction;
/// `content` is the customer-visible payload and does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationResult {
    /// The metered artifact this operation produced, when it produced one.
    pub measurement: Option<MeasurementId>,
    /// The content-bearing payload.
    pub content: Option<CanonicalJson>,
}

impl OperationResult {
    /// A content-free receipt.
    #[must_use]
    pub const fn receipt() -> Self {
        Self {
            measurement: None,
            content: None,
        }
    }

    /// Whether the result carries customer content.
    #[must_use]
    pub const fn is_content_bearing(&self) -> bool {
        self.content.is_some()
    }
}

/// Why an operation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationFailure {
    /// The stable public code.
    pub code: ErrorCode,
    /// How the failure should be treated.
    pub class: FailureClass,
    /// The content-bearing detail.
    pub detail: Option<CanonicalJson>,
}

impl OperationFailure {
    /// A content-free failure.
    #[must_use]
    pub const fn bare(code: ErrorCode, class: FailureClass) -> Self {
        Self {
            code,
            class,
            detail: None,
        }
    }
}

/// The scope an operation acts in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OperationScope {
    /// One session.
    Session(SessionId),
    /// A whole workspace.
    Workspace(WorkspaceId),
}

/// One durable operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operation {
    /// The caller-minted identity.
    pub id: OperationId,
    /// The owning workspace.
    pub workspace: WorkspaceId,
    /// The session, when the operation names one.
    pub session: Option<SessionId>,
    /// What it does.
    pub kind: OperationKind,
    /// Where it is.
    pub status: OperationStatus,
    /// What was asked for.
    pub intent: IntentDigest,
    /// What it acts on.
    pub scope: OperationScope,
    /// How far it has got.
    pub progress: Option<Progress>,
    /// Where to resume.
    pub cursor: Option<ContinuationCursor>,
    /// Whether a cancel has been recorded.
    pub cancel_requested: bool,
    /// What it produced.
    pub result: Option<OperationResult>,
    /// Why it failed.
    pub error: Option<OperationFailure>,
    /// When it was accepted.
    pub created_at: Timestamp,
    /// When it started.
    pub started_at: Option<Timestamp>,
    /// When it last changed.
    pub updated_at: Timestamp,
    /// When its destructive point of no return was latched.
    pub committed_at: Option<Timestamp>,
    /// When it reached a terminal status.
    pub terminal_at: Option<Timestamp>,
}

impl Operation {
    /// Whether a cancel would be accepted right now.
    ///
    /// Three things must all hold: the kind is cancelable at all, the commit
    /// latch is open, and the status is not already terminal.
    #[must_use]
    pub const fn cancelable(&self) -> bool {
        self.kind.cancelable_on_accept()
            && self.committed_at.is_none()
            && !self.status.is_terminal()
    }
}

/// The next state of an operation plus what changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationCommit {
    /// The operation after the transition.
    pub operation: Operation,
    /// Whether the transition latched `committed_at`.
    pub latched_commit: bool,
}

impl OperationCommit {
    fn of(operation: Operation, latched_commit: bool) -> Self {
        Self {
            operation,
            latched_commit,
        }
    }
}

/// Why a transition was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TransitionError {
    /// The transition is not legal from the current status.
    #[error("cannot move an operation from {from:?} to {to:?}")]
    WrongStatus {
        /// The current status.
        from: OperationStatus,
        /// The requested status.
        to: OperationStatus,
    },
    /// A failure was reported after the commit latch closed.
    #[error("operation committed at {committed_at:?} and can no longer fail")]
    FailAfterCommit {
        /// When the latch closed.
        committed_at: Timestamp,
    },
    /// The progress report was not a legal successor.
    #[error(transparent)]
    InvalidProgress(#[from] ProgressError),
}

/// Why a cancel was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CancelRejection {
    /// The kind never accepts a cancel, or the commit latch has closed.
    #[error("operation of kind {kind:?} is not cancelable (committed: {committed})")]
    NotCancelable {
        /// The kind.
        kind: OperationKind,
        /// Whether the latch had closed.
        committed: bool,
    },
    /// The operation is already terminal.
    #[error("operation is already {0:?}")]
    AlreadyTerminal(OperationStatus),
}

/// Starts a queued operation.
///
/// # Errors
///
/// Returns [`TransitionError::WrongStatus`] from any status but `Queued`.
pub fn start(operation: &Operation, now: Timestamp) -> Result<OperationCommit, TransitionError> {
    if operation.status != OperationStatus::Queued {
        return Err(TransitionError::WrongStatus {
            from: operation.status,
            to: OperationStatus::Running,
        });
    }
    let mut next = operation.clone();
    next.status = OperationStatus::Running;
    next.started_at = Some(now);
    next.updated_at = now;
    Ok(OperationCommit::of(next, false))
}

/// Records progress on a running operation.
///
/// # Errors
///
/// Returns [`TransitionError::WrongStatus`] unless the operation is `Running`,
/// and [`TransitionError::InvalidProgress`] when the report regresses.
pub fn progress(
    operation: &Operation,
    reported: Progress,
    now: Timestamp,
) -> Result<OperationCommit, TransitionError> {
    if operation.status != OperationStatus::Running {
        return Err(TransitionError::WrongStatus {
            from: operation.status,
            to: OperationStatus::Running,
        });
    }
    operation
        .progress
        .unwrap_or(Progress::START)
        .check_successor(reported)?;
    let mut next = operation.clone();
    next.progress = Some(reported);
    next.updated_at = now;
    Ok(OperationCommit::of(next, false))
}

/// Latches the destructive point of no return.
///
/// The status is untouched: an operation can be `Running` and committed, which
/// is exactly the state that makes it non-cancelable while still working.
/// Latching twice is idempotent and keeps the first instant.
///
/// # Errors
///
/// Returns [`TransitionError::WrongStatus`] for a terminal operation.
pub fn commit_point(
    operation: &Operation,
    result: Option<OperationResult>,
    now: Timestamp,
) -> Result<OperationCommit, TransitionError> {
    if operation.status.is_terminal() {
        return Err(TransitionError::WrongStatus {
            from: operation.status,
            to: operation.status,
        });
    }
    let already = operation.committed_at.is_some();
    let mut next = operation.clone();
    if !already {
        next.committed_at = Some(now);
    }
    if let Some(result) = result {
        next.result = Some(result);
    }
    next.updated_at = now;
    Ok(OperationCommit::of(next, !already))
}

/// Terminalizes an operation as successful.
///
/// Back-fills `committed_at` when a `Queued` operation terminalizes directly,
/// which is how every inline operation is created (D-07).
///
/// # Errors
///
/// Returns [`TransitionError::WrongStatus`] for an already terminal operation.
pub fn succeed(
    operation: &Operation,
    result: OperationResult,
    now: Timestamp,
) -> Result<OperationCommit, TransitionError> {
    if operation.status.is_terminal() {
        return Err(TransitionError::WrongStatus {
            from: operation.status,
            to: OperationStatus::Succeeded,
        });
    }
    let latched = operation.committed_at.is_none();
    let mut next = operation.clone();
    next.status = OperationStatus::Succeeded;
    next.result = Some(result);
    next.error = None;
    if latched {
        next.committed_at = Some(now);
    }
    next.terminal_at = Some(now);
    next.updated_at = now;
    if next.started_at.is_none() {
        next.started_at = Some(now);
    }
    Ok(OperationCommit::of(next, latched))
}

/// Terminalizes an operation as failed.
///
/// # Errors
///
/// Returns [`TransitionError::FailAfterCommit`] once the latch has closed and
/// [`TransitionError::WrongStatus`] for an already terminal operation.
pub fn fail(
    operation: &Operation,
    failure: OperationFailure,
    now: Timestamp,
) -> Result<OperationCommit, TransitionError> {
    if operation.status.is_terminal() {
        return Err(TransitionError::WrongStatus {
            from: operation.status,
            to: OperationStatus::Failed,
        });
    }
    if let Some(committed_at) = operation.committed_at {
        return Err(TransitionError::FailAfterCommit { committed_at });
    }
    let mut next = operation.clone();
    next.status = OperationStatus::Failed;
    next.error = Some(failure);
    next.result = None;
    next.terminal_at = Some(now);
    next.updated_at = now;
    Ok(OperationCommit::of(next, false))
}

/// Cancels an operation.
///
/// A `Queued` operation terminalizes immediately. A `Running` one records the
/// request; its next fenced step observes it and returns
/// [`crate::lease::StepOutcome::Cancelled`]. Cancel is idempotent.
///
/// # Errors
///
/// Returns [`CancelRejection`] when the kind never accepts a cancel, the commit
/// latch has closed, or the operation is already terminal.
pub fn cancel(operation: &Operation, now: Timestamp) -> Result<OperationCommit, CancelRejection> {
    if operation.status.is_terminal() {
        if operation.status == OperationStatus::Cancelled {
            return Ok(OperationCommit::of(operation.clone(), false));
        }
        return Err(CancelRejection::AlreadyTerminal(operation.status));
    }
    if !operation.kind.cancelable_on_accept() || operation.committed_at.is_some() {
        return Err(CancelRejection::NotCancelable {
            kind: operation.kind,
            committed: operation.committed_at.is_some(),
        });
    }
    let mut next = operation.clone();
    next.cancel_requested = true;
    next.updated_at = now;
    if operation.status == OperationStatus::Queued {
        next.status = OperationStatus::Cancelled;
        next.terminal_at = Some(now);
    }
    Ok(OperationCommit::of(next, false))
}

#[cfg(test)]
mod tests {
    use aex_wire::error::ErrorCode;
    use aex_wire::idempotency::IntentDigest;
    use aex_wire::ids::{OperationId, PrefixedId as _, Uuid7, WorkspaceId};
    use aex_wire::types::Timestamp;

    use super::{
        CancelRejection, Execution, FailureClass, InlineBudget, Operation, OperationFailure,
        OperationKind, OperationResult, OperationScope, OperationStatus, PersistShape, Progress,
        TransitionError, cancel, classify_persist, commit_point, fail, start, succeed,
    };

    fn moment(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("in range")
    }

    fn operation(kind: OperationKind) -> Operation {
        let workspace = WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]));
        Operation {
            id: OperationId::from_uuid7(Uuid7::compose(1, [2; 10])),
            workspace,
            session: None,
            kind,
            status: OperationStatus::Queued,
            intent: IntentDigest::from_bytes([0; 32]),
            scope: OperationScope::Workspace(workspace),
            progress: None,
            cursor: None,
            cancel_requested: false,
            result: None,
            error: None,
            created_at: moment(0),
            started_at: None,
            updated_at: moment(0),
            committed_at: None,
            terminal_at: None,
        }
    }

    #[test]
    fn every_kind_classifies_and_persist_escalation_is_monotone() {
        for kind in OperationKind::ALL {
            let _ = kind.execution();
        }
        let small = PersistShape {
            changed_leaves: 1,
            changed_pages: 1,
            bytes_moved: 1,
        };
        assert_eq!(
            classify_persist(&small, &InlineBudget::DEFAULT),
            Execution::Inline
        );
        let big = PersistShape {
            changed_leaves: u64::MAX,
            ..small
        };
        assert_eq!(
            classify_persist(&big, &InlineBudget::DEFAULT),
            Execution::Continued
        );
    }

    #[test]
    fn the_commit_latch_closes_cancel_and_fail() {
        let queued = operation(OperationKind::SessionPersist);
        let running = start(&queued, moment(1)).expect("starts").operation;
        assert!(running.cancelable());
        let committed = commit_point(&running, None, moment(2)).expect("latches");
        assert!(committed.latched_commit);
        let committed = committed.operation;
        assert_eq!(committed.status, OperationStatus::Running);
        assert!(!committed.cancelable());
        assert_eq!(
            fail(
                &committed,
                OperationFailure::bare(ErrorCode::InternalError, FailureClass::Terminal),
                moment(3)
            ),
            Err(TransitionError::FailAfterCommit {
                committed_at: moment(2)
            })
        );
        assert_eq!(
            cancel(&committed, moment(3)),
            Err(CancelRejection::NotCancelable {
                kind: OperationKind::SessionPersist,
                committed: true,
            })
        );
    }

    #[test]
    fn succeed_from_queued_back_fills_the_latch() {
        let queued = operation(OperationKind::SessionStop);
        let done = succeed(&queued, OperationResult::receipt(), moment(5)).expect("succeeds");
        assert!(done.latched_commit);
        assert_eq!(done.operation.committed_at, Some(moment(5)));
        assert_eq!(done.operation.status, OperationStatus::Succeeded);
    }

    #[test]
    fn a_queued_cancel_terminalizes_and_a_running_one_records() {
        let queued = operation(OperationKind::SessionPersist);
        let cancelled = cancel(&queued, moment(1)).expect("cancels").operation;
        assert_eq!(cancelled.status, OperationStatus::Cancelled);
        assert!(cancelled.cancel_requested);
        assert_eq!(
            cancel(&cancelled, moment(2)).expect("idempotent").operation,
            cancelled
        );

        let running = start(&queued, moment(1)).expect("starts").operation;
        let requested = cancel(&running, moment(2)).expect("records").operation;
        assert_eq!(requested.status, OperationStatus::Running);
        assert!(requested.cancel_requested);
    }

    #[test]
    fn a_non_cancelable_kind_refuses_from_acceptance() {
        for kind in [
            OperationKind::SessionStop,
            OperationKind::SessionTrash,
            OperationKind::SessionPurge,
            OperationKind::WorkspacePurge,
        ] {
            assert_eq!(
                cancel(&operation(kind), moment(1)),
                Err(CancelRejection::NotCancelable {
                    kind,
                    committed: false
                })
            );
        }
    }

    #[test]
    fn progress_never_regresses() {
        let running = start(&operation(OperationKind::ContentGc), moment(1))
            .expect("starts")
            .operation;
        let advanced = super::progress(
            &running,
            Progress {
                processed: 10,
                total_hint: Some(100),
            },
            moment(2),
        )
        .expect("advances")
        .operation;
        assert!(
            super::progress(
                &advanced,
                Progress {
                    processed: 9,
                    total_hint: Some(100)
                },
                moment(3)
            )
            .is_err()
        );
    }
}
