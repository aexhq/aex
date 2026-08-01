//! Shared fixtures for the `aex-secret-custody-dynamodb` test targets.

#![allow(dead_code, reason = "each test target uses a different subset")]
#![allow(missing_docs, reason = "the module doc states what these fixtures are")]

use aex_secret_custody_dynamodb::codec::{
    CallAuthorization, CustodyHead, ProviderCredential, RedactionManifest, SecretMetadata,
    StoredGeneration,
};
use aex_secret_domain::custody::{CustodyEntry, CustodyRevision, CustodyState, OwnerKeyEdgeId};
use aex_secret_domain::revocation::RevocationEpoch;
use aex_secret_domain::secret::{
    CiphertextRef, SecretName, SecretRevision, SecretState, SourceGeneration,
};
use aex_wire::ids::{
    PrefixedId, ProviderCredentialId, ResourceName, SessionId, Uuid7, WorkspaceId,
};
use aex_wire::types::Timestamp;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::config::{BehaviorVersion, Credentials, Region};
use aws_smithy_http_client::test_util::{CaptureRequestReceiver, capture_request};

pub const TABLE: &str = "dev-eu-west-1-regional-secret-custody";

pub const DEFINITION: &str =
    include_str!("../../../../migrations/regional/tables/regional-secret-custody.json");

#[must_use]
pub fn capturing_client() -> (Client, CaptureRequestReceiver) {
    let (http_client, receiver) = capture_request(None);
    let config = aws_sdk_dynamodb::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("eu-west-1"))
        .credentials_provider(Credentials::new(
            "AKIDTESTTESTTESTTEST",
            "test-secret",
            None,
            None,
            "aex-tests",
        ))
        .http_client(http_client)
        .build();
    (Client::from_conf(config), receiver)
}

/// The captured request body, parsed as JSON.
///
/// # Panics
///
/// If no request was captured or the body is not JSON.
#[must_use]
pub fn captured_body(receiver: CaptureRequestReceiver) -> serde_json::Value {
    let request = receiver.expect_request();
    let bytes = request
        .body()
        .bytes()
        .expect("the DynamoDB request body is always in memory");
    serde_json::from_slice(bytes).expect("the DynamoDB request body is JSON")
}

#[must_use]
pub fn now() -> Timestamp {
    Timestamp::parse("2026-08-01T12:34:56.789Z").expect("the pinned spelling")
}

#[must_use]
pub fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [1; 10]))
}

#[must_use]
pub fn other_workspace() -> WorkspaceId {
    WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [9; 10]))
}

#[must_use]
pub fn session() -> SessionId {
    SessionId::from_uuid7(Uuid7::compose(1_754_051_696_789, [3; 10]))
}

#[must_use]
pub fn secret_name() -> SecretName {
    ResourceName::parse("openai-key").expect("an ASCII resource name")
}

#[must_use]
pub fn owner_key_edge() -> OwnerKeyEdgeId {
    OwnerKeyEdgeId(Uuid7::compose(1_754_051_696_789, [5; 10]))
}

#[must_use]
pub fn ciphertext() -> CiphertextRef {
    CiphertextRef {
        key_generation: 1,
        wrapped_key: vec![0xaa; 48],
        nonce: vec![0xbb; 12],
        ciphertext: vec![0xcc; 64],
    }
}

#[must_use]
pub fn metadata() -> SecretMetadata {
    SecretMetadata {
        workspace: workspace(),
        name: secret_name(),
        generation: SourceGeneration::FIRST,
        revision: SecretRevision::FIRST,
        state: SecretState::Ready,
        revocation_epoch: RevocationEpoch::INITIAL,
        revoked_through_revision: SecretRevision(0),
        created_at: now(),
        updated_at: now(),
        revoked_at: None,
    }
}

#[must_use]
pub fn generation() -> StoredGeneration {
    StoredGeneration {
        workspace: workspace(),
        name: secret_name(),
        generation: SourceGeneration::FIRST,
        ciphertext: ciphertext(),
        context_digest: [7; 32],
        created_at: now(),
        revoked_at: None,
    }
}

#[must_use]
pub fn custody_head() -> CustodyHead {
    CustodyHead {
        session: session(),
        workspace: workspace(),
        revision: CustodyRevision::FIRST,
        owner_key_edge: owner_key_edge(),
        state: CustodyState::Active,
        idle_epoch: 4,
        updated_at: now(),
    }
}

#[must_use]
pub fn entry() -> CustodyEntry {
    CustodyEntry {
        name: secret_name(),
        source_generation: SourceGeneration::FIRST,
        epoch_at_admission: RevocationEpoch::INITIAL,
        ciphertext: ciphertext(),
    }
}

#[must_use]
pub fn authorization() -> CallAuthorization {
    CallAuthorization {
        authorization_id: "auth-0001".to_owned(),
        session: session(),
        workspace: workspace(),
        name: secret_name(),
        custody_revision: CustodyRevision::FIRST,
        source_generation: SourceGeneration::FIRST,
        owner_key_edge: owner_key_edge(),
        created_at: now(),
    }
}

#[must_use]
pub fn manifest() -> RedactionManifest {
    RedactionManifest {
        session: session(),
        workspace: workspace(),
        revision: CustodyRevision::FIRST,
        algorithm: "HMAC-SHA-256".to_owned(),
        key_id: "redact-key-1".to_owned(),
        digests: vec!["a".repeat(64), "b".repeat(64)],
        updated_at: now(),
    }
}

#[must_use]
pub fn provider_credential() -> ProviderCredential {
    ProviderCredential {
        credential: ProviderCredentialId::from_uuid7(Uuid7::compose(1_754_051_696_789, [6; 10])),
        workspace: workspace(),
        provider: "openai".to_owned(),
        secret_name: secret_name(),
        source_generation: SourceGeneration::FIRST,
        state: "active".to_owned(),
        created_at: now(),
    }
}
