//! The canonical replay intent.
//!
//! Two headers, two identities, one digest. The identity binds the replay key,
//! the authenticated principal, the scope the request acts in, the method and
//! the canonical route template. The digest is RFC 8785 JCS over the validated
//! body under a **strict integer-only profile**.
//!
//! Canonicalization itself is `aex_wire::canonical` and nothing else. This
//! module adds exactly one rule on top: a floating-point number anywhere in the
//! body is [`IntentError::FloatingPointForbidden`] rather than a normalized
//! integer. Two requests that differ only in `1.0` versus `1` are the same
//! request to a canonicalizer and different requests to a database `NUMERIC`
//! column, and the system this replaces carried two canonical-JSON
//! implementations with different strictness both feeding one `sha256:` hash.

use aex_wire::canonical::{CanonicalError, to_jcs_bytes};
use aex_wire::types::{HttpMethod, JsonPointer};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::authz::PrincipalKindTag;

/// The digest that decides whether two requests are the same request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IntentHash([u8; 32]);

impl IntentHash {
    /// Wraps a stored digest.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The raw digest.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Which replay header carried the identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IdempotencyKeyKind {
    /// `Idempotency-Key`, for a mutation that returns its own result.
    IdempotencyKey,
    /// `Aex-Operation-Id`, for a mutation that creates a durable operation.
    OperationId,
}

impl IdempotencyKeyKind {
    /// The database spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IdempotencyKey => "idempotency_key",
            Self::OperationId => "operation_id",
        }
    }
}

/// What the request acts inside.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ScopeKind {
    /// The request is organization-scoped.
    Organization,
    /// The request is workspace-scoped.
    Workspace,
}

impl ScopeKind {
    /// The database spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Organization => "organization",
            Self::Workspace => "workspace",
        }
    }
}

/// The eight-field identity a replay record is keyed by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdempotencyIdentity {
    /// Which header carried the key.
    pub key_kind: IdempotencyKeyKind,
    /// The caller-supplied key.
    pub key_value: String,
    /// Which kind of principal presented the credential.
    pub principal_kind: PrincipalKindTag,
    /// Which principal.
    pub principal_id: Uuid,
    /// Which kind of scope the request acts in.
    pub scope_kind: ScopeKind,
    /// Which scope.
    pub scope_id: Uuid,
    /// The HTTP method.
    pub method: HttpMethod,
    /// The canonical route template, exactly as the generated route table
    /// spells it. Binding the template rather than the concrete path is what
    /// stops one key from replaying across two resources.
    pub route: String,
}

/// Why an intent could not be canonicalized.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IntentError {
    /// The body carried a floating-point number.
    #[error("the value at `{pointer}` is a floating-point number; a replay intent is integer-only")]
    FloatingPointForbidden {
        /// Where the offending number sits.
        pointer: JsonPointer,
    },
    /// The body could not be canonicalized at all.
    #[error(transparent)]
    Canonical(#[from] CanonicalError),
}

/// The domain-separating prefix, so an intent digest can never be replayed as a
/// digest over anything else this platform hashes.
const DOMAIN: &[u8] = b"aex/control/intent/v1\x1f";

/// Computes the intent digest for one request.
///
/// # Errors
///
/// Returns [`IntentError::FloatingPointForbidden`] when any number in `body` is
/// not an integer, and [`IntentError::Canonical`] when the body cannot be
/// canonicalized.
pub fn canonical_intent_hash(
    identity: &IdempotencyIdentity,
    body: &serde_json::Value,
) -> Result<IntentHash, IntentError> {
    reject_floats(body, &mut Vec::new())?;
    let canonical = to_jcs_bytes(body)?;

    let mut hasher = Sha256::new();
    hasher.update(DOMAIN);
    for part in [
        identity.key_kind.as_str().as_bytes(),
        identity.key_value.as_bytes(),
        identity.principal_kind.as_str().as_bytes(),
        identity.principal_id.as_bytes().as_slice(),
        identity.scope_kind.as_str().as_bytes(),
        identity.scope_id.as_bytes().as_slice(),
        identity.method.as_str().as_bytes(),
        identity.route.as_bytes(),
        canonical.as_slice(),
    ] {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    Ok(IntentHash(hasher.finalize().into()))
}

/// Walks the document rejecting every non-integer number.
fn reject_floats(value: &serde_json::Value, path: &mut Vec<String>) -> Result<(), IntentError> {
    match value {
        serde_json::Value::Number(number) => {
            if number.is_i64() || number.is_u64() {
                Ok(())
            } else {
                Err(IntentError::FloatingPointForbidden {
                    pointer: pointer(path),
                })
            }
        }
        serde_json::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                path.push(index.to_string());
                reject_floats(item, path)?;
                path.pop();
            }
            Ok(())
        }
        serde_json::Value::Object(members) => {
            for (key, member) in members {
                path.push(key.clone());
                reject_floats(member, path)?;
                path.pop();
            }
            Ok(())
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::String(_) => {
            Ok(())
        }
    }
}

/// Builds the RFC 6901 pointer for the current walk position.
fn pointer(path: &[String]) -> JsonPointer {
    if path.is_empty() {
        return JsonPointer::root();
    }
    path.iter()
        .fold(JsonPointer::root(), |pointer, token| pointer.child(token))
}

#[cfg(test)]
mod tests {
    use super::{
        IdempotencyIdentity, IdempotencyKeyKind, IntentError, ScopeKind, canonical_intent_hash,
    };
    use crate::authz::PrincipalKindTag;
    use aex_wire::types::HttpMethod;
    use uuid::Uuid;

    fn identity() -> IdempotencyIdentity {
        IdempotencyIdentity {
            key_kind: IdempotencyKeyKind::IdempotencyKey,
            key_value: "k-1".to_owned(),
            principal_kind: PrincipalKindTag::AccountActor,
            principal_id: Uuid::from_u128(1),
            scope_kind: ScopeKind::Organization,
            scope_id: Uuid::from_u128(2),
            method: HttpMethod::Post,
            route: "/api/workspaces".to_owned(),
        }
    }

    #[test]
    fn the_same_identity_and_body_produce_the_same_digest() {
        let body = serde_json::json!({ "name": "acme", "region": "eu-west-1" });
        let reordered = serde_json::json!({ "region": "eu-west-1", "name": "acme" });
        assert_eq!(
            canonical_intent_hash(&identity(), &body),
            canonical_intent_hash(&identity(), &reordered)
        );
    }

    #[test]
    fn a_changed_body_changes_the_digest() {
        let first = canonical_intent_hash(&identity(), &serde_json::json!({ "name": "acme" }));
        let second = canonical_intent_hash(&identity(), &serde_json::json!({ "name": "acme2" }));
        assert_ne!(first, second);
    }

    #[test]
    fn every_identity_field_participates() {
        let body = serde_json::json!({ "name": "acme" });
        let base = canonical_intent_hash(&identity(), &body).expect("base");

        let mut changed = identity();
        changed.key_kind = IdempotencyKeyKind::OperationId;
        assert_ne!(canonical_intent_hash(&changed, &body).expect("k"), base);

        let mut changed = identity();
        changed.key_value = "k-2".to_owned();
        assert_ne!(canonical_intent_hash(&changed, &body).expect("v"), base);

        let mut changed = identity();
        changed.principal_kind = PrincipalKindTag::WorkspaceKey;
        assert_ne!(canonical_intent_hash(&changed, &body).expect("pk"), base);

        let mut changed = identity();
        changed.principal_id = Uuid::from_u128(9);
        assert_ne!(canonical_intent_hash(&changed, &body).expect("pi"), base);

        let mut changed = identity();
        changed.scope_kind = ScopeKind::Workspace;
        assert_ne!(canonical_intent_hash(&changed, &body).expect("sk"), base);

        let mut changed = identity();
        changed.scope_id = Uuid::from_u128(9);
        assert_ne!(canonical_intent_hash(&changed, &body).expect("si"), base);

        let mut changed = identity();
        changed.method = HttpMethod::Delete;
        assert_ne!(canonical_intent_hash(&changed, &body).expect("m"), base);

        let mut changed = identity();
        changed.route = "/api/api-keys".to_owned();
        assert_ne!(canonical_intent_hash(&changed, &body).expect("r"), base);
    }

    #[test]
    fn a_field_boundary_cannot_be_shifted_between_adjacent_fields() {
        let body = serde_json::json!({});
        let mut left = identity();
        left.key_value = "ab".to_owned();
        left.route = "c".to_owned();
        let mut right = identity();
        right.key_value = "a".to_owned();
        right.route = "bc".to_owned();
        assert_ne!(
            canonical_intent_hash(&left, &body),
            canonical_intent_hash(&right, &body),
            "length prefixing makes the concatenation unambiguous"
        );
    }

    #[test]
    fn a_floating_point_number_anywhere_is_refused() {
        for body in [
            serde_json::json!(1.5),
            serde_json::json!({ "amount": 1.5 }),
            serde_json::json!({ "nested": { "amount": 1.5 } }),
            serde_json::json!([0, 1.5]),
            serde_json::json!({ "list": [{ "amount": 1.5 }] }),
        ] {
            let error = canonical_intent_hash(&identity(), &body)
                .expect_err("a replay intent is integer-only");
            assert!(
                matches!(error, IntentError::FloatingPointForbidden { .. }),
                "{body} produced {error:?}"
            );
        }
    }

    #[test]
    fn the_offending_pointer_names_the_number() {
        let body = serde_json::json!({ "policy": { "thresholds": [0, 2.5] } });
        let error = canonical_intent_hash(&identity(), &body).expect_err("a float is refused");
        let IntentError::FloatingPointForbidden { pointer } = error else {
            panic!("expected a float rejection, got {error:?}");
        };
        assert_eq!(pointer.as_str(), "/policy/thresholds/1");
    }

    #[test]
    fn an_integer_valued_float_is_still_refused() {
        let body = serde_json::json!({ "amount": 1.0 });
        assert!(matches!(
            canonical_intent_hash(&identity(), &body),
            Err(IntentError::FloatingPointForbidden { .. })
        ));
    }

    #[test]
    fn integers_of_both_signs_are_accepted() {
        let body = serde_json::json!({ "a": -9_007_199_254_740_991_i64, "b": 42 });
        assert!(canonical_intent_hash(&identity(), &body).is_ok());
    }
}
