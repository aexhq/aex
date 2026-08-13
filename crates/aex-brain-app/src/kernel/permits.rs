//! `PermitSet` — weighted permits with RAII release (Loom target L4).
//!
//! A failed conditional claim must not leak a permit. That is the whole reason release is
//! a `Drop` impl rather than a call: every early return, every `?`, every panic path
//! releases, and there is no code path a reviewer has to check.

use super::sync::{Arc, Mutex};
use std::collections::BTreeMap;

/// Which bounded resource a reservation draws on.
///
/// Byte-sized kinds are weighted: one reservation may take many units. Count-sized kinds
/// take one. Keeping them in one map means a caller acquires everything an activation
/// needs in a single fallible step, so a partial acquisition can never be observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PermitKind {
    /// Concurrently active activations.
    Activation,
    /// Concurrently open provider streams.
    ProviderStream,
    /// Concurrent Hands RPCs for one session generation.
    HandsRpc,
    /// Concurrent outbound tool network calls — managed web and MCP.
    ///
    /// Its own pool rather than a share of [`PermitKind::ProviderStream`]: a tool
    /// network call weighs four units against a stream's one, so a handful of
    /// concurrent fetches drawn from the stream pool would defer every model
    /// dispatch in the task. That is unreachable while the driver runs one tool
    /// call at a time, and reachable the moment it does not.
    NetworkLane,
    /// Bounded compute-lane jobs.
    ComputeLane,
    /// Bytes reserved for hydrated context.
    ContextBytes,
    /// Bytes reserved for the warm fold cache.
    ///
    /// A separate budget on purpose: the cache may never borrow memory that accepted work
    /// has already been promised, because evicting accepted work to keep a cache entry
    /// turns an optimization into a failure.
    WarmCacheBytes,
}

/// Every bounded resource in the process.
#[derive(Debug)]
pub struct PermitSet {
    limits: BTreeMap<PermitKind, u64>,
    held: Mutex<BTreeMap<PermitKind, u64>>,
}

/// A held reservation. Releases on drop.
#[derive(Debug)]
#[must_use = "dropping a reservation immediately releases it; bind it for the work's lifetime"]
pub struct Reservation {
    permits: Arc<PermitSet>,
    kind: PermitKind,
    units: u64,
}

/// The requested units are not available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{kind:?} has {available} of {limit} available, {requested} requested")]
pub struct PermitSetFull {
    /// Which resource.
    pub kind: PermitKind,
    /// How much was asked for.
    pub requested: u64,
    /// How much is free.
    pub available: u64,
    /// The configured ceiling.
    pub limit: u64,
}

impl PermitSet {
    /// A set with the given ceilings. An absent kind has a ceiling of zero, so acquiring it
    /// always fails — an unconfigured resource is unavailable, never unbounded.
    #[must_use]
    pub fn new(limits: BTreeMap<PermitKind, u64>) -> Self {
        Self {
            limits,
            held: Mutex::new(BTreeMap::new()),
        }
    }

    /// Acquires `units` of `kind`.
    ///
    /// # Errors
    ///
    /// Returns [`PermitSetFull`] when the ceiling would be exceeded. Never blocks: the
    /// caller decides whether to queue, defer or shed, and all three keep the durable wake.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock was poisoned.
    pub fn acquire(
        self: &Arc<Self>,
        kind: PermitKind,
        units: u64,
    ) -> Result<Reservation, PermitSetFull> {
        let limit = self.limits.get(&kind).copied().unwrap_or(0);
        let mut held = self.held.lock().expect("the permit lock is not poisoned");
        let current = held.get(&kind).copied().unwrap_or(0);
        let available = limit.saturating_sub(current);
        if units > available {
            return Err(PermitSetFull {
                kind,
                requested: units,
                available,
                limit,
            });
        }
        held.insert(kind, current.saturating_add(units));
        Ok(Reservation {
            permits: Arc::clone(self),
            kind,
            units,
        })
    }

    /// Atomically acquires every requested resource.
    ///
    /// Duplicate kinds are combined before capacity is checked. Either all reservations are
    /// returned or no counter changes; another activation can therefore never take a
    /// provider slot between this activation's memory and stream-buffer reservations.
    ///
    /// # Errors
    ///
    /// Returns the first exhausted resource in request order.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock was poisoned.
    pub fn acquire_many(
        self: &Arc<Self>,
        requests: &[(PermitKind, u64)],
    ) -> Result<Vec<Reservation>, PermitSetFull> {
        let mut held = self.held.lock().expect("the permit lock is not poisoned");
        for (index, _) in requests.iter().enumerate() {
            let Some((kind, units)) = combined_request_at(requests, index) else {
                continue;
            };
            let limit = self.limits.get(&kind).copied().unwrap_or(0);
            let current = held.get(&kind).copied().unwrap_or(0);
            let available = limit.saturating_sub(current);
            if units > available {
                return Err(PermitSetFull {
                    kind,
                    requested: units,
                    available,
                    limit,
                });
            }
        }
        let mut reservations = Vec::with_capacity(requests.len());
        for (index, _) in requests.iter().enumerate() {
            let Some((kind, units)) = combined_request_at(requests, index) else {
                continue;
            };
            let current = held.get(&kind).copied().unwrap_or(0);
            held.insert(kind, current.saturating_add(units));
            reservations.push(Reservation {
                permits: Arc::clone(self),
                kind,
                units,
            });
        }
        Ok(reservations)
    }

    /// Whether every request could be acquired together at this instant.
    ///
    /// This is an admission hint, not a reservation. The caller must still use
    /// [`Self::acquire_many`], whose atomic check owns the race with another activation.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock was poisoned.
    #[must_use]
    pub fn can_acquire_many(&self, requests: &[(PermitKind, u64)]) -> bool {
        let held = self.held.lock().expect("the permit lock is not poisoned");
        requests.iter().enumerate().all(|(index, _)| {
            let Some((kind, units)) = combined_request_at(requests, index) else {
                return true;
            };
            let limit = self.limits.get(&kind).copied().unwrap_or(0);
            let current = held.get(&kind).copied().unwrap_or(0);
            units <= limit.saturating_sub(current)
        })
    }

    /// How many units of `kind` are held.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock was poisoned.
    #[must_use]
    pub fn held(&self, kind: PermitKind) -> u64 {
        self.held
            .lock()
            .expect("the permit lock is not poisoned")
            .get(&kind)
            .copied()
            .unwrap_or(0)
    }

    /// The configured ceiling for `kind`.
    #[must_use]
    pub fn limit(&self, kind: PermitKind) -> u64 {
        self.limits.get(&kind).copied().unwrap_or(0)
    }

    fn release(&self, kind: PermitKind, units: u64) {
        let mut held = self.held.lock().expect("the permit lock is not poisoned");
        let current = held.get(&kind).copied().unwrap_or(0);
        // Saturating rather than wrapping: a double release is a defect, and going
        // negative would silently hand out permits that do not exist.
        held.insert(kind, current.saturating_sub(units));
    }
}

fn combined_request_at(requests: &[(PermitKind, u64)], index: usize) -> Option<(PermitKind, u64)> {
    let (kind, units) = requests[index];
    if units == 0 || requests[..index].iter().any(|(prior, _)| *prior == kind) {
        return None;
    }
    let total = requests[index + 1..]
        .iter()
        .filter(|(candidate, _)| *candidate == kind)
        .fold(units, |sum, (_, extra)| sum.saturating_add(*extra));
    Some((kind, total))
}

impl Reservation {
    /// Which resource this reservation draws on.
    #[must_use]
    pub const fn kind(&self) -> PermitKind {
        self.kind
    }

    /// How many units it holds.
    #[must_use]
    pub const fn units(&self) -> u64 {
        self.units
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.permits.release(self.kind, self.units);
    }
}
