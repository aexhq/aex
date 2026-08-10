//! Plaintext admission boundary for secrets and provider credentials.

pub mod config;
pub mod handlers;

use std::sync::Arc;

use aex_regional_http::router::RouteOwner;
use aex_wire::routes::RouteId;

use crate::config::Config;

pub use handlers::{Routes, Shared};

/// The composed secret edge: the custody authority plus the envelope crypto over
/// the secret `KMS` key, and nothing else.
///
/// The type is the capability statement. There is no field here that can reach
/// the session authority, the content bucket, the work table or a queue, so the
/// composition test that asserts the absence has something structural to assert
/// rather than a comment to trust.
///
/// It is a **statement**, not a second holder of the request path's adapters:
/// the value the router dispatches through is [`Shared`], and the composition
/// assertion is pointed there (D-4). This type keeps the readiness derivation
/// and nothing else.
pub struct SecretEdge<S, C> {
    custody: Arc<S>,
    crypto: Arc<C>,
}

impl<S, C> SecretEdge<S, C> {
    /// Binds the edge to its two adapters.
    #[must_use]
    pub const fn new(custody: Arc<S>, crypto: Arc<C>) -> Self {
        Self { custody, crypto }
    }

    /// The custody authority.
    #[must_use]
    pub fn custody(&self) -> &S {
        &self.custody
    }

    /// The envelope crypto.
    #[must_use]
    pub fn crypto(&self) -> &C {
        &self.crypto
    }

    /// The deployable this edge composes.
    #[must_use]
    pub const fn owner() -> RouteOwner {
        RouteOwner::SecretApi
    }

    /// Dependencies of the served route set that are not yet resolved.
    ///
    /// Readiness is derived from the composition rather than declared: a name
    /// appears here only while something the served routes need is unbound.
    #[must_use]
    pub fn unresolved(&self, config: &Config) -> Vec<String> {
        let mut unresolved = Vec::new();
        if config.secret_custody_table.is_empty() {
            unresolved.push("regional-secret-custody".to_owned());
        }
        if config.secret_keystore_table.is_empty() {
            unresolved.push("regional-secret-keystore".to_owned());
        }
        unresolved
    }
}

/// Plaintext-bearing routes owned exclusively by this edge.
#[must_use]
pub fn secret_route_ids() -> Vec<RouteId> {
    vec![
        RouteId::ProviderCredentialRegister,
        RouteId::SecretDelete,
        RouteId::SecretPut,
        RouteId::SecretRevoke,
    ]
}

// D-14, applied. A second `SecretPlaintext` / `Ciphertext` / `SecretRecord` /
// `admit_plaintext` / `AdmissionError` / `derive_etag` kernel used to live here,
// duplicating `aex-secret-domain`. It was authored as a pure kernel for a
// composition root that did not exist yet; the root exists now and composes the
// real crates, so the duplicate was dead **and** wrong by current decisions.
//
// Two `SecretPlaintext` types in one binary is exactly how the redaction
// guarantee gets weakened by accident: the local one implemented neither the
// `expose_for_encryption` naming convention nor `Zeroizing`, so a reviewer
// grepping that one symbol for every plaintext touch point would have missed
// every use of it. Its `derive_etag` hashed `(generation, epoch, ciphertext)`,
// contradicting RS-29 (a tag is a digest of the **projected representation**),
// and its `SecretRecord::revoke` bumped a per-record epoch rather than raising
// the `revokedThroughRevision` fence RS-30 reads.
//
// `secret_put` is written against `aex_secret_domain::plaintext::SecretPlaintext`
// and `aex_secret_custody_dynamodb::codec` and can no longer reach for the wrong
// type, because the wrong type is gone.
