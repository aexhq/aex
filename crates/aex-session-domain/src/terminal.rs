//! The run terminal barrier.
//!
//! Exactly one writer settles a run. The winner's commit contains — and only
//! contains — the terminal run status and outcome, the seal of every open
//! message of that run, the final agent control and journal tail, the session
//! advance with `active_run` cleared, the terminal outbox event, the immutable
//! usage-closure identity.
//!
//! A loser returns [`TerminalRejection::AlreadyTerminal`] carrying the winning
//! outcome and produces **no writes at all** — not even a lifecycle fact.
//! Observation projection, central settlement and stream delivery are
//! after-commit hints and can never gate the barrier.

use aex_internal_contracts::RunId;
use aex_operation_domain::DeletionState;
use aex_wire::ids::{AgentId, MessageId};
use aex_wire::types::Timestamp;

use crate::ids::{AgentFence, CancellationEpoch, SessionRevision, UsageClosureId};
use crate::message::{Message, MessageDelta, seal};
use crate::run::{Run, RunOutcome};
use crate::session::{Session, SessionStatus};

/// Largest number of open messages one run may carry into its terminal
/// barrier.
///
/// The application commits four fixed rows plus both the mutable base row and
/// immutable sealed projection for each message. Forty-eight therefore fills,
/// but never exceeds, `DynamoDB`'s 100-action transaction envelope.
pub const MAX_OPEN_MESSAGES_PER_RUN: usize = 48;

/// One attempt to settle a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalAttempt {
    /// Which run.
    pub run: RunId,
    /// How it ended.
    pub outcome: RunOutcome,
    /// When.
    pub at: Timestamp,
    /// The session revision the attempt was built against.
    pub session_revision_seen: SessionRevision,
    /// The cancellation epoch the attempt was built against.
    pub cancellation_seen: CancellationEpoch,
    /// The agent fence the attempt presents.
    pub agent_fence: AgentFence,
    /// The immutable usage-closure identity.
    pub usage_closure: UsageClosureId,
}

// The one durable notification the barrier writes, and nothing outside the
// transaction can prevent it. `regional-stream` and the observation materializer
// both decode it, so the envelope itself lives in `aex-internal-contracts` and
// this crate builds it rather than declaring it.
pub use aex_internal_contracts::outbox::OutboxEvent;

/// Everything the winning terminal commit contains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalCommit {
    /// The run, settled.
    pub run: Run,
    /// Every open message of that run, sealed.
    pub sealed_messages: Vec<Message>,
    /// The session head, advanced with `active_run` cleared.
    pub session: Session,
    /// The final agent fence the barrier observed.
    pub agent_fence: AgentFence,
    /// The terminal outbox event.
    pub outbox: OutboxEvent,
    /// The immutable usage-closure identity.
    pub usage_closure: UsageClosureId,
}

/// Why a terminal attempt lost.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TerminalRejection {
    /// The producer violated the bounded open-message invariant.
    #[error("run has {seen} open messages, above the maximum of {max}")]
    TooManyOpenMessages {
        /// What the terminal reader observed.
        seen: usize,
        /// The invariant ceiling.
        max: usize,
    },
    /// Someone else settled the run first.
    #[error("run is already terminal")]
    AlreadyTerminal {
        /// The winning outcome.
        winner: RunOutcome,
    },
    /// The run does not own the session.
    #[error("run does not own the session")]
    NotSessionOwner {
        /// The run that does, when one does.
        active: Option<RunId>,
    },
    /// The session moved under the attempt.
    #[error("expected session revision {expected} but saw {seen}")]
    StaleSessionRevision {
        /// The session's revision.
        expected: SessionRevision,
        /// What the attempt saw.
        seen: SessionRevision,
    },
    /// A cancellation happened under the attempt.
    #[error("cancellation epoch is {current} but the attempt saw {seen}")]
    StaleCancellationEpoch {
        /// The session's epoch.
        current: CancellationEpoch,
        /// What the attempt saw.
        seen: CancellationEpoch,
    },
    /// The agent was re-claimed under the attempt.
    #[error("agent fence is {current} but the attempt presented {presented}")]
    StaleAgentFence {
        /// The agent's fence.
        current: AgentFence,
        /// What the attempt presented.
        presented: AgentFence,
    },
    /// The session is past the irreversible deletion fence.
    #[error("session is deleting")]
    SessionDeleting,
}

/// Settles a run, or reports why this attempt is not the winner.
///
/// # Errors
///
/// Returns the [`TerminalRejection`] naming the first failing condition. Every
/// rejection produces no commit at all.
pub fn claim_terminal(
    run: &Run,
    session: &Session,
    agent: AgentId,
    current_fence: AgentFence,
    open_messages: &[Message],
    attempt: &TerminalAttempt,
) -> Result<TerminalCommit, TerminalRejection> {
    if open_messages.len() > MAX_OPEN_MESSAGES_PER_RUN {
        return Err(TerminalRejection::TooManyOpenMessages {
            seen: open_messages.len(),
            max: MAX_OPEN_MESSAGES_PER_RUN,
        });
    }
    if run.status.is_terminal() {
        return Err(TerminalRejection::AlreadyTerminal {
            winner: run
                .outcome
                .clone()
                .unwrap_or_else(|| unreachable!("a terminal run always records its outcome")),
        });
    }
    if session.deletion.state != DeletionState::Live {
        return Err(TerminalRejection::SessionDeleting);
    }
    if session.active_run != Some(run.id) {
        return Err(TerminalRejection::NotSessionOwner {
            active: session.active_run,
        });
    }
    if session.revision != attempt.session_revision_seen {
        return Err(TerminalRejection::StaleSessionRevision {
            expected: session.revision,
            seen: attempt.session_revision_seen,
        });
    }
    if session.cancellation != attempt.cancellation_seen {
        return Err(TerminalRejection::StaleCancellationEpoch {
            current: session.cancellation,
            seen: attempt.cancellation_seen,
        });
    }
    if current_fence != attempt.agent_fence {
        return Err(TerminalRejection::StaleAgentFence {
            current: current_fence,
            presented: attempt.agent_fence,
        });
    }

    let mut settled = run.clone();
    settled.status = attempt.outcome.status();
    settled.outcome = Some(attempt.outcome.clone());
    settled.terminal_at = Some(attempt.at);

    let sealed_messages: Vec<Message> = open_messages
        .iter()
        .filter(|message| message.run == Some(run.id) && message.agent == agent)
        .map(|message| {
            let MessageDelta { message, .. } = seal(message, attempt.at);
            message
        })
        .collect();

    let mut head = session.clone();
    head.status = SessionStatus::Idle;
    head.active_run = None;
    head.revision = session.revision.next();
    head.updated_at = attempt.at;

    Ok(TerminalCommit {
        run: settled,
        sealed_messages,
        agent_fence: current_fence,
        outbox: OutboxEvent {
            schema_version: aex_internal_contracts::SchemaVersion::V1,
            session: session.id,
            run: run.id,
            status: attempt.outcome.status(),
            session_revision: head.revision,
            usage_closure: attempt.usage_closure,
            at: attempt.at,
        },
        session: head,
        usage_closure: attempt.usage_closure,
    })
}

/// The identity of every message the barrier sealed, for a caller that only
/// needs the ids.
#[must_use]
pub fn sealed_ids(commit: &TerminalCommit) -> Vec<MessageId> {
    commit
        .sealed_messages
        .iter()
        .map(|message| message.id)
        .collect()
}

#[cfg(test)]
mod tests {
    use aex_internal_contracts::RunId;
    use aex_wire::ids::Uuid7;

    use super::{MAX_OPEN_MESSAGES_PER_RUN, TerminalRejection, claim_terminal};
    use crate::ids::{AgentFence, CancellationEpoch, SessionRevision};
    use crate::run::RunStatus;
    use crate::session::SessionStatus;
    use crate::testing::{running_session, terminal_attempt};

    #[test]
    fn the_winner_carries_every_required_part() {
        let (session, run, agent, message) = running_session();
        let attempt = terminal_attempt(&session, &run);
        let commit = claim_terminal(
            &run,
            &session,
            agent.id,
            AgentFence::INITIAL,
            std::slice::from_ref(&message),
            &attempt,
        )
        .expect("wins");

        assert!(commit.run.status.is_terminal());
        assert!(commit.run.outcome.is_some());
        assert_eq!(commit.sealed_messages.len(), 1);
        assert_eq!(commit.session.active_run, None);
        assert_eq!(commit.session.status, SessionStatus::Idle);
        assert_eq!(commit.session.revision, session.revision.next());
        assert_eq!(commit.outbox.run, run.id);
        assert_eq!(commit.usage_closure, attempt.usage_closure);
    }

    #[test]
    fn the_terminal_barrier_refuses_an_unbounded_open_message_set() {
        let (session, run, agent, message) = running_session();
        let messages = vec![message; MAX_OPEN_MESSAGES_PER_RUN + 1];
        assert_eq!(
            claim_terminal(
                &run,
                &session,
                agent.id,
                AgentFence::INITIAL,
                &messages,
                &terminal_attempt(&session, &run),
            ),
            Err(TerminalRejection::TooManyOpenMessages {
                seen: MAX_OPEN_MESSAGES_PER_RUN + 1,
                max: MAX_OPEN_MESSAGES_PER_RUN,
            })
        );
    }

    #[test]
    fn a_loser_gets_the_winning_outcome_and_writes_nothing() {
        let (session, run, agent, message) = running_session();
        let attempt = terminal_attempt(&session, &run);
        let won = claim_terminal(
            &run,
            &session,
            agent.id,
            AgentFence::INITIAL,
            std::slice::from_ref(&message),
            &attempt,
        )
        .expect("wins");

        let second = claim_terminal(
            &won.run,
            &session,
            agent.id,
            AgentFence::INITIAL,
            std::slice::from_ref(&message),
            &attempt,
        );
        assert_eq!(
            second,
            Err(TerminalRejection::AlreadyTerminal {
                winner: attempt.outcome.clone()
            })
        );
    }

    #[test]
    fn every_stale_condition_is_reported_distinctly() {
        let (session, run, agent, message) = running_session();
        let mut attempt = terminal_attempt(&session, &run);

        attempt.session_revision_seen = SessionRevision(99);
        assert!(matches!(
            claim_terminal(
                &run,
                &session,
                agent.id,
                AgentFence::INITIAL,
                std::slice::from_ref(&message),
                &attempt
            ),
            Err(TerminalRejection::StaleSessionRevision { .. })
        ));

        let mut attempt = terminal_attempt(&session, &run);
        attempt.cancellation_seen = CancellationEpoch(99);
        assert!(matches!(
            claim_terminal(
                &run,
                &session,
                agent.id,
                AgentFence::INITIAL,
                std::slice::from_ref(&message),
                &attempt
            ),
            Err(TerminalRejection::StaleCancellationEpoch { .. })
        ));

        let attempt = terminal_attempt(&session, &run);
        assert!(matches!(
            claim_terminal(
                &run,
                &session,
                agent.id,
                AgentFence(7),
                std::slice::from_ref(&message),
                &attempt
            ),
            Err(TerminalRejection::StaleAgentFence { .. })
        ));

        let mut foreign = session.clone();
        foreign.active_run = Some(RunId::from_uuid7(Uuid7::compose(9, [9; 10])));
        assert!(matches!(
            claim_terminal(
                &run,
                &foreign,
                agent.id,
                AgentFence::INITIAL,
                std::slice::from_ref(&message),
                &terminal_attempt(&foreign, &run)
            ),
            Err(TerminalRejection::NotSessionOwner { .. })
        ));
    }

    #[test]
    fn a_queued_run_may_settle_without_ever_running() {
        let (session, mut run, agent, message) = running_session();
        run.status = RunStatus::Queued;
        run.started_at = None;
        let attempt = terminal_attempt(&session, &run);
        let commit = claim_terminal(
            &run,
            &session,
            agent.id,
            AgentFence::INITIAL,
            std::slice::from_ref(&message),
            &attempt,
        )
        .expect("wins");
        assert!(commit.run.status.is_terminal());
        assert_eq!(commit.run.started_at, None);
    }
}
