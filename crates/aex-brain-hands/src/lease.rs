//! Bounded process-local Hands endpoint/token leases.
//!
//! The cache is an accelerator only. Durable runtime activity remains decisive and every
//! lookup supplies the generation's current provider identity and lifecycle fence. A token
//! refresh creates a new process-local token generation; a late reply from an older token
//! generation cannot invalidate its successor.

use core::num::NonZeroUsize;
use std::collections::HashMap;

use aex_hands_protocol::rpc::Fence;
use aex_runtime_control::lifecycle::MicrovmId;
use aex_wire::ids::GenerationId;
use aex_wire::types::Timestamp;

/// Five minutes remain when a thirty-minute provider token reaches its documented refresh
/// point. A cached token is never handed out inside that margin.
pub(crate) const TOKEN_REUSE_MARGIN_MS: i64 = 5 * 60 * 1_000;

/// Durable facts that must still match before a process-local lease can be reused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LeaseIdentity {
    pub(crate) generation: GenerationId,
    pub(crate) fence: Fence,
    pub(crate) microvm: MicrovmId,
}

/// One checked-out lease. `token_generation` distinguishes a refreshed credential from a
/// late response that used the same durable identity with the previous credential.
#[derive(Clone)]
pub(crate) struct LeaseHandle<T> {
    pub(crate) identity: LeaseIdentity,
    pub(crate) token_generation: u64,
    pub(crate) value: T,
}

impl<T> core::fmt::Debug for LeaseHandle<T> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("LeaseHandle")
            .field("identity", &self.identity)
            .field("token_generation", &self.token_generation)
            .field("value", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GuestObservation {
    Recorded,
    Unchanged,
    Invalidated,
    StaleLease,
}

struct Entry<T> {
    handle: LeaseHandle<T>,
    expires_at: Timestamp,
    guest_revision: Option<u32>,
    agent_build: Option<[u8; 8]>,
    last_used: u64,
}

/// A generation-partitioned LRU with an exact entry ceiling.
pub(crate) struct EndpointLeaseCache<T> {
    capacity: NonZeroUsize,
    entries: HashMap<GenerationId, Entry<T>>,
    use_serial: u64,
    next_token_generation: u64,
}

impl<T> EndpointLeaseCache<T> {
    pub(crate) fn new(capacity: NonZeroUsize) -> Self {
        Self {
            capacity,
            entries: HashMap::with_capacity(capacity.get()),
            use_serial: 0,
            next_token_generation: 0,
        }
    }

    pub(crate) fn get(&mut self, identity: &LeaseIdentity, now: Timestamp) -> Option<LeaseHandle<T>>
    where
        T: Clone,
    {
        let reusable_until = now.unix_millis().saturating_add(TOKEN_REUSE_MARGIN_MS);
        let reusable = self.entries.get(&identity.generation).is_some_and(|entry| {
            entry.handle.identity == *identity && entry.expires_at.unix_millis() > reusable_until
        });
        if !reusable {
            self.entries.remove(&identity.generation);
            return None;
        }
        self.use_serial = self.use_serial.saturating_add(1);
        let entry = self.entries.get_mut(&identity.generation)?;
        entry.last_used = self.use_serial;
        Some(entry.handle.clone())
    }

    pub(crate) fn insert(
        &mut self,
        identity: LeaseIdentity,
        expires_at: Timestamp,
        value: T,
    ) -> LeaseHandle<T>
    where
        T: Clone,
    {
        self.use_serial = self.use_serial.saturating_add(1);
        self.next_token_generation = self.next_token_generation.saturating_add(1);
        if !self.entries.contains_key(&identity.generation)
            && self.entries.len() == self.capacity.get()
        {
            self.evict_lru();
        }
        let handle = LeaseHandle {
            identity,
            token_generation: self.next_token_generation,
            value,
        };
        self.entries.insert(
            handle.identity.generation,
            Entry {
                handle: handle.clone(),
                expires_at,
                guest_revision: None,
                agent_build: None,
                last_used: self.use_serial,
            },
        );
        handle
    }

    pub(crate) fn observe_guest(
        &mut self,
        lease: &LeaseHandle<T>,
        guest_revision: u32,
        agent_build: [u8; 8],
    ) -> GuestObservation {
        let Some(entry) = self.entries.get_mut(&lease.identity.generation) else {
            return GuestObservation::StaleLease;
        };
        if entry.handle.identity != lease.identity
            || entry.handle.token_generation != lease.token_generation
        {
            return GuestObservation::StaleLease;
        }
        match (entry.guest_revision, entry.agent_build) {
            (None, None) => {
                entry.guest_revision = Some(guest_revision);
                entry.agent_build = Some(agent_build);
                GuestObservation::Recorded
            }
            (Some(known_revision), Some(known_build))
                if known_revision == guest_revision && known_build == agent_build =>
            {
                GuestObservation::Unchanged
            }
            _ => {
                self.entries.remove(&lease.identity.generation);
                GuestObservation::Invalidated
            }
        }
    }

    pub(crate) fn invalidate(&mut self, generation: GenerationId) {
        self.entries.remove(&generation);
    }

    /// Drops a lease only when it is still the cache's current token generation.
    ///
    /// A request can fail after a refresh has already installed a successor. In
    /// that case the old request must not evict the successor's credential.
    pub(crate) fn invalidate_lease(&mut self, lease: &LeaseHandle<T>) -> bool {
        let matches = self
            .entries
            .get(&lease.identity.generation)
            .is_some_and(|entry| {
                entry.handle.identity == lease.identity
                    && entry.handle.token_generation == lease.token_generation
            });
        if matches {
            self.entries.remove(&lease.identity.generation);
        }
        matches
    }

    fn evict_lru(&mut self) {
        let oldest = self
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(generation, _)| *generation);
        if let Some(generation) = oldest {
            self.entries.remove(&generation);
        }
    }
}

impl<T> core::fmt::Debug for EndpointLeaseCache<T> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("EndpointLeaseCache")
            .field("capacity", &self.capacity)
            .field("entries", &self.entries.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use core::num::NonZeroUsize;

    use aex_hands_protocol::rpc::Fence;
    use aex_runtime_control::lifecycle::MicrovmId;
    use aex_wire::ids::{GenerationId, PrefixedId as _, Uuid7};
    use aex_wire::types::Timestamp;

    use super::{EndpointLeaseCache, GuestObservation, LeaseIdentity, TOKEN_REUSE_MARGIN_MS};

    fn generation(seed: u8) -> GenerationId {
        GenerationId::from_uuid7(Uuid7::compose(u64::from(seed), [seed; 10]))
    }

    fn identity(seed: u8, fence: u64, microvm: &str) -> LeaseIdentity {
        LeaseIdentity {
            generation: generation(seed),
            fence: Fence(fence),
            microvm: MicrovmId(microvm.to_owned()),
        }
    }

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("test timestamp")
    }

    fn cache(capacity: usize) -> EndpointLeaseCache<&'static str> {
        EndpointLeaseCache::new(NonZeroUsize::new(capacity).expect("positive capacity"))
    }

    #[test]
    fn reuse_requires_the_exact_generation_fence_provider_and_safe_token_window() {
        let mut cache = cache(4);
        let exact = identity(1, 7, "vm-a");
        cache.insert(exact.clone(), at(1_800_000), "lease-a");

        assert_eq!(
            cache.get(&exact, at(0)).map(|lease| lease.value),
            Some("lease-a")
        );
        assert!(cache.get(&identity(1, 8, "vm-a"), at(0)).is_none());

        cache.insert(exact.clone(), at(1_800_000), "lease-b");
        let refresh_at = 1_800_000 - TOKEN_REUSE_MARGIN_MS;
        assert!(cache.get(&exact, at(refresh_at)).is_none());

        cache.insert(exact.clone(), at(1_800_000), "lease-c");
        assert!(cache.get(&identity(1, 7, "vm-b"), at(0)).is_none());
    }

    #[test]
    fn a_guest_incarnation_or_build_change_invalidates_the_lease() {
        let mut cache = cache(2);
        let exact = identity(1, 7, "vm-a");
        let lease = cache.insert(exact.clone(), at(1_800_000), "lease");
        assert_eq!(
            cache.observe_guest(&lease, 3, [4; 8]),
            GuestObservation::Recorded
        );
        assert_eq!(
            cache.observe_guest(&lease, 3, [4; 8]),
            GuestObservation::Unchanged
        );
        assert_eq!(
            cache.observe_guest(&lease, 4, [4; 8]),
            GuestObservation::Invalidated
        );
        assert!(cache.get(&exact, at(0)).is_none());
    }

    #[test]
    fn a_late_reply_from_an_old_token_cannot_evict_its_refresh() {
        let mut cache = cache(2);
        let exact = identity(1, 7, "vm-a");
        let old = cache.insert(exact.clone(), at(1_800_000), "old");
        let fresh = cache.insert(exact.clone(), at(3_600_000), "fresh");

        assert_eq!(
            cache.observe_guest(&old, 99, [9; 8]),
            GuestObservation::StaleLease
        );
        assert_eq!(
            cache.get(&exact, at(0)).map(|lease| lease.value),
            Some("fresh")
        );
        assert_eq!(
            cache.observe_guest(&fresh, 1, [1; 8]),
            GuestObservation::Recorded
        );
    }

    #[test]
    fn a_lifecycle_fence_change_invalidates_the_old_lease_without_touching_the_successor() {
        let mut cache = cache(2);
        let old_identity = identity(1, 7, "vm-a");
        let old = cache.insert(old_identity.clone(), at(1_800_000), "old");
        let successor_identity = identity(1, 8, "vm-a");
        let successor = cache.insert(successor_identity.clone(), at(3_600_000), "successor");

        assert!(!cache.invalidate_lease(&old));
        assert_eq!(
            cache
                .get(&successor_identity, at(0))
                .map(|lease| lease.value),
            Some("successor")
        );
        assert!(!cache.invalidate_lease(&old));
        assert_eq!(
            cache
                .get(&successor_identity, at(0))
                .map(|lease| lease.value),
            Some("successor")
        );
        assert_eq!(
            cache.observe_guest(&successor, 1, [1; 8]),
            GuestObservation::Recorded
        );
    }

    #[test]
    fn terminal_lifecycle_transition_drops_the_generation_lease() {
        let mut cache = cache(1);
        let exact = identity(1, 7, "vm-a");
        cache.insert(exact.clone(), at(1_800_000), "lease");
        cache.invalidate(exact.generation);
        assert!(cache.get(&exact, at(0)).is_none());
    }

    #[test]
    fn one_tenant_churn_cannot_grow_the_cache_past_its_ceiling() {
        let mut cache = cache(2);
        let first = identity(1, 1, "vm-a");
        let second = identity(2, 1, "vm-b");
        let third = identity(3, 1, "vm-c");
        cache.insert(first.clone(), at(1_800_000), "first");
        cache.insert(second.clone(), at(1_800_000), "second");
        let _ = cache.get(&second, at(0));
        cache.insert(third.clone(), at(1_800_000), "third");

        assert!(cache.get(&first, at(0)).is_none());
        assert_eq!(
            cache.get(&second, at(0)).map(|lease| lease.value),
            Some("second")
        );
        assert_eq!(
            cache.get(&third, at(0)).map(|lease| lease.value),
            Some("third")
        );
    }

    #[test]
    fn debug_never_renders_a_cached_value() {
        let mut cache = cache(1);
        cache.insert(identity(1, 1, "vm-a"), at(1_800_000), "secret-token-value");
        let rendered = format!("{cache:?}");
        assert!(!rendered.contains("secret-token-value"));
    }
}
