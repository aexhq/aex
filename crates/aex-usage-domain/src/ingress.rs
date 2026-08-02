//! Strict producer-to-authority ingress envelope.
//!
//! An ingress record carries an untrusted [`FactDraft`], not an admitted fact.
//! The explicit category is redundant on purpose: the producer queue, envelope,
//! draft authority key, and linked worker must all agree before authority state
//! can change.

use serde::{Deserialize, Serialize};

use crate::fact::FactDraft;
use crate::meter::Category;

/// The one accepted ingress payload version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum EnvelopeKind {
    #[serde(rename = "usage_fact_draft.v1")]
    UsageFactDraftV1,
}

/// One canonical usage draft enqueued for authority-owned admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FactDraftEnvelope {
    #[serde(rename = "type")]
    kind: EnvelopeKind,
    /// Category named by the producer queue.
    pub category: Category,
    /// Untrusted producer draft.
    pub draft: FactDraft,
}

impl FactDraftEnvelope {
    /// Builds an envelope only when its redundant category fence agrees.
    ///
    /// # Errors
    ///
    /// Returns [`EnvelopeError::CategoryMismatch`] for a draft addressed to a
    /// different authority.
    pub fn new(category: Category, draft: FactDraft) -> Result<Self, EnvelopeError> {
        if category != draft.authority.category {
            return Err(EnvelopeError::CategoryMismatch {
                envelope: category,
                draft: draft.authority.category,
            });
        }
        Ok(Self {
            kind: EnvelopeKind::UsageFactDraftV1,
            category,
            draft,
        })
    }

    /// Removes the envelope after checking the linked worker's category.
    ///
    /// # Errors
    ///
    /// Returns [`EnvelopeError::WorkerMismatch`] when the message escaped from
    /// a sibling category queue, or [`EnvelopeError::CategoryMismatch`] when its
    /// redundant fields disagree.
    pub fn into_draft(self, worker: Category) -> Result<FactDraft, EnvelopeError> {
        if self.category != worker {
            return Err(EnvelopeError::WorkerMismatch {
                envelope: self.category,
                worker,
            });
        }
        if self.draft.authority.category != self.category {
            return Err(EnvelopeError::CategoryMismatch {
                envelope: self.category,
                draft: self.draft.authority.category,
            });
        }
        Ok(self.draft)
    }
}

/// Why an ingress category fence refused a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EnvelopeError {
    /// Envelope and draft disagree.
    #[error("envelope category {envelope} disagrees with draft category {draft}")]
    CategoryMismatch {
        /// Category on the envelope.
        envelope: Category,
        /// Category in the authority key.
        draft: Category,
    },
    /// Envelope arrived at a sibling worker.
    #[error("{envelope} envelope reached the {worker} worker")]
    WorkerMismatch {
        /// Category on the envelope.
        envelope: Category,
        /// Category linked into the worker.
        worker: Category,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fact::{Attribution, FactKind, ResourceGeneration, ResourceKind, SCHEMA_VERSION};
    use crate::identity::{AuthorityId, AuthorityKey, AuthorityKind, SegmentOrdinal};
    use crate::measurement::{
        BoundaryId, Evidence, FactBasis, Measurement, ReceiptKind, ServiceTime, SourceReceipt,
    };
    use crate::meter::Meter;
    use crate::wire_pending::{
        OrganizationId, PricingVersion, RegionId, ServiceId, Timestamp, WorkspaceId,
    };

    fn draft() -> FactDraft {
        let at = Timestamp::from_unix_millis(0).expect("time");
        FactDraft {
            schema_version: SCHEMA_VERSION,
            organization: OrganizationId::parse("org-1").expect("org"),
            workspace: WorkspaceId::parse("ws-1").expect("workspace"),
            region: RegionId::parse("eu-west-1").expect("region"),
            attribution: Attribution::default(),
            service: ServiceId::parse("runtime-control-worker").expect("service"),
            resource: ResourceGeneration {
                kind: ResourceKind::HandsGeneration,
                generation: "gen-1".into(),
            },
            authority: AuthorityKey {
                region: RegionId::parse("eu-west-1").expect("region"),
                category: Category::Transfer,
                kind: AuthorityKind::EgressCrossing,
                authority_id: AuthorityId::parse("cross-1").expect("authority"),
                segment_ordinal: SegmentOrdinal::FIRST,
            },
            pricing_version: PricingVersion::parse("2026-08-01").expect("pricing"),
            reservation: None,
            kind: FactKind::Measured(
                Measurement::new(
                    Meter::DataTransferEgressByte,
                    FactBasis::Consumed,
                    ServiceTime::Instant { at },
                    SourceReceipt {
                        kind: ReceiptKind::DeliveryLog,
                        id: "r-1".into(),
                        digest: None,
                    },
                    Evidence::DeliveryReceipt {
                        boundary: BoundaryId::HANDS_EGRESS,
                        receipt_id: "r-1".into(),
                        bytes: 1,
                    },
                )
                .expect("measurement"),
            ),
        }
    }

    #[test]
    fn strict_round_trip_keeps_all_category_fences() {
        let envelope = FactDraftEnvelope::new(Category::Transfer, draft()).expect("envelope");
        let json = serde_json::to_string(&envelope).expect("json");
        assert!(json.contains("usage_fact_draft.v1"));
        let decoded: FactDraftEnvelope = serde_json::from_str(&json).expect("decode");
        assert_eq!(
            decoded.into_draft(Category::Transfer).expect("draft"),
            draft()
        );
    }

    #[test]
    fn sibling_worker_and_unknown_fields_fail_closed() {
        let envelope = FactDraftEnvelope::new(Category::Transfer, draft()).expect("envelope");
        assert!(matches!(
            envelope.into_draft(Category::Compute),
            Err(EnvelopeError::WorkerMismatch { .. })
        ));
        let mut value = serde_json::to_value(
            FactDraftEnvelope::new(Category::Transfer, draft()).expect("envelope"),
        )
        .expect("value");
        value
            .as_object_mut()
            .expect("object")
            .insert("extra".into(), true.into());
        assert!(serde_json::from_value::<FactDraftEnvelope>(value).is_err());
    }
}
