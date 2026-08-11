//! Runs.
//!
//! `Queued -> Running -> {Succeeded|Failed|TimedOut|Cancelled|Interrupted}`,
//! plus the legal shortcut `Queued -> {Cancelled|Interrupted}`: a run stopped or
//! fenced before it starts never passes through `Running`. `start` on a running
//! run is an idempotent no-op that emits no `run.started` fact, and no
//! terminal-to-terminal transition exists at all.

use std::num::NonZeroU64;

use aex_internal_contracts::RunId;
use aex_operation_domain::DeletionState;
use aex_wire::CanonicalJson;
use aex_wire::ids::{GenerationId, MessageId, OperationId, SessionId, TelemetryGapId};
use aex_wire::types::Timestamp;

use crate::ids::{CancellationEpoch, EffectId};
use crate::session::{Session, WorkAdmission};

/// The exact public current-message spend default, in whole cents.
pub const DEFAULT_MESSAGE_MAX_SPEND_CENTS: NonZeroU64 =
    NonZeroU64::new(1_000).expect("the launch default is positive");

/// The resolved caller-controlled bounds for one session message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedMessageBounds {
    /// The caller's explicit positive value, or exactly 1000 when omitted.
    pub max_spend_cents: NonZeroU64,
    /// The caller's earlier deadline, or the earlier run/session fence.
    pub deadline: Timestamp,
}

/// Why public message bounds were refused before an internal run was minted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MessageBoundsError {
    /// Explicit zero is never treated as omission.
    #[error("maxSpendCents must be positive when provided")]
    ZeroSpend,
    /// No message may be admitted without remaining execution time.
    #[error("the effective message deadline is not in the future")]
    DeadlineNotInFuture,
    /// A caller may shorten, but never extend, the session's fence.
    #[error("the requested deadline exceeds the session lifetime or drain fence")]
    DeadlineAfterSessionFence,
}

/// Resolves the public optional message bounds without reserving account funds.
///
/// `session_fence` is the earlier of the immutable provider `expiresAt` and any
/// internal drain deadline. Omission uses the earlier of that fence and the
/// revisioned `max_run_duration_ms` fence. An explicit deadline may only
/// shorten it; an explicit positive spend cap is preserved even when it is
/// above the launch default.
///
/// # Errors
///
/// Returns [`MessageBoundsError`] for explicit zero, an elapsed fence/deadline,
/// or a caller deadline later than the session fence.
pub fn resolve_message_bounds(
    requested_spend_cents: Option<u64>,
    requested_deadline: Option<Timestamp>,
    now: Timestamp,
    session_fence: Timestamp,
    max_run_duration_ms: u64,
) -> Result<ResolvedMessageBounds, MessageBoundsError> {
    let max_spend_cents = match requested_spend_cents {
        Some(value) => NonZeroU64::new(value).ok_or(MessageBoundsError::ZeroSpend)?,
        None => DEFAULT_MESSAGE_MAX_SPEND_CENTS,
    };
    let duration = i64::try_from(max_run_duration_ms).unwrap_or(i64::MAX);
    let run_fence = Timestamp::from_unix_millis(now.unix_millis().saturating_add(duration))
        .unwrap_or(session_fence);
    let effective_fence = session_fence.min(run_fence);
    let deadline = requested_deadline.unwrap_or(effective_fence);
    if deadline > effective_fence {
        return Err(MessageBoundsError::DeadlineAfterSessionFence);
    }
    if deadline <= now {
        return Err(MessageBoundsError::DeadlineNotInFuture);
    }
    Ok(ResolvedMessageBounds {
        max_spend_cents,
        deadline,
    })
}

// Where a run is. The terminal outbox event carries it across a process
// boundary, so it is owned by the contract crate both readers of that event
// depend on and re-exported here for every existing call site.
pub use aex_internal_contracts::outbox::RunStatus;

/// Why the platform fenced a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptReason {
    /// A public session cancellation asked for it.
    SessionCancel {
        /// Which operation.
        operation: OperationId,
    },
    /// The account is paused.
    AccountPaused,
    /// Workspace authorization was revoked.
    AuthorizationRevoked,
    /// The workspace generation is gone.
    ContinuityLost {
        /// The generation that vanished, when one was named.
        generation: Option<GenerationId>,
    },
    /// The run-local spend ceiling was exhausted.
    SpendCapExhausted,
    /// An effect's outcome could not be determined.
    AmbiguousEffect {
        /// Which effect.
        effect: EffectId,
    },
}

/// A domain-level failure a run reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainError {
    /// The stable public code.
    pub code: aex_wire::error::ErrorCode,
    /// A human-readable summary. Never carries a secret or a body.
    pub message: String,
    /// Typed customer-safe detail, when the domain produced it.
    pub detail: Option<CanonicalJson>,
    /// Whether retrying the same run step is permitted.
    pub retryable: bool,
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    /// It finished normally.
    Succeeded {
        /// The messages it produced.
        output_messages: Vec<MessageId>,
    },
    /// It failed.
    Failed {
        /// Why.
        error: DomainError,
    },
    /// It ran past its deadline.
    TimedOut {
        /// The deadline it passed.
        deadline: Timestamp,
    },
    /// An operation cancelled it.
    Cancelled {
        /// Which operation.
        by: OperationId,
    },
    /// The platform fenced it.
    Interrupted(InterruptReason),
}

impl RunOutcome {
    /// The status this outcome settles the run at.
    #[must_use]
    pub const fn status(&self) -> RunStatus {
        match self {
            Self::Succeeded { .. } => RunStatus::Succeeded,
            Self::Failed { .. } => RunStatus::Failed,
            Self::TimedOut { .. } => RunStatus::TimedOut,
            Self::Cancelled { .. } => RunStatus::Cancelled,
            Self::Interrupted(_) => RunStatus::Interrupted,
        }
    }

    /// Whether the outcome carries a typed non-success reason.
    #[must_use]
    pub const fn has_typed_reason(&self) -> bool {
        !matches!(self, Self::Succeeded { .. })
    }
}

/// One admitted turn of execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    /// Its identity.
    pub id: RunId,
    /// The owning session.
    pub session: SessionId,
    /// The message that admitted it.
    pub message: MessageId,
    /// Where it is.
    pub status: RunStatus,
    /// Its spend ceiling.
    pub max_spend_cents: NonZeroU64,
    /// When it must stop.
    pub deadline: Timestamp,
    /// The cancellation epoch it was admitted at.
    pub cancellation_at_admission: CancellationEpoch,
    /// When it was admitted.
    pub queued_at: Timestamp,
    /// When it started.
    pub started_at: Option<Timestamp>,
    /// When it settled.
    pub terminal_at: Option<Timestamp>,
    /// How it ended.
    pub outcome: Option<RunOutcome>,
    /// Whether the observation authority has proved that no telemetry gap
    /// belongs to this run. `None` means that authority has not settled yet.
    pub telemetry_complete: Option<bool>,
    /// The gaps the observation authority has attached to this run. `None`
    /// means the observation projection has not settled yet; an empty vector is
    /// an explicit settled observation.
    pub telemetry_gaps: Option<Vec<TelemetryGapId>>,
}

/// What a run transition changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunCommit {
    /// The run after the transition.
    pub run: Run,
    /// Whether the transition changed anything. `false` marks the idempotent
    /// `start`-on-running case, which must emit no `run.started` fact.
    pub changed: bool,
}

/// What a caller asks for when queueing a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueRun {
    /// The message that admits it.
    pub message: MessageId,
    /// Its spend ceiling.
    pub max_spend_cents: NonZeroU64,
    /// When it must stop.
    pub deadline: Timestamp,
}

/// Why a run transition was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SessionDomainRunError {
    /// The run is already terminal.
    #[error("run is already {0:?}")]
    AlreadyTerminal(RunStatus),
    /// Another run owns the session.
    #[error("session already has an active run")]
    SessionBusy {
        /// The owning run.
        active: RunId,
    },
    /// The session is not admitting work.
    #[error("session admission is {0:?}")]
    AdmissionClosed(WorkAdmission),
    /// The session is being or has been deleted.
    #[error("session deletion state is {0:?}")]
    Deleted(DeletionState),
    /// A whole-session command holds the exclusion.
    #[error("session is held by operation {holder}")]
    MutationGuardHeld {
        /// The holder.
        holder: OperationId,
    },
    /// The deadline was not in the future.
    #[error("run deadline must be after the admission instant")]
    DeadlineNotInFuture,
}

/// Queues a run against a session.
///
/// # Errors
///
/// Returns [`SessionDomainRunError`] when the session already has an active run, is not
/// admitting work, is not `Live`, is held by a whole-session command, or when the
/// deadline is not in the future.
pub fn queue(
    id: RunId,
    command: &QueueRun,
    session: &Session,
    now: Timestamp,
) -> Result<RunCommit, SessionDomainRunError> {
    if session.deletion.state != DeletionState::Live {
        return Err(SessionDomainRunError::Deleted(session.deletion.state));
    }
    if !session.work_admission.is_open() {
        return Err(SessionDomainRunError::AdmissionClosed(
            session.work_admission,
        ));
    }
    if let Some(guard) = session.mutation_guard {
        return Err(SessionDomainRunError::MutationGuardHeld {
            holder: guard.holder,
        });
    }
    if let Some(active) = session.active_run {
        return Err(SessionDomainRunError::SessionBusy { active });
    }
    if command.deadline.unix_millis() <= now.unix_millis() {
        return Err(SessionDomainRunError::DeadlineNotInFuture);
    }
    Ok(RunCommit {
        run: Run {
            id,
            session: session.id,
            message: command.message,
            status: RunStatus::Queued,
            max_spend_cents: command.max_spend_cents,
            deadline: command.deadline,
            cancellation_at_admission: session.cancellation,
            queued_at: now,
            started_at: None,
            terminal_at: None,
            outcome: None,
            telemetry_complete: None,
            telemetry_gaps: None,
        },
        changed: true,
    })
}

/// Starts a queued run.
///
/// Starting an already running run is an idempotent no-op reporting
/// `changed = false`; starting a terminal run is an error, because there is no
/// terminal-to-terminal or terminal-to-running transition.
///
/// # Errors
///
/// Returns [`SessionDomainRunError::AlreadyTerminal`] for a settled run and the session
/// guards otherwise.
pub fn start(
    run: &Run,
    session: &Session,
    now: Timestamp,
) -> Result<RunCommit, SessionDomainRunError> {
    if run.status.is_terminal() {
        return Err(SessionDomainRunError::AlreadyTerminal(run.status));
    }
    if run.status == RunStatus::Running {
        return Ok(RunCommit {
            run: run.clone(),
            changed: false,
        });
    }
    if session.deletion.state != DeletionState::Live {
        return Err(SessionDomainRunError::Deleted(session.deletion.state));
    }
    if !session.work_admission.is_open() {
        return Err(SessionDomainRunError::AdmissionClosed(
            session.work_admission,
        ));
    }
    let mut next = run.clone();
    next.status = RunStatus::Running;
    next.started_at = Some(now);
    Ok(RunCommit {
        run: next,
        changed: true,
    })
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use aex_internal_contracts::RunId;
    use aex_wire::ids::{MessageId, PrefixedId as _, Uuid7};
    use aex_wire::types::Timestamp;

    use super::{
        DEFAULT_MESSAGE_MAX_SPEND_CENTS, MessageBoundsError, QueueRun, RunStatus,
        SessionDomainRunError, queue, resolve_message_bounds, start,
    };
    use crate::session::WorkAdmission;
    use crate::testing::session_fixture;

    fn moment(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("in range")
    }

    fn command() -> QueueRun {
        QueueRun {
            message: MessageId::from_uuid7(Uuid7::compose(1, [4; 10])),
            max_spend_cents: NonZeroU64::new(500).expect("non-zero"),
            deadline: moment(60_000),
        }
    }

    fn run_id() -> RunId {
        RunId::from_uuid7(Uuid7::compose(1, [6; 10]))
    }

    #[test]
    fn a_queued_run_records_the_admission_epoch() {
        let session = session_fixture();
        let commit = queue(run_id(), &command(), &session, moment(0)).expect("queues");
        assert_eq!(commit.run.status, RunStatus::Queued);
        assert_eq!(commit.run.cancellation_at_admission, session.cancellation);
    }

    #[test]
    fn a_closed_admission_refuses_a_run() {
        let mut session = session_fixture();
        session.work_admission = WorkAdmission::Paused;
        assert_eq!(
            queue(run_id(), &command(), &session, moment(0)),
            Err(SessionDomainRunError::AdmissionClosed(
                WorkAdmission::Paused
            ))
        );
    }

    #[test]
    fn starting_a_running_run_changes_nothing() {
        let session = session_fixture();
        let queued = queue(run_id(), &command(), &session, moment(0))
            .expect("queues")
            .run;
        let running = start(&queued, &session, moment(1)).expect("starts");
        assert!(running.changed);
        let again = start(&running.run, &session, moment(2)).expect("idempotent");
        assert!(!again.changed);
        assert_eq!(again.run.started_at, Some(moment(1)));
    }

    #[test]
    fn a_deadline_in_the_past_is_refused() {
        let session = session_fixture();
        let mut command = command();
        command.deadline = moment(0);
        assert_eq!(
            queue(run_id(), &command, &session, moment(0)),
            Err(SessionDomainRunError::DeadlineNotInFuture)
        );
    }

    #[test]
    fn omitted_public_bounds_resolve_to_exact_default_and_session_fence() {
        let resolved = resolve_message_bounds(None, None, moment(1_000), moment(60_000), 59_000)
            .expect("the session has remaining time");
        assert_eq!(resolved.max_spend_cents, DEFAULT_MESSAGE_MAX_SPEND_CENTS);
        assert_eq!(resolved.max_spend_cents.get(), 1_000);
        assert_eq!(resolved.deadline, moment(60_000));
    }

    #[test]
    fn omitted_deadline_is_capped_by_the_revisioned_run_duration() {
        let resolved =
            resolve_message_bounds(None, None, moment(1_000), moment(28_800_000), 3_600_000)
                .expect("the one-hour run fence is still in the future");
        assert_eq!(resolved.deadline, moment(3_601_000));
        assert_eq!(
            resolve_message_bounds(
                None,
                Some(moment(3_601_001)),
                moment(1_000),
                moment(28_800_000),
                3_600_000,
            ),
            Err(MessageBoundsError::DeadlineAfterSessionFence)
        );
    }

    #[test]
    fn caller_may_raise_or_lower_spend_and_only_shorten_the_deadline() {
        for cents in [1, 999, 1_001, 50_000] {
            let resolved = resolve_message_bounds(
                Some(cents),
                Some(moment(30_000)),
                moment(1_000),
                moment(60_000),
                59_000,
            )
            .expect("positive spend and earlier deadline are caller-controlled");
            assert_eq!(resolved.max_spend_cents.get(), cents);
            assert_eq!(resolved.deadline, moment(30_000));
        }

        assert_eq!(
            resolve_message_bounds(Some(0), None, moment(1_000), moment(60_000), 59_000),
            Err(MessageBoundsError::ZeroSpend)
        );
        assert_eq!(
            resolve_message_bounds(
                Some(2_000),
                Some(moment(60_001)),
                moment(1_000),
                moment(60_000),
                59_000,
            ),
            Err(MessageBoundsError::DeadlineAfterSessionFence)
        );
    }
}
