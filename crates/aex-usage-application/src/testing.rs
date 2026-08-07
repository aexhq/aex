//! Deterministic fact builders shared by this crate's own tests.
//!
//! Every value is scripted. A pipeline test that sampled a real clock or a real
//! identifier would be asserting the test machine rather than the accounting
//! rule, and a fact whose identity moved between two runs would make the replay
//! properties vacuous.

use aex_usage_domain::correction::{Correction, CorrectionReason};
use aex_usage_domain::fact::{
    Attribution, FactDraft, FactKind, ObservabilityMeasurement, ResourceGeneration, ResourceKind,
    SCHEMA_VERSION, UsageFact,
};
use aex_usage_domain::frontier::{AcceptedSequence, Frontier};
use aex_usage_domain::identity::{
    AuthorityId, AuthorityKey, AuthorityKind, FactId, SegmentOrdinal,
};
use aex_usage_domain::interval::StorageClose;
use aex_usage_domain::interval::storage::{StorageOwner, StorageOwnerKind, StorageSource};
use aex_usage_domain::measurement::{
    BoundaryId, Evidence, FactBasis, Measurement, ReceiptKind, ReservationClass, ServiceTime,
    SourceReceipt,
};
use aex_usage_domain::meter::{Category, Meter, ObservabilityMeter, TokenClass};
use aex_usage_domain::wire_pending::{
    ActorRef, CaseId, CredentialBindingId, ModelId, OrganizationId, PricingVersion, ProviderId,
    RegionId, ServiceId, Timestamp, WorkspaceId,
};
use aex_wire::ids::PrefixedId as _;
use std::collections::BTreeMap;

/// A canonical instant from whole milliseconds.
pub fn at(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(millis).expect("representable")
}

/// The fixture organization, in the canonical `aex_wire` spelling.
///
/// Minted from the wire owner rather than hand-typed: the outbox boundary
/// parses these strings into the central contract's identifier types, and a
/// hand-written spelling would keep passing after the format changed.
pub fn organization() -> OrganizationId {
    let wire = aex_wire::ids::OrganizationId::from_uuid7(aex_wire::ids::Uuid7::compose(1, [7; 10]));
    OrganizationId::parse(wire.encode().as_str()).expect("org")
}

/// The fixture workspace, in the canonical `aex_wire` spelling.
pub fn workspace() -> WorkspaceId {
    let wire = aex_wire::ids::WorkspaceId::from_uuid7(aex_wire::ids::Uuid7::compose(2, [9; 10]));
    WorkspaceId::parse(wire.encode().as_str()).expect("workspace")
}

/// The fixture region.
pub fn region() -> RegionId {
    RegionId::parse("eu-west-1").expect("region")
}

/// The fixture rate book. Synthetic and zero, per `OD-09`.
pub fn pricing() -> PricingVersion {
    PricingVersion::parse("synthetic-zero-v1").expect("version")
}

/// One measurement per authority, each with the evidence shape that authority
/// actually carries.
pub fn measurement_for(category: Category, magnitude: u64) -> Measurement {
    match category {
        Category::Storage => Measurement::new(
            Meter::StorageByteMin,
            FactBasis::Consumed,
            ServiceTime::Interval {
                start: at(0),
                end: at(180_000),
            },
            SourceReceipt {
                kind: ReceiptKind::StorageCommit,
                id: Box::from("commit-1"),
                digest: None,
            },
            Evidence::StorageResidence {
                owner: StorageOwner {
                    kind: StorageOwnerKind::ContentObject,
                    id: AuthorityId::parse("obj-1").expect("id"),
                    generation: 1,
                },
                source: StorageSource::S3,
                bytes: magnitude,
                minutes: 3,
                commit_id: Box::from("commit-1"),
                close: StorageClose::InteriorFloor,
            },
        ),
        Category::Compute => Measurement::new(
            Meter::MemoryByteMs,
            FactBasis::Reserved,
            ServiceTime::Interval {
                start: at(0),
                end: at(250),
            },
            SourceReceipt {
                kind: ReceiptKind::ReservationToken,
                id: Box::from("res-1"),
                digest: None,
            },
            Evidence::Reservation {
                class: ReservationClass::Context,
                bytes: magnitude,
                held_ms: 250,
            },
        ),
        Category::Transfer => Measurement::new(
            Meter::DataTransferEgressByte,
            FactBasis::Consumed,
            ServiceTime::Instant { at: at(0) },
            SourceReceipt {
                kind: ReceiptKind::DeliveryLog,
                id: Box::from("d1"),
                digest: None,
            },
            Evidence::DeliveryReceipt {
                boundary: BoundaryId::CONTENT_DOWNLOAD,
                receipt_id: Box::from("d1"),
                bytes: magnitude,
            },
        ),
    }
    .expect("a fixture measurement is valid for its own meter")
}

/// The authority kind each category's fixture facts use.
pub fn authority_kind(category: Category) -> AuthorityKind {
    match category {
        Category::Storage => AuthorityKind::StorageResidence,
        Category::Compute => AuthorityKind::MemoryReservation,
        Category::Transfer => AuthorityKind::EgressCrossing,
    }
}

/// A draft with a distinct authority identity per `authority_id`.
pub fn draft(category: Category, authority_id: &str, kind: FactKind) -> FactDraft {
    FactDraft {
        schema_version: SCHEMA_VERSION,
        organization: organization(),
        workspace: workspace(),
        region: region(),
        attribution: Attribution::default(),
        service: ServiceId::parse("regional-stream").expect("service"),
        resource: ResourceGeneration {
            kind: ResourceKind::MuxTask,
            generation: Box::from("task-1"),
        },
        authority: AuthorityKey {
            region: region(),
            category,
            kind: authority_kind(category),
            authority_id: AuthorityId::parse(authority_id).expect("id"),
            segment_ordinal: SegmentOrdinal::FIRST,
        },
        pricing_version: pricing(),
        reservation: None,
        kind,
    }
}

/// An admitted measured fact at `sequence`, with a magnitude keyed to it so two
/// fixture facts never coincidentally cancel.
pub fn fact(category: Category, sequence: u64) -> UsageFact {
    admit(
        draft(
            category,
            &format!("a-{sequence}"),
            FactKind::Measured(measurement_for(category, 1_000 + sequence)),
        ),
        sequence,
    )
}

/// An admitted zero-dollar observability fact. Compute authority only.
pub fn observability_fact(sequence: u64) -> UsageFact {
    admit(
        draft(
            Category::Compute,
            &format!("obs-{sequence}"),
            FactKind::Observability(ObservabilityMeasurement {
                meter: ObservabilityMeter::ModelTokens,
                counts: BTreeMap::from([(TokenClass::Input, 1_200), (TokenClass::Output, 340)]),
                provider: ProviderId::parse("deepseek").expect("provider"),
                model: ModelId::parse("deepseek-chat").expect("model"),
                credential: CredentialBindingId::parse("pcr-1").expect("binding"),
                service_time: ServiceTime::Interval {
                    start: at(0),
                    end: at(900),
                },
            }),
        ),
        sequence,
    )
}

/// An admitted void withdrawing `target`.
pub fn void_fact(category: Category, sequence: u64, target: &FactId) -> UsageFact {
    admit(
        draft(
            category,
            &format!("void-{sequence}"),
            FactKind::Void {
                correction: correction(target),
            },
        ),
        sequence,
    )
}

/// An admitted replace restating `target` with `magnitude`.
pub fn replace_fact(
    category: Category,
    sequence: u64,
    target: &FactId,
    magnitude: u64,
) -> UsageFact {
    admit(
        draft(
            category,
            &format!("replace-{sequence}"),
            FactKind::Replace {
                correction: correction(target),
                replacement: measurement_for(category, magnitude),
            },
        ),
        sequence,
    )
}

/// The fixture correction envelope.
pub fn correction(target: &FactId) -> Correction {
    Correction {
        case_id: CaseId::parse("case-1").expect("case"),
        target: target.clone(),
        prior_head: None,
        reason: CorrectionReason::WrongQuantity,
        actor: ActorRef::Reconciler {
            service: ServiceId::parse("provider-cost-reconciler").expect("service"),
        },
    }
}

/// Admits a draft at `sequence`, on the authority's own clock.
pub fn admit(draft: FactDraft, sequence: u64) -> UsageFact {
    draft
        .admit(
            AcceptedSequence::new(sequence).expect("a sequence is one based"),
            at(600_000 + i64::try_from(sequence).unwrap_or(0)),
        )
        .expect("a fixture draft is admissible")
}

/// A frontier that has already projected `projected` facts of `accepted`.
pub fn frontier_at(category: Category, accepted: u64, projected: u64) -> Frontier {
    let mut frontier = Frontier::empty(region(), workspace(), category);
    for step in 1..=accepted {
        frontier = frontier
            .admit(AcceptedSequence::new(step).expect("positive"))
            .expect("contiguous admission");
    }
    for step in 1..=projected {
        frontier = frontier
            .project(AcceptedSequence::new(step).expect("positive"))
            .expect("contiguous projection");
    }
    frontier
}

/// In-memory doubles for the four usage ports.
pub mod doubles;

pub use doubles::{FakeAuthority, FakeProjection, FakeQueue, FrozenClock};
