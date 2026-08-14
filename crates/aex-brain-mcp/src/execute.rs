//! Actual MCP 2026-07-28 Streamable HTTP execution over an already screened client.

use aex_wire::CanonicalJson;
use rmcp::model::{
    CallToolRequestParams, ClientInfo, JsonObject, PaginatedRequestParams, ProtocolVersion,
};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::{ClientLifecycleMode, ClientServiceExt as _};
use std::collections::HashMap;

use crate::client::{
    MAX_LIST_PAGES, MAX_REMOTE_TOOLS, MAX_RESPONSE_BYTES, MAX_SSE_EVENT_BYTES, PROTOCOL_REVISION,
};
use crate::pool::PooledClient;

/// A complete bounded MCP tool result.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteToolResult {
    /// Canonical JSON encoding of the rmcp result.
    pub body: Vec<u8>,
    /// Server's tool-result error flag.
    pub is_error: bool,
}

/// Bounded tool names observed during one pinned MCP handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteQualification {
    /// Canonical server tool names in stable order.
    pub tools: Vec<String>,
}

/// Why an already-qualified remote call could not complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RemoteCallError {
    /// Arguments were not a JSON object.
    #[error("MCP tool arguments must be an object")]
    Arguments,
    /// Transport or protocol startup failed after dispatch may have begun.
    #[error("MCP Streamable HTTP transport failed")]
    Transport,
    /// The complete returned value exceeded the retained response bound.
    #[error("MCP result exceeds the complete response bound")]
    ResponseTooLarge,
    /// Result could not be encoded.
    #[error("MCP result is not encodable")]
    InvalidResult,
}

/// Executes one qualified tools/call using rmcp's maintained Streamable HTTP
/// implementation. The caller must obtain `client` through `screened_client`, so
/// DNS/IP rebinding and tenant-isolated connection reuse have already passed.
///
/// This function deliberately disables transparent session reinitialization:
/// replaying a tools/call after a 404 can duplicate a side effect. The durable
/// scheduler applies task-based recovery or records an ambiguous outcome.
///
/// # Errors
///
/// Returns [`RemoteCallError`] for invalid object arguments, transport/protocol
/// failure, oversize complete response, or invalid serialization.
pub async fn call_streamable_http(
    client: PooledClient,
    headers: impl IntoIterator<Item = (String, String)>,
    tool: &str,
    arguments: &CanonicalJson,
) -> Result<RemoteToolResult, RemoteCallError> {
    debug_assert_eq!(PROTOCOL_REVISION, "2026-07-28");
    let object = arguments_object(arguments)?;
    let mut config = StreamableHttpClientTransportConfig::with_uri(client.target().to_string())
        .max_sse_event_size(MAX_SSE_EVENT_BYTES)
        .reinit_on_expired_session(false);
    let mut custom_headers = HashMap::new();
    for (name, value) in headers {
        let name = http::HeaderName::try_from(name).map_err(|_| RemoteCallError::Transport)?;
        let mut value =
            http::HeaderValue::try_from(value).map_err(|_| RemoteCallError::Transport)?;
        value.set_sensitive(true);
        custom_headers.insert(name, value);
    }
    config.custom_headers = custom_headers;
    let transport =
        rmcp::transport::StreamableHttpClientTransport::with_client(client.http_client(), config);
    let service = ClientInfo::default()
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .map_err(|_| RemoteCallError::Transport)?;
    let response = service
        .call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(object))
        .await
        .map_err(|_| RemoteCallError::Transport)?;
    let is_error = response.is_error.unwrap_or(false);
    let body = serde_json::to_vec(&response).map_err(|_| RemoteCallError::InvalidResult)?;
    let _ = service.cancel().await;
    if body.len() > MAX_RESPONSE_BYTES {
        return Err(RemoteCallError::ResponseTooLarge);
    }
    Ok(RemoteToolResult { body, is_error })
}

/// Completes the pinned Streamable HTTP handshake and reads at most the launch
/// page/tool bounds before session readiness can be published.
///
/// # Errors
///
/// Returns [`RemoteCallError`] for transport failure or an over-bound/invalid
/// tool surface.
pub async fn qualify_streamable_http(
    client: PooledClient,
    headers: impl IntoIterator<Item = (String, String)>,
) -> Result<RemoteQualification, RemoteCallError> {
    let mut config = StreamableHttpClientTransportConfig::with_uri(client.target().to_string())
        .max_sse_event_size(MAX_SSE_EVENT_BYTES)
        .reinit_on_expired_session(false);
    let mut custom_headers = HashMap::new();
    for (name, value) in headers {
        let name = http::HeaderName::try_from(name).map_err(|_| RemoteCallError::Transport)?;
        let mut value =
            http::HeaderValue::try_from(value).map_err(|_| RemoteCallError::Transport)?;
        value.set_sensitive(true);
        custom_headers.insert(name, value);
    }
    config.custom_headers = custom_headers;
    let transport =
        rmcp::transport::StreamableHttpClientTransport::with_client(client.http_client(), config);
    let service = ClientInfo::default()
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .map_err(|_| RemoteCallError::Transport)?;
    let mut tools = Vec::new();
    let mut cursor = None;
    for _ in 0..MAX_LIST_PAGES {
        let page = service
            .list_tools(Some(PaginatedRequestParams::default().with_cursor(cursor)))
            .await
            .map_err(|_| RemoteCallError::Transport)?;
        for tool in page.tools {
            if tools.len() == MAX_REMOTE_TOOLS || tool.name.is_empty() || tool.name.len() > 128 {
                let _ = service.cancel().await;
                return Err(RemoteCallError::ResponseTooLarge);
            }
            tools.push(tool.name.to_string());
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            tools.sort();
            let _ = service.cancel().await;
            return Ok(RemoteQualification { tools });
        }
    }
    let _ = service.cancel().await;
    Err(RemoteCallError::ResponseTooLarge)
}

fn arguments_object(arguments: &CanonicalJson) -> Result<JsonObject, RemoteCallError> {
    arguments
        .to_value()
        .as_object()
        .cloned()
        .ok_or(RemoteCallError::Arguments)
}

#[cfg(test)]
mod tests {
    use super::{RemoteCallError, arguments_object};
    use aex_wire::CanonicalJson;

    #[test]
    fn only_object_arguments_can_reach_an_mcp_tool_call() {
        let object = CanonicalJson::parse(r#"{"path":"/workspace/a"}"#).expect("canonical");
        assert_eq!(
            arguments_object(&object)
                .expect("object")
                .get("path")
                .and_then(serde_json::Value::as_str),
            Some("/workspace/a")
        );
        let array = CanonicalJson::parse("[]").expect("canonical");
        assert_eq!(arguments_object(&array), Err(RemoteCallError::Arguments));
    }
}
