//! Generated-route partition for the finite session API.

use aex_regional_http::router::RouteOwner;
use aex_wire::routes::{RouteId, TransportKind, route};

/// Routes this finite deployable may fully serve.
#[must_use]
pub fn session_route_ids() -> Vec<RouteId> {
    RouteOwner::SessionApi
        .routes()
        .into_iter()
        .filter(|id| {
            matches!(
                route(*id).transport,
                TransportKind::Unary | TransportKind::Binary
            )
        })
        .collect()
}
