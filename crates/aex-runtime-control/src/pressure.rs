//! Regional memory pressure release.
//!
//! Suspension preserves guest state but still consumes provider memory quota: AWS
//! counts running *and* suspended memory against the regional pool. When
//! utilization reaches the high-water mark, the worker terminates persisted
//! true-idle generations in rank order until utilization is below the low-water
//! mark. A running or keepalive-leased generation is never terminated for pressure.

use serde::{Deserialize, Serialize};

use crate::generation::GenerationState;
use crate::idle::TRUE_IDLE_THRESHOLD_MS;
use crate::wire_pending::{GenerationId, Timestamp};

/// Utilization at which pressure release starts.
pub const PRESSURE_HIGH_WATER: u32 = 850;
/// Utilization at which pressure release stops.
pub const PRESSURE_LOW_WATER: u32 = 750;

/// One candidate for pressure release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PressureCandidate {
    /// The generation.
    pub generation: GenerationId,
    /// Its lifecycle state.
    pub state: GenerationState,
    /// Proven idle milliseconds.
    pub idle_ms: u64,
    /// Retained snapshot bytes, which is the memory quota it holds.
    pub snapshot_bytes: u64,
    /// When the owning session was last active.
    pub session_last_active: Timestamp,
    /// Whether a paid keepalive lease still holds.
    pub keepalive_held: bool,
}

impl PressureCandidate {
    /// Only a suspended generation past the exact true-idle threshold, with no
    /// live keepalive lease, may be terminated for pressure.
    #[must_use]
    pub fn is_eligible(&self) -> bool {
        self.state == GenerationState::Suspended
            && self.idle_ms >= TRUE_IDLE_THRESHOLD_MS
            && !self.keepalive_held
    }

    /// The total order pressure release ranks by:
    /// `(idle_ms desc, snapshot_bytes desc, session_last_active asc)`, with the
    /// generation id as the final tie-break so the order is total rather than
    /// merely deterministic-per-run.
    fn rank_key(&self) -> (core::cmp::Reverse<u64>, core::cmp::Reverse<u64>, i64, uuid::Uuid) {
        (
            core::cmp::Reverse(self.idle_ms),
            core::cmp::Reverse(self.snapshot_bytes),
            self.session_last_active.millis(),
            self.generation.uuid(),
        )
    }
}

/// A pressure-release plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PressurePlan {
    /// Generations to terminate, in the order they are terminated.
    pub terminate: Vec<GenerationId>,
    /// Bytes the plan releases.
    pub released_bytes: u64,
    /// Projected utilization in per-mille once the plan completes.
    pub projected_utilization: u32,
}

/// Ranks and selects generations to terminate.
///
/// `used_bytes` and `pool_bytes` are the regional memory pool's counters. Nothing
/// is selected below the high-water mark, and selection stops as soon as projected
/// utilization is below the low-water mark.
#[must_use]
pub fn plan_release(
    candidates: &[PressureCandidate],
    used_bytes: u64,
    pool_bytes: u64,
) -> PressurePlan {
    let utilization = per_mille(used_bytes, pool_bytes);
    if utilization < PRESSURE_HIGH_WATER {
        return PressurePlan {
            terminate: Vec::new(),
            released_bytes: 0,
            projected_utilization: utilization,
        };
    }
    let mut eligible: Vec<&PressureCandidate> = candidates
        .iter()
        .filter(|candidate| candidate.is_eligible())
        .collect();
    eligible.sort_by_key(|candidate| candidate.rank_key());

    let mut terminate = Vec::new();
    let mut released_bytes = 0_u64;
    for candidate in eligible {
        if per_mille(used_bytes.saturating_sub(released_bytes), pool_bytes) < PRESSURE_LOW_WATER {
            break;
        }
        terminate.push(candidate.generation);
        released_bytes = released_bytes.saturating_add(candidate.snapshot_bytes);
    }
    PressurePlan {
        projected_utilization: per_mille(used_bytes.saturating_sub(released_bytes), pool_bytes),
        terminate,
        released_bytes,
    }
}

fn per_mille(used: u64, pool: u64) -> u32 {
    if pool == 0 {
        return 1_000;
    }
    let raw = u128::from(used) * 1_000 / u128::from(pool);
    u32::try_from(raw.min(u128::from(u32::MAX))).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::{PRESSURE_HIGH_WATER, PressureCandidate, plan_release};
    use crate::generation::GenerationState;
    use crate::wire_pending::{GenerationId, Timestamp};

    fn candidate(
        tag: u8,
        state: GenerationState,
        idle_ms: u64,
        snapshot_bytes: u64,
        last_active: i64,
        keepalive_held: bool,
    ) -> PressureCandidate {
        PressureCandidate {
            generation: GenerationId::from_bytes([tag; 16]),
            state,
            idle_ms,
            snapshot_bytes,
            session_last_active: Timestamp::from_millis(last_active),
            keepalive_held,
        }
    }

    #[test]
    fn nothing_is_selected_below_the_high_water_mark() {
        let candidates = vec![candidate(
            1,
            GenerationState::Suspended,
            600_000,
            1_000,
            0,
            false,
        )];
        let plan = plan_release(&candidates, 840, 1_000);
        assert!(plan.terminate.is_empty());
        assert_eq!(plan.projected_utilization, 840);
        assert!(plan.projected_utilization < PRESSURE_HIGH_WATER);
    }

    #[test]
    fn a_running_or_leased_generation_is_never_terminated_for_pressure() {
        for state in GenerationState::ALL {
            let probe = candidate(1, state, 600_000, 1_000, 0, false);
            assert_eq!(
                probe.is_eligible(),
                state == GenerationState::Suspended,
                "{state:?}"
            );
        }
        let leased = candidate(2, GenerationState::Suspended, 600_000, 1_000, 0, true);
        assert!(!leased.is_eligible());
        let plan = plan_release(&[leased], 900, 1_000);
        assert!(plan.terminate.is_empty());
    }

    #[test]
    fn a_generation_below_the_exact_idle_threshold_is_ineligible() {
        let just_under = candidate(1, GenerationState::Suspended, 179_999, 1_000, 0, false);
        let exactly = candidate(2, GenerationState::Suspended, 180_000, 1_000, 0, false);
        assert!(!just_under.is_eligible());
        assert!(exactly.is_eligible());
    }

    #[test]
    fn the_rank_is_idle_then_snapshot_bytes_then_session_recency() {
        let candidates = vec![
            candidate(1, GenerationState::Suspended, 200_000, 100, 500, false),
            candidate(2, GenerationState::Suspended, 900_000, 100, 500, false),
            candidate(3, GenerationState::Suspended, 900_000, 900, 500, false),
            candidate(4, GenerationState::Suspended, 900_000, 900, 100, false),
        ];
        // Pool is entirely full and nothing releases enough, so everything is
        // selected and the order is fully observable.
        let plan = plan_release(&candidates, 10_000, 10_000);
        assert_eq!(
            plan.terminate,
            vec![
                GenerationId::from_bytes([4; 16]),
                GenerationId::from_bytes([3; 16]),
                GenerationId::from_bytes([2; 16]),
                GenerationId::from_bytes([1; 16]),
            ]
        );
    }

    #[test]
    fn the_rank_is_a_total_order_even_when_every_field_ties() {
        let candidates = vec![
            candidate(9, GenerationState::Suspended, 500_000, 10, 5, false),
            candidate(1, GenerationState::Suspended, 500_000, 10, 5, false),
            candidate(5, GenerationState::Suspended, 500_000, 10, 5, false),
        ];
        let first = plan_release(&candidates, 10_000, 10_000).terminate;
        let mut reversed = candidates;
        reversed.reverse();
        let second = plan_release(&reversed, 10_000, 10_000).terminate;
        assert_eq!(first, second, "ranking must not depend on input order");
        assert_eq!(
            first,
            vec![
                GenerationId::from_bytes([1; 16]),
                GenerationId::from_bytes([5; 16]),
                GenerationId::from_bytes([9; 16]),
            ]
        );
    }

    #[test]
    fn release_stops_at_the_low_water_mark() {
        let candidates = vec![
            candidate(1, GenerationState::Suspended, 900_000, 100, 0, false),
            candidate(2, GenerationState::Suspended, 800_000, 100, 0, false),
            candidate(3, GenerationState::Suspended, 700_000, 100, 0, false),
            candidate(4, GenerationState::Suspended, 600_000, 100, 0, false),
        ];
        // 860/1000 used. Releasing 100 -> 760 (still >= 750), releasing 200 -> 660.
        let plan = plan_release(&candidates, 860, 1_000);
        assert_eq!(
            plan.terminate,
            vec![
                GenerationId::from_bytes([1; 16]),
                GenerationId::from_bytes([2; 16])
            ]
        );
        assert_eq!(plan.released_bytes, 200);
        assert_eq!(plan.projected_utilization, 660);
    }

    #[test]
    fn an_empty_pool_reports_full_utilization_rather_than_dividing_by_zero() {
        let plan = plan_release(&[], 0, 0);
        assert_eq!(plan.projected_utilization, 1_000);
        assert!(plan.terminate.is_empty());
    }
}
