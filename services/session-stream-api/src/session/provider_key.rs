//! Write-only, session-scoped provider-key custody.
//!
//! The public create request carries the only plaintext copy. This adapter
//! converts it immediately into [`SecretPlaintext`], seals it with the existing
//! workspace branch-key/KMS envelope, and atomically commits the hidden source
//! rows, the Brain-readable binding and a replay receipt. No public credential
//! resource or lifecycle is mounted.

use std::sync::Arc;

use aex_secret_aws::{SealedSecret, SecretCrypto};
use aex_secret_custody_dynamodb::codec::{
    CredentialState, ProviderCredential, SecretMetadata, StoredGeneration,
};
use aex_secret_custody_dynamodb::expressions;
use aex_secret_custody_dynamodb::store::SecretCustodyStore;
use aex_secret_domain::context::{EncryptionContext, Plane};
use aex_secret_domain::plaintext::SecretPlaintext;
use aex_secret_domain::revocation::RevocationEpoch;
use aex_secret_domain::secret::{SecretRevision, SecretState, SourceGeneration};
use aex_secret_keystore_dynamodb::provision::BranchKeyAuthority;
use aex_session_app::ports::{
    CredentialState as AppCredentialState, PortError, ProviderCredentialBinding,
    ProviderCredentialReader,
};
use aex_session_domain::IdempotencyIdentity;
use aex_session_dynamodb::error::{RetryPolicy, StoreError};
use aex_session_dynamodb::plan::Participant;
use aex_session_dynamodb::replay::{
    Backoff, DecodeReceipt, IdempotencyScope, RECEIPT_RETENTION, Receipt, ReceiptBody,
    ReceiptStore, ReplayRequest, commit_or_replay, key_digest,
};
use aex_wire::ids::{OrganizationId, ProviderCredentialId, ResourceName, SessionId, WorkspaceId};
use aex_wire::models::{McpServer, McpTransport};
use aex_wire::provider::ProviderId;
use aex_wire::types::{Region, Timestamp};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};

const RECEIPT_KIND: &str = "session_provider_key";
const MCP_RECEIPT_KIND: &str = "session_mcp_secret";
const MCP_SECRET_FRAME_V1: u8 = 0x01;

/// The only production implementation of session provider-key admission.
pub struct SessionProviderKeys {
    custody: Arc<dyn SecretCustodyStore>,
    custody_table: String,
    crypto: Arc<dyn SecretCrypto>,
    branch_keys: Arc<dyn BranchKeyAuthority>,
    plane: Plane,
    region: Region,
}

impl std::fmt::Debug for SessionProviderKeys {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SessionProviderKeys")
            .field("custody_table", &self.custody_table)
            .field("plane", &self.plane)
            .field("region", &self.region)
            .finish_non_exhaustive()
    }
}

impl SessionProviderKeys {
    /// Composes the existing regional custody, envelope and branch-key
    /// authorities into the narrow session-create writer.
    #[must_use]
    pub fn new(
        custody: Arc<dyn SecretCustodyStore>,
        custody_table: impl Into<String>,
        crypto: Arc<dyn SecretCrypto>,
        branch_keys: Arc<dyn BranchKeyAuthority>,
        plane: Plane,
        region: Region,
    ) -> Self {
        Self {
            custody,
            custody_table: custody_table.into(),
            crypto,
            branch_keys,
            plane,
            region,
        }
    }

    async fn seal(
        &self,
        organization: OrganizationId,
        workspace: WorkspaceId,
        name: &ResourceName,
        plaintext: &SecretPlaintext,
        now: Timestamp,
        kind: &'static str,
    ) -> Result<StoredGeneration, PortError> {
        let context = EncryptionContext {
            plane: self.plane,
            region: self.region,
            organization,
            workspace,
            name: name.clone(),
            generation: SourceGeneration::FIRST,
            custody_revision: None,
        };
        let branch_key = self
            .branch_keys
            .active_or_create(workspace)
            .await
            .map_err(|_| PortError::Unavailable { kind })?;
        let sealed = self
            .crypto
            .seal(&context, &branch_key.wrapped_material, plaintext, now)
            .await
            .map_err(|_| PortError::Unavailable { kind })?;
        Ok(StoredGeneration {
            workspace,
            name: name.clone(),
            generation: SourceGeneration::FIRST,
            ciphertext: sealed.to_ciphertext_ref(branch_key.hierarchy_version),
            context_digest: sealed.context_digest,
            created_at: now,
            revoked_at: None,
        })
    }

    async fn bind_mcp_secret(
        &self,
        workspace: WorkspaceId,
        organization: OrganizationId,
        secret_name: ResourceName,
        value: &str,
        identity: &IdempotencyIdentity,
        now: Timestamp,
    ) -> Result<(), PortError> {
        let key = identity.key().ok_or(PortError::Corrupt {
            kind: "session MCP secret",
            reason: "session create requires an idempotency key",
        })?;
        let scope = IdempotencyScope::new("session.mcp_secret", Some(secret_name.as_str()))
            .map_err(|_| PortError::Corrupt {
                kind: "session MCP secret",
                reason: "the derived MCP secret cannot enter the replay scope",
            })?;

        // Each value has its own subject-qualified receipt. A retry resolves the
        // receipt before branch-key/KMS work, including after a partially completed
        // multi-secret create, so already committed values are never resealed.
        if let Some(receipt) = self
            .custody
            .load_receipt(workspace, &scope.render(), &key_digest(key), now)
            .await
            .map_err(|error| mcp_store_error(&error))?
        {
            if receipt.intent != identity.intent() {
                return Err(PortError::IdempotencyConflict);
            }
            McpSecretReceipt::decode_receipt(&receipt).map_err(|error| mcp_store_error(&error))?;
            return Ok(());
        }

        // MCP values are purpose/version framed. Besides separating them from raw
        // provider keys, the prefix lets an admitted empty environment value enter
        // SecretPlaintext, whose empty byte string is deliberately invalid.
        let mut framed = Vec::with_capacity(value.len().saturating_add(1));
        framed.push(MCP_SECRET_FRAME_V1);
        framed.extend_from_slice(value.as_bytes());
        let plaintext = SecretPlaintext::new(framed).map_err(|_| PortError::Corrupt {
            kind: "session MCP secret",
            reason: "the validated MCP value is outside the secret plaintext bounds",
        })?;
        let generation = self
            .seal(
                organization,
                workspace,
                &secret_name,
                &plaintext,
                now,
                "session MCP secret",
            )
            .await?;
        drop(plaintext);

        let metadata = SecretMetadata {
            workspace,
            name: secret_name.clone(),
            generation: SourceGeneration::FIRST,
            revision: SecretRevision::FIRST,
            state: SecretState::Ready,
            revocation_epoch: RevocationEpoch::INITIAL,
            revoked_through_revision: SecretRevision(0),
            created_at: now,
            updated_at: now,
            revoked_at: None,
        };
        let receipt = Receipt {
            scope: scope.render(),
            key_sha256: key_digest(key),
            intent: identity.intent(),
            response_kind: MCP_RECEIPT_KIND.to_owned(),
            response: ReceiptBody::Inline(Vec::new()),
            committed_at: now,
            expires_at: receipt_expiry(now, "session MCP secret")?,
        };
        let receipts = CustodyReceipts(self.custody.as_ref());
        commit_or_replay::<McpSecretReceipt, _, _>(
            &receipts,
            &YieldBackoff,
            ReplayRequest {
                workspace,
                scope,
                key,
                intent: identity.intent(),
                receipt_participant: Participant::SECRET_IDEMPOTENCY,
                policy: RetryPolicy::PINNED,
                now,
            },
            || async {
                let plan = expressions::set(
                    &self.custody_table,
                    &generation,
                    &metadata,
                    None,
                    Some(&receipt),
                    None,
                )?;
                self.custody.commit(&plan).await?;
                Ok(McpSecretReceipt)
            },
        )
        .await
        .map_err(|error| mcp_store_error(&error))?;
        Ok(())
    }
}

/// Read-only create-time boundary for session-frozen MCP transport values.
/// Plaintext remains zeroizing and exists only through one qualification call.
pub trait SessionMcpSecretReader: Send + Sync + 'static {
    /// Reveals and purpose-unframes one exact session MCP secret.
    fn reveal_mcp_secret<'a>(
        &'a self,
        organization: OrganizationId,
        workspace: WorkspaceId,
        name: &'a aex_secret_domain::SecretName,
        now: Timestamp,
    ) -> BoxFuture<'a, Result<zeroize::Zeroizing<String>, PortError>>;
}

impl SessionMcpSecretReader for SessionProviderKeys {
    fn reveal_mcp_secret<'a>(
        &'a self,
        organization: OrganizationId,
        workspace: WorkspaceId,
        name: &'a aex_secret_domain::SecretName,
        now: Timestamp,
    ) -> BoxFuture<'a, Result<zeroize::Zeroizing<String>, PortError>> {
        Box::pin(async move {
            let (metadata, generation) = tokio::join!(
                self.custody.load_secret(workspace, name),
                self.custody
                    .load_generation(workspace, name, SourceGeneration::FIRST),
            );
            let metadata = metadata
                .map_err(|_| PortError::Unavailable { kind: "MCP secret" })?
                .ok_or(PortError::NotFound { kind: "MCP secret" })?;
            let generation = generation
                .map_err(|_| PortError::Unavailable { kind: "MCP secret" })?
                .ok_or(PortError::NotFound { kind: "MCP secret" })?;
            if metadata.workspace != workspace
                || metadata.name != *name
                || metadata.state != SecretState::Ready
                || metadata.generation != SourceGeneration::FIRST
                || generation.workspace != workspace
                || generation.name != *name
                || generation.generation != SourceGeneration::FIRST
                || generation.revoked_at.is_some()
            {
                return Err(PortError::Corrupt {
                    kind: "MCP secret",
                    reason: "state or identity mismatch",
                });
            }
            let context = EncryptionContext {
                plane: self.plane,
                region: self.region,
                organization,
                workspace,
                name: name.clone(),
                generation: SourceGeneration::FIRST,
                custody_revision: None,
            };
            if context.digest() != generation.context_digest {
                return Err(PortError::Corrupt {
                    kind: "MCP secret",
                    reason: "encryption context mismatch",
                });
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
                    now,
                )
                .await
                .map_err(|_| PortError::Unavailable { kind: "MCP secret" })?;
            let Some((&MCP_SECRET_FRAME_V1, value)) =
                plaintext.expose_for_encryption().split_first()
            else {
                return Err(PortError::Corrupt {
                    kind: "MCP secret",
                    reason: "purpose frame mismatch",
                });
            };
            let value = core::str::from_utf8(value).map_err(|_| PortError::Corrupt {
                kind: "MCP secret",
                reason: "plaintext is not UTF-8",
            })?;
            Ok(zeroize::Zeroizing::new(value.to_owned()))
        })
    }
}

#[derive(Clone, Copy)]
struct McpSecretReceipt;

impl DecodeReceipt for McpSecretReceipt {
    fn decode_receipt(receipt: &Receipt) -> Result<Self, StoreError> {
        if receipt.response_kind != MCP_RECEIPT_KIND
            || !matches!(&receipt.response, ReceiptBody::Inline(bytes) if bytes.is_empty())
        {
            return Err(StoreError::Invalid {
                detail: "the session MCP-secret receipt is malformed".to_owned(),
            });
        }
        Ok(Self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct BoundProviderKey {
    credential: ProviderCredentialId,
    provider: ProviderId,
    source_generation: u64,
    revision: u64,
}

impl From<BoundProviderKey> for ProviderCredentialBinding {
    fn from(value: BoundProviderKey) -> Self {
        Self {
            credential: value.credential,
            provider: value.provider,
            source_generation: value.source_generation,
            revision: value.revision,
            state: AppCredentialState::Ready,
        }
    }
}

impl DecodeReceipt for BoundProviderKey {
    fn decode_receipt(receipt: &Receipt) -> Result<Self, StoreError> {
        if receipt.response_kind != RECEIPT_KIND {
            return Err(StoreError::Invalid {
                detail: "the session provider-key receipt has another response kind".to_owned(),
            });
        }
        let ReceiptBody::Inline(bytes) = &receipt.response else {
            return Err(StoreError::Invalid {
                detail: "the bounded provider-key receipt must be inline".to_owned(),
            });
        };
        serde_json::from_slice(bytes).map_err(|error| StoreError::Invalid {
            detail: format!("the session provider-key receipt is malformed: {error}"),
        })
    }
}

struct CustodyReceipts<'a>(&'a dyn SecretCustodyStore);

#[async_trait::async_trait]
impl ReceiptStore for CustodyReceipts<'_> {
    async fn read_receipt(
        &self,
        workspace: WorkspaceId,
        scope: &IdempotencyScope<'_>,
        key: &aex_wire::idempotency::IdempotencyKey,
        now: Timestamp,
    ) -> Result<Option<Receipt>, StoreError> {
        self.0
            .load_receipt(workspace, &scope.render(), &key_digest(key), now)
            .await
    }
}

struct YieldBackoff;

#[async_trait::async_trait]
impl Backoff for YieldBackoff {
    async fn wait(&self, _policy: RetryPolicy, _attempt: u32) {
        tokio::task::yield_now().await;
    }
}

#[async_trait::async_trait]
impl ProviderCredentialReader for SessionProviderKeys {
    async fn bind_session_api_key(
        &self,
        request: aex_session_app::ports::BindSessionApiKey<'_>,
    ) -> Result<ProviderCredentialBinding, PortError> {
        let aex_session_app::ports::BindSessionApiKey {
            workspace,
            organization,
            credential,
            provider,
            api_key,
            identity,
            now,
        } = request;
        let key = identity.key().ok_or(PortError::Corrupt {
            kind: "session provider key",
            reason: "session create requires an idempotency key",
        })?;
        let scope = IdempotencyScope::new("session.provider_key", None).map_err(|_| {
            PortError::Corrupt {
                kind: "session provider key",
                reason: "the compiled replay scope is invalid",
            }
        })?;

        // The normal retry path must not ask KMS to seal another envelope. Apart from
        // wasting a comparatively expensive call, sealing before replay resolution leaves
        // two ciphertexts for one logical key in memory and makes the write-only boundary
        // harder to audit. `commit_or_replay` repeats this read after a raced commit, so the
        // remaining seal-before-commit window is only the unavoidable concurrent-writer
        // case; exactly one ciphertext can still become durable.
        if let Some(receipt) = self
            .custody
            .load_receipt(workspace, &scope.render(), &key_digest(key), now)
            .await
            .map_err(|error| store_error(&error))?
        {
            if receipt.intent != identity.intent() {
                return Err(PortError::IdempotencyConflict);
            }
            return BoundProviderKey::decode_receipt(&receipt)
                .map(Into::into)
                .map_err(|error| store_error(&error));
        }

        let plaintext =
            SecretPlaintext::new(api_key.as_bytes().to_vec()).map_err(|_| PortError::Corrupt {
                kind: "session provider key",
                reason: "the validated API key is outside the secret plaintext bounds",
            })?;
        let secret_name =
            ResourceName::parse(&credential.to_string()).map_err(|_| PortError::Corrupt {
                kind: "session provider key",
                reason: "the credential identity cannot enter the hidden secret namespace",
            })?;
        let fingerprint = plaintext.credential_fingerprint(workspace, credential);
        let generation = self
            .seal(
                organization,
                workspace,
                &secret_name,
                &plaintext,
                now,
                "session provider key",
            )
            .await?;
        drop(plaintext);

        let metadata = SecretMetadata {
            workspace,
            name: secret_name.clone(),
            generation: SourceGeneration::FIRST,
            revision: SecretRevision::FIRST,
            state: SecretState::Ready,
            revocation_epoch: RevocationEpoch::INITIAL,
            revoked_through_revision: SecretRevision(0),
            created_at: now,
            updated_at: now,
            revoked_at: None,
        };
        let stored = ProviderCredential {
            credential,
            workspace,
            name: secret_name.clone(),
            provider,
            secret_name,
            source_generation: SourceGeneration::FIRST,
            fingerprint,
            revision: 1,
            state: CredentialState::Ready,
            created_at: now,
            updated_at: now,
            revoked_at: None,
        };
        let response = BoundProviderKey {
            credential,
            provider,
            source_generation: SourceGeneration::FIRST.0,
            revision: 1,
        };
        let response_bytes =
            aex_wire::canonical::to_jcs_bytes(&response).map_err(|_| PortError::Corrupt {
                kind: "session provider key",
                reason: "the binding receipt could not be canonicalized",
            })?;
        let receipt = Receipt {
            scope: scope.render(),
            key_sha256: key_digest(key),
            intent: identity.intent(),
            response_kind: RECEIPT_KIND.to_owned(),
            response: ReceiptBody::Inline(response_bytes),
            committed_at: now,
            expires_at: receipt_expiry(now, "session provider key")?,
        };
        let receipts = CustodyReceipts(self.custody.as_ref());
        let result = commit_or_replay::<BoundProviderKey, _, _>(
            &receipts,
            &YieldBackoff,
            ReplayRequest {
                workspace,
                scope,
                key,
                intent: identity.intent(),
                receipt_participant: Participant::SECRET_IDEMPOTENCY,
                policy: RetryPolicy::PINNED,
                now,
            },
            || async {
                let plan = expressions::bind_session_provider_key(
                    &self.custody_table,
                    &generation,
                    &metadata,
                    &stored,
                    &receipt,
                )?;
                self.custody.commit(&plan).await?;
                Ok(response.clone())
            },
        )
        .await
        .map_err(|error| store_error(&error))?;
        Ok(result.into_inner().into())
    }

    async fn bind_session_mcp_config(
        &self,
        workspace: WorkspaceId,
        organization: OrganizationId,
        session: SessionId,
        servers: &[McpServer],
        identity: &IdempotencyIdentity,
        now: Timestamp,
    ) -> Result<(), PortError> {
        for server in servers {
            match &server.transport {
                McpTransport::RemoteHttp(remote) => {
                    for (key, value) in remote.headers.as_ref().into_iter().flatten() {
                        self.bind_mcp_secret(
                            workspace,
                            organization,
                            aex_brain_domain::mcp::mcp_secret_name(
                                session,
                                &server.name,
                                "header",
                                key,
                            ),
                            value,
                            identity,
                            now,
                        )
                        .await?;
                    }
                }
                McpTransport::SandboxProcess(process) => {
                    for (key, value) in process.environment.as_ref().into_iter().flatten() {
                        self.bind_mcp_secret(
                            workspace,
                            organization,
                            aex_brain_domain::mcp::mcp_secret_name(
                                session,
                                &server.name,
                                "environment",
                                key,
                            ),
                            value,
                            identity,
                            now,
                        )
                        .await?;
                    }
                }
            }
        }
        Ok(())
    }

    async fn read_provider_credential(
        &self,
        _workspace: WorkspaceId,
        _credential: ProviderCredentialId,
    ) -> Result<Option<ProviderCredentialBinding>, PortError> {
        Err(PortError::Unowned {
            kind: "public provider credential",
            seam: "session credentials are write-only and have no public read surface",
        })
    }
}

fn receipt_expiry(now: Timestamp, kind: &'static str) -> Result<Timestamp, PortError> {
    let retention_ms = i64::try_from(RECEIPT_RETENTION.as_millis()).unwrap_or(i64::MAX);
    Timestamp::from_unix_millis(now.unix_millis().saturating_add(retention_ms)).map_err(|_| {
        PortError::Corrupt {
            kind,
            reason: "the replay receipt expiry is not representable",
        }
    })
}

fn store_error(error: &StoreError) -> PortError {
    match error {
        StoreError::IdempotencyConflict => PortError::IdempotencyConflict,
        StoreError::Throttled { .. } | StoreError::Contended => PortError::Throttled {
            kind: "session provider key",
        },
        StoreError::Corrupt(_) | StoreError::Invalid { .. } => PortError::Corrupt {
            kind: "session provider key",
            reason: "the encrypted binding or its replay receipt is malformed",
        },
        _ => PortError::Unavailable {
            kind: "session provider key",
        },
    }
}

fn mcp_store_error(error: &StoreError) -> PortError {
    match error {
        StoreError::IdempotencyConflict => PortError::IdempotencyConflict,
        StoreError::Throttled { .. } | StoreError::Contended => PortError::Throttled {
            kind: "session MCP secret",
        },
        StoreError::Corrupt(_) | StoreError::Invalid { .. } => PortError::Corrupt {
            kind: "session MCP secret",
            reason: "the encrypted MCP value or its replay receipt is malformed",
        },
        _ => PortError::Unavailable {
            kind: "session MCP secret",
        },
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use aex_secret_aws::{SealedSecret, SecretCryptoError};
    use aex_secret_custody_dynamodb::store::Page;
    use aex_secret_domain::SecretName;
    use aex_secret_keystore_dynamodb::provision::ProvisionError;
    use aex_secret_keystore_dynamodb::{ActiveBranchKey, BranchKeyId};
    use aex_session_app::ports::BindSessionApiKey;
    use aex_session_dynamodb::attr::{ITEM_TYPE, Item};
    use aex_session_dynamodb::paging::{PageBudget, PagePosition};
    use aex_session_dynamodb::plan::TransactionPlan;
    use aex_wire::idempotency::{IdempotencyKey, IntentDigest, PrincipalScope, ReplayIdentity};
    use aex_wire::ids::{ApiKeyId, Uuid7};
    use aex_wire::models::{RemoteMcpServer, SandboxMcpServer};
    use aex_wire::routes::RouteId;
    use aex_wire::types::HttpsUrl;

    use super::*;

    const PLAINTEXT: &str = "sk-session-round-trip";

    fn sample<I: aex_wire::ids::PrefixedId>(tag: u8) -> I {
        I::from_uuid7(Uuid7::compose(1_800_000_000_000, [tag; 10]))
    }

    fn workspace() -> WorkspaceId {
        sample(1)
    }

    fn organization() -> OrganizationId {
        sample(2)
    }

    fn credential(tag: u8) -> ProviderCredentialId {
        sample(tag)
    }

    fn test_binding(
        credential: ProviderCredentialId,
        identity: &'static IdempotencyIdentity,
    ) -> BindSessionApiKey<'static> {
        test_binding_with_key(credential, identity, PLAINTEXT)
    }

    fn test_binding_with_key(
        credential: ProviderCredentialId,
        identity: &'static IdempotencyIdentity,
        api_key: &'static str,
    ) -> BindSessionApiKey<'static> {
        BindSessionApiKey {
            workspace: workspace(),
            organization: organization(),
            credential,
            provider: ProviderId::Openai,
            api_key,
            identity,
            now: now(),
        }
    }

    fn identity_binding(intent: u8) -> &'static IdempotencyIdentity {
        Box::leak(Box::new(identity(intent)))
    }

    fn session() -> SessionId {
        sample(7)
    }

    fn now() -> Timestamp {
        Timestamp::from_unix_millis(1_800_000_000_000).expect("time")
    }

    fn identity(intent: u8) -> IdempotencyIdentity {
        IdempotencyIdentity::Key(Box::new(ReplayIdentity {
            principal: PrincipalScope::WorkspaceKey {
                key: sample::<ApiKeyId>(9),
                workspace: workspace(),
                organization: organization(),
            },
            route: RouteId::SessionCreate,
            key: IdempotencyKey::parse("session-key-replay").expect("key"),
            intent: IntentDigest::from_bytes([intent; 32]),
        }))
    }

    #[derive(Debug, Default)]
    struct MemoryCustody {
        rows: Mutex<Vec<Item>>,
        commits: Mutex<usize>,
    }

    impl MemoryCustody {
        fn rows(&self) -> Vec<Item> {
            self.rows.lock().expect("rows").clone()
        }

        fn commits(&self) -> usize {
            *self.commits.lock().expect("commits")
        }

        fn kind(item: &Item) -> Option<&str> {
            item.get(ITEM_TYPE)?.as_s().ok().map(String::as_str)
        }

        fn generation(&self) -> StoredGeneration {
            self.rows()
                .iter()
                .find(|item| {
                    Self::kind(item)
                        == Some(aex_secret_custody_dynamodb::codec::SECRET_SOURCE_GENERATION)
                })
                .map(|item| {
                    aex_secret_custody_dynamodb::codec::decode_generation(item, workspace())
                        .expect("stored generation")
                })
                .expect("generation row")
        }

        fn generation_named(&self, name: &ResourceName) -> StoredGeneration {
            self.rows()
                .iter()
                .filter(|item| {
                    Self::kind(item)
                        == Some(aex_secret_custody_dynamodb::codec::SECRET_SOURCE_GENERATION)
                })
                .map(|item| {
                    aex_secret_custody_dynamodb::codec::decode_generation(item, workspace())
                        .expect("stored generation")
                })
                .find(|generation| generation.name == *name)
                .expect("named generation row")
        }

        fn receipts(&self) -> Vec<Receipt> {
            self.rows()
                .iter()
                .filter(|item| {
                    Self::kind(item) == Some(aex_session_dynamodb::replay::IDEMPOTENCY_RECEIPT)
                })
                .map(|item| {
                    aex_session_dynamodb::replay::decode_receipt_row(item).expect("receipt")
                })
                .collect()
        }
    }

    #[async_trait::async_trait]
    impl SecretCustodyStore for MemoryCustody {
        async fn load_secret(
            &self,
            workspace: WorkspaceId,
            _name: &SecretName,
        ) -> Result<Option<SecretMetadata>, StoreError> {
            self.rows()
                .iter()
                .find(|item| {
                    Self::kind(item) == Some(aex_secret_custody_dynamodb::codec::WORKSPACE_SECRET)
                })
                .map(|item| {
                    aex_secret_custody_dynamodb::codec::decode_secret(item, workspace)
                        .map_err(StoreError::from)
                })
                .transpose()
        }

        async fn list_secrets(
            &self,
            _workspace: WorkspaceId,
            _budget: PageBudget,
        ) -> Result<Vec<SecretMetadata>, StoreError> {
            unreachable!("the writer never lists")
        }

        async fn load_generation(
            &self,
            workspace: WorkspaceId,
            _name: &SecretName,
            _generation: SourceGeneration,
        ) -> Result<Option<StoredGeneration>, StoreError> {
            self.rows()
                .iter()
                .find(|item| {
                    Self::kind(item)
                        == Some(aex_secret_custody_dynamodb::codec::SECRET_SOURCE_GENERATION)
                })
                .map(|item| {
                    aex_secret_custody_dynamodb::codec::decode_generation(item, workspace)
                        .map_err(StoreError::from)
                })
                .transpose()
        }

        async fn load_custody(
            &self,
            _workspace: WorkspaceId,
            _session: aex_wire::ids::SessionId,
        ) -> Result<Option<aex_secret_custody_dynamodb::codec::CustodyHead>, StoreError> {
            unreachable!("the writer never reads session custody")
        }

        async fn list_provider_credentials(
            &self,
            _workspace: WorkspaceId,
            _budget: PageBudget,
        ) -> Result<Vec<ProviderCredential>, StoreError> {
            unreachable!("session keys have no public list")
        }

        async fn load_provider_credential(
            &self,
            workspace: WorkspaceId,
            _credential: ProviderCredentialId,
        ) -> Result<Option<ProviderCredential>, StoreError> {
            self.rows()
                .iter()
                .find(|item| {
                    Self::kind(item)
                        == Some(aex_secret_custody_dynamodb::codec::PROVIDER_CREDENTIAL)
                })
                .map(|item| {
                    aex_secret_custody_dynamodb::codec::decode_provider_credential(item, workspace)
                        .map_err(StoreError::from)
                })
                .transpose()
        }

        async fn page_secrets(
            &self,
            _workspace: WorkspaceId,
            _budget: PageBudget,
            _after: Option<&PagePosition>,
        ) -> Result<Page<SecretMetadata>, StoreError> {
            unreachable!("the writer never pages")
        }

        async fn page_provider_credentials(
            &self,
            _workspace: WorkspaceId,
            _budget: PageBudget,
            _after: Option<&PagePosition>,
        ) -> Result<Page<ProviderCredential>, StoreError> {
            unreachable!("session keys have no public page")
        }

        async fn load_receipt(
            &self,
            _workspace: WorkspaceId,
            scope: &str,
            key_sha256_hex: &str,
            _now: Timestamp,
        ) -> Result<Option<Receipt>, StoreError> {
            self.rows()
                .iter()
                .filter(|item| {
                    Self::kind(item) == Some(aex_session_dynamodb::replay::IDEMPOTENCY_RECEIPT)
                })
                .map(|item| {
                    aex_session_dynamodb::replay::decode_receipt_row(item).map_err(StoreError::from)
                })
                .find_map(|result| match result {
                    Ok(receipt)
                        if receipt.scope == scope && receipt.key_sha256 == key_sha256_hex =>
                    {
                        Some(Ok(receipt))
                    }
                    Ok(_) => None,
                    Err(error) => Some(Err(error)),
                })
                .transpose()
        }

        async fn commit(&self, plan: &TransactionPlan) -> Result<(), StoreError> {
            let mut rows = self.rows.lock().expect("rows");
            rows.extend(
                plan.actions()
                    .iter()
                    .filter_map(|action| action.put().map(|put| put.item().clone())),
            );
            *self.commits.lock().expect("commits") += 1;
            Ok(())
        }

        async fn commit_update(
            &self,
            _builder: aws_sdk_dynamodb::types::builders::UpdateBuilder,
            _participant: Participant,
        ) -> Result<(), StoreError> {
            unreachable!("the writer commits one transaction")
        }
    }

    #[derive(Debug)]
    struct TestBranchKeys;

    #[async_trait::async_trait]
    impl BranchKeyAuthority for TestBranchKeys {
        async fn active_or_create(
            &self,
            workspace: WorkspaceId,
        ) -> Result<ActiveBranchKey, ProvisionError> {
            Ok(ActiveBranchKey {
                branch_key_id: BranchKeyId::of(workspace),
                version: Some("branch:version:test".to_owned()),
                create_time: "2027-01-15T08:00:00Z".to_owned(),
                kms_arn: "arn:aws:kms:eu-west-1:000000000000:key/test".to_owned(),
                hierarchy_version: 1,
                wrapped_material: vec![0x44; 32],
            })
        }
    }

    #[derive(Debug, Default)]
    struct TestCrypto {
        seals: AtomicUsize,
    }

    impl TestCrypto {
        fn seals(&self) -> usize {
            self.seals.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl SecretCrypto for TestCrypto {
        async fn seal(
            &self,
            context: &EncryptionContext,
            wrapped_branch_key: &[u8],
            plaintext: &SecretPlaintext,
            _now: Timestamp,
        ) -> Result<SealedSecret, SecretCryptoError> {
            self.seals.fetch_add(1, Ordering::SeqCst);
            Ok(SealedSecret {
                frame: plaintext
                    .expose_for_encryption()
                    .iter()
                    .map(|byte| byte ^ 0xaa)
                    .collect(),
                context_digest: context.digest(),
                wrapped_branch_key: wrapped_branch_key.to_vec(),
            })
        }

        async fn rewrap(
            &self,
            _sealed: &SealedSecret,
            _from: &EncryptionContext,
            _to: &EncryptionContext,
            _now: Timestamp,
        ) -> Result<SealedSecret, SecretCryptoError> {
            unreachable!("the first seal never rewraps")
        }

        async fn reveal(
            &self,
            sealed: &SealedSecret,
            context: &EncryptionContext,
            _now: Timestamp,
        ) -> Result<SecretPlaintext, SecretCryptoError> {
            if sealed.context_digest != context.digest() {
                return Err(SecretCryptoError::ContextMismatch);
            }
            SecretPlaintext::new(sealed.frame.iter().map(|byte| byte ^ 0xaa).collect())
                .map_err(|_| SecretCryptoError::Plaintext)
        }
    }

    fn writer(custody: Arc<MemoryCustody>) -> (SessionProviderKeys, Arc<TestCrypto>) {
        let crypto = Arc::new(TestCrypto::default());
        (
            SessionProviderKeys::new(
                custody,
                "regional-secret-custody",
                Arc::clone(&crypto) as Arc<dyn SecretCrypto>,
                Arc::new(TestBranchKeys),
                Plane::Dev,
                Region::EuWest1,
            ),
            crypto,
        )
    }

    #[tokio::test]
    async fn session_key_round_trips_only_through_ciphertext_and_replays_the_binding() {
        let custody = Arc::new(MemoryCustody::default());
        let (writer, crypto) = writer(Arc::clone(&custody));
        let first = writer
            .bind_session_api_key(test_binding(credential(3), identity_binding(1)))
            .await
            .expect("first binding");
        let replay = writer
            .bind_session_api_key(test_binding(credential(4), identity_binding(1)))
            .await
            .expect("receipt replay");
        assert_eq!(first, replay);
        assert_eq!(custody.commits(), 1, "the replay cannot write another key");
        assert_eq!(crypto.seals(), 1, "the replay cannot seal another envelope");
        assert!(
            !format!("{:?}", custody.rows()).contains(PLAINTEXT),
            "no durable row may contain plaintext"
        );

        let generation = custody.generation();
        let context = EncryptionContext {
            plane: Plane::Dev,
            region: Region::EuWest1,
            organization: organization(),
            workspace: workspace(),
            name: ResourceName::parse(&first.credential.to_string()).expect("name"),
            generation: SourceGeneration::FIRST,
            custody_revision: None,
        };
        let opened = TestCrypto::default()
            .reveal(
                &SealedSecret {
                    frame: generation.ciphertext.ciphertext,
                    context_digest: generation.context_digest,
                    wrapped_branch_key: generation.ciphertext.wrapped_key,
                },
                &context,
                now(),
            )
            .await
            .expect("correct authority context opens");
        assert_eq!(opened.expose_for_encryption(), PLAINTEXT.as_bytes());
    }

    #[tokio::test]
    async fn a_changed_intent_and_a_changed_encryption_context_both_fail_closed() {
        let custody = Arc::new(MemoryCustody::default());
        let (writer, crypto) = writer(Arc::clone(&custody));
        let first = writer
            .bind_session_api_key(test_binding(credential(3), identity_binding(1)))
            .await
            .expect("first binding");
        assert_eq!(
            writer
                .bind_session_api_key(test_binding_with_key(
                    credential(4),
                    identity_binding(2),
                    "sk-another-value"
                ))
                .await,
            Err(PortError::IdempotencyConflict)
        );
        assert_eq!(
            crypto.seals(),
            1,
            "a conflicting retry is rejected before encryption"
        );

        let generation = custody.generation();
        let wrong = EncryptionContext {
            plane: Plane::Dev,
            region: Region::EuWest1,
            organization: sample(8),
            workspace: workspace(),
            name: ResourceName::parse(&first.credential.to_string()).expect("name"),
            generation: SourceGeneration::FIRST,
            custody_revision: None,
        };
        let error = TestCrypto::default()
            .reveal(
                &SealedSecret {
                    frame: generation.ciphertext.ciphertext,
                    context_digest: generation.context_digest,
                    wrapped_branch_key: generation.ciphertext.wrapped_key,
                },
                &wrong,
                now(),
            )
            .await
            .expect_err("another organization cannot open the ciphertext");
        assert_eq!(error, SecretCryptoError::ContextMismatch);
    }

    #[tokio::test]
    async fn every_mcp_value_is_individually_framed_sealed_and_replayed_before_kms() {
        const HEADER_SECRET: &str = "Bearer remote-mcp-token";
        const ENV_SECRET: &str = "sandbox-mcp-token";

        let remote_name = ResourceName::parse("remote-tools").expect("name");
        let sandbox_name = ResourceName::parse("sandbox-tools").expect("name");
        let servers = vec![
            McpServer {
                name: remote_name.clone(),
                transport: McpTransport::RemoteHttp(RemoteMcpServer {
                    headers: Some(BTreeMap::from([(
                        "Authorization".to_owned(),
                        HEADER_SECRET.to_owned(),
                    )])),
                    url: HttpsUrl::parse("https://mcp.example.test/rpc").expect("HTTPS URL"),
                }),
            },
            McpServer {
                name: sandbox_name.clone(),
                transport: McpTransport::SandboxProcess(SandboxMcpServer {
                    args: Some(vec!["serve".to_owned()]),
                    command: "mcp-server".to_owned(),
                    environment: Some(BTreeMap::from([
                        ("EMPTY_VALUE".to_owned(), String::new()),
                        ("REMOTE_TOKEN".to_owned(), ENV_SECRET.to_owned()),
                    ])),
                    working_directory: None,
                }),
            },
        ];
        let expected = [
            (
                aex_brain_domain::mcp::mcp_secret_name(
                    session(),
                    &remote_name,
                    "header",
                    "Authorization",
                ),
                HEADER_SECRET.as_bytes(),
            ),
            (
                aex_brain_domain::mcp::mcp_secret_name(
                    session(),
                    &sandbox_name,
                    "environment",
                    "EMPTY_VALUE",
                ),
                b"".as_slice(),
            ),
            (
                aex_brain_domain::mcp::mcp_secret_name(
                    session(),
                    &sandbox_name,
                    "environment",
                    "REMOTE_TOKEN",
                ),
                ENV_SECRET.as_bytes(),
            ),
        ];

        let custody = Arc::new(MemoryCustody::default());
        let (writer, crypto) = writer(Arc::clone(&custody));
        writer
            .bind_session_mcp_config(
                workspace(),
                organization(),
                session(),
                &servers,
                &identity(3),
                now(),
            )
            .await
            .expect("first MCP binding");
        writer
            .bind_session_mcp_config(
                workspace(),
                organization(),
                session(),
                &servers,
                &identity(3),
                now(),
            )
            .await
            .expect("MCP replay");

        assert_eq!(custody.commits(), 3, "one atomic commit per secret value");
        assert_eq!(crypto.seals(), 3, "replay must not reseal any MCP value");
        let durable_debug = format!("{:?}", custody.rows());
        for forbidden in [
            HEADER_SECRET,
            ENV_SECRET,
            "Authorization",
            "EMPTY_VALUE",
            "REMOTE_TOKEN",
        ] {
            assert!(
                !durable_debug.contains(forbidden),
                "plaintext MCP config must not enter durable rows"
            );
        }
        assert!(!format!("{writer:?}").contains(HEADER_SECRET));

        let receipts = custody.receipts();
        assert_eq!(receipts.len(), 3);
        for (name, expected_value) in expected {
            let scope = format!("session.mcp_secret:{name}");
            let receipt = receipts
                .iter()
                .find(|receipt| receipt.scope == scope)
                .expect("subject-qualified receipt");
            assert_eq!(receipt.response_kind, MCP_RECEIPT_KIND);
            assert!(matches!(&receipt.response, ReceiptBody::Inline(body) if body.is_empty()));

            let generation = custody.generation_named(&name);
            let context = EncryptionContext {
                plane: Plane::Dev,
                region: Region::EuWest1,
                organization: organization(),
                workspace: workspace(),
                name,
                generation: SourceGeneration::FIRST,
                custody_revision: None,
            };
            let opened = TestCrypto::default()
                .reveal(
                    &SealedSecret {
                        frame: generation.ciphertext.ciphertext,
                        context_digest: generation.context_digest,
                        wrapped_branch_key: generation.ciphertext.wrapped_key,
                    },
                    &context,
                    now(),
                )
                .await
                .expect("MCP authority context opens");
            assert_eq!(opened.expose_for_encryption()[0], MCP_SECRET_FRAME_V1);
            assert_eq!(&opened.expose_for_encryption()[1..], expected_value);
        }

        let conflict = writer
            .bind_session_mcp_config(
                workspace(),
                organization(),
                session(),
                &servers,
                &identity(4),
                now(),
            )
            .await;
        assert_eq!(conflict, Err(PortError::IdempotencyConflict));
        assert_eq!(crypto.seals(), 3, "conflict is rejected before KMS");
    }
}
