//! MCP 2026-07-28 request-header conformance.

use aex_brain_mcp::client::{McpHeaderAnnotations, McpHeaders, PROTOCOL_REVISION};
use aex_wire::CanonicalJson;
use serde_json::json;

#[test]
fn every_call_emits_the_revision_and_encoded_identity_headers() {
    let arguments = CanonicalJson::from_value(&json!({})).expect("canonical arguments");
    let headers = McpHeaders::for_call(
        "tools/call",
        "remote.tool",
        &arguments,
        &McpHeaderAnnotations::empty(),
    )
    .expect("headers");

    assert_eq!(headers.get("MCP-Protocol-Version"), Some(PROTOCOL_REVISION));
    assert_eq!(headers.get("Mcp-Method"), Some("tools/call"));
    assert_eq!(headers.get("Mcp-Name"), Some("remote.tool"));
    assert_eq!(headers.get("Mcp-Session-Id"), None);
    assert_eq!(headers.get("Last-Event-ID"), None);
}

#[test]
fn non_ascii_identity_uses_the_unambiguous_base64_sentinel() {
    let arguments = CanonicalJson::from_value(&json!({})).expect("canonical arguments");
    let headers = McpHeaders::for_call(
        "tools/call",
        "weather.天气",
        &arguments,
        &McpHeaderAnnotations::empty(),
    )
    .expect("headers");

    assert_eq!(
        headers.get("Mcp-Name"),
        Some("=?base64?d2VhdGhlci7lpKnmsJQ=?=")
    );
}
