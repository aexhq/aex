//! Release-bound collections of signed model-catalog revisions.
//!
//! This contract lives beside [`crate::Catalog`], rather than in a runtime
//! adapter, so the protected publisher and every consumer execute the same
//! bounded chain, signature, content-address and retention checks.

use std::collections::BTreeMap;
use std::sync::Arc;

use aex_wire::provider::ModelSelection;
use aex_wire::to_jcs_bytes;
use aex_wire::types::Timestamp;
use serde::{Deserialize, Serialize};

use crate::CatalogRevision;
use crate::catalog::{Catalog, CatalogHead, CatalogLoadError};
use crate::signature::{CatalogEnvelope, TrustedKeys};

/// Closed schema discriminator for the release/session-retention contract.
pub const CATALOG_COLLECTION_SCHEMA: &str = "aex.model-catalog-collection.v1";

/// Maximum serialized release collection accepted by a consumer.
pub const MAX_CATALOG_COLLECTION_BYTES: usize = 64 * 1024 * 1024;

/// Maximum serialized envelope accepted inside a collection.
pub const MAX_CATALOG_ENVELOPE_BYTES: usize = 32 * 1024 * 1024;

/// Maximum revisions one release may retain.
pub const MAX_CATALOG_REVISIONS: usize = 32;

/// One exact content-addressed signed artifact in release chain order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogArtifact {
    /// Blake3 revision derived from the exact canonical document bytes.
    pub pin: CatalogRevision,
    /// Exact document bytes and detached publisher signatures.
    pub envelope: CatalogEnvelope,
}

impl CatalogArtifact {
    /// Binds an envelope to the digest of the exact document bytes it carries.
    #[must_use]
    pub fn from_envelope(envelope: CatalogEnvelope) -> Self {
        let pin = CatalogRevision(crate::Blake3Digest::of(&envelope.document));
        Self { pin, envelope }
    }
}

/// The release/session-retention contract compiled into one runtime build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogCollection {
    /// Must equal [`CATALOG_COLLECTION_SCHEMA`].
    pub schema: String,
    /// Exact head used for new admission by this release.
    pub admission_pin: CatalogRevision,
    /// Sorted, unique pins required by sessions that may still wake.
    pub live_session_pins: Vec<CatalogRevision>,
    /// Oldest-to-newest contiguous signed artifacts.
    pub artifacts: Vec<CatalogArtifact>,
}

impl CatalogCollection {
    /// Constructs the first release collection around one signed revision.
    #[must_use]
    pub fn genesis(envelope: CatalogEnvelope) -> Self {
        let artifact = CatalogArtifact::from_envelope(envelope);
        Self {
            schema: CATALOG_COLLECTION_SCHEMA.to_owned(),
            admission_pin: artifact.pin,
            live_session_pins: Vec::new(),
            artifacts: vec![artifact],
        }
    }

    /// Emits the one canonical representation accepted by publication.
    ///
    /// # Errors
    ///
    /// Returns a serialization failure only if the closed schema cannot be
    /// represented as JSON.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, CatalogCollectionError> {
        to_jcs_bytes(self).map_err(|_| CatalogCollectionError::MalformedCollection)
    }
}

/// Why a release collection could not become lookup authority.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogCollectionError {
    /// The aggregate collection exceeded the startup allocation bound.
    #[error("model catalog collection is {seen} bytes, above the {limit}-byte startup bound")]
    CollectionTooLarge {
        /// Bytes offered.
        seen: usize,
        /// Maximum accepted bytes.
        limit: usize,
    },
    /// The JSON collection could not be decoded under the closed schema.
    #[error("model catalog collection is malformed")]
    MalformedCollection,
    /// The bytes are valid JSON but not the unique canonical representation.
    #[error("model catalog collection is not exact canonical JSON")]
    NonCanonicalCollection,
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
        /// Maximum retained revisions.
        limit: usize,
    },
    /// One envelope exceeded its individual allocation/work bound.
    #[error("model catalog envelope for {pin:?} is {seen} bytes, above the {limit}-byte bound")]
    EnvelopeTooLarge {
        /// Artifact being checked.
        pin: CatalogRevision,
        /// Serialized bytes offered.
        seen: usize,
        /// Maximum accepted bytes.
        limit: usize,
    },
    /// Signature, canonical-byte, chain or compatibility verification failed.
    #[error("model catalog artifact {pin:?} failed verification: {source}")]
    Verification {
        /// Artifact being checked.
        pin: CatalogRevision,
        /// Exact catalog verification failure.
        source: CatalogLoadError,
    },
    /// The content-addressed name did not match the verified document bytes.
    #[error("model catalog artifact names {expected:?}, but its verified bytes are {actual:?}")]
    ContentAddressMismatch {
        /// Address declared by the collection.
        expected: CatalogRevision,
        /// Address derived from document bytes.
        actual: CatalogRevision,
    },
    /// The same immutable pin appeared twice.
    #[error("model catalog artifact {0:?} appears more than once")]
    DuplicateArtifact(CatalogRevision),
    /// Coverage pins must have one canonical sorted representation.
    #[error("live session catalog pins are not strictly sorted and unique")]
    LivePinsUnsorted,
    /// The release retention contract names a pin absent from the cache.
    #[error("still-live sessions require catalog pin {0:?}, but the release omitted it")]
    LivePinMissing(CatalogRevision),
    /// New admission must use the verified newest chain head.
    #[error("admission pin {expected:?} is not the verified chain head {actual:?}")]
    AdmissionHeadMismatch {
        /// Address declared for new admission.
        expected: CatalogRevision,
        /// Newest verified chain head.
        actual: CatalogRevision,
    },
}

/// A fully verified immutable release collection.
#[derive(Debug)]
pub struct VerifiedCatalogCollection {
    catalogs: BTreeMap<CatalogRevision, Arc<Catalog>>,
    admission_pin: CatalogRevision,
    serviceable_models: usize,
}

impl VerifiedCatalogCollection {
    /// Verifies exact canonical collection bytes into an immutable map.
    ///
    /// # Errors
    ///
    /// Refuses invalid bounds, schema, signatures, content addresses, chains,
    /// catalog compatibility metadata, or retained-pin coverage.
    pub fn load(
        bytes: &[u8],
        keys: &TrustedKeys,
        now: Timestamp,
    ) -> Result<Self, CatalogCollectionError> {
        if bytes.len() > MAX_CATALOG_COLLECTION_BYTES {
            return Err(CatalogCollectionError::CollectionTooLarge {
                seen: bytes.len(),
                limit: MAX_CATALOG_COLLECTION_BYTES,
            });
        }
        let collection: CatalogCollection = serde_json::from_slice(bytes)
            .map_err(|_| CatalogCollectionError::MalformedCollection)?;
        if collection.canonical_bytes()?.as_slice() != bytes {
            return Err(CatalogCollectionError::NonCanonicalCollection);
        }
        Self::verify(collection, keys, now)
    }

    fn verify(
        collection: CatalogCollection,
        keys: &TrustedKeys,
        now: Timestamp,
    ) -> Result<Self, CatalogCollectionError> {
        if collection.schema != CATALOG_COLLECTION_SCHEMA {
            return Err(CatalogCollectionError::UnsupportedSchema(collection.schema));
        }
        if collection.artifacts.is_empty() {
            return Err(CatalogCollectionError::EmptyCollection);
        }
        if collection.artifacts.len() > MAX_CATALOG_REVISIONS {
            return Err(CatalogCollectionError::TooManyRevisions {
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
            return Err(CatalogCollectionError::LivePinsUnsorted);
        }

        let mut catalogs = BTreeMap::new();
        let mut head: Option<CatalogHead> = None;
        for artifact in collection.artifacts {
            let envelope_size = to_jcs_bytes(&artifact.envelope)
                .map_err(|_| CatalogCollectionError::MalformedCollection)?
                .len();
            if envelope_size > MAX_CATALOG_ENVELOPE_BYTES {
                return Err(CatalogCollectionError::EnvelopeTooLarge {
                    pin: artifact.pin,
                    seen: envelope_size,
                    limit: MAX_CATALOG_ENVELOPE_BYTES,
                });
            }
            let catalog =
                Catalog::load(&artifact.envelope, keys, now, head.as_ref()).map_err(|source| {
                    CatalogCollectionError::Verification {
                        pin: artifact.pin,
                        source,
                    }
                })?;
            let actual = catalog.revision();
            if actual != artifact.pin {
                return Err(CatalogCollectionError::ContentAddressMismatch {
                    expected: artifact.pin,
                    actual,
                });
            }
            head = Some(catalog.head());
            if catalogs.insert(actual, Arc::new(catalog)).is_some() {
                return Err(CatalogCollectionError::DuplicateArtifact(actual));
            }
        }

        let actual_head = CatalogRevision(
            head.expect("a non-empty collection produced a head")
                .digest
                .0,
        );
        if collection.admission_pin != actual_head {
            return Err(CatalogCollectionError::AdmissionHeadMismatch {
                expected: collection.admission_pin,
                actual: actual_head,
            });
        }
        for pin in collection.live_session_pins {
            if !catalogs.contains_key(&pin) {
                return Err(CatalogCollectionError::LivePinMissing(pin));
            }
        }
        let admission = catalogs
            .get(&collection.admission_pin)
            .expect("verified admission pin exists");
        let serviceable_models = admission
            .document()
            .entries
            .iter()
            .filter(|entry| {
                admission
                    .admit(&ModelSelection {
                        credential_id: None,
                        provider: entry.provider,
                        model: entry.model.as_str().to_owned(),
                    })
                    .is_ok()
            })
            .count();
        Ok(Self {
            catalogs,
            admission_pin: collection.admission_pin,
            serviceable_models,
        })
    }

    /// Exact head used for new session admission.
    #[must_use]
    pub const fn admission_pin(&self) -> CatalogRevision {
        self.admission_pin
    }

    /// Verified newest head, including publisher and sequence.
    ///
    /// # Panics
    ///
    /// Panics only if the collection's verified admission pin is absent from
    /// its retained catalogs. Construction establishes that invariant, and
    /// the collection exposes no operation that can remove a retained catalog.
    #[must_use]
    pub fn head(&self) -> CatalogHead {
        self.catalogs
            .get(&self.admission_pin)
            .expect("verified admission pin exists")
            .head()
    }

    /// Every immutable revision retained in this release.
    pub fn retained_pins(&self) -> impl Iterator<Item = CatalogRevision> + '_ {
        self.catalogs.keys().copied()
    }

    /// How many verified revisions the collection holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.catalogs.len()
    }

    /// Whether no revision is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.catalogs.is_empty()
    }

    /// How many active, non-disabled entries the collection can route.
    #[must_use]
    pub fn active_models(&self) -> usize {
        self.serviceable_models
    }

    /// Whether cryptographic validity also leaves a serviceable model.
    #[must_use]
    pub fn is_service_capable(&self) -> bool {
        self.active_models() > 0
    }

    /// Looks up one exact verified revision.
    #[must_use]
    pub fn catalog(&self, pin: CatalogRevision) -> Option<&Catalog> {
        self.catalogs.get(&pin).map(AsRef::as_ref)
    }
}
