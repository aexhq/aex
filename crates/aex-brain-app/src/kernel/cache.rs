//! `WarmCacheShard` — a byte-budgeted fold cache keyed by revision (Loom target L5).
//!
//! The cache may change latency and nothing else. Its entries are keyed by
//! `(agent, revision)`, so an entry can only ever be *stale*, never *wrong*, and a cache
//! hit still validates the authoritative revision through the conditional claim before the
//! activation acts on it. Losing the whole cache is always harmless.

use super::sync::{Arc, Mutex};
use aex_brain_domain::fold::FoldState;
use aex_brain_domain::ids::{AgentKey, AgentRevision};
use std::collections::HashMap;

/// Process-local fold acceleration used by an activation after its durable claim.
///
/// Implementations are deliberately synchronous: a cache lookup must never become another
/// authority or a network dependency. Every hit is checked against the exact revision and
/// claimed journal tail before use, and a miss always falls back to the authoritative store.
pub trait FoldCache: core::fmt::Debug + Send + Sync + 'static {
    /// Loads one exact revision, updating its recency tick.
    fn load(&self, key: &AgentKey, revision: AgentRevision, tick: u64) -> Option<WarmEntry>;

    /// Stores a fold that has already proved the claimed authoritative tail.
    ///
    /// The state is borrowed so a disabled or oversized cache can refuse before cloning a
    /// potentially large context.
    fn store(
        &self,
        key: AgentKey,
        revision: AgentRevision,
        state: &FoldState,
        bytes: usize,
        tick: u64,
    ) -> bool;

    /// Removes an exact entry that failed the redundant claimed-tail check.
    fn remove(&self, key: &AgentKey, revision: AgentRevision);
}

/// A cached fold plus its accounting.
#[derive(Debug, Clone)]
pub struct WarmEntry {
    /// The revision this fold was valid at.
    pub revision: AgentRevision,
    /// The validated fold.
    pub state: FoldState,
    /// What the entry costs against the cache byte budget.
    pub bytes: usize,
    /// A monotonic tick recording last use, for eviction order.
    pub last_used: u64,
}

/// One shard of the warm fold cache.
///
/// One independently budgeted cache shard. A caller may create multiple shards, but every
/// shard keeps the expensive [`FoldState`] clone outside its map lock so an unrelated key is
/// never serialized behind a large cache hit.
#[derive(Debug)]
pub struct WarmCacheShard {
    entries: Mutex<HashMap<AgentKey, CachedWarmEntry>>,
    budget_bytes: usize,
    entry_cap_bytes: usize,
}

/// The internal representation shares the immutable state long enough to move its deep clone
/// outside the shard lock. Recency remains map-owned and is still updated atomically with lookup.
#[derive(Debug)]
struct CachedWarmEntry {
    revision: AgentRevision,
    state: Arc<FoldState>,
    bytes: usize,
    last_used: u64,
}

impl From<WarmEntry> for CachedWarmEntry {
    fn from(entry: WarmEntry) -> Self {
        Self {
            revision: entry.revision,
            state: Arc::new(entry.state),
            bytes: entry.bytes,
            last_used: entry.last_used,
        }
    }
}

impl WarmCacheShard {
    /// A shard holding at most `budget_bytes`, refusing any single entry above
    /// `entry_cap_bytes`.
    #[must_use]
    pub fn new(budget_bytes: usize, entry_cap_bytes: usize) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            budget_bytes,
            entry_cap_bytes,
        }
    }

    /// Inserts or replaces the entry for `key`.
    ///
    /// An insert whose revision is **below** the stored one is dropped. Without that check
    /// a slow hydration finishing after a fast commit would install a fold older than the
    /// committed revision, and the next activation would start from state the authority has
    /// already moved past.
    ///
    /// Returns whether the entry was stored.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock was poisoned.
    pub fn insert(&self, key: AgentKey, entry: WarmEntry) -> bool {
        if entry.bytes == 0 || entry.bytes > self.entry_cap_bytes || entry.bytes > self.budget_bytes
        {
            return false;
        }
        let mut entries = self.entries.lock().expect("the cache lock is not poisoned");
        if let Some(existing) = entries.get(&key)
            && existing.revision >= entry.revision
        {
            return false;
        }
        entries.insert(key, entry.into());
        Self::evict_to_budget(&mut entries, self.budget_bytes);
        true
    }

    /// The entry for `key` at exactly `revision`, if it is present.
    ///
    /// Exact rather than "at least": a fold at a different revision is a different fold,
    /// and returning one would be the cache inventing an answer.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock was poisoned.
    pub fn get(&self, key: &AgentKey, revision: AgentRevision, tick: u64) -> Option<FoldState> {
        self.get_entry(key, revision, tick).map(|entry| entry.state)
    }

    /// The complete entry for `key` at exactly `revision`, if it is present.
    ///
    /// This is used by activation so retained-byte accounting survives a cache hit. The
    /// returned fold is still a clone: cache eviction can therefore never invalidate an
    /// admitted activation's state.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock was poisoned.
    pub fn get_entry(
        &self,
        key: &AgentKey,
        revision: AgentRevision,
        tick: u64,
    ) -> Option<WarmEntry> {
        self.get_entry_with_max_idle(key, revision, tick, u64::MAX)
    }

    /// The complete exact-revision entry when it has been idle for less than
    /// `max_idle_ticks`; an expired exact entry is removed before returning a miss.
    ///
    /// `tick` and `max_idle_ticks` must use the same monotonic unit. The mux uses
    /// milliseconds. A different revision remains a miss without deletion because a
    /// concurrent activation may still be entitled to that exact entry.
    ///
    /// The map lock protects only lookup and recency. The potentially multi-megabyte state
    /// clone happens after it is released.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock was poisoned.
    pub fn get_entry_with_max_idle(
        &self,
        key: &AgentKey,
        revision: AgentRevision,
        tick: u64,
        max_idle_ticks: u64,
    ) -> Option<WarmEntry> {
        let cached = {
            let mut entries = self.entries.lock().expect("the cache lock is not poisoned");
            let expired = entries.get(key).is_some_and(|entry| {
                max_idle_ticks != u64::MAX
                    && entry.revision == revision
                    && tick.saturating_sub(entry.last_used) >= max_idle_ticks
            });
            if expired {
                entries.remove(key);
                return None;
            }
            let entry = entries.get_mut(key)?;
            if entry.revision != revision {
                return None;
            }
            entry.last_used = tick;
            (
                entry.revision,
                Arc::clone(&entry.state),
                entry.bytes,
                entry.last_used,
            )
        };
        Some(WarmEntry {
            revision: cached.0,
            state: (*cached.1).clone(),
            bytes: cached.2,
            last_used: cached.3,
        })
    }

    /// Removes every entry idle for at least `max_idle_ticks` and returns the canonical
    /// bytes released. This is called by cache stores so entries for inactive agents do not
    /// wait for a key-specific lookup to expire.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock was poisoned.
    pub fn evict_idle(&self, tick: u64, max_idle_ticks: u64) -> usize {
        let mut entries = self.entries.lock().expect("the cache lock is not poisoned");
        let before = Self::total_bytes(&entries);
        entries.retain(|_, entry| tick.saturating_sub(entry.last_used) < max_idle_ticks);
        before.saturating_sub(Self::total_bytes(&entries))
    }

    /// Drops the entry for `key` when the committed revision has moved past it.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock was poisoned.
    pub fn invalidate(&self, key: &AgentKey, committed: AgentRevision) {
        let mut entries = self.entries.lock().expect("the cache lock is not poisoned");
        if entries
            .get(key)
            .is_some_and(|entry| entry.revision < committed)
        {
            entries.remove(key);
        }
    }

    /// Removes `key` only when the stored revision is exactly `revision`.
    ///
    /// A redundant tail check can reject a damaged process-local entry. Removing only the
    /// observed revision prevents that cleanup from deleting a newer fold concurrently
    /// installed after this activation looked.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock was poisoned.
    pub fn remove(&self, key: &AgentKey, revision: AgentRevision) {
        let mut entries = self.entries.lock().expect("the cache lock is not poisoned");
        if entries
            .get(key)
            .is_some_and(|entry| entry.revision == revision)
        {
            entries.remove(key);
        }
    }

    /// Drops every entry, releasing the whole shard's byte reservation.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock was poisoned.
    pub fn clear(&self) {
        self.entries
            .lock()
            .expect("the cache lock is not poisoned")
            .clear();
    }

    /// How many bytes the shard currently holds.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock was poisoned.
    #[must_use]
    pub fn bytes(&self) -> usize {
        let entries = self.entries.lock().expect("the cache lock is not poisoned");
        Self::total_bytes(&entries)
    }

    /// How many entries the shard holds.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock was poisoned.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries
            .lock()
            .expect("the cache lock is not poisoned")
            .len()
    }

    /// Whether the shard is empty.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock was poisoned.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn evict_to_budget(entries: &mut HashMap<AgentKey, CachedWarmEntry>, budget: usize) {
        let mut total = Self::total_bytes(entries);
        while total > budget {
            let Some(victim) = entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| *key)
            else {
                break;
            };
            if let Some(entry) = entries.remove(&victim) {
                total = total.saturating_sub(entry.bytes);
            } else {
                break;
            }
        }
    }

    fn total_bytes(entries: &HashMap<AgentKey, CachedWarmEntry>) -> usize {
        entries
            .values()
            .fold(0_usize, |total, entry| total.saturating_add(entry.bytes))
    }
}

impl FoldCache for WarmCacheShard {
    fn load(&self, key: &AgentKey, revision: AgentRevision, tick: u64) -> Option<WarmEntry> {
        self.get_entry(key, revision, tick)
    }

    fn store(
        &self,
        key: AgentKey,
        revision: AgentRevision,
        state: &FoldState,
        bytes: usize,
        tick: u64,
    ) -> bool {
        if bytes == 0 || bytes > self.entry_cap_bytes || bytes > self.budget_bytes {
            return false;
        }
        self.insert(
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
        WarmCacheShard::remove(self, key, revision);
    }
}
