//! Service-visible security boundaries.

use std::collections::BTreeMap;

use aex_tool_mux::ToolTarget;
use aex_wire::ids::ResourceName;

#[test]
fn remote_mcp_wire_carries_secret_references_not_plaintext() {
    let target = ToolTarget::RemoteMcp {
        server: ResourceName::parse("fixture").expect("server"),
        endpoint: "https://mcp.example".to_owned(),
        headers: BTreeMap::from([(
            "authorization".to_owned(),
            ResourceName::parse("sealed-header").expect("secret name"),
        )]),
        tool: "echo".to_owned(),
    };
    let wire = serde_json::to_string(&target).expect("encodes");
    assert!(wire.contains("sealed-header"));
    assert!(!wire.contains("PLAINTEXT-CANARY"));
}
