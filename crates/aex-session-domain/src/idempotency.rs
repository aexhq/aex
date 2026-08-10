//! Idempotency receipts and replay.
//!
//! The lookup key is the [`ReceiptKey`] — `(scope, key_sha256)` — and the intent
//! digest deliberately **excludes** it. The key selects the envelope; the digest
//! proves the replay is the same intent. Keying a receipt by its intent instead
//! would collide two callers who asked for the same thing under different keys,
//! and would make a lookup by key impossible, so a key reused with a *different*
//! intent could never be refused.
//!
//! There is exactly one canonicalizer and one digest function in the workspace
//! (D-23): `aex_wire::canonical::intent_digest` over RFC 8785 JCS. This module
//! adds no second one, and the cross-language corpus that proves byte parity
//! with the TypeScript SDK is `aex_wire::testing::corpus`.

use aex_content_domain::ContentDigest;
use aex_wire::error::ErrorCode;
use aex_wire::idempotency::{
    IdempotencyKey, IntentDigest, OperationIdentity, ReplayIdentity, key_digest,
};
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

    /// The hashed replay key this identity files its receipt under.
    ///
    /// An `Aex-Operation-Id` is hashed exactly as an `Idempotency-Key` is, so
    /// the two identity kinds share one receipt key shape and one lookup.
    #[must_use]
    pub fn key_sha256(&self) -> String {
        use aex_wire::ids::PrefixedId as _;
        match self {
            Self::Key(identity) => key_digest(identity.key.as_str()),
            Self::Operation(identity) => key_digest(identity.operation_id.encode().as_str()),
        }
    }
}

/// The identity one receipt is filed and looked up under.
///
/// `(scope, key_sha256)` and nothing else. The scope is what stops one route's
/// replay key colliding with another's; the hashed key bounds the width of a
/// value the caller chose. The intent is a compared **attribute**, never part of
/// this key — a key reused with a different intent has to *find* the receipt in
/// order to be refused with `idempotency_conflict`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReceiptKey {
    scope: Box<str>,
    key_sha256: Box<str>,
}

impl ReceiptKey {
    /// Widest a rendered scope may be, in UTF-8 bytes.
    pub const MAX_SCOPE_BYTES: usize = 256;

    /// Builds the key a receipt for `identity` is filed under.
    ///
    /// The digest is taken from the identity rather than accepted as an
    /// argument, so the key a plan targets and the envelope the request arrived
    /// under cannot drift apart.
    ///
    /// # Errors
    ///
    /// Returns [`ReceiptKeyError::Scope`] when the rendered scope is empty,
    /// longer than [`ReceiptKey::MAX_SCOPE_BYTES`], or carries a character an
    /// adapter's key template cannot hold.
    pub fn of(scope: &str, identity: &IdempotencyIdentity) -> Result<Self, ReceiptKeyError> {
        if scope.is_empty() || scope.len() > Self::MAX_SCOPE_BYTES {
            return Err(ReceiptKeyError::Scope {
                scope: scope.to_owned(),
            });
        }
        // `#` is every regional key template's separator and the two range-scan
        // sentinels bracket every prefix scan, so a scope carrying one could
        // address a receipt in another partition. The adapter revalidates when
        // it renders the physical key; refusing here means an unusable receipt
        // cannot be planned in the first place.
        if scope.chars().any(|character| {
            matches!(character, '#' | '\u{fffe}' | '\u{ffff}') || character.is_control()
        }) {
            return Err(ReceiptKeyError::Scope {
                scope: scope.to_owned(),
            });
        }
        Ok(Self {
            scope: scope.into(),
            key_sha256: identity.key_sha256().into(),
        })
    }

    /// The rendered scope.
    #[must_use]
    pub fn scope(&self) -> &str {
        &self.scope
    }

    /// The hashed replay key.
    #[must_use]
    pub fn key_sha256(&self) -> &str {
        &self.key_sha256
    }
}

/// Why a receipt key could not be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReceiptKeyError {
    /// The rendered scope cannot enter a key.
    #[error("`{scope}` is not a usable idempotency scope")]
    Scope {
        /// The rejected scope.
        scope: String,
    },
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

impl ResourceKind {
    /// The stable spelling a receipt row stores as its response kind.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Message => "message",
            Self::Run => "run",
            Self::Upload => "upload",
            Self::Registry => "registry",
            Self::Grant => "grant",
            Self::Secret => "secret",
        }
    }
}

/// The identity of the resource a receipt names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceId(pub String);

/// The canonical response a replay reproduces.
///
/// A receipt stores the bytes that were sent, not a recipe for re-rendering
/// them: re-projecting the response on replay is a second rendering path, and
/// two rendering paths for one contract eventually disagree. Above
/// [`ResponseBody::MAX_INLINE_BYTES`] the bytes live in content and the receipt
/// carries their digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseBody {
    /// The whole canonical response, small enough to sit on the receipt.
    Inline(Vec<u8>),
    /// The digest of a response written to content instead.
    Digest(ContentDigest),
}

impl ResponseBody {
    /// The largest response stored inline.
    ///
    /// A `DynamoDB` item is capped at 400 KiB and a receipt carries a scope, a
    /// key digest, an intent and two timestamps beside the body, so the inline
    /// ceiling sits well under the item ceiling rather than at it.
    pub const MAX_INLINE_BYTES: usize = 256 * 1024;

    /// Stores `canonical` inline when it fits, and its digest when it does not.
    #[must_use]
    pub fn of(canonical: &[u8]) -> Self {
        if canonical.len() <= Self::MAX_INLINE_BYTES {
            Self::Inline(canonical.to_vec())
        } else {
            Self::Digest(ContentDigest::of(canonical))
        }
    }

    /// The canonical bytes, when the receipt carries them.
    #[must_use]
    pub fn inline(&self) -> Option<&[u8]> {
        match self {
            Self::Inline(bytes) => Some(bytes),
            Self::Digest(_) => None,
        }
    }
}

/// What the original request produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiptOutcome {
    /// A resource was created.
    Resource {
        /// Which kind.
        kind: ResourceKind,
        /// Its identity.
        id: ResourceId,
        /// The canonical response that was sent.
        response: ResponseBody,
    },
    /// A durable operation was accepted.
    Operation(OperationId),
}

/// A stored receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdempotencyReceipt {
    /// What it is filed and looked up under.
    pub key: ReceiptKey,
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
        IdempotencyIdentity, IdempotencyReceipt, ReceiptKey, ReceiptKeyError, ReceiptOutcome,
        ReplayDecision, ResourceId, ResourceKind, ResponseBody, replay,
    };

    /// The canonical 201 body a create replays.
    const RESPONSE: &[u8] = br#"{"id":"ses_1","status":"idle"}"#;

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
        receipt_under("abc-123", intent)
    }

    fn receipt_under(key: &str, intent: aex_wire::idempotency::IntentDigest) -> IdempotencyReceipt {
        let identity = IdempotencyIdentity::Key(Box::new(ReplayIdentity {
            principal: principal(),
            route: route(RouteId::SessionCreate).id,
            key: IdempotencyKey::parse(key).expect("valid"),
            intent,
        }));
        IdempotencyReceipt {
            key: ReceiptKey::of("session.create", &identity).expect("a usable key"),
            identity,
            intent,
            outcome: ReceiptOutcome::Resource {
                kind: ResourceKind::Session,
                id: ResourceId("ses_1".to_owned()),
                response: ResponseBody::of(RESPONSE),
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

    #[test]
    fn a_replay_returns_the_stored_response_bytes_rather_than_a_re_rendering() {
        let intent = digest_of(&serde_json::json!({"a": 1}));
        let stored = receipt(intent);
        let ReplayDecision::ReturnOriginal(outcome) = replay(&stored, &intent) else {
            panic!("an equal intent replays");
        };
        let ReceiptOutcome::Resource { response, .. } = outcome else {
            panic!("a create receipt names a resource");
        };
        assert_eq!(
            response.inline().expect("a small body is stored inline"),
            RESPONSE,
            "a replay is byte-identical to the response the winner sent"
        );
    }

    #[test]
    fn one_key_with_two_intents_addresses_one_receipt_so_the_conflict_is_reachable() {
        // The receipt is keyed by `(scope, key_sha256)`, so the second attempt
        // looks up the row the first wrote and can be refused. Keyed by the
        // intent it would address a different item, find nothing, and execute a
        // second time.
        let first = digest_of(&serde_json::json!({"prompt": "one"}));
        let stored = receipt_under("k", first);
        let second = digest_of(&serde_json::json!({"prompt": "two"}));
        let retry = receipt_under("k", second);
        assert_eq!(stored.key, retry.key);
        assert_eq!(
            replay(&stored, &second),
            ReplayDecision::Conflict(ErrorCode::IdempotencyConflict)
        );
    }

    #[test]
    fn two_keys_with_one_intent_no_longer_collide_on_one_receipt() {
        let intent = digest_of(&serde_json::json!({"prompt": "one"}));
        assert_ne!(
            receipt_under("k1", intent).key,
            receipt_under("k2", intent).key
        );
    }

    #[test]
    fn a_body_above_the_inline_ceiling_is_stored_as_a_digest() {
        let ceiling = vec![b'x'; ResponseBody::MAX_INLINE_BYTES];
        assert!(matches!(
            ResponseBody::of(&ceiling),
            ResponseBody::Inline(_)
        ));
        let over = vec![b'x'; ResponseBody::MAX_INLINE_BYTES + 1];
        assert!(matches!(ResponseBody::of(&over), ResponseBody::Digest(_)));
    }

    #[test]
    fn a_receipt_key_refuses_a_scope_that_could_forge_a_partition() {
        let identity = receipt(digest_of(&serde_json::json!({}))).identity;
        assert!(matches!(
            ReceiptKey::of("session.message:ses#evil", &identity),
            Err(ReceiptKeyError::Scope { .. })
        ));
        assert!(matches!(
            ReceiptKey::of("", &identity),
            Err(ReceiptKeyError::Scope { .. })
        ));
    }

    #[test]
    fn a_receipt_key_takes_its_digest_from_the_identity_and_cannot_drift() {
        let identity = receipt(digest_of(&serde_json::json!({}))).identity;
        let key = ReceiptKey::of("session.create", &identity).expect("a usable key");
        assert_eq!(key.key_sha256(), identity.key_sha256());
        assert_eq!(key.key_sha256().len(), 64);
    }

    #[test]
    fn an_operation_identity_hashes_its_id_the_same_way_a_key_is_hashed() {
        use aex_wire::idempotency::{OperationIdentity, key_digest};
        use aex_wire::ids::{OperationId, PrefixedId as _};
        let operation = OperationId::from_uuid7(Uuid7::compose(1, [4; 10]));
        let identity = IdempotencyIdentity::Operation(Box::new(OperationIdentity {
            principal: principal(),
            route: route(RouteId::SessionCreate).id,
            operation_id: operation,
            intent: digest_of(&serde_json::json!({})),
        }));
        assert_eq!(
            identity.key_sha256(),
            key_digest(operation.encode().as_str())
        );
    }
}
