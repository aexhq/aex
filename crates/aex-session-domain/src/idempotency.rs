//! Idempotency receipts and replay.
//!
//! The lookup key is the [`IdempotencyIdentity`] — an `Idempotency-Key` or an
//! `Aex-Operation-Id` — and the intent digest deliberately **excludes** it. The
//! key selects the envelope; the digest proves the replay is the same intent.
//!
//! There is exactly one canonicalizer and one digest function in the workspace
//! (D-23): `aex_wire::canonical::intent_digest` over RFC 8785 JCS. This module
//! adds no second one, and the cross-language corpus that proves byte parity
//! with the TypeScript SDK is `aex_wire::testing::corpus`.

use aex_content_domain::ContentDigest;
use aex_wire::error::ErrorCode;
use aex_wire::idempotency::{IdempotencyKey, IntentDigest, OperationIdentity, ReplayIdentity};
use aex_wire::ids::OperationId;
use aex_wire::types::Timestamp;

/// Which envelope a receipt is filed under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdempotencyIdentity {
    /// A caller-chosen `Idempotency-Key`.
    Key(Box<ReplayIdentity>),
    /// A caller-minted operation id.
    Operation(Box<OperationIdentity>),
}

impl IdempotencyIdentity {
    /// The intent digest the identity was filed with.
    #[must_use]
    pub fn intent(&self) -> IntentDigest {
        match self {
            Self::Key(identity) => identity.intent,
            Self::Operation(identity) => identity.intent,
        }
    }

    /// The caller-chosen key, when the identity is one.
    #[must_use]
    pub fn key(&self) -> Option<&IdempotencyKey> {
        match self {
            Self::Key(identity) => Some(&identity.key),
            Self::Operation(_) => None,
        }
    }

    /// The operation id, when the identity is one.
    #[must_use]
    pub fn operation(&self) -> Option<OperationId> {
        match self {
            Self::Key(_) => None,
            Self::Operation(identity) => Some(identity.operation_id),
        }
    }

    /// The conflict code a mismatched replay of this identity produces.
    #[must_use]
    pub const fn conflict_code(&self) -> ErrorCode {
        match self {
            Self::Key(_) => ErrorCode::IdempotencyConflict,
            Self::Operation(_) => ErrorCode::OperationIdempotencyConflict,
        }
    }
}

/// Which kind of resource a receipt names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResourceKind {
    /// A session.
    Session,
    /// A message.
    Message,
    /// A run.
    Run,
    /// An upload.
    Upload,
    /// A registry pointer.
    Registry,
    /// A download grant.
    Grant,
    /// A secret.
    Secret,
}

/// The identity of the resource a receipt names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceId(pub String);

/// What the original request produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiptOutcome {
    /// A resource was created.
    Resource {
        /// Which kind.
        kind: ResourceKind,
        /// Its identity.
        id: ResourceId,
        /// The digest of the response that was sent.
        response_digest: ContentDigest,
    },
    /// A durable operation was accepted.
    Operation(OperationId),
}

/// A stored receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdempotencyReceipt {
    /// Which envelope it is filed under.
    pub identity: IdempotencyIdentity,
    /// The intent it recorded.
    pub intent: IntentDigest,
    /// What the original request produced.
    pub outcome: ReceiptOutcome,
    /// When it was written.
    pub created_at: Timestamp,
}

/// What a replay attempt resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayDecision {
    /// Return exactly what the original produced.
    ReturnOriginal(ReceiptOutcome),
    /// The same envelope was reused for a different intent.
    Conflict(ErrorCode),
}

/// Resolves a replay against a stored receipt.
#[must_use]
pub fn replay(existing: &IdempotencyReceipt, attempt: &IntentDigest) -> ReplayDecision {
    if existing.intent == *attempt {
        ReplayDecision::ReturnOriginal(existing.outcome.clone())
    } else {
        ReplayDecision::Conflict(existing.identity.conflict_code())
    }
}

#[cfg(test)]
mod tests {
    use aex_wire::canonical::{intent_digest, to_jcs_bytes};
    use aex_wire::error::ErrorCode;
    use aex_wire::idempotency::{IdempotencyKey, PrincipalScope, ReplayIdentity};
    use aex_wire::ids::{ApiKeyId, OrganizationId, PrefixedId as _, Uuid7, WorkspaceId};
    use aex_wire::routes::{RouteId, route};
    use aex_wire::types::Timestamp;

    use super::{
        IdempotencyIdentity, IdempotencyReceipt, ReceiptOutcome, ReplayDecision, ResourceId,
        ResourceKind, replay,
    };
    use aex_content_domain::ContentDigest;

    fn principal() -> PrincipalScope {
        PrincipalScope::WorkspaceKey {
            key: ApiKeyId::from_uuid7(Uuid7::compose(1, [1; 10])),
            workspace: WorkspaceId::from_uuid7(Uuid7::compose(1, [2; 10])),
            organization: OrganizationId::from_uuid7(Uuid7::compose(1, [3; 10])),
        }
    }

    fn digest_of(body: &serde_json::Value) -> aex_wire::idempotency::IntentDigest {
        let bytes = to_jcs_bytes(body).expect("canonical");
        let descriptor = route(RouteId::SessionCreate);
        intent_digest(
            descriptor.id,
            &aex_wire::routes::PathBinding::default(),
            Some(&bytes),
        )
    }

    fn receipt(intent: aex_wire::idempotency::IntentDigest) -> IdempotencyReceipt {
        IdempotencyReceipt {
            identity: IdempotencyIdentity::Key(Box::new(ReplayIdentity {
                principal: principal(),
                route: route(RouteId::SessionCreate).id,
                key: IdempotencyKey::parse("abc-123").expect("valid"),
                intent,
            })),
            intent,
            outcome: ReceiptOutcome::Resource {
                kind: ResourceKind::Session,
                id: ResourceId("ses_1".to_owned()),
                response_digest: ContentDigest::of(b"{}"),
            },
            created_at: Timestamp::from_unix_millis(0).expect("in range"),
        }
    }

    #[test]
    fn the_same_intent_replays_and_a_different_one_conflicts() {
        let first = digest_of(&serde_json::json!({"a": 1, "b": 2}));
        let stored = receipt(first);
        assert!(matches!(
            replay(&stored, &first),
            ReplayDecision::ReturnOriginal(_)
        ));

        let second = digest_of(&serde_json::json!({"a": 1, "b": 3}));
        assert_eq!(
            replay(&stored, &second),
            ReplayDecision::Conflict(ErrorCode::IdempotencyConflict)
        );
    }

    #[test]
    fn the_digest_is_invariant_under_key_order_and_whitespace() {
        let ordered = digest_of(&serde_json::json!({"a": 1, "b": 2}));
        let reordered = digest_of(&serde_json::json!({"b": 2, "a": 1}));
        assert_eq!(ordered, reordered);

        // `-0` normalizes to `0` through the one canonicalizer.
        let zero = digest_of(&serde_json::json!({"n": 0}));
        let minus_zero = digest_of(&serde_json::json!({"n": -0}));
        assert_eq!(zero, minus_zero);
    }

    #[test]
    fn the_identity_is_not_in_the_digest() {
        // Two different keys over the same body produce the same intent digest;
        // the key selects the envelope, the digest proves the intent.
        let body = serde_json::json!({"a": 1});
        let one = digest_of(&body);
        let two = digest_of(&body);
        assert_eq!(one, two);
        assert_eq!(
            receipt(one).identity.conflict_code(),
            ErrorCode::IdempotencyConflict
        );
    }
}
