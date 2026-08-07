//! Exact binding lookup, revocation revalidation, and KMS-backed reveal.

use std::sync::Arc;

use aex_brain_app::ports::BoxFuture;
use aex_brain_provider_gateway::credential::{
    BindingState, CredentialResolveError, CredentialRevision, ProviderApiKey,
    ProviderCredentialBinding, ProviderCredentialDecryptor, ProviderCredentialDirectory,
};
use aex_secret_aws::{SealedSecret, SecretCrypto, SecretCryptoError};
use aex_secret_custody_dynamodb::{
    CredentialState, CustodyStore, ProviderCredential, SecretCustodyStore, SecretMetadata,
    StoredGeneration,
};
use aex_secret_domain::context::Plane;
use aex_secret_domain::{RevocationEpoch, SecretName, SecretState, SourceGeneration};
use aex_wire::ids::{OrganizationId, ProviderCredentialId, WorkspaceId};
use aex_wire::provider::ProviderId;
use aex_wire::types::{Region, Timestamp};

/// The three exact authority reads provider dispatch needs.
///
/// A small local port keeps tests in memory while the production implementation
/// delegates to the regional custody adapter. The provider-qualified lookup is
/// one strongly consistent point read rather than six probes.
pub trait CredentialCustody: Send + Sync + 'static {
    /// Reads one complete `pcr_` key.
    fn load_binding(
        &self,
        workspace: WorkspaceId,
        provider: ProviderId,
        credential: ProviderCredentialId,
    ) -> BoxFuture<'_, Result<Option<ProviderCredential>, CredentialResolveError>>;

    /// Reads the mutable secret metadata fence; never ciphertext.
    fn load_secret<'a>(
        &'a self,
        workspace: WorkspaceId,
        name: &'a SecretName,
    ) -> BoxFuture<'a, Result<Option<SecretMetadata>, CredentialResolveError>>;

    /// Reads the exact hidden source generation pinned by the binding.
    fn load_generation<'a>(
        &'a self,
        workspace: WorkspaceId,
        name: &'a SecretName,
        generation: SourceGeneration,
    ) -> BoxFuture<'a, Result<Option<StoredGeneration>, CredentialResolveError>>;
}

impl CredentialCustody for CustodyStore {
    fn load_binding(
        &self,
        workspace: WorkspaceId,
        provider: ProviderId,
        credential: ProviderCredentialId,
    ) -> BoxFuture<'_, Result<Option<ProviderCredential>, CredentialResolveError>> {
        Box::pin(async move {
            self.load_provider_credential_for_provider(workspace, provider, credential)
                .await
                .map_err(|_| CredentialResolveError::Transport)
        })
    }

    fn load_secret<'a>(
        &'a self,
        workspace: WorkspaceId,
        name: &'a SecretName,
    ) -> BoxFuture<'a, Result<Option<SecretMetadata>, CredentialResolveError>> {
        Box::pin(async move {
            SecretCustodyStore::load_secret(self, workspace, name)
                .await
                .map_err(|_| CredentialResolveError::Transport)
        })
    }

    fn load_generation<'a>(
        &'a self,
        workspace: WorkspaceId,
        name: &'a SecretName,
        generation: SourceGeneration,
    ) -> BoxFuture<'a, Result<Option<StoredGeneration>, CredentialResolveError>> {
        Box::pin(async move {
            SecretCustodyStore::load_generation(self, workspace, name, generation)
                .await
                .map_err(|_| CredentialResolveError::Transport)
        })
    }
}

/// One stateless regional credential authority.
///
/// Tenant identity arrives on each dispatch ticket and is copied into the KMS
/// context. There is no process-global current workspace and no serialization
/// point shared by unrelated tenants.
pub struct CredentialAuthority<C, K> {
    custody: Arc<C>,
    crypto: Arc<K>,
    plane: Plane,
    region: Region,
}

impl<C, K> core::fmt::Debug for CredentialAuthority<C, K> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("CredentialAuthority")
            .field("plane", &self.plane)
            .field("region", &self.region)
            .finish_non_exhaustive()
    }
}

impl<C, K> CredentialAuthority<C, K> {
    /// Binds the regional custody and KMS adapters.
    #[must_use]
    pub const fn new(custody: Arc<C>, crypto: Arc<K>, plane: Plane, region: Region) -> Self {
        Self {
            custody,
            crypto,
            plane,
            region,
        }
    }
}

impl<C, K> ProviderCredentialDirectory for CredentialAuthority<C, K>
where
    C: CredentialCustody,
    K: Send + Sync + 'static,
{
    fn resolve(
        &self,
        organization: OrganizationId,
        workspace: WorkspaceId,
        provider: ProviderId,
        id: Option<ProviderCredentialId>,
    ) -> BoxFuture<'_, Result<ProviderCredentialBinding, CredentialResolveError>> {
        Box::pin(async move {
            let id = id.ok_or(CredentialResolveError::NoDefault { provider })?;
            let stored = self
                .custody
                .load_binding(workspace, provider, id)
                .await?
                .ok_or(CredentialResolveError::NotFound {
                    requested: Some(id),
                })?;
            validate_stored_identity(&stored, workspace, provider, id)?;

            // These rows are in separate partitions and neither result depends
            // on the other, so issue both SDK requests concurrently in one
            // sequential latency stage.
            let (metadata, generation) = futures::future::join(
                self.custody.load_secret(workspace, &stored.secret_name),
                self.custody.load_generation(
                    workspace,
                    &stored.secret_name,
                    stored.source_generation,
                ),
            )
            .await;
            let metadata = metadata?.ok_or(CredentialResolveError::Deleted)?;
            let generation = generation?.ok_or(CredentialResolveError::Deleted)?;
            validate_source(&stored, &metadata, &generation)?;

            let context = aex_secret_domain::EncryptionContext {
                plane: self.plane,
                region: self.region,
                organization,
                workspace,
                name: stored.secret_name.clone(),
                generation: stored.source_generation,
                custody_revision: None,
            };
            if context.digest() != generation.context_digest {
                return Err(CredentialResolveError::DecryptFailed);
            }
            Ok(ProviderCredentialBinding {
                id,
                workspace,
                provider,
                revision: CredentialRevision(stored.revision),
                generation: stored.source_generation,
                revocation_epoch: metadata.revocation_epoch,
                is_default: false,
                state: effective_state(stored.state, metadata.state, &generation),
                ciphertext: generation.ciphertext,
                context,
                context_digest: generation.context_digest,
            })
        })
    }

    fn revalidate<'a>(
        &'a self,
        binding: &'a ProviderCredentialBinding,
    ) -> BoxFuture<'a, Result<RevocationEpoch, CredentialResolveError>> {
        Box::pin(async move {
            // All three mutable authorities are independent point reads, so
            // issue the three SDK requests concurrently in one sequential
            // latency stage. The generation row is a fence in its own right:
            // the lazy audit sweep may mark exactly this ciphertext generation
            // revoked without rewriting the metadata row.
            let (stored, metadata, generation) = futures::future::join3(
                self.custody
                    .load_binding(binding.workspace, binding.provider, binding.id),
                self.custody
                    .load_secret(binding.workspace, &binding.context.name),
                self.custody.load_generation(
                    binding.workspace,
                    &binding.context.name,
                    binding.generation,
                ),
            )
            .await;
            let stored = stored?.ok_or(CredentialResolveError::Deleted)?;
            let metadata = metadata?.ok_or(CredentialResolveError::Deleted)?;
            let generation = generation?.ok_or(CredentialResolveError::Deleted)?;
            validate_stored_identity(&stored, binding.workspace, binding.provider, binding.id)?;
            let revoked = stored.state == CredentialState::Revoked
                || metadata.state == SecretState::Revoked
                || metadata.revocation_epoch != binding.revocation_epoch
                || generation.revoked_at.is_some();
            if revoked {
                return Err(CredentialResolveError::Revoked {
                    admitted: binding.revocation_epoch,
                    current: metadata.revocation_epoch,
                });
            }
            if stored.revision != binding.revision.0
                || stored.source_generation != binding.generation
            {
                return Err(CredentialResolveError::NotFound {
                    requested: Some(binding.id),
                });
            }
            if metadata.state == SecretState::Deleted {
                return Err(CredentialResolveError::Deleted);
            }
            validate_source(&stored, &metadata, &generation)?;
            if generation.context_digest != binding.context_digest
                || generation.ciphertext != binding.ciphertext
            {
                return Err(CredentialResolveError::DecryptFailed);
            }
            Ok(metadata.revocation_epoch)
        })
    }
}

impl<C, K> ProviderCredentialDecryptor for CredentialAuthority<C, K>
where
    C: Send + Sync + 'static,
    K: SecretCrypto,
{
    fn decrypt<'a>(
        &'a self,
        binding: &'a ProviderCredentialBinding,
        now: Timestamp,
    ) -> BoxFuture<'a, Result<ProviderApiKey, CredentialResolveError>> {
        Box::pin(async move {
            let sealed = SealedSecret {
                frame: binding.ciphertext.ciphertext.clone(),
                context_digest: binding.context_digest,
                wrapped_branch_key: binding.ciphertext.wrapped_key.clone(),
            };
            let plaintext = self
                .crypto
                .reveal(&sealed, &binding.context, now)
                .await
                .map_err(decrypt_error)?;
            let text = String::from_utf8(plaintext.expose_for_encryption().to_vec())
                .map_err(|_| CredentialResolveError::DecryptFailed)?;
            Ok(ProviderApiKey::new(text))
        })
    }
}

fn validate_stored_identity(
    stored: &ProviderCredential,
    workspace: WorkspaceId,
    provider: ProviderId,
    id: ProviderCredentialId,
) -> Result<(), CredentialResolveError> {
    if stored.workspace != workspace || stored.credential != id {
        return Err(CredentialResolveError::NotFound {
            requested: Some(id),
        });
    }
    if stored.provider != provider {
        return Err(CredentialResolveError::ProviderMismatch {
            binding: stored.provider,
            requested: provider,
        });
    }
    Ok(())
}

fn validate_source(
    stored: &ProviderCredential,
    metadata: &SecretMetadata,
    generation: &StoredGeneration,
) -> Result<(), CredentialResolveError> {
    if metadata.workspace != stored.workspace
        || metadata.name != stored.secret_name
        || generation.workspace != stored.workspace
        || generation.name != stored.secret_name
        || generation.generation != stored.source_generation
    {
        return Err(CredentialResolveError::DecryptFailed);
    }
    Ok(())
}

fn effective_state(
    binding: CredentialState,
    secret: SecretState,
    generation: &StoredGeneration,
) -> BindingState {
    if secret == SecretState::Deleted {
        BindingState::Deleted
    } else if binding == CredentialState::Revoked
        || secret == SecretState::Revoked
        || generation.revoked_at.is_some()
    {
        BindingState::Revoked
    } else {
        BindingState::Ready
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "this conversion is a direct map_err adapter"
)]
fn decrypt_error(error: SecretCryptoError) -> CredentialResolveError {
    match error {
        SecretCryptoError::KeyMaterial(_) => CredentialResolveError::Transport,
        SecretCryptoError::ContextMismatch
        | SecretCryptoError::Envelope(_)
        | SecretCryptoError::Plaintext => CredentialResolveError::DecryptFailed,
    }
}
