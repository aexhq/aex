//! Shared fixtures for the `aex-runtime-activity-dynamodb` test targets.

#![allow(dead_code, reason = "each test target uses a different subset")]
#![allow(missing_docs, reason = "the module doc states what these fixtures are")]

use aex_hands_protocol::rpc::Fence;
use aex_runtime_activity_dynamodb::codec::{
    GenerationRow, IdleProbe, LifecycleIntent, LifecycleReceipt,
};
use aex_runtime_control::generation::{GenerationState, Revision};
use aex_wire::ids::{GenerationId, OrganizationId, PrefixedId, SessionId, Uuid7, WorkspaceId};
use aex_wire::types::{ComputeSize, Timestamp};
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::config::{BehaviorVersion, Credentials, Region};
use aws_smithy_http_client::test_util::{CaptureRequestReceiver, capture_request};

pub const TABLE: &str = "dev-eu-west-1-runtime-activity";

pub const DEFINITION: &str =
    include_str!("../../../../migrations/regional/tables/runtime-activity.json");

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
pub fn organization() -> OrganizationId {
    OrganizationId::from_uuid7(Uuid7::compose(1_754_051_696_789, [2; 10]))
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
pub fn generation(byte: u8) -> GenerationId {
    GenerationId::from_uuid7(Uuid7::compose(1_754_051_696_789, [byte; 10]))
}

#[must_use]
pub fn head(state: GenerationState) -> GenerationRow {
    GenerationRow {
        session: session(),
        workspace: workspace(),
        organization: organization(),
        generation: generation(4),
        size: ComputeSize::ALL[0],
        state,
        fence: Fence(3),
        revision: Revision::new(5),
        provider_vm_id: Some("vm-0001".to_owned()),
        image_identifier: Some("hands:2026-08-01".to_owned()),
        open_operations: 0,
        last_busy_at: now(),
        idle_since: Some(now()),
        keepalive_lease_until: None,
        provider_lifetime_expires_at: Some(later(28_800_000)),
        microvm: Some(aex_runtime_control::lifecycle::MicrovmId(
            "vm-0001".to_owned(),
        )),
        lifetime: Some(aex_runtime_control::lifecycle::Lifetime { launched_at: now() }),
        accounted_from: now(),
        open_intent: None,
        suspended_at: None,
        snapshot_ordinal: 0,
        snapshot_bytes: 445_000_000,
        suspend_lock_expires_at: None,
        keepalive_lease: None,
        transport_mode: None,
        next_evaluate_at: later(180_000),
        updated_at: now(),
    }
}

#[must_use]
pub fn intent() -> LifecycleIntent {
    LifecycleIntent {
        session: session(),
        workspace: workspace(),
        generation: generation(4),
        intent_id: "int-0001".to_owned(),
        action: "suspend".to_owned(),
        requested_fence: Fence(3),
        state: "prepared".to_owned(),
        provider_request_id: None,
        requested_at: now(),
        dispatched_at: None,
    }
}

#[must_use]
pub fn receipt() -> LifecycleReceipt {
    LifecycleReceipt {
        session: session(),
        workspace: workspace(),
        generation: generation(4),
        intent_id: "int-0001".to_owned(),
        outcome: "succeeded".to_owned(),
        observed_state: "suspended".to_owned(),
        provider_error_code: None,
        active_ms: Some(120_000),
        suspended_ms: Some(0),
        snapshot_retained_byte_ms: Some(4_096),
        settled_at: later(1_000),
    }
}

#[must_use]
pub fn probe() -> IdleProbe {
    IdleProbe {
        session: session(),
        workspace: workspace(),
        generation: generation(4),
        observed_at: now(),
        open_operations: 0,
        queued_operations: 0,
        admitted_operations: 0,
        keepalive_lease_until: None,
    }
}
