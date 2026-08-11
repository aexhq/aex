//! Bounded local load contract for provider-credential route selection.

use aex_wire::routes::RouteId;
use regional_secret_api::provider_credential_route_ids;

#[test]
fn provider_credential_route_selection_stays_closed_under_load() {
    let expected = vec![RouteId::ProviderCredentialRegister];
    for _ in 0..10_000 {
        assert_eq!(provider_credential_route_ids(), expected);
    }
}
