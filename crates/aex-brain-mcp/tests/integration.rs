//! Discovery qualification integration evidence.

use aex_brain_mcp::client::{DiscoveredTool, PROTOCOL_REVISION, qualify_discovery};
use aex_wire::ids::ResourceName;
use serde_json::json;

#[test]
fn qualification_excludes_an_invalid_tool_without_hiding_valid_peers() {
    let server = ResourceName::parse("server").expect("server name");
    let tools = vec![
        DiscoveredTool {
            name: "valid".to_owned(),
            input_schema: json!({"type": "object", "properties": {}}),
            output_schema: None,
        },
        DiscoveredTool {
            name: "invalid".to_owned(),
            input_schema: json!({
                "type": "object",
                "x-mcp-header": "not-on-a-property"
            }),
            output_schema: None,
        },
    ];

    let manifest = qualify_discovery(&server, &[PROTOCOL_REVISION.to_owned()], 1, tools)
        .expect("bounded discovery");
    assert_eq!(manifest.tools.len(), 1);
    assert_eq!(manifest.tools[0].remote_name, "valid");
    assert_eq!(manifest.excluded.len(), 1);
    assert_eq!(manifest.excluded[0].remote_name, "invalid");
}
