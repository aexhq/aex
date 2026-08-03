//! Brain `CatalogPort` over verified immutable model-catalog artifacts.

use std::collections::BTreeMap;
use std::sync::Arc;

use aex_brain_application::ports::{CatalogError, CatalogPort};
use aex_brain_domain::ids::{CatalogPin, ModelSlug};
use aex_model_catalog::document::{CatalogDigest, DurableOperationSupport, EntryState};
use aex_model_catalog::signature::{CatalogEnvelope, TrustedKeys};
use aex_model_catalog::{Catalog, CatalogHead, CatalogLoadError, QualifiedModel};
use aex_wire::provider::{ModelSelection, ProviderId};

/// Maximum serialized envelope accepted at startup.
pub const MAX_CATALOG_ENVELOPE_BYTES: usize = 32 * 1024 * 1024;

/// Why a catalog artifact could not enter the verified cache.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogArtifactError {
    /// The binary carries no valid immutable adapter source-tree identity.
    #[error(transparent)]
    BuildIdentity(#[from] crate::build_identity::AdapterBuildIdentityError),
    /// The envelope exceeded the startup allocation bound.
    #[error("model catalog envelope is {seen} bytes, above the {limit}-byte startup bound")]
    TooLarge {
        /// Bytes offered.
        seen: usize,
        /// Configured hard bound.
        limit: usize,
    },
    /// The envelope JSON was malformed or had an unknown field.
    #[error("model catalog envelope is malformed")]
    MalformedEnvelope,
    /// Signature, canonical-byte, chain, adapter or conformance verification failed.
    #[error(transparent)]
    Verification(#[from] CatalogLoadError),
    /// The content-addressed name did not match the exact verified document bytes.
    #[error("model catalog artifact names {expected}, but its verified bytes are {actual}")]
    ContentAddressMismatch {
        /// Revision named by configuration or artifact placement.
        expected: CatalogPin,
        /// Revision derived from exact canonical document bytes.
        actual: CatalogPin,
    },
}

/// An in-memory cache containing only fully verified immutable revisions.
#[derive(Debug, Default)]
pub struct VerifiedCatalogPort {
    catalogs: BTreeMap<CatalogPin, Arc<Catalog>>,
}

impl VerifiedCatalogPort {
    /// Loads one content-addressed signed envelope into a fresh cache.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogArtifactError`] before publishing any revision when
    /// size, envelope decoding, signature, canonical bytes, adapter binding,
    /// conformance evidence, build identity or content address does not verify.
    pub fn load(
        bytes: &[u8],
        expected: CatalogPin,
        keys: &TrustedKeys,
        now: aex_wire::types::Timestamp,
        active: Option<&CatalogHead>,
    ) -> Result<Self, CatalogArtifactError> {
        if bytes.len() > MAX_CATALOG_ENVELOPE_BYTES {
            return Err(CatalogArtifactError::TooLarge {
                seen: bytes.len(),
                limit: MAX_CATALOG_ENVELOPE_BYTES,
            });
        }
        let envelope: CatalogEnvelope =
            serde_json::from_slice(bytes).map_err(|_| CatalogArtifactError::MalformedEnvelope)?;
        let adapter = crate::build_identity::adapter_source_digest()?;
        let catalog = Catalog::load(&envelope, keys, now, active, adapter)?;
        let actual = catalog.revision();
        if actual != expected {
            return Err(CatalogArtifactError::ContentAddressMismatch { expected, actual });
        }
        let mut catalogs = BTreeMap::new();
        catalogs.insert(actual, Arc::new(catalog));
        Ok(Self { catalogs })
    }

    /// Adds another verified revision without replacing an existing one.
    pub fn insert_verified(&mut self, catalog: Catalog) {
        self.catalogs
            .entry(catalog.revision())
            .or_insert_with(|| Arc::new(catalog));
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
