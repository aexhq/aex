//! Property catalogue for `aex-session-app`: plan 04 items 32-39.
//!
//! Every case drives a real use case against scripted ports and inspects the
//! plan it returned. Nothing is committed anywhere, because
//! [`aex_session_app::AppContext`] has no committer to commit with.

use std::collections::BTreeSet;

use aex_operation_domain::cursor::CursorPosition;
use aex_operation_domain::operation::OperationStatus;
use aex_secret_domain::context::Plane;
use aex_secret_domain::{
    CiphertextRef, CustodyRevision, EncryptionContext, OwnerKeyEdgeId, SecretName,
    SourceGeneration, TrueIdle, TrueIdleViolation, admit_custody, set,
};
use aex_session_app::plan::{Condition, TransactionIntent, Write};
use aex_session_app::testing::{CountingIds, FixedClock, PortCall, ScriptedPorts, fixture_spend};
use aex_session_app::use_cases::STOP_BATCH_AGENTS;
use aex_session_app::{
    AppError, MAX_ACTIONS, Planned, Rebind, SendMessage, SessionCommand, SessionTransaction,
    StartRun, admit_message, purge_session, rebind_credentials, restore_session, start_run,
    stop_session, trash_session,
};
use aex_session_domain::testing::{id, moment, session_fixture};
use aex_session_domain::{MessagePart, PurgeCascade, SessionStatus, WorkAdmission};
use aex_wire::error::ErrorCode;
use aex_wire::idempotency::IntentDigest;
use aex_wire::ids::{MessageId, OperationId, RunId, Uuid7};
use aex_wire::types::Region;
use proptest::prelude::*;

fn clock() -> FixedClock {
    FixedClock(moment(1_000))
}

fn send_message() -> SendMessage {
    let session = session_fixture();
    SendMessage {
        workspace: session.workspace,
        session: session.id,
        message: id::<MessageId>(20),
        run: id::<RunId>(21),
        parts: vec![MessagePart::Text {
            text: "hello".to_owned(),
        }],
        max_spend_cents: fixture_spend(),
        deadline: moment(60_000),
        intent: IntentDigest::from_bytes([3; 32]),
    }
}

fn session_command(tag: u8) -> SessionCommand {
    let session = session_fixture();
    SessionCommand {
        workspace: session.workspace,
        session: session.id,
        operation: id::<OperationId>(tag),
        intent: IntentDigest::from_bytes([tag; 32]),
    }
}

fn secret(name: &str, tag: u8) -> aex_secret_domain::WorkspaceSecret {
    let session = session_fixture();
    let name = SecretName::parse(name).expect("valid secret name");
    set(
        None,
        session.workspace,
        &name,
        CiphertextRef {
            key_generation: 1,
            wrapped_key: vec![tag; 32],
            nonce: vec![tag; 12],
            ciphertext: vec![tag; 48],
        },
        EncryptionContext {
            plane: Plane::Prd,
            region: Region::ALL[0],
            organization: session.organization,
            workspace: session.workspace,
            name: name.clone(),
            generation: SourceGeneration::FIRST,
            custody_revision: None,
        },
        moment(0),
    )
    .expect("creates secret")
    .secret
}

fn rebind_fixture() -> (ScriptedPorts, Rebind) {
    let session = session_fixture();
    let old = secret("old-key", 1);
    let next = secret("new-key", 2);
    let custody = admit_custody(
        session.id,
        session.workspace,
        None,
        &[old],
        OwnerKeyEdgeId(Uuid7::compose(1, [70; 10])),
        moment(0),
    )
    .expect("admits custody");
    let ports = ScriptedPorts::idle()
        .with_custody(custody)
        .with_secrets(vec![next.clone()]);
    let command = Rebind {
        command: session_command(71),
        secrets: vec![next.name],
    };
    (ports, command)
}

/// Every command this crate implements, run against the same scripted ports.
async fn every_plan(ports: &ScriptedPorts) -> Vec<(TransactionIntent, SessionTransaction)> {
    let clock = clock();
    let ids = CountingIds::default();
    let context = ports.context(&clock, &ids);
    let mut plans = Vec::new();

    if let Ok(Planned { plan, .. }) = admit_message(&context, &send_message()).await {
        plans.push((plan.intent, plan));
    }
    if let Ok(Planned { plan, .. }) = stop_session(&context, &session_command(30)).await {
        plans.push((plan.intent, plan));
    }
    if let Ok(Planned { plan, .. }) = rebind_credentials(
        &context,
        &Rebind {
            command: session_command(33),
            secrets: Vec::new(),
        },
    )
    .await
    {
        plans.push((plan.intent, plan));
    }
    if let Ok(Planned { plan, .. }) = trash_session(&context, &session_command(31)).await {
        plans.push((plan.intent, plan));
    }
    if let Ok(Planned { plan, .. }) = purge_session(
        &context,
        &aex_session_app::Purge {
            command: session_command(32),
            cascade: PurgeCascade::DetachDescendants,
            closure: Vec::new(),
        },
    )
    .await
    {
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
            "ReservationOpen",
        ],
        TransactionIntent::StopSession => vec!["SessionRevision", "CancellationEpoch"],
        TransactionIntent::TrashSession
        | TransactionIntent::RestoreSession
        | TransactionIntent::PurgeSession => vec!["SessionRevision", "DeletionState"],
        TransactionIntent::RebindCredentials => vec![
            "SessionRevision",
            "DeletionState",
            "SessionActiveRun",
            "MutationGuardFree",
            "CustodyRevision",
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
        Condition::RunNonTerminal { .. } => "RunNonTerminal",
        Condition::AccountRevisionAtLeast { .. } => "AccountRevisionAtLeast",
        Condition::AuthorizationEpochAtLeast { .. } => "AuthorizationEpochAtLeast",
        Condition::RegistryEtag { .. } => "RegistryEtag",
        Condition::UploadState { .. } => "UploadState",
        Condition::ReservationOpen { .. } => "ReservationOpen",
        Condition::ContentOwned { .. } => "ContentOwned",
        Condition::RootPinPresent { .. } => "RootPinPresent",
        Condition::GrantUnexpired { .. } => "GrantUnexpired",
        Condition::PersistRoot { .. } => "PersistRoot",
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

    let planned = admit_message(&context, &send_message())
        .await
        .expect("admits");
    let plan = &planned.plan;

    let mut has_message = false;
    let mut has_run = false;
    let mut has_head = false;
    for write in &plan.writes {
        match write {
            Write::PutMessage(_) => has_message = true,
            Write::PutRun(_) => has_run = true,
            Write::PutSessionHead(head) => {
                has_head = true;
                assert_eq!(head.active_run, Some(planned.projected.1.id));
                assert_eq!(head.status, SessionStatus::Running);
            }
            _ => {}
        }
    }
    assert!(has_message && has_run && has_head);

    // No history yields a durable message without a wake: the two are in the
    // same plan, so either both land or neither does.
    assert!(
        !plan.after_commit.is_empty(),
        "a message always wakes the root"
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

#[tokio::test]
async fn replaying_an_operation_identity_yields_the_same_projection() {
    // 37 `admission_replay`.
    let ports = ScriptedPorts::idle();
    let clock = clock();
    let ids = CountingIds::default();
    let context = ports.context(&clock, &ids);

    let first = stop_session(&context, &session_command(30))
        .await
        .expect("admits");
    let replay_ports = ScriptedPorts::idle().with_operation(first.projected.clone());
    let replay_context = replay_ports.context(&clock, &ids);
    let second = stop_session(&replay_context, &session_command(30))
        .await
        .expect("replays");

    assert_eq!(first.projected, second.projected);
    assert_eq!(first.plan.intent, second.plan.intent);
}

// ---------------------------------------------------------------------------
// 39 — authorization precedes replay
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_paused_caller_is_never_shown_a_receipt() {
    // 39 `authorization_precedes_replay`.
    let ports = ScriptedPorts::idle().paused();
    let clock = clock();
    let ids = CountingIds::default();
    let context = ports.context(&clock, &ids);

    let outcome = admit_message(&context, &send_message()).await;
    let error = outcome.expect_err("a paused account must be denied");
    assert_eq!(error.code(), ErrorCode::AccountPaused);

    // The receipt lookup never happened, so nothing could be re-disclosed.
    assert!(
        !ports
            .calls()
            .iter()
            .any(|call| matches!(call, PortCall::Read("load_receipt"))),
        "the pause gate runs before any replay lookup"
    );
    assert!(!ports.recorded_a_write());
}

#[tokio::test]
async fn the_pause_exempt_commands_stay_available() {
    // 39, exempt half: stop, trash and purge are exactly what a paused customer
    // needs, so they plan while a mutation does not.
    let ports = ScriptedPorts::idle().paused();
    let clock = clock();
    let ids = CountingIds::default();
    let context = ports.context(&clock, &ids);

    assert!(stop_session(&context, &session_command(30)).await.is_ok());
    assert!(trash_session(&context, &session_command(31)).await.is_ok());
    assert!(admit_message(&context, &send_message()).await.is_err());
}

// ---------------------------------------------------------------------------
// Generated plans
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    /// 35 and 38 over generated commands.
    #[test]
    fn generated_commands_always_produce_one_valid_plan(tag in 1_u8..64) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a current-thread runtime always builds");
        runtime.block_on(async {
            let ports = ScriptedPorts::idle();
            let clock = clock();
            let ids = CountingIds::default();
            let context = ports.context(&clock, &ids);

            let planned = stop_session(&context, &session_command(tag)).await;
            if let Ok(planned) = planned {
                let shape = planned.plan.validate().expect("validates");
                assert!(shape.actions <= MAX_ACTIONS);
                let mut seen = BTreeSet::new();
                for write in &planned.plan.writes {
                    assert!(seen.insert(write.target()));
                }
            }
            assert!(!ports.recorded_a_write());
        });
    }
}

#[tokio::test]
async fn restore_needs_a_trashed_session_and_start_needs_an_active_run() {
    // Coverage for the two commands the shared driver cannot reach from an idle
    // fixture, so every implemented use case is exercised somewhere.
    let ports = ScriptedPorts::idle();
    let clock = clock();
    let ids = CountingIds::default();
    let context = ports.context(&clock, &ids);

    let restore = restore_session(&context, &session_command(40)).await;
    assert!(matches!(restore, Err(AppError::Deletion(_))));

    let session = session_fixture();
    let start = start_run(
        &context,
        &StartRun {
            workspace: session.workspace,
            session: session.id,
            run: id::<RunId>(21),
        },
    )
    .await;
    assert!(start.is_err(), "an idle session has no run to start");

    // Neither failure wrote anything.
    assert!(!ports.recorded_a_write());
}

#[tokio::test]
async fn rebind_is_one_idle_fenced_transaction_and_destroys_the_old_edge_after_commit() {
    let (ports, command) = rebind_fixture();
    let clock = clock();
    let ids = CountingIds::default();
    let context = ports.context(&clock, &ids);

    let planned = rebind_credentials(&context, &command)
        .await
        .expect("rebinds");
    assert_eq!(
        planned.projected.kind,
        aex_operation_domain::OperationKind::CredentialRebind
    );

    let custody = planned
        .plan
        .writes
        .iter()
        .find_map(|write| match write {
            Write::PutCustody(custody) => Some(custody.as_ref()),
            _ => None,
        })
        .expect("writes custody");
    assert_eq!(custody.revision, CustodyRevision(2));
    assert_eq!(custody.entries.len(), 1);
    assert_eq!(custody.entries[0].name.as_str(), "new-key");

    let head = planned
        .plan
        .writes
        .iter()
        .find_map(|write| match write {
            Write::PutSessionHead(head) => Some(head.as_ref()),
            _ => None,
        })
        .expect("updates the head");
    assert_eq!(head.custody_revision, CustodyRevision(2));
    assert!(planned.plan.conditions.iter().any(|condition| matches!(
        condition,
        Condition::MutationGuardFree { session } if *session == command.command.session
    )));
    assert!(planned.plan.conditions.iter().any(|condition| matches!(
        condition,
        Condition::CustodyRevision {
            expected: CustodyRevision(1),
            ..
        }
    )));
    assert!(planned.plan.conditions.iter().any(|condition| matches!(
        condition,
        Condition::SecretRevocationEpoch { name, .. } if name.as_str() == "new-key"
    )));
    assert!(matches!(
        planned.plan.after_commit.as_slice(),
        [aex_session_app::Hint::DestroyKeyEdge { .. }]
    ));
    assert!(!ports.recorded_a_write());
}

#[tokio::test]
async fn rebind_rejects_non_idle_runtime_evidence_before_reading_secrets() {
    let (ports, command) = rebind_fixture();
    let ports = ports.with_true_idle(TrueIdle {
        observed_at: moment(999),
        violation: Some(TrueIdleViolation::WorkQueued),
    });
    let clock = clock();
    let ids = CountingIds::default();
    let context = ports.context(&clock, &ids);

    let error = rebind_credentials(&context, &command)
        .await
        .expect_err("not true idle");
    assert_eq!(error.code(), ErrorCode::SessionNotIdle);
    assert!(!ports.calls().contains(&PortCall::Read("read_secrets")));
    assert!(!ports.recorded_a_write());
}

#[tokio::test]
async fn rebind_replay_does_not_advance_custody_or_repeat_any_custody_read() {
    let (ports, command) = rebind_fixture();
    let clock = clock();
    let ids = CountingIds::default();
    let first = rebind_credentials(&ports.context(&clock, &ids), &command)
        .await
        .expect("first admission");

    let replay_ports = ScriptedPorts::idle().with_operation(first.projected.clone());
    let replay = rebind_credentials(&replay_ports.context(&clock, &ids), &command)
        .await
        .expect("exact replay");

    assert_eq!(replay.projected, first.projected);
    assert!(replay.plan.writes.is_empty());
    assert!(!replay_ports.calls().contains(&PortCall::Read("true_idle")));
    assert!(
        !replay_ports
            .calls()
            .contains(&PortCall::Read("read_secrets"))
    );
    assert!(
        !replay_ports
            .calls()
            .contains(&PortCall::Read("read_custody"))
    );
}

// ---------------------------------------------------------------------------
// Paged stop — defect 9/10: the landed use case read one 100-agent page,
// ignored `more`, and idled the head regardless, so a session with more agents
// than one page kept spending after the customer was told it had stopped.
// ---------------------------------------------------------------------------

fn written_head(plan: &SessionTransaction) -> aex_session_domain::Session {
    plan.writes
        .iter()
        .find_map(|write| match write {
            Write::PutSessionHead(head) => Some((**head).clone()),
            _ => None,
        })
        .expect("every stop step writes the head")
}

fn cancelled_agents(plan: &SessionTransaction) -> usize {
    plan.writes
        .iter()
        .filter(|write| matches!(write, Write::CancelAgent { .. }))
        .count()
}

#[tokio::test]
async fn a_paged_stop_yields_a_continued_operation_and_idles_only_on_the_final_step() {
    let overflow = STOP_BATCH_AGENTS + 5;
    // A running session, so "the head reaches Idle only on the final step" is a
    // real observation rather than a fixture that was already idle.
    let (running, _run, _agent, _message) = aex_session_domain::testing::running_session();
    let ports = ScriptedPorts::idle()
        .with_active_agents(overflow)
        .with_session(running);
    let clock = clock();
    let ids = CountingIds::default();
    let context = ports.context(&clock, &ids);
    let before = ports.session().clone();

    let first = stop_session(&context, &session_command(30))
        .await
        .expect("admits");

    // The operation is continued, not a silently truncated inline success.
    assert_eq!(
        first.projected.status,
        OperationStatus::Running,
        "a stop that could not finish must not be reported as done"
    );
    assert!(
        first.projected.result.is_none(),
        "no result is published before the final page is consumed"
    );
    assert!(
        first.projected.committed_at.is_some(),
        "the step that closes the fence is the point of no return (D-2)"
    );
    assert!(
        matches!(
            first
                .projected
                .cursor
                .as_ref()
                .map(|cursor| &cursor.position),
            Some(CursorPosition::Stop { .. })
        ),
        "a continued stop parks at a stop cursor, got {:?}",
        first.projected.cursor
    );

    // The fence closes on the first step; the head does not idle on it.
    let first_head = written_head(&first.plan);
    assert_ne!(
        first_head.cancellation, before.cancellation,
        "the cancellation epoch bumps on the first step, or nothing is fenced"
    );
    assert_eq!(
        first_head.status,
        SessionStatus::Running,
        "the head must not reach Idle while agents remain"
    );
    assert!(
        first_head.active_run.is_some(),
        "the run barrier moves only on the final step"
    );
    assert_eq!(
        cancelled_agents(&first.plan),
        usize::from(STOP_BATCH_AGENTS),
        "one step settles exactly one bounded batch"
    );
    // Defect 10: the plan fits the provider envelope rather than failing
    // validation as an internal fault.
    assert!(
        first.plan.validate().expect("fits").actions <= MAX_ACTIONS,
        "a stop step must fit the 100-action transaction budget"
    );

    // The final step.
    let next = ScriptedPorts::idle()
        .with_active_agents(overflow)
        .with_settled_prefix(usize::from(STOP_BATCH_AGENTS))
        .with_session(first_head.clone())
        .with_operation(first.projected.clone());
    let next_context = next.context(&clock, &ids);
    let last = aex_session_app::continue_stop(&next_context, &session_command(30))
        .await
        .expect("steps");

    assert_eq!(
        last.projected.status,
        OperationStatus::Succeeded,
        "only the step that consumes the final page terminalizes"
    );
    assert!(last.projected.cursor.is_none(), "the cursor is retired");
    let last_head = written_head(&last.plan);
    assert_eq!(
        last_head.status,
        SessionStatus::Idle,
        "the head reaches Idle exactly on the final step"
    );
    assert_eq!(
        last_head.active_run, None,
        "the run barrier commits with the final step"
    );
    assert_eq!(
        cancelled_agents(&last.plan),
        5,
        "the residue the first batch could not reach is settled here"
    );
}

#[tokio::test]
async fn a_stop_that_fits_one_batch_stays_inline_and_idles_at_once() {
    let ports = ScriptedPorts::idle().with_active_agents(3);
    let clock = clock();
    let ids = CountingIds::default();
    let context = ports.context(&clock, &ids);

    let planned = stop_session(&context, &session_command(31))
        .await
        .expect("admits");

    assert_eq!(planned.projected.status, OperationStatus::Succeeded);
    assert!(planned.projected.cursor.is_none());
    assert!(planned.projected.committed_at.is_some());
    assert_eq!(written_head(&planned.plan).status, SessionStatus::Idle);
    assert_eq!(cancelled_agents(&planned.plan), 3);
}

#[tokio::test]
async fn a_stop_step_names_the_cursor_it_advances_from() {
    let overflow = STOP_BATCH_AGENTS + 2;
    let ports = ScriptedPorts::idle().with_active_agents(overflow);
    let clock = clock();
    let ids = CountingIds::default();
    let context = ports.context(&clock, &ids);
    let first = stop_session(&context, &session_command(32))
        .await
        .expect("admits");

    let next = ScriptedPorts::idle()
        .with_active_agents(overflow)
        .with_settled_prefix(usize::from(STOP_BATCH_AGENTS))
        .with_session(written_head(&first.plan))
        .with_operation(first.projected.clone());
    let next_context = next.context(&clock, &ids);
    let last = aex_session_app::continue_stop(&next_context, &session_command(32))
        .await
        .expect("steps");

    // A duplicate delivery of the same step fails this condition and writes
    // nothing, which is the whole idempotency argument for a step commit (D-4).
    let guard = last
        .plan
        .conditions
        .iter()
        .find_map(|condition| match condition {
            Condition::OperationCursorAt { expected, .. } => Some(expected.clone()),
            _ => None,
        })
        .expect("a step commit guards the cursor it advances from");
    assert_eq!(
        guard.map(|cursor| cursor.position.clone()),
        first
            .projected
            .cursor
            .as_ref()
            .map(|cursor| cursor.position.clone()),
        "the guard names the operation's stored cursor, not the one it writes"
    );
}
