//! The immutable usage fact.
//!
//! A fact is an immutable, trusted, integer measurement admitted at a definite
//! position in one `(region, workspace, category)` sequence. It is never
//! updated. Everything that would change it is another fact.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::correction::Correction;
use crate::frontier::AcceptedSequence;
use crate::identity::{AuthorityKey, FactId};
use crate::intent::{CanonicalError, IntentHash};
use crate::measurement::{Measurement, ServiceTime};
use crate::meter::{Category, Meter, ObservabilityMeter, PublicCategory, TokenClass, UnknownMeter};
use crate::wire_pending::{
    AgentId, CredentialBindingId, ModelId, OperationId, OrganizationId, PricingVersion, ProviderId,
    RegionId, ReservationId, RunId, ServiceId, SessionId, Timestamp, WorkspaceId,
};

/// The fact schema this crate reads and writes.
pub const SCHEMA_VERSION: SchemaVersion = SchemaVersion(1);

/// How far a producer's service time may run past the authority's own clock.
///
/// `usage.service_time_skew_max`. A service interval that ends beyond this is
/// refused: producer clocks may drift, but they may not date work into the
/// future far enough to land in a different settlement period.
pub const SERVICE_TIME_SKEW_MAX_MS: u64 = 5 * 60 * 1_000;

/// The fact schema version pinned on every row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SchemaVersion(u16);

impl SchemaVersion {
    /// The underlying version number.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

impl fmt::Display for SchemaVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// What kind of physical resource a fact's generation names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    /// A Brain mux task.
    MuxTask,
    /// A Hands MicroVM generation.
    HandsGeneration,
    /// A Lambda function version.
    LambdaFunction,
    /// A regional stream connection.
    StreamConnection,
    /// A content root revision.
    ContentRoot,
    /// An observation export operation.
    ObservationExport,
}

impl ResourceKind {
    /// Every resource kind, in a stable order.
    pub const ALL: [Self; 6] = [
        Self::MuxTask,
        Self::HandsGeneration,
        Self::LambdaFunction,
        Self::StreamConnection,
        Self::ContentRoot,
        Self::ObservationExport,
    ];

    /// The stable identifier written to a row.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::MuxTask => "mux_task",
            Self::HandsGeneration => "hands_generation",
            Self::LambdaFunction => "lambda_function",
            Self::StreamConnection => "stream_connection",
            Self::ContentRoot => "content_root",
            Self::ObservationExport => "observation_export",
        }
    }
}

impl fmt::Display for ResourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

impl FromStr for ResourceKind {
    type Err = UnknownMeter;

    fn from_str(value: &str) -> Result<Self, UnknownMeter> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.id() == value)
            .ok_or_else(|| UnknownMeter {
                kind: "resource kind",
                value: value.to_owned(),
            })
    }
}

/// The exact physical thing measured, pinned to an immutable generation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ResourceGeneration {
    /// What kind of resource it is.
    pub kind: ResourceKind,
    /// The Hands generation id, the mux task ARN, the Lambda function version
    /// or the content root revision — never a mutable name.
    pub generation: Box<str>,
}

/// Which customer work a fact is attributed to.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Attribution {
    /// The session, when the work belongs to one.
    pub session: Option<SessionId>,
    /// The agent, when the work belongs to one.
    pub agent: Option<AgentId>,
    /// The run, when the work belongs to one.
    pub run: Option<RunId>,
    /// The durable operation, when the work belongs to one.
    pub operation: Option<OperationId>,
}

/// The admission fence a deterministic identity carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Idempotency {
    /// The canonical authority key the identity was derived from.
    pub key: String,
    /// The hash of everything the producer said about the measurement.
    pub intent_hash: IntentHash,
}

/// A zero-dollar BYOK observability measurement.
///
/// It has no [`crate::quantity::Quantity`], no basis and no rate path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservabilityMeasurement {
    /// Which observability counter this is.
    pub meter: ObservabilityMeter,
    /// Counts by class. Ordered, so the canonical form is stable.
    pub counts: BTreeMap<TokenClass, u64>,
    /// The provider the customer's credential belongs to.
    pub provider: ProviderId,
    /// The model that was called.
    pub model: ModelId,
    /// The provider-credential binding that was used.
    pub credential: CredentialBindingId,
    /// When the observed work happened.
    pub service_time: ServiceTime,
}

/// What one fact records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FactKind {
    /// A priced measurement.
    Measured(Measurement),
    /// A zero-dollar observability count; compute authority only.
    Observability(ObservabilityMeasurement),
    /// A correction that restates an earlier fact's measurement.
    Replace {
        /// What is being corrected and why.
        correction: Correction,
        /// The restated measurement.
        replacement: Measurement,
    },
    /// A correction that withdraws an earlier fact entirely.
    Void {
        /// What is being withdrawn and why.
        correction: Correction,
    },
}

impl FactKind {
    /// The stable discriminator written to a row.
    #[must_use]
    pub const fn id(&self) -> &'static str {
        match self {
            Self::Measured(_) => "measured",
            Self::Observability(_) => "observability",
            Self::Replace { .. } => "replace",
            Self::Void { .. } => "void",
        }
    }

    /// The priced meter this fact contributes to, when it has one.
    #[must_use]
    pub const fn meter(&self) -> Option<Meter> {
        match self {
            Self::Measured(measurement)
            | Self::Replace {
                replacement: measurement,
                ..
            } => Some(measurement.meter()),
            Self::Observability(_) | Self::Void { .. } => None,
        }
    }

    /// The measurement this fact carries, when it carries one.
    #[must_use]
    pub const fn measurement(&self) -> Option<&Measurement> {
        match self {
            Self::Measured(measurement)
            | Self::Replace {
                replacement: measurement,
                ..
            } => Some(measurement),
            Self::Observability(_) | Self::Void { .. } => None,
        }
    }

    /// The correction this fact carries, when it is one.
    #[must_use]
    pub const fn correction(&self) -> Option<&Correction> {
        match self {
            Self::Replace { correction, .. } | Self::Void { correction } => Some(correction),
            Self::Measured(_) | Self::Observability(_) => None,
        }
    }

    /// When the recorded work happened.
    #[must_use]
    pub const fn service_time(&self) -> Option<ServiceTime> {
        match self {
            Self::Measured(measurement)
            | Self::Replace {
                replacement: measurement,
                ..
            } => Some(measurement.service_time()),
            Self::Observability(observed) => Some(observed.service_time),
            Self::Void { .. } => None,
        }
    }

    /// The public category this fact is reported under, when it has one.
    #[must_use]
    pub const fn public_category(&self) -> Option<PublicCategory> {
        match self.meter() {
            Some(meter) => Some(meter.public()),
            None => None,
        }
    }
}

/// Why a draft could not be admitted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AdmissionError {
    /// The draft's region disagrees with its authority key.
    #[error("draft region `{draft}` disagrees with authority key region `{authority}`")]
    RegionMismatch {
        /// The region on the draft.
        draft: String,
        /// The region inside the authority key.
        authority: String,
    },
    /// The draft's meter disagrees with the authority key's category.
    #[error("meter `{meter}` belongs to category `{meter_category}`, not `{authority}`")]
    CategoryMismatch {
        /// The meter that was offered.
        meter: &'static str,
        /// The category that meter belongs to.
        meter_category: &'static str,
        /// The category inside the authority key.
        authority: &'static str,
    },
    /// An observability fact was offered outside the compute authority.
    #[error("an observability fact belongs to the compute authority, not `{authority}`")]
    ObservabilityOutsideCompute {
        /// The category inside the authority key.
        authority: &'static str,
    },
    /// The service time ran too far past the authority's own clock.
    #[error(
        "service time ends {skew_ms} ms past admission, over the {SERVICE_TIME_SKEW_MAX_MS} ms ceiling"
    )]
    ServiceTimeSkew {
        /// How far past admission the service time ended.
        skew_ms: u64,
    },
    /// The pricing version was a moving reference rather than a pinned one.
    #[error("pricing version `{value}` is a moving reference; a fact pins an exact rate book")]
    UnpinnedPricingVersion {
        /// The value that was refused.
        value: String,
    },
    /// The draft could not be canonicalized for its intent hash.
    #[error(transparent)]
    Canonical(#[from] CanonicalError),
}

/// A measurement a producer offers, before the authority admits it.
///
/// A draft carries no `accepted_sequence` and no `accepted_at`: the authority
/// assigns both. A producer that could supply its own sequence could forge a
/// frontier position by replaying one message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactDraft {
    /// The fact schema version.
    pub schema_version: SchemaVersion,
    /// The organization that owes the money.
    pub organization: OrganizationId,
    /// The workspace the fact partitions under.
    pub workspace: WorkspaceId,
    /// The region the measurement was taken in.
    pub region: RegionId,
    /// Which customer work this is attributed to.
    pub attribution: Attribution,
    /// The deployable that produced the measurement.
    pub service: ServiceId,
    /// The exact physical thing measured.
    pub resource: ResourceGeneration,
    /// The deterministic identity of the measurement.
    pub authority: AuthorityKey,
    /// The rate book pinned at admission.
    pub pricing_version: PricingVersion,
    /// The escrow reservation this draws down, when there is one.
    pub reservation: Option<ReservationId>,
    /// What is being recorded.
    pub kind: FactKind,
}

impl FactDraft {
    /// The deterministic identifier this draft will occupy.
    #[must_use]
    pub fn fact_id(&self) -> FactId {
        self.authority.fact_id()
    }

    /// The hash of everything the producer said about this measurement.
    ///
    /// # Errors
    ///
    /// Returns [`CanonicalError`] when the draft cannot be canonicalized.
    pub fn intent_hash(&self) -> Result<IntentHash, CanonicalError> {
        IntentHash::of(self)
    }

    /// Admits this draft at an authority-assigned position.
    ///
    /// # Errors
    ///
    /// Returns [`AdmissionError`] when the draft's region or category disagrees
    /// with its authority key, when an observability fact is offered outside
    /// the compute authority, when the service time runs past
    /// [`SERVICE_TIME_SKEW_MAX_MS`], or when the pricing version is not pinned.
    pub fn admit(
        self,
        accepted_sequence: AcceptedSequence,
        accepted_at: Timestamp,
    ) -> Result<UsageFact, AdmissionError> {
        if self.region != self.authority.region {
            return Err(AdmissionError::RegionMismatch {
                draft: self.region.to_string(),
                authority: self.authority.region.to_string(),
            });
        }
        if self.pricing_version.as_str() == "current" {
            return Err(AdmissionError::UnpinnedPricingVersion {
                value: self.pricing_version.to_string(),
            });
        }
        if matches!(self.kind, FactKind::Observability(_)) {
            if self.authority.category != Category::Compute {
                return Err(AdmissionError::ObservabilityOutsideCompute {
                    authority: self.authority.category.id(),
                });
            }
        } else if let Some(meter) = self.kind.meter()
            && meter.category() != self.authority.category
        {
            return Err(AdmissionError::CategoryMismatch {
                meter: meter.id(),
                meter_category: meter.category().id(),
                authority: self.authority.category.id(),
            });
        }
        if let Some(service_time) = self.kind.service_time()
            && let Some(skew_ms) = accepted_at.millis_until(service_time.end())
            && skew_ms > SERVICE_TIME_SKEW_MAX_MS
        {
            return Err(AdmissionError::ServiceTimeSkew { skew_ms });
        }
        let idempotency = Idempotency {
            key: self.authority.canonical(),
            intent_hash: self.intent_hash()?,
        };
        Ok(UsageFact {
            fact_id: self.authority.fact_id(),
            schema_version: self.schema_version,
            organization: self.organization,
            workspace: self.workspace,
            region: self.region,
            attribution: self.attribution,
            service: self.service,
            resource: self.resource,
            authority: self.authority,
            accepted_sequence,
            accepted_at,
            pricing_version: self.pricing_version,
            reservation: self.reservation,
            idempotency,
            kind: self.kind,
        })
    }
}

/// One admitted, immutable usage fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageFact {
    /// The deterministic identity derived from the authority key.
    pub fact_id: FactId,
    /// The fact schema version.
    pub schema_version: SchemaVersion,
    /// The organization that owes the money.
    pub organization: OrganizationId,
    /// The workspace the fact partitions under.
    pub workspace: WorkspaceId,
    /// The region the measurement was taken in.
    pub region: RegionId,
    /// Which customer work this is attributed to.
    pub attribution: Attribution,
    /// The deployable that produced the measurement.
    pub service: ServiceId,
    /// The exact physical thing measured.
    pub resource: ResourceGeneration,
    /// The deterministic identity of the measurement.
    pub authority: AuthorityKey,
    /// The position the authority assigned. Never supplied by a producer.
    pub accepted_sequence: AcceptedSequence,
    /// When the authority admitted the fact, on the authority's clock.
    pub accepted_at: Timestamp,
    /// The rate book pinned at admission.
    pub pricing_version: PricingVersion,
    /// The escrow reservation this draws down, when there is one.
    pub reservation: Option<ReservationId>,
    /// The admission fence the deterministic identity carries.
    pub idempotency: Idempotency,
    /// What was recorded.
    pub kind: FactKind,
}

impl UsageFact {
    /// The authority table this fact belongs to.
    #[must_use]
    pub const fn category(&self) -> Category {
        self.authority.category
    }

    /// The public category this fact is reported under, when it has one.
    #[must_use]
    pub const fn public_category(&self) -> Option<PublicCategory> {
        self.kind.public_category()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AdmissionError, Attribution, FactDraft, FactKind, ResourceGeneration, ResourceKind,
        SCHEMA_VERSION, SERVICE_TIME_SKEW_MAX_MS,
    };
    use crate::frontier::AcceptedSequence;
    use crate::identity::{AuthorityId, AuthorityKey, AuthorityKind, SegmentOrdinal};
    use crate::measurement::{
        BoundaryId, Evidence, FactBasis, Measurement, ReceiptKind, ServiceTime, SourceReceipt,
    };
    use crate::meter::{Category, Meter};
    use crate::wire_pending::{
        OrganizationId, PricingVersion, RegionId, ServiceId, Timestamp, WorkspaceId,
    };

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("representable")
    }

    fn egress(bytes: u64, when: i64) -> Measurement {
        Measurement::new(
            Meter::DataTransferEgressByte,
            FactBasis::Consumed,
            ServiceTime::Instant { at: at(when) },
            SourceReceipt {
                kind: ReceiptKind::DeliveryLog,
                id: Box::from("d1"),
                digest: None,
            },
            Evidence::DeliveryReceipt {
                boundary: BoundaryId::CONTENT_DOWNLOAD,
                receipt_id: Box::from("d1"),
                bytes,
            },
        )
        .expect("valid egress measurement")
    }

    fn draft(category: Category, kind: FactKind) -> FactDraft {
        FactDraft {
            schema_version: SCHEMA_VERSION,
            organization: OrganizationId::parse("org-1").expect("org"),
            workspace: WorkspaceId::parse("ws-1").expect("workspace"),
            region: RegionId::parse("eu-west-1").expect("region"),
            attribution: Attribution::default(),
            service: ServiceId::parse("regional-session-api").expect("service"),
            resource: ResourceGeneration {
                kind: ResourceKind::LambdaFunction,
                generation: Box::from("7"),
            },
            authority: AuthorityKey {
                region: RegionId::parse("eu-west-1").expect("region"),
                category,
                kind: AuthorityKind::EgressCrossing,
                authority_id: AuthorityId::parse("cross-1").expect("id"),
                segment_ordinal: SegmentOrdinal::FIRST,
            },
            pricing_version: PricingVersion::parse("synthetic-zero-v1").expect("version"),
            reservation: None,
            kind,
        }
    }

    #[test]
    fn admission_assigns_identity_and_the_intent_fence() {
        let draft = draft(Category::Transfer, FactKind::Measured(egress(1_024, 0)));
        let expected_id = draft.fact_id();
        let fact = draft
            .admit(AcceptedSequence::new(1).expect("sequence"), at(1_000))
            .expect("admits");
        assert_eq!(fact.fact_id, expected_id);
        assert_eq!(
            fact.idempotency.key,
            "eu-west-1/transfer/egress_crossing/cross-1/0"
        );
        assert_eq!(
            fact.accepted_sequence,
            AcceptedSequence::new(1).expect("sequence")
        );
    }

    #[test]
    fn a_meter_may_not_be_admitted_into_a_foreign_authority() {
        let error = draft(Category::Compute, FactKind::Measured(egress(1_024, 0)))
            .admit(AcceptedSequence::new(1).expect("sequence"), at(1_000))
            .expect_err("a transfer meter is not a compute fact");
        assert!(matches!(error, AdmissionError::CategoryMismatch { .. }));
    }

    #[test]
    fn service_time_beyond_the_skew_ceiling_is_refused() {
        let future = i64::try_from(SERVICE_TIME_SKEW_MAX_MS).expect("fits") + 1;
        let error = draft(Category::Transfer, FactKind::Measured(egress(1_024, future)))
            .admit(AcceptedSequence::new(1).expect("sequence"), at(0))
            .expect_err("future-dated service time is refused");
        assert!(matches!(
            error,
            AdmissionError::ServiceTimeSkew { skew_ms }
                if skew_ms == u64::try_from(future).expect("fits")
        ));
    }

    #[test]
    fn a_moving_pricing_version_is_refused() {
        let mut candidate = draft(Category::Transfer, FactKind::Measured(egress(1, 0)));
        candidate.pricing_version = PricingVersion::parse("current").expect("parses as an id");
        let error = candidate
            .admit(AcceptedSequence::new(1).expect("sequence"), at(0))
            .expect_err("a fact pins an exact rate book");
        assert!(matches!(
            error,
            AdmissionError::UnpinnedPricingVersion { .. }
        ));
    }

    #[test]
    fn the_intent_hash_is_stable_across_equal_drafts_and_moves_with_the_quantity() {
        let first = draft(Category::Transfer, FactKind::Measured(egress(1_024, 0)));
        let same = draft(Category::Transfer, FactKind::Measured(egress(1_024, 0)));
        let different = draft(Category::Transfer, FactKind::Measured(egress(2_048, 0)));
        assert_eq!(
            first.intent_hash().expect("hash"),
            same.intent_hash().expect("hash")
        );
        assert_eq!(first.fact_id(), different.fact_id());
        assert_ne!(
            first.intent_hash().expect("hash"),
            different.intent_hash().expect("hash"),
            "one identity with a different quantity must be an identity conflict"
        );
    }
}
