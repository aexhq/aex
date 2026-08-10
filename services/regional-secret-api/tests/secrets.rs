//! Secret-edge route isolation and the reserved credential namespace.
//!
//! The plaintext, ciphertext and record kernel this target used to exercise is
//! gone (D-14): it was a second copy of `aex-secret-domain` inside this
//! deployable, and the properties it asserted are asserted on the real types in
//! `aex-secret-domain`'s own suites. What is left here is what belongs to the
//! deployable rather than to a domain crate: which routes it owns, and which
//! names it must refuse.

use aex_secret_domain::plaintext::SecretPlaintext;
use aex_wire::ids::{PrefixedId as _, ProviderCredentialId, ResourceName, Uuid7, WorkspaceId};
use aex_wire::routes::RouteId;
use regional_secret_api::handlers::{is_reserved_name, secret_limits};
use regional_secret_api::secret_route_ids;

fn credential_id() -> ProviderCredentialId {
    ProviderCredentialId::from_uuid7(Uuid7::compose(1_754_051_696_789, [5; 10]))
}

#[test]
fn secret_edge_mounts_only_plaintext_admission_and_revocation_routes() {
    let routes = secret_route_ids();
    assert_eq!(
        routes,
        vec![
            RouteId::ProviderCredentialRegister,
            RouteId::SecretDelete,
            RouteId::SecretPut,
            RouteId::SecretRevoke,
        ]
    );
    assert!(!routes.contains(&RouteId::SecretGet));
    assert!(!routes.contains(&RouteId::SessionCreate));
}

/// D-10. The reservation is **typed**, not a string-prefix convention: a name is
/// reserved exactly when it parses as a `ProviderCredentialId`. A prefix check
/// would reserve `pcr_anything` including names no registration can ever mint,
/// and would miss nothing in exchange.
#[test]
fn a_credential_identity_is_a_reserved_secret_name_and_an_ordinary_name_is_not() {
    let minted = credential_id().to_string();
    let reserved = ResourceName::parse(&minted).expect("a credential id is a legal resource name");
    assert!(is_reserved_name(&reserved));

    for ordinary in ["openai-key", "pcr", "pcr_", "pcr_not-a-uuid7", "prod.key"] {
        let name = ResourceName::parse(ordinary).expect("a resource name");
        assert!(!is_reserved_name(&name), "`{ordinary}` is not reserved");
    }
}

/// The minted name has to be usable as a secret name without transformation, or
/// D-10's "the minted secret name **is** the credential id's own string" is not
/// implementable.
#[test]
fn every_credential_identity_satisfies_the_resource_name_grammar() {
    for seed in 0..32u8 {
        let id = ProviderCredentialId::from_uuid7(Uuid7::compose(1_754_051_696_789, [seed; 10]));
        ResourceName::parse(&id.to_string())
            .unwrap_or_else(|error| panic!("`{id}` is not a resource name: {error}"));
    }
}

/// B8, as the owner answered it. The constants are a domain input rather than a
/// tunable: a limit nobody has ever hit is the one to pick, and raising it later
/// is a constant edit with no migration behind it.
#[test]
fn the_declared_bounds_are_the_owner_accepted_ones() {
    assert_eq!(secret_limits().max_secrets, 500);
    assert_eq!(secret_limits().max_provider_credentials, 100);
}

/// The structural ceiling that stops an unbounded body reaching an encryptor is
/// the domain's, and it is the only one this deployable relies on.
#[test]
fn the_plaintext_bound_is_the_domain_bound() {
    assert_eq!(SecretPlaintext::MAX_BYTES, 65_536);
    assert!(SecretPlaintext::new(Vec::new()).is_err());
    assert!(SecretPlaintext::new(vec![1; SecretPlaintext::MAX_BYTES + 1]).is_err());
}

#[test]
fn a_workspace_identity_is_never_a_reserved_secret_name() {
    let workspace = WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [2; 10]));
    let name = ResourceName::parse(&workspace.to_string()).expect("a resource name");
    assert!(
        !is_reserved_name(&name),
        "only the credential namespace is reserved"
    );
}
