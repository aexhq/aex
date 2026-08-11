//! Shared fixtures for the `aex-session-dynamodb` test targets.
//!
//! The request-capture client is the reason this crate asserts requests on the
//! wire rather than against a hand-written expected struct: a mirror drifts with
//! the code it is meant to check, and the serialized body is what `DynamoDB`
//! actually evaluates.

#![allow(dead_code, reason = "each test target uses a different subset")]
#![allow(missing_docs, reason = "the module doc states what these fixtures are")]

use aex_internal_contracts::RunId;
use aex_session_dynamodb::plan::RegionalTables;
use aex_session_dynamodb::wire_pending::{
    AgentControl, AgentDecisionPlan, Body, EffectIntent, EffectStage, FanoutPagePlan, HeadGuard,
    JournalEntry, ReplayIntent, SessionEvent, WakeIntent,
};
use aex_wire::idempotency::{IdempotencyKey, IntentDigest};
use aex_wire::ids::{
    AgentId, GenerationId, ObservationId, OperationId, OrganizationId, PrefixedId, SessionId,
    Uuid7, WorkspaceId,
};
use aex_wire::types::Timestamp;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::config::{BehaviorVersion, Credentials, Region};
use aws_smithy_http_client::test_util::{
    CaptureRequestReceiver, ReplayEvent, StaticReplayClient, capture_request,
};
use aws_smithy_types::body::SdkBody;

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

/// A client that answers a scripted response sequence and records each request.
#[must_use]
pub fn scripted_client(responses: Vec<String>) -> (Client, StaticReplayClient) {
    let events = responses
        .into_iter()
        .map(|response| {
            ReplayEvent::new(
                http::Request::builder()
                    .method("POST")
                    .uri("https://dynamodb.eu-west-1.amazonaws.com/")
                    .body(SdkBody::empty())
                    .expect("a request"),
                http::Response::builder()
                    .status(200)
                    .body(SdkBody::from(response))
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

/// The pinned physical table names.
#[must_use]
pub fn tables() -> RegionalTables {
    RegionalTables::composed("dev", "eu-west-1")
}

/// A fixed instant, so every snapshot is byte-stable.
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
pub fn session() -> SessionId {
    SessionId::from_uuid7(Uuid7::compose(1_754_051_696_789, [3; 10]))
}

#[must_use]
pub fn run_id() -> RunId {
    RunId::from_uuid7(Uuid7::compose(1_754_051_696_789, [4; 10]))
}

#[must_use]
pub fn root_agent() -> AgentId {
    AgentId::from_uuid7(Uuid7::compose(1_754_051_696_789, [6; 10]))
}

#[must_use]
pub fn generation() -> GenerationId {
    GenerationId::from_uuid7(Uuid7::compose(1_754_051_696_789, [8; 10]))
}

#[must_use]
pub fn child_agent(byte: u8) -> AgentId {
    AgentId::from_uuid7(Uuid7::compose(1_754_051_696_790, [byte; 10]))
}

#[must_use]
pub fn operation() -> OperationId {
    OperationId::from_uuid7(Uuid7::compose(1_754_051_696_789, [7; 10]))
}

#[must_use]
pub fn control() -> AgentControl {
    AgentControl {
        agent: root_agent(),
        session: session(),
        workspace: workspace(),
        generation: generation(),
        revision: 4,
        journal_tail: 9,
        journal_tail_hash: Some("a".repeat(64)),
        claim_owner: Some("worker-1".to_owned()),
        lease_expires_at: Some(later(30_000)),
        fence: 2,
        child_budget_remaining: 255,
        child_budget_granted: 255,
        status: "running".to_owned(),
        created_at: now(),
        updated_at: now(),
    }
}

#[must_use]
pub fn event(seq: u64, kind: &str) -> SessionEvent {
    SessionEvent {
        workspace: workspace(),
        event_seq: seq,
        event_id: ObservationId::from_uuid7(Uuid7::compose(1, [5; 10])),
        event_type: kind.to_owned(),
        run: Some(run_id()),
        agent: None,
        body: Body::Inline(b"{}".to_vec()),
        occurred_at: now(),
        outbox_state: "pending",
    }
}

#[must_use]
pub fn wake() -> WakeIntent {
    WakeIntent {
        work_id: "wrk_01j0000000000000000000000".to_owned(),
        kind: "agent.wake".to_owned(),
        dedupe_key_sha256: "b".repeat(64),
        due_at: now(),
        priority: 0,
    }
}

#[must_use]
pub fn replay() -> ReplayIntent {
    ReplayIntent {
        workspace: workspace(),
        scope: "session.message:".to_owned() + &session().to_string(),
        key: IdempotencyKey::parse("caller-chosen-key").expect("a key"),
        intent: IntentDigest::from_bytes([0xab; 32]),
        response_kind: "run".to_owned(),
        response: b"{\"runId\":\"run_x\"}".to_vec(),
        expires_at: later(86_400_000),
    }
}

#[must_use]
pub fn decision() -> AgentDecisionPlan {
    AgentDecisionPlan {
        session: session(),
        agent: root_agent(),
        head_guard: HeadGuard {
            cancel_epoch: 3,
            deletion_epoch: 0,
        },
        control: control(),
        next_status: "running".to_owned(),
        lease_expires_at: later(30_000),
        entry: JournalEntry {
            seq: 10,
            entry_id: "b".repeat(64),
            kind: "tool_call".to_owned(),
            body: Body::Inline(b"{}".to_vec()),
            body_bytes: 2,
            occurred_at: now(),
        },
        effect: Some(EffectIntent {
            effect_id: "eff-1".to_owned(),
            kind: "provider_call".to_owned(),
            request_hash: "d".repeat(64),
            attempt: 1,
            stage: EffectStage::Prepare,
        }),
        event: Some(event(3, "agent.preview")),
        wake: Some(wake()),
        now: now(),
    }
}

#[must_use]
pub fn fanout(children: usize) -> FanoutPagePlan {
    FanoutPagePlan {
        session: session(),
        parent: root_agent(),
        parent_revision: 4,
        intent_id: "fan-1".to_owned(),
        page: 0,
        children: (0..children)
            .map(|index| aex_session_dynamodb::wire_pending::ChildAgent {
                agent: child_agent(u8::try_from(index).unwrap_or(u8::MAX)),
                generation: generation(),
                child_budget: 4,
                wake: wake(),
            })
            .collect(),
        now: now(),
    }
}
