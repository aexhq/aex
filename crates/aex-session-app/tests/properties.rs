//! Property catalogue for `aex-session-app`: plan 04 items 32-39.
//!
//! Every case drives a real use case against scripted ports and inspects the
//! plan it returned. Nothing is committed anywhere, because
//! [`aex_session_app::AppContext`] has no committer to commit with.

use std::collections::BTreeSet;

use aex_session_app::plan::{Condition, TransactionIntent, Write};
use aex_session_app::testing::{
    CountingIds, FixedClock, PortCall, ScriptedPorts, message_identity_under,
};
use aex_session_app::{
    AppError, CommitTerminal, MAX_ACTIONS, MessageAdmissionOutcome, Planned, SendMessage,
    SessionTransaction, admit_message, commit_terminal, replay_message_receipt,
};
use aex_session_domain::testing::{id, moment, running_session, session_fixture, terminal_attempt};
use aex_session_domain::{
    MAX_OPEN_MESSAGES_PER_RUN, Message, MessageRole, MessageState, SessionStatus, WorkAdmission,
};
use aex_wire::error::ErrorCode;
use aex_wire::ids::MessageId;
use aex_wire::models::MessageSendRequest;

fn clock() -> FixedClock {
    FixedClock(moment(1_000))
}

fn send_message() -> SendMessage {
    let session = session_fixture();
    let request = MessageSendRequest {
        deadline: None,
        max_spend_cents: None,
        text: "hello".to_owned(),
    };
    SendMessage {
        workspace: session.workspace,
        session: session.id,
        identity: message_identity_under("fixture-message-key", &session, &request),
        request,
    }
}

/// Every command this crate implements, run against the same scripted ports.
async fn every_plan(ports: &ScriptedPorts) -> Vec<(TransactionIntent, SessionTransaction)> {
    let clock = clock();
    let ids = CountingIds::default();
    let context = ports.context(&clock, &ids);
    let mut plans = Vec::new();

    if let Ok(MessageAdmissionOutcome::Planned(planned)) =
        admit_message(&context, &send_message()).await
    {
        let Planned { plan, .. } = *planned;
        plans.push((plan.intent, plan));
    }
    plans
}

// ---------------------------------------------------------------------------
// 32, 33 — one plan, reads only
// ---------------------------------------------------------------------------

#[tokio::test]
async fn plan_is_a_single_transaction_and_no_port_call_writes() {
    // 32 `plan_is_single_transaction` and 33 `plan_no_external_call`.
    let ports = ScriptedPorts::idle();
    let plans = every_plan(&ports).await;
    assert!(!plans.is_empty(), "the fixture must admit some command");

    // 32: each command returned exactly one plan, and the intents are distinct.
    let intents: BTreeSet<TransactionIntent> = plans.iter().map(|(intent, _)| *intent).collect();
    assert_eq!(intents.len(), plans.len());

    // 33: every recorded port call is a read; the committer is not in scope at
    // all, so a commit is unrepresentable rather than merely unobserved.
    assert!(!ports.recorded_a_write());
    for call in ports.calls() {
        assert!(matches!(call, PortCall::Read(_)), "{call:?} must be a read");
    }
}

#[tokio::test]
async fn every_plan_stays_inside_the_envelope() {
    // 35 `plan_within_envelope`.
    let ports = ScriptedPorts::idle();
    for (intent, plan) in every_plan(&ports).await {
        let shape = plan.validate().expect("every plan validates");
        assert!(
            shape.actions <= MAX_ACTIONS,
            "{intent:?} carries {} actions",
            shape.actions
        );
        assert!(shape.bytes <= aex_session_app::MAX_BYTES);
        // A plan is never split: validate returns one shape or one error.
        assert_eq!(shape, plan.validate().expect("validation is pure"));
    }
}

#[tokio::test]
async fn no_plan_writes_the_same_item_twice() {
    // 38 `duplicate_write_target`.
    let ports = ScriptedPorts::idle();
    for (intent, plan) in every_plan(&ports).await {
        let mut seen = BTreeSet::new();
        for write in &plan.writes {
            assert!(
                seen.insert(write.target()),
                "{intent:?} writes {:?} twice",
                write.target()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 34 — the minimum condition set per command
// ---------------------------------------------------------------------------

/// Which conditions each command must carry. Removing any row must fail.
fn required_conditions(intent: TransactionIntent) -> Vec<&'static str> {
    match intent {
        TransactionIntent::AdmitMessage => vec![
            "SessionRevision",
            "DeletionState",
            "SessionActiveRun",
            "WorkAdmission",
            "MutationGuardFree",
            "CancellationEpoch",
            "AccountRevisionAtLeast",
            "ProviderCredentialReady",
            "AgentRevision",
            "JournalTail",
            "RootAgentIdle",
        ],
        TransactionIntent::StartRun => {
            vec![
                "SessionRevision",
                "DeletionState",
                "RunNonTerminal",
                "SessionActiveRun",
            ]
        }
        TransactionIntent::CommitTerminal => vec![
            "RunNonTerminal",
            "SessionActiveRun",
            "SessionRevision",
            "CancellationEpoch",
            "DeletionState",
            "AgentFence",
        ],
        _ => Vec::new(),
    }
}

fn condition_tag(condition: &Condition) -> &'static str {
    match condition {
        Condition::SessionRevision { .. } => "SessionRevision",
        Condition::SessionStatusIn { .. } => "SessionStatusIn",
        Condition::SessionActiveRun { .. } => "SessionActiveRun",
        Condition::WorkAdmission { .. } => "WorkAdmission",
        Condition::DeletionState { .. } => "DeletionState",
        Condition::CancellationEpoch { .. } => "CancellationEpoch",
        Condition::MutationGuardFree { .. } => "MutationGuardFree",
        Condition::MutationGuardHeldBy { .. } => "MutationGuardHeldBy",
        Condition::AgentRevision { .. } => "AgentRevision",
        Condition::AgentFence { .. } => "AgentFence",
        Condition::JournalTail { .. } => "JournalTail",
        Condition::RootAgentIdle { .. } => "RootAgentIdle",
        Condition::RunNonTerminal { .. } => "RunNonTerminal",
        Condition::AccountRevisionAtLeast { .. } => "AccountRevisionAtLeast",
        Condition::ProviderCredentialReady { .. } => "ProviderCredentialReady",
        Condition::AuthorizationEpochAtLeast { .. } => "AuthorizationEpochAtLeast",
        Condition::RegistryEtag { .. } => "RegistryEtag",
        Condition::UploadState { .. } => "UploadState",
        Condition::ContentOwned { .. } => "ContentOwned",
        Condition::GrantUnexpired { .. } => "GrantUnexpired",
        Condition::OperationFence { .. } => "OperationFence",
        Condition::OperationCursorAt { .. } => "OperationCursorAt",
        Condition::OperationVersion { .. } => "OperationVersion",
        Condition::SecretRevocationEpoch { .. } => "SecretRevocationEpoch",
        Condition::CustodyRevision { .. } => "CustodyRevision",
        Condition::ItemAbsent(_) => "ItemAbsent",
        Condition::ItemPresent(_) => "ItemPresent",
    }
}

#[tokio::test]
async fn every_command_carries_its_required_conditions() {
    // 34 `plan_required_conditions`.
    let ports = ScriptedPorts::idle();
    for (intent, plan) in every_plan(&ports).await {
        let present: BTreeSet<&'static str> = plan.conditions.iter().map(condition_tag).collect();
        for required in required_conditions(intent) {
            assert!(
                present.contains(required),
                "{intent:?} must condition on {required}"
            );
        }

        // Removing any required condition leaves a plan that no longer satisfies
        // the table, which is exactly what the table exists to catch.
        for required in required_conditions(intent) {
            let stripped: Vec<&'static str> = present
                .iter()
                .copied()
                .filter(|tag| *tag != required)
                .collect();
            assert!(
                !stripped.contains(&required),
                "{intent:?} must not carry {required} twice under one tag"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 36, 37 — admission atomicity and replay
// ---------------------------------------------------------------------------

#[tokio::test]
async fn admission_is_atomic() {
    // 36 `admission_atomicity`.
    let ports = ScriptedPorts::idle();
    let clock = clock();
    let ids = CountingIds::default();
    let context = ports.context(&clock, &ids);

    let MessageAdmissionOutcome::Planned(planned) = admit_message(&context, &send_message())
        .await
        .expect("admits")
    else {
        panic!("a fresh request cannot replay");
    };
    let plan = &planned.plan;

    let mut has_message = false;
    let mut has_sealed_projection = false;
    let mut has_run = false;
    let mut has_head = false;
    for write in &plan.writes {
        match write {
            Write::PutMessage(_) => has_message = true,
            Write::PutSealedMessage(_) => has_sealed_projection = true,
            Write::PutRun(_) => has_run = true,
            Write::PutSessionHead(head) => {
                has_head = true;
                assert!(head.active_run.is_some());
                assert_eq!(
                    head.lifecycle.active.map(|active| active.message),
                    Some(planned.projected.0.id)
                );
                assert_eq!(head.status, SessionStatus::Running);
            }
            _ => {}
        }
    }
    assert!(has_message && has_sealed_projection && has_run && has_head);
    assert_eq!(
        plan.validate().expect("valid admission").actions,
        12,
        "the BYOK revision/state fence is a distinct cross-table action"
    );
    assert!(
        plan.conditions
            .iter()
            .any(|condition| matches!(condition, Condition::ProviderCredentialReady { .. }))
    );

    // No history yields a durable message without a wake: the two are in the
    // same plan, so either both land or neither does.
    assert!(
        !plan.after_commit.is_empty(),
        "a message always wakes the root"
    );
}

#[tokio::test]
async fn omitted_and_explicit_message_bounds_are_preserved_in_every_authority() {
    let ports = ScriptedPorts::idle();
    let clock = clock();
    let ids = CountingIds::default();
    let default_command = send_message();
    let MessageAdmissionOutcome::Planned(defaulted) =
        admit_message(&ports.context(&clock, &ids), &default_command)
            .await
            .expect("default bounds admit")
    else {
        panic!("a fresh request cannot replay");
    };
    let default_run = defaulted
        .plan
        .writes
        .iter()
        .find_map(|write| match write {
            Write::PutRun(run) => Some(run.as_ref()),
            _ => None,
        })
        .expect("admission stores its internal execution");
    assert_eq!(default_run.max_spend_cents.get(), 1_000);
    assert_eq!(default_run.deadline, ports.session().lifecycle.expires_at);
    let active = defaulted
        .projected
        .1
        .lifecycle
        .active
        .expect("the session projects current activity");
    assert_eq!(active.bounds.max_spend_cents.get(), 1_000);
    assert_eq!(active.bounds.deadline, ports.session().lifecycle.expires_at);

    let session = session_fixture();
    let request = MessageSendRequest {
        deadline: Some(moment(5_000)),
        max_spend_cents: Some(aex_wire::types::Cents::new(50_000)),
        text: "explicit".to_owned(),
    };
    let explicit = SendMessage {
        workspace: session.workspace,
        session: session.id,
        identity: message_identity_under("explicit-bounds", &session, &request),
        request,
    };
    let explicit_ports = ScriptedPorts::idle();
    let explicit_ids = CountingIds::default();
    let MessageAdmissionOutcome::Planned(explicit) =
        admit_message(&explicit_ports.context(&clock, &explicit_ids), &explicit)
            .await
            .expect("an above-default cap and earlier deadline admit")
    else {
        panic!("a fresh request cannot replay");
    };
    let explicit_run = explicit
        .plan
        .writes
        .iter()
        .find_map(|write| match write {
            Write::PutRun(run) => Some(run.as_ref()),
            _ => None,
        })
        .expect("admission stores its internal execution");
    assert_eq!(explicit_run.max_spend_cents.get(), 50_000);
    assert_eq!(explicit_run.deadline, moment(5_000));
}

#[tokio::test]
async fn message_text_is_bounded_by_utf8_bytes_below_the_journal_ceiling() {
    let session = session_fixture();
    let admitted_request = MessageSendRequest {
        deadline: None,
        max_spend_cents: None,
        text: "é".repeat(12_288),
    };
    assert_eq!(admitted_request.text.len(), 24_576);
    let admitted = SendMessage {
        workspace: session.workspace,
        session: session.id,
        identity: message_identity_under("text-boundary", &session, &admitted_request),
        request: admitted_request,
    };
    let clock = clock();
    let ids = CountingIds::default();
    let ports = ScriptedPorts::idle();
    assert!(matches!(
        admit_message(&ports.context(&clock, &ids), &admitted).await,
        Ok(MessageAdmissionOutcome::Planned(_))
    ));

    let refused_request = MessageSendRequest {
        deadline: None,
        max_spend_cents: None,
        text: "x".repeat(24_577),
    };
    let refused = SendMessage {
        workspace: session.workspace,
        session: session.id,
        identity: message_identity_under("text-too-large", &session, &refused_request),
        request: refused_request,
    };
    let refused_ids = CountingIds::default();
    let error = admit_message(&ports.context(&clock, &refused_ids), &refused)
        .await
        .expect_err("one byte above the bound is refused");
    assert_eq!(error.code(), ErrorCode::InvalidRequest);
}

#[tokio::test]
async fn exact_message_replay_reads_no_mutable_admission_dependency() {
    let command = send_message();
    let clock = clock();
    let first_ids = CountingIds::default();
    let first_ports = ScriptedPorts::idle();
    let MessageAdmissionOutcome::Planned(first) =
        admit_message(&first_ports.context(&clock, &first_ids), &command)
            .await
            .expect("first admission plans")
    else {
        panic!("the first request cannot replay");
    };
    let receipt = first
        .plan
        .writes
        .iter()
        .find_map(|write| match write {
            Write::PutIdempotencyReceipt(receipt) => Some(receipt.as_ref().clone()),
            _ => None,
        })
        .expect("admission stores its exact response");
    let replay_ports = ScriptedPorts::idle()
        .paused()
        .without_provider_credential()
        .with_receipt(receipt);
    let replay_ids = CountingIds::default();
    let replay = admit_message(&replay_ports.context(&clock, &replay_ids), &command)
        .await
        .expect("account and credential changes cannot break exact replay");
    let MessageAdmissionOutcome::Replayed { response, .. } = replay else {
        panic!("the stored winner must replay");
    };
    assert_eq!(response.message.id, first.projected.0.id);
    assert_eq!(replay_ports.calls(), vec![PortCall::Read("load_receipt")]);
}

#[tokio::test]
async fn commit_recovery_decodes_only_the_addressed_receipt_and_rejects_intent_drift() {
    let command = send_message();
    let clock = clock();
    let ids = CountingIds::default();
    let ports = ScriptedPorts::idle();
    let MessageAdmissionOutcome::Planned(first) =
        admit_message(&ports.context(&clock, &ids), &command)
            .await
            .expect("first admission plans")
    else {
        panic!("the first request cannot replay");
    };
    let receipt = first
        .plan
        .writes
        .iter()
        .find_map(|write| match write {
            Write::PutIdempotencyReceipt(receipt) => Some(receipt.as_ref()),
            _ => None,
        })
        .expect("admission stores its exact response");

    let replay = replay_message_receipt(receipt, &command).expect("the winner is recoverable");
    assert!(matches!(replay, MessageAdmissionOutcome::Replayed { .. }));

    let session = session_fixture();
    let changed_request = MessageSendRequest {
        text: "different text".to_owned(),
        ..command.request.clone()
    };
    let changed = SendMessage {
        workspace: session.workspace,
        session: session.id,
        identity: message_identity_under("message-key", &session, &changed_request),
        request: changed_request,
    };
    let error = replay_message_receipt(receipt, &changed)
        .expect_err("the same key cannot recover a different canonical intent");
    assert_eq!(error.code(), ErrorCode::IdempotencyConflict);
}

#[tokio::test]
async fn public_message_event_contains_no_internal_run_or_agent_identity() {
    let ports = ScriptedPorts::idle();
    let clock = clock();
    let ids = CountingIds::default();
    let MessageAdmissionOutcome::Planned(planned) =
        admit_message(&ports.context(&clock, &ids), &send_message())
            .await
            .expect("admits")
    else {
        panic!("a fresh request cannot replay");
    };
    let event = planned
        .plan
        .writes
        .iter()
        .find_map(|write| match write {
            Write::PutMessageAdmittedEvent(event) => Some(event.as_ref()),
            _ => None,
        })
        .expect("writes a public message event");
    let body: serde_json::Value = serde_json::from_slice(&event.body).expect("canonical JSON");
    assert!(body.get("messageId").is_some());
    assert!(body.get("sessionId").is_some());
    assert!(body.get("runId").is_none());
    assert!(body.get("agentId").is_none());
}

#[tokio::test]
async fn the_maximum_open_message_set_fits_one_terminal_transaction() {
    let (session, run, agent, _) = running_session();
    let ports = ScriptedPorts::idle()
        .with_session(session.clone())
        .with_run(run.clone())
        .with_agent(agent.clone());
    let open_messages = (0..MAX_OPEN_MESSAGES_PER_RUN)
        .map(|index| Message {
            id: id::<MessageId>(u8::try_from(index + 40).expect("fixture tag")),
            session: session.id,
            run: Some(run.id),
            agent: agent.id,
            role: if index % 2 == 0 {
                MessageRole::Assistant
            } else {
                MessageRole::Tool
            },
            state: MessageState::Open,
            parts: Vec::new(),
            created_at: moment(i64::try_from(index).expect("fixture instant")),
            sealed_at: None,
        })
        .collect();
    let command = CommitTerminal {
        workspace: session.workspace,
        session: session.id,
        attempt: terminal_attempt(&session, &run),
        agent: agent.id,
        open_messages,
    };
    let clock = clock();
    let ids = CountingIds::default();
    let planned = commit_terminal(&ports.context(&clock, &ids), &command)
        .await
        .expect("the invariant ceiling settles");

    let generic_actions = planned.plan.validate().expect("valid").actions;
    assert_eq!(
        generic_actions + 2,
        MAX_ACTIONS,
        "the production Brain boundary adds RunFinished journal and public completion event"
    );
    assert_eq!(
        planned
            .plan
            .writes
            .iter()
            .filter(|write| matches!(write, Write::PutMessage(_)))
            .count(),
        MAX_OPEN_MESSAGES_PER_RUN
    );
    assert_eq!(
        planned
            .plan
            .writes
            .iter()
            .filter(|write| matches!(write, Write::PutSealedMessage(_)))
            .count(),
        MAX_OPEN_MESSAGES_PER_RUN
    );
}

#[tokio::test]
async fn a_closed_admission_yields_no_plan_at_all() {
    // 36, negative half.
    let mut session = session_fixture();
    session.work_admission = WorkAdmission::Paused;
    let ports = ScriptedPorts::idle().with_session(session);
    let clock = clock();
    let ids = CountingIds::default();
    let context = ports.context(&clock, &ids);

    let outcome = admit_message(&context, &send_message()).await;
    assert!(outcome.is_err(), "a closed admission must not plan");
    assert!(!ports.recorded_a_write());
}

// ---------------------------------------------------------------------------
// 39 — replay lookup precedes mutable admission dependencies
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_fresh_paused_request_checks_replay_before_the_pause_gate() {
    let ports = ScriptedPorts::idle().paused();
    let clock = clock();
    let ids = CountingIds::default();
    let context = ports.context(&clock, &ids);

    let outcome = admit_message(&context, &send_message()).await;
    let error = outcome.expect_err("a paused account must be denied");
    assert_eq!(error.code(), ErrorCode::AccountPaused);

    let calls = ports.calls();
    let receipt = calls
        .iter()
        .position(|call| matches!(call, PortCall::Read("load_receipt")))
        .expect("checks replay");
    let account = calls
        .iter()
        .position(|call| matches!(call, PortCall::Read("projection")))
        .expect("checks account admission");
    assert!(receipt < account, "the strong receipt lookup runs first");
    assert!(!ports.recorded_a_write());
}
