//! Brain `CatalogPort` over one release-bound immutable model-catalog collection.
//!
//! Startup verifies the complete collection before publishing any lookup
//! authority. The hot path is a synchronous `BTreeMap` lookup and cannot reach
//! a network, filesystem or mutable tenant/process authority.

use std::collections::BTreeMap;
use std::sync::Arc;

use aex_brain_application::ports::{CatalogError, CatalogPort};
use aex_brain_domain::ids::{CatalogPin, ModelSlug};
use aex_model_catalog::document::{CatalogDigest, DurableOperationSupport, EntryState};
use aex_model_catalog::signature::{CatalogEnvelope, TrustedKeys};
use aex_model_catalog::{Catalog, CatalogHead, CatalogLoadError, QualifiedModel};
use aex_wire::provider::{ModelSelection, ProviderId};
use serde::{Deserialize, Serialize};

/// Closed schema discriminator for the release/session-retention contract.
pub const CATALOG_COLLECTION_SCHEMA: &str = "aex.model-catalog-collection.v1";

/// Maximum serialized release collection accepted at startup.
///
/// The bound covers all JSON framing, document bytes and detached signatures,
/// so parsing cannot turn an unbounded file into an unbounded resident cache.
pub const MAX_CATALOG_COLLECTION_BYTES: usize = 64 * 1024 * 1024;

/// Maximum serialized envelope accepted inside a collection.
pub const MAX_CATALOG_ENVELOPE_BYTES: usize = 32 * 1024 * 1024;

/// Maximum revisions one process may retain.
///
/// This is a memory/startup-work safety bound, not a retention policy. The
/// release input names every exact still-live pin; it must fail release rather
/// than truncate that set when more revisions remain live.
pub const MAX_CATALOG_REVISIONS: usize = 32;

/// One exact content-addressed signed artifact in release chain order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogArtifact {
    /// Blake3 revision derived from the exact canonical document bytes.
    pub pin: CatalogPin,
    /// Exact document bytes and detached publisher signatures.
    pub envelope: CatalogEnvelope,
}

/// The release/session-retention contract compiled into one `brain-mux` build.
///
/// `live_session_pins` is an explicit output of release-time session retention
/// accounting. It is not "last N" and the loader never guesses it. Artifacts
/// not named there may be present only to connect the predecessor chain to the
/// admission head.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogCollection {
    /// Must equal [`CATALOG_COLLECTION_SCHEMA`].
    pub schema: String,
    /// Exact head used for new admission by this release.
    pub admission_pin: CatalogPin,
    /// Sorted, unique pins required by sessions that may still wake.
    pub live_session_pins: Vec<CatalogPin>,
    /// Oldest-to-newest contiguous signed artifacts.
    pub artifacts: Vec<CatalogArtifact>,
}

/// Why a catalog release collection could not become lookup authority.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogArtifactError {
    /// The binary carries no valid immutable adapter source-tree identity.
    #[error(transparent)]
    BuildIdentity(#[from] crate::build_identity::AdapterBuildIdentityError),
    /// The aggregate collection exceeded the startup allocation bound.
    #[error("model catalog collection is {seen} bytes, above the {limit}-byte startup bound")]
    CollectionTooLarge {
        /// Bytes offered.
        seen: usize,
        /// Hard aggregate bound.
        limit: usize,
    },
    /// The JSON collection could not be decoded under the closed schema.
    #[error("model catalog collection is malformed")]
    MalformedCollection,
    /// The schema discriminator is not the one this binary implements.
    #[error("model catalog collection schema `{0}` is unsupported")]
    UnsupportedSchema(String),
    /// A release with no artifact cannot serve or preserve any pin.
    #[error("model catalog collection carries no artifact")]
    EmptyCollection,
    /// More revisions were supplied than bounded startup permits.
    #[error("model catalog collection carries {seen} revisions, above the {limit}-revision bound")]
    TooManyRevisions {
        /// Revisions offered.
        seen: usize,
        /// Hard count bound.
        limit: usize,
    },
    /// One envelope exceeded its individual allocation/work bound.
    #[error("model catalog envelope for {pin} is {seen} bytes, above the {limit}-byte bound")]
    EnvelopeTooLarge {
        /// Artifact identity.
        pin: CatalogPin,
        /// Serialized bytes.
        seen: usize,
        /// Hard per-envelope bound.
        limit: usize,
    },
    /// Signature, canonical-byte, chain, adapter or conformance verification failed.
    #[error("model catalog artifact {pin} failed verification: {source}")]
    Verification {
        /// Artifact identity named by release input.
        pin: CatalogPin,
        /// Exact catalog load failure.
        source: CatalogLoadError,
    },
    /// The content-addressed name did not match the exact verified document bytes.
    #[error("model catalog artifact names {expected}, but its verified bytes are {actual}")]
    ContentAddressMismatch {
        /// Revision named by the release contract.
        expected: CatalogPin,
        /// Revision derived from exact canonical document bytes.
        actual: CatalogPin,
    },
    /// The same immutable pin appeared twice.
    #[error("model catalog artifact {0} appears more than once")]
    DuplicateArtifact(CatalogPin),
    /// Coverage pins must have one canonical sorted representation.
    #[error("live session catalog pins are not strictly sorted and unique")]
    LivePinsUnsorted,
    /// The release retention contract names a pin absent from the cache.
    #[error("still-live sessions require catalog pin {0}, but the release omitted it")]
    LivePinMissing(CatalogPin),
    /// New admission must use the verified newest chain head.
    #[error("admission pin {expected} is not the verified chain head {actual}")]
    AdmissionHeadMismatch {
        /// Pin named for new admission.
        expected: CatalogPin,
        /// Last verified chain member.
        actual: CatalogPin,
    },
}

/// A fully verified immutable release collection.
///
/// There is deliberately no insertion or replacement method. A refresh is a
/// new release/build whose entire collection passes startup verification.
#[derive(Debug)]
pub struct VerifiedCatalogPort {
    catalogs: BTreeMap<CatalogPin, Arc<Catalog>>,
    admission_pin: CatalogPin,
}

impl VerifiedCatalogPort {
    /// Verifies one build-bound release collection into a fresh immutable map.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogArtifactError`] before publishing any revision when
    /// aggregate size/count, envelope size/decoding, trust signatures, exact
    /// content addresses, chain/time gates, adapter source, Active receipts or
    /// explicit still-live pin coverage does not verify.
    pub fn load_collection(
        bytes: &[u8],
        keys: &TrustedKeys,
        now: aex_wire::types::Timestamp,
    ) -> Result<Self, CatalogArtifactError> {
        if bytes.len() > MAX_CATALOG_COLLECTION_BYTES {
            return Err(CatalogArtifactError::CollectionTooLarge {
                seen: bytes.len(),
                limit: MAX_CATALOG_COLLECTION_BYTES,
            });
        }
        let collection: CatalogCollection =
            serde_json::from_slice(bytes).map_err(|_| CatalogArtifactError::MalformedCollection)?;
        Self::verify_collection(
            collection,
            keys,
            now,
            crate::build_identity::adapter_source_digest()?,
        )
    }

    fn verify_collection(
        collection: CatalogCollection,
        keys: &TrustedKeys,
        now: aex_wire::types::Timestamp,
        adapter: aex_model_catalog::document::AdapterSourceDigest,
    ) -> Result<Self, CatalogArtifactError> {
        if collection.schema != CATALOG_COLLECTION_SCHEMA {
            return Err(CatalogArtifactError::UnsupportedSchema(collection.schema));
        }
        if collection.artifacts.is_empty() {
            return Err(CatalogArtifactError::EmptyCollection);
        }
        if collection.artifacts.len() > MAX_CATALOG_REVISIONS {
            return Err(CatalogArtifactError::TooManyRevisions {
                seen: collection.artifacts.len(),
                limit: MAX_CATALOG_REVISIONS,
            });
        }
        if collection.live_session_pins.len() > MAX_CATALOG_REVISIONS
            || collection
                .live_session_pins
                .windows(2)
                .any(|pins| pins[0] >= pins[1])
        {
            return Err(CatalogArtifactError::LivePinsUnsorted);
        }

        let mut catalogs = BTreeMap::new();
        let mut head: Option<CatalogHead> = None;
        for artifact in collection.artifacts {
            let envelope_size = serde_json::to_vec(&artifact.envelope)
                .map_err(|_| CatalogArtifactError::MalformedCollection)?
                .len();
            if envelope_size > MAX_CATALOG_ENVELOPE_BYTES {
                return Err(CatalogArtifactError::EnvelopeTooLarge {
                    pin: artifact.pin,
                    seen: envelope_size,
                    limit: MAX_CATALOG_ENVELOPE_BYTES,
                });
            }
            let catalog = Catalog::load(&artifact.envelope, keys, now, head.as_ref(), adapter)
                .map_err(|source| CatalogArtifactError::Verification {
                    pin: artifact.pin,
                    source,
                })?;
            let actual = catalog.revision();
            if actual != artifact.pin {
                return Err(CatalogArtifactError::ContentAddressMismatch {
                    expected: artifact.pin,
                    actual,
                });
            }
            head = Some(catalog.head());
            if catalogs.insert(actual, Arc::new(catalog)).is_some() {
                return Err(CatalogArtifactError::DuplicateArtifact(actual));
            }
        }

        let actual_head = head.expect("a non-empty collection produced a head").digest;
        let actual_head = CatalogPin(actual_head.0);
        if collection.admission_pin != actual_head {
            return Err(CatalogArtifactError::AdmissionHeadMismatch {
                expected: collection.admission_pin,
                actual: actual_head,
            });
        }
        for pin in collection.live_session_pins {
            if !catalogs.contains_key(&pin) {
                return Err(CatalogArtifactError::LivePinMissing(pin));
            }
        }

        Ok(Self {
            catalogs,
            admission_pin: collection.admission_pin,
        })
    }

    /// Exact catalog head used for new session admission by this release.
    #[must_use]
    pub const fn admission_pin(&self) -> CatalogPin {
        self.admission_pin
    }

    /// How many verified revisions the process holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.catalogs.len()
    }

    /// Whether no revision is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.catalogs.is_empty()
    }

    /// How many active, receipt-admitted entries the cache can route.
    #[must_use]
    pub fn active_models(&self) -> usize {
        self.catalogs
            .values()
            .map(|catalog| {
                catalog
                    .document()
                    .entries
                    .iter()
                    .filter(|entry| {
                        entry.state == EntryState::Active
                            && catalog.disabled(entry.provider, &entry.model).is_none()
                    })
                    .count()
            })
            .sum()
    }

    /// Whether cryptographic validity also leaves at least one serviceable model.
    #[must_use]
    pub fn is_service_capable(&self) -> bool {
        self.active_models() > 0
    }

    /// Whether a specific immutable revision is present.
    #[must_use]
    pub fn contains(&self, pin: CatalogPin) -> bool {
        self.catalogs.contains_key(&pin)
    }

    fn get(&self, pin: &CatalogPin) -> Result<&Catalog, CatalogError> {
        self.catalogs
            .get(pin)
            .map(AsRef::as_ref)
            .ok_or(CatalogError::UnknownPin { pin: *pin })
    }
}

impl CatalogPort for VerifiedCatalogPort {
    fn digest(&self, pin: &CatalogPin) -> Result<CatalogDigest, CatalogError> {
        self.get(pin).map(Catalog::digest)
    }

    fn model(
        &self,
        pin: &CatalogPin,
        provider: ProviderId,
        model: &ModelSlug,
    ) -> Result<QualifiedModel, CatalogError> {
        let qualified = self
            .get(pin)?
            .qualified(&ModelSelection {
                credential_id: None,
                provider,
                model: model.as_str().to_owned(),
            })
            .map_err(CatalogError::from)?;
        if qualified.state() != EntryState::Active {
            return Err(CatalogError::Lookup(
                aex_model_catalog::CatalogError::UnqualifiedPair {
                    state: qualified.state(),
                },
            ));
        }
        if let Some(reason) = self.get(pin)?.disabled(provider, model) {
            return Err(CatalogError::Lookup(
                aex_model_catalog::CatalogError::EmergencyDisabled { reason },
            ));
        }
        Ok(qualified)
    }

    fn durable_operation_support(
        &self,
        pin: &CatalogPin,
        provider: ProviderId,
        model: &ModelSlug,
    ) -> DurableOperationSupport {
        self.catalogs
            .get(pin)
            .map_or(DurableOperationSupport::None, |catalog| {
                catalog.durable_operation_support(provider, model)
            })
    }
}

#[cfg(test)]
mod tests {
    use aws_lc_rs::rand::SystemRandom;
    use aws_lc_rs::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};

    use super::{
        CATALOG_COLLECTION_SCHEMA, CatalogArtifact, CatalogArtifactError, CatalogCollection,
        MAX_CATALOG_COLLECTION_BYTES, MAX_CATALOG_REVISIONS, VerifiedCatalogPort,
    };
    use aex_brain_application::ports::CatalogPort;
    use aex_brain_domain::ids::CatalogPin;
    use aex_model_catalog::document::{CapabilitySet, CatalogDigest};
    use aex_model_catalog::fixture;
    use aex_model_catalog::primitives::Blake3Digest;
    use aex_model_catalog::signature::{
        CatalogEnvelope, CatalogSignature, P256_PUBLIC_KEY_BYTES, SIGNING_PREFIX, SigAlg,
        SigningKeyId, TrustedKey, TrustedKeys,
    };
    use aex_wire::provider::ProviderId;

    const PUBLISHER: &str = "aex-catalog-test";
    const NOW_MS: i64 = 1_800_000_000_000;

    struct Publisher {
        key_id: &'static str,
        pair: EcdsaKeyPair,
        random: SystemRandom,
        keys: TrustedKeys,
    }

    impl Publisher {
        fn new() -> Self {
            Self::named(PUBLISHER)
        }

        fn named(key_id: &'static str) -> Self {
            let random = SystemRandom::new();
            let pair =
                EcdsaKeyPair::generate(&ECDSA_P256_SHA256_ASN1_SIGNING).expect("key generation");
            let mut public = [0u8; P256_PUBLIC_KEY_BYTES];
            public.copy_from_slice(pair.public_key().as_ref());
            let keys: &'static [TrustedKey] = Box::leak(Box::new([(key_id, public)]));
            Self {
                key_id,
                pair,
                random,
                keys: TrustedKeys::new(keys),
            }
        }

        fn public(&self) -> [u8; P256_PUBLIC_KEY_BYTES] {
            let mut public = [0u8; P256_PUBLIC_KEY_BYTES];
            public.copy_from_slice(self.pair.public_key().as_ref());
            public
        }

        fn artifact(
            &self,
            document: &aex_model_catalog::document::CatalogDocument,
        ) -> CatalogArtifact {
            let document = fixture::canonical_bytes(document);
            let mut signed = Vec::from(SIGNING_PREFIX);
            signed.extend_from_slice(&document);
            let signature = self.pair.sign(&self.random, &signed).expect("sign");
            CatalogArtifact {
                pin: CatalogPin(Blake3Digest::of(&document)),
                envelope: CatalogEnvelope {
                    document,
                    signatures: vec![CatalogSignature {
                        key_id: SigningKeyId(fixture::bounded(self.key_id)),
                        algorithm: SigAlg::EcdsaP256Sha256Asn1,
                        bytes: bytes::Bytes::copy_from_slice(signature.as_ref()),
                    }],
                },
            }
        }
    }

    fn now() -> aex_wire::types::Timestamp {
        fixture::at(NOW_MS)
    }

    fn adapter() -> aex_model_catalog::document::AdapterSourceDigest {
        crate::build_identity::adapter_source_digest().expect("build-stamped adapter")
    }

    fn active_entry(model: &str) -> aex_model_catalog::document::ModelEntry {
        let mut entry = fixture::entry(ProviderId::Openai, model, CapabilitySet::EMPTY);
        fixture::promote(&mut entry, adapter(), now());
        entry
    }

    fn two_revision_collection(active: bool) -> (Publisher, CatalogCollection) {
        let publisher = Publisher::new();
        let first_entry = if active {
            active_entry("gpt-old")
        } else {
            fixture::entry(ProviderId::Openai, "gpt-old", CapabilitySet::EMPTY)
        };
        let first_document = fixture::document(PUBLISHER, 1, vec![first_entry], now(), adapter());
        let first = publisher.artifact(&first_document);

        let second_entry = if active {
            active_entry("gpt-new")
        } else {
            fixture::entry(ProviderId::Openai, "gpt-new", CapabilitySet::EMPTY)
        };
        let mut second_document =
            fixture::document(PUBLISHER, 2, vec![second_entry], now(), adapter());
        second_document.predecessor = Some(CatalogDigest(first.pin.0));
        let second = publisher.artifact(&second_document);

        let collection = CatalogCollection {
            schema: CATALOG_COLLECTION_SCHEMA.to_owned(),
            admission_pin: second.pin,
            live_session_pins: vec![first.pin],
            artifacts: vec![first, second],
        };
        (publisher, collection)
    }

    fn load(
        publisher: &Publisher,
        collection: &CatalogCollection,
    ) -> Result<VerifiedCatalogPort, CatalogArtifactError> {
        let bytes = serde_json::to_vec(collection).expect("collection JSON");
        VerifiedCatalogPort::load_collection(&bytes, &publisher.keys, now())
    }

    #[test]
    fn the_complete_chain_preserves_old_session_pins_and_the_admission_head() {
        let (publisher, collection) = two_revision_collection(true);
        let old_pin = collection.artifacts[0].pin;
        let new_pin = collection.artifacts[1].pin;
        let port = load(&publisher, &collection).expect("complete signed collection");

        assert_eq!(port.len(), 2);
        assert_eq!(port.admission_pin(), new_pin);
        assert!(port.contains(old_pin));
        assert!(port.contains(new_pin));
        assert!(port.is_service_capable());
        assert!(
            port.model(
                &old_pin,
                ProviderId::Openai,
                &fixture::entry(ProviderId::Openai, "gpt-old", CapabilitySet::EMPTY).model,
            )
            .is_ok(),
            "a still-live pinned session resolves synchronously from the immutable cache"
        );
    }

    #[test]
    fn an_overlapping_trust_set_preserves_live_sessions_across_key_rotation() {
        let old = Publisher::named("aex-catalog-2025");
        let new = Publisher::named("aex-catalog-2026");
        let first_document = fixture::document(
            PUBLISHER,
            1,
            vec![active_entry("gpt-old")],
            now(),
            adapter(),
        );
        let first = old.artifact(&first_document);
        let mut second_document = fixture::document(
            PUBLISHER,
            2,
            vec![active_entry("gpt-new")],
            now(),
            adapter(),
        );
        second_document.predecessor = Some(CatalogDigest(first.pin.0));
        let second = new.artifact(&second_document);
        let old_pin = first.pin;
        let new_pin = second.pin;
        let collection = CatalogCollection {
            schema: CATALOG_COLLECTION_SCHEMA.to_owned(),
            admission_pin: new_pin,
            live_session_pins: vec![old_pin],
            artifacts: vec![first, second],
        };
        let keys: &'static [TrustedKey] = Box::leak(Box::new([
            (old.key_id, old.public()),
            (new.key_id, new.public()),
        ]));
        let bytes = serde_json::to_vec(&collection).expect("collection JSON");
        let port = VerifiedCatalogPort::load_collection(&bytes, &TrustedKeys::new(keys), now())
            .expect("overlap release verifies old and new artifacts");

        assert!(port.contains(old_pin));
        assert!(port.contains(new_pin));
        assert_eq!(port.catalogs[&old_pin].signed_by().0.as_str(), old.key_id);
        assert_eq!(port.catalogs[&new_pin].signed_by().0.as_str(), new.key_id);
    }

    #[test]
    fn loading_only_the_newest_revision_cannot_strand_a_declared_live_pin() {
        let (publisher, mut collection) = two_revision_collection(true);
        let old_pin = collection.artifacts[0].pin;
        collection.artifacts.remove(0);
        assert_eq!(
            load(&publisher, &collection).expect_err("coverage must fail"),
            CatalogArtifactError::LivePinMissing(old_pin)
        );
    }

    #[test]
    fn a_broken_intermediate_chain_refuses_the_entire_collection() {
        let (publisher, mut collection) = two_revision_collection(true);
        let second_document: aex_model_catalog::document::CatalogDocument =
            serde_json::from_slice(&collection.artifacts[1].envelope.document)
                .expect("catalog document");
        let mut broken = second_document;
        broken.predecessor = Some(CatalogDigest(Blake3Digest::of(b"not the predecessor")));
        collection.artifacts[1] = publisher.artifact(&broken);
        collection.admission_pin = collection.artifacts[1].pin;

        assert!(matches!(
            load(&publisher, &collection),
            Err(CatalogArtifactError::Verification {
                source: aex_model_catalog::CatalogLoadError::BrokenChain { .. },
                ..
            })
        ));
    }

    #[test]
    fn exact_content_addresses_are_checked_for_every_artifact() {
        let (publisher, mut collection) = two_revision_collection(true);
        let actual = collection.artifacts[0].pin;
        collection.artifacts[0].pin = CatalogPin(Blake3Digest::of(b"misnamed"));
        let expected = collection.artifacts[0].pin;
        assert!(matches!(
            load(&publisher, &collection),
            Err(CatalogArtifactError::ContentAddressMismatch {
                expected: found_expected,
                actual: found_actual,
            }) if found_expected == expected && found_actual == actual
        ));
    }

    #[test]
    fn aggregate_size_and_revision_count_are_bounded_before_verification() {
        let publisher = Publisher::new();
        assert_eq!(
            VerifiedCatalogPort::load_collection(
                &vec![b' '; MAX_CATALOG_COLLECTION_BYTES + 1],
                &publisher.keys,
                now(),
            )
            .expect_err("aggregate size must fail"),
            CatalogArtifactError::CollectionTooLarge {
                seen: MAX_CATALOG_COLLECTION_BYTES + 1,
                limit: MAX_CATALOG_COLLECTION_BYTES,
            }
        );

        let (_, mut collection) = two_revision_collection(true);
        collection.artifacts = vec![collection.artifacts[0].clone(); MAX_CATALOG_REVISIONS + 1];
        assert_eq!(
            load(&publisher, &collection).expect_err("count must fail"),
            CatalogArtifactError::TooManyRevisions {
                seen: MAX_CATALOG_REVISIONS + 1,
                limit: MAX_CATALOG_REVISIONS,
            }
        );
    }

    #[test]
    fn a_verified_collection_with_no_active_model_is_not_service_capable() {
        let (publisher, collection) = two_revision_collection(false);
        let port = load(&publisher, &collection).expect("cryptographically valid collection");
        assert_eq!(port.active_models(), 0);
        assert!(!port.is_service_capable());
    }

    #[test]
    fn the_admission_pin_must_be_the_verified_newest_head() {
        let (publisher, mut collection) = two_revision_collection(true);
        let old = collection.artifacts[0].pin;
        let new = collection.artifacts[1].pin;
        collection.admission_pin = old;
        assert_eq!(
            load(&publisher, &collection).expect_err("head must fail"),
            CatalogArtifactError::AdmissionHeadMismatch {
                expected: old,
                actual: new,
            }
        );
    }

    #[test]
    fn live_pin_coverage_has_one_sorted_unique_release_representation() {
        let (publisher, mut collection) = two_revision_collection(true);
        let pin = collection.live_session_pins[0];
        collection.live_session_pins = vec![pin, pin];
        assert_eq!(
            load(&publisher, &collection).expect_err("coverage order must fail"),
            CatalogArtifactError::LivePinsUnsorted
        );
    }

    #[test]
    fn a_duplicate_artifact_is_rejected_deterministically() {
        let (publisher, mut collection) = two_revision_collection(true);
        let duplicate = collection.artifacts[0].clone();
        collection.artifacts.insert(1, duplicate.clone());
        assert_eq!(
            load(&publisher, &collection).expect_err("duplicate must fail"),
            CatalogArtifactError::DuplicateArtifact(duplicate.pin)
        );
    }
}
