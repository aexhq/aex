//! The detached catalog signature envelope (plan 08 §2.3).
//!
//! # Algorithm
//!
//! Plan 08 D-02 wrote "detached Ed25519". `00-orchestrator-conventions.md`
//! OD-21 then pinned **`ECDSA_SHA_256`** wherever the signing key lives in AWS
//! KMS, because KMS offers no Ed25519 asymmetric key spec, and the catalog
//! publishing key does live there. Verification is therefore ECDSA P-256 with
//! SHA-256 over an ASN.1 DER signature — exactly what `kms:Sign` with
//! `SigningAlgorithm=ECDSA_SHA_256` produces. Recorded as D-30 in
//! `references/rewrite/providers.md`.
//!
//! # Trust
//!
//! The trusted key set is **compiled into the binary**. A catalog therefore
//! cannot introduce its own trust root: an unknown `key_id` is a load failure,
//! not a key to fetch. Every supplied signature must be unique, trusted and
//! valid. One valid signature is sufficient; additional signatures support a
//! release-bound key rotation and therefore have to verify as well.

use aws_lc_rs::signature::{ECDSA_P256_SHA256_ASN1, UnparsedPublicKey};
use serde::{Deserialize, Serialize};

use crate::primitives::BoundedString;

/// The domain-separation prefix the signature covers.
///
/// The signed input is `SIGNING_PREFIX || document`, so a signature over some
/// other AEX artefact can never be replayed as a catalog signature.
pub const SIGNING_PREFIX: &[u8] = b"aex-model-catalog/v1\n";

/// The largest ASN.1 DER P-256 signature, plus slack for the maximum-length
/// integer encoding.
pub const MAX_SIGNATURE_BYTES: usize = 80;

/// An uncompressed SEC1 P-256 public key is one tag byte plus two 32-byte
/// coordinates.
pub const P256_PUBLIC_KEY_BYTES: usize = 65;

/// The most signatures one envelope may carry.
///
/// A release needs at most the outgoing and incoming publisher keys during a
/// rotation. Eight leaves ample overlap without allowing an attacker to turn
/// startup verification into unbounded public-key work.
pub const MAX_SIGNATURES: usize = 8;

/// One compiled trusted key: its id and its uncompressed SEC1 public key.
pub type TrustedKey = (&'static str, [u8; P256_PUBLIC_KEY_BYTES]);

/// The identity of a catalog signing key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SigningKeyId(pub BoundedString<64>);

/// The closed signature-algorithm set. One member, by OD-21.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SigAlg {
    /// ECDSA over NIST P-256 with SHA-256, ASN.1 DER signature encoding.
    EcdsaP256Sha256Asn1,
}

/// One detached signature over the canonical document bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogSignature {
    /// Which compiled key signed.
    pub key_id: SigningKeyId,
    /// Which algorithm produced the signature.
    pub algorithm: SigAlg,
    /// The DER signature, base64 on the wire.
    #[serde(with = "crate::primitives::base64_bytes")]
    pub bytes: bytes::Bytes,
}

/// The signed catalog artefact: exact bytes plus detached signatures.
///
/// The document is carried as **bytes**, never as a parsed value, so the
/// signature is verified over what was actually published rather than over a
/// re-serialization of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogEnvelope {
    /// The canonical (JCS) document bytes, base64 on the wire.
    #[serde(with = "crate::primitives::base64_bytes")]
    pub document: bytes::Bytes,
    /// One or more detached signatures.
    pub signatures: Vec<CatalogSignature>,
}

/// The compiled trusted key set.
///
/// Being `&'static` is the point: the set cannot be widened at runtime, by
/// configuration, or by the catalog itself.
#[derive(Debug, Clone, Copy)]
pub struct TrustedKeys(&'static [TrustedKey]);

impl TrustedKeys {
    /// Wraps a compiled key set.
    #[must_use]
    pub const fn new(keys: &'static [TrustedKey]) -> Self {
        Self(keys)
    }

    /// The empty set. A process that has been given no publishing key trusts no
    /// catalog at all, which is the correct closed default.
    #[must_use]
    pub const fn empty() -> Self {
        Self(&[])
    }

    /// The public key for an id.
    #[must_use]
    pub fn get(&self, id: &SigningKeyId) -> Option<&'static [u8; P256_PUBLIC_KEY_BYTES]> {
        self.0
            .iter()
            .find(|(name, _)| *name == id.0.as_str())
            .map(|(_, key)| key)
    }

    /// How many keys are compiled in.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether no key is compiled in.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Why an envelope's signatures were rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SignatureError {
    /// The envelope carries no signature at all.
    #[error("the envelope carries no signature")]
    Unsigned,
    /// More signatures than a bounded release rotation can require.
    #[error("the envelope carries {0} signatures, over the {MAX_SIGNATURES}-signature bound")]
    TooMany(usize),
    /// A `key_id` that is not compiled into this binary.
    #[error("signing key `{0:?}` is not a compiled trusted key")]
    UntrustedKey(SigningKeyId),
    /// The same key signed twice, which would let one key meet a threshold of
    /// two.
    #[error("signing key `{0:?}` appears more than once")]
    DuplicateKey(SigningKeyId),
    /// The signature does not verify over the document bytes.
    #[error("the signature from `{0:?}` does not verify")]
    BadSignature(SigningKeyId),
    /// A signature longer than any well-formed P-256 DER signature.
    #[error("the signature from `{0:?}` is {1} bytes, over the bound")]
    Oversized(SigningKeyId, usize),
}

/// Verifies an envelope against the compiled key set. Threshold is one, and
/// every signature supplied must verify. Success returns the lexically first
/// signer id, independent of trust-set or envelope-signature ordering.
///
/// # Errors
///
/// Returns [`SignatureError`] when the envelope is unsigned, exceeds the
/// signature-count bound, names an untrusted or duplicated key, carries an
/// oversized signature, or when any signature does not verify over
/// `SIGNING_PREFIX || envelope.document`.
pub fn verify(
    envelope: &CatalogEnvelope,
    keys: &TrustedKeys,
) -> Result<SigningKeyId, SignatureError> {
    if envelope.signatures.is_empty() {
        return Err(SignatureError::Unsigned);
    }
    if envelope.signatures.len() > MAX_SIGNATURES {
        return Err(SignatureError::TooMany(envelope.signatures.len()));
    }

    let mut seen: Vec<&SigningKeyId> = Vec::with_capacity(envelope.signatures.len());
    for signature in &envelope.signatures {
        if seen.contains(&&signature.key_id) {
            return Err(SignatureError::DuplicateKey(signature.key_id.clone()));
        }
        seen.push(&signature.key_id);
    }
    for signature in &envelope.signatures {
        if signature.bytes.len() > MAX_SIGNATURE_BYTES {
            return Err(SignatureError::Oversized(
                signature.key_id.clone(),
                signature.bytes.len(),
            ));
        }
    }

    let trusted = envelope
        .signatures
        .iter()
        .map(|signature| {
            keys.get(&signature.key_id)
                .ok_or_else(|| SignatureError::UntrustedKey(signature.key_id.clone()))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut signed = Vec::with_capacity(SIGNING_PREFIX.len() + envelope.document.len());
    signed.extend_from_slice(SIGNING_PREFIX);
    signed.extend_from_slice(&envelope.document);
    for (signature, public) in envelope.signatures.iter().zip(trusted) {
        let SigAlg::EcdsaP256Sha256Asn1 = signature.algorithm;
        let verifier = UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, public.as_slice());
        verifier
            .verify(&signed, &signature.bytes)
            .map_err(|_| SignatureError::BadSignature(signature.key_id.clone()))?;
    }

    envelope
        .signatures
        .iter()
        .map(|signature| &signature.key_id)
        .min()
        .cloned()
        .ok_or(SignatureError::Unsigned)
}

#[cfg(test)]
mod tests {
    use aws_lc_rs::rand::SystemRandom;
    use aws_lc_rs::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};

    use super::{
        CatalogEnvelope, CatalogSignature, MAX_SIGNATURES, P256_PUBLIC_KEY_BYTES, SIGNING_PREFIX,
        SigAlg, SignatureError, SigningKeyId, TrustedKey, TrustedKeys, verify,
    };
    use crate::primitives::BoundedString;

    fn key_id(name: &str) -> SigningKeyId {
        SigningKeyId(BoundedString::new(name).expect("short name"))
    }

    struct Signer {
        pair: EcdsaKeyPair,
        random: SystemRandom,
    }

    impl Signer {
        fn new() -> Self {
            let random = SystemRandom::new();
            let pair =
                EcdsaKeyPair::generate(&ECDSA_P256_SHA256_ASN1_SIGNING).expect("key generation");
            Self { pair, random }
        }

        fn public(&self) -> [u8; P256_PUBLIC_KEY_BYTES] {
            let mut out = [0u8; P256_PUBLIC_KEY_BYTES];
            out.copy_from_slice(self.pair.public_key().as_ref());
            out
        }

        fn sign_with_prefix(&self, document: &[u8]) -> bytes::Bytes {
            let mut signed = Vec::from(SIGNING_PREFIX);
            signed.extend_from_slice(document);
            self.sign_raw(&signed)
        }

        fn sign_raw(&self, message: &[u8]) -> bytes::Bytes {
            let signature = self.pair.sign(&self.random, message).expect("sign");
            bytes::Bytes::copy_from_slice(signature.as_ref())
        }
    }

    fn compiled(signer: &Signer) -> TrustedKeys {
        let keys: &'static [TrustedKey] =
            Box::leak(Box::new([("aex-catalog-prd", signer.public())]));
        TrustedKeys::new(keys)
    }

    #[test]
    fn a_signature_from_a_compiled_key_verifies() {
        let signer = Signer::new();
        let document = bytes::Bytes::from_static(b"{\"schemaVersion\":1}");
        let envelope = CatalogEnvelope {
            document: document.clone(),
            signatures: vec![CatalogSignature {
                key_id: key_id("aex-catalog-prd"),
                algorithm: SigAlg::EcdsaP256Sha256Asn1,
                bytes: signer.sign_with_prefix(&document),
            }],
        };
        assert_eq!(
            verify(&envelope, &compiled(&signer)),
            Ok(key_id("aex-catalog-prd"))
        );
    }

    #[test]
    fn a_single_bit_mutation_of_the_document_fails() {
        let signer = Signer::new();
        let document = bytes::Bytes::from_static(b"{\"schemaVersion\":1}");
        let signature = signer.sign_with_prefix(&document);
        let mut mutated = document.to_vec();
        mutated[2] ^= 0x01;
        let envelope = CatalogEnvelope {
            document: bytes::Bytes::from(mutated),
            signatures: vec![CatalogSignature {
                key_id: key_id("aex-catalog-prd"),
                algorithm: SigAlg::EcdsaP256Sha256Asn1,
                bytes: signature,
            }],
        };
        assert_eq!(
            verify(&envelope, &compiled(&signer)),
            Err(SignatureError::BadSignature(key_id("aex-catalog-prd")))
        );
    }

    #[test]
    fn an_uncompiled_key_is_untrusted_however_valid_its_signature() {
        let signer = Signer::new();
        let document = bytes::Bytes::from_static(b"{\"schemaVersion\":1}");
        let envelope = CatalogEnvelope {
            document: document.clone(),
            signatures: vec![CatalogSignature {
                key_id: key_id("attacker"),
                algorithm: SigAlg::EcdsaP256Sha256Asn1,
                bytes: signer.sign_with_prefix(&document),
            }],
        };
        assert_eq!(
            verify(&envelope, &compiled(&signer)),
            Err(SignatureError::UntrustedKey(key_id("attacker")))
        );
    }

    #[test]
    fn a_valid_trusted_signature_cannot_hide_an_untrusted_signature() {
        let trusted = Signer::new();
        let attacker = Signer::new();
        let document = bytes::Bytes::from_static(b"{\"schemaVersion\":1}");
        for signatures in [
            vec![
                CatalogSignature {
                    key_id: key_id("aex-catalog-prd"),
                    algorithm: SigAlg::EcdsaP256Sha256Asn1,
                    bytes: trusted.sign_with_prefix(&document),
                },
                CatalogSignature {
                    key_id: key_id("attacker"),
                    algorithm: SigAlg::EcdsaP256Sha256Asn1,
                    bytes: attacker.sign_with_prefix(&document),
                },
            ],
            vec![
                CatalogSignature {
                    key_id: key_id("attacker"),
                    algorithm: SigAlg::EcdsaP256Sha256Asn1,
                    bytes: attacker.sign_with_prefix(&document),
                },
                CatalogSignature {
                    key_id: key_id("aex-catalog-prd"),
                    algorithm: SigAlg::EcdsaP256Sha256Asn1,
                    bytes: trusted.sign_with_prefix(&document),
                },
            ],
        ] {
            let envelope = CatalogEnvelope {
                document: document.clone(),
                signatures,
            };
            assert_eq!(
                verify(&envelope, &compiled(&trusted)),
                Err(SignatureError::UntrustedKey(key_id("attacker")))
            );
        }
    }

    #[test]
    fn a_valid_signature_cannot_hide_a_bad_signature_from_a_trusted_key() {
        let first = Signer::new();
        let second = Signer::new();
        let keys: &'static [TrustedKey] = Box::leak(Box::new([
            ("aex-catalog-prd", first.public()),
            ("aex-catalog-next", second.public()),
        ]));
        let document = bytes::Bytes::from_static(b"{\"schemaVersion\":1}");
        let envelope = CatalogEnvelope {
            document: document.clone(),
            signatures: vec![
                CatalogSignature {
                    key_id: key_id("aex-catalog-prd"),
                    algorithm: SigAlg::EcdsaP256Sha256Asn1,
                    bytes: first.sign_with_prefix(&document),
                },
                CatalogSignature {
                    key_id: key_id("aex-catalog-next"),
                    algorithm: SigAlg::EcdsaP256Sha256Asn1,
                    bytes: second.sign_with_prefix(b"different document"),
                },
            ],
        };
        assert_eq!(
            verify(&envelope, &TrustedKeys::new(keys)),
            Err(SignatureError::BadSignature(key_id("aex-catalog-next")))
        );
    }

    #[test]
    fn trusted_key_set_order_does_not_change_rotation_verification() {
        let old = Signer::new();
        let new = Signer::new();
        let old_first: &'static [TrustedKey] = Box::leak(Box::new([
            ("aex-catalog-2025", old.public()),
            ("aex-catalog-2026", new.public()),
        ]));
        let new_first: &'static [TrustedKey] = Box::leak(Box::new([
            ("aex-catalog-2026", new.public()),
            ("aex-catalog-2025", old.public()),
        ]));
        let document = bytes::Bytes::from_static(b"{\"schemaVersion\":1}");
        let envelope = CatalogEnvelope {
            document: document.clone(),
            signatures: vec![
                CatalogSignature {
                    key_id: key_id("aex-catalog-2025"),
                    algorithm: SigAlg::EcdsaP256Sha256Asn1,
                    bytes: old.sign_with_prefix(&document),
                },
                CatalogSignature {
                    key_id: key_id("aex-catalog-2026"),
                    algorithm: SigAlg::EcdsaP256Sha256Asn1,
                    bytes: new.sign_with_prefix(&document),
                },
            ],
        };

        assert_eq!(
            verify(&envelope, &TrustedKeys::new(old_first)),
            Ok(key_id("aex-catalog-2025"))
        );
        assert_eq!(
            verify(&envelope, &TrustedKeys::new(new_first)),
            Ok(key_id("aex-catalog-2025"))
        );
        let mut reversed = envelope;
        reversed.signatures.reverse();
        assert_eq!(
            verify(&reversed, &TrustedKeys::new(new_first)),
            Ok(key_id("aex-catalog-2025"))
        );
    }

    #[test]
    fn signature_count_is_bounded_before_public_key_work() {
        let signer = Signer::new();
        let document = bytes::Bytes::from_static(b"{}");
        let signature = CatalogSignature {
            key_id: key_id("aex-catalog-prd"),
            algorithm: SigAlg::EcdsaP256Sha256Asn1,
            bytes: signer.sign_with_prefix(&document),
        };
        let envelope = CatalogEnvelope {
            document,
            signatures: vec![signature; MAX_SIGNATURES + 1],
        };
        assert_eq!(
            verify(&envelope, &compiled(&signer)),
            Err(SignatureError::TooMany(MAX_SIGNATURES + 1))
        );
    }

    #[test]
    fn an_unsigned_envelope_is_rejected() {
        let envelope = CatalogEnvelope {
            document: bytes::Bytes::from_static(b"{}"),
            signatures: Vec::new(),
        };
        assert_eq!(
            verify(&envelope, &TrustedKeys::empty()),
            Err(SignatureError::Unsigned)
        );
    }

    #[test]
    fn an_empty_compiled_key_set_trusts_nothing() {
        let signer = Signer::new();
        let document = bytes::Bytes::from_static(b"{}");
        let envelope = CatalogEnvelope {
            document: document.clone(),
            signatures: vec![CatalogSignature {
                key_id: key_id("aex-catalog-prd"),
                algorithm: SigAlg::EcdsaP256Sha256Asn1,
                bytes: signer.sign_with_prefix(&document),
            }],
        };
        assert_eq!(
            verify(&envelope, &TrustedKeys::empty()),
            Err(SignatureError::UntrustedKey(key_id("aex-catalog-prd")))
        );
    }

    #[test]
    fn a_duplicated_key_id_is_rejected_before_any_verification() {
        let signer = Signer::new();
        let document = bytes::Bytes::from_static(b"{}");
        let signature = signer.sign_with_prefix(&document);
        let envelope = CatalogEnvelope {
            document,
            signatures: vec![
                CatalogSignature {
                    key_id: key_id("aex-catalog-prd"),
                    algorithm: SigAlg::EcdsaP256Sha256Asn1,
                    bytes: signature.clone(),
                },
                CatalogSignature {
                    key_id: key_id("aex-catalog-prd"),
                    algorithm: SigAlg::EcdsaP256Sha256Asn1,
                    bytes: signature,
                },
            ],
        };
        assert_eq!(
            verify(&envelope, &compiled(&signer)),
            Err(SignatureError::DuplicateKey(key_id("aex-catalog-prd")))
        );
    }

    #[test]
    fn an_oversized_signature_is_rejected_before_verification() {
        let signer = Signer::new();
        let envelope = CatalogEnvelope {
            document: bytes::Bytes::from_static(b"{}"),
            signatures: vec![CatalogSignature {
                key_id: key_id("aex-catalog-prd"),
                algorithm: SigAlg::EcdsaP256Sha256Asn1,
                bytes: bytes::Bytes::from(vec![0u8; 4096]),
            }],
        };
        assert_eq!(
            verify(&envelope, &compiled(&signer)),
            Err(SignatureError::Oversized(key_id("aex-catalog-prd"), 4096))
        );
    }

    #[test]
    fn the_prefix_domain_separates_the_signature() {
        // A signature over the bare document, without the prefix, must not
        // verify as a catalog signature.
        let signer = Signer::new();
        let document = bytes::Bytes::from_static(b"{\"schemaVersion\":1}");
        let bare = signer.sign_raw(&document);
        let envelope = CatalogEnvelope {
            document,
            signatures: vec![CatalogSignature {
                key_id: key_id("aex-catalog-prd"),
                algorithm: SigAlg::EcdsaP256Sha256Asn1,
                bytes: bare,
            }],
        };
        assert_eq!(
            verify(&envelope, &compiled(&signer)),
            Err(SignatureError::BadSignature(key_id("aex-catalog-prd")))
        );
    }

    #[test]
    fn the_envelope_round_trips_through_json() {
        let signer = Signer::new();
        let document = bytes::Bytes::from_static(b"{\"schemaVersion\":1}");
        let envelope = CatalogEnvelope {
            document: document.clone(),
            signatures: vec![CatalogSignature {
                key_id: key_id("aex-catalog-prd"),
                algorithm: SigAlg::EcdsaP256Sha256Asn1,
                bytes: signer.sign_with_prefix(&document),
            }],
        };
        let text = serde_json::to_string(&envelope).expect("serialize");
        let parsed: CatalogEnvelope = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(parsed, envelope);
        assert_eq!(
            verify(&parsed, &compiled(&signer)),
            Ok(key_id("aex-catalog-prd"))
        );
    }
}
