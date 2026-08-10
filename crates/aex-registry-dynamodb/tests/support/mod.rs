//! Shared fixtures for the `aex-registry-dynamodb` test targets.

#![allow(dead_code, reason = "each test target uses a different subset")]
#![allow(missing_docs, reason = "the module doc states what these fixtures are")]

use aex_content_domain::identity::{RegistryKind, Revision};
use aex_wire::ids::{ContentHash, PrefixedId, ResourceName, UploadId, Uuid7, WorkspaceId};
use aex_wire::types::Timestamp;
use aex_registry_dynamodb::store::{SET_RESPONSE_KIND, SetReceipt};
use aex_session_dynamodb::replay::Receipt;
use aex_workspace_domain::registry::{
    RegistryCommit, RegistryPointer, RegistryRow, SetOutcome, ValueDocument, etag_of,
};
use aex_workspace_domain::upload::{PartPlan, PlannedPart, Upload, UploadState};
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::config::{BehaviorVersion, Credentials, Region};
use aws_smithy_http_client::test_util::{CaptureRequestReceiver, capture_request};

pub const TABLE: &str = "dev-eu-west-1-regional-registry";

pub const DEFINITION: &str =
    include_str!("../../../../migrations/regional/tables/regional-registry.json");

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
pub fn later(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(now().unix_millis() + millis).expect("in range")
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
pub fn upload_id() -> UploadId {
    UploadId::from_uuid7(Uuid7::compose(1_754_051_696_789, [4; 10]))
}

#[must_use]
pub fn name(text: &str) -> ResourceName {
    ResourceName::parse(text).expect("an ASCII resource name")
}

/// One canonical tool value document, over a payload digest the caller chooses.
#[must_use]
pub fn value_doc(payload: ContentHash) -> ValueDocument {
    ValueDocument::new(
        aex_wire::CanonicalJson::parse(&format!(
            r#"{{"description":"search","entry":"main.js","bundleFormat":"tar.gz",
                "inputSchema":{{"type":"object"}},
                "bundle":{{"sha256":"{}","sizeBytes":"2048"}}}}"#,
            payload.to_wire()
        ))
        .expect("valid JSON"),
    )
}

#[must_use]
pub fn pointer() -> RegistryPointer {
    let document = value_doc(ContentHash::from_bytes([0xab; 32]));
    let digest = document.digest();
    RegistryPointer {
        row: RegistryRow {
            workspace: workspace(),
            kind: RegistryKind::Tool,
            name: name("search"),
            revision: Revision::FIRST,
            etag: etag_of(RegistryKind::Tool, Revision::FIRST, &digest),
            sha256: digest,
            size_bytes: document.size_bytes(),
            created_at: now(),
            updated_at: now(),
        },
        value_doc: document,
    }
}

#[must_use]
pub fn next_pointer() -> RegistryPointer {
    let document = value_doc(ContentHash::from_bytes([0xcd; 32]));
    let digest = document.digest();
    let revision = Revision::FIRST.next();
    let base = pointer();
    RegistryPointer {
        row: RegistryRow {
            revision,
            etag: etag_of(RegistryKind::Tool, revision, &digest),
            sha256: digest,
            size_bytes: document.size_bytes(),
            updated_at: later(1_000),
            ..base.row
        },
        value_doc: document,
    }
}

#[must_use]
pub fn upload() -> Upload {
    Upload {
        id: upload_id(),
        workspace: workspace(),
        state: UploadState::PartsGranted,
        provider_upload_id: "provider-mpu-1".to_owned(),
        object_key: "wks/ab/cd/abcd".to_owned(),
        declared_size: 6 * 1024 * 1024,
        declared_sha256: ContentHash::from_bytes([2; 32]),
        content_type: Some("application/zip".to_owned()),
        parts: PartPlan {
            parts: vec![
                PlannedPart {
                    number: 1,
                    bytes: 5 * 1024 * 1024,
                    sha256: Some(ContentHash::from_bytes([3; 32])),
                },
                PlannedPart {
                    number: 2,
                    bytes: 1024 * 1024,
                    sha256: Some(ContentHash::from_bytes([4; 32])),
                },
            ],
        },
        completion_manifest: Vec::new(),
        completion: None,
        consumed_by: None,
        created_at: now(),
        expires_at: later(24 * 60 * 60 * 1_000),
    }
}

/// The commit a create produces, over the fixture pointer.
#[must_use]
pub fn created_commit() -> RegistryCommit {
    RegistryCommit {
        pointer: pointer(),
        consumed_upload: None,
        wrote: true,
    }
}

/// The commit a replace produces.
#[must_use]
pub fn replaced_commit() -> RegistryCommit {
    RegistryCommit {
        pointer: next_pointer(),
        consumed_upload: None,
        wrote: true,
    }
}

/// The durable receipt one commit writes.
#[must_use]
pub fn receipt_for(commit: &RegistryCommit) -> Receipt {
    let answer = SetReceipt::of(SetOutcome::Created, &commit.pointer);
    Receipt {
        scope: "registry.set:tool".to_owned(),
        key_sha256: "a".repeat(64),
        intent: aex_wire::idempotency::IntentDigest::from_bytes([7; 32]),
        response_kind: SET_RESPONSE_KIND.to_owned(),
        response: answer.to_body().expect("a serializable answer"),
        committed_at: now(),
        expires_at: later(24 * 60 * 60 * 1_000),
    }
}
