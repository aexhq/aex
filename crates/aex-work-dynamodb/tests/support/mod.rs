//! Shared fixtures for the `aex-work-dynamodb` test targets.

#![allow(dead_code, reason = "each test target uses a different subset")]
#![allow(missing_docs, reason = "the module doc states what these fixtures are")]

use aex_wire::ids::{AgentId, OrganizationId, PrefixedId, SessionId, Uuid7, WorkspaceId};
use aex_wire::types::Timestamp;
use aex_work_dynamodb::codec::{DeliveryEvidence, Payload, WorkRecord};
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::config::{BehaviorVersion, Credentials, Region};
use aws_smithy_http_client::test_util::{CaptureRequestReceiver, capture_request};

pub const TABLE: &str = "dev-eu-west-1-regional-work";

/// A `DynamoDB` client whose transport captures exactly one request.
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

/// A fixed instant.
///
/// # Panics
///
/// Never: the spelling is the pinned one.
#[must_use]
pub fn now() -> Timestamp {
    Timestamp::parse("2026-08-01T12:34:56.789Z").expect("the pinned spelling")
}

/// An instant `millis` after [`now`].
///
/// # Panics
///
/// Never: the offset stays inside the wire range.
#[must_use]
pub fn later(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(now().unix_millis() + millis).expect("in range")
}

#[must_use]
pub fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [1; 10]))
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
pub fn agent() -> AgentId {
    AgentId::from_uuid7(Uuid7::compose(1_754_051_696_789, [4; 10]))
}

#[must_use]
pub fn record() -> WorkRecord {
    WorkRecord {
        work_id: "wrk_01j0000000000000000000000".to_owned(),
        workspace: workspace(),
        organization: organization(),
        session: Some(session()),
        agent: Some(agent()),
        kind: "agent.wake".to_owned(),
        priority: 0,
        due_at: now(),
        state: "pending".to_owned(),
        attempt: 0,
        max_attempts: 5,
        fence: 0,
        claim_owner: None,
        lease_expires_at: None,
        dedupe_key: "b".repeat(64),
        payload: Payload::new().set("fromSeq", "10").set("cancelEpoch", "3"),
        delivery: DeliveryEvidence::default(),
        created_at: now(),
        updated_at: now(),
    }
}
