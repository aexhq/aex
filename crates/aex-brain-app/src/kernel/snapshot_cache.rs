//! Optional byte-bounded cache for immutable fold-snapshot bodies.
//!
//! The cache is keyed by workspace plus the regional content authority's SHA-256 identity. It never
//! caches the mutable per-agent pointer and never bypasses pointer/body verification, so
//! losing or clearing it can change latency but not semantics. The production snapshot
//! adapter is not wired yet; this primitive fixes the collision and eviction contract for
//! that later, measured optimization.

use std::collections::HashMap;

use aex_wire::ids::{ContentHash, WorkspaceId};

use super::sync::Mutex;

#[derive(Debug, Clone)]
struct Entry {
    body: Vec<u8>,
    last_used: u64,
}

/// Why bytes cannot enter the immutable-body cache.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SnapshotCacheError {
    /// The claimed key is not SHA-256 over the supplied bytes.
    #[error("snapshot cache key does not match the supplied body")]
    DigestMismatch,
    /// Distinct bytes were observed under one SHA-256 identity.
    #[error("distinct snapshot bodies claimed one SHA-256 identity")]
    DigestCollision,
}

/// One byte-budgeted immutable-body cache shard.
#[derive(Debug)]
pub struct SnapshotBodyCache {
    entries: Mutex<HashMap<(WorkspaceId, ContentHash), Entry>>,
    budget_bytes: usize,
    entry_cap_bytes: usize,
}

impl SnapshotBodyCache {
    /// Creates a cache with separate aggregate and per-entry byte ceilings.
    #[must_use]
    pub fn new(budget_bytes: usize, entry_cap_bytes: usize) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            budget_bytes,
            entry_cap_bytes,
        }
    }

    /// Inserts verified immutable bytes, evicting least-recently-used bodies as required.
    ///
    /// `Ok(false)` is an ordinary cache refusal for a disabled cache or an oversized entry.
    /// A digest mismatch/collision is typed because silently accepting either would turn an
    /// optimization into a substitution channel.
    ///
    /// # Errors
    ///
    /// Returns [`SnapshotCacheError::DigestMismatch`] when `digest` does not name `body`,
    /// or [`SnapshotCacheError::DigestCollision`] if different resident bytes already claim
    /// that identity.
    ///
    /// # Panics
    ///
    /// Panics only if a prior test already poisoned the cache lock.
    pub fn insert(
        &self,
        workspace: WorkspaceId,
        digest: ContentHash,
        body: Vec<u8>,
        tick: u64,
    ) -> Result<bool, SnapshotCacheError> {
        if ContentHash::of(&body) != digest {
            return Err(SnapshotCacheError::DigestMismatch);
        }
        if self.budget_bytes == 0 || body.len() > self.entry_cap_bytes {
            return Ok(false);
        }
        let mut entries = self.entries.lock().expect("the cache lock is not poisoned");
        let key = (workspace, digest);
        if let Some(existing) = entries.get_mut(&key) {
            if existing.body != body {
                return Err(SnapshotCacheError::DigestCollision);
            }
            existing.last_used = tick;
            return Ok(true);
        }
        entries.insert(
            key,
            Entry {
                body,
                last_used: tick,
            },
        );
        Self::evict_to_budget(&mut entries, self.budget_bytes);
        Ok(entries.contains_key(&key))
    }

    /// Returns exact immutable bytes and marks them recently used.
    ///
    /// # Panics
    ///
    /// Panics only if a prior test already poisoned the cache lock.
    #[must_use]
    pub fn get(&self, workspace: WorkspaceId, digest: ContentHash, tick: u64) -> Option<Vec<u8>> {
        let mut entries = self.entries.lock().expect("the cache lock is not poisoned");
        let entry = entries.get_mut(&(workspace, digest))?;
        entry.last_used = tick;
        Some(entry.body.clone())
    }

    /// Drops all process-local bytes; authority remains untouched.
    ///
    /// # Panics
    ///
    /// Panics only if a prior caller already poisoned the cache lock.
    pub fn clear(&self) {
        self.entries
            .lock()
            .expect("the cache lock is not poisoned")
            .clear();
    }

    /// Exact cached body bytes, excluding map/allocation overhead.
    ///
    /// # Panics
    ///
    /// Panics only if a prior caller already poisoned the cache lock.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.entries
            .lock()
            .expect("the cache lock is not poisoned")
            .values()
            .map(|entry| entry.body.len())
            .sum()
    }

    fn evict_to_budget(entries: &mut HashMap<(WorkspaceId, ContentHash), Entry>, budget: usize) {
        let mut total = entries
            .values()
            .map(|entry| entry.body.len())
            .sum::<usize>();
        while total > budget {
            let Some(victim) = entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| *key)
            else {
                break;
            };
            if let Some(removed) = entries.remove(&victim) {
                total = total.saturating_sub(removed.body.len());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Entry, SnapshotBodyCache, SnapshotCacheError};
    use aex_wire::PrefixedId;
    use aex_wire::ids::{ContentHash, Uuid7, WorkspaceId};

    fn workspace(seed: u8) -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1, [seed; 10]))
    }

    #[test]
    fn mismatched_identity_and_same_digest_substitution_are_refused() {
        let cache = SnapshotBodyCache::new(1_024, 1_024);
        let digest = ContentHash::of(b"canonical");
        assert_eq!(
            cache.insert(workspace(1), digest, b"substituted".to_vec(), 0),
            Err(SnapshotCacheError::DigestMismatch)
        );

        cache.entries.lock().expect("not poisoned").insert(
            (workspace(1), digest),
            Entry {
                body: b"impossible collision".to_vec(),
                last_used: 0,
            },
        );
        assert_eq!(
            cache.insert(workspace(1), digest, b"canonical".to_vec(), 1),
            Err(SnapshotCacheError::DigestCollision)
        );
    }

    #[test]
    fn least_recently_used_bytes_are_evicted_and_restart_is_a_miss() {
        let cache = SnapshotBodyCache::new(6, 6);
        let first = ContentHash::of(b"aaa");
        let second = ContentHash::of(b"bbb");
        let third = ContentHash::of(b"ccc");
        assert_eq!(
            cache.insert(workspace(1), first, b"aaa".to_vec(), 1),
            Ok(true)
        );
        assert_eq!(
            cache.insert(workspace(1), second, b"bbb".to_vec(), 2),
            Ok(true)
        );
        assert_eq!(cache.get(workspace(1), first, 3), Some(b"aaa".to_vec()));
        assert_eq!(
            cache.insert(workspace(1), third, b"ccc".to_vec(), 4),
            Ok(true)
        );
        assert!(
            cache.get(workspace(1), second, 5).is_none(),
            "the coldest body was evicted"
        );
        assert!(
            cache.get(workspace(2), first, 5).is_none(),
            "another workspace cannot reuse resident bytes"
        );
        assert_eq!(cache.bytes(), 6);

        cache.clear();
        assert!(cache.get(workspace(1), first, 6).is_none());
        assert!(cache.get(workspace(1), third, 6).is_none());
        assert_eq!(cache.bytes(), 0);
    }
}
