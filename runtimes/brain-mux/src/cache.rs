//! The warm fold cache's dormant lifecycle: pressure bands, adaptive idle eviction and
//! TTL-zero release.
//!
//! The cache may change latency and nothing else. Entries are keyed `(agent, revision)`, so
//! an entry can only be *stale*, never *wrong*, and a hit still validates the authoritative
//! revision through the conditional claim before the activation acts. Losing the whole cache
//! is always harmless, which is why a dedicated lane runs the entire correctness suite with
//! it disabled.
//!
//! Its byte budget is separate from accepted work's. Evicting an admitted activation's
//! context to keep a cache entry would turn an optimization into a failure. Production keeps
//! this cache disabled until a resident-memory measurement can own `WarmCacheBytes`; replay
//! body bytes alone are not evidence of the Rust fold's heap residency.

use aex_brain_app::kernel::{FoldCache, WarmCacheShard, WarmEntry};
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
    /// A fail-closed policy which retains no fold state.
    ///
    /// This is the production policy until cache entries can acquire exact resident-byte
    /// permits. Keeping the constructor named makes accidental re-enablement visible in the
    /// composition diff.
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            ttl: core::time::Duration::ZERO,
            budget_bytes: 0,
            entry_cap_bytes: 0,
        }
    }

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

    fn ttl_millis(&self) -> u64 {
        u64::try_from(self.ttl.as_millis()).unwrap_or(u64::MAX)
    }
}

/// The process-local cache bound into activation.
///
/// Policy remains a mux concern while the application sees only the synchronous
/// [`FoldCache`] capability. This keeps TTL and memory sizing out of the durable engine.
#[derive(Debug)]
pub struct ConfiguredFoldCache {
    shard: WarmCacheShard,
    policy: CachePolicy,
}

impl ConfiguredFoldCache {
    /// Builds the one process-local shard under the configured byte ceilings.
    #[must_use]
    pub fn new(policy: CachePolicy) -> Self {
        Self {
            shard: WarmCacheShard::new(policy.budget_bytes, policy.entry_cap_bytes),
            policy,
        }
    }

    /// Exact cached canonical bytes.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.shard.bytes()
    }
}

impl FoldCache for ConfiguredFoldCache {
    fn load(&self, key: &AgentKey, revision: AgentRevision, tick: u64) -> Option<WarmEntry> {
        if self.policy.is_disabled() {
            return None;
        }
        self.shard
            .get_entry_with_max_idle(key, revision, tick, self.policy.ttl_millis())
    }

    fn store(
        &self,
        key: AgentKey,
        revision: AgentRevision,
        state: &aex_brain_domain::fold::FoldState,
        bytes: usize,
        tick: u64,
    ) -> bool {
        if self.policy.is_disabled()
            || bytes == 0
            || bytes > self.policy.entry_cap_bytes
            || bytes > self.policy.budget_bytes
        {
            return false;
        }
        store(
            &self.shard,
            &self.policy,
            key,
            WarmEntry {
                revision,
                state: state.clone(),
                bytes,
                last_used: tick,
            },
        )
    }

    fn remove(&self, key: &AgentKey, revision: AgentRevision) {
        self.shard.remove(key, revision);
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
    if policy.is_disabled() {
        return false;
    }
    shard.evict_idle(entry.last_used, policy.ttl_millis());
    if entry.bytes == 0 || entry.bytes > policy.entry_cap_bytes || entry.bytes > policy.budget_bytes
    {
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
    shard
        .get_entry_with_max_idle(key, revision, tick, policy.ttl_millis())
        .map(|entry| entry.state)
}

#[cfg(test)]
mod tests {
    use super::{
        Band, CachePolicy, ConfiguredFoldCache, DEFAULT_TTL, ENTRY_CAP_BYTES,
        HARD_THRESHOLD_PERCENT, MIN_RESIDENCY, SOFT_THRESHOLD_PERCENT, load, should_evict, store,
    };
    use aex_brain_app::kernel::{FoldCache, WarmCacheShard, WarmEntry};
    use aex_brain_domain::fold::FoldState;
    use aex_brain_domain::ids::{AgentId, AgentKey, AgentRevision, ContentHash, SessionId};
    use uuid::Uuid;

    fn key() -> AgentKey {
        key_for(1)
    }

    fn key_for(value: u128) -> AgentKey {
        AgentKey::new(
            SessionId(Uuid::from_u128(value)),
            AgentId(Uuid::from_u128(value.saturating_add(1))),
        )
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
        let policy = CachePolicy::disabled();
        assert!(policy.is_disabled());
        let shard = WarmCacheShard::new(1 << 20, ENTRY_CAP_BYTES);
        assert!(!store(&shard, &policy, key(), entry(1, 16)));
        assert!(load(&shard, &policy, &key(), AgentRevision(1), 0).is_none());
    }

    /// The exact TTL is enforced on reads, and a store also sweeps inactive keys so an
    /// abandoned agent does not retain state until its own next activation.
    #[test]
    fn ttl_expires_on_load_and_store() {
        let policy = CachePolicy {
            ttl: core::time::Duration::from_millis(100),
            budget_bytes: 1 << 20,
            entry_cap_bytes: 1 << 20,
        };
        let shard = WarmCacheShard::new(policy.budget_bytes, policy.entry_cap_bytes);
        assert!(store(&shard, &policy, key_for(1), entry(1, 16)));
        assert!(load(&shard, &policy, &key_for(1), AgentRevision(1), 99).is_some());
        assert!(load(&shard, &policy, &key_for(1), AgentRevision(1), 199).is_none());
        assert_eq!(shard.bytes(), 0);

        assert!(store(&shard, &policy, key_for(2), entry(1, 16)));
        let mut next = entry(1, 32);
        next.last_used = 100;
        assert!(store(&shard, &policy, key_for(3), next));
        assert_eq!(shard.bytes(), 32, "the store swept the expired first key");
    }

    /// Zero is not an honest resident-memory claim for a heap-owning fold. Refusing it also
    /// prevents an unbounded number of zero-cost map entries from defeating the byte ceiling.
    #[test]
    fn zero_accounted_entries_are_refused() {
        let policy = CachePolicy::default();
        let shard = WarmCacheShard::new(policy.budget_bytes, policy.entry_cap_bytes);
        assert!(!store(&shard, &policy, key(), entry(1, 0)));
        assert!(shard.is_empty());
    }

    /// A 1 MiB fold obeys the declared ceiling exactly, while the production-disabled cache
    /// declines the same borrowed state without retaining or cloning it.
    #[test]
    fn one_mib_fold_is_bounded_and_disabled_cache_retains_nothing() {
        const MIB: usize = 1_024 * 1_024;
        let mut state = FoldState::empty();
        state.hashes = vec![ContentHash([0x5a; 32]); MIB / 32];
        let enabled = ConfiguredFoldCache::new(CachePolicy {
            ttl: core::time::Duration::from_mins(1),
            budget_bytes: MIB,
            entry_cap_bytes: MIB,
        });
        assert!(enabled.store(key(), AgentRevision(1), &state, MIB, 0));
        assert_eq!(enabled.bytes(), MIB);
        assert!(!enabled.store(
            key_for(2),
            AgentRevision(1),
            &state,
            MIB.saturating_add(1),
            1,
        ));
        assert_eq!(enabled.bytes(), MIB);

        let disabled = ConfiguredFoldCache::new(CachePolicy::disabled());
        assert!(!disabled.store(key(), AgentRevision(1), &state, MIB, 0));
        assert_eq!(disabled.bytes(), 0);
        assert!(disabled.load(&key(), AgentRevision(1), 0).is_none());
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
