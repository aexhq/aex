//! Production credential-authority boundary tests with an in-memory custody port.

use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use aex_brain_app::ports::BoxFuture;
use aex_brain_provider_custody::{CredentialAuthority, CredentialCustody};
use aex_brain_provider_gateway::credential::{
    CredentialResolveError, ProviderCredentialDecryptor, ProviderCredentialDirectory,
};
use aex_brain_provider_gateway::transport::AuthScheme;
use aex_secret_aws::{SealedSecret, SecretCrypto, SecretCryptoError};
use aex_secret_custody_dynamodb::{
    CredentialState, ProviderCredential, SecretMetadata, StoredGeneration,
};
use aex_secret_domain::context::Plane;
use aex_secret_domain::secret::SecretRevision;
use aex_secret_domain::{
    CiphertextRef, RevocationEpoch, SecretPlaintext, SecretState, SourceGeneration,
};
use aex_wire::ids::{
    ContentHash, OrganizationId, PrefixedId as _, ProviderCredentialId, Uuid7, WorkspaceId,
};
use aex_wire::provider::ProviderId;
use aex_wire::types::{Region, Timestamp};
use async_trait::async_trait;
use proptest::prelude::*;

fn timestamp(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(millis).expect("timestamp")
}

fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]))
}

fn organization() -> OrganizationId {
    OrganizationId::from_uuid7(Uuid7::compose(1, [2; 10]))
}

fn credential() -> ProviderCredentialId {
    ProviderCredentialId::from_uuid7(Uuid7::compose(1, [3; 10]))
}

fn context() -> aex_secret_domain::EncryptionContext {
    aex_secret_domain::EncryptionContext {
        plane: Plane::Dev,
        region: Region::EuWest1,
        organization: organization(),
        workspace: workspace(),
        name: aex_secret_domain::SecretName::parse("provider-key").expect("name"),
        generation: SourceGeneration(3),
        custody_revision: None,
    }
}

fn stored_binding() -> ProviderCredential {
    ProviderCredential {
        credential: credential(),
        workspace: workspace(),
        name: aex_wire::ids::ResourceName::parse("primary").expect("name"),
        provider: ProviderId::Openai,
        secret_name: context().name,
        source_generation: SourceGeneration(3),
        fingerprint: ContentHash::of(b"fingerprint"),
        revision: 2,
        state: CredentialState::Ready,
        created_at: timestamp(1),
        updated_at: timestamp(1),
        revoked_at: None,
    }
}

fn metadata() -> SecretMetadata {
    SecretMetadata {
        workspace: workspace(),
        name: context().name,
        generation: SourceGeneration(3),
        revision: SecretRevision(2),
        state: SecretState::Ready,
        revocation_epoch: RevocationEpoch(7),
        revoked_through_revision: SecretRevision(0),
        created_at: timestamp(1),
        updated_at: timestamp(1),
        revoked_at: None,
    }
}

fn generation() -> StoredGeneration {
    StoredGeneration {
        workspace: workspace(),
        name: context().name,
        generation: SourceGeneration(3),
        ciphertext: CiphertextRef {
            key_generation: 1,
            wrapped_key: vec![1; 32],
            nonce: Vec::new(),
            ciphertext: vec![2; 32],
        },
        context_digest: context().digest(),
        created_at: timestamp(1),
        revoked_at: None,
    }
}

struct FakeCustody {
    binding: Mutex<ProviderCredential>,
    metadata: Mutex<SecretMetadata>,
    generation: Mutex<StoredGeneration>,
}

impl FakeCustody {
    fn ready() -> Self {
        Self {
            binding: Mutex::new(stored_binding()),
            metadata: Mutex::new(metadata()),
            generation: Mutex::new(generation()),
        }
    }
}

impl CredentialCustody for FakeCustody {
    fn load_binding(
        &self,
        workspace: WorkspaceId,
        provider: ProviderId,
        credential: ProviderCredentialId,
    ) -> BoxFuture<'_, Result<Option<ProviderCredential>, CredentialResolveError>> {
        Box::pin(async move {
            let row = self.binding.lock().expect("not poisoned").clone();
            Ok((row.workspace == workspace
                && row.provider == provider
                && row.credential == credential)
                .then_some(row))
        })
    }

    fn load_secret<'a>(
        &'a self,
        workspace: WorkspaceId,
        name: &'a aex_secret_domain::SecretName,
    ) -> BoxFuture<'a, Result<Option<SecretMetadata>, CredentialResolveError>> {
        Box::pin(async move {
            let row = self.metadata.lock().expect("not poisoned").clone();
            Ok((row.workspace == workspace && row.name == *name).then_some(row))
        })
    }

    fn load_generation<'a>(
        &'a self,
        workspace: WorkspaceId,
        name: &'a aex_secret_domain::SecretName,
        generation: SourceGeneration,
    ) -> BoxFuture<'a, Result<Option<StoredGeneration>, CredentialResolveError>> {
        Box::pin(async move {
            let row = self.generation.lock().expect("not poisoned").clone();
            Ok(
                (row.workspace == workspace && row.name == *name && row.generation == generation)
                    .then_some(row),
            )
        })
    }
}

struct FakeCrypto;

#[async_trait]
impl SecretCrypto for FakeCrypto {
    async fn seal(
        &self,
        _context: &aex_secret_domain::EncryptionContext,
        _wrapped_branch_key: &[u8],
        _plaintext: &SecretPlaintext,
        _now: Timestamp,
    ) -> Result<SealedSecret, SecretCryptoError> {
        panic!("dispatch never seals")
    }

    async fn rewrap(
        &self,
        _sealed: &SealedSecret,
        _from: &aex_secret_domain::EncryptionContext,
        _to: &aex_secret_domain::EncryptionContext,
        _now: Timestamp,
    ) -> Result<SealedSecret, SecretCryptoError> {
        panic!("dispatch never rewraps")
    }

    async fn reveal(
        &self,
        sealed: &SealedSecret,
        context: &aex_secret_domain::EncryptionContext,
        _now: Timestamp,
    ) -> Result<SecretPlaintext, SecretCryptoError> {
        assert_eq!(sealed.context_digest, context.digest());
        SecretPlaintext::new(b"sk-test-0123456789".to_vec())
            .map_err(|_| SecretCryptoError::Plaintext)
    }
}

fn authority(custody: Arc<FakeCustody>) -> CredentialAuthority<FakeCustody, FakeCrypto> {
    CredentialAuthority::new(custody, Arc::new(FakeCrypto), Plane::Dev, Region::EuWest1)
}

#[tokio::test]
async fn exact_binding_resolves_and_decrypts_under_ticket_scope() {
    let custody = Arc::new(FakeCustody::ready());
    let authority = authority(custody);
    let binding = authority
        .resolve(
            organization(),
            workspace(),
            ProviderId::Openai,
            Some(credential()),
        )
        .await
        .expect("the exact binding resolves");
    assert_eq!(binding.context, context());
    assert_eq!(binding.revision.0, 2);
    assert_eq!(binding.generation, SourceGeneration(3));
    authority
        .revalidate(&binding)
        .await
        .expect("the fences did not move");
    let key = authority
        .decrypt(&binding, timestamp(2))
        .await
        .expect("the KMS-backed adapter reveals the key");
    let (_, header) = key
        .sensitive_header(AuthScheme::BearerAuthorization)
        .expect("valid provider header");
    assert!(header.is_sensitive());
}

#[tokio::test]
async fn provider_only_revocation_wins_after_initial_resolution() {
    let custody = Arc::new(FakeCustody::ready());
    let authority = authority(Arc::clone(&custody));
    let binding = authority
        .resolve(
            organization(),
            workspace(),
            ProviderId::Openai,
            Some(credential()),
        )
        .await
        .expect("initial resolution");
    {
        let mut stored = custody.binding.lock().expect("not poisoned");
        stored.state = CredentialState::Revoked;
        stored.revision = 3;
    }
    assert!(matches!(
        authority.revalidate(&binding).await,
        Err(CredentialResolveError::Revoked { .. })
    ));
}

#[tokio::test]
async fn secret_epoch_revocation_wins_after_initial_resolution() {
    let custody = Arc::new(FakeCustody::ready());
    let authority = authority(Arc::clone(&custody));
    let binding = authority
        .resolve(
            organization(),
            workspace(),
            ProviderId::Openai,
            Some(credential()),
        )
        .await
        .expect("initial resolution");
    {
        let mut metadata = custody.metadata.lock().expect("not poisoned");
        metadata.state = SecretState::Revoked;
        metadata.revocation_epoch = RevocationEpoch(8);
    }
    assert_eq!(
        authority.revalidate(&binding).await,
        Err(CredentialResolveError::Revoked {
            admitted: RevocationEpoch(7),
            current: RevocationEpoch(8),
        })
    );
}

#[tokio::test]
async fn exact_generation_revocation_wins_after_initial_resolution() {
    let custody = Arc::new(FakeCustody::ready());
    let authority = authority(Arc::clone(&custody));
    let binding = authority
        .resolve(
            organization(),
            workspace(),
            ProviderId::Openai,
            Some(credential()),
        )
        .await
        .expect("initial resolution");
    custody.generation.lock().expect("not poisoned").revoked_at = Some(timestamp(2));
    assert_eq!(
        authority.revalidate(&binding).await,
        Err(CredentialResolveError::Revoked {
            admitted: RevocationEpoch(7),
            current: RevocationEpoch(7),
        })
    );
}

#[tokio::test]
async fn moved_ciphertext_context_fails_before_decrypt() {
    let custody = Arc::new(FakeCustody::ready());
    custody
        .generation
        .lock()
        .expect("not poisoned")
        .context_digest = [9; 32];
    let error = authority(custody)
        .resolve(
            organization(),
            workspace(),
            ProviderId::Openai,
            Some(credential()),
        )
        .await
        .expect_err("a moved ciphertext must not reach KMS");
    assert_eq!(error, CredentialResolveError::DecryptFailed);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// No non-empty combination of cross-authority identity mutations may
    /// reconstruct a provider binding. This is the property behind the
    /// individual revocation examples above: independently read rows fail
    /// closed unless every typed identity and ciphertext context agrees.
    #[test]
    fn any_cross_authority_identity_mutation_fails_closed(mask in 1_u16..1_024) {
        let custody = Arc::new(FakeCustody::ready());
        let other_workspace = WorkspaceId::from_uuid7(Uuid7::compose(2, [4; 10]));
        let other_credential = ProviderCredentialId::from_uuid7(Uuid7::compose(2, [5; 10]));
        let other_name = aex_secret_domain::SecretName::parse("other-provider-key")
            .expect("alternate name");

        {
            let mut binding = custody.binding.lock().expect("not poisoned");
            if mask & 1 != 0 {
                binding.workspace = other_workspace;
            }
            if mask & 2 != 0 {
                binding.provider = ProviderId::Anthropic;
            }
            if mask & 4 != 0 {
                binding.credential = other_credential;
            }
            if mask & 8 != 0 {
                binding.secret_name = other_name.clone();
            }
        }
        {
            let mut metadata = custody.metadata.lock().expect("not poisoned");
            if mask & 16 != 0 {
                metadata.workspace = other_workspace;
            }
            if mask & 32 != 0 {
                metadata.name = other_name.clone();
            }
        }
        {
            let mut generation = custody.generation.lock().expect("not poisoned");
            if mask & 64 != 0 {
                generation.workspace = other_workspace;
            }
            if mask & 128 != 0 {
                generation.name = other_name;
            }
            if mask & 256 != 0 {
                generation.generation = SourceGeneration(4);
            }
            if mask & 512 != 0 {
                generation.context_digest = [9; 32];
            }
        }

        let authority = authority(custody);
        let mut resolution = authority.resolve(
            organization(),
            workspace(),
            ProviderId::Openai,
            Some(credential()),
        );
        let waker = futures::task::noop_waker();
        let result = match resolution.as_mut().poll(&mut Context::from_waker(&waker)) {
            Poll::Ready(result) => result,
            Poll::Pending => panic!("in-memory credential ports must resolve synchronously"),
        };
        prop_assert!(result.is_err());
    }
}
