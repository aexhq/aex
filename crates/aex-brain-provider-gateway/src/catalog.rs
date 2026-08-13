//! Loading, activation and qualification (plan 08 §2.4–§2.6).
//!
//! [`Catalog::load`] is total and fail-closed. Signed catalog metadata is the
//! admission authority: schema, dialect, endpoint and bounded retry-policy
//! checks all run before an `Active` entry can be selected. Live qualification
//! evidence is external assurance data and is not a runtime availability gate.

use std::sync::Arc;

use aex_wire::provider::{ModelSelection, ProviderId};
use aex_wire::to_jcs_bytes;
use aex_wire::types::{JsonPointer, Timestamp};

use aex_model_catalog::CatalogRevision;
use aex_model_catalog::document::{
    CatalogDigest, CatalogDocument, CatalogSequence, Dialect, DialectRevision, DisableReason,
    DurableOperationSupport, EndpointPin, EntryState, ModelEntry, PreDispatchRetryPolicy,
    PublisherId, SCHEMA_VERSION,
};
use aex_model_catalog::primitives::{Blake3Digest, BoundedString, ModelSlug};
use aex_model_catalog::qualified::{CatalogError, QualifiedModel};

use crate::signature::{CatalogEnvelope, SignatureError, SigningKeyId, TrustedKeys, verify};

/// Which revision is currently serving, for the downgrade and chain rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogHead {
    /// Who published it.
    pub publisher: PublisherId,
    /// Its sequence.
    pub sequence: CatalogSequence,
    /// Its digest.
    pub digest: CatalogDigest,
}

/// A verified, loaded catalog revision.
#[derive(Debug, Clone)]
pub struct Catalog {
    document: CatalogDocument,
    entries: Vec<Arc<ModelEntry>>,
    digest: CatalogDigest,
    revision: CatalogRevision,
    signed_by: SigningKeyId,
}

/// Why a catalog envelope could not be loaded.
///
/// Every arm is fatal for the whole document: there is no partial load, so a
/// catalog can never be half-trusted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogLoadError {
    /// The bytes parse as JSON but are not their own canonical rendering.
    #[error("the document bytes are not canonical JSON at `{at:?}`")]
    NotCanonical {
        /// Where the two renderings first differ.
        at: JsonPointer,
    },
    /// The document carries a field this binary does not know.
    #[error("unknown field at `{at:?}`")]
    UnknownField {
        /// Where.
        at: JsonPointer,
    },
    /// The document carries an enum member this binary does not know.
    #[error("unknown enum member `{value}` at `{at:?}`")]
    UnknownEnumMember {
        /// Where.
        at: JsonPointer,
        /// The unknown token.
        value: BoundedString<64>,
    },
    /// The document is not schema version 1.
    #[error("schema version {found} is not the supported version")]
    SchemaVersion {
        /// What the document declared.
        found: u16,
    },
    /// The bytes are not JSON at all.
    #[error("the document is not JSON: {reason}")]
    Malformed {
        /// The parser's own message, bounded.
        reason: BoundedString<256>,
    },
    /// Signature verification failed.
    #[error("signature verification failed: {0}")]
    Signature(#[from] SignatureError),
    /// The offered sequence does not advance the active one.
    #[error("sequence {offered:?} does not advance {active:?}")]
    Downgrade {
        /// The sequence currently serving.
        active: CatalogSequence,
        /// The sequence offered.
        offered: CatalogSequence,
    },
    /// The publisher changed. Never automatic.
    #[error("publisher changed from {active:?} to {offered:?}")]
    PublisherChanged {
        /// The publisher currently serving.
        active: PublisherId,
        /// The publisher offered.
        offered: PublisherId,
    },
    /// The predecessor link does not point at the active revision.
    #[error("predecessor {found:?} is not the active digest {expected:?}")]
    BrokenChain {
        /// What the chain required.
        expected: CatalogDigest,
        /// What the document declared.
        found: Option<CatalogDigest>,
    },
    /// `now` is before `not_before`.
    #[error("the document is not valid before {not_before:?}")]
    NotYetValid {
        /// When it becomes valid.
        not_before: Timestamp,
    },
    /// The signed issuance and activation instants are not ordered.
    #[error("the time gates are not ordered: issued_at <= not_before")]
    TimeGatesUnordered,
    /// `entries` is not sorted by `(provider, model)`.
    #[error("entries are unsorted at index {at}")]
    EntriesUnsorted {
        /// The first index out of order.
        at: usize,
    },
    /// Two entries name the same pair.
    #[error("duplicate entry for `{provider}` / `{model}`")]
    DuplicateEntry {
        /// The provider half.
        provider: ProviderId,
        /// The model half.
        model: ModelSlug,
    },
    /// `emergency_disable` is unsorted or duplicated.
    #[error("the emergency-disable list is unsorted or duplicated at index {at}")]
    DisableListUnsorted {
        /// The first index out of order.
        at: usize,
    },
    /// An entry names a reserved dialect this binary does not implement.
    #[error("`{provider}` / `{model}` names unimplemented dialect {dialect:?}")]
    DialectNotImplemented {
        /// The provider half.
        provider: ProviderId,
        /// The model half.
        model: ModelSlug,
        /// The unsupported dialect.
        dialect: Dialect,
    },
    /// The dialect belongs to a different provider authority.
    #[error("`{provider}` / `{model}` names dialect {dialect:?} for another provider")]
    DialectProviderMismatch {
        /// The provider half.
        provider: ProviderId,
        /// The model half.
        model: ModelSlug,
        /// The mismatched dialect.
        dialect: Dialect,
    },
    /// The endpoint belongs to a different provider authority.
    #[error("`{provider}` / `{model}` names endpoint {endpoint:?} for another provider")]
    EndpointProviderMismatch {
        /// The provider half.
        provider: ProviderId,
        /// The model half.
        model: ModelSlug,
        /// The mismatched endpoint.
        endpoint: EndpointPin,
    },
    /// The entry names a dialect revision this binary does not implement.
    #[error("`{provider}` / `{model}` names unsupported dialect revision {found:?}")]
    DialectRevisionUnsupported {
        /// The provider half.
        provider: ProviderId,
        /// The model half.
        model: ModelSlug,
        /// The unsupported revision.
        found: DialectRevision,
    },
    /// A capability bit outside this binary's vocabulary.
    #[error("`{provider}` / `{model}` declares a capability bit this binary does not know")]
    UnknownCapabilityBit {
        /// The provider half.
        provider: ProviderId,
        /// The model half.
        model: ModelSlug,
    },
    /// An entry declares an in-call retry for a status D-20 does not permit.
    ///
    /// Only 429 and 503 are definitive non-generation rejections; retrying
    /// anything else risks a second generation, so a document that asks for it
    /// is refused rather than obeyed.
    #[error("`{provider}` / `{model}` permits an in-call retry for status {status}")]
    RetryStatusForbidden {
        /// The provider half.
        provider: ProviderId,
        /// The model half.
        model: ModelSlug,
        /// The status the entry asked for.
        status: u16,
    },
    /// An entry declares more in-call attempts than D-20 permits.
    #[error("`{provider}` / `{model}` declares {attempts} attempts; 1..=3 is permitted")]
    RetryAttemptsOutOfRange {
        /// The provider half.
        provider: ProviderId,
        /// The model half.
        model: ModelSlug,
        /// What the entry asked for.
        attempts: u16,
    },
}

impl Catalog {
    /// Validates canonical document bytes before a protected signer is invoked.
    ///
    /// This is a preflight, not catalog authority: it verifies no signature and
    /// cannot construct a [`Catalog`] or [`QualifiedModel`]. The signed envelope
    /// must still pass [`Catalog::load`] before publication or use.
    ///
    /// # Errors
    ///
    /// Returns the same document, chain, time, dialect, endpoint and policy
    /// failures as [`Catalog::load`], excluding signature failures.
    pub fn preflight_document(
        document: &[u8],
        now: Timestamp,
        active: Option<&CatalogHead>,
    ) -> Result<CatalogHead, CatalogLoadError> {
        let (document, digest) = validate_document(document, now, active)?;
        Ok(CatalogHead {
            publisher: document.publisher,
            sequence: document.sequence,
            digest,
        })
    }

    /// Verifies, parses and admits a catalog envelope.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogLoadError`] for any of the failures in its enumeration.
    /// There is no partial success.
    pub fn load(
        envelope: &CatalogEnvelope,
        keys: &TrustedKeys,
        now: Timestamp,
        active: Option<&CatalogHead>,
    ) -> Result<Self, CatalogLoadError> {
        let signed_by = verify(envelope, keys)?;
        let (document, digest) = validate_document(&envelope.document, now, active)?;

        let entries = document.entries.iter().cloned().map(Arc::new).collect();
        Ok(Self {
            revision: CatalogRevision(digest.0),
            document,
            entries,
            digest,
            signed_by,
        })
    }

    /// The blake3 digest of the exact published bytes.
    #[must_use]
    pub fn digest(&self) -> CatalogDigest {
        self.digest
    }

    /// The public opaque revision.
    #[must_use]
    pub fn revision(&self) -> CatalogRevision {
        self.revision
    }

    /// Which compiled key's signature admitted this revision.
    #[must_use]
    pub fn signed_by(&self) -> &SigningKeyId {
        &self.signed_by
    }

    /// The activation head this revision presents.
    #[must_use]
    pub fn head(&self) -> CatalogHead {
        CatalogHead {
            publisher: self.document.publisher.clone(),
            sequence: self.document.sequence,
            digest: self.digest,
        }
    }

    /// The underlying document.
    #[must_use]
    pub fn document(&self) -> &CatalogDocument {
        &self.document
    }

    /// Binary search over the sorted entry slice. No allocation.
    #[must_use]
    pub fn entry(&self, provider: ProviderId, model: &ModelSlug) -> Option<&ModelEntry> {
        self.index_of(provider, model)
            .map(|index| self.entries[index].as_ref())
    }

    fn index_of(&self, provider: ProviderId, model: &ModelSlug) -> Option<usize> {
        self.entries
            .binary_search_by(|entry| {
                (entry.provider, entry.model.as_str()).cmp(&(provider, model.as_str()))
            })
            .ok()
    }

    /// Resolves a pair that a committed history already used.
    ///
    /// Deliberately **weaker** than [`Catalog::admit`]: it ignores state and
    /// emergency disable so a journal written under this revision stays
    /// readable (D-08, MC-6).
    ///
    /// # Errors
    ///
    /// Returns [`CatalogError::UnknownProvider`] or
    /// [`CatalogError::UnknownModel`] when no entry exists.
    pub fn qualified(&self, selection: &ModelSelection) -> Result<QualifiedModel, CatalogError> {
        let model =
            ModelSlug::new(selection.model.clone()).map_err(|_| CatalogError::UnknownModel {
                provider: selection.provider,
                model: ModelSlug::truncating(&selection.model),
            })?;
        let Some(index) = self.index_of(selection.provider, &model) else {
            return Err(self.absent(selection.provider, model));
        };
        Ok(QualifiedModel::new(
            Arc::clone(&self.entries[index]),
            self.revision,
        ))
    }

    /// Resolves a pair for a **new** run: every gate applies.
    ///
    /// # Errors
    ///
    /// Returns the typed [`CatalogError`] for an unknown provider, unknown
    /// model, non-`Active` state or emergency disable. Every arm fails before
    /// any reservation.
    pub fn admit(&self, selection: &ModelSelection) -> Result<QualifiedModel, CatalogError> {
        let candidate = self.qualified(selection)?;

        if let Some(reason) = self.disabled(candidate.provider(), candidate.model()) {
            return Err(CatalogError::EmergencyDisabled { reason });
        }
        if candidate.state() != EntryState::Active {
            return Err(CatalogError::UnqualifiedPair {
                state: candidate.state(),
            });
        }
        Ok(candidate)
    }

    /// Whether the pair is emergency-disabled in this revision.
    #[must_use]
    pub fn disabled(&self, provider: ProviderId, model: &ModelSlug) -> Option<DisableReason> {
        self.document
            .emergency_disable
            .binary_search_by(|row| {
                (row.provider, row.model.as_str()).cmp(&(provider, model.as_str()))
            })
            .ok()
            .map(|index| self.document.emergency_disable[index].reason)
    }

    /// Whether the provider offers a durable result lookup for the pair.
    #[must_use]
    pub fn durable_operation_support(
        &self,
        provider: ProviderId,
        model: &ModelSlug,
    ) -> DurableOperationSupport {
        self.entry(provider, model)
            .map_or(DurableOperationSupport::None, |entry| {
                entry.durable_operation
            })
    }

    /// How many entries the revision carries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the revision carries no entries at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many entries carry signed `Active` state.
    #[must_use]
    pub fn active_len(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.state == EntryState::Active)
            .count()
    }

    fn absent(&self, provider: ProviderId, model: ModelSlug) -> CatalogError {
        if self.entries.iter().any(|entry| entry.provider == provider) {
            CatalogError::UnknownModel { provider, model }
        } else {
            CatalogError::UnknownProvider { provider }
        }
    }
}

fn validate_document(
    bytes: &[u8],
    now: Timestamp,
    active: Option<&CatalogHead>,
) -> Result<(CatalogDocument, CatalogDigest), CatalogLoadError> {
    let document = parse_canonical(bytes)?;
    if document.schema_version != SCHEMA_VERSION {
        return Err(CatalogLoadError::SchemaVersion {
            found: document.schema_version,
        });
    }
    let digest = CatalogDigest(Blake3Digest::of(bytes));
    check_time_gates(&document, now)?;
    check_activation(&document, digest, active)?;
    check_ordering(&document)?;
    check_entry_policy(&document)?;
    Ok((document, digest))
}

/// Parses the document bytes and proves they are their own canonical rendering.
fn parse_canonical(bytes: &[u8]) -> Result<CatalogDocument, CatalogLoadError> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|error| CatalogLoadError::Malformed {
            reason: BoundedString::truncating(&error.to_string()),
        })?;
    let canonical = to_jcs_bytes(&value).map_err(|error| CatalogLoadError::Malformed {
        reason: BoundedString::truncating(&error.to_string()),
    })?;
    if canonical != bytes {
        return Err(CatalogLoadError::NotCanonical {
            at: first_divergence(&canonical, bytes),
        });
    }
    serde_json::from_slice(bytes).map_err(|error| classify_decode_error(&error))
}

/// Names the offending position of a non-canonical byte string.
///
/// A byte offset is the only honest answer: the two renderings are the same
/// *value*, so no JSON member "is" the difference. The pointer carries the
/// offset as a single token rather than pretending to a path.
fn first_divergence(canonical: &[u8], offered: &[u8]) -> JsonPointer {
    let at = canonical
        .iter()
        .zip(offered.iter())
        .position(|(left, right)| left != right)
        .unwrap_or_else(|| canonical.len().min(offered.len()));
    JsonPointer::root().child(&format!("byte:{at}"))
}

/// Turns a `serde_json` failure into the exact typed arm plus its position.
fn classify_decode_error(error: &serde_json::Error) -> CatalogLoadError {
    let message = error.to_string();
    let at = JsonPointer::root().child(&format!("line:{}", error.line()));
    if message.starts_with("unknown field ") {
        return CatalogLoadError::UnknownField { at };
    }
    if let Some(start) = message.find("unknown variant `") {
        let tail = &message[start + "unknown variant `".len()..];
        let value = tail.split('`').next().unwrap_or_default();
        return CatalogLoadError::UnknownEnumMember {
            at,
            value: BoundedString::truncating(value),
        };
    }
    CatalogLoadError::Malformed {
        reason: BoundedString::truncating(&message),
    }
}

fn check_time_gates(document: &CatalogDocument, now: Timestamp) -> Result<(), CatalogLoadError> {
    if document.issued_at > document.not_before {
        return Err(CatalogLoadError::TimeGatesUnordered);
    }
    if now < document.not_before {
        return Err(CatalogLoadError::NotYetValid {
            not_before: document.not_before,
        });
    }
    Ok(())
}

fn check_activation(
    document: &CatalogDocument,
    digest: CatalogDigest,
    active: Option<&CatalogHead>,
) -> Result<(), CatalogLoadError> {
    let Some(active) = active else {
        return Ok(());
    };
    if active.publisher != document.publisher {
        return Err(CatalogLoadError::PublisherChanged {
            active: active.publisher.clone(),
            offered: document.publisher.clone(),
        });
    }
    if digest == active.digest && document.sequence == active.sequence {
        // Re-offering the exact active revision is idempotent, not a downgrade.
        return Ok(());
    }
    if document.sequence <= active.sequence {
        return Err(CatalogLoadError::Downgrade {
            active: active.sequence,
            offered: document.sequence,
        });
    }
    if document.predecessor != Some(active.digest) {
        return Err(CatalogLoadError::BrokenChain {
            expected: active.digest,
            found: document.predecessor,
        });
    }
    Ok(())
}

fn check_ordering(document: &CatalogDocument) -> Result<(), CatalogLoadError> {
    for index in 1..document.entries.len() {
        let previous = &document.entries[index - 1];
        let current = &document.entries[index];
        let left = (previous.provider, previous.model.as_str());
        let right = (current.provider, current.model.as_str());
        if left == right {
            return Err(CatalogLoadError::DuplicateEntry {
                provider: current.provider,
                model: current.model.clone(),
            });
        }
        if left > right {
            return Err(CatalogLoadError::EntriesUnsorted { at: index });
        }
    }
    for index in 1..document.emergency_disable.len() {
        let previous = &document.emergency_disable[index - 1];
        let current = &document.emergency_disable[index];
        if (previous.provider, previous.model.as_str())
            >= (current.provider, current.model.as_str())
        {
            return Err(CatalogLoadError::DisableListUnsorted { at: index });
        }
    }
    Ok(())
}

/// Validates the signed compatibility metadata understood by this binary.
fn check_entry_policy(document: &CatalogDocument) -> Result<(), CatalogLoadError> {
    for entry in &document.entries {
        if !entry.dialect.is_implemented() {
            return Err(CatalogLoadError::DialectNotImplemented {
                provider: entry.provider,
                model: entry.model.clone(),
                dialect: entry.dialect,
            });
        }
        if entry.dialect.provider() != entry.provider {
            return Err(CatalogLoadError::DialectProviderMismatch {
                provider: entry.provider,
                model: entry.model.clone(),
                dialect: entry.dialect,
            });
        }
        if entry.endpoint.provider() != entry.provider {
            return Err(CatalogLoadError::EndpointProviderMismatch {
                provider: entry.provider,
                model: entry.model.clone(),
                endpoint: entry.endpoint,
            });
        }
        if entry.dialect_revision != entry.dialect.revision() {
            return Err(CatalogLoadError::DialectRevisionUnsupported {
                provider: entry.provider,
                model: entry.model.clone(),
                found: entry.dialect_revision,
            });
        }
        if entry.capabilities.has_unknown_bits() {
            return Err(CatalogLoadError::UnknownCapabilityBit {
                provider: entry.provider,
                model: entry.model.clone(),
            });
        }
        if entry.retry_policy.max_attempts == 0 || entry.retry_policy.max_attempts > 3 {
            return Err(CatalogLoadError::RetryAttemptsOutOfRange {
                provider: entry.provider,
                model: entry.model.clone(),
                attempts: entry.retry_policy.max_attempts,
            });
        }
        if let Some(status) = entry
            .retry_policy
            .in_call_status_retry
            .iter()
            .copied()
            .find(|status| !PreDispatchRetryPolicy::PERMITTED_STATUSES.contains(status))
        {
            return Err(CatalogLoadError::RetryStatusForbidden {
                provider: entry.provider,
                model: entry.model.clone(),
                status,
            });
        }
    }
    Ok(())
}

/// Hashes the exact canonical compatibility metadata an external qualification
/// receipt proves.
///
/// # Errors
///
/// Returns [`CatalogLoadError::Malformed`] if the closed entry schema cannot
/// be rendered into canonical JSON.
pub fn catalog_entry_digest(entry: &ModelEntry) -> Result<aex_wire::ContentHash, CatalogLoadError> {
    let bytes = to_jcs_bytes(entry).map_err(|error| CatalogLoadError::Malformed {
        reason: BoundedString::truncating(&error.to_string()),
    })?;
    Ok(aex_wire::ContentHash::of(&bytes))
}
