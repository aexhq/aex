//! Frozen session MCP configuration with secret values replaced by custody names.

use std::collections::BTreeMap;

use aex_wire::ids::{ContentHash, ResourceName, SessionId};
use serde::{Deserialize, Serialize};

/// One frozen server safe to persist in the Brain root configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenMcpServer {
    /// Stable server namespace.
    pub name: ResourceName,
    /// Frozen non-secret transport and secret references.
    pub transport: FrozenMcpTransport,
}

/// Frozen MCP transport. Plaintext headers and environment values cannot enter it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FrozenMcpTransport {
    /// Remote Streamable HTTP.
    RemoteHttp {
        /// Frozen HTTPS endpoint.
        endpoint: String,
        /// Header name to distinct custody secret name.
        headers: BTreeMap<String, ResourceName>,
    },
    /// Process inside the exact sandbox generation.
    SandboxProcess {
        /// Executable inside the sandbox.
        command: String,
        /// Bounded arguments.
        args: Vec<String>,
        /// Environment name to distinct custody secret name.
        environment: BTreeMap<String, ResourceName>,
        /// Normalized working directory.
        working_directory: Option<String>,
    },
}

/// Derives the hidden custody name for one MCP header or environment value.
/// The original key name is intentionally not retained in the secret name.
#[must_use]
pub fn mcp_secret_name(
    session: SessionId,
    server: &ResourceName,
    class: &str,
    key: &str,
) -> ResourceName {
    let digest = ContentHash::of(
        format!("aex.session.mcp.secret.v1\0{session}\0{server}\0{class}\0{key}").as_bytes(),
    );
    let encoded = digest.to_string();
    ResourceName::parse(&format!("mcp-{}", &encoded[7..39]))
        .unwrap_or_else(|_| unreachable!("fixed lowercase hex is a resource name"))
}
