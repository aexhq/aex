//! Tenant-isolated, DNS-screened connection reuse for warm Brain tasks.
//!
//! A pool entry is an accelerator, never MCP or Brain authority. Every request
//! resolves and screens the complete address set before selecting an entry. A
//! DNS change therefore selects a different client, while a stable target can
//! reuse TLS and HTTP/2 connections inside one warm mux process.

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::num::{NonZeroU64, NonZeroUsize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aex_brain_managed_web::egress::{
    DnsResolver, EgressPolicy, EgressRejection, ValidatedTarget, resolve_and_screen, validate,
};
use aex_wire::ids::{ContentHash, OrganizationId, ResourceName, WorkspaceId};

use crate::client::PROTOCOL_REVISION;

/// Tenant identity that must match before one transport can be reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TenantScope {
    /// Organization whose encryption context admits the request.
    pub organization: OrganizationId,
    /// Workspace whose registration and secrets are pinned.
    pub workspace: WorkspaceId,
}

/// Immutable registration facts that make one server connection reusable.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ServerRevision {
    /// Registered server identity.
    pub server: ResourceName,
    /// Monotonic registered-resource revision.
    pub revision: NonZeroU64,
    /// Exact generation of every request-header secret.
    pub secret_generation: NonZeroU64,
    /// Digest of the frozen qualified manifest.
    pub manifest_digest: ContentHash,
}

/// Exact isolation identity for one reusable HTTP client.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PoolKey {
    scope: TenantScope,
    server: ServerRevision,
    protocol_revision: &'static str,
    origin: Box<str>,
    host: Box<str>,
    addresses: Vec<IpAddr>,
}

impl PoolKey {
    fn new(scope: TenantScope, server: ServerRevision, target: &ValidatedTarget) -> Self {
        let mut addresses = target.addrs.clone();
        addresses.sort_unstable();
        addresses.dedup();
        let origin = target.url.origin().ascii_serialization().into_boxed_str();
        Self {
            scope,
            server,
            protocol_revision: PROTOCOL_REVISION,
            origin,
            host: target.host.clone(),
            addresses,
        }
    }

    /// Tenant scope carried by this entry.
    #[must_use]
    pub const fn scope(&self) -> TenantScope {
        self.scope
    }

    /// Frozen server revision carried by this entry.
    #[must_use]
    pub const fn server(&self) -> &ServerRevision {
        &self.server
    }

    /// Exact screened addresses this client is allowed to connect to.
    #[must_use]
    pub fn addresses(&self) -> &[IpAddr] {
        &self.addresses
    }
}

struct PoolEntry {
    client: Arc<reqwest::Client>,
    last_used: u64,
}

/// One acquired, address-pinned client.
#[derive(Clone)]
pub struct PooledClient {
    key: PoolKey,
    target: url::Url,
    client: Arc<reqwest::Client>,
}

impl core::fmt::Debug for PooledClient {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("PooledClient")
            .field("key", &self.key)
            .field("target", &self.target)
            .finish_non_exhaustive()
    }
}

impl PooledClient {
    /// Exact reuse identity.
    #[must_use]
    pub const fn key(&self) -> &PoolKey {
        &self.key
    }

    /// Whether two acquisitions share one reqwest connection pool.
    #[must_use]
    pub fn shares_connections_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.client, &other.client)
    }

    /// Starts one POST against the exact screened target.
    ///
    /// Callers add request-scoped secret headers; the pooled client never owns
    /// plaintext credentials or ambient default headers.
    pub fn post(&self) -> reqwest::RequestBuilder {
        self.client.post(self.target.clone())
    }
}

/// Bounded process-local connection pool.
pub struct ConnectionPool {
    max_entries: NonZeroUsize,
    sequence: AtomicU64,
    entries: Mutex<BTreeMap<PoolKey, PoolEntry>>,
}

impl core::fmt::Debug for ConnectionPool {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ConnectionPool")
            .field("max_entries", &self.max_entries)
            .field("sequence", &self.sequence.load(Ordering::Relaxed))
            .field("entries", &self.len())
            .finish()
    }
}

impl ConnectionPool {
    /// Creates a bounded pool.
    #[must_use]
    pub fn new(max_entries: NonZeroUsize) -> Self {
        Self {
            max_entries,
            sequence: AtomicU64::new(0),
            entries: Mutex::new(BTreeMap::new()),
        }
    }

    /// Number of currently retained accelerators.
    ///
    /// # Panics
    ///
    /// Panics if another thread poisoned the pool mutex.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.lock().expect("MCP pool mutex poisoned").len()
    }

    /// Whether the pool currently holds no accelerators.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn acquire(
        &self,
        scope: TenantScope,
        server: ServerRevision,
        target: ValidatedTarget,
    ) -> Result<PooledClient, PoolError> {
        let key = PoolKey::new(scope, server, &target);
        let stamp = self.sequence.fetch_add(1, Ordering::Relaxed);
        {
            let mut entries = self.entries.lock().expect("MCP pool mutex poisoned");
            if let Some(entry) = entries.get_mut(&key) {
                entry.last_used = stamp;
                return Ok(PooledClient {
                    key,
                    target: target.url,
                    client: Arc::clone(&entry.client),
                });
            }
        }

        let client = Arc::new(build_client(&target)?);
        let mut entries = self.entries.lock().expect("MCP pool mutex poisoned");
        if let Some(entry) = entries.get_mut(&key) {
            entry.last_used = stamp;
            return Ok(PooledClient {
                key,
                target: target.url,
                client: Arc::clone(&entry.client),
            });
        }
        if entries.len() == self.max_entries.get()
            && let Some(evicted) = entries
                .iter()
                .min_by_key(|(key, entry)| (entry.last_used, *key))
                .map(|(key, _)| key.clone())
        {
            entries.remove(&evicted);
        }
        entries.insert(
            key.clone(),
            PoolEntry {
                client: Arc::clone(&client),
                last_used: stamp,
            },
        );
        Ok(PooledClient {
            key,
            target: target.url,
            client,
        })
    }
}

/// Production entry point: screen DNS on every request, then reuse only the
/// exact matching client.
///
/// # Errors
///
/// Returns [`PoolError::Egress`] when the endpoint or complete DNS answer fails
/// the shared SSRF policy, or [`PoolError::ClientBuild`] when the address-pinned
/// HTTP client cannot be constructed.
pub async fn screened_client(
    pool: &ConnectionPool,
    resolver: &dyn DnsResolver,
    scope: TenantScope,
    server: ServerRevision,
    endpoint: &str,
) -> Result<PooledClient, PoolError> {
    let parsed = validate(&EgressPolicy::managed_web(), endpoint)?;
    let target = resolve_and_screen(parsed, resolver).await?;
    pool.acquire(scope, server, target)
}

fn build_client(target: &ValidatedTarget) -> Result<reqwest::Client, PoolError> {
    let pinned = target
        .addrs
        .iter()
        .map(|address| SocketAddr::new(*address, 443))
        .collect::<Vec<_>>();
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(30))
        .no_gzip()
        .no_brotli()
        .resolve_to_addrs(&target.host, &pinned)
        .build()
        .map_err(|_| PoolError::ClientBuild)
}

/// A screened pooled client could not be acquired.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PoolError {
    /// Shared SSRF policy rejected the URL or complete DNS answer.
    #[error(transparent)]
    Egress(#[from] EgressRejection),
    /// reqwest could not build the pinned client.
    #[error("MCP pinned HTTP client construction failed")]
    ClientBuild,
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};
    use std::num::{NonZeroU64, NonZeroUsize};
    use std::sync::{Arc, Mutex};

    use aex_brain_managed_web::egress::{DnsResolver, EgressRejection};
    use aex_wire::ids::{ContentHash, OrganizationId, PrefixedId as _, Uuid7, WorkspaceId};

    use super::{ConnectionPool, ServerRevision, TenantScope, screened_client};

    #[derive(Debug)]
    struct Resolver(Mutex<Vec<Vec<IpAddr>>>);

    #[async_trait::async_trait]
    impl DnsResolver for Resolver {
        async fn resolve(&self, _host: &str) -> Result<Vec<IpAddr>, EgressRejection> {
            Ok(self.0.lock().expect("not poisoned").remove(0))
        }
    }

    fn scope(organization: u8, workspace: u8) -> TenantScope {
        TenantScope {
            organization: OrganizationId::from_uuid7(Uuid7::compose(1, [organization; 10])),
            workspace: WorkspaceId::from_uuid7(Uuid7::compose(1, [workspace; 10])),
        }
    }

    fn revision(revision: u64, secret_generation: u64) -> ServerRevision {
        ServerRevision {
            server: aex_wire::ids::ResourceName::parse("docs").expect("resource name"),
            revision: NonZeroU64::new(revision).expect("positive revision"),
            secret_generation: NonZeroU64::new(secret_generation).expect("positive generation"),
            manifest_digest: ContentHash::of(b"manifest"),
        }
    }

    fn resolver(addresses: impl IntoIterator<Item = Vec<IpAddr>>) -> Arc<Resolver> {
        Arc::new(Resolver(Mutex::new(addresses.into_iter().collect())))
    }

    fn public(last: u8) -> Vec<IpAddr> {
        vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, last))]
    }

    #[tokio::test]
    async fn exact_identity_reuses_one_warm_connection_pool() {
        let pool = ConnectionPool::new(NonZeroUsize::new(8).expect("positive"));
        assert!(format!("{pool:?}").contains("sequence: 0"));
        let resolver = resolver([public(34), public(34)]);
        let first = screened_client(
            &pool,
            resolver.as_ref(),
            scope(1, 2),
            revision(3, 4),
            "https://example.com/mcp",
        )
        .await
        .expect("first client");
        let second = screened_client(
            &pool,
            resolver.as_ref(),
            scope(1, 2),
            revision(3, 4),
            "https://example.com/mcp",
        )
        .await
        .expect("second client");
        assert!(first.shares_connections_with(&second));
        assert_eq!(pool.len(), 1);
    }

    #[tokio::test]
    async fn tenant_secret_revision_and_dns_generations_never_cross() {
        let pool = ConnectionPool::new(NonZeroUsize::new(16).expect("positive"));
        let resolver = resolver([
            public(34),
            public(34),
            public(34),
            public(34),
            public(34),
            public(35),
        ]);
        let baseline = screened_client(
            &pool,
            resolver.as_ref(),
            scope(1, 2),
            revision(3, 4),
            "https://example.com/mcp",
        )
        .await
        .expect("baseline");
        let variants = [
            (scope(9, 2), revision(3, 4)),
            (scope(1, 8), revision(3, 4)),
            (scope(1, 2), revision(5, 4)),
            (scope(1, 2), revision(3, 6)),
        ];
        for (scope, revision) in variants {
            let variant = screened_client(
                &pool,
                resolver.as_ref(),
                scope,
                revision,
                "https://example.com/mcp",
            )
            .await
            .expect("variant");
            assert!(!baseline.shares_connections_with(&variant));
        }
        let rebound = screened_client(
            &pool,
            resolver.as_ref(),
            scope(1, 2),
            revision(3, 4),
            "https://example.com/mcp",
        )
        .await
        .expect("rebound client");
        assert!(!baseline.shares_connections_with(&rebound));
    }

    #[tokio::test]
    async fn mixed_public_private_dns_is_rejected_before_pooling() {
        let pool = ConnectionPool::new(NonZeroUsize::new(8).expect("positive"));
        let resolver = resolver([vec![
            IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)),
            IpAddr::V4(Ipv4Addr::LOCALHOST),
        ]]);
        let error = screened_client(
            &pool,
            resolver.as_ref(),
            scope(1, 2),
            revision(3, 4),
            "https://example.com/mcp",
        )
        .await
        .expect_err("mixed DNS is denied");
        assert!(matches!(
            error,
            super::PoolError::Egress(EgressRejection::MixedPublicPrivate)
        ));
        assert!(pool.is_empty());
    }

    #[tokio::test]
    async fn the_pool_is_bounded_and_evicts_without_changing_authority() {
        let pool = ConnectionPool::new(NonZeroUsize::new(2).expect("positive"));
        let resolver = resolver([public(34), public(35), public(36)]);
        for workspace in 1..=3 {
            screened_client(
                &pool,
                resolver.as_ref(),
                scope(1, workspace),
                revision(1, 1),
                "https://example.com/mcp",
            )
            .await
            .expect("client");
        }
        assert_eq!(pool.len(), 2);
    }
}
