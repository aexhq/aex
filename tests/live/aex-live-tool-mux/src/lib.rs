//! Live exact-generation, builtin MCP and local-result scenarios for tool-mux.

/// Required private service URL for an explicitly approved live run.
pub const URL_ENV: &str = "AEX_LIVE_TOOL_MUX_URL";
/// Disabled-sandbox official-tool request plus its exact body-bound assertion.
pub const DISABLED_REQUEST_ENV: &str = "AEX_LIVE_TOOL_MUX_DISABLED_REQUEST";
/// Enabled-sandbox official-tool request plus its exact body-bound assertion.
pub const SANDBOX_REQUEST_ENV: &str = "AEX_LIVE_TOOL_MUX_SANDBOX_REQUEST";
/// Enabled-sandbox remote Streamable HTTP MCP request plus assertion.
pub const REMOTE_MCP_REQUEST_ENV: &str = "AEX_LIVE_TOOL_MUX_REMOTE_MCP_REQUEST";
/// Exact-generation storage.persist request plus assertion.
pub const STORAGE_REQUEST_ENV: &str = "AEX_LIVE_TOOL_MUX_STORAGE_REQUEST";
/// Large-result request plus assertion, known to exceed the preview ceiling.
pub const LARGE_RESULT_REQUEST_ENV: &str = "AEX_LIVE_TOOL_MUX_LARGE_RESULT_REQUEST";
