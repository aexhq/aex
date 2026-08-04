//! Fail-closed, deterministic model-catalog publication primitives.
//!
//! The protected signer remains external. This module prepares the exact
//! digest AWS KMS must sign, records its closed response, and assembles bytes
//! only after the shared runtime verifier accepts the signature, catalog,
//! conformance receipts, adapter identity and serviceability.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use aex_model_catalog::collection::{CatalogCollection, VerifiedCatalogCollection};
use aex_model_catalog::document::{AdapterSourceDigest, CatalogDocument, EntryState};
use aex_model_catalog::signature::{
    CatalogEnvelope, CatalogSignature, MAX_SIGNATURE_BYTES, P256_PUBLIC_KEY_BYTES, SIGNING_PREFIX,
    SigAlg, SigningKeyId, TrustedKey, TrustedKeys,
};
use aex_model_catalog::{BoundedString, Catalog, CatalogRevision};
use aex_wire::to_jcs_bytes;
use aex_wire::types::Timestamp;

/// Closed request emitted before the protected KMS signing step.
pub const SIGNING_REQUEST_SCHEMA: &str = "aex.model-catalog-signing-request.v1";
/// Closed record emitted from an AWS KMS `Sign` response.
pub const SIGNATURE_RECORD_SCHEMA: &str = "aex.model-catalog-signature-record.v1";
/// Closed release bindings emitted beside the signed collection.
pub const PUBLICATION_BINDING_SCHEMA: &str = "aex.model-catalog-publication-binding.v1";
/// Existing build-time trust-root schema consumed by `brain-mux`.
pub const TRUST_ROOTS_SCHEMA: &str = "aex.model-catalog-trust-roots.v1";
/// The only KMS signing algorithm this protocol admits.
pub const KMS_SIGNING_ALGORITHM: &str = "ECDSA_SHA_256";
/// KMS receives a precomputed digest, avoiding its 4 KiB raw-message bound.
pub const KMS_MESSAGE_TYPE: &str = "DIGEST";
const MAX_TRUST_ROOTS: usize = 8;
const MAX_TRUST_ROOTS_BYTES: usize = 8 * 1024;

/// Exact request a protected signer consumes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SigningRequest {
    /// Schema discriminator.
    pub schema: String,
    /// Build-stamped adapter identity the receipt and document require.
    pub adapter_source: AdapterSourceDigest,
    /// Content address of the exact canonical document bytes.
    pub catalog_revision: CatalogRevision,
    /// KMS signing algorithm.
    pub signing_algorithm: String,
    /// KMS message type.
    pub message_type: String,
    /// SHA-256 digest KMS signs, base64-encoded for its API.
    pub message_digest_base64: String,
    /// Digest identity used to bind the later signature record.
    pub message_digest_sha256: String,
}

/// Minimal closed projection of `aws kms sign` JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KmsSignOutput {
    /// KMS key ARN/id returned by the service, retained for audit correlation.
    #[serde(rename = "KeyId")]
    pub provider_key_id: String,
    /// DER signature encoded by the AWS CLI as base64.
    #[serde(rename = "Signature")]
    pub signature: String,
    /// Must be `ECDSA_SHA_256`.
    #[serde(rename = "SigningAlgorithm")]
    pub signing_algorithm: String,
}

/// Detached signature plus the request digest it answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SignatureRecord {
    /// Schema discriminator.
    pub schema: String,
    /// Stable logical key id compiled into the runtime trust set.
    pub key_id: String,
    /// KMS key ARN/id returned by the service.
    pub provider_key_id: String,
    /// KMS signing algorithm.
    pub signing_algorithm: String,
    /// Request digest this response answers.
    pub message_digest_sha256: String,
    /// Detached ASN.1 DER signature, base64-encoded.
    pub signature: String,
}

/// Build-time identities to install after successful assembly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicationBinding {
    /// Schema discriminator.
    pub schema: String,
    /// Exact admission revision.
    pub catalog_revision: CatalogRevision,
    /// Build-stamped adapter source identity.
    pub adapter_source: AdapterSourceDigest,
    /// SHA-256 of canonical trust-root JSON.
    pub trust_roots_sha256: String,
    /// SHA-256 of canonical collection JSON.
    pub collection_sha256: String,
}

/// Successfully assembled canonical collection bytes and their bindings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publication {
    /// Canonical collection JSON.
    pub collection: Vec<u8>,
    /// Non-secret exact identities consumed by release configuration.
    pub binding: PublicationBinding,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrustRootsDocument {
    keys: Vec<TrustRoot>,
    schema: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TrustRoot {
    key_id: String,
    sec1: String,
}

/// Why publication refused to create authority.
#[derive(Debug, thiserror::Error)]
pub enum PublisherError {
    /// An input is not the one canonical JSON representation.
    #[error("{input} must be exact canonical JSON with no trailing bytes")]
    NonCanonical {
        /// Input contract being checked.
        input: &'static str,
    },
    /// A closed input could not be decoded.
    #[error("{input} is malformed: {reason}")]
    Malformed {
        /// Input contract being checked.
        input: &'static str,
        /// Bounded-to-process diagnostic from the parser.
        reason: String,
    },
    /// The document is structurally invalid or lacks earned evidence.
    #[error("catalog document preflight failed: {0}")]
    Catalog(#[from] aex_model_catalog::CatalogLoadError),
    /// The document has no model that can serve new admission.
    #[error("catalog document contains no Active model with current conformance evidence")]
    NoActiveModel,
    /// A signing response does not answer the prepared request.
    #[error("signature response does not match the prepared request: {0}")]
    SignatureBinding(&'static str),
    /// The detached signature is malformed or outside its bound.
    #[error("detached signature is invalid: {0}")]
    Signature(String),
    /// The trust-root document is invalid.
    #[error("publisher trust roots are invalid: {0}")]
    TrustRoots(String),
    /// Full runtime-equivalent collection verification failed.
    #[error("signed catalog collection failed verification: {0}")]
    Collection(#[from] aex_model_catalog::CatalogCollectionError),
}

/// Validates exact canonical document bytes and emits the KMS digest request.
///
/// # Errors
///
/// Refuses noncanonical bytes, adapter drift, invalid receipts, invalid time
/// gates and documents with no currently usable `Active` model.
pub fn prepare_signing_request(
    document_bytes: &[u8],
    now: Timestamp,
    adapter: AdapterSourceDigest,
) -> Result<SigningRequest, PublisherError> {
    let document: CatalogDocument = parse_canonical(document_bytes, "catalog document")?;
    let head = Catalog::preflight_document(document_bytes, now, None, adapter)?;
    if !document.entries.iter().any(|entry| {
        entry.state == EntryState::Active
            && entry.receipt.expires_at > now
            && now < document.expires_at
            && !document
                .emergency_disable
                .iter()
                .any(|row| row.provider == entry.provider && row.model == entry.model)
    }) {
        return Err(PublisherError::NoActiveModel);
    }
    let mut message = Vec::with_capacity(SIGNING_PREFIX.len() + document_bytes.len());
    message.extend_from_slice(SIGNING_PREFIX);
    message.extend_from_slice(document_bytes);
    let digest = Sha256::digest(&message);
    Ok(SigningRequest {
        schema: SIGNING_REQUEST_SCHEMA.to_owned(),
        adapter_source: adapter,
        catalog_revision: CatalogRevision(head.digest.0),
        signing_algorithm: KMS_SIGNING_ALGORITHM.to_owned(),
        message_type: KMS_MESSAGE_TYPE.to_owned(),
        message_digest_base64: BASE64.encode(digest),
        message_digest_sha256: format!("sha256:{}", hex::encode(digest)),
    })
}

/// Converts exact AWS KMS output into the closed detached-signature record.
///
/// # Errors
///
/// Refuses a foreign request schema/algorithm or malformed/oversized DER bytes.
pub fn record_kms_signature(
    request: &SigningRequest,
    logical_key_id: &str,
    output: KmsSignOutput,
) -> Result<SignatureRecord, PublisherError> {
    validate_request(request)?;
    validate_key_id(logical_key_id)?;
    if output.signing_algorithm != KMS_SIGNING_ALGORITHM {
        return Err(PublisherError::SignatureBinding("KMS algorithm differs"));
    }
    if output.provider_key_id.trim().is_empty() {
        return Err(PublisherError::SignatureBinding("KMS key id is empty"));
    }
    let signature = decode_signature(&output.signature)?;
    if signature.is_empty() {
        return Err(PublisherError::Signature("signature is empty".to_owned()));
    }
    Ok(SignatureRecord {
        schema: SIGNATURE_RECORD_SCHEMA.to_owned(),
        key_id: logical_key_id.to_owned(),
        provider_key_id: output.provider_key_id,
        signing_algorithm: output.signing_algorithm,
        message_digest_sha256: request.message_digest_sha256.clone(),
        signature: BASE64.encode(signature),
    })
}

/// Assembles and runtime-verifies the first production collection.
///
/// # Errors
///
/// Refuses every mismatch before returning bytes, including a bad signature,
/// unknown trust root, stale receipt, adapter mismatch or zero serviceable model.
pub fn assemble_genesis(
    document_bytes: &[u8],
    record: &SignatureRecord,
    trust_roots_json: &[u8],
    now: Timestamp,
    adapter: AdapterSourceDigest,
) -> Result<Publication, PublisherError> {
    let request = prepare_signing_request(document_bytes, now, adapter)?;
    if record.schema != SIGNATURE_RECORD_SCHEMA {
        return Err(PublisherError::SignatureBinding("record schema differs"));
    }
    if record.signing_algorithm != request.signing_algorithm {
        return Err(PublisherError::SignatureBinding("record algorithm differs"));
    }
    if record.message_digest_sha256 != request.message_digest_sha256 {
        return Err(PublisherError::SignatureBinding("record digest differs"));
    }
    validate_key_id(&record.key_id)?;
    let signature = decode_signature(&record.signature)?;
    let roots = load_trusted_keys(trust_roots_json)?;
    let envelope = CatalogEnvelope {
        document: bytes::Bytes::copy_from_slice(document_bytes),
        signatures: vec![CatalogSignature {
            key_id: SigningKeyId(
                BoundedString::new(record.key_id.clone())
                    .map_err(|error| PublisherError::TrustRoots(error.to_string()))?,
            ),
            algorithm: SigAlg::EcdsaP256Sha256Asn1,
            bytes: bytes::Bytes::from(signature),
        }],
    };
    let collection = CatalogCollection::genesis(envelope);
    let collection_bytes = collection.canonical_bytes()?;
    let verified = VerifiedCatalogCollection::load(&collection_bytes, &roots, now, adapter)?;
    if !verified.is_service_capable() {
        return Err(PublisherError::NoActiveModel);
    }
    Ok(Publication {
        binding: PublicationBinding {
            schema: PUBLICATION_BINDING_SCHEMA.to_owned(),
            catalog_revision: verified.admission_pin(),
            adapter_source: adapter,
            trust_roots_sha256: sha256_identity(trust_roots_json),
            collection_sha256: sha256_identity(&collection_bytes),
        },
        collection: collection_bytes,
    })
}

fn validate_request(request: &SigningRequest) -> Result<(), PublisherError> {
    if request.schema != SIGNING_REQUEST_SCHEMA {
        return Err(PublisherError::SignatureBinding("request schema differs"));
    }
    if request.signing_algorithm != KMS_SIGNING_ALGORITHM {
        return Err(PublisherError::SignatureBinding(
            "request algorithm differs",
        ));
    }
    if request.message_type != KMS_MESSAGE_TYPE {
        return Err(PublisherError::SignatureBinding(
            "request message type differs",
        ));
    }
    let digest = BASE64
        .decode(&request.message_digest_base64)
        .map_err(|error| PublisherError::Malformed {
            input: "signing request digest",
            reason: error.to_string(),
        })?;
    if digest.len() != 32
        || format!("sha256:{}", hex::encode(&digest)) != request.message_digest_sha256
    {
        return Err(PublisherError::SignatureBinding(
            "request digest identity differs",
        ));
    }
    Ok(())
}

fn parse_canonical<T>(bytes: &[u8], input: &'static str) -> Result<T, PublisherError>
where
    T: serde::de::DeserializeOwned + Serialize,
{
    let value = serde_json::from_slice(bytes).map_err(|error| PublisherError::Malformed {
        input,
        reason: error.to_string(),
    })?;
    let canonical = to_jcs_bytes(&value).map_err(|error| PublisherError::Malformed {
        input,
        reason: error.to_string(),
    })?;
    if canonical != bytes {
        return Err(PublisherError::NonCanonical { input });
    }
    Ok(value)
}

fn load_trusted_keys(bytes: &[u8]) -> Result<TrustedKeys, PublisherError> {
    if bytes.len() > MAX_TRUST_ROOTS_BYTES {
        return Err(PublisherError::TrustRoots(format!(
            "document is {} bytes, above the {MAX_TRUST_ROOTS_BYTES}-byte bound",
            bytes.len()
        )));
    }
    let roots: TrustRootsDocument = parse_canonical(bytes, "publisher trust roots")?;
    if roots.schema != TRUST_ROOTS_SCHEMA
        || roots.keys.is_empty()
        || roots.keys.len() > MAX_TRUST_ROOTS
    {
        return Err(PublisherError::TrustRoots(format!(
            "schema must be {TRUST_ROOTS_SCHEMA} with 1..={MAX_TRUST_ROOTS} keys"
        )));
    }
    if roots
        .keys
        .windows(2)
        .any(|pair| pair[0].key_id >= pair[1].key_id)
    {
        return Err(PublisherError::TrustRoots(
            "key ids must be strictly sorted and unique".to_owned(),
        ));
    }
    let mut trusted: Vec<TrustedKey> = Vec::with_capacity(roots.keys.len());
    for root in roots.keys {
        validate_key_id(&root.key_id)?;
        let public = decode_public_key(&root.sec1)?;
        let id: &'static str = Box::leak(root.key_id.into_boxed_str());
        trusted.push((id, public));
    }
    let trusted: &'static [TrustedKey] = Box::leak(trusted.into_boxed_slice());
    Ok(TrustedKeys::new(trusted))
}

fn validate_key_id(key_id: &str) -> Result<(), PublisherError> {
    if key_id.is_empty()
        || key_id.len() > 64
        || !key_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(PublisherError::TrustRoots(
            "key id must be 1..=64 ASCII alphanumeric, '-' or '_'".to_owned(),
        ));
    }
    Ok(())
}

fn decode_public_key(encoded: &str) -> Result<[u8; P256_PUBLIC_KEY_BYTES], PublisherError> {
    if encoded.len() != P256_PUBLIC_KEY_BYTES * 2
        || !encoded
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(PublisherError::TrustRoots(
            "SEC1 key must be 130 lowercase hexadecimal characters".to_owned(),
        ));
    }
    let decoded =
        hex::decode(encoded).map_err(|error| PublisherError::TrustRoots(error.to_string()))?;
    p256::ecdsa::VerifyingKey::from_sec1_bytes(&decoded)
        .map_err(|_| PublisherError::TrustRoots("SEC1 key is not a P-256 point".to_owned()))?;
    let mut public = [0_u8; P256_PUBLIC_KEY_BYTES];
    public.copy_from_slice(&decoded);
    Ok(public)
}

fn decode_signature(encoded: &str) -> Result<Vec<u8>, PublisherError> {
    let bytes = BASE64
        .decode(encoded)
        .map_err(|error| PublisherError::Signature(error.to_string()))?;
    if bytes.len() > MAX_SIGNATURE_BYTES {
        return Err(PublisherError::Signature(format!(
            "{} bytes exceeds the {MAX_SIGNATURE_BYTES}-byte bound",
            bytes.len()
        )));
    }
    p256::ecdsa::Signature::from_der(&bytes).map_err(|_| {
        PublisherError::Signature("bytes are not an ASN.1 DER P-256 signature".to_owned())
    })?;
    Ok(bytes)
}

fn sha256_identity(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

/// Emits exact canonical JSON for a publisher protocol value.
///
/// # Errors
///
/// Returns a serialization error for a value that cannot be represented.
pub fn canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>, PublisherError> {
    to_jcs_bytes(value).map_err(|error| PublisherError::Malformed {
        input: "publisher output",
        reason: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use aws_lc_rs::rand::SystemRandom;
    use aws_lc_rs::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};
    use base64::Engine as _;

    use aex_model_catalog::document::CapabilitySet;
    use aex_model_catalog::fixture;
    use aex_wire::provider::ProviderId;
    use sha2::Digest as _;

    use super::{
        BASE64, KMS_SIGNING_ALGORITHM, KmsSignOutput, PublisherError, SIGNING_PREFIX,
        SignatureRecord, TrustRoot, TrustRootsDocument, assemble_genesis, canonical_json,
        prepare_signing_request, record_kms_signature,
    };

    const NOW_MS: i64 = 1_800_000_000_000;

    struct Signer {
        pair: EcdsaKeyPair,
        random: SystemRandom,
    }

    impl Signer {
        fn new() -> Self {
            let random = SystemRandom::new();
            let pair = EcdsaKeyPair::generate(&ECDSA_P256_SHA256_ASN1_SIGNING)
                .expect("P-256 key generation");
            Self { pair, random }
        }

        fn sign(&self, document: &[u8]) -> String {
            let mut message = Vec::from(SIGNING_PREFIX);
            message.extend_from_slice(document);
            let signature = self.pair.sign(&self.random, &message).expect("sign");
            BASE64.encode(signature.as_ref())
        }

        fn roots(&self) -> Vec<u8> {
            canonical_json(&TrustRootsDocument {
                keys: vec![TrustRoot {
                    key_id: "catalog-test".to_owned(),
                    sec1: hex::encode(self.pair.public_key().as_ref()),
                }],
                schema: super::TRUST_ROOTS_SCHEMA.to_owned(),
            })
            .expect("canonical roots")
        }
    }

    fn adapter() -> aex_model_catalog::document::AdapterSourceDigest {
        fixture::adapter("publisher-adapter")
    }

    fn now() -> aex_wire::types::Timestamp {
        fixture::at(NOW_MS)
    }

    fn active_document() -> Vec<u8> {
        let mut entry = fixture::entry(ProviderId::Openai, "gpt-test", CapabilitySet::EMPTY);
        fixture::promote(&mut entry, adapter(), now());
        fixture::canonical_bytes(&fixture::document(
            "catalog-test",
            1,
            vec![entry],
            now(),
            adapter(),
        ))
        .to_vec()
    }

    fn signature_record(document: &[u8], signer: &Signer) -> SignatureRecord {
        let request = prepare_signing_request(document, now(), adapter()).expect("request");
        record_kms_signature(
            &request,
            "catalog-test",
            KmsSignOutput {
                provider_key_id: "arn:aws:kms:eu-west-1:000000000000:key/test".to_owned(),
                signature: signer.sign(document),
                signing_algorithm: KMS_SIGNING_ALGORITHM.to_owned(),
            },
        )
        .expect("signature record")
    }

    #[test]
    fn staged_fixture_cannot_be_prepared_for_signing() {
        let document = fixture::canonical_bytes(&fixture::document(
            "catalog-test",
            1,
            vec![fixture::entry(
                ProviderId::Openai,
                "gpt-test",
                CapabilitySet::EMPTY,
            )],
            now(),
            adapter(),
        ));
        assert!(matches!(
            prepare_signing_request(&document, now(), adapter()),
            Err(PublisherError::NoActiveModel)
        ));
    }

    #[test]
    fn signing_request_is_exact_kms_digest_mode_not_an_unbounded_raw_message() {
        let document = active_document();
        let request = prepare_signing_request(&document, now(), adapter()).expect("request");
        let mut message = Vec::from(SIGNING_PREFIX);
        message.extend_from_slice(&document);
        let expected = sha2::Sha256::digest(message);
        assert_eq!(request.message_digest_base64, BASE64.encode(expected));
        assert_eq!(request.message_type, "DIGEST");
        assert_eq!(request.signing_algorithm, KMS_SIGNING_ALGORITHM);
    }

    #[test]
    fn a_foreign_kms_algorithm_never_becomes_a_signature_record() {
        let document = active_document();
        let request = prepare_signing_request(&document, now(), adapter()).expect("request");
        let error = record_kms_signature(
            &request,
            "catalog-test",
            KmsSignOutput {
                provider_key_id: "key/test".to_owned(),
                signature: BASE64.encode([1_u8; 64]),
                signing_algorithm: "RSASSA_PSS_SHA_256".to_owned(),
            },
        )
        .expect_err("algorithm must fail");
        assert!(matches!(error, PublisherError::SignatureBinding(_)));
    }

    #[test]
    fn assembly_emits_only_runtime_verified_canonical_bytes_and_exact_bindings() {
        let document = active_document();
        let signer = Signer::new();
        let roots = signer.roots();
        let publication = assemble_genesis(
            &document,
            &signature_record(&document, &signer),
            &roots,
            now(),
            adapter(),
        )
        .expect("verified publication");

        let parsed: aex_model_catalog::CatalogCollection =
            serde_json::from_slice(&publication.collection).expect("collection");
        assert_eq!(
            publication.collection,
            parsed.canonical_bytes().expect("canonical bytes")
        );
        assert_eq!(publication.binding.catalog_revision, parsed.admission_pin);
        assert!(publication.binding.collection_sha256.starts_with("sha256:"));
        assert!(
            publication
                .binding
                .trust_roots_sha256
                .starts_with("sha256:")
        );
    }

    #[test]
    fn a_signature_over_different_bytes_cannot_publish() {
        let document = active_document();
        let signer = Signer::new();
        let mut record = signature_record(&document, &signer);
        record.signature = signer.sign(b"different canonical document");
        assert!(matches!(
            assemble_genesis(&document, &record, &signer.roots(), now(), adapter()),
            Err(PublisherError::Collection(_))
        ));
    }

    #[test]
    fn a_noncanonical_document_is_refused_before_signing() {
        let mut document = active_document();
        document.push(b'\n');
        assert!(matches!(
            prepare_signing_request(&document, now(), adapter()),
            Err(PublisherError::NonCanonical { .. })
        ));
    }
}
