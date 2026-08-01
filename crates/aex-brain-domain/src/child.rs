//! Child agents, their durable states, and the join ledger.
//!
//! `create_subagent` always returns a durable child id and state. It never blocks and
//! never fails for lack of capacity, because a caller that cannot distinguish "refused"
//! from "not yet scheduled" will retry and create a second child.

use serde::{Deserialize, Serialize};

use crate::budget::BudgetGrant;
use crate::ids::{AgentId, JoinId};

/// Why a child is durably queued rather than running.
///
/// A queued child reports its reason so the parent's model, and the operator, can see
/// which limit is binding instead of watching an opaque delay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueuedReason {
    /// The session's active-children budget is full.
    ActiveBudget,
    /// The session's own materialized-agent limit is full.
    SessionActiveLimit,
    /// No provider permit is available.
    ProviderPermits,
    /// No Hands permit is available.
    HandsPermits,
    /// Another tenant's fair share is being protected.
    TenantFairness,
    /// The region has no capacity.
    RegionalCapacity,
    /// The child sits at a depth the scheduler defers.
    DepthDeferred,
}

/// Where a child stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ChildState {
    /// Durably created, not yet claimed.
    Queued {
        /// Which limit is binding.
        reason: QueuedReason,
    },
    /// Claimed by the scheduler, not yet running.
    Starting,
    /// Running.
    Running,
    /// A stop was requested and is being honoured.
    Stopping,
    /// Finished on its own terms.
    Completed,
    /// Finished with a typed failure.
    Failed,
    /// Cancelled, whether dequeued before claim or stopped after it.
    Cancelled,
}

impl ChildState {
    /// Whether this state admits no further transition.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    /// Whether the child still counts against the parent's active-children reservation.
    #[must_use]
    pub const fn is_materialized(self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Stopping)
    }
}

/// How a child ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ChildOutcome {
    /// Finished on its own terms.
    Completed,
    /// Finished with a typed failure. It cancels no sibling and does not fail the root:
    /// the parent's model decides what to do about it.
    Failed,
    /// Cancelled.
    Cancelled {
        /// Whether the cancellation happened before or after the scheduler's claim.
        cause: CancelCause,
    },
}

impl ChildOutcome {
    /// The durable child state this outcome implies.
    #[must_use]
    pub const fn state(self) -> ChildState {
        match self {
            Self::Completed => ChildState::Completed,
            Self::Failed => ChildState::Failed,
            Self::Cancelled { .. } => ChildState::Cancelled,
        }
    }
}

/// Why a child was cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelCause {
    /// The parent removed it from the queue before any claim. History is preserved.
    Dequeued,
    /// A fenced stop was requested after the claim.
    Stopped,
    /// The session's cancellation epoch advanced.
    EpochAdvanced,
}

/// One child, as the parent's fold holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildRecord {
    /// The child's deterministic identity.
    pub child: AgentId,
    /// The parent's monotonic spawn counter value that derived the identity.
    pub ordinal: u32,
    /// What the parent reserved for it.
    pub grant: BudgetGrant,
    /// The join this child belongs to.
    pub join: JoinId,
    /// Where it stands.
    pub state: ChildState,
}

/// Why a join operation was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JoinError {
    /// A terminal was recorded for a child that is not a member of the group.
    #[error("agent {child:?} is not a member of join {join:?}")]
    NotAMember {
        /// The join group.
        join: JoinId,
        /// The agent that claimed membership.
        child: AgentId,
    },
    /// The join group does not exist in this fold.
    #[error("join {join:?} is unknown")]
    UnknownJoin {
        /// The join group.
        join: JoinId,
    },
}

/// Whether a join has been satisfied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinStatus {
    /// Members remain outstanding.
    Pending,
    /// The waiter may be released.
    Satisfied,
}

/// Whether `group` is satisfied.
///
/// The shard counters in the store are a projection, not completion truth. This function
/// takes the immutable membership and the immutable terminal set, which is the only
/// authority; a disagreement with a counter makes the parent re-derive from these rows.
#[must_use]
pub fn join_status(group: &crate::wire_pending::JoinGroup) -> JoinStatus {
    let satisfied = match group.mode {
        crate::wire_pending::JoinMode::Any => !group.done.is_empty(),
        crate::wire_pending::JoinMode::All => group
            .members
            .iter()
            .all(|member| group.done.contains(member)),
    };
    if satisfied {
        JoinStatus::Satisfied
    } else {
        JoinStatus::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::{CancelCause, ChildOutcome, ChildState, JoinStatus, QueuedReason, join_status};
    use crate::ids::{AgentId, JoinId};
    use crate::wire_pending::{JoinGroup, JoinMode};
    use uuid::Uuid;

    fn group(mode: JoinMode, members: usize, done: usize) -> JoinGroup {
        let all: Vec<AgentId> = (0..members)
            .map(|index| AgentId(Uuid::from_u128(index as u128)))
            .collect();
        JoinGroup {
            join: JoinId(Uuid::from_u128(99)),
            mode,
            done: all.iter().take(done).copied().collect(),
            members: all,
            shards: 1,
        }
    }

    #[test]
    fn any_releases_on_the_first_terminal_member() {
        assert_eq!(
            join_status(&group(JoinMode::Any, 4, 0)),
            JoinStatus::Pending
        );
        assert_eq!(
            join_status(&group(JoinMode::Any, 4, 1)),
            JoinStatus::Satisfied
        );
    }

    #[test]
    fn all_releases_only_when_every_member_is_terminal() {
        assert_eq!(
            join_status(&group(JoinMode::All, 4, 3)),
            JoinStatus::Pending
        );
        assert_eq!(
            join_status(&group(JoinMode::All, 4, 4)),
            JoinStatus::Satisfied
        );
    }

    #[test]
    fn duplicate_terminals_cannot_over_satisfy_a_join() {
        let mut duplicated = group(JoinMode::All, 3, 2);
        let repeated = duplicated.done[0];
        duplicated.done.push(repeated);
        duplicated.done.push(repeated);
        assert_eq!(join_status(&duplicated), JoinStatus::Pending);
    }

    #[test]
    fn only_running_states_hold_the_parents_active_reservation() {
        assert!(ChildState::Running.is_materialized());
        assert!(ChildState::Starting.is_materialized());
        assert!(ChildState::Stopping.is_materialized());
        assert!(
            !ChildState::Queued {
                reason: QueuedReason::ActiveBudget
            }
            .is_materialized()
        );
        assert!(!ChildState::Completed.is_materialized());
    }

    #[test]
    fn each_outcome_maps_to_exactly_one_terminal_state() {
        assert_eq!(ChildOutcome::Completed.state(), ChildState::Completed);
        assert_eq!(ChildOutcome::Failed.state(), ChildState::Failed);
        assert_eq!(
            ChildOutcome::Cancelled {
                cause: CancelCause::Dequeued
            }
            .state(),
            ChildState::Cancelled
        );
        for state in [
            ChildState::Completed,
            ChildState::Failed,
            ChildState::Cancelled,
        ] {
            assert!(state.is_terminal());
        }
    }
}
