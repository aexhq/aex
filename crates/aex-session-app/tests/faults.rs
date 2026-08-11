//! Fault behaviour for `aex-session-app`.
//!
//! Two things must hold when something goes wrong. A port failure must surface
//! as a typed [`AppError`] that says whether the caller may retry, and a use
//! case that fails must leave no partial plan behind — there is no "half a
//! transaction" to submit.

use aex_session_app::testing::{CountingIds, FixedClock, ScriptedPorts, fixture_spend};
use aex_session_app::{
    AppError, CommitError, PortError, SendMessage, SessionCommand, admit_message, stop_session,
};
use aex_session_domain::testing::{id, moment, run_id, session_fixture};
use aex_session_domain::{MessagePart, WorkAdmission};
use aex_wire::error::ErrorCode;
use aex_wire::idempotency::IntentDigest;
use aex_wire::ids::{MessageId, OperationId};

fn clock() -> FixedClock {
    FixedClock(moment(1_000))
}

fn send_message() -> SendMessage {
    let session = session_fixture();
    SendMessage {
        workspace: session.workspace,
        session: session.id,
        message: id::<MessageId>(20),
        run: run_id(21),
        parts: vec![MessagePart::Text {
            text: "hello".to_owned(),
        }],
        max_spend_cents: fixture_spend(),
        deadline: moment(60_000),
        intent: IntentDigest::from_bytes([3; 32]),
    }
}

#[test]
fn every_port_failure_class_maps_onto_a_code_and_a_retry_verdict() {
    let cases = [
        (
            PortError::NotFound { kind: "session" },
            ErrorCode::NotFound,
            false,
        ),
        (
            PortError::Throttled { kind: "session" },
            ErrorCode::InternalError,
            true,
        ),
        (
            PortError::Unavailable { kind: "session" },
            ErrorCode::InternalError,
            true,
        ),
        (
            PortError::Corrupt {
                kind: "session",
                reason: "bad record",
            },
            ErrorCode::InternalError,
            false,
        ),
    ];
    for (port, code, retryable) in cases {
        let error = AppError::Port(port.clone());
        assert_eq!(error.code(), code, "{port:?}");
        assert_eq!(error.retryable(), retryable, "{port:?}");
    }
}

#[test]
fn a_commit_failure_reports_exactly_which_conditions_failed() {
    // The adapter contract: a rejection names the failed conditions so the
    // application maps a typed customer error instead of guessing.
    let failed = vec![
        aex_session_app::ConditionId(0),
        aex_session_app::ConditionId(3),
    ];
    let error = AppError::Commit(CommitError::ConditionFailed {
        failed: failed.clone(),
    });
    assert_eq!(error.code(), ErrorCode::InternalError);
    assert!(!error.retryable(), "a failed condition is not a retry");
    let AppError::Commit(CommitError::ConditionFailed { failed: reported }) = error else {
        panic!("the arm carries its condition ids");
    };
    assert_eq!(reported, failed);
}

#[tokio::test]
async fn a_refused_command_leaves_no_partial_plan() {
    let mut session = session_fixture();
    session.work_admission = WorkAdmission::ContinuityLost;
    let ports = ScriptedPorts::idle().with_session(session);
    let clock = clock();
    let ids = CountingIds::default();
    let context = ports.context(&clock, &ids);

    let outcome = admit_message(&context, &send_message()).await;
    assert!(outcome.is_err());
    // There is no partial plan to submit: the use case returns a plan or an
    // error, never both and never half of one.
    assert!(!ports.recorded_a_write());
}

#[tokio::test]
async fn a_pause_exempt_command_still_plans_under_a_paused_account() {
    let ports = ScriptedPorts::idle().paused();
    let clock = clock();
    let ids = CountingIds::default();
    let context = ports.context(&clock, &ids);

    let session = session_fixture();
    let planned = stop_session(
        &context,
        &SessionCommand {
            workspace: session.workspace,
            session: session.id,
            operation: id::<OperationId>(30),
            intent: IntentDigest::from_bytes([30; 32]),
        },
    )
    .await
    .expect("stop is pause exempt");
    planned.plan.validate().expect("validates");
}
