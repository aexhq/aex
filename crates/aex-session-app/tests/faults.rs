//! Fault behaviour for `aex-session-app`.
//!
//! Two things must hold when something goes wrong. A port failure must surface
//! as a typed [`AppError`] that says whether the caller may retry, and a use
//! case that fails must leave no partial plan behind — there is no "half a
//! transaction" to submit.

use aex_session_app::testing::{CountingIds, FixedClock, ScriptedPorts, message_identity_under};
use aex_session_app::{
    AppError, CommitError, MESSAGE_TEXT_MAX_BYTES, PortError, SendMessage, admit_message,
};
use aex_session_domain::WorkAdmission;
use aex_session_domain::testing::{moment, session_fixture};
use aex_wire::error::ErrorCode;
use aex_wire::models::MessageSendRequest;

fn clock() -> FixedClock {
    FixedClock(moment(1_000))
}

fn send_message() -> SendMessage {
    let session = session_fixture();
    let request = MessageSendRequest {
        deadline: None,
        max_spend_cents: None,
        response_format: None,
        text: "hello".to_owned(),
    };
    SendMessage {
        workspace: session.workspace,
        session: session.id,
        identity: message_identity_under("fixture-message-key", &session, &request),
        request,
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
async fn message_text_accepts_exact_utf8_byte_ceiling() {
    for text in [
        "a".repeat(MESSAGE_TEXT_MAX_BYTES),
        "🦀".repeat(MESSAGE_TEXT_MAX_BYTES / 4),
    ] {
        assert_eq!(text.len(), MESSAGE_TEXT_MAX_BYTES);
        let ports = ScriptedPorts::idle();
        let clock = clock();
        let ids = CountingIds::default();
        let context = ports.context(&clock, &ids);
        let mut command = send_message();
        command.request.text = text;
        command.identity = message_identity_under(
            "message-text-at-ceiling",
            &session_fixture(),
            &command.request,
        );

        admit_message(&context, &command)
            .await
            .expect("the exact UTF-8 byte ceiling is admitted");
    }
}

#[tokio::test]
async fn message_text_rejects_one_byte_over_before_any_port_read() {
    for text in [
        "a".repeat(MESSAGE_TEXT_MAX_BYTES + 1),
        format!("{}a", "🦀".repeat(MESSAGE_TEXT_MAX_BYTES / 4)),
    ] {
        assert_eq!(text.len(), MESSAGE_TEXT_MAX_BYTES + 1);
        let ports = ScriptedPorts::idle();
        let clock = clock();
        let ids = CountingIds::default();
        let context = ports.context(&clock, &ids);
        let mut command = send_message();
        command.request.text = text;
        command.identity = message_identity_under(
            "message-text-over-ceiling",
            &session_fixture(),
            &command.request,
        );

        let error = admit_message(&context, &command)
            .await
            .expect_err("one byte over the ceiling is refused");
        assert_eq!(error.code(), ErrorCode::InvalidRequest);
        assert!(
            ports.calls().is_empty(),
            "request-shape admission precedes session, account, spend, and provider work"
        );
    }
}
