//! Remote Streamable HTTP MCP composition.

use std::collections::BTreeMap;
use std::sync::Arc;

use aex_brain_mcp::pool::PooledClient;
use aex_tool_mux::{ExecutorOutput, FullOutput, McpPort, ToolCallIdentity, ToolMuxFuture};
use aex_wire::CanonicalJson;
use aex_wire::ids::ResourceName;

/// Registration/security authority that screens DNS on every use and returns
/// only a tenant- and revision-isolated client. The client contains no bearer
/// headers and is safe to hand to rmcp's maintained transport.
pub trait QualifiedMcpClientPort: Send + Sync + 'static {
    /// Resolves the frozen registration and acquires its screened pool entry.
    fn acquire<'a>(
        &'a self,
        server: &'a ResourceName,
        endpoint: &'a str,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<PooledClient, String>>;
}

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

/// Tool Mux's concrete maintained-library remote MCP executor.
pub struct RemoteMcpAdapter {
    clients: Arc<dyn QualifiedMcpClientPort>,
    secrets: Arc<dyn McpSecretReader>,
}

impl RemoteMcpAdapter {
    /// Binds the qualified client authority.
    #[must_use]
    pub const fn new(
        clients: Arc<dyn QualifiedMcpClientPort>,
        secrets: Arc<dyn McpSecretReader>,
    ) -> Self {
        Self { clients, secrets }
    }
}

impl McpPort for RemoteMcpAdapter {
    fn call_remote<'a>(
        &'a self,
        endpoint: &'a str,
        headers: &'a BTreeMap<String, ResourceName>,
        server: &'a ResourceName,
        tool: &'a str,
        arguments: &'a serde_json::Value,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<ExecutorOutput, String>> {
        Box::pin(async move {
            let arguments = CanonicalJson::from_value(arguments)
                .map_err(|_| "remote MCP arguments are not canonical JSON".to_owned())?;
            let mut revealed = BTreeMap::new();
            for (header, secret) in headers {
                revealed.insert(header.clone(), self.secrets.reveal(secret, call).await?);
            }
            let client = self.clients.acquire(server, endpoint, call).await?;
            let result = aex_brain_mcp::execute::call_streamable_http(
                client,
                revealed
                    .iter()
                    .map(|(name, value)| (name.clone(), value.to_string())),
                tool,
                &arguments,
            )
            .await
            .map_err(|error| error.to_string())?;
            Ok(ExecutorOutput {
                preview: result.body.clone(),
                full: FullOutput::Inline(result.body),
                is_error: result.is_error,
            })
        })
    }
}
