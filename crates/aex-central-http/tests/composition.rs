//! Structural coverage for the consolidated central HTTP composition.

use std::collections::BTreeSet;

use aex_central_http::{CentralServiceId, central_groups};
use aex_wire::routes::{Plane, ROUTES};
use aex_wire::server::{API_KEYS_ROUTES, AUTH_ROUTES, BILLING_ROUTES, BOOTSTRAP_ROUTES};

#[test]
fn the_consolidated_runtime_owns_the_exact_personal_central_surface() {
    let published: BTreeSet<_> = ROUTES
        .iter()
        .filter(|route| route.plane == Plane::Central)
        .map(|route| route.id)
        .collect();
    let owned: BTreeSet<_> = CentralServiceId::CentralApi.routes().into_iter().collect();

    assert_eq!(published.len(), 14);
    assert_eq!(owned, published);
    assert_eq!(API_KEYS_ROUTES.len(), 3);
    assert_eq!(AUTH_ROUTES.len(), 3);
    assert_eq!(BILLING_ROUTES.len(), 7);
    assert_eq!(BOOTSTRAP_ROUTES.len(), 1);
    assert_eq!(central_groups().len(), 4);
}
