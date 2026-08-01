//! True idle — H-IDLE.
//!
//! The timer starts only when Brain has no authoritative admitted, queued or open
//! Hands operation **and** no unexpired bounded paid keepalive lease. Guest
//! heartbeats, process lists, cgroup population and load average are diagnostic
//! only: with real root inside the VM none of them can be trusted, and a
//! traffic-only measure is exactly what H-IDLE removes.
//!
//! [`aex_hands_protocol::TrueIdleEvidence`] is the transported counter snapshot,
//! and its own `is_true_idle` answers only "is anything outstanding right now".
//! The *decision* — has quiescence lasted 180000 ms — needs one value the wire
//! evidence does not carry, `last_busy_at`, which lives on the generation head
//! where guest root cannot reach it. [`IdleAssessment`] is that pairing, and it
//! owns the boundary.

use aex_hands_protocol::lifecycle::{KeepaliveLease, TrueIdleEvidence};
use aex_wire::types::Timestamp;
use serde::{Deserialize, Serialize};

use crate::clock::{millis_between, plus_millis};
use crate::generation::GenerationHead;

/// Proven quiescence, in milliseconds, before a generation is idle.
///
/// The boundary is exact: 179999 ms is not idle, 180000 ms is. Jitter belongs on
/// the evaluation schedule ([`IdleAssessment::next_evaluate_at`]), never on this
/// threshold.
pub const TRUE_IDLE_THRESHOLD_MS: u64 = 180_000;

/// Maximum jitter added to an evaluation schedule.
pub const IDLE_EVALUATION_JITTER_MS: u64 = 15_000;

/// Launch default for the maximum keepalive lease duration. It never exceeds the
/// generation's remaining provider lifetime.
pub const KEEPALIVE_MAX_MS: u64 = 3_600_000;

/// Why a keepalive lease request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum KeepaliveRefused {
    /// The requested duration exceeds the effective policy maximum.
    #[error("keepalive of {requested_ms} ms exceeds the effective maximum of {maximum_ms} ms")]
    ExceedsPolicy {
        /// Requested duration in milliseconds.
        requested_ms: u64,
        /// Effective policy maximum in milliseconds.
        maximum_ms: u64,
    },
    /// The requested duration outlives the generation's provider lifetime.
    #[error(
        "keepalive of {requested_ms} ms outlives the {remaining_ms} ms of provider lifetime left"
    )]
    ExceedsLifetime {
        /// Requested duration in milliseconds.
        requested_ms: u64,
        /// Provider lifetime remaining in milliseconds.
        remaining_ms: u64,
    },
    /// A zero-length lease holds nothing alive and is a caller bug.
    #[error("a keepalive lease must be longer than zero milliseconds")]
    ZeroLength,
}

/// Issues a keepalive lease, refusing anything the effective policy or the
/// remaining provider lifetime cannot honour.
///
/// A lease is stored on the generation head, where guest root cannot reach it.
/// Extending one requires a new AEX request; it cannot be forged or renewed from
/// inside the VM, which is why issuance is a pure function of AEX-held inputs and
/// takes nothing the guest can influence.
///
/// # Errors
///
/// See [`KeepaliveRefused`].
pub fn issue_keepalive(
    lease_id: impl Into<String>,
    now: Timestamp,
    requested_ms: u64,
    maximum_ms: u64,
    remaining_lifetime_ms: u64,
) -> Result<KeepaliveLease, KeepaliveRefused> {
    if requested_ms == 0 {
        return Err(KeepaliveRefused::ZeroLength);
    }
    if requested_ms > maximum_ms {
        return Err(KeepaliveRefused::ExceedsPolicy {
            requested_ms,
            maximum_ms,
        });
    }
    if requested_ms > remaining_lifetime_ms {
        return Err(KeepaliveRefused::ExceedsLifetime {
            requested_ms,
            remaining_ms: remaining_lifetime_ms,
        });
    }
    Ok(KeepaliveLease {
        lease_id: lease_id.into(),
        expires_at: plus_millis(now, requested_ms),
    })
}

/// Whether the lease still holds at `now`.
///
/// Expiry is exclusive of the instant itself: a lease expiring exactly at `now`
/// no longer holds.
#[must_use]
pub fn lease_holds_at(lease: &KeepaliveLease, now: Timestamp) -> bool {
    lease.expires_at > now
}

/// The authoritative activity evidence a true-idle decision is taken from.
///
/// `last_busy_at` is deliberately not part of the wire evidence: it is the
/// generation head's own record of when AEX last had work, and pairing it here
/// keeps the transported snapshot free of a value a guest could try to influence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IdleAssessment {
    /// The counter snapshot Brain published.
    pub evidence: TrueIdleEvidence,
    /// The last instant the generation was authoritatively busy.
    pub last_busy_at: Timestamp,
}

impl IdleAssessment {
    /// The assessment a lifecycle evaluation takes straight off a generation head.
    ///
    /// The head carries the settled counter, so `admitted` and `queued` are zero
    /// here by construction: those two are Brain's in-flight counts, which reach the
    /// head only by being folded into `open_operations` on admission. Reading them
    /// as anything else would double-count the same work.
    #[must_use]
    pub fn from_head(head: &GenerationHead, observed_at: Timestamp) -> Self {
        Self {
            evidence: TrueIdleEvidence {
                generation: head.generation,
                activity_revision: head.revision.value(),
                admitted: 0,
                queued: 0,
                open: head.open_operations,
                keepalive_lease: head.keepalive_lease.clone(),
                observed_at,
            },
            last_busy_at: head.last_busy_at,
        }
    }

    /// Total authoritative work holding the generation busy.
    #[must_use]
    pub const fn busy_count(&self) -> u32 {
        self.evidence
            .admitted
            .saturating_add(self.evidence.queued)
            .saturating_add(self.evidence.open)
    }

    /// The instant from which quiescence is measured, or `None` while any
    /// authoritative work is outstanding.
    #[must_use]
    pub fn idle_since(&self) -> Option<Timestamp> {
        if self.busy_count() > 0 {
            return None;
        }
        Some(match &self.evidence.keepalive_lease {
            Some(lease) if lease.expires_at > self.last_busy_at => lease.expires_at,
            _ => self.last_busy_at,
        })
    }

    /// The exact boundary: 179999 ms is not idle, 180000 ms is.
    #[must_use]
    pub fn is_true_idle(&self, now: Timestamp) -> bool {
        match self.idle_since() {
            None => false,
            Some(since) => millis_between(since, now) >= TRUE_IDLE_THRESHOLD_MS,
        }
    }

    /// When the reaper should next look at this generation.
    ///
    /// Jitter is applied here — to the *schedule* — so the decision itself stays
    /// deterministic and exactly testable. `jitter_ms` is the caller's draw from
    /// `uniform(0, IDLE_EVALUATION_JITTER_MS)`; it is clamped rather than trusted.
    #[must_use]
    pub fn next_evaluate_at(&self, jitter_ms: u64) -> Option<Timestamp> {
        self.idle_since().map(|since| {
            plus_millis(
                since,
                TRUE_IDLE_THRESHOLD_MS + jitter_ms.min(IDLE_EVALUATION_JITTER_MS),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        IDLE_EVALUATION_JITTER_MS, IdleAssessment, KeepaliveRefused, TRUE_IDLE_THRESHOLD_MS,
        issue_keepalive, lease_holds_at,
    };
    use crate::clock::plus_millis;
    use crate::generation::{GenerationHead, GenerationState, Revision};
    use aex_hands_protocol::lifecycle::{KeepaliveLease, TrueIdleEvidence};
    use aex_hands_protocol::rpc::Fence;
    use aex_wire::ids::{GenerationId, PrefixedId as _, Uuid7};
    use aex_wire::types::{ComputeSize, Timestamp};
    use proptest::prelude::{Just, any, prop_oneof, proptest};

    const BUSY_AT: i64 = 1_000_000;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("a bounded instant")
    }

    fn generation() -> GenerationId {
        GenerationId::from_uuid7(Uuid7::compose(1, [1; 10]))
    }

    fn head(open: u32, lease: Option<KeepaliveLease>) -> GenerationHead {
        GenerationHead {
            generation: generation(),
            size: ComputeSize::Gb1,
            state: GenerationState::Running,
            fence: Fence(2),
            revision: Revision::new(5),
            open_operations: open,
            last_busy_at: at(BUSY_AT),
            idle_since: None,
            suspend_lock_expires_at: None,
            keepalive_lease: lease,
            transport_mode: None,
        }
    }

    #[test]
    fn an_assessment_read_off_a_head_decides_the_same_boundary() {
        let idle = IdleAssessment::from_head(&head(0, None), at(BUSY_AT));
        assert_eq!(idle.evidence.admitted, 0);
        assert_eq!(idle.evidence.queued, 0);
        assert_eq!(idle.evidence.open, 0);
        assert_eq!(idle.last_busy_at, at(BUSY_AT));
        assert!(!idle.is_true_idle(at(BUSY_AT + 179_999)));
        assert!(idle.is_true_idle(at(BUSY_AT + 180_000)));

        let busy = IdleAssessment::from_head(&head(1, None), at(BUSY_AT));
        assert_eq!(busy.busy_count(), 1);
        assert!(!busy.is_true_idle(at(BUSY_AT + 3_600_000)));
    }

    #[test]
    fn a_head_held_lease_travels_into_the_assessment() {
        let lease = KeepaliveLease {
            lease_id: "kal_head".to_owned(),
            expires_at: at(BUSY_AT + 600_000),
        };
        let leased = IdleAssessment::from_head(&head(0, Some(lease.clone())), at(BUSY_AT));
        assert_eq!(leased.evidence.keepalive_lease, Some(lease));
        assert_eq!(leased.idle_since(), Some(at(BUSY_AT + 600_000)));
    }

    fn assessment(admitted: u32, queued: u32, open: u32) -> IdleAssessment {
        IdleAssessment {
            evidence: TrueIdleEvidence {
                generation: generation(),
                activity_revision: 7,
                admitted,
                queued,
                open,
                keepalive_lease: None,
                observed_at: at(BUSY_AT),
            },
            last_busy_at: at(BUSY_AT),
        }
    }

    fn lease(expires_at: i64) -> KeepaliveLease {
        KeepaliveLease {
            lease_id: "kal_test".to_owned(),
            expires_at: at(expires_at),
        }
    }

    #[test]
    fn the_boundary_table_is_exhaustive_and_exact() {
        let idle = assessment(0, 0, 0);
        for (elapsed, expected) in [
            (0_u64, false),
            (179_998, false),
            (179_999, false),
            (180_000, true),
            (180_001, true),
            (u64::MAX, true),
        ] {
            let now = plus_millis(at(BUSY_AT), elapsed);
            assert_eq!(
                idle.is_true_idle(now),
                expected,
                "elapsed {elapsed} ms must be idle={expected}"
            );
        }
    }

    #[test]
    fn the_same_boundary_holds_with_a_lease_expiring_either_side_of_the_last_busy_instant() {
        for (offset, measured_from) in [(-1_i64, BUSY_AT), (1, BUSY_AT + 1)] {
            let mut probe = assessment(0, 0, 0);
            probe.evidence.keepalive_lease = Some(lease(BUSY_AT + offset));
            for (elapsed, expected) in [
                (0_u64, false),
                (179_998, false),
                (179_999, false),
                (180_000, true),
                (180_001, true),
                (u64::MAX, true),
            ] {
                let now = plus_millis(at(measured_from), elapsed);
                assert_eq!(
                    probe.is_true_idle(now),
                    expected,
                    "lease offset {offset}, elapsed {elapsed} ms"
                );
            }
        }
    }

    #[test]
    fn any_outstanding_work_dominates_regardless_of_elapsed_time() {
        for (admitted, queued, open) in [(1, 0, 0), (0, 1, 0), (0, 0, 1), (5, 5, 5)] {
            let busy = assessment(admitted, queued, open);
            assert_eq!(busy.idle_since(), None);
            assert!(!busy.is_true_idle(plus_millis(at(BUSY_AT), u64::MAX)));
            assert_eq!(busy.next_evaluate_at(0), None);
        }
    }

    #[test]
    fn a_lease_that_outlives_the_last_busy_instant_moves_the_measurement_start() {
        let mut probe = assessment(0, 0, 0);
        probe.evidence.keepalive_lease = Some(lease(BUSY_AT + 60_000));
        assert_eq!(probe.idle_since(), Some(at(BUSY_AT + 60_000)));
        assert!(!probe.is_true_idle(at(BUSY_AT + 60_000 + 179_999)));
        assert!(probe.is_true_idle(at(BUSY_AT + 60_000 + 180_000)));
    }

    #[test]
    fn an_already_expired_lease_never_moves_the_measurement_start() {
        let mut probe = assessment(0, 0, 0);
        probe.evidence.keepalive_lease = Some(lease(BUSY_AT - 500_000));
        assert_eq!(probe.idle_since(), Some(at(BUSY_AT)));
    }

    #[test]
    fn the_schedule_carries_jitter_and_the_threshold_does_not() {
        let idle = assessment(0, 0, 0);
        assert_eq!(idle.next_evaluate_at(0), Some(at(BUSY_AT + 180_000)));
        assert_eq!(idle.next_evaluate_at(15_000), Some(at(BUSY_AT + 195_000)));
        // A caller that draws outside the range is clamped, not trusted.
        let clamped = i64::try_from(TRUE_IDLE_THRESHOLD_MS + IDLE_EVALUATION_JITTER_MS)
            .expect("the clamp fits an i64");
        assert_eq!(
            idle.next_evaluate_at(u64::MAX),
            Some(at(BUSY_AT + clamped)),
            "a hostile jitter draw is clamped to the schedule window, never applied raw"
        );
        // The decision is unaffected by any jitter draw.
        for jitter in [0_u64, 1, 7_500, 15_000, u64::MAX] {
            let _ = idle.next_evaluate_at(jitter);
            assert!(!idle.is_true_idle(at(BUSY_AT + 179_999)));
            assert!(idle.is_true_idle(at(BUSY_AT + 180_000)));
        }
    }

    #[test]
    fn a_lease_is_refused_when_it_exceeds_policy_or_lifetime() {
        let now = at(BUSY_AT);
        assert_eq!(
            issue_keepalive("kal_1", now, 0, 3_600_000, 3_600_000),
            Err(KeepaliveRefused::ZeroLength)
        );
        assert_eq!(
            issue_keepalive("kal_1", now, 3_600_001, 3_600_000, 28_800_000),
            Err(KeepaliveRefused::ExceedsPolicy {
                requested_ms: 3_600_001,
                maximum_ms: 3_600_000
            })
        );
        assert_eq!(
            issue_keepalive("kal_1", now, 3_600_000, 3_600_000, 60_000),
            Err(KeepaliveRefused::ExceedsLifetime {
                requested_ms: 3_600_000,
                remaining_ms: 60_000
            })
        );
        let granted = issue_keepalive("kal_1", now, 3_600_000, 3_600_000, 28_800_000)
            .expect("a within-policy lease is granted");
        assert_eq!(granted.expires_at, at(BUSY_AT + 3_600_000));
        assert!(lease_holds_at(&granted, at(BUSY_AT + 3_599_999)));
        assert!(!lease_holds_at(&granted, at(BUSY_AT + 3_600_000)));
    }

    proptest! {
        /// Idleness is monotone in elapsed time: once true it never returns to false.
        #[test]
        fn idleness_is_monotone_in_elapsed_time(
            first in 0_u64..400_000,
            extra in 0_u64..400_000,
        ) {
            let idle = assessment(0, 0, 0);
            let earlier = idle.is_true_idle(plus_millis(at(BUSY_AT), first));
            let later = idle.is_true_idle(plus_millis(at(BUSY_AT), first + extra));
            proptest::prop_assert!(!earlier || later);
            proptest::prop_assert_eq!(earlier, first >= 180_000);
        }

        /// Any non-zero admitted/queued/open count dominates every elapsed time.
        #[test]
        fn busy_dominates_every_elapsed_time(
            admitted in 0_u32..8,
            queued in 0_u32..8,
            open in 0_u32..8,
            elapsed in any::<u64>(),
        ) {
            let probe = assessment(admitted, queued, open);
            let verdict = probe.is_true_idle(plus_millis(at(BUSY_AT), elapsed));
            if admitted + queued + open > 0 {
                proptest::prop_assert!(!verdict);
            }
        }

        /// The schedule always lands at or after the threshold and never beyond
        /// threshold plus the jitter window, whatever the caller draws.
        #[test]
        fn the_schedule_stays_inside_its_window(
            jitter in prop_oneof![Just(0_u64), Just(u64::MAX), 0_u64..60_000],
        ) {
            let idle = assessment(0, 0, 0);
            let scheduled = idle.next_evaluate_at(jitter).expect("an idle generation schedules");
            let low = at(BUSY_AT + 180_000);
            let high = at(BUSY_AT + 195_000);
            proptest::prop_assert!(scheduled >= low);
            proptest::prop_assert!(scheduled <= high);
        }
    }
}
