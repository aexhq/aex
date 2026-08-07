//! Loading, activation and qualification (plan 08 §2.4–§2.6).
//!
//! [`Catalog::load`] is total and fail-closed. Two of its checks are the reason
//! this crate exists at all:
//!
//! - the **receipt gate** is a document load invariant (D-05), so an `Active`
//!   entry whose declared capabilities are not all covered by passing probes
//!   cannot be loaded — it is impossible to forget the runtime check because
//!   there is no runtime check.
//! - the **adapter binding** (D-06) refuses the whole document when the running
//!   adapter source tree is not the one the receipts were earned against, so
//!   editing an adapter and shipping the old catalog fails at process start
//!   rather than at the first customer request.

use std::sync::Arc;

use aex_wire::provider::{ModelSelection, ProviderId};
use aex_wire::to_jcs_bytes;
use aex_wire::types::{JsonPointer, Timestamp};

use crate::document::{
    AdapterSourceDigest, CatalogDigest, CatalogDocument, CatalogSequence, DisableReason,
    DurableOperationSupport, EntryState, ModelEntry, PreDispatchRetryPolicy, PublisherId,
    SCHEMA_VERSION,
};
use crate::primitives::{Blake3Digest, BoundedString, ModelSlug};
use crate::qualified::{CatalogError, QualifiedModel};
use crate::receipt::ProbeId;
use crate::signature::{CatalogEnvelope, SignatureError, SigningKeyId, TrustedKeys, verify};
use crate::wire_pending::CatalogRevision;

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
    /// `now` is at or after `retired_at`.
    #[error("the document retired at {retired_at:?}")]
    Retired {
        /// When it retired.
        retired_at: Timestamp,
    },
    /// The three time gates are not ordered.
    #[error("the time gates are not ordered: not_before <= expires_at <= retired_at")]
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
    /// The receipts were earned against a different adapter source tree.
    #[error("the running adapter {running:?} is not the required {required:?}")]
    AdapterMismatch {
        /// What the document requires.
        required: AdapterSourceDigest,
        /// What is actually running.
        running: AdapterSourceDigest,
    },
    /// An `Active` entry whose receipt does not pass a required probe.
    #[error("`{provider}` / `{model}` is Active without a passing {probe:?}")]
    ActiveWithoutReceipt {
        /// The provider half.
        provider: ProviderId,
        /// The model half.
        model: ModelSlug,
        /// The probe that did not pass.
        probe: ProbeId,
    },
    /// An `Active` entry declares a capability no probe proved.
    #[error("`{provider}` / `{model}` declares `{capability}` with no proving probe")]
    CapabilityWithoutProbe {
        /// The provider half.
        provider: ProviderId,
        /// The model half.
        model: ModelSlug,
        /// The capability with no evidence.
        capability: BoundedString<64>,
    },
    /// A capability bit outside this binary's vocabulary.
    #[error("`{provider}` / `{model}` declares a capability bit this binary does not know")]
    UnknownCapabilityBit {
        /// The provider half.
        provider: ProviderId,
        /// The model half.
        model: ModelSlug,
    },
    /// A receipt is incomplete: it does not carry a result for every probe.
    #[error("`{provider}` / `{model}` carries no result for {probe:?}")]
    IncompleteReceipt {
        /// The provider half.
        provider: ProviderId,
        /// The model half.
        model: ModelSlug,
        /// The missing probe.
        probe: ProbeId,
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
        adapter: AdapterSourceDigest,
    ) -> Result<Self, CatalogLoadError> {
        let signed_by = verify(envelope, keys)?;

        let document = parse_canonical(&envelope.document)?;
        if document.schema_version != SCHEMA_VERSION {
            return Err(CatalogLoadError::SchemaVersion {
                found: document.schema_version,
            });
        }

        let digest = CatalogDigest(Blake3Digest::of(&envelope.document));

        check_time_gates(&document, now)?;
        check_activation(&document, digest, active)?;
        check_ordering(&document)?;

        if document.required_adapter_source != adapter {
            return Err(CatalogLoadError::AdapterMismatch {
                required: document.required_adapter_source,
                running: adapter,
            });
        }
        check_receipt_gate(&document, adapter)?;

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
    /// Deliberately **weaker** than [`Catalog::admit`]: it ignores expiry and
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
    /// model, non-`Active` state, emergency disable, stale receipt, expired
    /// revision or retired revision. Every arm fails before any reservation.
    pub fn admit(
        &self,
        selection: &ModelSelection,
        now: Timestamp,
    ) -> Result<QualifiedModel, CatalogError> {
        let candidate = self.qualified(selection)?;

        if now >= self.document.retired_at {
            return Err(CatalogError::CatalogRetired {
                retired_at: self.document.retired_at,
            });
        }
        if now >= self.document.expires_at {
            return Err(CatalogError::CatalogExpired {
                expires_at: self.document.expires_at,
            });
        }
        if let Some(reason) = self.disabled(candidate.provider(), candidate.model()) {
            return Err(CatalogError::EmergencyDisabled { reason });
        }
        if candidate.state() != EntryState::Active {
            return Err(CatalogError::UnqualifiedPair {
                state: candidate.state(),
            });
        }
        let expires_at = candidate.entry().receipt.expires_at;
        if now >= expires_at {
            return Err(CatalogError::ReceiptExpired { expires_at });
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

    /// How many entries are admissible in principle. A launch catalog answers
    /// zero (OD-24).
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
    if document.not_before > document.expires_at || document.expires_at > document.retired_at {
        return Err(CatalogLoadError::TimeGatesUnordered);
    }
    if now < document.not_before {
        return Err(CatalogLoadError::NotYetValid {
            not_before: document.not_before,
        });
    }
    if now >= document.retired_at {
        return Err(CatalogLoadError::Retired {
            retired_at: document.retired_at,
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

/// The receipt gate as a document load invariant (D-05).
fn check_receipt_gate(
    document: &CatalogDocument,
    adapter: AdapterSourceDigest,
) -> Result<(), CatalogLoadError> {
    for entry in &document.entries {
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
        for probe in ProbeId::ALL {
            if entry.receipt.result(probe).is_none() {
                return Err(CatalogLoadError::IncompleteReceipt {
                    provider: entry.provider,
                    model: entry.model.clone(),
                    probe,
                });
            }
        }
        if entry.state != EntryState::Active {
            continue;
        }
        if entry.receipt.adapter_source != adapter {
            return Err(CatalogLoadError::AdapterMismatch {
                required: entry.receipt.adapter_source,
                running: adapter,
            });
        }
        if entry.receipt.probe_suite_revision != document.probe_suite_revision {
            return Err(CatalogLoadError::ActiveWithoutReceipt {
                provider: entry.provider,
                model: entry.model.clone(),
                probe: ProbeId::P01,
            });
        }
        for capability in entry.capabilities.declared() {
            let Some(probe) = ProbeId::proving(capability) else {
                continue;
            };
            let passes = entry
                .receipt
                .result(probe)
                .is_some_and(|result| result.outcome.is_pass());
            if !passes {
                return Err(CatalogLoadError::CapabilityWithoutProbe {
                    provider: entry.provider,
                    model: entry.model.clone(),
                    capability: BoundedString::truncating(capability.as_str()),
                });
            }
        }
        for probe in ProbeId::ALL {
            if !probe.is_required_for(entry.capabilities) {
                continue;
            }
            let passes = entry
                .receipt
                .result(probe)
                .is_some_and(|result| result.outcome.is_pass());
            if !passes {
                return Err(CatalogLoadError::ActiveWithoutReceipt {
                    provider: entry.provider,
                    model: entry.model.clone(),
                    probe,
                });
            }
        }
    }
    Ok(())
}
