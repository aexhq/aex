//! Attached delivery integration at the Brain tool-executor boundary.

use aex_brain_tool_catalog::router::ToolExecutor as _;

use super::{
    CancelToken, ContentHash, ExecutorRoute, HandsOperationId, HandsResult, ToolName, ToolOutcome,
    ToolResultPart, call, fixture, generation, ticket,
};

/// No detached reference is minted and status is never polled.
#[tokio::test]
async fn an_attached_start_completes_in_place_and_polls_nothing() {
    let (executor, hands) = fixture();
    let body = "dir 4096 src/\nfile 12 a.txt";
    *hands.attached.lock().expect("attached") = Some(HandsResult {
        operation: HandsOperationId(String::new()),
        generation: generation(),
        exit_code: 0,
        inline: Some(body.to_owned()),
        placed: None,
        truncated: false,
        duration_ms: 4,
        checksum: ContentHash::of(body.as_bytes()),
    });
    let mut listing = call();
    listing.route.name = ToolName::parse("list_dir").expect("tool name");
    listing.input =
        aex_wire::CanonicalJson::from_value(&serde_json::json!({})).expect("canonical input");

    let outcome = executor
        .invoke(&ticket(), &listing, &CancelToken::new())
        .await
        .expect("the attached start answered in place");
    let ToolOutcome::Completed(result) = outcome else {
        panic!("an attached delivery is not a detached operation");
    };
    assert_eq!(result.executed_on, ExecutorRoute::Hands);
    assert_eq!(result.duration_ms, 4);
    let [ToolResultPart::Text { text }] = result.content.as_slice() else {
        panic!("a guest listing is text");
    };
    assert_eq!(text.as_str(), body);
    assert!(
        hands.statuses.lock().expect("statuses").is_empty(),
        "the whole point: nothing was polled"
    );
}
