//! The continuation property suite.
//!
//! `tests/properties.rs` proves that a paged stop *pages*: it yields a
//! `Continued` operation, it idles only on the final step, and its step commit
//! names the cursor it advances from. This file proves the other half — that a
//! step commit is a **safe** write against a row somebody else also writes.
//!
//! The operation row has two writers. The continuation advances it, and the
//! already-served public cancellation advances it under its own optimistic loop
//! over `version`. Before the plan could name a version, an adapter compiling a
//! resumed step had to invent one, and there is no honest invention: reusing
//! the first version resets a monotone counter, and reading a fresh one races
//! the very writer the counter exists to order. So the properties here are
//! about a fact the plan must carry, not about a value it computes.

use aex_operation_domain::cursor::CursorPosition;
use aex_operation_domain::operation::{OperationStatus, OperationVersion};
use aex_session_app::plan::{Condition, SessionTransaction, TransactionIntent, Write};
use aex_session_app::testing::{CountingIds, FixedClock, ScriptedPorts};
use aex_session_app::use_cases::{
    STOP_BATCH_AGENTS, SessionCommand, continue_operation, continue_stop, stop_session,
};
use aex_wire::idempotency::IntentDigest;

fn clock() -> FixedClock {
    FixedClock(aex_session_domain::testing::moment(1_000))
}

fn command(tag: u8) -> SessionCommand {
    // The scripted head and the command must name the same session, or the
    // plan's guards and its head write land on two different items and the
    // action count is one higher than the batch that produced it.
    let session = aex_session_domain::testing::session_fixture();
    SessionCommand {
        workspace: session.workspace,
        session: session.id,
        operation: aex_session_domain::testing::id(tag),
        intent: IntentDigest::from_bytes([tag; 32]),
    }
}

fn written_head(plan: &SessionTransaction) -> aex_session_domain::Session {
    plan.writes
        .iter()
        .find_map(|write| match write {
            Write::PutSessionHead(head) => Some((**head).clone()),
            _ => None,
        })
        .expect("every stop step writes the head")
}

fn version_guard(plan: &SessionTransaction) -> Option<OperationVersion> {
    plan.conditions.iter().find_map(|condition| match condition {
        Condition::OperationVersion { expected, .. } => Some(*expected),
        _ => None,
    })
}

fn cursor_guard(plan: &SessionTransaction) -> Option<Option<CursorPosition>> {
    plan.conditions.iter().find_map(|condition| match condition {
        Condition::OperationCursorAt { expected, .. } => {
            Some(expected.as_ref().map(|cursor| cursor.position.clone()))
        }
        _ => None,
    })
}

/// Drives one paged stop to its first step and returns the admission.
async fn first_step(
    tag: u8,
    overflow: u16,
) -> (
    aex_session_app::plan::Planned<aex_operation_domain::Operation>,
    FixedClock,
    CountingIds,
) {
    let (running, _run, _agent, _message) = aex_session_domain::testing::running_session();
    let ports = ScriptedPorts::idle()
        .with_active_agents(overflow)
        .with_session(running);
    let clock = clock();
    let ids = CountingIds::default();
    let planned = {
        let context = ports.context(&clock, &ids);
        stop_session(&context, &command(tag)).await.expect("admits")
    };
    (planned, clock, ids)
}

#[tokio::test]
async fn an_admission_never_names_a_version_it_could_not_have_observed() {
    // The row does not exist yet, so there is no version to have read. A plan
    // that named one would be asserting a fact about an absent item, and the
    // adapter refuses exactly that rather than quietly treating the write as an
    // update.
    let (first, _clock, _ids) = first_step(40, STOP_BATCH_AGENTS + 3).await;

    assert_eq!(
        version_guard(&first.plan),
        None,
        "an admission is an insert and has no observed version"
    );
    assert_eq!(
        cursor_guard(&first.plan),
        Some(None),
        "an admission guards the operation item as absent"
    );
    assert_eq!(first.plan.intent, TransactionIntent::StopSession);
}

#[tokio::test]
async fn a_resumed_step_names_the_exact_version_it_read() {
    let overflow = STOP_BATCH_AGENTS + 3;
    let (first, clock, ids) = first_step(41, overflow).await;

    // A row that has already been written more than once: the admission plus
    // whatever else touched it. Hard-coding the first version here would make
    // the property pass against an adapter that ignores the guard entirely.
    let stored_at = OperationVersion(7);
    let next = ScriptedPorts::idle()
        .with_active_agents(overflow)
        .with_settled_prefix(usize::from(STOP_BATCH_AGENTS))
        .with_session(written_head(&first.plan))
        .with_operation_at(first.projected.clone(), stored_at);
    let context = next.context(&clock, &ids);
    let step = continue_stop(&context, &command(41)).await.expect("steps");

    assert_eq!(
        version_guard(&step.plan),
        Some(stored_at),
        "a step conditions on the version it observed, never on a constant"
    );
    assert_eq!(
        cursor_guard(&step.plan),
        Some(
            first
                .projected
                .cursor
                .as_ref()
                .map(|cursor| cursor.position.clone())
        ),
        "the same step also names the cursor it advances from"
    );
}

#[tokio::test]
async fn the_cursor_and_version_guards_share_one_physical_action_with_the_write() {
    // Both guards and the operation write target the same item, so naming the
    // version costs nothing out of the 100-action budget. If it ever stopped
    // being free, a stop batch would have to shrink to pay for it.
    let overflow = STOP_BATCH_AGENTS + 3;
    let (first, clock, ids) = first_step(42, overflow).await;
    let next = ScriptedPorts::idle()
        .with_active_agents(overflow)
        .with_settled_prefix(usize::from(STOP_BATCH_AGENTS))
        .with_session(written_head(&first.plan))
        .with_operation_at(first.projected.clone(), OperationVersion(4));
    let context = next.context(&clock, &ids);
    let step = continue_stop(&context, &command(42)).await.expect("steps");

    let cursor_target = step
        .plan
        .conditions
        .iter()
        .find(|condition| matches!(condition, Condition::OperationCursorAt { .. }))
        .expect("a cursor guard")
        .target();
    let version_target = step
        .plan
        .conditions
        .iter()
        .find(|condition| matches!(condition, Condition::OperationVersion { .. }))
        .expect("a version guard")
        .target();
    let write_target = step
        .plan
        .writes
        .iter()
        .find(|write| matches!(write, Write::PutOperation(_)))
        .expect("a step writes the operation")
        .target();

    assert_eq!(cursor_target, version_target);
    assert_eq!(cursor_target, write_target);

    // The whole step is the head, the operation and one settled agent each. If
    // the version guard ever cost an action of its own, the stop batch would
    // have to shrink to pay for it.
    let settled = step
        .plan
        .writes
        .iter()
        .filter(|write| matches!(write, Write::CancelAgent { .. }))
        .count();
    assert_eq!(
        step.plan.validate().expect("fits").actions,
        settled + 2,
        "the version guard is free: it merges into the action the operation write already needs"
    );
}

#[tokio::test]
async fn progress_is_monotone_across_steps_and_no_step_reports_a_total_it_has_not_reached() {
    let overflow = STOP_BATCH_AGENTS + 6;
    let (first, clock, ids) = first_step(43, overflow).await;

    let carried = first
        .projected
        .progress
        .as_ref()
        .expect("a continued step reports progress")
        .clone();
    assert_eq!(carried.processed, u64::from(STOP_BATCH_AGENTS));
    assert_eq!(
        carried.total_hint, None,
        "a mid-cursor step does not know the total and must not invent one"
    );
    assert_eq!(
        first.projected.status,
        OperationStatus::Running,
        "a mid-cursor operation projects Running, never a partial success"
    );

    let next = ScriptedPorts::idle()
        .with_active_agents(overflow)
        .with_settled_prefix(usize::from(STOP_BATCH_AGENTS))
        .with_session(written_head(&first.plan))
        .with_operation_at(first.projected.clone(), OperationVersion(3));
    let context = next.context(&clock, &ids);
    let last = continue_stop(&context, &command(43)).await.expect("steps");

    let closed = last
        .projected
        .progress
        .as_ref()
        .expect("the final step closes progress");
    carried
        .check_successor(closed)
        .expect("progress never regresses across a step");
    assert_eq!(closed.processed, u64::from(overflow));
    assert_eq!(
        closed.total_hint,
        Some(u64::from(overflow)),
        "the total is published only once it is a fact"
    );
    assert_eq!(last.projected.status, OperationStatus::Succeeded);
    assert!(
        last.projected.cursor.is_none(),
        "the final step retires the cursor"
    );
}

#[tokio::test]
async fn a_step_against_a_terminal_operation_is_refused_rather_than_replayed() {
    // The duplicate-delivery defence is the cursor guard, but a delivery that
    // arrives after the operation finished has no cursor left to guard. It must
    // fail in the use case rather than build a plan that writes over a
    // succeeded envelope.
    let overflow = STOP_BATCH_AGENTS + 2;
    let (first, clock, ids) = first_step(44, overflow).await;
    let mut finished = first.projected.clone();
    finished.status = OperationStatus::Succeeded;
    finished.cursor = None;

    let next = ScriptedPorts::idle()
        .with_active_agents(overflow)
        .with_settled_prefix(usize::from(STOP_BATCH_AGENTS))
        .with_session(written_head(&first.plan))
        .with_operation_at(finished, OperationVersion(2));
    let context = next.context(&clock, &ids);

    let outcome = continue_stop(&context, &command(44)).await;
    assert!(
        outcome.is_err(),
        "a step against a terminal operation must refuse, not rewrite it"
    );
}

#[tokio::test]
async fn a_continued_operation_that_lost_its_cursor_is_corrupt_and_never_guessed_at() {
    let overflow = STOP_BATCH_AGENTS + 2;
    let (first, clock, ids) = first_step(45, overflow).await;
    let mut cursorless = first.projected.clone();
    cursorless.cursor = None;

    let next = ScriptedPorts::idle()
        .with_active_agents(overflow)
        .with_settled_prefix(usize::from(STOP_BATCH_AGENTS))
        .with_session(written_head(&first.plan))
        .with_operation_at(cursorless, OperationVersion(2));
    let context = next.context(&clock, &ids);

    let outcome = continue_stop(&context, &command(45)).await;
    assert!(
        outcome.is_err(),
        "resuming from position zero would re-settle agents the first step already settled"
    );
}

#[tokio::test]
async fn no_continuation_step_calls_a_port_that_writes() {
    let overflow = STOP_BATCH_AGENTS + 2;
    let (first, clock, ids) = first_step(46, overflow).await;
    let next = ScriptedPorts::idle()
        .with_active_agents(overflow)
        .with_settled_prefix(usize::from(STOP_BATCH_AGENTS))
        .with_session(written_head(&first.plan))
        .with_operation_at(first.projected.clone(), OperationVersion(2));
    {
        let context = next.context(&clock, &ids);
        continue_stop(&context, &command(46)).await.expect("steps");
    }
    assert!(
        !next.recorded_a_write(),
        "a step decides; the deployable commits. There is no committer in the context at all"
    );
}

#[tokio::test]
async fn the_dispatcher_routes_a_stop_to_its_step_and_attributes_the_plan_to_the_continuation() {
    let overflow = STOP_BATCH_AGENTS + 2;
    let (first, clock, ids) = first_step(47, overflow).await;
    let next = ScriptedPorts::idle()
        .with_active_agents(overflow)
        .with_settled_prefix(usize::from(STOP_BATCH_AGENTS))
        .with_session(written_head(&first.plan))
        .with_operation_at(first.projected.clone(), OperationVersion(2));
    let context = next.context(&clock, &ids);

    let step = continue_operation(&context, &command(47))
        .await
        .expect("the dispatcher finds the stop step");

    assert_eq!(
        step.plan.intent,
        TransactionIntent::ContinueOperation,
        "a step and the admission that started it write the same rows under different          preconditions, so they must not share an intent"
    );
    assert_ne!(
        first.plan.intent, step.plan.intent,
        "the client token is derived from the whole plan; a shared intent would leave a provider          failure ambiguous between the two"
    );
}

#[tokio::test]
async fn the_dispatcher_refuses_a_kind_it_has_no_step_for_and_names_the_seam() {
    let overflow = STOP_BATCH_AGENTS + 2;
    let (first, clock, ids) = first_step(48, overflow).await;
    let mut purge = first.projected.clone();
    purge.kind = aex_operation_domain::OperationKind::SessionPurge;

    let next = ScriptedPorts::idle()
        .with_active_agents(overflow)
        .with_session(written_head(&first.plan))
        .with_operation_at(purge, OperationVersion(2));
    let context = next.context(&clock, &ids);

    let error = continue_operation(&context, &command(48))
        .await
        .expect_err("a kind with no step must refuse rather than loop making no progress");
    let text = format!("{error}");
    assert!(
        text.contains("cascade"),
        "the refusal names what is missing, not just that something is: {text}"
    );
}
