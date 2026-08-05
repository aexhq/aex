//! Brain `CatalogPort` over one release-bound immutable model-catalog collection.
//!
//! Startup verifies the complete collection before publishing any lookup
//! authority. The hot path is a synchronous `BTreeMap` lookup and cannot reach
//! a network, filesystem or mutable tenant/process authority.

use aex_brain_application::ports::{CatalogError, CatalogPort};
use aex_brain_domain::ids::{CatalogPin, ModelSlug};
pub use aex_model_catalog::collection::{
    CATALOG_COLLECTION_SCHEMA, CatalogArtifact, CatalogCollection, CatalogCollectionError,
    MAX_CATALOG_COLLECTION_BYTES, MAX_CATALOG_REVISIONS,
};
use aex_model_catalog::document::{CatalogDigest, DurableOperationSupport, EntryState};
use aex_model_catalog::signature::TrustedKeys;
use aex_model_catalog::{Catalog, QualifiedModel, VerifiedCatalogCollection};
use aex_wire::provider::{ModelSelection, ProviderId};

/// Why a catalog release collection could not become lookup authority.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogArtifactError {
    /// The shared signed-collection authority rejected the bytes.
    #[error(transparent)]
    Collection(#[from] CatalogCollectionError),
}

/// A fully verified immutable release collection.
///
/// There is deliberately no insertion or replacement method. A refresh is a
/// new release/build whose entire collection passes startup verification.
#[derive(Debug)]
pub struct VerifiedCatalogPort {
    inner: VerifiedCatalogCollection,
}

impl VerifiedCatalogPort {
    /// Verifies one build-bound release collection into a fresh immutable map.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogArtifactError`] before publishing any revision when
    /// aggregate size/count, envelope size/decoding, trust signatures, exact
    /// content addresses, chain/time/compatibility gates or explicit
    /// still-live pin coverage does not verify.
    pub fn load_collection(
        bytes: &[u8],
        keys: &TrustedKeys,
        now: aex_wire::types::Timestamp,
    ) -> Result<Self, CatalogArtifactError> {
        let inner = VerifiedCatalogCollection::load(bytes, keys, now)?;
        Ok(Self { inner })
    }

    /// Exact catalog head used for new session admission by this release.
    #[must_use]
    pub const fn admission_pin(&self) -> CatalogPin {
        self.inner.admission_pin()
    }

    /// Every immutable revision retained for sessions that may still wake.
    ///
    /// Composition uses this exact set when installing the tool catalog. Installing tools
    /// only for the admission head would make an older, explicitly retained session fail
    /// after its wake had already been consumed.
    pub fn retained_pins(&self) -> impl Iterator<Item = CatalogPin> + '_ {
        self.inner.retained_pins()
    }

    /// How many verified revisions the process holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether no revision is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// How many signed, active entries the cache can route.
    #[must_use]
    pub fn active_models(&self) -> usize {
        self.inner.active_models()
    }

    /// Whether cryptographic validity also leaves at least one serviceable model.
    #[must_use]
    pub fn is_service_capable(&self) -> bool {
        self.inner.is_service_capable()
    }

    /// Whether a specific immutable revision is present.
    #[must_use]
    pub fn contains(&self, pin: CatalogPin) -> bool {
        self.inner.catalog(pin).is_some()
    }

    fn get(&self, pin: &CatalogPin) -> Result<&Catalog, CatalogError> {
        self.inner
            .catalog(*pin)
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
        self.inner
            .catalog(*pin)
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
    use aex_model_catalog::CatalogCollectionError;
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

    fn active_entry(model: &str) -> aex_model_catalog::document::ModelEntry {
        let mut entry = fixture::entry(ProviderId::Openai, model, CapabilitySet::EMPTY);
        fixture::promote(&mut entry);
        entry
    }

    fn two_revision_collection(active: bool) -> (Publisher, CatalogCollection) {
        let publisher = Publisher::new();
        let first_entry = if active {
            active_entry("gpt-old")
        } else {
            fixture::entry(ProviderId::Openai, "gpt-old", CapabilitySet::EMPTY)
        };
        let first_document = fixture::document(PUBLISHER, 1, vec![first_entry], now());
        let first = publisher.artifact(&first_document);

        let second_entry = if active {
            active_entry("gpt-new")
        } else {
            fixture::entry(ProviderId::Openai, "gpt-new", CapabilitySet::EMPTY)
        };
        let mut second_document = fixture::document(PUBLISHER, 2, vec![second_entry], now());
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
        let bytes = collection
            .canonical_bytes()
            .expect("canonical collection JSON");
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
        let first_document = fixture::document(PUBLISHER, 1, vec![active_entry("gpt-old")], now());
        let first = old.artifact(&first_document);
        let mut second_document =
            fixture::document(PUBLISHER, 2, vec![active_entry("gpt-new")], now());
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
        let bytes = collection
            .canonical_bytes()
            .expect("canonical collection JSON");
        let port = VerifiedCatalogPort::load_collection(&bytes, &TrustedKeys::new(keys), now())
            .expect("overlap release verifies old and new artifacts");

        assert!(port.contains(old_pin));
        assert!(port.contains(new_pin));
        assert_eq!(
            port.get(&old_pin).unwrap().signed_by().0.as_str(),
            old.key_id
        );
        assert_eq!(
            port.get(&new_pin).unwrap().signed_by().0.as_str(),
            new.key_id
        );
    }

    #[test]
    fn loading_only_the_newest_revision_cannot_strand_a_declared_live_pin() {
        let (publisher, mut collection) = two_revision_collection(true);
        let old_pin = collection.artifacts[0].pin;
        collection.artifacts.remove(0);
        assert_eq!(
            load(&publisher, &collection).expect_err("coverage must fail"),
            CatalogArtifactError::Collection(CatalogCollectionError::LivePinMissing(old_pin))
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
            Err(CatalogArtifactError::Collection(
                CatalogCollectionError::Verification {
                    source: aex_model_catalog::CatalogLoadError::BrokenChain { .. },
                    ..
                }
            ))
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
            Err(CatalogArtifactError::Collection(CatalogCollectionError::ContentAddressMismatch {
                expected: found_expected,
                actual: found_actual,
            })) if found_expected == expected && found_actual == actual
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
            CatalogArtifactError::Collection(CatalogCollectionError::CollectionTooLarge {
                seen: MAX_CATALOG_COLLECTION_BYTES + 1,
                limit: MAX_CATALOG_COLLECTION_BYTES,
            })
        );

        let (_, mut collection) = two_revision_collection(true);
        collection.artifacts = vec![collection.artifacts[0].clone(); MAX_CATALOG_REVISIONS + 1];
        assert_eq!(
            load(&publisher, &collection).expect_err("count must fail"),
            CatalogArtifactError::Collection(CatalogCollectionError::TooManyRevisions {
                seen: MAX_CATALOG_REVISIONS + 1,
                limit: MAX_CATALOG_REVISIONS,
            })
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
    fn signed_active_metadata_remains_service_capable_without_a_freshness_horizon() {
        let publisher = Publisher::new();
        let entry = active_entry("gpt-stable");
        let document = fixture::document(PUBLISHER, 1, vec![entry], now());
        let collection = CatalogCollection::genesis(publisher.artifact(&document).envelope);
        let bytes = collection.canonical_bytes().expect("canonical collection");
        let far_future = fixture::at(NOW_MS + 10 * 365 * 24 * 60 * 60 * 1000);
        let port = VerifiedCatalogPort::load_collection(&bytes, &publisher.keys, far_future)
            .expect("signed metadata does not expire");
        assert_eq!(port.active_models(), 1);
        assert!(port.is_service_capable());
    }

    #[test]
    fn a_noncanonical_collection_is_refused_before_it_can_be_build_bound() {
        let (publisher, collection) = two_revision_collection(true);
        let mut bytes = collection.canonical_bytes().expect("canonical collection");
        bytes.push(b'\n');
        assert_eq!(
            VerifiedCatalogPort::load_collection(&bytes, &publisher.keys, now())
                .expect_err("noncanonical bytes must fail"),
            CatalogArtifactError::Collection(CatalogCollectionError::NonCanonicalCollection)
        );
    }

    #[test]
    fn the_admission_pin_must_be_the_verified_newest_head() {
        let (publisher, mut collection) = two_revision_collection(true);
        let old = collection.artifacts[0].pin;
        let new = collection.artifacts[1].pin;
        collection.admission_pin = old;
        assert_eq!(
            load(&publisher, &collection).expect_err("head must fail"),
            CatalogArtifactError::Collection(CatalogCollectionError::AdmissionHeadMismatch {
                expected: old,
                actual: new,
            })
        );
    }

    #[test]
    fn live_pin_coverage_has_one_sorted_unique_release_representation() {
        let (publisher, mut collection) = two_revision_collection(true);
        let pin = collection.live_session_pins[0];
        collection.live_session_pins = vec![pin, pin];
        assert_eq!(
            load(&publisher, &collection).expect_err("coverage order must fail"),
            CatalogArtifactError::Collection(CatalogCollectionError::LivePinsUnsorted)
        );
    }

    #[test]
    fn a_duplicate_artifact_is_rejected_deterministically() {
        let (publisher, mut collection) = two_revision_collection(true);
        let duplicate = collection.artifacts[0].clone();
        collection.artifacts.insert(1, duplicate.clone());
        assert_eq!(
            load(&publisher, &collection).expect_err("duplicate must fail"),
            CatalogArtifactError::Collection(CatalogCollectionError::DuplicateArtifact(
                duplicate.pin
            ))
        );
    }
}
