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
    pub const ALL: [Self; 11] = [
        Self::SessionStop,
        Self::SessionPersist,
        Self::SessionClone,
        Self::WorkspaceDiscard,
        Self::CredentialRebind,
        Self::SessionTrash,
        Self::SessionRestore,
        Self::SessionPurge,
        Self::WorkspaceDelete,
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
            Self::SessionPurge
            | Self::WorkspaceDelete
            | Self::TelemetryExport
            | Self::ContentGc => Execution::Continued,
        }
    }

    /// Whether a caller may still cancel the operation the instant it is
    /// accepted.
    #[must_use]
    pub const fn cancelable_on_accept(self) -> bool {
        !matches!(
            self,
            Self::SessionStop | Self::SessionTrash | Self::SessionPurge | Self::WorkspaceDelete
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
            Self::SessionStop => models::OperationKind::SessionStop,
            Self::SessionPersist => models::OperationKind::SessionPersist,
            Self::SessionClone => models::OperationKind::SessionClone,
            Self::WorkspaceDiscard => models::OperationKind::WorkspaceDiscard,
            Self::CredentialRebind => models::OperationKind::CredentialRebind,
            Self::SessionTrash => models::OperationKind::SessionTrash,
            Self::SessionRestore => models::OperationKind::SessionRestore,
            Self::SessionPurge => models::OperationKind::SessionPurge,
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
            models::OperationKind::SessionStop => Self::SessionStop,
            models::OperationKind::SessionPersist => Self::SessionPersist,
            models::OperationKind::SessionClone => Self::SessionClone,
            models::OperationKind::WorkspaceDiscard => Self::WorkspaceDiscard,
            models::OperationKind::CredentialRebind => Self::CredentialRebind,
            models::OperationKind::SessionTrash => Self::SessionTrash,
            models::OperationKind::SessionRestore => Self::SessionRestore,
            models::OperationKind::SessionPurge => Self::SessionPurge,
            models::OperationKind::WorkspaceDelete => Self::WorkspaceDelete,
            models::OperationKind::TelemetryExport => Self::TelemetryExport,
        }
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
            OperationKind::SessionStop => payload!(models::SessionStopResult, SessionStop),
            OperationKind::SessionPersist => {
                payload!(models::SessionPersistResult, SessionPersist)
            }
            OperationKind::SessionClone => payload!(models::SessionCloneResult, SessionClone),
            OperationKind::WorkspaceDiscard => {
                payload!(models::WorkspaceDiscardResult, WorkspaceDiscard)
            }
            OperationKind::CredentialRebind => {
                payload!(models::CredentialRebindResult, CredentialRebind)
            }
            OperationKind::SessionTrash => payload!(models::SessionTrashResult, SessionTrash),
            OperationKind::SessionRestore => {
                payload!(models::SessionRestoreResult, SessionRestore)
            }
            OperationKind::SessionPurge => payload!(models::SessionTombstone, SessionPurge),
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

#[cfg(test)]
mod tests {
    use aex_wire::CanonicalJson;
    use aex_wire::error::ErrorCode;
    use aex_wire::idempotency::IntentDigest;
    use aex_wire::ids::{OperationId, PrefixedId as _, SessionId, Uuid7, WorkspaceId};
    use aex_wire::models;
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
        assert_eq!(OperationKind::WorkspaceDelete.as_str(), "workspace_delete");
        assert!(OperationKind::WorkspaceDelete.is_public());
        assert!(!OperationKind::ContentGc.is_public());
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
        let payload = models::SessionStopResult {
            changed: true,
            session_id: session,
            session_revision: 9,
        };
        let mut stored = operation(OperationKind::SessionStop);
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
            Some(models::OperationResult::SessionStop(result))
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

        let mut malformed = operation(OperationKind::SessionStop);
        malformed.result = Some(OperationResult {
            measurement: None,
            content: Some(CanonicalJson::parse(r#"{"wrong":true}"#).expect("canonical")),
        });
        assert!(malformed.public().is_err());
    }
}
