//! Credential resolution, decrypted-key cache, and single-flight.
//!
//! The port definitions — the binding type, the directory/decryptor traits,
//! the error vocabulary, and the decrypted key — live in
//! `aex-brain-provider-custody` and are re-exported here until this crate's
//! transport core is retired (model-provider simplification 2026-08-13).

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aex_wire::ids::{OrganizationId, ProviderCredentialId, WorkspaceId};
use aex_wire::provider::ProviderId;
use zeroize::Zeroizing;

pub use aex_brain_domain::wire_pending::SessionCredentialPin;
pub use aex_brain_provider_custody::credential::{
    BindingState, CredentialResolveError, CredentialRevision, DenyAllCredentialDecryptor,
    DenyAllCredentialDirectory, ProviderApiKey, ProviderCredentialBinding,
    ProviderCredentialDecryptor, ProviderCredentialDirectory, RevocationEpoch,
};
pub use aex_model_catalog::canonical::CredentialBindingRef;

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
    pub revision: CredentialRevision,
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
/// in one reference-counted, zeroizing allocation plus the sensitive
/// `HeaderValue` used by transport. Concurrent cold misses for one identity
/// share one decrypt and that one allocation rather than each calling the
/// authority. Revocation calls [`CredentialCache::invalidate`], which drops
/// every matching cache reference and retires every matching running decrypt.
/// An already in-flight dispatch may keep its private reference until that
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
    ///
    /// `capacity` bounds both the decrypted keys held and the decrypts running
    /// at once, so there is one number to reason about: the cache never tracks
    /// more distinct credentials than it could hold results for. The live
    /// flight count is really bounded far below that by admission — a flight
    /// exists only while a caller sits inside [`CredentialCache::decrypt`], and
    /// the Brain admits sixteen concurrent activations — so at the default
    /// capacity of [`CACHE_CAPACITY`] the ceiling is a memory backstop rather
    /// than a limit a workload reaches. A caller past it decrypts alone and
    /// does not cache, because a decrypt no flight covers cannot observe a
    /// revocation that lands while it runs.
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
    /// A hit clones only a private `Arc`, not the plaintext string, while the
    /// shard lock is held. Concurrent cold misses for one identity produce one
    /// decryptor call and one zeroizing allocation, shared by every caller. A
    /// concurrent [`CredentialCache::invalidate`] removes future hits, retires
    /// the running decrypt and keeps its result out of the cache, but does not
    /// invalidate memory held by an in-flight caller; the shared allocation is
    /// zeroized when its final owner drops.
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
    ///
    /// A flight exists only while a caller is inside
    /// [`CredentialCache::decrypt`], so this returns to zero once a burst
    /// drains. It holds no plaintext of its own to leak: it is a count.
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
        // Read while the shard lock is held. `invalidate` marks every matching
        // flight before it touches a shard, so a mark landing after this read
        // belongs to a revocation whose own removal pass must first wait for
        // this lock and then takes the entry written below straight back out.
        // Reading it any earlier leaves a window where a revoked key is cached
        // after the revocation finished.
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
    ///
    /// The count covers entries only. A running decrypt is marked instead:
    /// its result never enters the cache, no later caller can join it, and the
    /// caller already inside it keeps the reference it asked for until the
    /// pre-send revalidation fence refuses the send.
    #[must_use]
    pub fn invalidate(&self, workspace: WorkspaceId, binding: ProviderCredentialId) -> usize {
        self.revoke(&|key| key.workspace == workspace && key.binding == binding)
    }

    /// Drops every entry for a workspace and retires its running decrypts.
    ///
    /// Returns how many cached entries were removed, on the same terms as
    /// [`CredentialCache::invalidate`].
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

/// Resolves a binding and enforces the affinity and revocation rules.
///
/// # Errors
///
/// Returns [`CredentialResolveError`] for a missing, deleted, revoked or
/// provider-mismatched binding. Every arm happens before the send gate is
/// consumed, so every one is `DispatchProof::NotSent`.
pub async fn resolve(
    directory: &dyn ProviderCredentialDirectory,
    organization: OrganizationId,
    workspace: WorkspaceId,
    provider: ProviderId,
    requested: Option<ProviderCredentialId>,
    pin: Option<&SessionCredentialPin>,
) -> Result<ProviderCredentialBinding, CredentialResolveError> {
    let binding = directory
        .resolve(organization, workspace, provider, requested)
        .await?;

    if binding.provider != provider {
        return Err(CredentialResolveError::ProviderMismatch {
            binding: binding.provider,
            requested: provider,
        });
    }
    match binding.state {
        BindingState::Deleted => return Err(CredentialResolveError::Deleted),
        BindingState::Revoked => {
            return Err(CredentialResolveError::Revoked {
                admitted: pin.map_or(binding.revocation_epoch, |pin| {
                    RevocationEpoch(pin.revocation_epoch)
                }),
                current: binding.revocation_epoch,
            });
        }
        BindingState::Ready => {}
    }
    if let Some(pin) = pin
        && binding.revocation_epoch > RevocationEpoch(pin.revocation_epoch)
    {
        return Err(CredentialResolveError::Revoked {
            admitted: RevocationEpoch(pin.revocation_epoch),
            current: binding.revocation_epoch,
        });
    }
    Ok(binding)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    use aex_wire::ids::{OrganizationId, PrefixedId, ProviderCredentialId, WorkspaceId};
    use aex_wire::provider::ProviderId;
    use aex_wire::types::Region;
    use tokio::sync::Notify;
    use zeroize::Zeroizing;

    use super::{
        BindingState, CredentialCache, CredentialResolveError, CredentialRevision,
        DenyAllCredentialDirectory, ProviderApiKey, ProviderCredentialBinding,
        ProviderCredentialDecryptor, ProviderCredentialDirectory, SessionCredentialPin, resolve,
    };
    use crate::wire_pending::{
        BoxFuture, CiphertextRef, EncryptionContext, RevocationEpoch, SourceGeneration,
    };

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(aex_wire::Uuid7::compose(1, [1; 10]))
    }

    fn organization() -> OrganizationId {
        OrganizationId::from_uuid7(aex_wire::Uuid7::compose(1, [2; 10]))
    }

    fn binding_id(seed: u8) -> ProviderCredentialId {
        ProviderCredentialId::from_uuid7(aex_wire::Uuid7::compose(2, [seed; 10]))
    }

    fn binding(provider: ProviderId, state: BindingState, epoch: u64) -> ProviderCredentialBinding {
        ProviderCredentialBinding {
            id: binding_id(7),
            workspace: workspace(),
            provider,
            revision: CredentialRevision(1),
            generation: SourceGeneration(1),
            revocation_epoch: RevocationEpoch(epoch),
            is_default: true,
            state,
            ciphertext: CiphertextRef {
                key_generation: 1,
                wrapped_key: vec![1; 32],
                nonce: Vec::new(),
                ciphertext: vec![2; 32],
            },
            context: EncryptionContext {
                plane: aex_secret_domain::context::Plane::Dev,
                region: Region::EuWest1,
                organization: organization(),
                workspace: workspace(),
                name: aex_secret_domain::SecretName::parse("provider-key").expect("name"),
                generation: SourceGeneration(1),
                custody_revision: None,
            },
            context_digest: EncryptionContext {
                plane: aex_secret_domain::context::Plane::Dev,
                region: Region::EuWest1,
                organization: organization(),
                workspace: workspace(),
                name: aex_secret_domain::SecretName::parse("provider-key").expect("name"),
                generation: SourceGeneration(1),
                custody_revision: None,
            }
            .digest(),
        }
    }

    struct Fixed(ProviderCredentialBinding);
    impl ProviderCredentialDirectory for Fixed {
        fn resolve(
            &self,
            _organization: OrganizationId,
            _workspace: WorkspaceId,
            _provider: ProviderId,
            _id: Option<ProviderCredentialId>,
        ) -> BoxFuture<'_, Result<ProviderCredentialBinding, CredentialResolveError>> {
            let binding = self.0.clone();
            Box::pin(async move { Ok(binding) })
        }

        fn revalidate<'a>(
            &'a self,
            _binding: &'a ProviderCredentialBinding,
        ) -> BoxFuture<'a, Result<RevocationEpoch, CredentialResolveError>> {
            let epoch = self.0.revocation_epoch;
            Box::pin(async move { Ok(epoch) })
        }
    }

    struct Constant(&'static str);
    impl ProviderCredentialDecryptor for Constant {
        fn decrypt<'a>(
            &'a self,
            _binding: &'a ProviderCredentialBinding,
            _now: aex_wire::types::Timestamp,
        ) -> BoxFuture<'a, Result<ProviderApiKey, CredentialResolveError>> {
            let text = self.0.to_owned();
            Box::pin(async move { Ok(ProviderApiKey::new(text)) })
        }
    }

    /// What the gated fixture decryptor does once it is released.
    #[derive(Clone, Copy)]
    enum FixtureOutcome {
        Succeeds(&'static str),
        Refuses,
    }

    /// A decryptor that counts its calls and finishes only when told to.
    ///
    /// Holding every call open is what makes "exactly one decrypt" observable
    /// rather than timing-dependent: the assertion is made while the whole
    /// burst is parked, not after it has drained.
    struct Gated {
        outcome: FixtureOutcome,
        calls: AtomicUsize,
        release: Notify,
    }

    impl Gated {
        fn new(outcome: FixtureOutcome) -> Self {
            Self {
                outcome,
                calls: AtomicUsize::new(0),
                release: Notify::new(),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(AtomicOrdering::Relaxed)
        }

        fn release(&self) {
            self.release.notify_waiters();
        }
    }

    impl ProviderCredentialDecryptor for Gated {
        fn decrypt<'a>(
            &'a self,
            _binding: &'a ProviderCredentialBinding,
            _now: aex_wire::types::Timestamp,
        ) -> BoxFuture<'a, Result<ProviderApiKey, CredentialResolveError>> {
            Box::pin(async move {
                self.calls.fetch_add(1, AtomicOrdering::Relaxed);
                self.release.notified().await;
                match self.outcome {
                    FixtureOutcome::Succeeds(text) => Ok(ProviderApiKey::new(text.to_owned())),
                    FixtureOutcome::Refuses => Err(CredentialResolveError::DecryptFailed),
                }
            })
        }
    }

    fn now() -> aex_wire::types::Timestamp {
        aex_wire::types::Timestamp::from_unix_millis(1).expect("timestamp")
    }

    #[tokio::test]
    async fn the_placeholder_directory_refuses_everything() {
        let directory = DenyAllCredentialDirectory;
        let error = resolve(
            &directory,
            organization(),
            workspace(),
            ProviderId::Anthropic,
            None,
            None,
        )
        .await
        .expect_err("the placeholder must refuse");
        assert_eq!(
            error,
            CredentialResolveError::RegistrationAuthorityUnavailable
        );
        assert_eq!(
            error.error_code(),
            aex_wire::ErrorCode::ProviderCredentialNotFound
        );
    }

    #[tokio::test]
    async fn a_provider_mismatch_is_typed_never_a_silent_substitution() {
        let directory = Fixed(binding(ProviderId::Openai, BindingState::Ready, 0));
        let error = resolve(
            &directory,
            organization(),
            workspace(),
            ProviderId::Anthropic,
            None,
            None,
        )
        .await
        .expect_err("a mismatch must fail");
        assert_eq!(
            error,
            CredentialResolveError::ProviderMismatch {
                binding: ProviderId::Openai,
                requested: ProviderId::Anthropic,
            }
        );
    }

    #[tokio::test]
    async fn a_revoked_binding_fails() {
        let directory = Fixed(binding(ProviderId::Openai, BindingState::Revoked, 3));
        let error = resolve(
            &directory,
            organization(),
            workspace(),
            ProviderId::Openai,
            None,
            None,
        )
        .await
        .expect_err("a revoked binding must fail");
        assert!(matches!(error, CredentialResolveError::Revoked { .. }));
        assert_eq!(
            error.error_code(),
            aex_wire::ErrorCode::ProviderCredentialRevoked
        );
    }

    #[tokio::test]
    async fn a_deleted_binding_fails() {
        let directory = Fixed(binding(ProviderId::Openai, BindingState::Deleted, 0));
        let error = resolve(
            &directory,
            organization(),
            workspace(),
            ProviderId::Openai,
            None,
            None,
        )
        .await
        .expect_err("a deleted binding must fail");
        assert_eq!(error, CredentialResolveError::Deleted);
    }

    #[tokio::test]
    async fn an_epoch_bump_overrides_an_existing_session_pin() {
        let directory = Fixed(binding(ProviderId::Openai, BindingState::Ready, 5));
        let pin = SessionCredentialPin::new(binding_id(7), 1, 1, 4).expect("non-zero fixture pin");
        let error = resolve(
            &directory,
            organization(),
            workspace(),
            ProviderId::Openai,
            None,
            Some(&pin),
        )
        .await
        .expect_err("a bumped epoch must override the pin");
        assert_eq!(
            error,
            CredentialResolveError::Revoked {
                admitted: RevocationEpoch(4),
                current: RevocationEpoch(5),
            }
        );
    }

    #[tokio::test]
    async fn a_matching_epoch_still_resolves() {
        let directory = Fixed(binding(ProviderId::Openai, BindingState::Ready, 4));
        let pin = SessionCredentialPin::new(binding_id(7), 1, 1, 4).expect("non-zero fixture pin");
        resolve(
            &directory,
            organization(),
            workspace(),
            ProviderId::Openai,
            None,
            Some(&pin),
        )
        .await
        .expect("an unrevoked pin resolves");
    }

    #[tokio::test]
    async fn the_cache_reuses_a_fresh_entry_and_expires_a_stale_one() {
        let cache = CredentialCache::new(4, core::time::Duration::from_millis(50));
        let binding = binding(ProviderId::Openai, BindingState::Ready, 0);
        let first = cache
            .decrypt(&binding, &Constant("sk-one"), now())
            .await
            .expect("first decrypt");
        assert_eq!(cache.len(), 1);
        let reused = cache
            .decrypt(&binding, &Constant("sk-two"), now())
            .await
            .expect("cached");
        assert_eq!(reused.plaintext(), "sk-one");
        assert!(
            std::sync::Arc::ptr_eq(first.shared_allocation(), reused.shared_allocation()),
            "a hot hit should share the zeroizing allocation, not copy plaintext"
        );

        tokio::time::sleep(core::time::Duration::from_millis(60)).await;
        let refreshed = cache
            .decrypt(&binding, &Constant("sk-two"), now())
            .await
            .expect("expired then decrypted again");
        assert_eq!(refreshed.plaintext(), "sk-two");
    }

    #[tokio::test]
    async fn a_rotation_never_hits_a_stale_entry() {
        let cache = CredentialCache::default();
        let first = binding(ProviderId::Openai, BindingState::Ready, 0);
        let mut rotated = first.clone();
        rotated.revision = CredentialRevision(2);

        cache
            .decrypt(&first, &Constant("sk-old"), now())
            .await
            .expect("first");
        let after = cache
            .decrypt(&rotated, &Constant("sk-new"), now())
            .await
            .expect("rotated");
        assert_eq!(after.plaintext(), "sk-new");
        assert_eq!(cache.len(), 2);
    }

    #[tokio::test]
    async fn another_organization_never_hits_a_cached_plaintext() {
        let cache = CredentialCache::default();
        let first = binding(ProviderId::Openai, BindingState::Ready, 0);
        cache
            .decrypt(&first, &Constant("sk-first"), now())
            .await
            .expect("first decrypt");
        let mut second = first;
        second.context.organization =
            OrganizationId::from_uuid7(aex_wire::Uuid7::compose(1, [9; 10]));
        let resolved = cache
            .decrypt(&second, &Constant("sk-second"), now())
            .await
            .expect("organization-isolated decrypt");
        assert_eq!(resolved.plaintext(), "sk-second");
        assert_eq!(cache.len(), 2);
    }

    #[tokio::test]
    async fn invalidate_drops_exactly_the_matching_entries() {
        let cache = CredentialCache::default();
        let binding = binding(ProviderId::Openai, BindingState::Ready, 0);
        cache
            .decrypt(&binding, &Constant("sk-one"), now())
            .await
            .expect("decrypt");
        assert_eq!(cache.invalidate(workspace(), binding_id(9)), 0);
        assert_eq!(cache.invalidate(workspace(), binding.id), 1);
        assert!(cache.is_empty());
    }

    #[tokio::test]
    async fn the_cache_stays_inside_its_capacity() {
        let cache = CredentialCache::new(2, core::time::Duration::from_mins(1));
        for seed in 0..8u8 {
            let mut entry = binding(ProviderId::Openai, BindingState::Ready, 0);
            entry.revision = CredentialRevision(u64::from(seed));
            cache
                .decrypt(&entry, &Constant("sk-value"), now())
                .await
                .expect("decrypt");
        }
        assert!(cache.len() <= 2, "cache grew to {}", cache.len());
    }

    #[test]
    fn the_cache_debug_rendering_names_no_binding() {
        let cache = CredentialCache::default();
        assert_eq!(format!("{cache:?}"), "CredentialCache { .. }");
    }

    #[tokio::test]
    async fn a_cold_burst_of_one_hundred_callers_decrypts_once_into_one_allocation() {
        let cache = CredentialCache::default();
        let binding = binding(ProviderId::Openai, BindingState::Ready, 0);
        let decryptor = Gated::new(FixtureOutcome::Succeeds("sk-burst"));
        let mut burst = Box::pin(futures::future::join_all(
            (0..100).map(|_| cache.decrypt(&binding, &decryptor, now())),
        ));

        assert!(
            futures::poll!(&mut burst).is_pending(),
            "the whole burst must park behind one held-open decrypt"
        );
        assert_eq!(
            decryptor.calls(),
            1,
            "one cold burst must reach the credential authority once"
        );
        assert_eq!(cache.active_flights(), 1);

        decryptor.release();
        let keys = burst.await;
        let shared: Vec<&Arc<Zeroizing<String>>> = keys
            .iter()
            .map(|outcome| {
                let key = outcome.as_ref().expect("every caller receives the decrypt");
                assert_eq!(key.plaintext(), "sk-burst");
                key.shared_allocation()
            })
            .collect();
        let first = shared[0];
        assert!(
            shared
                .iter()
                .all(|candidate| Arc::ptr_eq(*candidate, first)),
            "a burst must share one zeroizing allocation, not copy plaintext per caller"
        );
        assert_eq!(
            Arc::strong_count(first),
            101,
            "one hundred callers plus the single cache entry"
        );
        assert_eq!(cache.len(), 1);
        assert_eq!(
            cache.active_flights(),
            0,
            "a settled flight must not be left behind"
        );
    }

    #[tokio::test]
    async fn a_different_revision_or_generation_never_shares_a_flight() {
        let cache = CredentialCache::default();
        let base = binding(ProviderId::Openai, BindingState::Ready, 0);
        let mut rotated = base.clone();
        rotated.revision = CredentialRevision(2);
        let mut regenerated = base.clone();
        regenerated.generation = SourceGeneration(2);
        regenerated.context.generation = SourceGeneration(2);
        regenerated.context_digest = regenerated.context.digest();

        let decryptor = Gated::new(FixtureOutcome::Succeeds("sk-distinct"));
        let mut burst = Box::pin(futures::future::join_all((0..30).flat_map(|_| {
            [
                cache.decrypt(&base, &decryptor, now()),
                cache.decrypt(&rotated, &decryptor, now()),
                cache.decrypt(&regenerated, &decryptor, now()),
            ]
        })));

        assert!(futures::poll!(&mut burst).is_pending());
        assert_eq!(
            decryptor.calls(),
            3,
            "one flight per exact cache identity, never one for the binding"
        );
        assert_eq!(cache.active_flights(), 3);

        decryptor.release();
        assert!(burst.await.iter().all(Result::is_ok));
        assert_eq!(cache.len(), 3);
        assert_eq!(cache.active_flights(), 0);
    }

    #[tokio::test]
    async fn a_failed_flight_is_shared_by_every_waiter_and_never_positively_cached() {
        let cache = CredentialCache::default();
        let binding = binding(ProviderId::Openai, BindingState::Ready, 0);
        let decryptor = Gated::new(FixtureOutcome::Refuses);
        let mut burst = Box::pin(futures::future::join_all(
            (0..16).map(|_| cache.decrypt(&binding, &decryptor, now())),
        ));

        assert!(futures::poll!(&mut burst).is_pending());
        assert_eq!(decryptor.calls(), 1);

        decryptor.release();
        for outcome in burst.await {
            match outcome {
                Ok(_) => panic!("a refused decrypt must not produce a key"),
                Err(error) => assert_eq!(error, CredentialResolveError::DecryptFailed),
            }
        }
        assert!(
            cache.is_empty(),
            "a failure must never enter the positive cache"
        );
        assert_eq!(cache.active_flights(), 0);

        let recovered = cache
            .decrypt(&binding, &Constant("sk-after-failure"), now())
            .await
            .expect("nothing negative was kept, so the next caller resolves again");
        assert_eq!(recovered.plaintext(), "sk-after-failure");
    }

    #[tokio::test]
    async fn an_invalidation_during_a_flight_keeps_its_result_out_of_the_cache() {
        let cache = CredentialCache::default();
        let binding = binding(ProviderId::Openai, BindingState::Ready, 0);
        let decryptor = Gated::new(FixtureOutcome::Succeeds("sk-revoked"));
        let mut burst = Box::pin(futures::future::join_all(
            (0..4).map(|_| cache.decrypt(&binding, &decryptor, now())),
        ));

        assert!(futures::poll!(&mut burst).is_pending());
        assert_eq!(cache.active_flights(), 1);
        assert_eq!(
            cache.invalidate(workspace(), binding.id),
            0,
            "nothing was cached yet, so only the flight is affected"
        );
        assert_eq!(
            cache.active_flights(),
            0,
            "a revocation retires the decrypt that is still running"
        );

        decryptor.release();
        for outcome in burst.await {
            // The callers already inside the decrypt keep the reference they
            // asked for. Erasing it is not attempted; the pre-send
            // revalidation fence is what refuses their send.
            let key = outcome.expect("a retired flight still settles its own waiters");
            assert_eq!(key.plaintext(), "sk-revoked");
        }
        assert!(
            cache.is_empty(),
            "a revoked flight must never be positively cached"
        );

        let refreshed = cache
            .decrypt(&binding, &Constant("sk-fresh"), now())
            .await
            .expect("a later caller resolves and decrypts again");
        assert_eq!(refreshed.plaintext(), "sk-fresh");
        assert_eq!(decryptor.calls(), 1);
    }

    #[tokio::test]
    async fn a_cancelled_leader_hands_its_identity_to_the_next_caller() {
        let cache = CredentialCache::default();
        let binding = binding(ProviderId::Openai, BindingState::Ready, 0);
        let decryptor = Gated::new(FixtureOutcome::Succeeds("sk-re-elected"));
        let mut leader = Box::pin(cache.decrypt(&binding, &decryptor, now()));
        let mut follower = Box::pin(cache.decrypt(&binding, &decryptor, now()));

        assert!(futures::poll!(&mut leader).is_pending());
        assert!(futures::poll!(&mut follower).is_pending());
        assert_eq!(cache.active_flights(), 1);
        assert_eq!(decryptor.calls(), 1);

        // Exactly what an effect deadline or a cancellation does to the leader.
        drop(leader);
        assert_eq!(cache.active_flights(), 0);

        assert!(futures::poll!(&mut follower).is_pending());
        assert_eq!(
            decryptor.calls(),
            2,
            "the follower must re-elect rather than wait on a leader that is gone"
        );
        assert_eq!(cache.active_flights(), 1);

        decryptor.release();
        let key = follower.await.expect("the re-elected caller completes");
        assert_eq!(key.plaintext(), "sk-re-elected");
    }

    #[tokio::test]
    async fn a_concurrent_burst_of_distinct_identities_stays_inside_its_capacity() {
        let cache = CredentialCache::new(2, core::time::Duration::from_mins(1));
        let bindings: Vec<ProviderCredentialBinding> = (0..8u8)
            .map(|seed| {
                let mut entry = binding(ProviderId::Openai, BindingState::Ready, 0);
                entry.revision = CredentialRevision(u64::from(seed));
                entry
            })
            .collect();
        let decryptor = Gated::new(FixtureOutcome::Succeeds("sk-capped"));
        let mut burst = Box::pin(futures::future::join_all(
            bindings
                .iter()
                .map(|entry| cache.decrypt(entry, &decryptor, now())),
        ));

        assert!(futures::poll!(&mut burst).is_pending());
        assert!(
            cache.active_flights() <= 2,
            "flights grew to {}",
            cache.active_flights()
        );

        decryptor.release();
        assert!(burst.await.iter().all(Result::is_ok));
        assert!(cache.len() <= 2, "cache grew to {}", cache.len());
        assert_eq!(cache.active_flights(), 0);
    }

    #[test]
    fn a_binding_ref_carries_only_non_secret_identity() {
        let binding = binding(ProviderId::Openai, BindingState::Ready, 0);
        let reference = binding.receipt_ref();
        let rendered = serde_json::to_string(&reference).expect("serialize");
        assert!(!rendered.contains("kms://"), "{rendered}");
        assert!(rendered.contains("pcr_"), "{rendered}");
    }
}
