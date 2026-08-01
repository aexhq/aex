//! Shared axum composition and the fixed error-precedence order.

use aex_wire::error::PrecedenceStage;
use aex_wire::routes::Plane;
use aex_wire::types::Region;

use crate::capability::CompositionManifest;
use crate::wire_pending::{RegionalSecretApi, RegionalSessionApi, RegionalStreamApi};

/// One-to-one with the wire-contract precedence table.
pub const EDGE_PRECEDENCE: [PrecedenceStage; 13] = PrecedenceStage::ALL;

/// Shared edge services passed to each generated router.
#[derive(Debug, Clone)]
pub struct EdgeStack<V, L, K> {
    verifier: V,
    limits: L,
    cursor_keys: K,
    region: Region,
    plane: Plane,
}

impl<V, L, K> EdgeStack<V, L, K> {
    /// Constructs a fully resolved edge stack.
    #[must_use]
    pub const fn new(verifier: V, limits: L, cursor_keys: K, region: Region, plane: Plane) -> Self {
        Self {
            verifier,
            limits,
            cursor_keys,
            region,
            plane,
        }
    }

    /// Region fixed at startup.
    #[must_use]
    pub const fn region(&self) -> Region {
        self.region
    }

    /// Plane fixed at startup.
    #[must_use]
    pub const fn plane(&self) -> Plane {
        self.plane
    }

    /// Borrow the assertion verifier.
    #[must_use]
    pub const fn verifier(&self) -> &V {
        &self.verifier
    }

    /// Borrow the effective-limit resolver.
    #[must_use]
    pub const fn limits(&self) -> &L {
        &self.limits
    }

    /// Borrow the cursor key ring.
    #[must_use]
    pub const fn cursor_keys(&self) -> &K {
        &self.cursor_keys
    }
}

/// Mounts only the finite-session routes the implementation provides.
pub fn mount_session_api<A, V, L, K>(
    api: A,
    _edge: EdgeStack<V, L, K>,
    _caps: &CompositionManifest,
) -> axum::Router
where
    A: RegionalSessionApi + Clone + Send + Sync + 'static,
{
    api.router()
}

/// Mounts only plaintext-bearing secret registration routes.
pub fn mount_secret_api<A, V, L, K>(
    api: A,
    _edge: EdgeStack<V, L, K>,
    _caps: &CompositionManifest,
) -> axum::Router
where
    A: RegionalSecretApi + Clone + Send + Sync + 'static,
{
    api.router()
}

/// Mounts only the read-only streaming routes.
pub fn mount_stream_api<A, V, L, K>(
    api: A,
    _edge: EdgeStack<V, L, K>,
    _caps: &CompositionManifest,
) -> axum::Router
where
    A: RegionalStreamApi + Clone + Send + Sync + 'static,
{
    api.router()
}
