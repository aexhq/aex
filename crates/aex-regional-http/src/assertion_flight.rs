//! Bounded, operation-lifetime single flight for central assertion resolution.
//!
//! # Why a flight and not a lock
//!
//! What this replaced was an `Arc<Mutex<()>>` per credential digest, inserted on
//! first sight and removed by nothing. That digest is derived from a *presented*
//! token, so its cardinality is chosen by whoever is sending tokens — no valid
//! secret is needed, only a syntactically valid one — and a long-lived regional
//! process retained one map entry and one mutex allocation for every distinct
//! token it had ever been shown.
//!
//! A flight exists for one *resolution* rather than for one credential. Exactly
//! one caller leads it, every other caller waits on the same outcome, and the
//! leader retires it whether it published an answer or was cancelled mid-await.
//! What the registry holds is therefore bounded by how many resolutions are
//! running at once, which is a number this module declares a ceiling for.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::Notify;

use crate::assertion::{AuthFailure, CredentialKey, VerifiedAuthorization};

/// How many distinct credentials a process resolves centrally at the same time.
///
/// The constraint this converts is our own exposure to an input we do not
/// control: the flight key is the digest of a presented token, so any structure
/// keyed by it grows with the tokens an unauthenticated caller chooses to send.
/// A flight lives for one central exchange rather than for a credential's
/// lifetime, so the number only has to cover the widest simultaneous
/// *first-use* burst a regional process sees — a cold start under full
/// connection load — and it refuses beyond that instead of growing.
pub const MAX_ACTIVE_FLIGHTS: NonZeroUsize =
    NonZeroUsize::new(4_096).expect("the declared ceiling is positive");

/// What one flight publishes: the verified authorization, or the refusal every
/// waiter on that flight shares.
pub type FlightOutcome = Result<VerifiedAuthorization, AuthFailure>;

/// One central resolution in progress, and the slot its outcome is published to.
pub struct Flight {
    outcome: Mutex<Option<FlightOutcome>>,
    published: Notify,
}

impl Flight {
    fn new() -> Self {
        Self {
            outcome: Mutex::new(None),
            published: Notify::new(),
        }
    }

    /// The outcome, once its leader has published one.
    #[must_use]
    pub fn outcome(&self) -> Option<FlightOutcome> {
        *self.slot()
    }

    /// Waits for this flight's outcome.
    ///
    /// A waiter that is itself cancelled leaves the flight untouched: it holds
    /// no registry entry and leads nothing, so its disappearance is invisible to
    /// the leader and to every other waiter.
    ///
    /// # Errors
    ///
    /// Returns whatever the leader published — every waiter shares the failure
    /// exactly as it shares the success — or [`AuthFailure::FlightCancelled`]
    /// when the leader was dropped before it produced an answer.
    pub async fn wait(&self) -> FlightOutcome {
        loop {
            let published = self.published.notified();
            tokio::pin!(published);
            // Registering interest *before* reading the slot is what closes the
            // window in which a publication lands between a read that missed it
            // and a wait that would then have nobody left to wake it.
            published.as_mut().enable();
            if let Some(outcome) = self.outcome() {
                return outcome;
            }
            published.await;
        }
    }

    /// Publishes the first outcome and wakes every waiter.
    ///
    /// A second publication is ignored rather than overwriting: the first answer
    /// is the one waiters may already have returned to their callers, and two
    /// answers from one flight is the state this type exists to make impossible.
    fn publish(&self, outcome: &FlightOutcome) {
        let mut slot = self.slot();
        if slot.is_none() {
            *slot = Some(*outcome);
        }
        drop(slot);
        self.published.notify_waiters();
    }

    /// The outcome slot.
    ///
    /// Nothing that can panic runs while this lock is held — the guarded value
    /// is a `Copy` option that is read or assigned and nothing else — so a
    /// poisoned lock here means a panic that was impossible actually happened,
    /// and continuing on a value of unknown provenance is the worse answer on an
    /// authentication path.
    fn slot(&self) -> MutexGuard<'_, Option<FlightOutcome>> {
        self.outcome
            .lock()
            .expect("a flight outcome lock is never held across code that can panic")
    }
}

/// What a caller became when it entered the registry for a credential.
pub enum FlightRole<'registry> {
    /// This caller performs the resolution and publishes its outcome.
    Leader(FlightLeader<'registry>),
    /// Another caller is already resolving this credential; wait on its answer.
    Follower(Arc<Flight>),
}

/// Every central resolution this process currently has outstanding.
pub struct FlightRegistry {
    active: Mutex<HashMap<CredentialKey, Arc<Flight>>>,
    max_active: NonZeroUsize,
}

impl FlightRegistry {
    /// Builds an empty registry that admits `max_active` concurrent flights.
    #[must_use]
    pub fn new(max_active: NonZeroUsize) -> Self {
        Self {
            active: Mutex::new(HashMap::new()),
            max_active,
        }
    }

    /// Becomes the leader of a new flight for `key`, or a follower of the flight
    /// already running for it.
    ///
    /// The decision is taken under one lock, so two callers that miss the cache
    /// at the same instant cannot both become leaders and cannot both call the
    /// central authority.
    ///
    /// # Errors
    ///
    /// Returns [`AuthFailure::FlightCapacity`] when a **new** flight would
    /// exceed the ceiling. Joining an existing flight is never refused, because
    /// it costs no entry. The ceiling refuses rather than evicting: every active
    /// flight has callers waiting on it, and dropping one to make room strands
    /// them without freeing the resolution they were waiting for.
    pub fn board(&self, key: CredentialKey) -> Result<FlightRole<'_>, AuthFailure> {
        let mut active = self.active();
        let occupancy = active.len();
        match active.entry(key) {
            Entry::Occupied(running) => Ok(FlightRole::Follower(Arc::clone(running.get()))),
            Entry::Vacant(vacant) => {
                if occupancy >= self.max_active.get() {
                    return Err(AuthFailure::FlightCapacity);
                }
                let flight = Arc::new(Flight::new());
                vacant.insert(Arc::clone(&flight));
                Ok(FlightRole::Leader(FlightLeader {
                    registry: self,
                    key,
                    flight,
                }))
            }
        }
    }

    /// How many resolutions are outstanding right now.
    #[must_use]
    pub fn outstanding(&self) -> usize {
        self.active().len()
    }

    /// The ceiling this registry refuses at.
    #[must_use]
    pub const fn ceiling(&self) -> NonZeroUsize {
        self.max_active
    }

    /// Removes `flight` if it is still the flight registered for `key`.
    ///
    /// Identity is the pointer, not the key: a leader that finishes after its
    /// entry has already been replaced must not retire the newer flight, whose
    /// own callers are still waiting on it. The leader holds a strong reference
    /// for the whole comparison, so the address it compares cannot have been
    /// reused by the flight it is comparing against.
    fn retire(&self, key: CredentialKey, flight: &Arc<Flight>) {
        let mut active = self.active();
        if active
            .get(&key)
            .is_some_and(|current| Arc::ptr_eq(current, flight))
        {
            active.remove(&key);
        }
    }

    /// The active-flight map.
    ///
    /// Held only across hash lookups and insertions, never across an await and
    /// never across anything that can panic; see [`Flight::slot`] for why a
    /// poisoned lock is not recovered from on this path.
    fn active(&self) -> MutexGuard<'_, HashMap<CredentialKey, Arc<Flight>>> {
        self.active
            .lock()
            .expect("the active-flight lock is never held across code that can panic")
    }
}

/// The one caller responsible for resolving a credential and for retiring its
/// flight afterwards.
///
/// Retirement lives in `Drop` rather than in [`FlightLeader::publish`] because
/// the case that strands waiters is the one where `publish` is never reached: a
/// cancelled leader is dropped mid-await, and its followers would otherwise wait
/// on an outcome that nobody is producing any more.
pub struct FlightLeader<'registry> {
    registry: &'registry FlightRegistry,
    key: CredentialKey,
    flight: Arc<Flight>,
}

impl FlightLeader<'_> {
    /// Publishes this resolution's outcome to every waiter and retires the
    /// flight, so the next caller for this credential starts a fresh one.
    pub fn publish(self, outcome: &FlightOutcome) {
        self.flight.publish(outcome);
        // The retirement is `Drop`'s, which runs as this leader ends here.
    }
}

impl Drop for FlightLeader<'_> {
    fn drop(&mut self) {
        // A leader dropped before it published was cancelled: its request went
        // away mid-resolution. Waiters are told to retry rather than left on a
        // resolution that no longer has anyone performing it, and the entry is
        // retired so a later caller leads a new flight instead of joining a dead
        // one. Publishing is a no-op when `publish` already ran.
        self.flight.publish(&Err(AuthFailure::FlightCancelled));
        self.registry.retire(self.key, &self.flight);
    }
}
