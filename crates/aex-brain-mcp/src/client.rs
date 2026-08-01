//! MCP 2026-07-28 request identity and frozen qualification helpers.

use std::collections::{BTreeMap, BTreeSet};

use aex_brain_tool_catalog::manifest::ToolName;
use aex_wire::CanonicalJson;
use aex_wire::ids::ResourceName;
use base64::Engine as _;
use base64::prelude::BASE64_STANDARD;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use aex_brain_managed_web::egress::{
    DnsResolver, EgressPolicy, EgressRejection, ParsedTarget, ValidatedTarget, resolve_and_screen,
    validate,
};

/// The only protocol revision accepted or emitted by this adapter.
pub const PROTOCOL_REVISION: &str = "2026-07-28";
/// Maximum tools/list pages admitted during registration.
pub const MAX_LIST_PAGES: usize = 8;
/// Maximum tools frozen for one server.
pub const MAX_REMOTE_TOOLS: usize = 128;
/// Maximum complete response bytes.
pub const MAX_RESPONSE_BYTES: usize = 1_048_576;
/// Maximum one SSE event bytes.
pub const MAX_SSE_EVENT_BYTES: usize = 65_536;
/// Maximum request-scoped notifications.
pub const MAX_NOTIFICATIONS: u16 = 256;
/// Maximum parsed JSON nesting depth.
pub const MAX_JSON_DEPTH: usize = 32;

/// The only transport type enabled by this crate's exact `rmcp` feature pin.
pub use rmcp::transport::streamable_http_client::StreamableHttpClientTransport;

/// One validated mapping from an argument path to a request header suffix.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HeaderAnnotation {
    path: Vec<String>,
    name: String,
}

/// Validated `x-mcp-header` annotations for one frozen input schema.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct McpHeaderAnnotations(Vec<HeaderAnnotation>);

impl McpHeaderAnnotations {
    /// An annotation-free schema mapping.
    #[must_use]
    pub const fn empty() -> Self {
        Self(Vec::new())
    }

    /// Validates annotations and their statically reachable primitive paths.
    ///
    /// # Errors
    ///
    /// Any invalid annotation excludes this tool from qualification.
    pub fn validate(schema: &Value) -> Result<Self, AnnotationExclusion> {
        let total = count_annotations(schema);
        let mut annotations = Vec::new();
        collect_property_annotations(schema, &mut Vec::new(), &mut annotations)?;
        if annotations.len() != total {
            return Err(AnnotationExclusion::InvalidMcpHeaderAnnotation);
        }
        let mut names = BTreeSet::new();
        for annotation in &annotations {
            if !names.insert(annotation.name.to_ascii_lowercase()) {
                return Err(AnnotationExclusion::InvalidMcpHeaderAnnotation);
            }
        }
        Ok(Self(annotations))
    }
}

/// Why one remote tool is excluded while its peers remain eligible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationExclusion {
    /// Header annotation violated name, placement, uniqueness, or type rules.
    InvalidMcpHeaderAnnotation,
}

/// Complete headers for one MCP POST. Construction is closed so session and
/// resumability-era headers cannot be inserted by this adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpHeaders(BTreeMap<String, String>);

impl McpHeaders {
    /// Derives request identity and every argument mirror from the same
    /// canonical argument object sent in the JSON-RPC body.
    ///
    /// # Errors
    ///
    /// Rejects a missing or non-primitive annotated argument.
    pub fn for_call(
        method: &str,
        name: &str,
        arguments: &CanonicalJson,
        annotations: &McpHeaderAnnotations,
    ) -> Result<Self, HeaderBuildError> {
        let argument_value = arguments.to_value();
        let mut headers = BTreeMap::from([
            (
                "accept".to_owned(),
                "application/json, text/event-stream".to_owned(),
            ),
            (
                "mcp-protocol-version".to_owned(),
                PROTOCOL_REVISION.to_owned(),
            ),
            ("mcp-method".to_owned(), encode_header_value(method)),
            ("mcp-name".to_owned(), encode_header_value(name)),
        ]);
        for annotation in &annotations.0 {
            let Some(value) = value_at_path(&argument_value, &annotation.path) else {
                continue;
            };
            let value =
                primitive_header_value(value).ok_or(HeaderBuildError::ArgumentNotPrimitive)?;
            headers.insert(
                format!("mcp-param-{}", annotation.name.to_ascii_lowercase()),
                encode_header_value(&value),
            );
        }
        Ok(Self(headers))
    }

    /// Returns a header value by case-insensitive HTTP name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(&name.to_ascii_lowercase()).map(String::as_str)
    }

    /// Iterates deterministic lowercase header names.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }
}

/// Header/body construction failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HeaderBuildError {
    /// Runtime argument did not match its frozen primitive schema.
    #[error("annotated MCP argument is not a primitive")]
    ArgumentNotPrimitive,
}

/// One tool from a bounded registration-time tools/list response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredTool {
    /// Remote name before AEX namespacing.
    pub name: String,
    /// Remote input schema.
    pub input_schema: Value,
    /// Optional remote output schema.
    pub output_schema: Option<Value>,
}

/// One qualified tool frozen into the server manifest.
#[derive(Debug, Clone, PartialEq)]
pub struct QualifiedTool {
    /// Provider-visible AEX namespace.
    pub advertised_name: ToolName,
    /// Original server spelling.
    pub remote_name: String,
    /// Validated input header mapping.
    pub headers: McpHeaderAnnotations,
    /// Frozen input schema.
    pub input_schema: Value,
    /// Frozen output schema.
    pub output_schema: Option<Value>,
}

/// Registration record for tools that were safely excluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExcludedTool {
    /// Remote name.
    pub remote_name: String,
    /// Typed exclusion reason.
    pub reason: ToolExclusionReason,
}

/// Frozen result of registration-time discovery.
#[derive(Debug, Clone, PartialEq)]
pub struct McpServerManifest {
    /// Exact accepted protocol revision.
    pub protocol_revision: &'static str,
    /// Tools safe to advertise.
    pub tools: Vec<QualifiedTool>,
    /// Independently excluded tools.
    pub excluded: Vec<ExcludedTool>,
}

/// Why qualification omitted one tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExclusionReason {
    /// Invalid x-mcp-header annotation.
    InvalidMcpHeaderAnnotation,
    /// Namespaced provider name exceeded 64 bytes or its grammar.
    NameInvalid,
}

/// Qualifies already bounded discovery results without any runtime rediscovery.
///
/// # Errors
///
/// Refuses protocol mismatch, duplicate remote names, and catalog-level bounds.
pub fn qualify_discovery(
    server: &ResourceName,
    supported_versions: &[String],
    pages: usize,
    tools: Vec<DiscoveredTool>,
) -> Result<McpServerManifest, QualificationError> {
    if !supported_versions
        .iter()
        .any(|version| version == PROTOCOL_REVISION)
    {
        return Err(QualificationError::ProtocolVersionUnsupported);
    }
    if pages > MAX_LIST_PAGES || tools.len() > MAX_REMOTE_TOOLS {
        return Err(QualificationError::DiscoveryBoundsExceeded);
    }
    let mut remote_names = BTreeSet::new();
    if tools
        .iter()
        .any(|tool| !remote_names.insert(tool.name.clone()))
    {
        return Err(QualificationError::DuplicateRemoteName);
    }
    let mut qualified = Vec::new();
    let mut excluded = Vec::new();
    for tool in tools {
        let headers = match McpHeaderAnnotations::validate(&tool.input_schema) {
            Ok(headers) => headers,
            Err(AnnotationExclusion::InvalidMcpHeaderAnnotation) => {
                excluded.push(ExcludedTool {
                    remote_name: tool.name,
                    reason: ToolExclusionReason::InvalidMcpHeaderAnnotation,
                });
                continue;
            }
        };
        let Ok(advertised_name) = ToolName::namespaced_mcp(server.as_str(), &tool.name) else {
            excluded.push(ExcludedTool {
                remote_name: tool.name,
                reason: ToolExclusionReason::NameInvalid,
            });
            continue;
        };
        qualified.push(QualifiedTool {
            advertised_name,
            remote_name: tool.name,
            headers,
            input_schema: tool.input_schema,
            output_schema: tool.output_schema,
        });
    }
    Ok(McpServerManifest {
        protocol_revision: PROTOCOL_REVISION,
        tools: qualified,
        excluded,
    })
}

/// Whole-server qualification failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum QualificationError {
    /// Server did not support exactly 2026-07-28.
    #[error("MCP server does not support protocol 2026-07-28")]
    ProtocolVersionUnsupported,
    /// Bounded discovery page/tool ceiling was exceeded.
    #[error("MCP discovery bounds exceeded")]
    DiscoveryBoundsExceeded,
    /// One server returned a duplicate remote name.
    #[error("MCP server returned duplicate tool names")]
    DuplicateRemoteName,
}

/// Per-call hostile-stream counters. This value owns no connection and is
/// discarded with the response stream on cancellation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResponseBounds {
    response_bytes: usize,
    notifications: u16,
}

impl ResponseBounds {
    /// Empty response counters.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            response_bytes: 0,
            notifications: 0,
        }
    }

    /// Admits one complete event or fails without truncating it.
    ///
    /// # Errors
    ///
    /// Refuses an oversized event/response, invalid UTF-8/JSON, excessive
    /// nesting, or notification overflow.
    pub fn accept_event(
        &mut self,
        bytes: &[u8],
        notification: bool,
    ) -> Result<Value, ResponseBoundError> {
        if bytes.len() > MAX_SSE_EVENT_BYTES {
            return Err(ResponseBoundError::EventTooLarge);
        }
        if self.response_bytes.saturating_add(bytes.len()) > MAX_RESPONSE_BYTES {
            return Err(ResponseBoundError::ResponseTooLarge);
        }
        let text = std::str::from_utf8(bytes).map_err(|_| ResponseBoundError::InvalidUtf8)?;
        let value: Value =
            serde_json::from_str(text).map_err(|_| ResponseBoundError::InvalidJson)?;
        if json_depth(&value) > MAX_JSON_DEPTH {
            return Err(ResponseBoundError::JsonTooDeep);
        }
        if notification && self.notifications == MAX_NOTIFICATIONS {
            return Err(ResponseBoundError::TooManyNotifications);
        }
        self.response_bytes += bytes.len();
        if notification {
            self.notifications += 1;
        }
        Ok(value)
    }
}

/// Hostile or out-of-bounds Streamable HTTP response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ResponseBoundError {
    /// One SSE event exceeded 64 KiB.
    #[error("MCP SSE event exceeds 65536 bytes")]
    EventTooLarge,
    /// Complete response exceeded 1 MiB.
    #[error("MCP response exceeds 1048576 bytes")]
    ResponseTooLarge,
    /// Event was not valid UTF-8.
    #[error("MCP response is not valid UTF-8")]
    InvalidUtf8,
    /// Event was not valid JSON.
    #[error("MCP response event is not valid JSON")]
    InvalidJson,
    /// Parsed JSON exceeded depth 32.
    #[error("MCP response JSON exceeds depth 32")]
    JsonTooDeep,
    /// More than 256 request-scoped notifications arrived.
    #[error("MCP response exceeds 256 notifications")]
    TooManyNotifications,
}

fn collect_property_annotations(
    schema: &Value,
    path: &mut Vec<String>,
    output: &mut Vec<HeaderAnnotation>,
) -> Result<(), AnnotationExclusion> {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return Ok(());
    };
    for (property, property_schema) in properties {
        path.push(property.clone());
        if let Some(annotation) = property_schema.get("x-mcp-header") {
            let name = annotation
                .as_str()
                .filter(|name| !name.is_empty() && name.bytes().all(is_tchar))
                .ok_or(AnnotationExclusion::InvalidMcpHeaderAnnotation)?;
            validate_annotated_type(property_schema)?;
            output.push(HeaderAnnotation {
                path: path.clone(),
                name: name.to_owned(),
            });
        }
        collect_property_annotations(property_schema, path, output)?;
        path.pop();
    }
    Ok(())
}

fn validate_annotated_type(schema: &Value) -> Result<(), AnnotationExclusion> {
    match schema.get("type").and_then(Value::as_str) {
        Some("string" | "boolean") => Ok(()),
        Some("integer") => {
            const MAX_SAFE: i64 = 9_007_199_254_740_991;
            let minimum = schema.get("minimum").and_then(Value::as_i64);
            let maximum = schema.get("maximum").and_then(Value::as_i64);
            if minimum.is_some_and(|value| value >= -MAX_SAFE)
                && maximum.is_some_and(|value| value <= MAX_SAFE)
            {
                Ok(())
            } else {
                Err(AnnotationExclusion::InvalidMcpHeaderAnnotation)
            }
        }
        _ => Err(AnnotationExclusion::InvalidMcpHeaderAnnotation),
    }
}

fn count_annotations(value: &Value) -> usize {
    match value {
        Value::Object(object) => {
            usize::from(object.contains_key("x-mcp-header"))
                + object.values().map(count_annotations).sum::<usize>()
        }
        Value::Array(values) => values.iter().map(count_annotations).sum(),
        _ => 0,
    }
}

const fn is_tchar(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

fn value_at_path<'a>(mut value: &'a Value, path: &[String]) -> Option<&'a Value> {
    for component in path {
        value = value.as_object()?.get(component)?;
    }
    Some(value)
}

fn primitive_header_value(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(value) if value.is_i64() || value.is_u64() => Some(value.to_string()),
        _ => None,
    }
}

fn encode_header_value(value: &str) -> String {
    if value.bytes().all(|byte| (0x20..=0x7e).contains(&byte)) && !value.starts_with("=?base64?") {
        value.to_owned()
    } else {
        format!("=?base64?{}?=", BASE64_STANDARD.encode(value))
    }
}

fn json_depth(value: &Value) -> usize {
    match value {
        Value::Array(values) => 1 + values.iter().map(json_depth).max().unwrap_or(0),
        Value::Object(object) => 1 + object.values().map(json_depth).max().unwrap_or(0),
        _ => 0,
    }
}
