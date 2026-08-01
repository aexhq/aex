//! Sharding, backoff and the due scan.
//!
//! `shard_of` is a required key component derived from the work id, so a
//! literal single `due` partition is unrepresentable. `backoff` derives its
//! jitter from the same id rather than an RNG, so a schedule is reproducible in
//! a test and identical on every worker that computes it.

use std::num::{NonZeroU16, NonZeroU32};

use aex_wire::types::Timestamp;
use time::Duration;

use crate::lease::WorkId;

/// Domain tag for the shard function.
const SHARD_TAG: &[u8] = b"aex.work.shard.v1";
/// Domain tag for the backoff jitter.
const JITTER_TAG: &[u8] = b"aex.work.jitter.v1";

/// One due-scan shard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DueShard(pub u16);

/// The shard a work item lives in.
///
/// BLAKE3 derived and uniform, so no shard becomes a hot partition.
#[must_use]
pub fn shard_of(id: &WorkId, shards: NonZeroU16) -> DueShard {
    let mut hasher = blake3::Hasher::new();
    hasher.update(SHARD_TAG);
    hasher.update(id.as_bytes());
    let digest = hasher.finalize();
    let bytes = digest.as_bytes();
    let value = u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]);
    DueShard(u16::try_from(value % u64::from(shards.get())).unwrap_or(0))
}

/// How many parts the per-item jitter fraction is expressed in.
const JITTER_DENOMINATOR: i128 = 1_024;

/// The un-jittered exponential step for an attempt.
///
/// `base * 2^attempt`, capped at `cap`. Non-decreasing in `attempt` by
/// construction.
#[must_use]
pub fn backoff_step(attempt: u16, base: Duration, cap: Duration) -> Duration {
    let base_ms = base.whole_milliseconds().max(0);
    let cap_ms = cap.whole_milliseconds().max(base_ms);
    let shift = u32::from(attempt.min(40));
    let step_ms = base_ms.saturating_mul(1_i128 << shift).min(cap_ms);
    Duration::milliseconds(i64::try_from(step_ms).unwrap_or(i64::MAX))
}

/// The delay before attempt `attempt` runs.
///
/// Exponential from `base` and capped at `cap`, spread by a jitter fraction
/// derived from the **work id alone**. Deriving the fraction from the id rather
/// than from `(id, attempt)` is what keeps the schedule non-decreasing: a
/// per-attempt fraction would let a later attempt land earlier than an earlier
/// one once the cap is reached, which is exactly the retry-storm shape backoff
/// exists to prevent.
///
/// The result is pure in `(attempt, work id)`, lies in
/// `[step/2, step]`, is non-decreasing in `attempt` and never exceeds `cap`.
#[must_use]
pub fn backoff(attempt: u16, base: Duration, cap: Duration, id: &WorkId) -> Duration {
    let cap_ms = cap.whole_milliseconds().max(0);
    let step_ms = backoff_step(attempt, base, cap).whole_milliseconds();

    let mut hasher = blake3::Hasher::new();
    hasher.update(JITTER_TAG);
    hasher.update(id.as_bytes());
    let digest = hasher.finalize();
    let bytes = digest.as_bytes();
    let entropy = u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]);
    let numerator = i128::from(entropy % 1_024);

    let half = step_ms / 2;
    let total = (half + (half * numerator) / JITTER_DENOMINATOR).min(cap_ms.max(0));
    Duration::milliseconds(i64::try_from(total).unwrap_or(i64::MAX))
}

/// How much one page of a scan may read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PageBudget {
    /// Largest number of items one page returns.
    pub limit: NonZeroU32,
}

impl PageBudget {
    /// The default page budget.
    ///
    /// # Panics
    ///
    /// Never: the literal is non-zero.
    #[must_use]
    pub fn default_budget() -> Self {
        Self {
            limit: NonZeroU32::new(u32::from(crate::cursor::PAGE_DEFAULT))
                .unwrap_or_else(|| unreachable!("PAGE_DEFAULT is non-zero")),
        }
    }
}

/// One shard's slice of a due scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShardScan {
    /// Which shard.
    pub shard: DueShard,
    /// The exclusive upper bound on `due_at`.
    pub horizon: Timestamp,
    /// How many items this shard may return.
    pub limit: NonZeroU32,
}

/// A whole due scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueScanPlan {
    /// Every shard, in ascending order.
    pub shards: Vec<ShardScan>,
    /// The instant the plan was computed for.
    pub now: Timestamp,
}

impl DueScanPlan {
    /// Whether an item due at `due_at` in `shard` falls inside the plan.
    #[must_use]
    pub fn covers(&self, shard: DueShard, due_at: Timestamp) -> bool {
        self.shards
            .iter()
            .any(|scan| scan.shard == shard && due_at.unix_millis() <= scan.horizon.unix_millis())
    }
}

/// Plans one bounded due scan across every shard.
///
/// The scheduled scan and the stream redrive use the same plan, so a lost queue
/// hint only delays work: it can never lose it.
#[must_use]
pub fn plan_due_scan(shards: NonZeroU16, now: Timestamp, budget: PageBudget) -> DueScanPlan {
    DueScanPlan {
        shards: (0..shards.get())
            .map(|index| ShardScan {
                shard: DueShard(index),
                horizon: now,
                limit: budget.limit,
            })
            .collect(),
        now,
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU16;

    use aex_wire::ids::Uuid7;
    use aex_wire::types::Timestamp;
    use time::Duration;

    use super::{DueShard, PageBudget, backoff, plan_due_scan, shard_of};
    use crate::lease::WorkId;

    fn work(tag: u8) -> WorkId {
        WorkId(Uuid7::compose(1, [tag; 10]))
    }

    #[test]
    fn shards_are_stable_and_inside_the_range() {
        let shards = NonZeroU16::new(64).expect("non-zero");
        for tag in 0..64_u8 {
            let id = work(tag);
            let first = shard_of(&id, shards);
            assert_eq!(first, shard_of(&id, shards));
            assert!(first.0 < 64);
        }
    }

    #[test]
    fn backoff_is_pure_non_decreasing_and_capped() {
        let id = work(3);
        let base = Duration::seconds(1);
        let cap = Duration::seconds(60);
        let mut previous = Duration::ZERO;
        for attempt in 0..12_u16 {
            let delay = backoff(attempt, base, cap, &id);
            assert_eq!(delay, backoff(attempt, base, cap, &id));
            assert!(delay >= previous, "attempt {attempt} went backwards");
            assert!(delay <= cap);
            previous = delay;
        }
    }

    #[test]
    fn a_plan_covers_every_shard_up_to_the_horizon() {
        let now = Timestamp::from_unix_millis(1_000).expect("in range");
        let plan = plan_due_scan(
            NonZeroU16::new(8).expect("non-zero"),
            now,
            PageBudget::default_budget(),
        );
        assert_eq!(plan.shards.len(), 8);
        assert!(plan.covers(DueShard(7), now));
        assert!(!plan.covers(
            DueShard(7),
            Timestamp::from_unix_millis(1_001).expect("in range")
        ));
    }
}
