//! Isolation-keyed client pooling (plan 08 §3.5).
//!
//! One `reqwest::Client` per isolation key, never one per run. The key includes
//! the workspace and exact credential binding revision, so two workspaces or
//! two provider accounts can never share a warm TLS session and a revoked
//! credential cannot keep one alive.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aex_model_catalog::CatalogRevision;
use aex_model_catalog::document::EndpointPin;
use aex_wire::ids::{ProviderCredentialId, WorkspaceId};

use crate::budget::StreamBudget;
use crate::credential::CredentialRevision;
use crate::wire_pending::SourceGeneration;

/// How long an idle client may stay warm.
pub const IDLE_EVICTION: Duration = Duration::from_secs(90);
/// How many distinct isolation keys the pool holds.
pub const CAPACITY: usize = 512;
/// Idle connections kept per host inside one client.
pub const POOL_MAX_IDLE_PER_HOST: usize = 8;
/// In-flight requests permitted per isolation key.
pub const INFLIGHT_PER_KEY: usize = 64;

/// What makes two dispatches able to share a connection.
///
/// Every member is load-bearing: dropping the workspace would let two tenants
/// share a socket; dropping the binding or revision would couple independent
/// provider accounts whose generation counters happen to match; dropping the
/// generation would let a revoked key keep a warm session; and dropping the
/// catalog revision would let a re-activated catalog serve over connections
/// established under the old one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IsolationKey {
    /// The compiled origin.
    pub origin: EndpointPin,
    /// The owning workspace.
    pub workspace: WorkspaceId,
    /// The exact provider-credential binding.
    pub credential_binding: ProviderCredentialId,
    /// The immutable binding revision.
    pub credential_revision: CredentialRevision,
    /// The credential generation in force.
    pub credential_generation: SourceGeneration,
    /// The catalog revision in force.
    pub catalog: CatalogRevision,
}

/// A warm client plus its concurrency permit.
#[derive(Debug)]
pub struct PooledClient {
    /// The HTTP client.
    pub http: reqwest::Client,
    /// Per-key in-flight permits.
    pub inflight: Arc<tokio::sync::Semaphore>,
    /// When the client was built.
    pub created_at: Instant,
}

/// Why a client could not be acquired. Every arm is `DispatchProof::NotSent`:
/// acquisition happens strictly before the send gate is consumed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PoolError {
    /// The client could not be built.
    #[error("the HTTP client could not be built: {reason}")]
    ClientBuild {
        /// What went wrong, in this crate's own words.
        reason: &'static str,
    },
    /// The per-key or per-provider permit could not be acquired.
    #[error("no dispatch permit was available")]
    PermitUnavailable,
    /// The pool is draining and refuses new work.
    #[error("the pool is draining")]
    Draining,
}

/// The client pool.
#[derive(Debug)]
pub struct ClientPool {
    entries: Mutex<HashMap<IsolationKey, Entry>>,
    capacity: usize,
    idle_eviction: Duration,
    draining: std::sync::atomic::AtomicBool,
}

#[derive(Debug)]
struct Entry {
    client: Arc<PooledClient>,
    last_used: Instant,
}

impl Default for ClientPool {
    fn default() -> Self {
        Self::new(CAPACITY, IDLE_EVICTION)
    }
}

impl ClientPool {
    /// Builds a pool with an explicit capacity and idle window.
    #[must_use]
    pub fn new(capacity: usize, idle_eviction: Duration) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            capacity,
            idle_eviction,
            draining: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Returns the client for a key, building one if none is warm.
    ///
    /// # Errors
    ///
    /// Returns [`PoolError`] when the pool is draining or the client cannot be
    /// built.
    pub fn acquire(
        &self,
        key: &IsolationKey,
        budget: &StreamBudget,
    ) -> Result<Arc<PooledClient>, PoolError> {
        if self.draining.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(PoolError::Draining);
        }
        let mut entries = self.entries.lock().map_err(|_| PoolError::ClientBuild {
            reason: "the pool lock was poisoned",
        })?;
        entries.retain(|_, entry| entry.last_used.elapsed() < self.idle_eviction);

        if let Some(entry) = entries.get_mut(key) {
            entry.last_used = Instant::now();
            return Ok(Arc::clone(&entry.client));
        }
        if entries.len() >= self.capacity
            && let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| *key)
        {
            entries.remove(&oldest);
        }
        let client = Arc::new(PooledClient {
            http: build_client(budget)?,
            inflight: Arc::new(tokio::sync::Semaphore::new(INFLIGHT_PER_KEY)),
            created_at: Instant::now(),
        });
        entries.insert(
            *key,
            Entry {
                client: Arc::clone(&client),
                last_used: Instant::now(),
            },
        );
        Ok(client)
    }

    /// Drops every client whose key matches. Returns how many were dropped.
    ///
    /// Called on revocation and on catalog activation, so a stale credential
    /// generation cannot keep a warm TLS session.
    pub fn close(&self, predicate: &dyn Fn(&IsolationKey) -> bool) -> usize {
        let Ok(mut entries) = self.entries.lock() else {
            return 0;
        };
        let before = entries.len();
        entries.retain(|key, _| !predicate(key));
        before - entries.len()
    }

    /// Refuses further acquisition.
    pub fn drain(&self) {
        self.draining
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// How many clients are warm.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.lock().map_or(0, |entries| entries.len())
    }

    /// Whether no client is warm.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Builds one client.
///
/// The policy choices are all refusals: no proxy, no redirect following, no
/// cookie store. A redirect from a provider origin is an incident or an attack,
/// never a hop to chase (D-22).
fn build_client(budget: &StreamBudget) -> Result<reqwest::Client, PoolError> {
    reqwest::Client::builder()
        .connect_timeout(budget.connect_timeout)
        .pool_idle_timeout(IDLE_EVICTION)
        .pool_max_idle_per_host(POOL_MAX_IDLE_PER_HOST)
        .tcp_keepalive(Duration::from_secs(30))
        .tcp_nodelay(true)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .use_rustls_tls()
        .https_only(true)
        .user_agent(concat!(
            "aex-brain-provider-gateway/",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .map_err(|_| PoolError::ClientBuild {
            reason: "reqwest refused the pinned client configuration",
        })
}

#[cfg(test)]
mod tests {
    use aex_model_catalog::CatalogRevision;
    use aex_model_catalog::document::EndpointPin;
    use aex_model_catalog::primitives::Blake3Digest;
    use aex_wire::ids::{PrefixedId, ProviderCredentialId, WorkspaceId};

    use super::{ClientPool, IsolationKey, PoolError};
    use crate::budget::StreamBudget;
    use crate::credential::CredentialRevision;
    use crate::wire_pending::SourceGeneration;

    fn workspace(seed: u8) -> WorkspaceId {
        WorkspaceId::from_uuid7(aex_wire::Uuid7::compose(1, [seed; 10]))
    }

    fn credential(seed: u8) -> ProviderCredentialId {
        ProviderCredentialId::from_uuid7(aex_wire::Uuid7::compose(2, [seed; 10]))
    }

    fn key(
        workspace_seed: u8,
        binding_seed: u8,
        revision: u64,
        generation: u64,
        catalog: &str,
    ) -> IsolationKey {
        IsolationKey {
            origin: EndpointPin::OpenAiApi,
            workspace: workspace(workspace_seed),
            credential_binding: credential(binding_seed),
            credential_revision: CredentialRevision(revision),
            credential_generation: SourceGeneration(generation),
            catalog: CatalogRevision(Blake3Digest::of(catalog.as_bytes())),
        }
    }

    #[test]
    fn the_same_key_reuses_one_client() {
        let pool = ClientPool::default();
        let budget = StreamBudget::default();
        let first = pool.acquire(&key(1, 1, 1, 1, "a"), &budget).expect("first");
        let second = pool
            .acquire(&key(1, 1, 1, 1, "a"), &budget)
            .expect("second");
        assert!(std::sync::Arc::ptr_eq(&first, &second));
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn each_key_component_creates_a_separate_client() {
        let pool = ClientPool::default();
        let budget = StreamBudget::default();
        let base = pool.acquire(&key(1, 1, 1, 1, "a"), &budget).expect("base");
        for other in [
            key(2, 1, 1, 1, "a"),
            key(1, 2, 1, 1, "a"),
            key(1, 1, 2, 1, "a"),
            key(1, 1, 1, 2, "a"),
            key(1, 1, 1, 1, "b"),
        ] {
            let client = pool.acquire(&other, &budget).expect("other");
            assert!(
                !std::sync::Arc::ptr_eq(&base, &client),
                "{other:?} shared a client with the base key"
            );
        }
        assert_eq!(pool.len(), 6);
    }

    #[test]
    fn two_workspaces_never_share_a_client() {
        let pool = ClientPool::default();
        let budget = StreamBudget::default();
        let left = pool.acquire(&key(1, 1, 1, 1, "a"), &budget).expect("left");
        let right = pool.acquire(&key(9, 1, 1, 1, "a"), &budget).expect("right");
        assert!(!std::sync::Arc::ptr_eq(&left, &right));
    }

    #[test]
    fn close_drops_exactly_the_matching_clients() {
        let pool = ClientPool::default();
        let budget = StreamBudget::default();
        pool.acquire(&key(1, 1, 1, 1, "a"), &budget).expect("one");
        pool.acquire(&key(2, 1, 1, 1, "a"), &budget).expect("two");
        pool.acquire(&key(1, 1, 1, 2, "a"), &budget).expect("three");
        let target = workspace(1);
        let closed = pool.close(&|key| key.workspace == target);
        assert_eq!(closed, 2);
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn a_revoked_generation_cannot_keep_a_warm_session() {
        let pool = ClientPool::default();
        let budget = StreamBudget::default();
        let stale = pool.acquire(&key(1, 1, 1, 1, "a"), &budget).expect("stale");
        pool.close(&|key| key.credential_generation == SourceGeneration(1));
        let fresh = pool.acquire(&key(1, 1, 1, 2, "a"), &budget).expect("fresh");
        assert!(!std::sync::Arc::ptr_eq(&stale, &fresh));
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn the_pool_stays_inside_its_capacity() {
        let pool = ClientPool::new(2, core::time::Duration::from_mins(1));
        let budget = StreamBudget::default();
        for seed in 0..8u8 {
            pool.acquire(&key(seed, 1, 1, 1, "a"), &budget)
                .expect("acquire");
        }
        assert!(pool.len() <= 2, "pool grew to {}", pool.len());
    }

    #[test]
    fn a_draining_pool_refuses_new_work() {
        let pool = ClientPool::default();
        let budget = StreamBudget::default();
        pool.acquire(&key(1, 1, 1, 1, "a"), &budget)
            .expect("before");
        pool.drain();
        assert_eq!(
            pool.acquire(&key(1, 1, 1, 1, "a"), &budget)
                .expect_err("draining"),
            PoolError::Draining
        );
    }

    #[test]
    fn an_idle_client_is_evicted() {
        let pool = ClientPool::new(8, core::time::Duration::from_millis(1));
        let budget = StreamBudget::default();
        let first = pool.acquire(&key(1, 1, 1, 1, "a"), &budget).expect("first");
        std::thread::sleep(core::time::Duration::from_millis(5));
        let second = pool
            .acquire(&key(1, 1, 1, 1, "a"), &budget)
            .expect("second");
        assert!(!std::sync::Arc::ptr_eq(&first, &second));
    }
}
