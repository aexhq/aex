//! Shared fixtures for the `aex-secret-custody-dynamodb` test targets.

#![allow(dead_code, reason = "each test target uses a different subset")]
#![allow(missing_docs, reason = "the module doc states what these fixtures are")]

use aex_secret_custody_dynamodb::codec::{
    CallAuthorization, CredentialState, CustodyHead, ProviderCredential, RedactionEntry,
    RedactionManifest, SecretMetadata, StoredGeneration,
};
use aex_secret_domain::custody::{CustodyEntry, CustodyRevision, CustodyState, OwnerKeyEdgeId};
use aex_secret_domain::plaintext::SecretPlaintext;
use aex_secret_domain::revocation::RevocationEpoch;
use aex_secret_domain::secret::{
    CiphertextRef, SecretName, SecretRevision, SecretState, SourceGeneration,
};
use aex_session_dynamodb::replay::{Receipt, ReceiptBody};
use aex_wire::idempotency::IntentDigest;
use aex_wire::ids::{
    PrefixedId, ProviderCredentialId, ResourceName, SessionId, Uuid7, WorkspaceId,
};
use aex_wire::models::ProviderId;
use aex_wire::types::Timestamp;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::config::{BehaviorVersion, Credentials, Region};
use aws_smithy_http_client::test_util::{
    CaptureRequestReceiver, ReplayEvent, StaticReplayClient, capture_request,
};
use aws_smithy_types::body::SdkBody;

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

/// A client that answers a scripted sequence and records every request.
///
/// The capture fixture answers exactly one request, so a bounded multi-read path
/// needs this instead: it proves *how many* requests a read costs, which is the
/// property under test.
#[must_use]
pub fn replaying_client(responses: usize) -> (Client, StaticReplayClient) {
    let events: Vec<ReplayEvent> = (0..responses)
        .map(|_| {
            ReplayEvent::new(
                http::Request::builder()
                    .method("POST")
                    .uri("https://dynamodb.eu-west-1.amazonaws.com/")
                    .body(SdkBody::empty())
                    .expect("a request"),
                http::Response::builder()
                    .status(200)
                    .body(SdkBody::from("{}"))
                    .expect("a response"),
            )
        })
        .collect();
    let replay = StaticReplayClient::new(events);
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
        .http_client(replay.clone())
        .build();
    (Client::from_conf(config), replay)
}

/// A client that answers the given response bodies in order and records every
/// request. This is how a multi-service-page read is scripted: each body is one
/// service page, and the recorded requests prove how the adapter resumed.
#[must_use]
pub fn scripted_client(bodies: Vec<serde_json::Value>) -> (Client, StaticReplayClient) {
    let events: Vec<ReplayEvent> = bodies
        .into_iter()
        .map(|body| {
            ReplayEvent::new(
                http::Request::builder()
                    .method("POST")
                    .uri("https://dynamodb.eu-west-1.amazonaws.com/")
                    .body(SdkBody::empty())
                    .expect("a request"),
                http::Response::builder()
                    .status(200)
                    .body(SdkBody::from(body.to_string()))
                    .expect("a response"),
            )
        })
        .collect();
    let replay = StaticReplayClient::new(events);
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
        .http_client(replay.clone())
        .build();
    (Client::from_conf(config), replay)
}

/// One stored item as the service serializes it in a response body.
#[must_use]
pub fn dynamo_json(item: &aex_session_dynamodb::attr::Item) -> serde_json::Value {
    serde_json::Value::Object(
        item.iter()
            .map(|(name, value)| {
                let value = match value {
                    aws_sdk_dynamodb::types::AttributeValue::S(value) => {
                        serde_json::json!({"S": value})
                    }
                    aws_sdk_dynamodb::types::AttributeValue::N(value) => {
                        serde_json::json!({"N": value})
                    }
                    aws_sdk_dynamodb::types::AttributeValue::Bool(value) => {
                        serde_json::json!({"BOOL": value})
                    }
                    other => panic!("the custody fixtures use no {other:?} attribute"),
                };
                (name.clone(), value)
            })
            .collect(),
    )
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
        source_revision: SecretRevision::FIRST,
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

/// The regional redaction key the fixture manifest is keyed under.
///
/// A fixture, never a deployed key: the real one is a regional secret the
/// collector resolves at cold start.
pub const REDACTION_KEY: &[u8] = b"regional-redaction-key-fixture";

/// The managed secret values the fixture manifest names.
///
/// Two different lengths on purpose. The reader indexes by declared length and
/// slides one window per distinct length, so a fixture whose entries were all
/// the same width would exercise a single window and prove nothing about the
/// length actually surviving the wire.
pub const REDACTED_SECRETS: [&str; 2] = ["sk-live-abcdef", "0123456789abcdef0123456789"];

/// One real `{len, hmac}` entry over a fixture secret.
///
/// The digest is `HMAC-SHA256(REDACTION_KEY, secret)` computed through the
/// writer's own constructor rather than a literal, so the fixture cannot assert
/// a shape the production encoder does not produce.
#[must_use]
pub fn redaction_entry(secret: &str) -> RedactionEntry {
    RedactionEntry::digest(REDACTION_KEY, secret.as_bytes()).expect("a fixture secret is narrow")
}

#[must_use]
pub fn manifest() -> RedactionManifest {
    RedactionManifest {
        session: session(),
        workspace: workspace(),
        revision: CustodyRevision::FIRST,
        algorithm: "HMAC-SHA-256".to_owned(),
        key_id: "redact-key-1".to_owned(),
        entries: REDACTED_SECRETS
            .iter()
            .copied()
            .map(redaction_entry)
            .collect(),
        updated_at: now(),
    }
}

#[must_use]
pub fn credential_id() -> ProviderCredentialId {
    ProviderCredentialId::from_uuid7(Uuid7::compose(1_754_051_696_789, [6; 10]))
}

#[must_use]
pub fn provider_credential() -> ProviderCredential {
    ProviderCredential {
        credential: credential_id(),
        workspace: workspace(),
        name: ResourceName::parse("primary-openai").expect("an ASCII resource name"),
        provider: ProviderId::Openai,
        secret_name: secret_name(),
        source_generation: SourceGeneration::FIRST,
        fingerprint: SecretPlaintext::new(b"sk-live-fixture".to_vec())
            .expect("a bounded plaintext")
            .credential_fingerprint(workspace(), credential_id()),
        revision: 1,
        state: CredentialState::Ready,
        created_at: now(),
        updated_at: now(),
        revoked_at: None,
    }
}

#[must_use]
pub fn receipt() -> Receipt {
    Receipt {
        scope: "secret:credential:primary-openai".to_owned(),
        key_sha256: "d".repeat(64),
        intent: IntentDigest::from_bytes([5; 32]),
        response_kind: "ProviderCredential".to_owned(),
        response: ReceiptBody::Inline(b"{}".to_vec()),
        committed_at: now(),
        expires_at: Timestamp::parse("2026-08-02T12:34:56.789Z").expect("the pinned spelling"),
    }
}
