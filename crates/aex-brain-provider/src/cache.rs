//! The decrypted-key cache with keyed single-flight.
//!
//! Moved from `aex-brain-provider-gateway` (model-provider simplification
//! 2026-08-13): the cache is a consumer-side optimization and belongs to the
//! crate that consumes decrypted keys, keeping `aex-brain-provider-custody` a
//! stateless adapter.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aex_brain_provider_custody::credential::{
    CredentialResolveError, ProviderApiKey, ProviderCredentialBinding, ProviderCredentialDecryptor,
};
use aex_wire::ids::{OrganizationId, ProviderCredentialId, WorkspaceId};
use zeroize::Zeroizing;

use crate::credential_flight::{
    FlightAdmission, FlightJoin, FlightLease, FlightOutcome, FlightRegistry, FlightValidity,
    MAX_FLIGHT_ELECTIONS,
};

/// The cache key. Every component participates, so a rotation or a generation
/// bump can never hit a stale entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CredentialCacheKey {
    /// The owning organization. Workspace ids are globally unique, but keeping
    /// the authority identity explicit makes accidental workspace movement a
    /// cache miss even before the context-digest check rejects it.
    pub organization: OrganizationId,
    /// The owning workspace.
    pub workspace: WorkspaceId,
    /// Which binding.
    pub binding: ProviderCredentialId,
    /// Which revision.
    pub revision: aex_brain_provider_custody::credential::CredentialRevision,
    /// Which generation.
    pub generation: aex_secret_domain::SourceGeneration,
}

impl From<&ProviderCredentialBinding> for CredentialCacheKey {
    fn from(binding: &ProviderCredentialBinding) -> Self {
        Self {
            organization: binding.context.organization,
            workspace: binding.workspace,
            binding: binding.id,
            revision: binding.revision,
            generation: binding.generation,
        }
    }
}

/// How long a decrypted key may be reused.
pub const CACHE_TTL: Duration = Duration::from_mins(1);
/// How many decrypted keys may be held at once.
pub const CACHE_CAPACITY: usize = 256;

struct CacheEntry {
    key: Arc<Zeroizing<String>>,
    inserted_at: Instant,
}

const CACHE_SHARDS: usize = 16;

struct CacheShard {
    entries: Mutex<HashMap<CredentialCacheKey, CacheEntry>>,
    capacity: usize,
}

/// A bounded, expiring cache of decrypted keys with keyed single-flight.
///
/// Decryption happens immediately before dispatch and the plaintext exists only
/// in one reference-counted, zeroizing allocation held by rig's client for the
/// duration of one send. Concurrent cold misses for one identity share one
/// decrypt and that one allocation rather than each calling the authority.
/// Revocation calls [`CredentialCache::invalidate`], which drops every
/// matching cache reference and retires every matching running decrypt. An
/// already in-flight dispatch may keep its private reference until that
/// attempt ends; the last reference drop zeroizes the allocation.
pub struct CredentialCache {
    shards: Vec<CacheShard>,
    flights: FlightRegistry,
    ttl: Duration,
}

impl core::fmt::Debug for CredentialCache {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Deliberately opaque: a cache whose `Debug` listed its keys would put
        // the workspace/binding graph into every log line that formats it.
        formatter.write_str("CredentialCache { .. }")
    }
}

impl Default for CredentialCache {
    fn default() -> Self {
        Self::new(CACHE_CAPACITY, CACHE_TTL)
    }
}

impl CredentialCache {
    /// Builds a cache with an explicit capacity and TTL.
    #[must_use]
    pub fn new(capacity: usize, ttl: Duration) -> Self {
        let shard_count = capacity.clamp(1, CACHE_SHARDS);
        let base_capacity = capacity / shard_count;
        let remainder = capacity % shard_count;
        Self {
            shards: (0..shard_count)
                .map(|index| CacheShard {
                    entries: Mutex::new(HashMap::new()),
                    capacity: base_capacity + usize::from(index < remainder),
                })
                .collect(),
            flights: FlightRegistry::new(capacity),
            ttl,
        }
    }

    /// Decrypts a binding, reusing a fresh entry where one exists and joining a
    /// decrypt already running for the same identity where there is one.
    ///
    /// # Errors
    ///
    /// Propagates the decryptor's own [`CredentialResolveError`], to the caller
    /// that ran it and to every caller that was waiting on it. A failure is
    /// shared but never cached, so the next caller resolves it again.
    pub async fn decrypt(
        &self,
        binding: &ProviderCredentialBinding,
        decryptor: &dyn ProviderCredentialDecryptor,
        now: aex_wire::types::Timestamp,
    ) -> Result<ProviderApiKey, CredentialResolveError> {
        let key = CredentialCacheKey::from(binding);
        for _ in 0..MAX_FLIGHT_ELECTIONS {
            if let Some(fresh) = self.fresh(&key) {
                return Ok(ProviderApiKey::from_shared(fresh));
            }
            match self.flights.admit(key) {
                FlightAdmission::Leading(lease) => {
                    let outcome: FlightOutcome = decryptor
                        .decrypt(binding, now)
                        .await
                        .map(ProviderApiKey::into_shared);
                    if let Ok(shared) = &outcome {
                        self.insert(key, Arc::clone(shared), &lease);
                    }
                    lease.settle(outcome.clone());
                    return outcome.map(ProviderApiKey::from_shared);
                }
                FlightAdmission::Following(follower) => match follower.joined().await {
                    FlightJoin::Settled(outcome) => {
                        return outcome.map(ProviderApiKey::from_shared);
                    }
                    // The leader ran out of deadline or was cancelled before it
                    // published anything, so nobody will. Re-elect.
                    FlightJoin::Abandoned => {}
                },
                FlightAdmission::Unregistered => break,
            }
        }
        // Reached only when the registry is full, or when every leader this
        // caller joined was abandoned. Decrypt alone, and do not cache: a
        // decrypt no flight covers cannot show that a revocation did not land
        // while it ran, and an entry that cannot be shown to be current is
        // exactly the stale insertion revocation must never leave behind.
        decryptor.decrypt(binding, now).await
    }

    /// How many decrypts are running right now.
    #[must_use]
    pub fn active_flights(&self) -> usize {
        self.flights.active()
    }

    fn fresh(&self, key: &CredentialCacheKey) -> Option<Arc<Zeroizing<String>>> {
        let shard = self.shard(key);
        let mut entries = shard.entries.lock().ok()?;
        let entry = entries.get(key)?;
        if entry.inserted_at.elapsed() >= self.ttl {
            entries.remove(key);
            return None;
        }
        Some(Arc::clone(&entry.key))
    }

    fn insert(&self, key: CredentialCacheKey, value: Arc<Zeroizing<String>>, lease: &FlightLease) {
        let shard = self.shard(&key);
        if shard.capacity == 0 {
            return;
        }
        let Ok(mut entries) = shard.entries.lock() else {
            return;
        };
        // Read while the shard lock is held: `invalidate` marks every matching
        // flight before it touches a shard, so a mark landing after this read
        // belongs to a revocation whose own removal pass must first wait for
        // this lock and then takes the entry written below straight back out.
        if lease.validity() == FlightValidity::Invalidated {
            return;
        }
        entries.retain(|_, entry| entry.inserted_at.elapsed() < self.ttl);
        if entries.len() >= shard.capacity {
            // Evict the oldest. Its allocation is zeroized now unless an
            // in-flight dispatch still owns a private reference.
            if let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, entry)| entry.inserted_at)
                .map(|(key, _)| *key)
            {
                entries.remove(&oldest);
            }
        }
        entries.insert(
            key,
            CacheEntry {
                key: value,
                inserted_at: Instant::now(),
            },
        );
    }

    /// Drops every entry for a binding and retires every decrypt running for
    /// it. Returns how many cached entries were removed.
    #[must_use]
    pub fn invalidate(&self, workspace: WorkspaceId, binding: ProviderCredentialId) -> usize {
        self.revoke(&|key| key.workspace == workspace && key.binding == binding)
    }

    /// Drops every entry for a workspace and retires its running decrypts.
    #[must_use]
    pub fn invalidate_workspace(&self, workspace: WorkspaceId) -> usize {
        self.revoke(&|key| key.workspace == workspace)
    }

    fn revoke(&self, covered: &dyn Fn(&CredentialCacheKey) -> bool) -> usize {
        // Flights are marked before any shard is touched. That order is what
        // makes the guarded insert safe: an insert that has already passed its
        // check still holds the shard lock this removal pass needs, so the
        // entry it writes is taken back out on the next line.
        self.flights.invalidate_matching(covered);
        self.retain_all(|key| !covered(key))
    }

    /// How many entries are held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.shards
            .iter()
            .map(|shard| shard.entries.lock().map_or(0, |entries| entries.len()))
            .sum()
    }

    /// Whether the cache holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn shard(&self, key: &CredentialCacheKey) -> &CacheShard {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        let shard_count = u64::try_from(self.shards.len()).expect("at most sixteen shards");
        let index = usize::try_from(hasher.finish() % shard_count).expect("index is below sixteen");
        &self.shards[index]
    }

    fn retain_all(&self, mut keep: impl FnMut(&CredentialCacheKey) -> bool) -> usize {
        self.shards
            .iter()
            .map(|shard| {
                let Ok(mut entries) = shard.entries.lock() else {
                    return 0;
                };
                let before = entries.len();
                entries.retain(|key, _| keep(key));
                before - entries.len()
            })
            .sum()
    }
}
