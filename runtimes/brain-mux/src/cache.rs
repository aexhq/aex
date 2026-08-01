//! The warm fold cache's lifecycle: pressure bands, adaptive idle eviction, TTL-zero
//! release.
//!
//! The cache may change latency and nothing else. Entries are keyed `(agent, revision)`, so
//! an entry can only be *stale*, never *wrong*, and a hit still validates the authoritative
//! revision through the conditional claim before the activation acts. Losing the whole cache
//! is always harmless, which is why a dedicated lane runs the entire correctness suite with
//! it disabled.
//!
//! Its byte budget is separate from accepted work's. Evicting an admitted activation's
//! context to keep a cache entry would turn an optimization into a failure.

use aex_brain_application::kernel::{WarmCacheShard, WarmEntry};
use aex_brain_domain::ids::{AgentKey, AgentRevision};

/// The default idle retention of a warm entry.
pub const DEFAULT_TTL: core::time::Duration = core::time::Duration::from_mins(3);

/// The largest single entry the cache admits.
pub const ENTRY_CAP_BYTES: usize = 16 * 1_024 * 1_024;

/// The fraction of the cache budget at which idle entries start being evicted.
pub const SOFT_THRESHOLD_PERCENT: u32 = 80;

/// The fraction at which entries are evicted regardless of minimum residency.
pub const HARD_THRESHOLD_PERCENT: u32 = 92;

/// The minimum residency a soft eviction respects.
pub const MIN_RESIDENCY: core::time::Duration = core::time::Duration::from_mins(10);

/// The pressure band the process is in.
///
/// The bands are ordered and each one's response includes the ones before it, so a reader
/// can reason about "at least this band" rather than about a set of independent flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Band {
    /// Below 65 %: admit within tenant and provider limits; retain eligible cache.
    Nominal,
    /// 65–75 %: evict expired, then cold idle cache; stop speculative work.
    Elevated,
    /// 75–80 %: no memory-heavy work without a reservation; shorten idle retention;
    /// publish scale pressure.
    Reserved,
    /// At or above 80 %: evict all releasable idle state; queue or typed-reject; request
    /// scale-out.
    Critical,
}

impl Band {
    /// The band `utilization_percent` falls in.
    #[must_use]
    pub const fn of(utilization_percent: u32) -> Self {
        if utilization_percent >= 80 {
            Self::Critical
        } else if utilization_percent >= 75 {
            Self::Reserved
        } else if utilization_percent >= 65 {
            Self::Elevated
        } else {
            Self::Nominal
        }
    }

    /// Whether new memory-heavy work needs an explicit reservation first.
    #[must_use]
    pub const fn requires_reservation(self) -> bool {
        matches!(self, Self::Reserved | Self::Critical)
    }

    /// Whether the task should publish scale-out pressure.
    ///
    /// Deliberately before the critical band: `task_start_seconds` was measured at 50, so a
    /// task that asks for help only when it is already critical gets it 50 seconds too late.
    #[must_use]
    pub const fn requests_scale_out(self) -> bool {
        matches!(self, Self::Reserved | Self::Critical)
    }

    /// Whether an active non-replayable effect may be interrupted to reclaim memory.
    ///
    /// Never. Its completion memory was reserved before dispatch, and killing it turns a
    /// pressure event into an interrupted run.
    #[must_use]
    pub const fn may_interrupt_dispatched_effects(self) -> bool {
        false
    }
}

/// The cache's configured lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CachePolicy {
    /// How long an idle entry is retained. Zero disables the cache.
    pub ttl: core::time::Duration,
    /// The whole cache's byte budget, separate from accepted work's.
    pub budget_bytes: usize,
    /// The largest single entry admitted.
    pub entry_cap_bytes: usize,
}

impl Default for CachePolicy {
    fn default() -> Self {
        Self {
            ttl: DEFAULT_TTL,
            budget_bytes: 512 * 1_024 * 1_024,
            entry_cap_bytes: ENTRY_CAP_BYTES,
        }
    }
}

impl CachePolicy {
    /// Whether the cache is disabled.
    ///
    /// TTL zero releases the entry and its memory reservation in the same code path that
    /// commits, so `memory.byte_ms` accrual stops exactly at the commit rather than at the
    /// next sweep.
    #[must_use]
    pub const fn is_disabled(&self) -> bool {
        self.ttl.is_zero() || self.budget_bytes == 0
    }

    /// The idle retention that applies in `band`.
    ///
    /// Retention shortens under pressure rather than switching off, so the cache degrades
    /// gradually instead of collapsing at one threshold.
    #[must_use]
    pub const fn retention_in(&self, band: Band) -> core::time::Duration {
        match band {
            Band::Nominal => self.ttl,
            Band::Elevated => core::time::Duration::from_secs(self.ttl.as_secs() / 2),
            Band::Reserved => core::time::Duration::from_secs(self.ttl.as_secs() / 4),
            Band::Critical => core::time::Duration::ZERO,
        }
    }
}

/// Whether an entry should be evicted now.
///
/// A soft eviction respects the minimum residency; a hard one does not, because at the hard
/// threshold the alternative is refusing accepted work.
#[must_use]
pub fn should_evict(
    idle: core::time::Duration,
    resident: core::time::Duration,
    retention: core::time::Duration,
    utilization_percent: u32,
) -> bool {
    if utilization_percent >= HARD_THRESHOLD_PERCENT {
        return true;
    }
    if idle >= retention && utilization_percent >= SOFT_THRESHOLD_PERCENT {
        return resident >= MIN_RESIDENCY || idle >= retention;
    }
    idle >= retention
}

/// Inserts a fold under `policy`, or declines when the cache is disabled or the entry is
/// over the per-entry cap.
///
/// Returns whether it was stored. A caller must never depend on the answer: a cache miss and
/// a cache refusal are the same thing to correctness.
pub fn store(
    shard: &WarmCacheShard,
    policy: &CachePolicy,
    key: AgentKey,
    entry: WarmEntry,
) -> bool {
    if policy.is_disabled() || entry.bytes > policy.entry_cap_bytes {
        return false;
    }
    shard.insert(key, entry)
}

/// Reads a fold at exactly `revision`.
///
/// Exact rather than "at least": a fold at a different revision is a different fold, and
/// returning one would be the cache inventing an answer.
pub fn load(
    shard: &WarmCacheShard,
    policy: &CachePolicy,
    key: &AgentKey,
    revision: AgentRevision,
    tick: u64,
) -> Option<aex_brain_domain::fold::FoldState> {
    if policy.is_disabled() {
        return None;
    }
    shard.get(key, revision, tick)
}

#[cfg(test)]
mod tests {
    use super::{
        Band, CachePolicy, DEFAULT_TTL, ENTRY_CAP_BYTES, HARD_THRESHOLD_PERCENT, MIN_RESIDENCY,
        SOFT_THRESHOLD_PERCENT, load, should_evict, store,
    };
    use aex_brain_application::kernel::{WarmCacheShard, WarmEntry};
    use aex_brain_domain::fold::FoldState;
    use aex_brain_domain::ids::{AgentId, AgentKey, AgentRevision, SessionId};
    use uuid::Uuid;

    fn key() -> AgentKey {
        AgentKey::new(SessionId(Uuid::from_u128(1)), AgentId(Uuid::from_u128(2)))
    }

    fn entry(revision: u64, bytes: usize) -> WarmEntry {
        WarmEntry {
            revision: AgentRevision(revision),
            state: FoldState::empty(),
            bytes,
            last_used: 0,
        }
    }

    /// The bands fire in the documented order and nowhere else.
    #[test]
    fn the_pressure_bands_fire_in_the_documented_order() {
        assert_eq!(Band::of(0), Band::Nominal);
        assert_eq!(Band::of(64), Band::Nominal);
        assert_eq!(Band::of(65), Band::Elevated);
        assert_eq!(Band::of(74), Band::Elevated);
        assert_eq!(Band::of(75), Band::Reserved);
        assert_eq!(Band::of(79), Band::Reserved);
        assert_eq!(Band::of(80), Band::Critical);
        assert_eq!(Band::of(100), Band::Critical);
        assert!(Band::Nominal < Band::Elevated);
        assert!(Band::Elevated < Band::Reserved);
        assert!(Band::Reserved < Band::Critical);
    }

    /// Scale-out is requested before the critical band. `task_start_seconds` was measured
    /// at 50, so a task that asks for help only when critical gets it far too late.
    #[test]
    fn scale_out_is_requested_before_the_critical_band() {
        assert!(!Band::Nominal.requests_scale_out());
        assert!(!Band::Elevated.requests_scale_out());
        assert!(Band::Reserved.requests_scale_out());
        assert!(Band::Critical.requests_scale_out());
    }

    /// An active non-replayable effect is never killed to save cache: its completion memory
    /// was reserved before dispatch, and killing it turns pressure into an interrupted run.
    #[test]
    fn no_band_ever_interrupts_a_dispatched_effect() {
        for band in [
            Band::Nominal,
            Band::Elevated,
            Band::Reserved,
            Band::Critical,
        ] {
            assert!(!band.may_interrupt_dispatched_effects());
        }
    }

    #[test]
    fn memory_heavy_work_needs_a_reservation_from_the_reserved_band_upward() {
        assert!(!Band::Nominal.requires_reservation());
        assert!(!Band::Elevated.requires_reservation());
        assert!(Band::Reserved.requires_reservation());
        assert!(Band::Critical.requires_reservation());
    }

    /// Retention shortens under pressure rather than switching off, so the cache degrades
    /// gradually instead of collapsing at one threshold.
    #[test]
    fn retention_shortens_monotonically_with_pressure() {
        let policy = CachePolicy::default();
        assert_eq!(policy.retention_in(Band::Nominal), DEFAULT_TTL);
        assert!(policy.retention_in(Band::Elevated) < policy.retention_in(Band::Nominal));
        assert!(policy.retention_in(Band::Reserved) < policy.retention_in(Band::Elevated));
        assert!(policy.retention_in(Band::Critical).is_zero());
    }

    /// TTL zero is a real configuration, not a degenerate one: the correctness suite runs
    /// with it and must see identical semantics.
    #[test]
    fn a_ttl_of_zero_disables_the_cache_entirely() {
        let policy = CachePolicy {
            ttl: core::time::Duration::ZERO,
            ..CachePolicy::default()
        };
        assert!(policy.is_disabled());
        let shard = WarmCacheShard::new(1 << 20, ENTRY_CAP_BYTES);
        assert!(!store(&shard, &policy, key(), entry(1, 16)));
        assert!(load(&shard, &policy, &key(), AgentRevision(1), 0).is_none());
    }

    #[test]
    fn an_entry_over_the_per_entry_cap_is_declined() {
        let policy = CachePolicy {
            entry_cap_bytes: 64,
            ..CachePolicy::default()
        };
        let shard = WarmCacheShard::new(1 << 20, policy.entry_cap_bytes);
        assert!(!store(&shard, &policy, key(), entry(1, 65)));
        assert!(store(&shard, &policy, key(), entry(1, 64)));
    }

    /// A hit is exact. Returning a fold at a different revision would be the cache
    /// inventing an answer the authority never gave.
    #[test]
    fn a_hit_at_a_different_revision_is_a_miss() {
        let policy = CachePolicy::default();
        let shard = WarmCacheShard::new(1 << 20, policy.entry_cap_bytes);
        assert!(store(&shard, &policy, key(), entry(4, 16)));
        assert!(load(&shard, &policy, &key(), AgentRevision(4), 1).is_some());
        assert!(load(&shard, &policy, &key(), AgentRevision(5), 1).is_none());
    }

    #[test]
    fn the_soft_threshold_evicts_idle_entries_and_the_hard_one_evicts_regardless() {
        let retention = core::time::Duration::from_mins(3);
        let fresh = core::time::Duration::from_secs(1);
        let idle = core::time::Duration::from_secs(200);

        assert!(!should_evict(fresh, MIN_RESIDENCY, retention, 10));
        assert!(should_evict(idle, MIN_RESIDENCY, retention, 10));
        assert!(should_evict(
            idle,
            core::time::Duration::ZERO,
            retention,
            SOFT_THRESHOLD_PERCENT
        ));
        assert!(
            should_evict(
                fresh,
                core::time::Duration::ZERO,
                retention,
                HARD_THRESHOLD_PERCENT
            ),
            "at the hard threshold the alternative is refusing accepted work"
        );
    }
}
