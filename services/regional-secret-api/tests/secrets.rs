//! Provider-credential route isolation and plaintext bounds.

use aex_secret_domain::plaintext::SecretPlaintext;
use aex_wire::ids::{PrefixedId as _, ProviderCredentialId, ResourceName, Uuid7};
use aex_wire::routes::RouteId;
use regional_secret_api::handlers::secret_limits;
use regional_secret_api::provider_credential_route_ids;

#[test]
fn secret_edge_mounts_only_provider_credential_registration() {
    let routes = provider_credential_route_ids();
    assert_eq!(routes, vec![RouteId::ProviderCredentialRegister]);
    assert!(!routes.contains(&RouteId::SessionCreate));
}

/// The minted identity must be a legal custody name without transformation.
#[test]
fn every_credential_identity_satisfies_the_resource_name_grammar() {
    for seed in 0..32u8 {
        let id = ProviderCredentialId::from_uuid7(Uuid7::compose(1_754_051_696_789, [seed; 10]));
        ResourceName::parse(&id.to_string())
            .unwrap_or_else(|error| panic!("`{id}` is not a resource name: {error}"));
    }
}

#[test]
fn the_declared_credential_bound_is_the_owner_accepted_one() {
    assert_eq!(secret_limits().max_provider_credentials, 100);
}

#[test]
fn the_plaintext_bound_is_the_domain_bound() {
    assert_eq!(SecretPlaintext::MAX_BYTES, 65_536);
    assert!(SecretPlaintext::new(Vec::new()).is_err());
    assert!(SecretPlaintext::new(vec![1; SecretPlaintext::MAX_BYTES + 1]).is_err());
}
