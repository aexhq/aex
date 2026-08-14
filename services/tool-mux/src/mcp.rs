//! Session-scoped MCP secret custody used by the builtin sandbox MCP tool.

use aex_tool_mux::{ToolCallIdentity, ToolMuxFuture};
use aex_wire::ids::ResourceName;

/// Session-scoped MCP custody reader. Implementations validate context and
/// purpose framing before returning one zeroizing value.
pub trait McpSecretReader: Send + Sync + 'static {
    /// Reveals one frozen header reference for this call's tenant.
    fn reveal<'a>(
        &'a self,
        name: &'a ResourceName,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<zeroize::Zeroizing<String>, String>>;
}
