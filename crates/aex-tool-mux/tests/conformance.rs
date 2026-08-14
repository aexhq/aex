//! Raw ToolMux wire-shape conformance.

use aex_tool_mux::{OfficialSandboxTool, ToolTarget};

#[test]
fn official_target_uses_the_closed_snake_case_wire_shape() {
    let target = ToolTarget::OfficialSandbox {
        tool: OfficialSandboxTool::Read,
    };
    let wire = serde_json::to_value(&target).expect("target encodes");
    assert_eq!(
        wire,
        serde_json::json!({"target":"official_sandbox","tool":"read"})
    );
    assert_eq!(
        serde_json::from_value::<ToolTarget>(wire).expect("target decodes"),
        target
    );
}
