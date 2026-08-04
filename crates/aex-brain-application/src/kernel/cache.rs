//! `WarmCacheShard` — a byte-budgeted fold cache keyed by revision (Loom target L5).
//!
//! The cache may change latency and nothing else. Its entries are keyed by
//! `(agent, revision)`, so an entry can only ever be *stale*, never *wrong*, and a cache
//! hit still validates the authoritative revision through the conditional claim before the
//! activation acts on it. Losing the whole cache is always harmless.

use super::sync::Mutex;
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
/// Sharded by agent hash so lock contention is bounded rather than global.
#[derive(Debug)]
pub struct WarmCacheShard {
    entries: Mutex<HashMap<AgentKey, WarmEntry>>,
    budget_bytes: usize,
    entry_cap_bytes: usize,
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
        if entry.bytes > self.entry_cap_bytes {
            return false;
        }
        let mut entries = self.entries.lock().expect("the cache lock is not poisoned");
        if let Some(existing) = entries.get(&key)
            && existing.revision >= entry.revision
        {
            return false;
        }
        entries.insert(key, entry);
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
        let mut entries = self.entries.lock().expect("the cache lock is not poisoned");
        let entry = entries.get_mut(key)?;
        if entry.revision != revision {
            return None;
        }
        entry.last_used = tick;
        Some(entry.clone())
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
        self.entries
            .lock()
            .expect("the cache lock is not poisoned")
            .values()
            .map(|entry| entry.bytes)
            .sum()
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

    fn evict_to_budget(entries: &mut HashMap<AgentKey, WarmEntry>, budget: usize) {
        let mut total: usize = entries.values().map(|entry| entry.bytes).sum();
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
        if bytes > self.entry_cap_bytes {
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
