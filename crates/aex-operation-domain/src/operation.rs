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
use aex_wire::{ObservedErrorCode, models};

use crate::cursor::ContinuationCursor;

/// What a durable operation does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OperationKind {
    /// Cancel the session's current message and return it to idle.
    SessionCancel,
    /// Suspend an idle session's exact retained generation.
    SessionSuspend,
    /// Resume the same retained generation to idle.
    SessionResume,
    /// Permanently destroy compute and ephemeral live files.
    SessionTerminate,
    /// Irreversibly delete session-scoped user content and telemetry.
    SessionDelete,
    /// Destroy a whole workspace.
    WorkspaceDelete,
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
    pub const ALL: [Self; 8] = [
        Self::SessionCancel,
        Self::SessionSuspend,
        Self::SessionResume,
        Self::SessionTerminate,
        Self::SessionDelete,
        Self::WorkspaceDelete,
        Self::TelemetryExport,
        Self::ContentGc,
    ];

    /// The stable wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SessionCancel => "session_cancel",
            Self::SessionSuspend => "session_suspend",
            Self::SessionResume => "session_resume",
            Self::SessionTerminate => "session_terminate",
            Self::SessionDelete => "session_delete",
            Self::WorkspaceDelete => "workspace_delete",
            Self::TelemetryExport => "telemetry_export",
            Self::ContentGc => "content_gc",
        }
    }

    /// Parses the stable stored spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == text)
    }

    /// How the kind runs by default.
    #[must_use]
    pub const fn execution(self) -> Execution {
        Execution::Continued
    }

    /// Whether a caller may still cancel the operation the instant it is
    /// accepted.
    ///
    /// `TelemetryExport` is **not** cancelable, and that is a correction rather
    /// than a policy choice. This function used to say it was while
    /// `aex_session_dynamodb`'s `operation_cancel_owned` said it was not, so
    /// two shipped crates disagreed about a published capability. The store's
    /// reasoning is the correct one and is repeated here so the two cannot
    /// drift apart again: an export's effect fence is the observation export
    /// row, and treating an operation-row update as its cancellation would
    /// acknowledge a command that cannot stop the export launcher or the export
    /// task. The capability is not lost, only relocated to the verb that owns
    /// it — `telemetry_export_revoke`, already mounted, sets the row's
    /// `cancelRequested` flag that both the launcher's claim condition and the
    /// task's publication fence already honour.
    ///
    /// The export task moves the operation to `Running` in the same transaction
    /// that accepts its fenced generation lease. Fine-grained progress remains
    /// on `telemetry_export_get`; the operation status still truthfully says
    /// whether a task has crossed the authoritative acceptance fence.
    #[must_use]
    pub const fn cancelable_on_accept(self) -> bool {
        !matches!(
            self,
            Self::SessionCancel
                | Self::SessionSuspend
                | Self::SessionResume
                | Self::SessionTerminate
                | Self::SessionDelete
                | Self::WorkspaceDelete
                | Self::TelemetryExport
        )
    }

    /// Whether this kind belongs on the customer operation surface.
    ///
    /// Content GC is an internal maintenance continuation. Keeping that fact
    /// here, beside the kind vocabulary, lets point reads and indexes exclude it
    /// before a public decoder ever sees a value it cannot represent.
    #[must_use]
    pub const fn is_public(self) -> bool {
        !matches!(self, Self::ContentGc)
    }

    /// The generated customer vocabulary, or `None` for internal maintenance.
    #[must_use]
    pub const fn public(self) -> Option<models::OperationKind> {
        Some(match self {
            Self::SessionCancel => models::OperationKind::SessionCancel,
            Self::SessionSuspend => models::OperationKind::SessionSuspend,
            Self::SessionResume => models::OperationKind::SessionResume,
            Self::SessionTerminate => models::OperationKind::SessionTerminate,
            Self::SessionDelete => models::OperationKind::SessionDelete,
            Self::WorkspaceDelete => models::OperationKind::WorkspaceDelete,
            Self::TelemetryExport => models::OperationKind::TelemetryExport,
            Self::ContentGc => return None,
        })
    }

    /// Resolves one generated customer kind to the authority vocabulary.
    ///
    /// Internal `ContentGc` has no generated arm, so this direction is total
    /// without making the maintenance kind customer-addressable.
    #[must_use]
    pub const fn from_public(kind: models::OperationKind) -> Self {
        match kind {
            models::OperationKind::SessionCancel => Self::SessionCancel,
            models::OperationKind::SessionSuspend => Self::SessionSuspend,
            models::OperationKind::SessionResume => Self::SessionResume,
            models::OperationKind::SessionTerminate => Self::SessionTerminate,
            models::OperationKind::SessionDelete => Self::SessionDelete,
            models::OperationKind::WorkspaceDelete => Self::WorkspaceDelete,
            models::OperationKind::TelemetryExport => Self::TelemetryExport,
        }
    }

    /// Whether the kind claims the session's deletion guard.
    #[must_use]
    pub const fn claims_session_deletion(self) -> bool {
        matches!(self, Self::SessionDelete)
    }

    /// Whether the kind's result is content free and therefore survives a purge
    /// redaction intact.
    #[must_use]
    pub const fn result_survives_session_delete(self) -> bool {
        matches!(self, Self::SessionDelete)
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

    /// The stable stored spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Parses the stable stored spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|status| status.as_str() == text)
    }

    /// The generated customer vocabulary.
    #[must_use]
    pub const fn public(self) -> models::OperationStatus {
        match self {
            Self::Queued => models::OperationStatus::Queued,
            Self::Running => models::OperationStatus::Running,
            Self::Succeeded => models::OperationStatus::Succeeded,
            Self::Failed => models::OperationStatus::Failed,
            Self::Cancelled => models::OperationStatus::Cancelled,
        }
    }

    /// Resolves one generated customer status to the authority vocabulary.
    #[must_use]
    pub const fn from_public(status: models::OperationStatus) -> Self {
        match status {
            models::OperationStatus::Queued => Self::Queued,
            models::OperationStatus::Running => Self::Running,
            models::OperationStatus::Succeeded => Self::Succeeded,
            models::OperationStatus::Failed => Self::Failed,
            models::OperationStatus::Cancelled => Self::Cancelled,
        }
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

impl FailureClass {
    /// The stable stored spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Retryable => "retryable",
            Self::Terminal => "terminal",
            Self::PoisonManualReview => "poison_manual_review",
        }
    }

    /// Parses the stable stored spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "retryable" => Some(Self::Retryable),
            "terminal" => Some(Self::Terminal),
            "poison_manual_review" => Some(Self::PoisonManualReview),
            _ => None,
        }
    }
}

/// How far a continued operation has got.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Progress {
    /// The bounded customer-visible phase name.
    pub phase: String,
    /// How many units are done.
    pub processed: u64,
    /// How many units are expected, when that is known.
    pub total_hint: Option<u64>,
}

impl Progress {
    /// Whether `next` is a legal successor of `self`.
    ///
    /// # Errors
    ///
    /// Returns [`ProgressError`] when `processed` regresses or exceeds a
    /// declared total.
    pub fn check_successor(&self, next: &Self) -> Result<(), ProgressError> {
        next.validate()?;
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

    /// Checks the invariant the public progress object requires.
    ///
    /// # Errors
    ///
    /// [`ProgressError::InvalidPhase`] for an empty or overlong UTF-8 phase,
    /// and [`ProgressError::AboveTotal`] when completion exceeds the total.
    pub fn validate(&self) -> Result<(), ProgressError> {
        let bytes = self.phase.len();
        if bytes == 0 || bytes > 64 {
            return Err(ProgressError::InvalidPhase { bytes });
        }
        if let Some(total) = self.total_hint
            && self.processed > total
        {
            return Err(ProgressError::AboveTotal {
                processed: self.processed,
                total,
            });
        }
        Ok(())
    }
}

/// Why a progress report was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProgressError {
    /// The customer-visible phase is empty or exceeds the wire bound.
    #[error("operation progress phase is {bytes} bytes; expected 1..=64")]
    InvalidPhase {
        /// Encoded UTF-8 byte length.
        bytes: usize,
    },
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

    /// Decodes the stored canonical payload under the operation's own kind.
    ///
    /// The payload is stored without a second discriminant, so the envelope kind
    /// is the authority. A payload for another result shape is corruption, not
    /// an untyped value that can poison a whole public page.
    ///
    /// # Errors
    ///
    /// [`PublicProjectionError::Result`] when the canonical body does not match
    /// the exact generated result type for `kind`.
    pub fn public(
        &self,
        kind: OperationKind,
    ) -> Result<Option<models::OperationResult>, PublicProjectionError> {
        let Some(content) = self.content.as_ref() else {
            return Ok(None);
        };
        if !kind.is_public() {
            return Ok(None);
        }
        let value = content.to_value();
        macro_rules! payload {
            ($type:ty, $variant:ident) => {
                serde_json::from_value::<$type>(value)
                    .map(models::OperationResult::$variant)
                    .map_err(|error| PublicProjectionError::Result {
                        kind,
                        reason: error.to_string(),
                    })
                    .map(Some)
            };
        }
        match kind {
            OperationKind::SessionCancel => {
                payload!(models::SessionCancelResult, SessionCancel)
            }
            OperationKind::SessionSuspend => {
                payload!(models::SessionSuspendResult, SessionSuspend)
            }
            OperationKind::SessionResume => {
                payload!(models::SessionResumeResult, SessionResume)
            }
            OperationKind::SessionTerminate => {
                payload!(models::SessionTerminateResult, SessionTerminate)
            }
            OperationKind::SessionDelete => {
                payload!(models::SessionTombstone, SessionDelete)
            }
            OperationKind::WorkspaceDelete => {
                payload!(models::WorkspaceTombstone, WorkspaceDelete)
            }
            OperationKind::TelemetryExport => {
                payload!(models::TelemetryExportResult, TelemetryExport)
            }
            OperationKind::ContentGc => Ok(None),
        }
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

    /// The durable public failure, without inventing a request identity.
    #[must_use]
    pub fn public(&self) -> models::OperationFailure {
        models::OperationFailure {
            code: ObservedErrorCode::Known(self.code),
            retryable: self.class == FailureClass::Retryable,
            detail: self.detail.clone(),
        }
    }
}

/// Why a durable operation could not be projected to the generated wire.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PublicProjectionError {
    /// A canonical result body did not match the envelope kind.
    #[error("operation result for {kind:?} is malformed: {reason}")]
    Result {
        /// The authoritative envelope kind.
        kind: OperationKind,
        /// The bounded serde diagnostic.
        reason: String,
    },
}

/// The optimistic version of one stored operation row.
///
/// It lives in the domain rather than in an adapter because a **step commit**
/// has to name the version it observed. The public cancellation command runs
/// its own optimistic loop against the same row
/// (`aex-session-dynamodb`'s `operation_cancel_requested`), so a step that
/// guessed the version — or reset it to [`OperationVersion::FIRST`] — would let
/// two writers believe they held the row. A resumed step therefore conditions
/// on the exact version it read and advances it by one inside the same
/// transaction that advances the cursor.
///
/// This is the operation-row analogue of [`crate::lease::Fence`] and behaves the
/// same way: monotone, never reused, and overflow is a corrupted authority
/// rather than a customer condition.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OperationVersion(pub u64);

impl OperationVersion {
    /// The version a freshly admitted operation row is born at.
    ///
    /// One rather than zero, so that a stored version is never confused with
    /// the absence of the attribute.
    pub const FIRST: Self = Self(1);

    /// The version a write that observed this one advances to.
    ///
    /// # Panics
    ///
    /// Panics on `u64` overflow. Reaching it needs more writes to a single
    /// operation row than the row can physically have received, so it is a
    /// corrupted authority and not a condition a caller can provoke.
    #[must_use]
    pub const fn next(self) -> Self {
        match self.0.checked_add(1) {
            Some(value) => Self(value),
            None => panic!("operation version overflowed"),
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

    /// Projects one customer-visible operation through generated types.
    ///
    /// `ContentGc` returns `Ok(None)`: it is an internal continuation and must
    /// be excluded by the sparse public index as well as by point reads.
    ///
    /// # Errors
    ///
    /// [`PublicProjectionError`] when a persisted result body disagrees with
    /// the authoritative operation kind.
    pub fn public(&self) -> Result<Option<models::Operation>, PublicProjectionError> {
        let Some(kind) = self.kind.public() else {
            return Ok(None);
        };
        let result = self
            .result
            .as_ref()
            .map(|value| value.public(self.kind))
            .transpose()?
            .flatten();
        let progress = self
            .progress
            .as_ref()
            .map(|value| models::OperationProgress {
                phase: value.phase.clone(),
                completed: Some(aex_wire::types::DecimalU128::new(u128::from(
                    value.processed,
                ))),
                total: value
                    .total_hint
                    .map(|total| aex_wire::types::DecimalU128::new(u128::from(total))),
            });
        Ok(Some(models::Operation {
            cancelable: self.cancelable(),
            committed_at: self.committed_at,
            created_at: self.created_at,
            error: self.error.as_ref().map(OperationFailure::public),
            id: self.id,
            kind,
            progress,
            result,
            session_id: self.session,
            started_at: self.started_at,
            status: self.status.public(),
            terminal_at: self.terminal_at,
            updated_at: self.updated_at,
            workspace_id: self.workspace,
        }))
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
    if let Some(current) = operation.progress.as_ref() {
        current.check_successor(&reported)?;
    } else {
        reported.validate()?;
    }
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

/// Terminalizes a queued telemetry export when its dedicated export revoke
/// fence wins.
///
/// Generic operation cancellation deliberately refuses telemetry exports: the
/// operation row cannot stop the launcher or task. The dedicated revoke route
/// owns the actual effect fence in `observation-authority`; after it has won,
/// this transition lets the same transaction record the matching canonical
/// operation outcome.
///
/// # Errors
///
/// Returns [`CancelRejection::NotCancelable`] for any other operation kind or
/// a committed export, and [`CancelRejection::AlreadyTerminal`] for a terminal
/// outcome other than an earlier cancellation.
pub fn revoke_telemetry_export(
    operation: &Operation,
    now: Timestamp,
) -> Result<OperationCommit, CancelRejection> {
    if operation.status.is_terminal() {
        if operation.status == OperationStatus::Cancelled {
            return Ok(OperationCommit::of(operation.clone(), false));
        }
        return Err(CancelRejection::AlreadyTerminal(operation.status));
    }
    if operation.kind != OperationKind::TelemetryExport || operation.committed_at.is_some() {
        return Err(CancelRejection::NotCancelable {
            kind: operation.kind,
            committed: operation.committed_at.is_some(),
        });
    }
    let mut next = operation.clone();
    next.cancel_requested = true;
    next.status = OperationStatus::Cancelled;
    next.updated_at = now;
    next.terminal_at = Some(now);
    Ok(OperationCommit::of(next, false))
}

#[cfg(test)]
mod tests {
    use aex_wire::CanonicalJson;
    use aex_wire::error::ErrorCode;
    use aex_wire::idempotency::IntentDigest;
    use aex_wire::ids::{OperationId, PrefixedId as _, SessionId, Uuid7, WorkspaceId};
    use aex_wire::models;
    use aex_wire::types::Timestamp;

    use super::{
        CancelRejection, Execution, FailureClass, Operation, OperationFailure, OperationKind,
        OperationResult, OperationScope, OperationStatus, Progress, TransitionError, cancel,
        commit_point, fail, start, succeed,
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
    fn every_kind_is_continued_and_maps_to_one_stable_spelling() {
        for kind in OperationKind::ALL {
            assert_eq!(kind.execution(), Execution::Continued);
            assert_eq!(OperationKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(OperationKind::WorkspaceDelete.as_str(), "workspace_delete");
        assert!(OperationKind::WorkspaceDelete.is_public());
        assert!(!OperationKind::ContentGc.is_public());
    }

    #[test]
    fn the_commit_latch_closes_cancel_and_fail() {
        let queued = operation(OperationKind::ContentGc);
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
                kind: OperationKind::ContentGc,
                committed: true,
            })
        );
    }

    #[test]
    fn succeed_from_queued_back_fills_the_latch() {
        let queued = operation(OperationKind::SessionTerminate);
        let done = succeed(&queued, OperationResult::receipt(), moment(5)).expect("succeeds");
        assert!(done.latched_commit);
        assert_eq!(done.operation.committed_at, Some(moment(5)));
        assert_eq!(done.operation.status, OperationStatus::Succeeded);
    }

    #[test]
    fn a_queued_cancel_terminalizes_and_a_running_one_records() {
        let queued = operation(OperationKind::ContentGc);
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
    fn the_dedicated_export_revoke_terminalizes_queued_and_running_exports() {
        let queued = operation(OperationKind::TelemetryExport);
        let running = start(&queued, moment(2)).expect("starts").operation;
        for export in [queued, running] {
            let revoked = super::revoke_telemetry_export(&export, moment(5))
                .expect("the effect-owning revoke accepts the export")
                .operation;
            assert_eq!(revoked.status, OperationStatus::Cancelled);
            assert!(revoked.cancel_requested);
            assert_eq!(revoked.terminal_at, Some(moment(5)));
            assert_eq!(
                super::revoke_telemetry_export(&revoked, moment(6))
                    .expect("an exact revoke replay is idempotent")
                    .operation,
                revoked
            );
        }
    }

    #[test]
    fn a_non_cancelable_kind_refuses_from_acceptance() {
        for kind in [
            OperationKind::SessionCancel,
            OperationKind::SessionSuspend,
            OperationKind::SessionResume,
            OperationKind::SessionTerminate,
            OperationKind::SessionDelete,
            OperationKind::WorkspaceDelete,
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
                phase: "copying".to_owned(),
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
                    phase: "copying".to_owned(),
                    processed: 9,
                    total_hint: Some(100)
                },
                moment(3)
            )
            .is_err()
        );
    }

    #[test]
    fn public_projection_uses_the_envelope_kind_and_keeps_the_phase() {
        let session = SessionId::from_uuid7(Uuid7::compose(1, [3; 10]));
        let payload = models::SessionCancelResult {
            changed: true,
            session_id: session,
            session_revision: 9,
        };
        let mut stored = operation(OperationKind::SessionCancel);
        stored.session = Some(session);
        stored.scope = OperationScope::Session(session);
        stored.status = OperationStatus::Succeeded;
        stored.progress = Some(Progress {
            phase: "stopping".to_owned(),
            processed: 1,
            total_hint: Some(1),
        });
        stored.result = Some(OperationResult {
            measurement: None,
            content: Some(
                CanonicalJson::from_value(&serde_json::to_value(payload).expect("json"))
                    .expect("canonical"),
            ),
        });

        let public = stored.public().expect("projects").expect("public kind");
        assert_eq!(public.progress.expect("progress").phase, "stopping");
        assert!(matches!(
            public.result,
            Some(models::OperationResult::SessionCancel(result))
                if result.session_id == session && result.session_revision == 9
        ));
    }

    #[test]
    fn internal_or_malformed_results_cannot_poison_the_public_surface() {
        assert!(
            operation(OperationKind::ContentGc)
                .public()
                .expect("internal exclusion is not an error")
                .is_none()
        );

        let mut malformed = operation(OperationKind::SessionCancel);
        malformed.result = Some(OperationResult {
            measurement: None,
            content: Some(CanonicalJson::parse(r#"{"wrong":true}"#).expect("canonical")),
        });
        assert!(malformed.public().is_err());
    }
}
