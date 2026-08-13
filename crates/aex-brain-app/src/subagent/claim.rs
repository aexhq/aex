//! The conditional claim, the dequeue that precedes it, and the fenced stop that follows.
//!
//! The three are separate operations rather than one operation with a race, because they
//! are answerable at different times and by different parties. Before the scheduler claims
//! a child, the parent owns it and may remove it. After the claim, the child owns itself and
//! only a fence-carrying stop is admissible.

use aex_brain_domain::budget::{BudgetNode, Dimension};
use aex_brain_domain::child::{CancelCause, ChildOutcome, ChildState, QueuedReason};
use aex_brain_domain::ids::{AgentId, CancelEpoch, Fence};

use crate::kernel::sync::Arc;
use crate::kernel::{PermitKind, PermitSet, PermitSetFull, Reservation};

/// Everything one child claim writes, as one transaction.
///
/// It is one transaction on purpose. Checking `active_used + 1 <= active_limit` and then
/// incrementing it in a second write leaves a window in which two claimants both conclude
/// there is room, and the whole point of the durable active limit is that they cannot.
#[derive(Debug)]
pub struct ClaimPlan {
    /// Which child.
    pub child: AgentId,
    /// The fence the claim conditions on. A claim of a child that has moved on is refused.
    pub observed_fence: Fence,
    /// The fence the claim writes.
    pub next_fence: Fence,
    /// The session cancellation epoch the claim conditions on.
    pub cancel_epoch: CancelEpoch,
    /// The local permits held for the duration of the claim.
    ///
    /// Acquired **before** the transaction and dropped if it fails. A failed conditional
    /// claim that leaked a permit would shrink the process's capacity every time two
    /// schedulers raced, which is exactly when capacity matters.
    pub permits: Vec<Reservation>,
}

/// What a claim attempt decided.
#[derive(Debug)]
pub enum ClaimDecision {
    /// The transaction may run.
    Claim(Box<ClaimPlan>),
    /// The child stays queued, with the reason that will be visible on its row.
    Deferred {
        /// Which limit is binding.
        reason: QueuedReason,
    },
}

/// Why a claim was refused outright.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClaimRefusal {
    /// The child is not queued, so there is nothing to claim.
    #[error("child is {state:?}, not queued")]
    NotQueued {
        /// Where the child actually is.
        state: ChildState,
    },
    /// The session was cancelled after the caller read it.
    #[error("the cancellation epoch advanced")]
    CancelEpochAdvanced,
}

/// Plans one child claim, acquiring its local permits first.
///
/// # Errors
///
/// [`ClaimRefusal::NotQueued`] when the child has already been claimed or terminalized, and
/// [`ClaimRefusal::CancelEpochAdvanced`] when the session was cancelled after the scheduler
/// read it.
pub fn plan_claim(
    permits: &Arc<PermitSet>,
    child: AgentId,
    state: ChildState,
    observed_fence: Fence,
    read_epoch: CancelEpoch,
    current_epoch: CancelEpoch,
    session: &BudgetNode,
) -> Result<ClaimDecision, ClaimRefusal> {
    if !matches!(state, ChildState::Queued { .. }) {
        return Err(ClaimRefusal::NotQueued { state });
    }
    if read_epoch != current_epoch {
        return Err(ClaimRefusal::CancelEpochAdvanced);
    }
    if session.free(Dimension::ActiveChildren) == 0 {
        return Ok(ClaimDecision::Deferred {
            reason: QueuedReason::ActiveBudget,
        });
    }
    // Every permit the activation will need, in one fallible step. Acquiring them one at a
    // time would let a claim hold two of three and block a claim that could have used all
    // three.
    let mut held = Vec::with_capacity(2);
    for (kind, units) in [
        (PermitKind::Activation, 1_u64),
        (PermitKind::ProviderStream, 1),
    ] {
        match permits.acquire(kind, units) {
            Ok(reservation) => held.push(reservation),
            Err(full) => {
                // `held` drops here, releasing everything already taken. That is a `Drop`
                // impl rather than an explicit release precisely so this path cannot forget.
                return Ok(ClaimDecision::Deferred {
                    reason: permit_reason(&full),
                });
            }
        }
    }
    Ok(ClaimDecision::Claim(Box::new(ClaimPlan {
        child,
        observed_fence,
        next_fence: observed_fence.advance(),
        cancel_epoch: current_epoch,
        permits: held,
    })))
}

const fn permit_reason(full: &PermitSetFull) -> QueuedReason {
    match full.kind {
        PermitKind::ProviderStream => QueuedReason::ProviderPermits,
        PermitKind::HandsRpc => QueuedReason::HandsPermits,
        // The network lane keeps the reason its shared-pool predecessor already
        // reported. A vocabulary of its own would be a durable enum change for a
        // deferral no child claim can produce today.
        PermitKind::Activation
        | PermitKind::NetworkLane
        | PermitKind::ComputeLane
        | PermitKind::ContextBytes
        | PermitKind::WarmCacheBytes => QueuedReason::RegionalCapacity,
    }
}

/// What a dequeue writes.
///
/// A dequeue **preserves history**: it terminalizes the child as cancelled with the cause
/// recorded, rather than deleting the row. A parent that could make a child it created
/// disappear would leave a swarm whose totals never add up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DequeuePlan {
    /// Which child.
    pub child: AgentId,
    /// How it ends.
    pub outcome: ChildOutcome,
}

/// Plans a dequeue of an unclaimed child.
///
/// # Errors
///
/// [`ClaimRefusal::NotQueued`] once the scheduler has claimed the child. After that point
/// the operation the caller wants is a fenced stop, and they are not interchangeable.
pub const fn plan_dequeue(child: AgentId, state: ChildState) -> Result<DequeuePlan, ClaimRefusal> {
    if !matches!(state, ChildState::Queued { .. }) {
        return Err(ClaimRefusal::NotQueued { state });
    }
    Ok(DequeuePlan {
        child,
        outcome: ChildOutcome::Cancelled {
            cause: CancelCause::Dequeued,
        },
    })
}

/// Why a stop was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StopRefusal {
    /// The caller's fence is not the child's current one.
    #[error("stop carries fence {observed:?} but the child is at {current:?}")]
    StopFenced {
        /// What the caller observed.
        observed: Fence,
        /// What the child actually carries.
        current: Fence,
    },
    /// The child is already terminal.
    #[error("child is already terminal")]
    Terminal,
}

/// What a fenced stop writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StopPlan {
    /// Which child.
    pub child: AgentId,
    /// The fence the write conditions on.
    pub observed_fence: Fence,
}

/// Plans a fenced stop of a claimed child.
///
/// The flag is set; the child's *next* commit observes it and settles `Cancelled`. A
/// dispatched non-replayable effect settles honestly first, so a stop never fabricates a
/// clean cancellation over an ambiguous outcome.
///
/// # Errors
///
/// [`StopRefusal::StopFenced`] when the caller's fence is stale — a stop aimed at a previous
/// incarnation would cancel work the caller has never seen — and [`StopRefusal::Terminal`]
/// when the child has already ended.
pub const fn plan_stop(
    child: AgentId,
    state: ChildState,
    observed: Fence,
    current: Fence,
) -> Result<StopPlan, StopRefusal> {
    if state.is_terminal() {
        return Err(StopRefusal::Terminal);
    }
    if observed.0 != current.0 {
        return Err(StopRefusal::StopFenced { observed, current });
    }
    Ok(StopPlan {
        child,
        observed_fence: observed,
    })
}

#[cfg(test)]
mod tests {
    use super::{ClaimDecision, ClaimRefusal, StopRefusal, plan_claim, plan_dequeue, plan_stop};
    use crate::kernel::sync::Arc;
    use crate::kernel::{PermitKind, PermitSet};
    use aex_brain_domain::budget::{BudgetNode, Dimension, DimensionVector};
    use aex_brain_domain::child::{CancelCause, ChildOutcome, ChildState, QueuedReason};
    use aex_brain_domain::ids::{AgentId, CancelEpoch, Fence};
    use std::collections::BTreeMap;
    use uuid::Uuid;

    fn permits(activations: u64, streams: u64) -> Arc<PermitSet> {
        Arc::new(PermitSet::new(BTreeMap::from([
            (PermitKind::Activation, activations),
            (PermitKind::ProviderStream, streams),
        ])))
    }

    fn child() -> AgentId {
        AgentId(Uuid::from_u128(1))
    }

    fn session(free: u64) -> BudgetNode {
        let mut node = BudgetNode::root(DimensionVector::uniform(free));
        node.limit.set(Dimension::ActiveChildren, free);
        node
    }

    fn queued() -> ChildState {
        ChildState::Queued {
            reason: QueuedReason::ActiveBudget,
        }
    }

    #[test]
    fn a_claim_advances_the_fence_and_holds_its_permits() {
        let permits = permits(1, 1);
        let decision = plan_claim(
            &permits,
            child(),
            queued(),
            Fence(3),
            CancelEpoch(2),
            CancelEpoch(2),
            &session(4),
        )
        .expect("a queued child is claimable");
        let ClaimDecision::Claim(plan) = decision else {
            panic!("expected a claim");
        };
        assert_eq!(plan.observed_fence, Fence(3));
        assert_eq!(plan.next_fence, Fence(4));
        assert_eq!(plan.permits.len(), 2);
        assert_eq!(permits.held(PermitKind::Activation), 1);
    }

    /// The load-bearing one. A failed claim must leave capacity exactly as it found it, or
    /// two racing schedulers shrink the process every time they collide.
    #[test]
    fn a_deferred_claim_leaks_no_permit() {
        let permits = permits(1, 0);
        let decision = plan_claim(
            &permits,
            child(),
            queued(),
            Fence(0),
            CancelEpoch(0),
            CancelEpoch(0),
            &session(4),
        )
        .expect("a queued child is claimable");
        assert!(matches!(
            decision,
            ClaimDecision::Deferred {
                reason: QueuedReason::ProviderPermits
            }
        ));
        assert_eq!(
            permits.held(PermitKind::Activation),
            0,
            "the activation permit taken before the stream permit failed must be released"
        );
        assert_eq!(permits.held(PermitKind::ProviderStream), 0);
    }

    #[test]
    fn a_dropped_claim_returns_every_permit() {
        let permits = permits(1, 1);
        {
            let _decision = plan_claim(
                &permits,
                child(),
                queued(),
                Fence(0),
                CancelEpoch(0),
                CancelEpoch(0),
                &session(4),
            )
            .expect("claimable");
            assert_eq!(permits.held(PermitKind::Activation), 1);
        }
        assert_eq!(permits.held(PermitKind::Activation), 0);
        assert_eq!(permits.held(PermitKind::ProviderStream), 0);
    }

    #[test]
    fn an_exhausted_active_budget_defers_before_any_permit_is_taken() {
        let permits = permits(4, 4);
        let mut exhausted = session(1);
        exhausted.used.set(Dimension::ActiveChildren, 1);
        let decision = plan_claim(
            &permits,
            child(),
            queued(),
            Fence(0),
            CancelEpoch(0),
            CancelEpoch(0),
            &exhausted,
        )
        .expect("claimable");
        assert!(matches!(
            decision,
            ClaimDecision::Deferred {
                reason: QueuedReason::ActiveBudget
            }
        ));
        assert_eq!(permits.held(PermitKind::Activation), 0);
    }

    #[test]
    fn a_child_that_is_no_longer_queued_is_refused_rather_than_claimed_twice() {
        let permits = permits(4, 4);
        let error = plan_claim(
            &permits,
            child(),
            ChildState::Running,
            Fence(0),
            CancelEpoch(0),
            CancelEpoch(0),
            &session(4),
        )
        .expect_err("a running child is not claimable");
        assert_eq!(
            error,
            ClaimRefusal::NotQueued {
                state: ChildState::Running
            }
        );
    }

    #[test]
    fn a_cancelled_session_admits_no_new_claim() {
        let permits = permits(4, 4);
        let error = plan_claim(
            &permits,
            child(),
            queued(),
            Fence(0),
            CancelEpoch(1),
            CancelEpoch(2),
            &session(4),
        )
        .expect_err("the epoch moved");
        assert_eq!(error, ClaimRefusal::CancelEpochAdvanced);
    }

    /// Dequeue preserves history. A parent that could make a child disappear would leave a
    /// swarm whose totals never add up.
    #[test]
    fn a_dequeue_terminalizes_rather_than_deletes() {
        let plan = plan_dequeue(child(), queued()).expect("a queued child may be dequeued");
        assert_eq!(
            plan.outcome,
            ChildOutcome::Cancelled {
                cause: CancelCause::Dequeued
            }
        );
    }

    #[test]
    fn a_dequeue_after_the_claim_is_refused_because_stop_is_the_operation() {
        let error = plan_dequeue(child(), ChildState::Starting)
            .expect_err("after the claim the operation is a fenced stop");
        assert_eq!(
            error,
            ClaimRefusal::NotQueued {
                state: ChildState::Starting
            }
        );
    }

    /// A stop aimed at a previous incarnation would cancel work the caller never saw.
    #[test]
    fn a_stale_fence_is_rejected_rather_than_applied() {
        let error = plan_stop(child(), ChildState::Running, Fence(3), Fence(4))
            .expect_err("a stale fence is refused");
        assert_eq!(
            error,
            StopRefusal::StopFenced {
                observed: Fence(3),
                current: Fence(4)
            }
        );
        let plan = plan_stop(child(), ChildState::Running, Fence(4), Fence(4))
            .expect("the observed incarnation");
        assert_eq!(plan.observed_fence, Fence(4));
    }

    #[test]
    fn a_terminal_child_cannot_be_stopped_again() {
        assert_eq!(
            plan_stop(child(), ChildState::Completed, Fence(1), Fence(1)),
            Err(StopRefusal::Terminal)
        );
    }
}
