//! Production remote MCP egress and session-secret custody adapters.

use std::num::{NonZeroU64, NonZeroUsize};
use std::sync::Arc;

use aex_brain_mcp::pool::{
    ConnectionPool, PooledClient, ServerRevision, TenantScope, screened_client,
};
use aex_secret_aws::{SealedSecret, SecretCrypto};
use aex_secret_custody_dynamodb::{CustodyStore, SecretCustodyStore};
use aex_secret_domain::{EncryptionContext, SecretState, SourceGeneration};
use aex_tool_mux::{ToolCallIdentity, ToolMuxFuture};
use aex_wire::ids::{ContentHash, ResourceName};

use crate::mcp::{McpSecretReader, QualifiedMcpClientPort};

const MCP_SECRET_FRAME_V1: u8 = 0x01;

/// DNS-screened, tenant/revision partitioned rmcp client authority.
pub struct ProductionQualifiedMcpClients {
    pool: ConnectionPool,
}

impl ProductionQualifiedMcpClients {
    /// Builds the bounded per-process connection pool.
    #[must_use]
    pub fn new() -> Self {
        Self {
            pool: ConnectionPool::new(NonZeroUsize::new(64).expect("positive")),
        }
    }
}

impl Default for ProductionQualifiedMcpClients {
    fn default() -> Self {
        Self::new()
    }
}

impl QualifiedMcpClientPort for ProductionQualifiedMcpClients {
    fn acquire<'a>(
        &'a self,
        server: &'a ResourceName,
        endpoint: &'a str,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<PooledClient, String>> {
        Box::pin(async move {
            screened_client(
                &self.pool,
                &aex_brain_managed_web::egress::SystemDnsResolver,
                TenantScope {
                    organization: call.organization,
                    workspace: call.workspace,
                },
                ServerRevision {
                    server: server.clone(),
                    revision: NonZeroU64::new(1).expect("positive"),
                    secret_generation: NonZeroU64::new(1).expect("positive"),
                    manifest_digest: ContentHash::of(endpoint.as_bytes()),
                },
                endpoint,
            )
            .await
            .map_err(|_| "remote MCP endpoint failed egress screening".to_owned())
        })
    }
}

/// Exact-context, purpose-framed MCP secret reader.
pub struct ProductionMcpSecrets {
    custody: Arc<CustodyStore>,
    crypto: Arc<aex_secret_aws::EnvelopeCrypto>,
    plane: aex_secret_domain::context::Plane,
    region: aex_wire::types::Region,
}

impl ProductionMcpSecrets {
    /// Binds the regional custody and envelope authorities.
    #[must_use]
    pub const fn new(
        custody: Arc<CustodyStore>,
        crypto: Arc<aex_secret_aws::EnvelopeCrypto>,
        plane: aex_secret_domain::context::Plane,
        region: aex_wire::types::Region,
    ) -> Self {
        Self {
            custody,
            crypto,
            plane,
            region,
        }
    }
}

impl core::fmt::Debug for ProductionMcpSecrets {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("ProductionMcpSecrets(<redacted>)")
    }
}

impl McpSecretReader for ProductionMcpSecrets {
    fn reveal<'a>(
        &'a self,
        name: &'a ResourceName,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<zeroize::Zeroizing<String>, String>> {
        Box::pin(async move {
            let (metadata, generation) = tokio::join!(
                self.custody.load_secret(call.workspace, name),
                self.custody
                    .load_generation(call.workspace, name, SourceGeneration::FIRST),
            );
            let metadata = metadata
                .map_err(|_| "MCP secret metadata unavailable".to_owned())?
                .ok_or_else(|| "MCP secret is absent".to_owned())?;
            let generation = generation
                .map_err(|_| "MCP secret generation unavailable".to_owned())?
                .ok_or_else(|| "MCP secret generation is absent".to_owned())?;
            if metadata.workspace != call.workspace
                || metadata.name != *name
                || metadata.state != SecretState::Ready
                || metadata.generation != SourceGeneration::FIRST
                || generation.workspace != call.workspace
                || generation.name != *name
                || generation.generation != SourceGeneration::FIRST
                || generation.revoked_at.is_some()
            {
                return Err("MCP secret is not ready".to_owned());
            }
            let context = EncryptionContext {
                plane: self.plane,
                region: self.region,
                organization: call.organization,
                workspace: call.workspace,
                name: name.clone(),
                generation: SourceGeneration::FIRST,
                custody_revision: None,
            };
            if context.digest() != generation.context_digest {
                return Err("MCP secret context is invalid".to_owned());
            }
            let plaintext = self
                .crypto
                .reveal(
                    &SealedSecret {
                        frame: generation.ciphertext.ciphertext,
                        context_digest: generation.context_digest,
                        wrapped_branch_key: generation.ciphertext.wrapped_key,
                    },
                    &context,
                    aex_wire::types::Timestamp::from_unix_millis(now_millis()?)
                        .map_err(|_| "MCP secret clock is invalid".to_owned())?,
                )
                .await
                .map_err(|_| "MCP secret could not be revealed".to_owned())?;
            let Some((&MCP_SECRET_FRAME_V1, value)) =
                plaintext.expose_for_encryption().split_first()
            else {
                return Err("MCP secret purpose frame is invalid".to_owned());
            };
            let value =
                core::str::from_utf8(value).map_err(|_| "MCP secret is not UTF-8".to_owned())?;
            Ok(zeroize::Zeroizing::new(value.to_owned()))
        })
    }
}

fn now_millis() -> Result<i64, String> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "system clock precedes the unix epoch".to_owned())?;
    i64::try_from(elapsed.as_millis())
        .map_err(|_| "system clock exceeds timestamp range".to_owned())
}
