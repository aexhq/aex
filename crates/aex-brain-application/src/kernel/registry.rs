//! `ActivationRegistry` — keyed single-flight over agent activations (Loom target L1).
//!
//! This removes duplicate *local* work. It is deliberately **not** the correctness
//! mechanism: two mux tasks in two processes can both believe they hold an agent, and only
//! the durable fence decides which one may write. Confusing this map for that guarantee is
//! the mistake it is documented against.

use super::sync::{Arc, Mutex};
use aex_brain_domain::ids::AgentKey;
use std::collections::HashMap;

/// A keyed single-flight map over activations.
#[derive(Debug, Default)]
pub struct ActivationRegistry {
    inner: Mutex<HashMap<AgentKey, u64>>,
}

/// The winner's proof that it holds the local slot for one agent.
///
/// Releases on drop, including on an early return or a panic, so a slot cannot leak and
/// wedge an agent out of this process for the rest of its life.
#[derive(Debug)]
pub struct RegistrySlot {
    registry: Arc<ActivationRegistry>,
    key: AgentKey,
    generation: u64,
}

/// Another activation for the same agent is already running in this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("an activation for this agent is already running locally")]
pub struct Busy;

impl ActivationRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Takes the local slot for `key`, or reports [`Busy`].
    ///
    /// # Errors
    ///
    /// Returns [`Busy`] when another activation for the same agent holds the slot. The
    /// loser must not wait: the durable wake survives, so returning promptly and letting
    /// the queue redeliver beats holding a thread on a lock.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock was poisoned by a panic while held, which would mean the
    /// map's invariants are already unknown.
    pub fn try_activate(self: &Arc<Self>, key: AgentKey) -> Result<RegistrySlot, Busy> {
        let mut map = self
            .inner
            .lock()
            .expect("the registry lock is not poisoned");
        if map.contains_key(&key) {
            return Err(Busy);
        }
        let generation = map.len() as u64;
        map.insert(key, generation);
        Ok(RegistrySlot {
            registry: Arc::clone(self),
            key,
            generation,
        })
    }

    /// How many activations hold a slot.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock was poisoned.
    #[must_use]
    pub fn active(&self) -> usize {
        self.inner
            .lock()
            .expect("the registry lock is not poisoned")
            .len()
    }

    /// Whether `key` holds a slot.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock was poisoned.
    #[must_use]
    pub fn holds(&self, key: &AgentKey) -> bool {
        self.inner
            .lock()
            .expect("the registry lock is not poisoned")
            .contains_key(key)
    }

    fn release(&self, key: &AgentKey, generation: u64) {
        let mut map = self
            .inner
            .lock()
            .expect("the registry lock is not poisoned");
        // Only the holder of this generation may release: a slot re-taken between drop
        // scheduling and execution belongs to somebody else.
        if map.get(key) == Some(&generation) {
            map.remove(key);
        }
    }
}

impl RegistrySlot {
    /// Which agent the slot covers.
    #[must_use]
    pub const fn key(&self) -> AgentKey {
        self.key
    }
}

impl Drop for RegistrySlot {
    fn drop(&mut self) {
        self.registry.release(&self.key, self.generation);
    }
}
