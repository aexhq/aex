//! The `SecretCrypto` port: seal, rewrap and reveal.
//!
//! Rewrapping is deliberately not a re-encryption of a plaintext the caller
//! holds. The value is opened under the source context and sealed under the
//! target context inside one call, and the opened bytes never leave this
//! module — a caller cannot ask for the plaintext on the way through.

use aex_secret_domain::context::EncryptionContext;
use aex_secret_domain::plaintext::SecretPlaintext;
use aex_secret_domain::secret::CiphertextRef;
use aex_wire::types::Timestamp;
use async_trait::async_trait;
use zeroize::Zeroizing;

use crate::context;
use crate::envelope::{self, BranchKeyMaterial, Entropy, SystemEntropy};
use crate::keystore::{BranchKeyCache, BranchKeyProvider, KeyMaterialError};

/// Which implementation the composition selected.
///
/// RS-01/OD-33 rejected `aws-esdk`, and the choice must be logged at startup
/// rather than discovered from behaviour. It is never a runtime fallback.
pub const IMPLEMENTATION: &str = "envelope-aead-aws-lc-rs";

/// Why a cryptographic operation failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretCryptoError {
    /// The stored context digest does not match the context the caller
    /// presented, so the row was moved between tenants, sessions or domains.
    ///
    /// Never retried, always alarmed, and caught **before** a KMS call is spent.
    #[error("the stored context digest does not match the presented context")]
    ContextMismatch,
    /// The envelope refused the value.
    #[error(transparent)]
    Envelope(#[from] envelope::EnvelopeError),
    /// Branch material could not be obtained.
    #[error(transparent)]
    KeyMaterial(#[from] KeyMaterialError),
    /// The opened bytes were not an acceptable plaintext.
    #[error("the opened value is not an acceptable plaintext")]
    Plaintext,
}

/// One stored value, with everything needed to open it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedSecret {
    /// The frame.
    pub frame: Vec<u8>,
    /// The digest of the context the frame is bound to.
    pub context_digest: [u8; 32],
    /// The wrapped branch key, as the key store holds it.
    pub wrapped_branch_key: Vec<u8>,
}

impl SealedSecret {
    /// The stored shape the custody adapter writes.
    #[must_use]
    pub fn to_ciphertext_ref(&self, key_generation: u64) -> CiphertextRef {
        CiphertextRef {
            key_generation,
            wrapped_key: self.wrapped_branch_key.clone(),
            nonce: Vec::new(),
            ciphertext: self.frame.clone(),
        }
    }
}

// TODO(cross-stream): `aex-secret-domain` has no `ports` module and publishes no
// traits. Its modules are `context`, `custody`, `plaintext`, `revocation` and
// `secret`, all data and decisions; this port has no peer to be replaced by.
/// Sealing, rewrapping and revealing a workspace secret.
#[async_trait]
pub trait SecretCrypto: Send + Sync + 'static {
    /// Seals a plaintext under `context`.
    ///
    /// # Errors
    ///
    /// [`SecretCryptoError`] for any key or primitive failure.
    async fn seal(
        &self,
        context: &EncryptionContext,
        wrapped_branch_key: &[u8],
        plaintext: &SecretPlaintext,
        now: Timestamp,
    ) -> Result<SealedSecret, SecretCryptoError>;

    /// Opens a value under `from` and re-seals it under `to`, without the
    /// plaintext ever leaving this call.
    ///
    /// # Errors
    ///
    /// As [`SecretCrypto::seal`], plus
    /// [`SecretCryptoError::ContextMismatch`] when the stored digest does not
    /// describe `from`.
    async fn rewrap(
        &self,
        sealed: &SealedSecret,
        from: &EncryptionContext,
        to: &EncryptionContext,
        now: Timestamp,
    ) -> Result<SealedSecret, SecretCryptoError>;

    /// Reveals a plaintext for one authorized managed call.
    ///
    /// # Errors
    ///
    /// As [`SecretCrypto::rewrap`].
    async fn reveal(
        &self,
        sealed: &SealedSecret,
        context: &EncryptionContext,
        now: Timestamp,
    ) -> Result<SecretPlaintext, SecretCryptoError>;
}

/// The adapter.
pub struct EnvelopeCrypto {
    keys: Box<dyn BranchKeyProvider>,
    cache: BranchKeyCache,
    entropy: Box<dyn Entropy>,
}

impl std::fmt::Debug for EnvelopeCrypto {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EnvelopeCrypto")
            .field("implementation", &IMPLEMENTATION)
            .field("cache", &self.cache)
            .finish_non_exhaustive()
    }
}

impl EnvelopeCrypto {
    /// Binds the adapter to a key provider and a cache partition.
    #[must_use]
    pub fn new(keys: Box<dyn BranchKeyProvider>, partition: impl Into<String>) -> Self {
        Self {
            keys,
            cache: BranchKeyCache::new(partition),
            entropy: Box::new(SystemEntropy),
        }
    }

    /// Replaces the random source, so a test can pin a frame.
    #[must_use]
    pub fn with_entropy(mut self, entropy: Box<dyn Entropy>) -> Self {
        self.entropy = entropy;
        self
    }

    /// The cache, for a readiness probe or a rotation drill.
    #[must_use]
    pub const fn cache(&self) -> &BranchKeyCache {
        &self.cache
    }

    async fn material(
        &self,
        context: &EncryptionContext,
        wrapped: &[u8],
        version: [u8; envelope::KEY_VERSION_BYTES],
        now: Timestamp,
    ) -> Result<BranchKeyMaterial, SecretCryptoError> {
        // The branch key id is the workspace id, which makes the supplier a pure
        // function of the context: a caller cannot select another tenant's key
        // by asking for it.
        let branch_key_id = context.workspace.to_string();
        let context_digest = context::context_digest(context);
        if let Some(cached) = self.cache.get(&branch_key_id, version, context_digest, now) {
            return Ok(cached);
        }
        // The branch key is opened under the **branch key's** context, not the
        // secret's: one wrapped key serves every name and generation in the
        // workspace, so the wrap context must be reproducible from the workspace
        // alone. The secret's full identity binds the value one layer down, as
        // AEAD additional data, which is where it is actually enforced.
        let material = self
            .keys
            .material(
                &branch_key_id,
                version,
                wrapped,
                &context::branch_key_pairs(context),
            )
            .await?;
        self.cache.put(material.clone(), context_digest, now);
        Ok(material)
    }
}

#[async_trait]
impl SecretCrypto for EnvelopeCrypto {
    async fn seal(
        &self,
        context: &EncryptionContext,
        wrapped_branch_key: &[u8],
        plaintext: &SecretPlaintext,
        now: Timestamp,
    ) -> Result<SealedSecret, SecretCryptoError> {
        let version = key_version(wrapped_branch_key);
        let material = self
            .material(context, wrapped_branch_key, version, now)
            .await?;
        let aad = context::aad_bytes(context);
        let frame = envelope::seal(
            &material,
            &aad,
            plaintext.expose_for_encryption(),
            self.entropy.as_ref(),
        )?;
        Ok(SealedSecret {
            frame,
            context_digest: context::context_digest(context),
            wrapped_branch_key: wrapped_branch_key.to_vec(),
        })
    }

    async fn rewrap(
        &self,
        sealed: &SealedSecret,
        from: &EncryptionContext,
        to: &EncryptionContext,
        now: Timestamp,
    ) -> Result<SealedSecret, SecretCryptoError> {
        let (opened, source_material) = self.open_with_material(sealed, from, now).await?;
        // `ReEncrypt` moves the wrapped branch key from one branch-key context
        // to another, which is a no-op when both name the same workspace and a
        // real re-wrap when a value crosses into another workspace's key.
        let wrapped = self
            .keys
            .rewrap(
                &sealed.wrapped_branch_key,
                &context::branch_key_pairs(from),
                &context::branch_key_pairs(to),
            )
            .await?;
        let version = key_version(wrapped.as_bytes());
        // KMS ReEncrypt preserves the plaintext branch key while authenticating
        // newly emitted ciphertext under the destination context. Reuse the
        // already-open material instead of spending a redundant destination
        // Decrypt call, but give it the destination ciphertext's derived
        // version and cache identity.
        let material = BranchKeyMaterial {
            branch_key_id: to.workspace.to_string(),
            version,
            material: source_material.material.clone(),
        };
        let aad = context::aad_bytes(to);
        let frame = envelope::seal(&material, &aad, &opened, self.entropy.as_ref())?;
        self.cache.put(material, context::context_digest(to), now);
        Ok(SealedSecret {
            frame,
            context_digest: context::context_digest(to),
            wrapped_branch_key: wrapped.into_bytes(),
        })
    }

    async fn reveal(
        &self,
        sealed: &SealedSecret,
        context: &EncryptionContext,
        now: Timestamp,
    ) -> Result<SecretPlaintext, SecretCryptoError> {
        let opened = self.open(sealed, context, now).await?;
        SecretPlaintext::new(opened.to_vec()).map_err(|_| SecretCryptoError::Plaintext)
    }
}

impl EnvelopeCrypto {
    async fn open(
        &self,
        sealed: &SealedSecret,
        context: &EncryptionContext,
        now: Timestamp,
    ) -> Result<Zeroizing<Vec<u8>>, SecretCryptoError> {
        self.open_with_material(sealed, context, now)
            .await
            .map(|(opened, _material)| opened)
    }

    async fn open_with_material(
        &self,
        sealed: &SealedSecret,
        context: &EncryptionContext,
        now: Timestamp,
    ) -> Result<(Zeroizing<Vec<u8>>, BranchKeyMaterial), SecretCryptoError> {
        // Checked before anything is spent: a mismatch here means the row was
        // moved between tenants, sessions or domains, and there is no reason to
        // ask KMS about it.
        if sealed.context_digest != context::context_digest(context) {
            return Err(SecretCryptoError::ContextMismatch);
        }
        let version = key_version(&sealed.wrapped_branch_key);
        let material = self
            .material(context, &sealed.wrapped_branch_key, version, now)
            .await?;
        let aad = context::aad_bytes(context);
        let opened = envelope::open(&material, &aad, &sealed.frame)?;
        Ok((opened, material))
    }
}

/// The version identifier of a wrapped branch key.
///
/// Derived from the wrapped bytes rather than carried beside them, so a stored
/// row cannot claim a version its material does not have.
#[must_use]
pub fn key_version(wrapped: &[u8]) -> [u8; envelope::KEY_VERSION_BYTES] {
    use sha2::Digest as _;
    let mut hasher = sha2::Sha256::new();
    hasher.update(wrapped);
    let digest = hasher.finalize();
    let mut version = [0u8; envelope::KEY_VERSION_BYTES];
    version.copy_from_slice(&digest[..envelope::KEY_VERSION_BYTES]);
    version
}
