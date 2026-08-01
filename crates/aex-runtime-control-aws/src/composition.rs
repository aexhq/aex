//! The suspend transition and the worker's decision surface.
//!
//! The transition is owned entirely by `runtime-control-worker` and competes with
//! Brain through **one** durable suspend/admission fence. That is what makes a
//! worker outage safe: it delays cost saving and raises an alarm, and it can never
//! pause an authority-open background job, because only the worker suspends and it
//! always takes the fence first.

use aex_hands_protocol::rpc::Fence;
use aex_runtime_control::clock::plus_millis;
use aex_runtime_control::generation::{GenerationHead, GenerationState, Revision, next_fence};
use aex_runtime_control::idle::{IdleAssessment, lease_holds_at};
use aex_runtime_control::lifecycle::{Lifetime, LifetimeVerdict, ProviderState};
use aex_wire::types::Timestamp;

/// How long the worker's suspend lock is leased for.
pub const SUSPEND_LOCK_MS: u64 = 60_000;

/// Why a suspend evaluation held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HoldReason {
    /// Authoritative work is outstanding.
    Busy,
    /// A paid keepalive lease still holds.
    KeepaliveLeased,
    /// Quiescence has not reached the exact threshold.
    NotYetIdle,
    /// The head is not in a suspendable state.
    NotRunning,
    /// The provider state is not `RUNNING`.
    ProviderNotRunning,
    /// Another worker already holds the suspend lock.
    LockHeld,
}

/// What one suspend evaluation decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuspendDecision {
    /// Take the lock, move to `suspending`, and recount before doing anything else.
    TakeLock {
        /// The fence the transition takes.
        next_fence: Fence,
        /// The revision the conditional write is conditional on.
        expected_revision: Revision,
        /// When the lease lapses.
        lock_expires_at: Timestamp,
    },
    /// The recount disagreed with the counter. Repair it, restore `running`, rearm
    /// the boundary, and suspend nothing on this pass.
    RepairCounter {
        /// What the authority says is really open.
        authoritative_open: u32,
        /// What the head claimed.
        recorded_open: u32,
    },
    /// Nothing to do: busy, leased, or not in a suspendable state.
    Hold {
        /// Why.
        reason: HoldReason,
    },
    /// The generation is inside its lifetime drain margin. Stop admitting.
    Drain {
        /// Remaining provider lifetime.
        remaining_ms: u64,
    },
    /// The generation is inside its terminate margin. Terminate and close cleanly.
    Terminate {
        /// Remaining provider lifetime.
        remaining_ms: u64,
    },
}

/// Decides what one suspend evaluation should do.
///
/// The order is deliberate. Lifetime comes first, because a generation about to be
/// hard-stopped by the provider must drain and terminate rather than take a
/// snapshot that will be retired seconds later. Then the lock, then liveness, then
/// the exact idle boundary.
#[must_use]
pub fn evaluate_suspend(
    head: &GenerationHead,
    assessment: &IdleAssessment,
    lifetime: Lifetime,
    provider: ProviderState,
    now: Timestamp,
) -> SuspendDecision {
    match lifetime.verdict(now) {
        LifetimeVerdict::Terminate { remaining_ms } => {
            return SuspendDecision::Terminate { remaining_ms };
        }
        LifetimeVerdict::Drain { remaining_ms } => {
            return SuspendDecision::Drain { remaining_ms };
        }
        LifetimeVerdict::Continue { .. } => {}
    }
    if head.state != GenerationState::Running {
        return SuspendDecision::Hold {
            reason: HoldReason::NotRunning,
        };
    }
    if let Some(until) = head.suspend_lock_expires_at
        && until > now
    {
        return SuspendDecision::Hold {
            reason: HoldReason::LockHeld,
        };
    }
    if provider != ProviderState::Running {
        return SuspendDecision::Hold {
            reason: HoldReason::ProviderNotRunning,
        };
    }
    if assessment.busy_count() > 0 {
        return SuspendDecision::Hold {
            reason: HoldReason::Busy,
        };
    }
    if let Some(lease) = &assessment.evidence.keepalive_lease
        && lease_holds_at(lease, now)
    {
        return SuspendDecision::Hold {
            reason: HoldReason::KeepaliveLeased,
        };
    }
    if !assessment.is_true_idle(now) {
        return SuspendDecision::Hold {
            reason: HoldReason::NotYetIdle,
        };
    }
    SuspendDecision::TakeLock {
        next_fence: next_fence(head.fence),
        expected_revision: head.revision,
        lock_expires_at: plus_millis(now, SUSPEND_LOCK_MS),
    }
}

/// The authoritative recount, run **after** the lock is taken and before any
/// provider call.
///
/// The counter on the head is a fence and a fast path; the authority is Brain's own
/// open Hands effects. When they disagree the counter is repaired and nothing is
/// suspended on that pass, because acting on a number that was just proven wrong is
/// how a running job gets snapshotted.
#[must_use]
pub fn recount(head: &GenerationHead, authoritative_open: u32) -> SuspendDecision {
    if authoritative_open == head.open_operations {
        SuspendDecision::TakeLock {
            next_fence: head.fence,
            expected_revision: head.revision,
            lock_expires_at: head.last_busy_at,
        }
    } else {
        SuspendDecision::RepairCounter {
            authoritative_open,
            recorded_open: head.open_operations,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{HoldReason, SUSPEND_LOCK_MS, SuspendDecision, evaluate_suspend, recount};
    use aex_hands_protocol::lifecycle::{KeepaliveLease, TrueIdleEvidence};
    use aex_hands_protocol::rpc::Fence;
    use aex_runtime_control::clock::{minus_millis, plus_millis};
    use aex_runtime_control::generation::{GenerationHead, GenerationState, Revision};
    use aex_runtime_control::idle::IdleAssessment;
    use aex_runtime_control::lifecycle::{
        LIFETIME_DRAIN_MARGIN_MS, LIFETIME_TERMINATE_MARGIN_MS, Lifetime, ProviderState,
    };
    use aex_wire::ids::{GenerationId, PrefixedId as _, Uuid7};
    use aex_wire::types::{ComputeSize, Timestamp};

    const BUSY_AT: i64 = 1_000_000;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("a bounded instant")
    }

    fn generation() -> GenerationId {
        GenerationId::from_uuid7(Uuid7::compose(1, [1; 10]))
    }

    fn head(state: GenerationState, open: u32) -> GenerationHead {
        GenerationHead {
            generation: generation(),
            size: ComputeSize::Gb1,
            state,
            fence: Fence(3),
            revision: Revision::new(11),
            open_operations: open,
            last_busy_at: at(BUSY_AT),
            idle_since: None,
            suspend_lock_expires_at: None,
            keepalive_lease: None,
            transport_mode: None,
        }
    }

    fn assessment(open: u32) -> IdleAssessment {
        IdleAssessment {
            evidence: TrueIdleEvidence {
                generation: generation(),
                activity_revision: 1,
                admitted: 0,
                queued: 0,
                open,
                keepalive_lease: None,
                observed_at: at(BUSY_AT),
            },
            last_busy_at: at(BUSY_AT),
        }
    }

    /// A lifetime with plenty of headroom at the instants these tests use.
    fn lifetime() -> Lifetime {
        Lifetime {
            launched_at: at(BUSY_AT),
        }
    }

    #[test]
    fn the_exact_boundary_decides_the_suspend() {
        let head = head(GenerationState::Running, 0);
        let idle = assessment(0);
        assert_eq!(
            evaluate_suspend(
                &head,
                &idle,
                lifetime(),
                ProviderState::Running,
                at(BUSY_AT + 179_999)
            ),
            SuspendDecision::Hold {
                reason: HoldReason::NotYetIdle
            },
            "179999 ms is not idle"
        );
        let boundary = at(BUSY_AT + 180_000);
        assert_eq!(
            evaluate_suspend(&head, &idle, lifetime(), ProviderState::Running, boundary),
            SuspendDecision::TakeLock {
                next_fence: Fence(4),
                expected_revision: Revision::new(11),
                lock_expires_at: plus_millis(boundary, SUSPEND_LOCK_MS),
            },
            "180000 ms is"
        );
    }

    #[test]
    fn an_open_background_operation_blocks_suspension_indefinitely() {
        let head = head(GenerationState::Running, 1);
        let busy = assessment(1);
        for elapsed in [180_000_i64, 3_600_000, 20_000_000] {
            assert_eq!(
                evaluate_suspend(
                    &head,
                    &busy,
                    lifetime(),
                    ProviderState::Running,
                    at(BUSY_AT + elapsed)
                ),
                SuspendDecision::Hold {
                    reason: HoldReason::Busy
                },
                "elapsed {elapsed} ms"
            );
        }
    }

    #[test]
    fn a_keepalive_lease_blocks_and_then_releases() {
        let head = head(GenerationState::Running, 0);
        let mut leased = assessment(0);
        leased.evidence.keepalive_lease = Some(KeepaliveLease {
            lease_id: "kal_1".to_owned(),
            expires_at: at(BUSY_AT + 600_000),
        });
        assert_eq!(
            evaluate_suspend(
                &head,
                &leased,
                lifetime(),
                ProviderState::Running,
                at(BUSY_AT + 599_999)
            ),
            SuspendDecision::Hold {
                reason: HoldReason::KeepaliveLeased
            }
        );
        // The lease lapses, and quiescence is then measured from its expiry.
        assert_eq!(
            evaluate_suspend(
                &head,
                &leased,
                lifetime(),
                ProviderState::Running,
                at(BUSY_AT + 600_000 + 179_999)
            ),
            SuspendDecision::Hold {
                reason: HoldReason::NotYetIdle
            }
        );
        assert!(matches!(
            evaluate_suspend(
                &head,
                &leased,
                lifetime(),
                ProviderState::Running,
                at(BUSY_AT + 600_000 + 180_000)
            ),
            SuspendDecision::TakeLock { .. }
        ));
    }

    #[test]
    fn a_held_lock_and_a_non_running_provider_both_hold() {
        let mut locked = head(GenerationState::Running, 0);
        locked.suspend_lock_expires_at = Some(at(BUSY_AT + 200_000));
        assert_eq!(
            evaluate_suspend(
                &locked,
                &assessment(0),
                lifetime(),
                ProviderState::Running,
                at(BUSY_AT + 180_000)
            ),
            SuspendDecision::Hold {
                reason: HoldReason::LockHeld
            }
        );
        assert_eq!(
            evaluate_suspend(
                &head(GenerationState::Running, 0),
                &assessment(0),
                lifetime(),
                ProviderState::Suspending,
                at(BUSY_AT + 180_000)
            ),
            SuspendDecision::Hold {
                reason: HoldReason::ProviderNotRunning
            }
        );
    }

    #[test]
    fn the_lifetime_margins_outrank_every_other_reason() {
        let idle = assessment(0);
        let head = head(GenerationState::Running, 0);
        let expiry = lifetime().expires_at();

        let draining = minus_millis(expiry, LIFETIME_DRAIN_MARGIN_MS);
        assert_eq!(
            evaluate_suspend(&head, &idle, lifetime(), ProviderState::Running, draining),
            SuspendDecision::Drain {
                remaining_ms: LIFETIME_DRAIN_MARGIN_MS
            }
        );

        let terminating = minus_millis(expiry, LIFETIME_TERMINATE_MARGIN_MS);
        assert_eq!(
            evaluate_suspend(
                &head,
                &idle,
                lifetime(),
                ProviderState::Running,
                terminating
            ),
            SuspendDecision::Terminate {
                remaining_ms: LIFETIME_TERMINATE_MARGIN_MS
            },
            "a generation about to be hard-stopped must not snapshot instead"
        );
    }

    #[test]
    fn a_stale_over_count_is_repaired_and_nothing_suspends_on_that_pass() {
        let stale = head(GenerationState::Suspending, 3);
        assert_eq!(
            recount(&stale, 0),
            SuspendDecision::RepairCounter {
                authoritative_open: 0,
                recorded_open: 3
            }
        );
        // An agreeing recount proceeds.
        assert!(matches!(
            recount(&head(GenerationState::Suspending, 0), 0),
            SuspendDecision::TakeLock { .. }
        ));
    }

    #[test]
    fn a_worker_outage_pauses_nothing() {
        // The only thing that produces a suspend is `evaluate_suspend`. If the
        // worker never runs, no decision is taken at all, so an authority-open
        // background job keeps running: the fence is only ever taken by the worker.
        let busy = assessment(2);
        assert!(busy.busy_count() > 0);
        assert!(
            !busy.is_true_idle(plus_millis(at(BUSY_AT), u64::MAX)),
            "not even at the end of representable time"
        );
    }
}
