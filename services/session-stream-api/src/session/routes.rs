//! Generated-route partition for the finite session API.

use aex_wire::routes::{Plane, RouteId, TransportKind, route};

/// Routes this finite deployable may fully serve.
#[must_use]
pub fn session_route_ids() -> Vec<RouteId> {
    RouteId::ALL
        .iter()
        .copied()
        .filter(|id| {
            let descriptor = route(*id);
            if descriptor.plane != Plane::Regional
                || !matches!(
                    descriptor.transport,
                    TransportKind::Unary | TransportKind::Binary
                )
            {
                return false;
            }
            matches!(
                descriptor.fragment,
                "operations"
                    | "registry"
                    | "sessions"
                    | "files"
                    | "uploads"
                    | "usage"
                    | "workspace"
                    | "provider-credentials"
            )
        })
        .collect()
}
