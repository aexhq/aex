//! True idle — H-IDLE.
//!
//! The timer starts only when Brain has no authoritative admitted, queued or open
//! Hands operation **and** no unexpired bounded paid keepalive lease. Guest
//! heartbeats, process lists, cgroup population and load average are diagnostic
//! only: with real root inside the VM none of them can be trusted, and a
//! traffic-only measure is exactly what H-IDLE removes.

use serde::{Deserialize, Serialize};

use crate::wire_pending::{GenerationId, KeepaliveLeaseId, Timestamp};

/// Proven quiescence, in milliseconds, before a generation is idle.
///
/// The boundary is exact: 179 999 ms is not idle, 180 000 ms is. Jitter belongs on
/// the evaluation schedule ([`TrueIdleEvidence::next_evaluate_at`]), never on this
/// threshold.
pub const TRUE_IDLE_THRESHOLD_MS: u64 = 180_000;

/// Maximum jitter added to an evaluation schedule.
pub const IDLE_EVALUATION_JITTER_MS: u64 = 15_000;

/// Launch default for the maximum keepalive lease duration. It never exceeds the
/// generation's remaining provider lifetime.
pub const KEEPALIVE_MAX_MS: u64 = 3_600_000;

/// A bounded, paid lease that keeps a generation alive while no AEX operation is open.
///
/// Stored on the generation head, where guest root cannot reach it. Extending it
/// requires a new AEX request; it cannot be forged or renewed from inside the VM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeepaliveLease {
    /// Identity of the issued lease.
    pub lease_id: KeepaliveLeaseId,
    /// When the lease stops holding the generation alive.
    pub expires_at: Timestamp,
}

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

impl KeepaliveLease {
    /// Issues a lease, refusing anything the effective policy or the remaining
    /// provider lifetime cannot honour.
    ///
    /// # Errors
    ///
    /// See [`KeepaliveRefused`].
    pub fn issue(
        lease_id: KeepaliveLeaseId,
        now: Timestamp,
        requested_ms: u64,
        maximum_ms: u64,
        remaining_lifetime_ms: u64,
    ) -> Result<Self, KeepaliveRefused> {
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
        Ok(Self {
            lease_id,
            expires_at: now.saturating_add_millis(requested_ms),
        })
    }

    /// Whether the lease still holds at `now`. Expiry is exclusive of the instant
    /// itself: a lease expiring exactly at `now` no longer holds.
    #[must_use]
    pub fn holds_at(&self, now: Timestamp) -> bool {
        self.expires_at > now
    }
}

/// The authoritative activity evidence a true-idle decision is taken from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrueIdleEvidence {
    /// The generation this evidence describes.
    pub generation: GenerationId,
    /// The activity revision the counters were read at.
    pub activity_revision: u64,
    /// Operations admitted on the generation head but not yet dispatched.
    pub admitted: u32,
    /// Operations queued behind admission.
    pub queued: u32,
    /// Operations dispatched and not yet settled.
    pub open: u32,
    /// The paid keepalive lease on the head, if any.
    pub keepalive_lease: Option<KeepaliveLease>,
    /// The last instant the generation was authoritatively busy.
    pub last_busy_at: Timestamp,
    /// When this evidence was read.
    pub observed_at: Timestamp,
}

impl TrueIdleEvidence {
    /// Total authoritative work holding the generation busy.
    #[must_use]
    pub const fn busy_count(&self) -> u32 {
        self.admitted
            .saturating_add(self.queued)
            .saturating_add(self.open)
    }

    /// The instant from which quiescence is measured, or `None` while any
    /// authoritative work is outstanding.
    #[must_use]
    pub fn idle_since(&self) -> Option<Timestamp> {
        if self.busy_count() > 0 {
            return None;
        }
        Some(match &self.keepalive_lease {
            Some(lease) if lease.expires_at > self.last_busy_at => lease.expires_at,
            _ => self.last_busy_at,
        })
    }

    /// The exact boundary: 179 999 ms is not idle, 180 000 ms is.
    #[must_use]
    pub fn is_true_idle(&self, now: Timestamp) -> bool {
        match self.idle_since() {
            None => false,
            Some(since) => now.saturating_millis_since(since) >= TRUE_IDLE_THRESHOLD_MS,
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
            since.saturating_add_millis(
                TRUE_IDLE_THRESHOLD_MS + jitter_ms.min(IDLE_EVALUATION_JITTER_MS),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        IDLE_EVALUATION_JITTER_MS, KeepaliveLease, KeepaliveRefused, TRUE_IDLE_THRESHOLD_MS,
        TrueIdleEvidence,
    };
    use crate::wire_pending::{GenerationId, KeepaliveLeaseId, Timestamp};
    use uuid::Uuid;

    const BUSY_AT: i64 = 1_000_000;

    fn evidence(admitted: u32, queued: u32, open: u32) -> TrueIdleEvidence {
        TrueIdleEvidence {
            generation: GenerationId::from_bytes([1; 16]),
            activity_revision: 7,
            admitted,
            queued,
            open,
            keepalive_lease: None,
            last_busy_at: Timestamp::from_millis(BUSY_AT),
            observed_at: Timestamp::from_millis(BUSY_AT),
        }
    }

    fn lease(expires_at: i64) -> KeepaliveLease {
        KeepaliveLease {
            lease_id: KeepaliveLeaseId::from_uuid(Uuid::from_bytes([2; 16])),
            expires_at: Timestamp::from_millis(expires_at),
        }
    }

    #[test]
    fn the_boundary_table_is_exhaustive_and_exact() {
        let idle = evidence(0, 0, 0);
        for (elapsed, expected) in [
            (0_u64, false),
            (179_998, false),
            (179_999, false),
            (180_000, true),
            (180_001, true),
            (u64::MAX, true),
        ] {
            let now = Timestamp::from_millis(BUSY_AT).saturating_add_millis(elapsed);
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
            let mut probe = evidence(0, 0, 0);
            probe.keepalive_lease = Some(lease(BUSY_AT + offset));
            for (elapsed, expected) in [
                (0_u64, false),
                (179_998, false),
                (179_999, false),
                (180_000, true),
                (180_001, true),
                (u64::MAX, true),
            ] {
                let now = Timestamp::from_millis(measured_from).saturating_add_millis(elapsed);
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
            let busy = evidence(admitted, queued, open);
            assert_eq!(busy.idle_since(), None);
            assert!(!busy.is_true_idle(Timestamp::from_millis(i64::MAX)));
            assert_eq!(busy.next_evaluate_at(0), None);
        }
    }

    #[test]
    fn idleness_is_monotone_in_elapsed_time() {
        let idle = evidence(0, 0, 0);
        let mut seen_true = false;
        for elapsed in 0_u64..200_000 {
            let now = Timestamp::from_millis(BUSY_AT).saturating_add_millis(elapsed);
            let verdict = idle.is_true_idle(now);
            if verdict {
                seen_true = true;
            }
            assert!(
                !(seen_true && !verdict),
                "idleness went back to false at {elapsed} ms"
            );
        }
        assert!(seen_true);
    }

    #[test]
    fn a_lease_that_outlives_the_last_busy_instant_moves_the_measurement_start() {
        let mut probe = evidence(0, 0, 0);
        probe.keepalive_lease = Some(lease(BUSY_AT + 60_000));
        assert_eq!(
            probe.idle_since(),
            Some(Timestamp::from_millis(BUSY_AT + 60_000))
        );
        assert!(!probe.is_true_idle(Timestamp::from_millis(BUSY_AT + 60_000 + 179_999)));
        assert!(probe.is_true_idle(Timestamp::from_millis(BUSY_AT + 60_000 + 180_000)));
    }

    #[test]
    fn an_already_expired_lease_never_moves_the_measurement_start() {
        let mut probe = evidence(0, 0, 0);
        probe.keepalive_lease = Some(lease(BUSY_AT - 500_000));
        assert_eq!(probe.idle_since(), Some(Timestamp::from_millis(BUSY_AT)));
    }

    #[test]
    fn the_schedule_carries_jitter_and_the_threshold_does_not() {
        let idle = evidence(0, 0, 0);
        assert_eq!(
            idle.next_evaluate_at(0),
            Some(Timestamp::from_millis(BUSY_AT + 180_000))
        );
        assert_eq!(
            idle.next_evaluate_at(15_000),
            Some(Timestamp::from_millis(BUSY_AT + 195_000))
        );
        // A caller that draws outside the range is clamped, not trusted.
        let clamped = i64::try_from(TRUE_IDLE_THRESHOLD_MS + IDLE_EVALUATION_JITTER_MS)
            .expect("the clamp fits an i64");
        assert_eq!(
            idle.next_evaluate_at(u64::MAX),
            Some(Timestamp::from_millis(BUSY_AT + clamped))
        );
        // The decision is unaffected by any jitter draw.
        for jitter in [0_u64, 1, 7_500, 15_000, u64::MAX] {
            let _ = idle.next_evaluate_at(jitter);
            assert!(!idle.is_true_idle(Timestamp::from_millis(BUSY_AT + 179_999)));
            assert!(idle.is_true_idle(Timestamp::from_millis(BUSY_AT + 180_000)));
        }
    }

    #[test]
    fn a_lease_is_refused_when_it_exceeds_policy_or_lifetime() {
        let id = KeepaliveLeaseId::from_uuid(Uuid::from_bytes([3; 16]));
        let now = Timestamp::from_millis(BUSY_AT);
        assert_eq!(
            KeepaliveLease::issue(id, now, 0, 3_600_000, 3_600_000),
            Err(KeepaliveRefused::ZeroLength)
        );
        assert_eq!(
            KeepaliveLease::issue(id, now, 3_600_001, 3_600_000, 28_800_000),
            Err(KeepaliveRefused::ExceedsPolicy {
                requested_ms: 3_600_001,
                maximum_ms: 3_600_000
            })
        );
        assert_eq!(
            KeepaliveLease::issue(id, now, 3_600_000, 3_600_000, 60_000),
            Err(KeepaliveRefused::ExceedsLifetime {
                requested_ms: 3_600_000,
                remaining_ms: 60_000
            })
        );
        let granted = KeepaliveLease::issue(id, now, 3_600_000, 3_600_000, 28_800_000)
            .expect("a within-policy lease is granted");
        assert_eq!(
            granted.expires_at,
            Timestamp::from_millis(BUSY_AT + 3_600_000)
        );
        assert!(granted.holds_at(Timestamp::from_millis(BUSY_AT + 3_599_999)));
        assert!(!granted.holds_at(Timestamp::from_millis(BUSY_AT + 3_600_000)));
    }
}
