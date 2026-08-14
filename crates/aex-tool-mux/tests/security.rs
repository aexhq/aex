//! Security invariants at the distributed request boundary.

#[test]
fn tenant_and_body_cannot_be_replayed_under_one_binding() {
    let first = aex_tool_mux::request_binding(&serde_json::json!({
        "workspace": "wsp_01900000000070008000000000000001",
        "arguments": {"path":"/workspace/a"},
    }))
    .expect("binds");
    let foreign = aex_tool_mux::request_binding(&serde_json::json!({
        "workspace": "wsp_01900000000070008000000000000002",
        "arguments": {"path":"/workspace/a"},
    }))
    .expect("binds");
    let changed_body = aex_tool_mux::request_binding(&serde_json::json!({
        "workspace": "wsp_01900000000070008000000000000001",
        "arguments": {"path":"/workspace/b"},
    }))
    .expect("binds");
    assert_ne!(first, foreign);
    assert_ne!(first, changed_body);
}

#[test]
fn trust_anchors_are_canonical_bounded_and_duplicate_free() {
    let key = "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc";
    let one = format!("018f47a2-65ee-7c61-a1d2-65097d0d8b11:{key}");
    assert_eq!(
        aex_tool_mux::parse_assertion_trust_anchors(&one)
            .expect("one anchor")
            .len(),
        1
    );
    assert!(aex_tool_mux::parse_assertion_trust_anchors(&format!("{one},{one}")).is_err());
    assert!(aex_tool_mux::parse_assertion_trust_anchors(&format!(" {one}")).is_err());
}
