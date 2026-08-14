//! Fail-closed raw-contract cases.

use aex_tool_mux::{SandboxConfig, ToolTarget};

#[test]
fn unknown_targets_and_sandbox_fields_are_rejected() {
    assert!(serde_json::from_value::<ToolTarget>(serde_json::json!({"target":"shell"})).is_err());
    assert!(
        serde_json::from_value::<SandboxConfig>(serde_json::json!({
            "enabled": false,
            "generation": null,
            "unexpected": true,
        }))
        .is_err()
    );
}
