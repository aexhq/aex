//! Small deterministic properties for request binding.

use std::collections::BTreeSet;

#[test]
fn every_changed_call_identity_changes_the_body_binding() {
    let bindings = (0_u32..256)
        .map(|attempt| {
            aex_tool_mux::request_binding(&serde_json::json!({
                "session": "ses_01900000000070008000000000000001",
                "call": "stable",
                "attempt": attempt,
            }))
            .expect("JSON binds")
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(bindings.len(), 256);
}
