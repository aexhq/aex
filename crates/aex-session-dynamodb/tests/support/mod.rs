//! Shared fixtures for the `aex-session-dynamodb` test targets.
//!
//! The request-capture client is the reason this crate asserts requests on the
//! wire rather than against a hand-written expected struct: a mirror drifts with
//! the code it is meant to check, and the serialized body is what `DynamoDB`
//! actually evaluates.

#![allow(dead_code, reason = "each test target uses a different subset")]
#![allow(missing_docs, reason = "the module doc states what these fixtures are")]

use aex_session_dynamodb::plan::RegionalTables;
use aex_session_dynamodb::wire_pending::{
    AdmissionPlan, AgentControl, AgentDecisionPlan, Body, EffectIntent, EffectStage,
    FanoutPagePlan, HeadGuard, JournalEntry, LifecyclePlan, LifecycleTransition, Message,
    PlacementGuard, ReplayIntent, Run, SessionEvent, SessionHead, SessionLifecycle, SessionStatus,
    TerminalPlan, WakeCommit, WakeIntent,
};
use aex_wire::idempotency::{IdempotencyKey, IntentDigest};
use aex_wire::ids::{
    AgentId, GenerationId, MessageId, ObservationId, OperationId, OrganizationId, PrefixedId,
    RunId, SessionId, Uuid7, WorkspaceId,
};
use aex_wire::types::Timestamp;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::config::{BehaviorVersion, Credentials, Region};
use aws_smithy_http_client::test_util::{CaptureRequestReceiver, capture_request};

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
pub fn message_id() -> MessageId {
    MessageId::from_uuid7(Uuid7::compose(1_754_051_696_789, [5; 10]))
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
pub fn head() -> SessionHead {
    SessionHead {
        session: session(),
        workspace: workspace(),
        organization: organization(),
        status: SessionStatus::Idle,
        lifecycle: SessionLifecycle::Active,
        revision: 12,
        deletion_epoch: 0,
        cancel_epoch: 3,
        content_admission_epoch: 1,
        active_run: None,
        root_agent: root_agent(),
        agent_budget: 256,
        resolved_config_digest: format!("sha256:{}", "a".repeat(64)),
        custody_revision: 2,
        created_at: now(),
        updated_at: now(),
        trashed_at: None,
        purged_at: None,
        deletion_operation: None,
    }
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
pub fn admission() -> AdmissionPlan {
    AdmissionPlan {
        placement: PlacementGuard {
            workspace: workspace(),
            key_epoch: 1,
            account_epoch: 2,
            revocation_epoch: 3,
        },
        staged_body: None,
        head: head(),
        message: Message {
            message: message_id(),
            session: session(),
            run: Some(run_id()),
            role: "user".to_owned(),
            body: Body::Inline(b"hello".to_vec()),
            content_bytes: 5,
            created_at: now(),
        },
        run: Run {
            run: run_id(),
            session: session(),
            message: message_id(),
            status: "queued",
            max_spend_cents: 500,
            reservation: "rsv-0001".to_owned(),
            deadline_at: later(600_000),
            queued_at: now(),
            started_at: None,
            terminal_at: None,
            result_digest: None,
        },
        event: event(1, "run.admitted"),
        root_control: control(),
        child_budget: 255,
        wake: wake(),
        replay: replay(),
        now: now(),
    }
}

#[must_use]
pub fn terminal() -> TerminalPlan {
    TerminalPlan {
        session: session(),
        run: run_id(),
        head_revision: 13,
        deletion_epoch: 0,
        terminal_status: "succeeded",
        result_digest: Some(format!("sha256:{}", "c".repeat(64))),
        usage_closure_id: "closure-1".to_owned(),
        root_agent: root_agent(),
        agent_revision: 5,
        agent_fence: 2,
        journal_tail: 40,
        terminal_agent_status: "succeeded".to_owned(),
        reservation: "rsv-0001".to_owned(),
        event: event(2, "run.succeeded"),
        wake: WakeCommit {
            work_id: "wrk_01j0000000000000000000000".to_owned(),
            fence: 2,
            owner: "worker-1".to_owned(),
            expires_at_epoch_seconds: 1_754_138_096,
        },
        usage_work_id: "wrk_01j0000000000000000000001".to_owned(),
        now: now(),
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
            entry_id: "je-10".to_owned(),
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

#[must_use]
pub fn lifecycle(transition: LifecycleTransition) -> LifecyclePlan {
    LifecyclePlan {
        transition,
        session: session(),
        workspace: workspace(),
        revision: 12,
        created_at: now(),
        operation: Some(operation()),
        wake: None,
        replay: None,
        now: now(),
    }
}
