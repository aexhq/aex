//! Agent control records.
//!
//! The ceiling counts **concurrently materialized** agents (D-22): a terminal
//! agent never consumes budget and there is no finalized-agent TTL, so semantic
//! history is kept without paying for it. `Parked` is internal and projects to
//! the public `running` (D-12); `Idle` is the root at rest and has no public
//! child projection at all.

use std::collections::BTreeSet;

use aex_wire::ids::{AgentId, ApprovalId, GenerationId, RunId, SessionId};
use aex_wire::limits::LimitId;
use aex_wire::types::Timestamp;

use crate::budget::{BudgetGrant, EffectiveLimits, LimitUnresolved};
use crate::ids::{
    AgentFence, AgentRevision, CancellationEpoch, EffectId, EntryIdentity, JournalSeq,
};
use crate::session::{Session, WorkAdmission};

/// Whether an agent is the session's root or a subagent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AgentKind {
    /// The session's own agent.
    Root,
    /// A spawned child.
    Subagent,
}

/// The public projection of an agent's status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PublicAgentStatus {
    /// Waiting for a scheduler claim.
    Queued,
    /// Being started.
    Starting,
    /// Executing, including parked.
    Running,
    /// Being stopped.
    Stopping,
    /// Finished normally.
    Completed,
    /// Finished with an error.
    Failed,
    /// Cancelled.
    Cancelled,
}

/// Where an agent is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AgentStatus {
    /// The root at rest.
    Idle,
    /// A child waiting for a scheduler claim.
    Queued,
    /// Being started.
    Starting,
    /// Executing.
    Running,
    /// Waiting on a join, an approval or a schedule. Holds no lease.
    Parked,
    /// Being stopped.
    Stopping,
    /// Finished normally.
    Completed,
    /// Finished with an error.
    Failed,
    /// Cancelled.
    Cancelled,
}

impl AgentStatus {
    /// Every status, in lifecycle order.
    pub const ALL: [Self; 9] = [
        Self::Idle,
        Self::Queued,
        Self::Starting,
        Self::Running,
        Self::Parked,
        Self::Stopping,
        Self::Completed,
        Self::Failed,
        Self::Cancelled,
    ];

    /// Whether no transition leaves this status.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    /// Whether the agent counts against the materialized-agent ceiling.
    #[must_use]
    pub const fn is_materialized(self) -> bool {
        !self.is_terminal()
    }

    /// The public projection. `Parked` reads as `running`; `Idle` — the root at
    /// rest — has no child projection.
    #[must_use]
    pub const fn public(self) -> Option<PublicAgentStatus> {
        match self {
            Self::Idle => None,
            Self::Queued => Some(PublicAgentStatus::Queued),
            Self::Starting => Some(PublicAgentStatus::Starting),
            Self::Running | Self::Parked => Some(PublicAgentStatus::Running),
            Self::Stopping => Some(PublicAgentStatus::Stopping),
            Self::Completed => Some(PublicAgentStatus::Completed),
            Self::Failed => Some(PublicAgentStatus::Failed),
            Self::Cancelled => Some(PublicAgentStatus::Cancelled),
        }
    }
}

/// Why a spawned agent is still queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QueueReason {
    /// The session's fan-out budget is full.
    FanoutBudget,
    /// Tenant fairness is holding it.
    TenantFairness,
    /// It is waiting for a provider permit.
    ProviderPermit,
    /// It is waiting for a Hands permit.
    HandsPermit,
    /// It is at the depth limit.
    DepthLimit,
}

/// Why a session-wide cancellation happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CancelCause {
    /// A stop operation asked for it.
    StopRequested,
    /// The session is being trashed.
    SessionTrashing,
    /// The account is paused.
    AccountPaused,
    /// The workspace generation is gone.
    ContinuityLost,
}

impl CancelCause {
    /// Every cause, in canonical order.
    pub const ALL: [Self; 4] = [
        Self::StopRequested,
        Self::SessionTrashing,
        Self::AccountPaused,
        Self::ContinuityLost,
    ];

    /// The admission state the cause leaves the session in.
    ///
    /// Only `StopRequested` returns admission to `Open`; the other three are
    /// conditions that outlive the cancellation.
    #[must_use]
    pub const fn target_admission(self) -> WorkAdmission {
        match self {
            Self::StopRequested => WorkAdmission::Open,
            Self::SessionTrashing => WorkAdmission::Trashing,
            Self::AccountPaused => WorkAdmission::Paused,
            Self::ContinuityLost => WorkAdmission::ContinuityLost,
        }
    }
}

/// How an agent ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AgentTerminal {
    /// Finished normally.
    Completed,
    /// Finished with an error.
    Failed,
    /// Cancelled.
    Cancelled,
}

impl AgentTerminal {
    /// Every terminal, in canonical order.
    pub const ALL: [Self; 3] = [Self::Completed, Self::Failed, Self::Cancelled];

    /// The status it settles the agent at.
    #[must_use]
    pub const fn status(self) -> AgentStatus {
        match self {
            Self::Completed => AgentStatus::Completed,
            Self::Failed => AgentStatus::Failed,
            Self::Cancelled => AgentStatus::Cancelled,
        }
    }
}

/// Who owns an agent's current claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentClaim {
    /// The owner.
    pub owner: aex_operation_domain::OwnerId,
    /// The fence every write must present.
    pub fence: AgentFence,
    /// When the claim lapses.
    pub expires_at: Timestamp,
}

/// Where a finished subagent's result goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JoinEdge {
    /// The parent waiting on it.
    pub parent: AgentId,
    /// The run that is waiting.
    pub run: RunId,
}

/// The effects an agent has prepared and not yet settled.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpenEffectSet(BTreeSet<EffectId>);

impl OpenEffectSet {
    /// An empty set.
    #[must_use]
    pub fn new() -> Self {
        Self(BTreeSet::new())
    }

    /// Records an opened effect, reporting whether it was new.
    pub fn open(&mut self, effect: EffectId) -> bool {
        self.0.insert(effect)
    }

    /// Settles an effect, reporting whether it was open.
    pub fn close(&mut self, effect: EffectId) -> bool {
        self.0.remove(&effect)
    }

    /// Whether nothing is open.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// How many effects are open.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Iterates the open effects in canonical order.
    pub fn iter(&self) -> impl Iterator<Item = &EffectId> {
        self.0.iter()
    }
}

/// One agent's control record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentControl {
    /// Its identity.
    pub id: AgentId,
    /// The owning session.
    pub session: SessionId,
    /// Root or subagent.
    pub kind: AgentKind,
    /// Its parent, when it has one.
    pub parent: Option<AgentId>,
    /// How deep it is; the root is zero.
    pub depth: u16,
    /// Where it is.
    pub status: AgentStatus,
    /// Its optimistic concurrency token.
    pub revision: AgentRevision,
    /// The last journal position it wrote.
    pub journal_tail: JournalSeq,
    /// The identity of the entry at [`AgentControl::journal_tail`].
    ///
    /// Held so a retried append at the same position can be recognised as the
    /// same entry rather than a regression.
    pub last_entry: Option<EntryIdentity>,
    /// Its current claim, when it is claimed.
    pub claim: Option<AgentClaim>,
    /// Where its result goes, when it is a child.
    pub join: Option<JoinEdge>,
    /// Its spend grant.
    pub budget: BudgetGrant,
    /// Its unsettled effects.
    pub open_effects: OpenEffectSet,
    /// The approval it is blocked on, when it is.
    pub pending_approval: Option<ApprovalId>,
    /// Why it is queued, when it is.
    pub queue_reason: Option<QueueReason>,
    /// The workspace generation it is bound to.
    pub generation: Option<GenerationId>,
    /// How it ended.
    pub terminal: Option<AgentTerminal>,
    /// When it was created.
    pub created_at: Timestamp,
}

impl AgentControl {
    /// The fence a writer must present.
    #[must_use]
    pub fn fence(&self) -> AgentFence {
        self.claim.map_or(AgentFence::INITIAL, |claim| claim.fence)
    }
}

/// The agent after a transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCommit {
    /// The record after the transition.
    pub agent: AgentControl,
    /// Whether anything changed.
    pub changed: bool,
}

/// What a session-wide cancellation produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancelSessionWorkCommit {
    /// The cancelled agents.
    pub agents: Vec<AgentControl>,
    /// The epoch after the cancellation.
    pub cancellation: CancellationEpoch,
    /// The admission the session is left in.
    pub admission: WorkAdmission,
    /// Whether the cancellation had any effect at all.
    pub changed: bool,
}

/// The state a materialized agent starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaterializedState {
    /// Its spend grant.
    pub budget: BudgetGrant,
    /// The generation it is bound to.
    pub generation: Option<GenerationId>,
}

/// Why an agent transition was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentError {
    /// No such agent.
    #[error("agent not found")]
    NotFound,
    /// The agent has already settled.
    #[error("agent is already {0:?}")]
    AlreadyResolved(AgentStatus),
    /// The materialized-agent ceiling is full.
    #[error("limit `{limit}` of {effective} materialized agents is reached")]
    CeilingExceeded {
        /// Which limit.
        limit: LimitId,
        /// Its effective value.
        effective: u64,
    },
    /// The session is not admitting work.
    #[error("session admission is {0:?}")]
    AdmissionClosed(WorkAdmission),
    /// The generation moved under the agent.
    #[error("expected generation {expected} but saw {seen:?}")]
    StaleGeneration {
        /// What the caller expected.
        expected: GenerationId,
        /// What the session has.
        seen: Option<GenerationId>,
    },
    /// The parent cannot spawn right now.
    #[error("parent agent is {0:?}")]
    ParentNotActive(AgentStatus),
    /// The parent belongs to a different session.
    #[error("parent agent belongs to another session")]
    ParentInOtherSession,
    /// The depth limit is reached.
    #[error("subagent depth limit of {max} is reached")]
    DepthExceeded {
        /// The limit.
        max: u16,
    },
    /// A root agent cannot report a result.
    #[error("a root agent can never complete")]
    RootCannotComplete,
    /// A required limit is not resolved.
    #[error(transparent)]
    LimitUnresolved(#[from] LimitUnresolved),
}

/// Creates the session's root agent.
#[must_use]
pub fn create_root(
    id: AgentId,
    session: &Session,
    state: MaterializedState,
    now: Timestamp,
) -> AgentCommit {
    AgentCommit {
        agent: AgentControl {
            id,
            session: session.id,
            kind: AgentKind::Root,
            parent: None,
            depth: 0,
            status: AgentStatus::Idle,
            revision: AgentRevision::INITIAL,
            journal_tail: JournalSeq::INITIAL,
            last_entry: None,
            claim: None,
            join: None,
            budget: state.budget,
            open_effects: OpenEffectSet::new(),
            pending_approval: None,
            queue_reason: None,
            generation: state.generation,
            terminal: None,
            created_at: now,
        },
        changed: true,
    }
}

/// Spawns a subagent.
///
/// The ceiling counts only non-terminal **subagents** in `materialized`, so a
/// completed child frees its slot and a session's semantic history costs nothing.
///
/// # Errors
///
/// Returns [`AgentError`] when the parent is not active or in another session,
/// when the depth or concurrency ceiling is reached, when admission is closed, or
/// when a required limit is unresolved.
pub fn spawn(
    id: AgentId,
    parent: &AgentControl,
    session: &Session,
    limits: &EffectiveLimits,
    materialized: &[AgentControl],
    state: MaterializedState,
    now: Timestamp,
) -> Result<AgentCommit, AgentError> {
    if !session.work_admission.is_open() {
        return Err(AgentError::AdmissionClosed(session.work_admission));
    }
    if parent.session != session.id {
        return Err(AgentError::ParentInOtherSession);
    }
    if !matches!(parent.status, AgentStatus::Idle | AgentStatus::Running) {
        return Err(AgentError::ParentNotActive(parent.status));
    }

    let depth_limit = limits.require(LimitId::SessionSubagentDepth)?;
    let max_depth = u16::try_from(depth_limit).unwrap_or(u16::MAX);
    let depth = parent.depth.saturating_add(1);
    if u64::from(depth) > depth_limit {
        return Err(AgentError::DepthExceeded { max: max_depth });
    }

    let ceiling = limits.require(LimitId::SessionSubagentConcurrency)?;
    let live = materialized
        .iter()
        .filter(|agent| {
            agent.session == session.id
                && agent.kind == AgentKind::Subagent
                && agent.status.is_materialized()
        })
        .count();
    if live as u64 >= ceiling {
        return Err(AgentError::CeilingExceeded {
            limit: LimitId::SessionSubagentConcurrency,
            effective: ceiling,
        });
    }

    Ok(AgentCommit {
        agent: AgentControl {
            id,
            session: session.id,
            kind: AgentKind::Subagent,
            parent: Some(parent.id),
            depth,
            status: AgentStatus::Queued,
            revision: AgentRevision::INITIAL,
            journal_tail: JournalSeq::INITIAL,
            last_entry: None,
            claim: None,
            join: None,
            budget: state.budget,
            open_effects: OpenEffectSet::new(),
            pending_approval: None,
            queue_reason: Some(QueueReason::FanoutBudget),
            generation: state.generation,
            terminal: None,
            created_at: now,
        },
        changed: true,
    })
}

/// Claims an agent and starts it.
///
/// A new owner advances the fence; the same owner reclaiming keeps it, which is
/// what stops renewal churn invalidating the owner's own in-flight writes.
///
/// # Errors
///
/// Returns [`AgentError`] for a settled agent, a closed admission or a stale
/// generation.
pub fn start_agent(
    agent: &AgentControl,
    session: &Session,
    owner: aex_operation_domain::OwnerId,
    expires_at: Timestamp,
) -> Result<AgentCommit, AgentError> {
    if agent.status.is_terminal() {
        return Err(AgentError::AlreadyResolved(agent.status));
    }
    if !session.work_admission.is_open() {
        return Err(AgentError::AdmissionClosed(session.work_admission));
    }
    if let Some(expected) = agent.generation
        && session.generation != Some(expected)
    {
        return Err(AgentError::StaleGeneration {
            expected,
            seen: session.generation,
        });
    }

    let same_owner = agent.claim.is_some_and(|claim| claim.owner == owner);
    let fence = if same_owner {
        agent.fence()
    } else {
        agent.fence().next()
    };
    let mut next = agent.clone();
    next.status = AgentStatus::Running;
    next.queue_reason = None;
    next.revision = agent.revision.next();
    next.claim = Some(AgentClaim {
        owner,
        fence,
        expires_at,
    });
    Ok(AgentCommit {
        agent: next,
        changed: true,
    })
}

/// Settles an agent.
///
/// # Errors
///
/// Returns [`AgentError::RootCannotComplete`] for a root agent — a session's own
/// agent has no result to report — [`AgentError::AlreadyResolved`] for a settled
/// agent, and a fence rejection as [`AgentError::AlreadyResolved`] never: a stale
/// fence is reported by the caller's transaction condition, not here.
pub fn complete_agent(
    agent: &AgentControl,
    terminal: AgentTerminal,
    fence: AgentFence,
    now: Timestamp,
) -> Result<AgentCommit, AgentError> {
    let _ = now;
    if agent.kind == AgentKind::Root {
        return Err(AgentError::RootCannotComplete);
    }
    if agent.status.is_terminal() {
        return Err(AgentError::AlreadyResolved(agent.status));
    }
    if fence != agent.fence() {
        return Err(AgentError::AlreadyResolved(agent.status));
    }
    let mut next = agent.clone();
    next.status = terminal.status();
    next.terminal = Some(terminal);
    next.revision = agent.revision.next();
    next.claim = None;
    next.pending_approval = None;
    next.queue_reason = None;
    Ok(AgentCommit {
        agent: next,
        changed: true,
    })
}

/// Cancels every active agent in a session.
///
/// A no-op — and the epoch does **not** advance — when nothing is active and the
/// admission already equals the target, or when a `ContinuityLost` cause names a
/// generation that is no longer current.
#[must_use]
pub fn cancel_session_work(
    session: &Session,
    agents: &[AgentControl],
    cause: CancelCause,
    named_generation: Option<GenerationId>,
    now: Timestamp,
) -> CancelSessionWorkCommit {
    let target = cause.target_admission();
    let stale_generation = cause == CancelCause::ContinuityLost
        && named_generation.is_some()
        && named_generation != session.generation;

    let active: Vec<&AgentControl> = agents
        .iter()
        .filter(|agent| {
            agent.session == session.id
                && matches!(
                    agent.status,
                    AgentStatus::Queued
                        | AgentStatus::Starting
                        | AgentStatus::Running
                        | AgentStatus::Parked
                )
        })
        .collect();

    if stale_generation || (active.is_empty() && session.work_admission == target) {
        return CancelSessionWorkCommit {
            agents: Vec::new(),
            cancellation: session.cancellation,
            admission: session.work_admission,
            changed: false,
        };
    }

    let cancelled = active
        .into_iter()
        .map(|agent| {
            let mut next = agent.clone();
            next.status = AgentStatus::Cancelled;
            next.terminal = Some(AgentTerminal::Cancelled);
            next.revision = agent.revision.next();
            next.claim = None;
            next.pending_approval = None;
            next.queue_reason = None;
            next.created_at = agent.created_at;
            next
        })
        .collect();
    let _ = now;

    CancelSessionWorkCommit {
        agents: cancelled,
        cancellation: session.cancellation.next(),
        admission: target,
        changed: true,
    }
}

#[cfg(test)]
mod tests {
    use aex_wire::ids::{AgentId, PrefixedId as _, Uuid7};
    use aex_wire::limits::LimitId;
    use aex_wire::types::Timestamp;

    use super::{
        AgentError, AgentKind, AgentStatus, AgentTerminal, CancelCause, PublicAgentStatus,
        cancel_session_work, complete_agent, create_root, spawn,
    };
    use crate::budget::EffectiveLimits;
    use crate::ids::AgentFence;
    use crate::testing::{materialized_state, session_fixture};

    fn moment(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("in range")
    }

    fn agent_id(tag: u8) -> AgentId {
        AgentId::from_uuid7(Uuid7::compose(1, [tag; 10]))
    }

    fn limits(concurrency: u64, depth: u64) -> EffectiveLimits {
        [
            (LimitId::SessionSubagentConcurrency, concurrency),
            (LimitId::SessionSubagentDepth, depth),
        ]
        .into_iter()
        .collect()
    }

    #[test]
    fn parked_projects_to_running_and_idle_has_no_projection() {
        assert_eq!(
            AgentStatus::Parked.public(),
            Some(PublicAgentStatus::Running)
        );
        assert_eq!(AgentStatus::Idle.public(), None);
        for status in AgentStatus::ALL {
            assert_eq!(status.is_materialized(), !status.is_terminal());
        }
    }

    #[test]
    fn the_ceiling_counts_only_non_terminal_subagents() {
        let session = session_fixture();
        let root = create_root(agent_id(1), &session, materialized_state(), moment(0)).agent;

        let mut live = Vec::new();
        for tag in 2..4_u8 {
            let child = spawn(
                agent_id(tag),
                &root,
                &session,
                &limits(2, 5),
                &live,
                materialized_state(),
                moment(1),
            )
            .expect("spawns")
            .agent;
            live.push(child);
        }
        assert_eq!(
            spawn(
                agent_id(9),
                &root,
                &session,
                &limits(2, 5),
                &live,
                materialized_state(),
                moment(2)
            ),
            Err(AgentError::CeilingExceeded {
                limit: LimitId::SessionSubagentConcurrency,
                effective: 2
            })
        );

        // Settling one frees its slot; terminal history never consumes budget.
        live[0] = complete_agent(
            &live[0],
            AgentTerminal::Completed,
            AgentFence::INITIAL,
            moment(3),
        )
        .expect("completes")
        .agent;
        assert!(
            spawn(
                agent_id(9),
                &root,
                &session,
                &limits(2, 5),
                &live,
                materialized_state(),
                moment(4)
            )
            .is_ok()
        );
    }

    #[test]
    fn a_root_can_never_complete() {
        let session = session_fixture();
        let root = create_root(agent_id(1), &session, materialized_state(), moment(0)).agent;
        assert_eq!(root.kind, AgentKind::Root);
        assert_eq!(
            complete_agent(
                &root,
                AgentTerminal::Completed,
                AgentFence::INITIAL,
                moment(1)
            ),
            Err(AgentError::RootCannotComplete)
        );
    }

    #[test]
    fn cancelling_nothing_does_not_advance_the_epoch() {
        let session = session_fixture();
        let commit =
            cancel_session_work(&session, &[], CancelCause::StopRequested, None, moment(1));
        assert!(!commit.changed);
        assert_eq!(commit.cancellation, session.cancellation);
    }

    #[test]
    fn cancelling_active_work_advances_the_epoch_by_one() {
        let session = session_fixture();
        let root = create_root(agent_id(1), &session, materialized_state(), moment(0)).agent;
        let child = spawn(
            agent_id(2),
            &root,
            &session,
            &limits(4, 5),
            &[],
            materialized_state(),
            moment(1),
        )
        .expect("spawns")
        .agent;
        let commit = cancel_session_work(
            &session,
            &[child],
            CancelCause::SessionTrashing,
            None,
            moment(2),
        );
        assert!(commit.changed);
        assert_eq!(commit.cancellation, session.cancellation.next());
        assert_eq!(commit.agents.len(), 1);
        assert_eq!(commit.agents[0].status, AgentStatus::Cancelled);
    }
}
