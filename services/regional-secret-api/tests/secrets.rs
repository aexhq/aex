//! Secret-edge plaintext, lineage and route-isolation requirements.

use aex_wire::routes::RouteId;
use regional_secret_api::{
    AdmissionError, SecretPlaintext, SecretRecord, admit_plaintext, secret_route_ids,
};

#[test]
fn plaintext_is_redacted_and_zeroized_before_admission_returns() {
    let secret = SecretPlaintext::new(b"customer-provider-key".to_vec()).expect("secret");
    assert_eq!(format!("{secret:?}"), "SecretPlaintext(<redacted>)");
    let mut observed_after_encrypt = Vec::new();
    let ciphertext = admit_plaintext(secret, |bytes| {
        let ciphertext = bytes.iter().map(|byte| byte ^ 0xa5).collect::<Vec<_>>();
        observed_after_encrypt.extend_from_slice(bytes);
        Ok::<_, &'static str>(ciphertext)
    })
    .expect("encrypted");
    assert_ne!(
        ciphertext.expose_for_persistence(),
        b"customer-provider-key"
    );
    assert_eq!(observed_after_encrypt, b"customer-provider-key");
    assert!(!format!("{ciphertext:?}").contains("customer-provider-key"));
}

#[test]
fn generations_and_revocation_epochs_are_monotonic_and_if_match_is_strict() {
    let first = SecretRecord::create(vec![1, 2, 3], "intent-a").expect("record");
    assert_eq!(first.generation(), 1);
    assert_eq!(first.revocation_epoch(), 0);
    assert_eq!(
        first.replace(vec![4], "wrong-etag", "intent-b"),
        Err(AdmissionError::PreconditionFailed)
    );
    let second = first
        .replace(vec![4], first.etag(), "intent-b")
        .expect("replacement");
    assert_eq!(second.generation(), 2);
    let revoked = second.revoke("revoke-a").expect("revoked");
    assert_eq!(revoked.revocation_epoch(), 1);
    assert_eq!(revoked.revoke("revoke-a").expect("exact replay"), revoked);
    assert_eq!(
        revoked.revoke("different-intent"),
        Err(AdmissionError::IdempotencyConflict)
    );
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
