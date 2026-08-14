//! Agent control records.
//!
//! A session may admit at most twelve distinct non-root agent identities over
//! its lifetime, and lineage stops at depth three. Terminal children continue
//! to count. `Parked` is internal and projects to the public `running` (D-12);
//! `Idle` is the root at rest and has no public child projection at all.

use std::collections::BTreeSet;

use aex_internal_contracts::RunId;
use aex_wire::ids::{AgentId, ApprovalId, GenerationId, SessionId};
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
    /// The public cancellation command asked for it.
    SessionCancel,
    /// The irreversible deletion fence was crossed.
    SessionDeleting,
    /// The account is paused.
    AccountPaused,
    /// The workspace generation is gone.
    ContinuityLost,
}

impl CancelCause {
    /// Every cause, in canonical order.
    pub const ALL: [Self; 4] = [
        Self::SessionCancel,
        Self::SessionDeleting,
        Self::AccountPaused,
        Self::ContinuityLost,
    ];

    /// The admission state the cause leaves the session in.
    ///
    /// Account state is not copied into every session head. Both an explicit
    /// cancellation and an account-pause interruption leave this local gate
    /// open; the workspace placement remains the sole pause/restore authority.
    #[must_use]
    pub const fn target_admission(self) -> WorkAdmission {
        match self {
            Self::SessionCancel | Self::AccountPaused => WorkAdmission::Open,
            Self::SessionDeleting => WorkAdmission::Deleting,
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
    /// Its run-local spend ceiling, when it has one. The root agent is born at
    /// rest with no run and receives the ceiling of the run it executes.
    pub budget: Option<BudgetGrant>,
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

/// The session-level half of a cancellation: the fence, without the per-agent
/// records.
///
/// A paged stop settles its agents over several bounded steps but has to close
/// the fence on the **first** one, so the two halves cannot be one value.
/// [`cancel_session_work`] is defined in terms of this, so there is exactly one
/// answer to "what fence does this cause establish".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionFence {
    /// The epoch after the cancellation.
    pub cancellation: CancellationEpoch,
    /// The admission the session is left in.
    pub admission: WorkAdmission,
    /// Whether the fence moved at all.
    pub changed: bool,
}

/// The fence a session-wide cancellation establishes.
///
/// `has_active_work` is the caller's bounded observation: a paged stop knows
/// only about the agents in the page it read, and reading the whole session to
/// answer "is anything still running" is the unbounded scan the paged protocol
/// exists to avoid. Reporting `true` costs one epoch bump, and the epoch is
/// monotone, so an extra bump is never wrong.
#[must_use]
pub fn cancel_session_fence(
    session: &Session,
    cause: CancelCause,
    has_active_work: bool,
) -> SessionFence {
    let target = cause.target_admission();
    if !has_active_work && session.work_admission == target {
        return SessionFence {
            cancellation: session.cancellation,
            admission: session.work_admission,
            changed: false,
        };
    }
    SessionFence {
        cancellation: session.cancellation.next(),
        admission: target,
        changed: true,
    }
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
    /// The requested lineage is deeper than the runtime permits.
    #[error("subagent depth {depth} exceeds the maximum {maximum}")]
    DepthExceeded {
        /// Requested depth, with the root at zero.
        depth: u16,
        /// The hard runtime maximum.
        maximum: u16,
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
            budget: Some(state.budget),
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
/// The ceiling counts every distinct subagent in `materialized`, including
/// terminal history. A caller retrying an existing `id` receives that existing
/// control record without consuming another slot.
///
/// # Errors
///
/// Returns [`AgentError`] when the parent is not active or in another session,
/// when the lifetime-agent or depth ceiling is reached, when admission is
/// closed, or when the required limit is unresolved.
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

    if let Some(existing) = materialized
        .iter()
        .find(|agent| agent.session == session.id && agent.id == id)
    {
        return Ok(AgentCommit {
            agent: existing.clone(),
            changed: false,
        });
    }

    let depth = parent.depth.saturating_add(1);
    if depth > aex_wire::limits::MAX_SUBAGENT_DEPTH {
        return Err(AgentError::DepthExceeded {
            depth,
            maximum: aex_wire::limits::MAX_SUBAGENT_DEPTH,
        });
    }

    let configured_agents = limits.require(LimitId::SessionMaterializedAgents)?;
    let ceiling = configured_agents
        .saturating_sub(1)
        .min(aex_wire::limits::MAX_SUBAGENTS_PER_SESSION);
    let lifetime_subagents = materialized
        .iter()
        .filter(|agent| agent.session == session.id && agent.kind == AgentKind::Subagent)
        .count();
    let admitted = u64::try_from(lifetime_subagents).unwrap_or(u64::MAX);
    if admitted >= ceiling {
        return Err(AgentError::CeilingExceeded {
            limit: LimitId::SessionMaterializedAgents,
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
            budget: Some(state.budget),
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

    let fence = cancel_session_fence(session, cause, !active.is_empty());
    if stale_generation || !fence.changed {
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
        cancellation: fence.cancellation,
        admission: fence.admission,
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

    fn limits(materialized_agents: u64) -> EffectiveLimits {
        [(LimitId::SessionMaterializedAgents, materialized_agents)]
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
    fn the_ceiling_counts_distinct_subagents_for_the_session_lifetime() {
        let session = session_fixture();
        let root = create_root(agent_id(1), &session, materialized_state(), moment(0)).agent;

        let mut live = Vec::new();
        for tag in 2..4_u8 {
            let child = spawn(
                agent_id(tag),
                &root,
                &session,
                &limits(3),
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
                &limits(3),
                &live,
                materialized_state(),
                moment(2)
            ),
            Err(AgentError::CeilingExceeded {
                limit: LimitId::SessionMaterializedAgents,
                effective: 2
            })
        );

        // Settling one does not free its identity slot.
        live[0] = complete_agent(
            &live[0],
            AgentTerminal::Completed,
            AgentFence::INITIAL,
            moment(3),
        )
        .expect("completes")
        .agent;
        assert_eq!(
            spawn(
                agent_id(9),
                &root,
                &session,
                &limits(3),
                &live,
                materialized_state(),
                moment(4)
            ),
            Err(AgentError::CeilingExceeded {
                limit: LimitId::SessionMaterializedAgents,
                effective: 2
            })
        );

        // Retrying an existing identity is idempotent and consumes no slot.
        let retried = spawn(
            live[0].id,
            &root,
            &session,
            &limits(3),
            &live,
            materialized_state(),
            moment(5),
        )
        .expect("same-id retry returns the existing child");
        assert!(!retried.changed);
        assert_eq!(retried.agent, live[0]);
    }

    #[test]
    fn depth_three_is_terminal_even_when_configured_limits_are_looser() {
        let session = session_fixture();
        let mut parent = create_root(agent_id(1), &session, materialized_state(), moment(0)).agent;
        parent.kind = AgentKind::Subagent;
        parent.depth = 3;
        parent.status = AgentStatus::Running;

        assert_eq!(
            spawn(
                agent_id(2),
                &parent,
                &session,
                &limits(100),
                &[parent.clone()],
                materialized_state(),
                moment(1),
            ),
            Err(AgentError::DepthExceeded {
                depth: 4,
                maximum: 3,
            })
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
            cancel_session_work(&session, &[], CancelCause::SessionCancel, None, moment(1));
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
            &limits(4),
            &[],
            materialized_state(),
            moment(1),
        )
        .expect("spawns")
        .agent;
        let commit = cancel_session_work(
            &session,
            &[child],
            CancelCause::SessionDeleting,
            None,
            moment(2),
        );
        assert!(commit.changed);
        assert_eq!(commit.cancellation, session.cancellation.next());
        assert_eq!(commit.agents.len(), 1);
        assert_eq!(commit.agents[0].status, AgentStatus::Cancelled);
    }
}
