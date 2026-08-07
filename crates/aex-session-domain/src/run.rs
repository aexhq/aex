//! Runs.
//!
//! `Queued -> Running -> {Succeeded|Failed|TimedOut|Cancelled|Interrupted}`,
//! plus the legal shortcut `Queued -> {Cancelled|Interrupted}`: a run stopped or
//! fenced before it starts never passes through `Running`. `start` on a running
//! run is an idempotent no-op that emits no `run.started` fact, and no
//! terminal-to-terminal transition exists at all.

use std::num::NonZeroU64;

use aex_operation_domain::DeletionState;
use aex_wire::CanonicalJson;
use aex_wire::ids::{GenerationId, MessageId, OperationId, RunId, SessionId, TelemetryGapId};
use aex_wire::types::Timestamp;

use crate::ids::{CancellationEpoch, EffectId, ReservationId};
use crate::session::{Session, WorkAdmission};

// Where a run is. The terminal outbox event carries it across a process
// boundary, so it is owned by the contract crate both readers of that event
// depend on and re-exported here for every existing call site.
pub use aex_internal_contracts::outbox::RunStatus;

/// Why the platform fenced a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptReason {
    /// A stop operation asked for it.
    StopRequested {
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
    /// The spend reservation ran out.
    SpendExhausted,
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
    /// The reservation backing that ceiling.
    pub reservation: ReservationId,
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
    /// The reservation backing it.
    pub reservation: ReservationId,
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
            reservation: command.reservation,
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

    use aex_wire::ids::{MessageId, PrefixedId as _, RunId, Uuid7};
    use aex_wire::types::Timestamp;

    use super::{QueueRun, RunStatus, SessionDomainRunError, queue, start};
    use crate::ids::ReservationId;
    use crate::session::WorkAdmission;
    use crate::testing::session_fixture;

    fn moment(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("in range")
    }

    fn command() -> QueueRun {
        QueueRun {
            message: MessageId::from_uuid7(Uuid7::compose(1, [4; 10])),
            max_spend_cents: NonZeroU64::new(500).expect("non-zero"),
            reservation: ReservationId(Uuid7::compose(1, [5; 10])),
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
}
