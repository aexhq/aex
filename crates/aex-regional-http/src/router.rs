//! Planned route ownership: the partition of the generated regional table
//! across the regional deployables responsible for closing it.
//!
//! There is exactly one route table. Ownership is a *projection* of it, derived
//! per route from the generated descriptor. This assigns responsibility and
//! selection closure; actual mount evidence is the narrower `servedArtifact`
//! projection in the generated delivery registry.

use aex_wire::error::PrecedenceStage;
use aex_wire::routes::{Plane, RouteId, TransportKind, route};
use aex_wire::server::RouteGroup;
use aex_wire::types::Region;

/// One-to-one with the wire-contract precedence table.
pub const EDGE_PRECEDENCE: [PrecedenceStage; 13] = PrecedenceStage::ALL;

/// Exactly one planned deployable owner for every generated regional route.
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
    ///
    /// [`Self::SessionApi`] and [`Self::Stream`] deploy as the *same* artifact:
    /// they were merged into `session-stream-api` to halve the regional Fargate
    /// floor. They remain two owners because they are two mount strategies — one
    /// unary through [`crate::mount::mount_unary`], one long-lived NDJSON — and
    /// because keeping the partition is what makes splitting them apart again a
    /// unit-registry change rather than a contract change. Use [`Self::half`]
    /// when a diagnostic needs to tell the two apart.
    #[must_use]
    pub const fn deployable(self) -> &'static str {
        match self {
            Self::SessionApi | Self::Stream => "session-stream-api",
            Self::SecretApi => "regional-secret-api",
            Self::ObservationApi => "regional-observation-api",
            Self::Otlp => "regional-otlp",
        }
    }

    /// The stable spelling of this owner itself, which is unique per variant.
    ///
    /// [`Self::deployable`] is not: two owners share one artifact. A refusal that
    /// says a route belongs to `session-stream-api` rather than to
    /// `session-stream-api` helps nobody, so [`crate::mount::MountError`] names
    /// this instead.
    #[must_use]
    pub const fn half(self) -> &'static str {
        match self {
            Self::SessionApi => "session-stream-api:unary",
            Self::SecretApi => "regional-secret-api",
            Self::Stream => "session-stream-api:ndjson",
            Self::ObservationApi => "regional-observation-api",
            Self::Otlp => "regional-otlp",
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
    /// A group is one authoring fragment, and two fragments are split across two
    /// deployables: `regional:secrets` (metadata reads here, plaintext admission
    /// there) and `regional:provider-credentials`. A composition narrows this
    /// planned slice to its actual-service set before mounting.
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
/// `session-stream-api` resolves to two owners, split by transport. That is the
/// whole cost of merging the two deployables: the artifact string alone no
/// longer identifies a mount strategy, so the transport the contract already
/// declares is what separates the unary half from the NDJSON half. It is not a
/// heuristic — [`TransportKind`] is generated per route from the same table, and
/// `the_two_halves_partition_the_merged_artifact_by_transport` proves the split
/// is total and disjoint.
#[must_use]
pub fn route_owner(id: RouteId) -> Option<RouteOwner> {
    let descriptor = route(id);
    if descriptor.plane != Plane::Regional {
        return None;
    }
    match descriptor.serving_artifact {
        "session-stream-api" => Some(match descriptor.transport {
            TransportKind::Ndjson => RouteOwner::Stream,
            TransportKind::Unary | TransportKind::Binary => RouteOwner::SessionApi,
        }),
        "regional-secret-api" => Some(RouteOwner::SecretApi),
        "regional-observation-api" => Some(RouteOwner::ObservationApi),
        "regional-otlp" => Some(RouteOwner::Otlp),
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
