//! Plaintext admission boundary for secrets and provider credentials.

pub mod config;

use std::sync::Arc;

use aex_regional_http::router::RouteOwner;
use aex_wire::routes::RouteId;

use crate::config::Config;

/// The composed secret edge: the custody authority plus the envelope crypto over
/// the secret `KMS` key, and nothing else.
///
/// The type is the capability statement. There is no field here that can reach
/// the session authority, the content bucket, the work table or a queue, so the
/// composition test that asserts the absence has something structural to assert
/// rather than a comment to trust.
pub struct SecretEdge<S, C> {
    custody: Arc<S>,
    crypto: Arc<C>,
}

impl<S, C> SecretEdge<S, C> {
    /// Binds the edge to its two adapters.
    #[must_use]
    pub const fn new(custody: Arc<S>, crypto: Arc<C>) -> Self {
        Self { custody, crypto }
    }

    /// The custody authority.
    #[must_use]
    pub fn custody(&self) -> &S {
        &self.custody
    }

    /// The envelope crypto.
    #[must_use]
    pub fn crypto(&self) -> &C {
        &self.crypto
    }

    /// The deployable this edge composes.
    #[must_use]
    pub const fn owner() -> RouteOwner {
        RouteOwner::SecretApi
    }

    /// Dependencies of the served route set that are not yet resolved.
    ///
    /// Readiness is derived from the composition rather than declared: a name
    /// appears here only while something the served routes need is unbound.
    #[must_use]
    pub fn unresolved(&self, config: &Config) -> Vec<String> {
        let mut unresolved = Vec::new();
        if config.secret_custody_table.is_empty() {
            unresolved.push("regional-secret-custody".to_owned());
        }
        if config.secret_keystore_table.is_empty() {
            unresolved.push("regional-secret-keystore".to_owned());
        }
        unresolved
    }
}
use sha2::Digest as _;
use zeroize::Zeroize as _;

/// Bounded plaintext that never implements serialization or ordinary debug.
pub struct SecretPlaintext(Vec<u8>);

impl SecretPlaintext {
    /// Constructs a non-empty plaintext value up to 64 KiB.
    ///
    /// # Errors
    ///
    /// Returns [`AdmissionError::InvalidPlaintext`] outside the bound.
    pub fn new(bytes: Vec<u8>) -> Result<Self, AdmissionError> {
        if bytes.is_empty() || bytes.len() > 65_536 {
            return Err(AdmissionError::InvalidPlaintext);
        }
        Ok(Self(bytes))
    }
}

impl Drop for SecretPlaintext {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl std::fmt::Debug for SecretPlaintext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SecretPlaintext(<redacted>)")
    }
}

/// Encrypted bytes accepted by the custody adapter.
#[derive(Clone, PartialEq, Eq)]
pub struct Ciphertext(Vec<u8>);

impl Ciphertext {
    /// Borrows bytes only for immediate persistence.
    #[must_use]
    pub fn expose_for_persistence(&self) -> &[u8] {
        &self.0
    }
}

impl std::fmt::Debug for Ciphertext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Ciphertext(<redacted>, {} bytes)", self.0.len())
    }
}

/// Encrypts synchronously inside the plaintext lifetime, then zeroizes before returning.
///
/// # Errors
///
/// Returns the encryptor's error without persisting any partial result.
pub fn admit_plaintext<E>(
    mut plaintext: SecretPlaintext,
    encrypt: impl FnOnce(&[u8]) -> Result<Vec<u8>, E>,
) -> Result<Ciphertext, E> {
    let encrypted = encrypt(&plaintext.0);
    plaintext.0.zeroize();
    encrypted.map(Ciphertext)
}

/// Immutable-view secret generation with monotonic revocation state.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretRecord {
    generation: u64,
    revocation_epoch: u64,
    etag: String,
    ciphertext: Vec<u8>,
    replacement_intent: String,
    revocation_intent: Option<String>,
}

impl std::fmt::Debug for SecretRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SecretRecord")
            .field("generation", &self.generation)
            .field("revocation_epoch", &self.revocation_epoch)
            .field("etag", &self.etag)
            .field("ciphertext", &"<redacted>")
            .field("replacement_intent", &"<redacted>")
            .field(
                "revocation_intent",
                &self.revocation_intent.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl SecretRecord {
    /// Creates generation one.
    ///
    /// # Errors
    ///
    /// Returns [`AdmissionError::InvalidCiphertext`] for empty bytes or intent.
    pub fn create(ciphertext: Vec<u8>, intent: impl Into<String>) -> Result<Self, AdmissionError> {
        let intent = intent.into();
        if ciphertext.is_empty() || intent.is_empty() {
            return Err(AdmissionError::InvalidCiphertext);
        }
        let etag = derive_etag(1, 0, &ciphertext);
        Ok(Self {
            generation: 1,
            revocation_epoch: 0,
            etag,
            ciphertext,
            replacement_intent: intent,
            revocation_intent: None,
        })
    }

    /// Replaces ciphertext under a strong entity tag.
    ///
    /// # Errors
    ///
    /// Returns a typed precondition, idempotency or ciphertext error.
    pub fn replace(
        &self,
        ciphertext: Vec<u8>,
        if_match: &str,
        intent: impl Into<String>,
    ) -> Result<Self, AdmissionError> {
        let intent = intent.into();
        if intent == self.replacement_intent {
            return Ok(self.clone());
        }
        if if_match != self.etag {
            return Err(AdmissionError::PreconditionFailed);
        }
        if ciphertext.is_empty() || intent.is_empty() {
            return Err(AdmissionError::InvalidCiphertext);
        }
        let generation = self
            .generation
            .checked_add(1)
            .ok_or(AdmissionError::GenerationExhausted)?;
        Ok(Self {
            generation,
            revocation_epoch: self.revocation_epoch,
            etag: derive_etag(generation, self.revocation_epoch, &ciphertext),
            ciphertext,
            replacement_intent: intent,
            revocation_intent: self.revocation_intent.clone(),
        })
    }

    /// Increments the lineage revocation epoch once per intent.
    ///
    /// # Errors
    ///
    /// Returns [`AdmissionError::IdempotencyConflict`] when a different revoke
    /// intent follows the terminal one.
    pub fn revoke(&self, intent: impl Into<String>) -> Result<Self, AdmissionError> {
        let intent = intent.into();
        if self.revocation_intent.as_deref() == Some(intent.as_str()) {
            return Ok(self.clone());
        }
        if self.revocation_intent.is_some() {
            return Err(AdmissionError::IdempotencyConflict);
        }
        let revocation_epoch = self
            .revocation_epoch
            .checked_add(1)
            .ok_or(AdmissionError::GenerationExhausted)?;
        let mut updated = self.clone();
        updated.revocation_epoch = revocation_epoch;
        updated.revocation_intent = Some(intent);
        updated.etag = derive_etag(updated.generation, revocation_epoch, &updated.ciphertext);
        Ok(updated)
    }

    /// Current generation.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Current revocation epoch.
    #[must_use]
    pub const fn revocation_epoch(&self) -> u64 {
        self.revocation_epoch
    }

    /// Strong entity tag.
    #[must_use]
    pub fn etag(&self) -> &str {
        &self.etag
    }
}

/// Plaintext-bearing routes owned exclusively by this edge.
#[must_use]
pub fn secret_route_ids() -> Vec<RouteId> {
    vec![
        RouteId::ProviderCredentialRegister,
        RouteId::SecretDelete,
        RouteId::SecretPut,
        RouteId::SecretRevoke,
    ]
}

/// Why secret admission was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AdmissionError {
    /// Plaintext was empty or over 64 KiB.
    #[error("secret plaintext is outside the accepted bound")]
    InvalidPlaintext,
    /// Ciphertext or its intent was empty.
    #[error("encrypted secret is invalid")]
    InvalidCiphertext,
    /// Strong entity tag did not match.
    #[error("secret precondition failed")]
    PreconditionFailed,
    /// Replay identity changed after a terminal revoke.
    #[error("secret idempotency conflict")]
    IdempotencyConflict,
    /// Generation or revocation counter overflowed.
    #[error("secret generation is exhausted")]
    GenerationExhausted,
}

fn derive_etag(generation: u64, revocation_epoch: u64, ciphertext: &[u8]) -> String {
    let mut digest = sha2::Sha256::new();
    digest.update(generation.to_be_bytes());
    digest.update(revocation_epoch.to_be_bytes());
    digest.update(ciphertext);
    format!("\"{}\"", hex_lower(&digest.finalize()))
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}
