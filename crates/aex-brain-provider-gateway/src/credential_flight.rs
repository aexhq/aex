//! Keyed single-flight for provider-credential decryption.
//!
//! # Why this exists
//!
//! `CredentialCache::decrypt` used to check its map, call the decryptor on a
//! miss, then insert. Every concurrent cold miss for one credential passed that
//! check, so one cold burst at the Brain activation cap became that many
//! independent authority decrypts of one key generation: that much KMS latency
//! and spend, that many chances to be throttled, and one plaintext allocation
//! per caller instead of one per credential.
//!
//! A flight is the one running decrypt for one exact cache identity. Every
//! other caller for that identity waits on it and receives the same
//! `Arc<Zeroizing<String>>`.
//!
//! # What a flight deliberately does not do
//!
//! It does not reach into a dispatch that already holds plaintext. A revocation
//! landing mid-flight marks the flight, retires it so no later caller can join,
//! and keeps its result out of the positive cache. A caller already inside the
//! decrypt keeps the reference it asked for and is stopped by the pre-send
//! revalidation fence — not by erasing memory that may already have been
//! written to a socket.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::watch;
use zeroize::Zeroizing;

use crate::credential::{CredentialCacheKey, CredentialResolveError};

/// How many times one caller re-elects a leader before decrypting on its own.
///
/// A re-election happens only when a leader's future is dropped — an effect
/// deadline or a cancellation — before it published anything. At most one such
/// event is owed to each concurrently admitted caller, so the approved
/// sixteen-activation Brain cap bounds the useful count. Past it the caller
/// stops waiting on other callers' cancellations and decrypts alone.
pub(crate) const MAX_FLIGHT_ELECTIONS: usize = 16;

/// What one flight settled on.
///
/// The success arm is the single reference-counted zeroizing allocation the
/// leader's decryptor produced. Every waiter receives that same allocation, so
/// a burst costs one plaintext copy rather than one per caller.
pub(crate) type FlightOutcome = Result<Arc<Zeroizing<String>>, CredentialResolveError>;

/// Whether a flight is still running or has published its outcome.
///
/// No `Debug`: the settled arm carries plaintext, and a derived rendering would
/// put it into any log line that formats a flight.
enum FlightPhase {
    /// The leader has not published yet.
    Running,
    /// The leader published this outcome.
    Settled(FlightOutcome),
}

/// Whether a revocation landed while a flight was still running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FlightValidity {
    /// Nothing revoked this identity while the decrypt ran.
    Intact,
    /// A revocation landed, so the result must not enter the positive cache.
    Invalidated,
}

/// One running decrypt.
///
/// The receiver held here is never received on; it is the template followers
/// clone, and it is what keeps the channel alive so the leader can always
/// publish even after the last follower has walked away.
struct Flight {
    invalidated: AtomicBool,
    phase: watch::Receiver<FlightPhase>,
}

/// The bounded set of decrypts running right now.
///
/// The map is touched only on a cache miss, so one mutex is cheaper than the
/// sharding the positive cache needs for its hot path.
pub(crate) struct FlightRegistry {
    running: Mutex<HashMap<CredentialCacheKey, Arc<Flight>>>,
    capacity: usize,
}

/// What one caller may do about a cold miss.
pub(crate) enum FlightAdmission<'registry> {
    /// This caller owns the decrypt and must publish its outcome.
    Leading(FlightLease<'registry>),
    /// Another caller owns the decrypt; wait for what it publishes.
    Following(FlightFollower),
    /// The registry is full. Decrypt alone — and do not cache the result,
    /// because a decrypt no flight covers cannot observe a revocation that
    /// lands while it runs.
    Unregistered,
}

impl FlightRegistry {
    /// Builds a registry that tracks at most `capacity` concurrent decrypts.
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            running: Mutex::new(HashMap::new()),
            capacity,
        }
    }

    /// Admits one caller against the complete cache identity.
    pub(crate) fn admit(&self, key: CredentialCacheKey) -> FlightAdmission<'_> {
        let Ok(mut running) = self.running.lock() else {
            return FlightAdmission::Unregistered;
        };
        if let Some(flight) = running.get(&key) {
            return FlightAdmission::Following(FlightFollower {
                phase: flight.phase.clone(),
            });
        }
        if running.len() >= self.capacity {
            return FlightAdmission::Unregistered;
        }
        let (sender, receiver) = watch::channel(FlightPhase::Running);
        let flight = Arc::new(Flight {
            invalidated: AtomicBool::new(false),
            phase: receiver,
        });
        running.insert(key, Arc::clone(&flight));
        FlightAdmission::Leading(FlightLease {
            registry: self,
            key,
            flight,
            phase: sender,
        })
    }

    /// Marks and retires every flight whose identity a revocation covers.
    ///
    /// Marking and retiring are one critical section so that a leader either
    /// sees the mark or is already past the point where its insert can survive:
    /// the caller runs this before it touches the positive cache.
    pub(crate) fn invalidate_matching(&self, matches: &dyn Fn(&CredentialCacheKey) -> bool) {
        let Ok(mut running) = self.running.lock() else {
            return;
        };
        running.retain(|key, flight| {
            if matches(key) {
                flight.invalidated.store(true, Ordering::Release);
                return false;
            }
            true
        });
    }

    /// How many decrypts are running.
    pub(crate) fn active(&self) -> usize {
        self.running.lock().map_or(0, |running| running.len())
    }

    /// Removes a flight, but only if the registry still holds that exact one.
    ///
    /// Identity is the `Arc` pointer, not the key. A revocation retires a
    /// flight while its decrypt is still running and a later caller may already
    /// have registered a successor under the same key; the abandoned
    /// predecessor must not remove it.
    fn retire(&self, key: &CredentialCacheKey, flight: &Arc<Flight>) {
        let Ok(mut running) = self.running.lock() else {
            return;
        };
        if running
            .get(key)
            .is_some_and(|current| Arc::ptr_eq(current, flight))
        {
            running.remove(key);
        }
    }
}

/// The right to run one decrypt and publish its outcome.
pub(crate) struct FlightLease<'registry> {
    registry: &'registry FlightRegistry,
    key: CredentialCacheKey,
    flight: Arc<Flight>,
    phase: watch::Sender<FlightPhase>,
}

impl FlightLease<'_> {
    /// Whether a revocation landed while this flight was running.
    pub(crate) fn validity(&self) -> FlightValidity {
        if self.flight.invalidated.load(Ordering::Acquire) {
            FlightValidity::Invalidated
        } else {
            FlightValidity::Intact
        }
    }

    /// Publishes the outcome to every waiter, then retires the flight.
    ///
    /// Publishing first means a caller arriving in the gap between the two
    /// joins a flight that is about to hand it the value the cache was just
    /// given, rather than starting a second decrypt of the same key.
    pub(crate) fn settle(&self, outcome: FlightOutcome) {
        let _running = self.phase.send_replace(FlightPhase::Settled(outcome));
        self.registry.retire(&self.key, &self.flight);
    }
}

impl Drop for FlightLease<'_> {
    fn drop(&mut self) {
        // A leader whose future was dropped before it settled — an effect
        // deadline or a cancellation — must not leave behind a flight nobody
        // will ever publish. Retiring here also drops the sender, which is how
        // its followers learn to re-elect instead of waiting forever.
        self.registry.retire(&self.key, &self.flight);
    }
}

/// A caller waiting on somebody else's decrypt.
pub(crate) struct FlightFollower {
    phase: watch::Receiver<FlightPhase>,
}

/// What a follower observed.
pub(crate) enum FlightJoin {
    /// The leader published this outcome.
    Settled(FlightOutcome),
    /// The leader's future was dropped before it published anything, so
    /// waiting longer would wait forever.
    Abandoned,
}

impl FlightFollower {
    /// Waits for the leader's outcome.
    pub(crate) async fn joined(mut self) -> FlightJoin {
        loop {
            // The borrow guard is confined to this statement: holding it across
            // the await below would make every dispatch future non-`Send`.
            let settled = match &*self.phase.borrow_and_update() {
                FlightPhase::Running => None,
                FlightPhase::Settled(outcome) => Some(outcome.clone()),
            };
            if let Some(outcome) = settled {
                return FlightJoin::Settled(outcome);
            }
            if self.phase.changed().await.is_err() {
                return FlightJoin::Abandoned;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aex_wire::ids::{OrganizationId, PrefixedId, ProviderCredentialId, WorkspaceId};
    use zeroize::Zeroizing;

    use super::{FlightAdmission, FlightJoin, FlightRegistry, FlightValidity};
    use crate::credential::{CredentialCacheKey, CredentialRevision};
    use crate::wire_pending::SourceGeneration;

    fn identity(binding_seed: u8) -> CredentialCacheKey {
        CredentialCacheKey {
            organization: OrganizationId::from_uuid7(aex_wire::Uuid7::compose(1, [2; 10])),
            workspace: WorkspaceId::from_uuid7(aex_wire::Uuid7::compose(1, [1; 10])),
            binding: ProviderCredentialId::from_uuid7(aex_wire::Uuid7::compose(
                2,
                [binding_seed; 10],
            )),
            revision: CredentialRevision(1),
            generation: SourceGeneration(1),
        }
    }

    fn lead(registry: &FlightRegistry, key: CredentialCacheKey) -> super::FlightLease<'_> {
        match registry.admit(key) {
            FlightAdmission::Leading(lease) => lease,
            FlightAdmission::Following(_) | FlightAdmission::Unregistered => {
                panic!("this caller was supposed to lead the flight")
            }
        }
    }

    fn follow(registry: &FlightRegistry, key: CredentialCacheKey) -> super::FlightFollower {
        match registry.admit(key) {
            FlightAdmission::Following(follower) => follower,
            FlightAdmission::Leading(_) | FlightAdmission::Unregistered => {
                panic!("this caller was supposed to follow the flight")
            }
        }
    }

    #[test]
    fn a_second_caller_for_one_identity_follows_instead_of_leading() {
        let registry = FlightRegistry::new(4);
        let lease = lead(&registry, identity(1));
        let _follower = follow(&registry, identity(1));
        assert_eq!(registry.active(), 1);
        drop(lease);
        assert_eq!(registry.active(), 0);
    }

    #[test]
    fn a_registry_at_capacity_leaves_the_next_identity_unregistered() {
        let registry = FlightRegistry::new(1);
        let _held = lead(&registry, identity(1));
        assert!(matches!(
            registry.admit(identity(2)),
            FlightAdmission::Unregistered
        ));
    }

    #[test]
    fn a_revocation_marks_and_retires_only_the_flights_it_covers() {
        let registry = FlightRegistry::new(4);
        let covered = lead(&registry, identity(1));
        let untouched = lead(&registry, identity(2));
        registry.invalidate_matching(&|candidate| candidate.binding == identity(1).binding);
        assert_eq!(covered.validity(), FlightValidity::Invalidated);
        assert_eq!(untouched.validity(), FlightValidity::Intact);
        assert_eq!(registry.active(), 1);
    }

    #[test]
    fn a_completion_after_a_revocation_cannot_retire_the_flight_that_replaced_it() {
        let registry = FlightRegistry::new(4);
        let revoked = lead(&registry, identity(1));
        registry.invalidate_matching(&|candidate| candidate.binding == identity(1).binding);
        let successor = lead(&registry, identity(1));
        assert_eq!(registry.active(), 1);
        drop(revoked);
        assert_eq!(
            registry.active(),
            1,
            "an abandoned flight must not remove the one that replaced it"
        );
        drop(successor);
        assert_eq!(registry.active(), 0);
    }

    #[tokio::test]
    async fn every_follower_receives_the_leaders_one_allocation() {
        let registry = FlightRegistry::new(4);
        let lease = lead(&registry, identity(1));
        let followers: Vec<_> = (0..8).map(|_| follow(&registry, identity(1))).collect();
        let secret = Arc::new(Zeroizing::new("sk-shared-allocation".to_owned()));
        lease.settle(Ok(Arc::clone(&secret)));
        for follower in followers {
            match follower.joined().await {
                FlightJoin::Settled(Ok(shared)) => assert!(Arc::ptr_eq(&shared, &secret)),
                FlightJoin::Settled(Err(_)) | FlightJoin::Abandoned => {
                    panic!("a settled flight must hand every follower the leader's allocation")
                }
            }
        }
    }

    #[tokio::test]
    async fn a_follower_of_an_abandoned_leader_is_told_to_re_elect() {
        let registry = FlightRegistry::new(4);
        let lease = lead(&registry, identity(1));
        let follower = follow(&registry, identity(1));
        drop(lease);
        assert!(matches!(follower.joined().await, FlightJoin::Abandoned));
        assert_eq!(registry.active(), 0);
    }
}
