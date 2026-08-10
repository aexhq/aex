//! Attached guest-serving acceptance tests.

use super::{
    Arc, AttachResponse, DeliveryMode, FakeRunner, Fence, FrameExpectation, PROTOCOL_V1,
    ResponseStatus, ResultRequest, ResultResponse, StartRequest, StatusCode, TerminalState, Verb,
    aws_run_hook_body, decode_response, exec_start, framed, generation, guest, operation, post,
    router,
};

fn expectation() -> FrameExpectation {
    FrameExpectation {
        generation: generation(),
        min_fence: Fence(0),
        schema_version: PROTOCOL_V1,
        max_frame_bytes: 1_048_576,
    }
}

async fn attach(app: &axum::Router, start: &StartRequest) -> AttachResponse {
    let (status, body) = post(
        app,
        Verb::Attach.path(),
        framed(
            Verb::Attach,
            &serde_json::to_vec(start).expect("it serializes"),
            Fence(1),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let frame = decode_response(&body, &expectation()).expect("a framed response");
    assert_eq!(frame.preamble.status, ResponseStatus::Payload);
    assert_eq!(frame.preamble.verb, Verb::Attach);
    serde_json::from_slice(frame.payload).expect("a typed attach answer")
}

#[tokio::test]
async fn an_attached_operation_returns_its_result_on_the_connection_that_started_it() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let runner = Arc::new(FakeRunner::default());
    let app = router(guest(dir.path(), Arc::clone(&runner)));
    let (status, _) = post(
        &app,
        aex_hands_agent::session::LifecycleHook::Run.path(),
        aws_run_hook_body(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let start = exec_start(DeliveryMode::Attached);
    let answered = attach(&app, &start).await;
    let AttachResponse::Terminal {
        operation: found,
        existing,
        terminal,
        chunk,
        ..
    } = answered
    else {
        panic!("an attached operation answers with its terminal: {answered:?}");
    };
    assert_eq!(found, operation());
    assert!(!existing);
    assert_eq!(terminal.state, TerminalState::Succeeded);
    assert_eq!(terminal.body_len, b"hello from the guest".len() as u64);
    let chunk = chunk.expect("the body rides the same response");
    assert_eq!(chunk.bytes, b"hello from the guest");
    assert!(chunk.last);
    assert_eq!(runner.started.lock().expect("fixture lock").len(), 1);

    let pull = ResultRequest {
        binding: start.binding,
        operation: operation(),
        from_offset: 0,
        max_bytes: 1_048_576,
    };
    let (status, body) = post(
        &app,
        Verb::Result.path(),
        framed(
            Verb::Result,
            &serde_json::to_vec(&pull).expect("it serializes"),
            Fence(1),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let frame = decode_response(&body, &expectation()).expect("a framed response");
    let ResultResponse::Terminal {
        terminal: pulled,
        chunk,
    } = serde_json::from_slice(frame.payload).expect("a typed result")
    else {
        panic!("the same operation is terminal");
    };
    assert_eq!(pulled.digest, terminal.digest);
    assert_eq!(chunk.expect("the body").bytes, b"hello from the guest");
}

#[tokio::test]
async fn a_repeated_attach_starts_no_second_process_and_says_it_found_one() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let runner = Arc::new(FakeRunner::default());
    let app = router(guest(dir.path(), Arc::clone(&runner)));
    post(
        &app,
        aex_hands_agent::session::LifecycleHook::Run.path(),
        aws_run_hook_body(),
    )
    .await;

    let start = exec_start(DeliveryMode::Attached);
    let _ = attach(&app, &start).await;
    let repeated = attach(&app, &start).await;
    let AttachResponse::Terminal {
        existing, terminal, ..
    } = repeated
    else {
        panic!("a replayed attach answers from the journal: {repeated:?}");
    };
    assert!(existing);
    assert_eq!(terminal.state, TerminalState::Succeeded);
    assert_eq!(runner.started.lock().expect("fixture lock").len(), 1);
}

#[tokio::test]
async fn a_detached_delivery_posted_to_the_attach_path_is_refused_rather_than_served() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let runner = Arc::new(FakeRunner::default());
    let app = router(guest(dir.path(), Arc::clone(&runner)));
    post(
        &app,
        aex_hands_agent::session::LifecycleHook::Run.path(),
        aws_run_hook_body(),
    )
    .await;

    let start = exec_start(DeliveryMode::Detached);
    let (status, body) = post(
        &app,
        Verb::Attach.path(),
        framed(
            Verb::Attach,
            &serde_json::to_vec(&start).expect("it serializes"),
            Fence(1),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let frame = decode_response(&body, &expectation()).expect("a framed response");
    assert_eq!(frame.preamble.status, ResponseStatus::ProtocolError);
    assert!(runner.started.lock().expect("fixture lock").is_empty());
}

#[tokio::test]
async fn an_attach_the_supervisor_refuses_starts_nothing_and_says_why() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let runner = Arc::new(FakeRunner::default());
    let app = router(guest(dir.path(), Arc::clone(&runner)));
    post(
        &app,
        aex_hands_agent::session::LifecycleHook::Run.path(),
        aws_run_hook_body(),
    )
    .await;

    let mut lapsed = exec_start(DeliveryMode::Attached);
    lapsed.deadline = aex_wire::types::Timestamp::from_unix_millis(1).expect("bounded instant");
    let answered = attach(&app, &lapsed).await;
    let AttachResponse::Rejected { failure, .. } = answered else {
        panic!("a lapsed deadline is a refusal, not a result: {answered:?}");
    };
    assert_eq!(failure.reason, "timeout");
    assert!(runner.started.lock().expect("fixture lock").is_empty());
}
