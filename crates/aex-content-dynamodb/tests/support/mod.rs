//! Shared fixtures for the `aex-content-dynamodb` test targets.

#![allow(dead_code, reason = "each test target uses a different subset")]
#![allow(missing_docs, reason = "the module doc states what these fixtures are")]

use aex_content_dynamodb::codec::{ContentDescriptor, DownloadGrant, GcEpoch, ObjectLocation};
use aex_content_dynamodb::wire_pending::InlineBody;
use aex_session_dynamodb::measure;
use aex_wire::ids::{
    ContentHash, MeasurementId, OrganizationId, PrefixedId, SessionId, Uuid7, WorkspaceId,
};
use aex_wire::types::Timestamp;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::config::{BehaviorVersion, Credentials, Region};
use aws_smithy_http_client::test_util::{
    CaptureRequestReceiver, ReplayEvent, StaticReplayClient, capture_request,
};
use aws_smithy_types::body::SdkBody;

/// The pinned physical table name.
pub const TABLE: &str = "dev-eu-west-1-regional-content";

/// The checked-in generation definition this crate's vocabulary must match.
pub const DEFINITION: &str =
    include_str!("../../../../migrations/regional/tables/regional-content.json");

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

/// One scripted answer: a status code and a `DynamoDB` JSON body.
pub struct Answer {
    pub status: u16,
    pub body: String,
}

/// A client that answers a scripted response sequence and records each request.
#[must_use]
pub fn scripted_client(answers: Vec<Answer>) -> (Client, StaticReplayClient) {
    let events = answers
        .into_iter()
        .map(|answer| {
            ReplayEvent::new(
                http::Request::builder()
                    .method("POST")
                    .uri("https://dynamodb.eu-west-1.amazonaws.com/")
                    .body(SdkBody::empty())
                    .expect("a request"),
                http::Response::builder()
                    .status(answer.status)
                    .body(SdkBody::from(answer.body))
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
        .retry_config(aws_sdk_dynamodb::config::retry::RetryConfig::disabled())
        .build();
    (Client::from_conf(config), replay)
}

/// The captured request body, parsed as JSON.
///
/// # Panics
///
/// If no request was captured or the body is not JSON, both of which mean the
/// adapter did not send what the test believes it sent.
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
pub fn organization() -> OrganizationId {
    OrganizationId::from_uuid7(Uuid7::compose(1_754_051_696_789, [2; 10]))
}

#[must_use]
pub fn session() -> SessionId {
    SessionId::from_uuid7(Uuid7::compose(1_754_051_696_789, [3; 10]))
}

#[must_use]
pub fn measurement() -> MeasurementId {
    MeasurementId::from_uuid7(Uuid7::compose(1_754_051_696_789, [4; 10]))
}

#[must_use]
pub fn digest(byte: u8) -> ContentHash {
    ContentHash::from_bytes([byte; 32])
}

#[must_use]
pub fn sealed(bytes: usize) -> InlineBody {
    InlineBody {
        ciphertext: vec![0x5a; bytes],
        enc_context_digest: "c".repeat(64),
    }
}

#[must_use]
pub fn descriptor() -> ContentDescriptor {
    ContentDescriptor {
        workspace: workspace(),
        organization: organization(),
        digest: digest(0xab),
        size_bytes: 4_096,
        media_type: "application/json".to_owned(),
        placement: measure::Placement::Inline,
        state: "staged".to_owned(),
        gc_epoch: 3,
        created_at: now(),
        verified_at: None,
        object: None,
    }
}

#[must_use]
pub fn object_descriptor() -> ContentDescriptor {
    ContentDescriptor {
        placement: measure::Placement::ObjectStore,
        state: "committed".to_owned(),
        object: Some(ObjectLocation {
            key: "ws/ab/cd/abcd".to_owned(),
            etag: "\"d41d8cd98f00b204e9800998ecf8427e\"".to_owned(),
            checksum_sha256: "qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqs=".to_owned(),
            checksum_crc64_nvme: "AAAAAAAAAAA=".to_owned(),
            part_count: Some(4),
            kms_key_id: "arn:aws:kms:eu-west-1:000000000000:key/content".to_owned(),
        }),
        ..descriptor()
    }
}

#[must_use]
pub fn grant() -> DownloadGrant {
    DownloadGrant {
        token_sha256: "b".repeat(64),
        workspace: workspace(),
        digest: digest(0xab),
        range_start: 0,
        range_end_exclusive: 4_096,
        authorized_bytes: 4_096,
        measurement: measurement(),
        media_type: "application/json".to_owned(),
        expires_at: later(300_000),
    }
}

#[must_use]
pub fn gc_epoch() -> GcEpoch {
    GcEpoch {
        workspace: workspace(),
        epoch: 4,
        state: "sweeping".to_owned(),
        mark_started_at: Some(now()),
        mark_bucket_cursor: Some(255),
        sweep_cursor: None,
        revision: 11,
    }
}
