//! Route ownership: the partition of the generated regional table across the
//! regional deployables.
//!
//! There is exactly one route table. Ownership is a *projection* of it, derived
//! per route from the generated descriptor, so a new regional route lands on a
//! deployable by construction rather than by somebody remembering to add it to a
//! second list.

use aex_wire::error::PrecedenceStage;
use aex_wire::routes::{Plane, RouteId, TransportKind, route};
use aex_wire::server::RouteGroup;
use aex_wire::types::Region;

/// One-to-one with the wire-contract precedence table.
pub const EDGE_PRECEDENCE: [PrecedenceStage; 13] = PrecedenceStage::ALL;

/// Exactly one deployable owner for every generated regional route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RouteOwner {
    /// Finite session/resource API.
    SessionApi,
    /// Plaintext secret/provider-credential admission API.
    SecretApi,
    /// Long-lived NDJSON service.
    Stream,
    /// Observation query/export peer.
    ObservationApi,
    /// OTLP admission peer.
    Otlp,
}

impl RouteOwner {
    /// Every owner, in declaration order.
    pub const ALL: [Self; 5] = [
        Self::SessionApi,
        Self::SecretApi,
        Self::Stream,
        Self::ObservationApi,
        Self::Otlp,
    ];

    /// The deployable name this owner deploys as.
    #[must_use]
    pub const fn deployable(self) -> &'static str {
        match self {
            Self::SessionApi => "regional-session-api",
            Self::SecretApi => "regional-secret-api",
            Self::Stream => "regional-stream",
            Self::ObservationApi => "regional-observation-api",
            Self::Otlp => "regional-otlp",
        }
    }

    /// Every regional route this deployable owns, in `RouteId` order.
    ///
    /// This is what a composition root mounts. Because it is derived from the
    /// table, a route that is authored and never mounted fails the composition
    /// test rather than answering `404` in production.
    #[must_use]
    pub fn routes(self) -> Vec<RouteId> {
        RouteId::ALL
            .iter()
            .copied()
            .filter(|id| route_owner(*id) == Some(self))
            .collect()
    }

    /// The subset of `group` this deployable owns, in `RouteId` order.
    ///
    /// A group is one authoring fragment, and two fragments are split across two
    /// deployables: `regional:secrets` (metadata reads here, plaintext admission
    /// there) and `regional:provider-credentials`. Mounting therefore iterates a
    /// group and filters by owner; it never lists templates.
    #[must_use]
    pub fn routes_in(self, group: RouteGroup) -> Vec<RouteId> {
        group
            .routes()
            .iter()
            .copied()
            .filter(|id| route_owner(*id) == Some(self))
            .collect()
    }

    /// Every group this deployable draws at least one route from, in group order.
    #[must_use]
    pub fn groups(self) -> Vec<RouteGroup> {
        RouteGroup::ALL
            .iter()
            .copied()
            .filter(|group| !self.routes_in(*group).is_empty())
            .collect()
    }
}

/// Resolves a generated route to its one regional deployable.
///
/// Returns `None` for a central route, which no regional deployable may serve.
#[must_use]
pub fn route_owner(id: RouteId) -> Option<RouteOwner> {
    let descriptor = route(id);
    if descriptor.plane != Plane::Regional {
        return None;
    }
    if descriptor.transport == TransportKind::Ndjson {
        return Some(RouteOwner::Stream);
    }
    if descriptor.fragment == "otlp" {
        return Some(RouteOwner::Otlp);
    }
    if matches!(descriptor.fragment, "observations" | "telemetry-lifecycle") {
        return Some(RouteOwner::ObservationApi);
    }
    if matches!(
        id,
        RouteId::SecretPut
            | RouteId::SecretDelete
            | RouteId::SecretRevoke
            | RouteId::ProviderCredentialRegister
    ) {
        return Some(RouteOwner::SecretApi);
    }
    Some(RouteOwner::SessionApi)
}

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
