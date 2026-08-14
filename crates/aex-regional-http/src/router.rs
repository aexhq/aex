//! Planned route ownership: the partition of the generated regional table
//! across the regional deployables responsible for closing it.
//!
//! There is exactly one route table. Ownership is a *projection* of it, derived
//! per route from the generated descriptor. This assigns responsibility and
//! selection closure; actual mount evidence is the narrower `servedArtifact`
//! projection in the generated delivery registry.

use aex_wire::error::PrecedenceStage;
use aex_wire::routes::{Plane, RouteId, route};
use aex_wire::server::RouteGroup;
use aex_wire::types::Region;

/// One-to-one with the wire-contract precedence table.
pub const EDGE_PRECEDENCE: [PrecedenceStage; 13] = PrecedenceStage::ALL;

/// Exactly one planned deployable owner for every generated regional route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RouteOwner {
    /// Finite session/resource API.
    SessionApi,
}

impl RouteOwner {
    /// Every owner, in declaration order.
    pub const ALL: [Self; 1] = [Self::SessionApi];

    /// The deployable name this owner deploys as.
    ///
    #[must_use]
    pub const fn deployable(self) -> &'static str {
        match self {
            Self::SessionApi => "session-api",
        }
    }

    /// The stable spelling of this owner itself, which is unique per variant.
    ///
    /// Used by mount diagnostics.
    #[must_use]
    pub const fn half(self) -> &'static str {
        match self {
            Self::SessionApi => "session-api:unary",
        }
    }

    /// Every regional route this deployable is planned to own, in `RouteId` order.
    ///
    /// A composition root may mount only the generated actual-service subset;
    /// callers must not treat this planned partition as mount evidence.
    #[must_use]
    pub fn routes(self) -> Vec<RouteId> {
        RouteId::ALL
            .iter()
            .copied()
            .filter(|id| route_owner(*id) == Some(self))
            .collect()
    }

    /// The planned subset of `group` for this deployable, in `RouteId` order.
    ///
    /// A composition narrows this planned slice to its actual-service set before
    /// mounting.
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

/// Resolves a generated route to its one planned regional deployable.
///
/// Returns `None` for a central route, which no regional deployable may serve.
///
#[must_use]
pub fn route_owner(id: RouteId) -> Option<RouteOwner> {
    let descriptor = route(id);
    if descriptor.plane != Plane::Regional {
        return None;
    }
    match descriptor.serving_artifact {
        "session-api" => Some(RouteOwner::SessionApi),
        _ => None,
    }
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
