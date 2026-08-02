//! Hands usage derivation and authority ingress.
//!
//! Runtime control is a producer, never a usage authority. It emits canonical
//! [`FactDraft`] values; the category worker assigns the accepted sequence and
//! timestamp in its own transaction. Provider lifecycle evidence is the only
//! input to priced Hands measurements.

use core::future::Future;
use core::pin::Pin;

use aex_hands_protocol::lifecycle::RuntimeReceipt;
use aex_internal_contracts::PricingVersion as RuntimePricingVersion;
use aex_usage_domain::fact::{
    Attribution, FactDraft, FactKind, ResourceGeneration, ResourceKind, SCHEMA_VERSION,
};
use aex_usage_domain::identity::{AuthorityId, AuthorityKey, AuthorityKind, SegmentOrdinal};
use aex_usage_domain::interval::storage::{
    StorageClose, StorageOwner, StorageOwnerKind, StorageSource,
};
use aex_usage_domain::measurement::{
    BoundaryId, Evidence, FactBasis, Measurement, ReceiptKind, ServiceTime, SourceReceipt,
};
use aex_usage_domain::meter::{Category, Meter};
use aex_usage_domain::shape::{ComputeShape, ShapeUnit};
use aex_usage_domain::wire_pending::{
    OrganizationId, PricingVersion, RegionId, ServiceId, SessionId, Timestamp as UsageTimestamp,
    WorkspaceId,
};
use aex_wire::ids::{
    OrganizationId as WireOrganizationId, SessionId as WireSessionId,
    WorkspaceId as WireWorkspaceId,
};
use aex_wire::types::{ComputeSize, Region, Timestamp};
use serde::{Deserialize, Serialize};

use crate::clock::millis_between;
use crate::lifecycle::LifecycleIntentId;
use crate::shape::ShapeCapacity as _;

/// Which authority ingress a fact belongs to.
pub type UsageCategory = Category;

/// The producer identity carried by every Hands draft.
pub const HANDS_SOURCE: &str = "runtime-control-worker";

/// Which authority ingress owns a meter.
#[must_use]
pub const fn category_of(meter: Meter) -> UsageCategory {
    meter.category()
}

/// Snapshot read/write bytes, recorded but never priced.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SnapshotIo {
    /// Bytes written into the snapshot on suspend.
    pub write_bytes: u64,
    /// Bytes read back from the snapshot on resume.
    pub read_bytes: u64,
}

/// Retained snapshot bytes over one suspension interval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SnapshotResidence {
    /// AEX-minted lifecycle identity for this retained snapshot.
    pub lifecycle_id: String,
    /// Monotone snapshot generation within the `MicroVM` generation.
    pub generation: u64,
    /// Retained provider-declared bytes.
    pub bytes: u64,
    /// Start of retention, inclusive.
    pub suspended_at: Timestamp,
    /// End of retention, exclusive.
    pub released_at: Timestamp,
    /// Whether this close permanently releases the snapshot.
    pub terminal: bool,
    /// Zero-dollar snapshot I/O observability.
    pub io: SnapshotIo,
}

/// One measured Hands quantity before it is wrapped in a canonical draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandsUsage {
    /// Which priced meter.
    pub meter: Meter,
    /// Which authority ingress owns it.
    pub category: UsageCategory,
    /// Exact quantity in the meter's base unit.
    pub quantity: u128,
    /// When the service happened.
    pub service_time: ServiceTime,
}

/// Everything one closed interval needs to become canonical drafts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactContext {
    /// Account owing the money.
    pub organization: WireOrganizationId,
    /// Workspace holding the usage sequence.
    pub workspace: WireWorkspaceId,
    /// Region where the provider work occurred.
    pub region: Region,
    /// Session owning the generation.
    pub session: WireSessionId,
    /// Pinned pricing book.
    pub pricing_version: RuntimePricingVersion,
    /// Lifecycle intent that closed the interval.
    pub intent: LifecycleIntentId,
    /// Provider receipt identity.
    pub source_receipt_id: String,
}

/// Why usage or a canonical draft could not be derived.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DerivationError {
    /// The provider receipt's interval accounting is not exact.
    #[error("the runtime receipt is not exact: {0}")]
    Receipt(#[from] aex_hands_protocol::lifecycle::ReceiptError),
    /// Snapshot residence runs backwards.
    #[error("the snapshot residence ends before it starts")]
    BackwardsSnapshot,
    /// Snapshot residence lies outside the provider receipt interval.
    #[error("the snapshot residence is not contained in the receipt interval")]
    SnapshotOutsideInterval,
    /// Canonical usage identifiers or evidence rejected the producer input.
    #[error("canonical usage draft is invalid: {0}")]
    Canonical(String),
}

fn usage_time(value: Timestamp) -> Result<UsageTimestamp, DerivationError> {
    UsageTimestamp::from_unix_millis(value.unix_millis())
        .map_err(|error| DerivationError::Canonical(error.to_string()))
}

fn service_time(start: Timestamp, end: Timestamp) -> Result<ServiceTime, DerivationError> {
    Ok(ServiceTime::Interval {
        start: usage_time(start)?,
        end: usage_time(end)?,
    })
}

fn shape(size: ComputeSize) -> ComputeShape {
    ComputeShape::new(size.baseline_millicpu(), size.baseline_memory_bytes())
}

fn snapshot_minutes(snapshot: &SnapshotResidence) -> u64 {
    let elapsed = millis_between(snapshot.suspended_at, snapshot.released_at);
    if snapshot.terminal {
        elapsed.div_ceil(60_000)
    } else {
        elapsed / 60_000
    }
}

/// Every priced quantity one closed interval produces, in stable order.
///
/// Compute and memory are provider-allocated capacity over provider-authoritative
/// running milliseconds. Storage floors interior closes and ceils terminal
/// closes, so no sub-minute tail is lost when a snapshot is permanently removed.
///
/// # Errors
///
/// Returns [`DerivationError`] when the provider receipt is inexact or a snapshot
/// residence is backwards or outside the receipt interval.
pub fn derive_usage(
    receipt: &RuntimeReceipt,
    residence: Option<&SnapshotResidence>,
) -> Result<Vec<HandsUsage>, DerivationError> {
    receipt.validate()?;
    if let Some(snapshot) = residence {
        if snapshot.released_at < snapshot.suspended_at {
            return Err(DerivationError::BackwardsSnapshot);
        }
        if snapshot.suspended_at < receipt.from || snapshot.released_at > receipt.to {
            return Err(DerivationError::SnapshotOutsideInterval);
        }
    }
    let whole = service_time(receipt.from, receipt.to)?;
    let running = u128::from(receipt.running_ms);
    let mut usage = vec![
        HandsUsage {
            meter: Meter::ComputeMillicpuMs,
            category: Category::Compute,
            quantity: u128::from(receipt.shape.baseline_millicpu()) * running,
            service_time: whole,
        },
        HandsUsage {
            meter: Meter::MemoryByteMs,
            category: Category::Compute,
            quantity: u128::from(receipt.shape.baseline_memory_bytes()) * running,
            service_time: whole,
        },
    ];
    if let Some(snapshot) = residence {
        let minutes = snapshot_minutes(snapshot);
        usage.push(HandsUsage {
            meter: Meter::StorageByteMin,
            category: Category::Storage,
            quantity: u128::from(snapshot.bytes) * u128::from(minutes),
            service_time: service_time(snapshot.suspended_at, snapshot.released_at)?,
        });
    }
    if let Some(bytes) = receipt.transmit_bytes {
        usage.push(HandsUsage {
            meter: Meter::DataTransferEgressByte,
            category: Category::Transfer,
            quantity: bytes.get(),
            service_time: whole,
        });
    }
    Ok(usage)
}

fn parsed_context(
    context: &FactContext,
) -> Result<
    (
        OrganizationId,
        WorkspaceId,
        RegionId,
        SessionId,
        PricingVersion,
    ),
    DerivationError,
> {
    Ok((
        OrganizationId::parse(&context.organization.to_string())
            .map_err(|error| DerivationError::Canonical(format!("organization: {error}")))?,
        WorkspaceId::parse(&context.workspace.to_string())
            .map_err(|error| DerivationError::Canonical(format!("workspace: {error}")))?,
        RegionId::parse(context.region.as_str())
            .map_err(|error| DerivationError::Canonical(format!("region: {error}")))?,
        SessionId::parse(&context.session.to_string())
            .map_err(|error| DerivationError::Canonical(format!("session: {error}")))?,
        PricingVersion::parse(&context.pricing_version.0)
            .map_err(|error| DerivationError::Canonical(format!("pricing version: {error}")))?,
    ))
}

fn measurement(
    usage: &HandsUsage,
    receipt: &RuntimeReceipt,
    residence: Option<&SnapshotResidence>,
    source_receipt_id: &str,
) -> Result<Measurement, DerivationError> {
    let source = SourceReceipt {
        kind: ReceiptKind::ProviderLifecycle,
        id: source_receipt_id.into(),
        digest: None,
    };
    let (basis, evidence) = match usage.meter {
        Meter::ComputeMillicpuMs => (
            FactBasis::Reserved,
            Evidence::ProviderShape {
                receipt_id: source_receipt_id.into(),
                generation: receipt.generation.to_string().into(),
                shape: shape(receipt.shape),
                running_ms: receipt.running_ms,
                unit: ShapeUnit::Millicpu,
            },
        ),
        Meter::MemoryByteMs => (
            FactBasis::Reserved,
            Evidence::ProviderShape {
                receipt_id: source_receipt_id.into(),
                generation: receipt.generation.to_string().into(),
                shape: shape(receipt.shape),
                running_ms: receipt.running_ms,
                unit: ShapeUnit::MemoryBytes,
            },
        ),
        Meter::StorageByteMin => {
            let snapshot = residence.ok_or_else(|| {
                DerivationError::Canonical("storage measurement has no residence".to_owned())
            })?;
            let owner_id = AuthorityId::parse(&snapshot.lifecycle_id)
                .map_err(|error| DerivationError::Canonical(error.to_string()))?;
            (
                FactBasis::Consumed,
                Evidence::StorageResidence {
                    owner: StorageOwner {
                        kind: StorageOwnerKind::MicrovmSnapshot,
                        id: owner_id,
                        generation: snapshot.generation,
                    },
                    source: StorageSource::S3,
                    bytes: snapshot.bytes,
                    minutes: snapshot_minutes(snapshot),
                    commit_id: source_receipt_id.into(),
                    close: if snapshot.terminal {
                        StorageClose::TerminalCeil
                    } else {
                        StorageClose::InteriorFloor
                    },
                },
            )
        }
        Meter::DataTransferEgressByte => (
            FactBasis::Consumed,
            Evidence::DeliveryReceipt {
                boundary: BoundaryId::HANDS_EGRESS,
                receipt_id: source_receipt_id.into(),
                bytes: u64::try_from(usage.quantity)
                    .map_err(|error| DerivationError::Canonical(error.to_string()))?,
            },
        ),
    };
    Measurement::new(usage.meter, basis, usage.service_time, source, evidence)
        .map_err(|error| DerivationError::Canonical(error.to_string()))
}

fn authority(
    context: &FactContext,
    region: &RegionId,
    usage: &HandsUsage,
    residence: Option<&SnapshotResidence>,
) -> Result<AuthorityKey, DerivationError> {
    let (kind, identity) = match usage.meter {
        Meter::StorageByteMin => (
            AuthorityKind::StorageResidence,
            residence
                .ok_or_else(|| DerivationError::Canonical("storage fact has no residence".into()))?
                .lifecycle_id
                .clone(),
        ),
        Meter::DataTransferEgressByte => (
            AuthorityKind::EgressCrossing,
            format!("{}:egress", context.intent.0),
        ),
        Meter::ComputeMillicpuMs => (
            AuthorityKind::HandsGeneration,
            format!("{}:compute", context.intent.0),
        ),
        Meter::MemoryByteMs => (
            AuthorityKind::HandsGeneration,
            format!("{}:memory", context.intent.0),
        ),
    };
    Ok(AuthorityKey {
        region: region.clone(),
        category: usage.category,
        kind,
        authority_id: AuthorityId::parse(&identity)
            .map_err(|error| DerivationError::Canonical(error.to_string()))?,
        segment_ordinal: SegmentOrdinal::FIRST,
    })
}

/// Derives canonical untrusted drafts for authority admission.
///
/// Equal lifecycle evidence produces equal drafts and therefore equal authority
/// fact identifiers. Compute and memory have distinct authority identities even
/// though both are admitted by the compute category.
///
/// # Errors
///
/// Returns [`DerivationError`] when the receipt or snapshot interval is invalid,
/// or when any identifier or evidence cannot satisfy the canonical usage model.
pub fn derive_facts(
    receipt: &RuntimeReceipt,
    residence: Option<&SnapshotResidence>,
    context: &FactContext,
) -> Result<Vec<FactDraft>, DerivationError> {
    let usage = derive_usage(receipt, residence)?;
    let (organization, workspace, region, session, pricing_version) = parsed_context(context)?;
    let service = ServiceId::parse(HANDS_SOURCE)
        .map_err(|error| DerivationError::Canonical(error.to_string()))?;
    usage
        .iter()
        .map(|usage| {
            Ok(FactDraft {
                schema_version: SCHEMA_VERSION,
                organization: organization.clone(),
                workspace: workspace.clone(),
                region: region.clone(),
                attribution: Attribution {
                    session: Some(session.clone()),
                    agent: None,
                    run: None,
                    operation: None,
                },
                service: service.clone(),
                resource: ResourceGeneration {
                    kind: ResourceKind::HandsGeneration,
                    generation: receipt.generation.to_string().into(),
                },
                authority: authority(context, &region, usage, residence)?,
                pricing_version: pricing_version.clone(),
                reservation: None,
                kind: FactKind::Measured(measurement(
                    usage,
                    receipt,
                    residence,
                    &context.source_receipt_id,
                )?),
            })
        })
        .collect()
}

/// Why a draft could not be handed to its authority ingress.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SinkError {
    /// The ingress refused the draft permanently.
    #[error("the {category:?} ingress refused the draft: {reason}")]
    Refused {
        /// Which ingress.
        category: UsageCategory,
        /// Why.
        reason: String,
    },
    /// The ingress was unavailable; the lifecycle record must redrive.
    #[error("the {category:?} ingress is unavailable: {reason}")]
    Unavailable {
        /// Which ingress.
        category: UsageCategory,
        /// Why.
        reason: String,
    },
}

/// A category-scoped producer of canonical usage drafts.
pub trait UsageFactSink: Send + Sync + 'static {
    /// Enqueues one draft for authority-owned admission.
    fn emit<'a>(
        &'a self,
        category: UsageCategory,
        draft: FactDraft,
    ) -> Pin<Box<dyn Future<Output = Result<(), SinkError>> + Send + 'a>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use aex_wire::ids::{GenerationId, PrefixedId as _, Uuid7};

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("bounded")
    }

    fn receipt(running_ms: u64, suspended_ms: u64) -> RuntimeReceipt {
        RuntimeReceipt {
            generation: GenerationId::from_uuid7(Uuid7::compose(11, [1; 10])),
            shape: ComputeSize::Gb1,
            running_ms,
            suspended_ms,
            from: at(0),
            to: at(i64::try_from(running_ms + suspended_ms).expect("bounded")),
            snapshot_bytes: None,
            transmit_bytes: None,
        }
    }

    fn context() -> FactContext {
        FactContext {
            organization: WireOrganizationId::from_uuid7(Uuid7::compose(2, [2; 10])),
            workspace: WireWorkspaceId::from_uuid7(Uuid7::compose(3, [3; 10])),
            region: Region::EuWest1,
            session: WireSessionId::from_uuid7(Uuid7::compose(4, [4; 10])),
            pricing_version: RuntimePricingVersion("2026-08-01".to_owned()),
            intent: LifecycleIntentId("lci_1".to_owned()),
            source_receipt_id: "lambda-microvm:mvm-1:suspend:req-1".to_owned(),
        }
    }

    #[test]
    fn provider_shape_is_exact_and_authority_admission_is_not_forged() {
        let drafts = derive_facts(&receipt(3_600_000, 0), None, &context()).expect("drafts");
        assert_eq!(drafts.len(), 2);
        assert_eq!(drafts[0].authority.category, Category::Compute);
        assert_eq!(drafts[1].authority.category, Category::Compute);
        assert_ne!(drafts[0].fact_id(), drafts[1].fact_id());
        for draft in drafts {
            assert_eq!(draft.schema_version, SCHEMA_VERSION);
            assert!(matches!(draft.kind, FactKind::Measured(_)));
        }
    }

    #[test]
    fn retry_derives_identical_drafts_and_a_new_intent_does_not_collapse() {
        let closed = receipt(1_000, 0);
        let first = derive_facts(&closed, None, &context()).expect("first");
        let second = derive_facts(&closed, None, &context()).expect("second");
        assert_eq!(first, second);
        let mut later = context();
        later.intent = LifecycleIntentId("lci_2".to_owned());
        let later = derive_facts(&closed, None, &later).expect("later");
        assert_ne!(first[0].fact_id(), later[0].fact_id());
    }

    #[test]
    fn terminal_storage_close_ceils_only_the_final_tail() {
        let closed = receipt(0, 60_001);
        let mut snapshot = SnapshotResidence {
            lifecycle_id: "snapshot-lifecycle:mvm-1:0".to_owned(),
            generation: 0,
            bytes: 10,
            suspended_at: at(0),
            released_at: at(60_001),
            terminal: false,
            io: SnapshotIo::default(),
        };
        let interior = derive_usage(&closed, Some(&snapshot)).expect("interior");
        assert_eq!(interior[2].quantity, 10);
        snapshot.terminal = true;
        let terminal = derive_usage(&closed, Some(&snapshot)).expect("terminal");
        assert_eq!(terminal[2].quantity, 20);
    }

    #[test]
    fn snapshot_io_and_unreceipted_egress_never_become_facts() {
        let drafts = derive_facts(&receipt(1_000, 0), None, &context()).expect("drafts");
        assert!(drafts.iter().all(|draft| {
            draft.kind.meter() != Some(Meter::DataTransferEgressByte)
                && draft.kind.meter() != Some(Meter::StorageByteMin)
        }));
    }
}
