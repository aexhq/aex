//! Fixed-width P-256 catalog signatures and trust-store verification.

use std::collections::BTreeMap;

use aex_wire::ids::ContentHash;
use aex_wire::types::Timestamp;
use p256::ecdsa::signature::Verifier as _;
use p256::ecdsa::{Signature, VerifyingKey};

/// The only catalog signature algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SignatureAlg {
    /// P-256 ECDSA over SHA-256, represented as fixed-width `r || s`.
    EcdsaP256Sha256,
}

/// The identifier of a pinned verification key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SigningKeyId(Box<str>);

impl SigningKeyId {
    /// Parses a bounded printable key identifier.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogVerifyError::InvalidKeyId`] for an empty, oversized,
    /// or non-printable value.
    pub fn parse(value: &str) -> Result<Self, CatalogVerifyError> {
        if value.is_empty()
            || value.len() > 128
            || value.bytes().any(|byte| !(0x21..=0x7e).contains(&byte))
        {
            return Err(CatalogVerifyError::InvalidKeyId);
        }
        Ok(Self(value.into()))
    }

    /// The configured identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A catalog signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogSignature {
    /// Algorithm marker.
    pub alg: SignatureAlg,
    /// Verification-key selector.
    pub key_id: SigningKeyId,
    bytes: [u8; 64],
}

impl CatalogSignature {
    /// Parses a fixed-width `r || s` signature.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogVerifyError::MalformedSignature`] unless `bytes` is
    /// exactly 64 bytes.
    pub fn from_bytes(key_id: SigningKeyId, bytes: &[u8]) -> Result<Self, CatalogVerifyError> {
        let bytes: [u8; 64] = bytes
            .try_into()
            .map_err(|_| CatalogVerifyError::MalformedSignature { bytes: bytes.len() })?;
        Ok(Self {
            alg: SignatureAlg::EcdsaP256Sha256,
            key_id,
            bytes,
        })
    }

    /// The fixed-width `r || s` bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 64] {
        &self.bytes
    }
}

/// One pinned public key and its hard expiry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationKey {
    /// Key selector.
    pub id: SigningKeyId,
    /// SEC1 encoded P-256 public key.
    pub public_key: Box<[u8]>,
    /// Last instant at which this key is accepted.
    pub not_after: Timestamp,
}

/// Immutable verification keys loaded at process start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustStore(BTreeMap<SigningKeyId, VerificationKey>);

impl TrustStore {
    /// Builds a trust store, rejecting duplicate identifiers.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogVerifyError::DuplicateKey`] on a duplicate id.
    pub fn new(keys: Vec<VerificationKey>) -> Result<Self, CatalogVerifyError> {
        let mut by_id = BTreeMap::new();
        for key in keys {
            let id = key.id.clone();
            if by_id.insert(id.clone(), key).is_some() {
                return Err(CatalogVerifyError::DuplicateKey {
                    key_id: id.as_str().to_owned(),
                });
            }
        }
        Ok(Self(by_id))
    }

    fn get(&self, id: &SigningKeyId) -> Option<&VerificationKey> {
        self.0.get(id)
    }
}

/// Builds the domain-separated bytes signed for one revision.
#[must_use]
pub fn catalog_signing_input(schema_version: u16, kind_tag: u8, digest: &ContentHash) -> Vec<u8> {
    const DOMAIN: &[u8] = b"aex.tool-catalog.v1\0";
    let mut input = Vec::with_capacity(DOMAIN.len() + 2 + 1 + 32);
    input.extend_from_slice(DOMAIN);
    input.extend_from_slice(&schema_version.to_le_bytes());
    input.push(kind_tag);
    input.extend_from_slice(digest.as_bytes());
    input
}

/// Verifies a catalog signature against the current trust store.
///
/// # Errors
///
/// Fails for unknown or expired keys, high-`s` signatures, or a cryptographic
/// mismatch. A high-`s` signature is refused before the crypto call so the
/// same mathematical signature cannot have two accepted byte identities.
pub fn verify_catalog_signature(
    schema_version: u16,
    kind_tag: u8,
    digest: &ContentHash,
    signature: &CatalogSignature,
    trust: &TrustStore,
    now: Timestamp,
) -> Result<(), CatalogVerifyError> {
    let key = trust
        .get(&signature.key_id)
        .ok_or_else(|| CatalogVerifyError::UnknownKey {
            key_id: signature.key_id.as_str().to_owned(),
        })?;
    if now > key.not_after {
        return Err(CatalogVerifyError::KeyExpired {
            key_id: signature.key_id.as_str().to_owned(),
        });
    }
    if signature.bytes[32..] > P256_HALF_ORDER[..] {
        return Err(CatalogVerifyError::MalleableSignature);
    }
    let input = catalog_signing_input(schema_version, kind_tag, digest);
    let verifying_key = VerifyingKey::from_sec1_bytes(&key.public_key)
        .map_err(|_| CatalogVerifyError::InvalidPublicKey)?;
    let parsed_signature = Signature::from_slice(&signature.bytes)
        .map_err(|_| CatalogVerifyError::InvalidSignature)?;
    verifying_key
        .verify(&input, &parsed_signature)
        .map_err(|_| CatalogVerifyError::InvalidSignature)
}

/// Why a catalog signature or trust store was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogVerifyError {
    /// A key id failed its grammar.
    #[error("catalog signing key id must be 1..=128 printable ASCII bytes")]
    InvalidKeyId,
    /// A trust store contained the same selector twice.
    #[error("duplicate catalog verification key `{key_id}`")]
    DuplicateKey {
        /// Duplicate selector.
        key_id: String,
    },
    /// No key matched the selector.
    #[error("unknown catalog verification key `{key_id}`")]
    UnknownKey {
        /// Unknown selector.
        key_id: String,
    },
    /// The selected key was past its configured expiry.
    #[error("catalog verification key `{key_id}` is expired")]
    KeyExpired {
        /// Expired selector.
        key_id: String,
    },
    /// The signature was not fixed-width P-256.
    #[error("catalog signature is {bytes} bytes; expected 64")]
    MalformedSignature {
        /// Observed length.
        bytes: usize,
    },
    /// The signature used the alternate high-`s` representation.
    #[error("catalog signature has a malleable high-s representation")]
    MalleableSignature,
    /// The trust store contained bytes that were not a P-256 SEC1 point.
    #[error("catalog verification key is not a valid P-256 public key")]
    InvalidPublicKey,
    /// Cryptographic verification failed.
    #[error("catalog signature does not match the signed digest")]
    InvalidSignature,
}

const P256_HALF_ORDER: [u8; 32] = [
    0x7f, 0xff, 0xff, 0xff, 0x80, 0x00, 0x00, 0x00, 0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xde, 0x73, 0x7d, 0x56, 0xd3, 0x8b, 0xcf, 0x42, 0x79, 0xdc, 0xe5, 0x61, 0x7e, 0x31, 0x92, 0xa8,
];
