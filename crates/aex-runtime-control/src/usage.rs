//! Hands metering.
//!
//! Four meters exist workspace-wide. Hands produces three of them, from **provider
//! evidence only**:
//!
//! - `compute.millicpu_ms.v1` and `memory.byte_ms.v1` from the exact provider shape
//!   times the provider-authoritative running interval, including retained-running
//!   idle time until suspension;
//! - `storage.byte_min.v1` from retained snapshot bytes between suspension and
//!   resume or termination.
//!
//! Snapshot read/write I/O is **zero-dollar observability** and is not a
//! [`UsageFact`] at all (OD-25): at the `data_transfer.egress_byte.v1` rate it
//! would overcharge roughly 74x on read and 183x on write against provider cost.
//! It is carried as [`SnapshotIo`], which has no meter and cannot reach a rate
//! book. Hands Internet egress is not charged at launch (OD-26), so no transfer
//! fact is produced while the provider exposes no per-generation transmit receipt.

use core::future::Future;
use core::pin::Pin;

use aex_hands_protocol::lifecycle::RuntimeReceipt;
use aex_internal_contracts::usage::{
    Attribution, AuthorityKind, FactAuthority, FactBasis, FactId, FactIdempotency, Meter,
    ServiceTime, SourceReceipt, UsageFact,
};
use aex_internal_contracts::{PricingVersion, SchemaVersion};
use aex_wire::ids::{GenerationId, OrganizationId, WorkspaceId};
use aex_wire::types::{DecimalU128, Region, Timestamp};
use serde::{Deserialize, Serialize};

use crate::clock::millis_between;
use crate::lifecycle::LifecycleIntentId;
use crate::shape::ShapeCapacity as _;

/// Which authority ingress a fact belongs to.
///
/// Deliberately the contract's own [`AuthorityKind`] rather than a second
/// vocabulary: a Hands-local copy would need a mapping table, and a mapping table
/// is a place for the two to drift.
pub type UsageCategory = AuthorityKind;

/// The producer name every Hands usage fact carries.
pub const HANDS_SOURCE: &str = "runtime-control-worker";

/// Which authority ingress a meter's facts are written to.
#[must_use]
pub const fn category_of(meter: Meter) -> UsageCategory {
    match meter {
        // Memory is a discriminated fact inside the compute authority.
        Meter::ComputeMillicpuMs | Meter::MemoryByteMs => AuthorityKind::Compute,
        Meter::StorageByteMin => AuthorityKind::Storage,
        Meter::DataTransferEgressByte => AuthorityKind::Transfer,
    }
}

/// Snapshot read/write bytes, recorded and never priced.
///
/// This type carries no [`Meter`] and there is no conversion from it into a
/// [`UsageFact`]. That is the enforcement, not a comment: a caller cannot
/// accidentally bill snapshot I/O because there is no code path that would.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SnapshotIo {
    /// Bytes written into the snapshot on suspend.
    pub write_bytes: u64,
    /// Bytes read back from the snapshot on resume.
    pub read_bytes: u64,
}

/// Retained snapshot bytes over one suspension interval.
///
/// `bytes` is the *declared* value signed into the image catalog. The provider
/// exposes no snapshot size in any response today; if one appears it becomes
/// authoritative and this becomes a bound-check, which is why it is a field rather
/// than a constant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SnapshotResidence {
    /// The AEX-minted snapshot lifecycle identity. Never an AWS snapshot id.
    pub lifecycle_id: String,
    /// Retained bytes.
    pub bytes: u64,
    /// Start of retention, inclusive.
    pub suspended_at: Timestamp,
    /// End of retention, exclusive.
    pub released_at: Timestamp,
    /// Snapshot I/O over this residence. Zero-dollar observability.
    pub io: SnapshotIo,
}

/// One measured Hands quantity before it is wrapped in a wire fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandsUsage {
    /// Which priced meter.
    pub meter: Meter,
    /// Which authority ingress it belongs to.
    pub category: UsageCategory,
    /// How much, in the meter's own unit.
    pub quantity: u128,
    /// When the service happened.
    pub service_time: ServiceTime,
}

/// Everything a Hands measurement needs to become a wire [`UsageFact`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactContext {
    /// The account the money belongs to.
    pub organization: OrganizationId,
    /// The workspace the usage happened in.
    pub workspace: WorkspaceId,
    /// The region the usage happened in.
    pub region: Region,
    /// What the usage is attributed to.
    pub attribution: Attribution,
    /// Which rate book will price it.
    pub pricing_version: PricingVersion,
    /// The lifecycle intent that closed the interval.
    pub intent: LifecycleIntentId,
    /// The provider receipt identity, which is the producer's own evidence.
    pub source_receipt_id: String,
    /// When the worker observed the close.
    pub observed_at: Timestamp,
}

impl FactContext {
    /// The authority key every fact from one closed interval shares.
    ///
    /// Compute and memory share it deliberately: memory is a discriminated fact
    /// inside the compute authority, and [`FactId::derive`] already mixes the
    /// meter, so the two ids differ without a second authority key.
    fn authority(&self, generation: GenerationId, kind: UsageCategory) -> FactAuthority {
        FactAuthority {
            kind,
            authority_id: format!("{generation}:{}", self.intent).into_boxed_str(),
            segment_ordinal: DecimalU128::ZERO,
        }
    }
}

/// Why facts could not be derived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DerivationError {
    /// The receipt's own accounting does not add up.
    #[error("the runtime receipt is not exact: {0}")]
    Receipt(#[from] aex_hands_protocol::lifecycle::ReceiptError),
    /// A snapshot residence runs backwards.
    #[error("the snapshot residence ends before it starts")]
    BackwardsSnapshot,
    /// A snapshot residence lies outside the receipt's interval.
    #[error("the snapshot residence is not contained in the receipt interval")]
    SnapshotOutsideInterval,
}

/// Every measurement one closed interval produces, in a stable order.
///
/// Compute and memory come from the exact provider shape times the
/// provider-authoritative running interval, which includes retained-running idle
/// time until suspension. Storage comes from retained snapshot bytes. No transfer
/// measurement is produced unless the provider supplied transmit bytes, and
/// snapshot I/O never becomes one at all.
///
/// # Errors
///
/// Returns [`DerivationError`] when the receipt's running/suspended split does not
/// exhaust its interval exactly, or when a snapshot residence is backwards or
/// outside the receipt's interval.
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
    let whole = ServiceTime::Interval {
        start: receipt.from,
        end: receipt.to,
    };
    let running = u128::from(receipt.running_ms);
    let mut usage = vec![
        HandsUsage {
            meter: Meter::ComputeMillicpuMs,
            category: category_of(Meter::ComputeMillicpuMs),
            quantity: u128::from(receipt.shape.baseline_millicpu()) * running,
            service_time: whole,
        },
        HandsUsage {
            meter: Meter::MemoryByteMs,
            category: category_of(Meter::MemoryByteMs),
            quantity: u128::from(receipt.shape.baseline_memory_bytes()) * running,
            service_time: whole,
        },
    ];
    if let Some(snapshot) = residence {
        let minutes =
            u128::from(millis_between(snapshot.suspended_at, snapshot.released_at) / 60_000);
        usage.push(HandsUsage {
            meter: Meter::StorageByteMin,
            category: category_of(Meter::StorageByteMin),
            quantity: u128::from(snapshot.bytes) * minutes,
            service_time: ServiceTime::Interval {
                start: snapshot.suspended_at,
                end: snapshot.released_at,
            },
        });
    }
    if let Some(bytes) = receipt.transmit_bytes {
        usage.push(HandsUsage {
            meter: Meter::DataTransferEgressByte,
            category: category_of(Meter::DataTransferEgressByte),
            quantity: bytes.get(),
            service_time: whole,
        });
    }
    Ok(usage)
}

/// Every wire fact one closed interval produces, in a stable order.
///
/// The identity is [`FactId::derive`] over the authority key, so a redelivered
/// lifecycle event mints the same id and cannot double-bill.
///
/// `TODO(cross-stream) usage + finance`: the `business_key` grammar below is
/// `{region}/{meter}/{fact_id}`. It is deterministic and idempotent, which is the
/// property that matters here, but the settlement inbox owns the grammar and may
/// pin a different spelling.
///
/// # Errors
///
/// See [`derive_usage`].
pub fn derive_facts(
    receipt: &RuntimeReceipt,
    residence: Option<&SnapshotResidence>,
    context: &FactContext,
) -> Result<Vec<UsageFact>, DerivationError> {
    let measurements = derive_usage(receipt, residence)?;
    Ok(measurements
        .into_iter()
        .map(|usage| {
            let authority = context.authority(receipt.generation, usage.category);
            let fact_id = FactId::derive(context.region, usage.meter, &authority);
            let business_key = format!(
                "{}/{}/{}",
                context.region.as_str(),
                usage.meter.as_str(),
                fact_id
            );
            UsageFact {
                schema_version: SchemaVersion::V1,
                fact_id,
                meter: usage.meter,
                organization: context.organization,
                workspace: context.workspace,
                region: context.region,
                attribution: context.attribution,
                authority,
                basis: FactBasis::Consumed,
                quantity: DecimalU128::new(usage.quantity),
                service_time: usage.service_time,
                source_receipt: SourceReceipt {
                    source: HANDS_SOURCE.into(),
                    receipt_id: context.source_receipt_id.clone().into_boxed_str(),
                    observed_at: context.observed_at,
                },
                pricing_version: context.pricing_version.clone(),
                idempotency: FactIdempotency {
                    deduplication_id: fact_id.to_hex().into_boxed_str(),
                    business_key: business_key.into_boxed_str(),
                },
            }
        })
        .collect())
}

/// Why a fact could not be handed to its authority ingress.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SinkError {
    /// The ingress refused the fact permanently.
    #[error("the {category:?} ingress refused the fact: {reason}")]
    Refused {
        /// Which ingress.
        category: UsageCategory,
        /// Why.
        reason: String,
    },
    /// The ingress was unavailable; the caller redrives.
    #[error("the {category:?} ingress is unavailable: {reason}")]
    Unavailable {
        /// Which ingress.
        category: UsageCategory,
        /// Why.
        reason: String,
    },
}

/// Where derived facts go.
///
/// The category is an explicit argument so a composition can bind one sink per
/// authority and a worker cannot reach a sibling category's authority by
/// accident. `TODO(cross-stream) usage stream`: bind this to the compute and
/// storage category ingresses; `runtime-control-worker` must not link an adapter
/// capable of writing a sibling category's authority.
pub trait UsageFactSink: Send + Sync + 'static {
    /// Hands one fact to `category`'s ingress.
    ///
    /// Boxed rather than an `async fn` so the trait stays object-safe: the worker
    /// holds one `Arc<dyn UsageFactSink>` per category.
    fn emit<'a>(
        &'a self,
        category: UsageCategory,
        fact: UsageFact,
    ) -> Pin<Box<dyn Future<Output = Result<(), SinkError>> + Send + 'a>>;
}

#[cfg(test)]
mod tests {
    use super::{
        DerivationError, FactContext, HANDS_SOURCE, SnapshotIo, SnapshotResidence, category_of,
        derive_facts, derive_usage,
    };
    use crate::lifecycle::LifecycleIntentId;
    use aex_hands_protocol::lifecycle::RuntimeReceipt;
    use aex_internal_contracts::PricingVersion;
    use aex_internal_contracts::usage::{Attribution, AuthorityKind, Meter};
    use aex_wire::ids::{GenerationId, OrganizationId, PrefixedId as _, Uuid7, WorkspaceId};
    use aex_wire::types::{ComputeSize, DecimalU128, Region, Timestamp};

    const HOUR_MS: u64 = 3_600_000;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("a bounded instant")
    }

    fn generation() -> GenerationId {
        GenerationId::from_uuid7(Uuid7::compose(11, [1; 10]))
    }

    fn context() -> FactContext {
        FactContext {
            organization: OrganizationId::from_uuid7(Uuid7::compose(2, [2; 10])),
            workspace: WorkspaceId::from_uuid7(Uuid7::compose(3, [3; 10])),
            region: Region::EuWest1,
            attribution: Attribution {
                session: None,
                run: None,
                operation: None,
            },
            pricing_version: PricingVersion("2026-08-01".to_owned()),
            intent: LifecycleIntentId("lci_1".to_owned()),
            source_receipt_id: "lambda-microvm:mvm-1:suspend:req-1".to_owned(),
            observed_at: at(3_600_000),
        }
    }

    fn receipt(shape: ComputeSize, running_ms: u64, suspended_ms: u64) -> RuntimeReceipt {
        let span = i64::try_from(running_ms + suspended_ms).expect("a bounded span");
        RuntimeReceipt {
            generation: generation(),
            shape,
            running_ms,
            suspended_ms,
            from: at(0),
            to: at(span),
            snapshot_bytes: None,
            transmit_bytes: None,
        }
    }

    #[test]
    fn every_shape_meters_the_exact_baseline_times_the_running_interval() {
        // shape, compute quantity per running millisecond, memory quantity per ms
        let expected = [
            (ComputeSize::Mb512, 250_u128, 536_870_912_u128),
            (ComputeSize::Gb1, 500, 1_073_741_824),
            (ComputeSize::Gb2, 1_000, 2_147_483_648),
            (ComputeSize::Gb4, 2_000, 4_294_967_296),
            (ComputeSize::Gb8, 4_000, 8_589_934_592),
        ];
        for (shape, millicpu, bytes) in expected {
            let usage = derive_usage(&receipt(shape, HOUR_MS, 0), None).expect("an exact receipt");
            assert_eq!(usage.len(), 2, "{shape}");
            assert_eq!(usage[0].meter, Meter::ComputeMillicpuMs);
            assert_eq!(usage[0].quantity, millicpu * u128::from(HOUR_MS), "{shape}");
            assert_eq!(usage[1].meter, Meter::MemoryByteMs);
            assert_eq!(usage[1].quantity, bytes * u128::from(HOUR_MS), "{shape}");
        }
    }

    #[test]
    fn retained_running_idle_is_charged_and_suspended_time_is_not() {
        // Ten minutes running, of which the last three were idle, then fifty suspended.
        let usage = derive_usage(&receipt(ComputeSize::Gb1, 600_000, 3_000_000), None)
            .expect("an exact receipt");
        assert_eq!(usage[0].quantity, 500 * 600_000);
        assert_eq!(usage[1].quantity, 1_073_741_824 * 600_000);
    }

    #[test]
    fn an_unexplained_remainder_is_a_hard_error_in_both_directions() {
        let mut over = receipt(ComputeSize::Gb1, HOUR_MS, 0);
        over.running_ms += 1;
        assert!(matches!(
            derive_usage(&over, None),
            Err(DerivationError::Receipt(_))
        ));
        let mut under = receipt(ComputeSize::Gb1, HOUR_MS, 0);
        under.running_ms -= 1;
        assert!(matches!(
            derive_usage(&under, None),
            Err(DerivationError::Receipt(_))
        ));
    }

    fn residence(bytes: u64, suspended_at: i64, released_at: i64) -> SnapshotResidence {
        SnapshotResidence {
            lifecycle_id: "snapshot-lifecycle:mvm-1:0".to_owned(),
            bytes,
            suspended_at: at(suspended_at),
            released_at: at(released_at),
            io: SnapshotIo {
                write_bytes: 445_000_000,
                read_bytes: 445_000_000,
            },
        }
    }

    #[test]
    fn retained_snapshot_bytes_become_a_storage_fact_and_snapshot_io_becomes_nothing() {
        let closed = receipt(ComputeSize::Gb1, 600_000, 3_000_000);
        let usage = derive_usage(&closed, Some(&residence(445_000_000, 600_000, 3_600_000)))
            .expect("an exact receipt");
        assert_eq!(usage.len(), 3);
        assert_eq!(usage[2].meter, Meter::StorageByteMin);
        assert_eq!(usage[2].quantity, 445_000_000 * 50);
        assert!(
            usage
                .iter()
                .all(|fact| fact.meter != Meter::DataTransferEgressByte),
            "snapshot read/write is zero-dollar observability, never a transfer fact"
        );
    }

    #[test]
    fn no_transfer_fact_exists_while_the_provider_exposes_no_transmit_receipt() {
        let plain = receipt(ComputeSize::Gb1, HOUR_MS, 0);
        assert!(plain.transmit_bytes.is_none());
        let usage = derive_usage(&plain, None).expect("an exact receipt");
        assert!(
            usage
                .iter()
                .all(|fact| fact.meter != Meter::DataTransferEgressByte)
        );

        let mut measured = receipt(ComputeSize::Gb1, HOUR_MS, 0);
        measured.transmit_bytes = Some(DecimalU128::new(4_096));
        let with_transfer = derive_usage(&measured, None).expect("an exact receipt");
        let transfer = with_transfer
            .iter()
            .find(|fact| fact.meter == Meter::DataTransferEgressByte)
            .expect("a provider receipt turns the meter on");
        assert_eq!(transfer.quantity, 4_096);
    }

    #[test]
    fn a_snapshot_outside_the_receipt_interval_is_refused() {
        let closed = receipt(ComputeSize::Gb1, 600_000, 3_000_000);
        assert_eq!(
            derive_usage(&closed, Some(&residence(1, 600_000, 3_600_001))),
            Err(DerivationError::SnapshotOutsideInterval)
        );
        assert_eq!(
            derive_usage(&closed, Some(&residence(1, 3_600_000, 600_000))),
            Err(DerivationError::BackwardsSnapshot)
        );
    }

    #[test]
    fn every_meter_lands_in_exactly_one_authority_and_memory_rides_compute() {
        assert_eq!(
            category_of(Meter::ComputeMillicpuMs),
            AuthorityKind::Compute
        );
        assert_eq!(category_of(Meter::MemoryByteMs), AuthorityKind::Compute);
        assert_eq!(category_of(Meter::StorageByteMin), AuthorityKind::Storage);
        assert_eq!(
            category_of(Meter::DataTransferEgressByte),
            AuthorityKind::Transfer
        );
    }

    #[test]
    fn a_redelivered_lifecycle_event_mints_the_same_fact_ids() {
        let closed = receipt(ComputeSize::Gb1, HOUR_MS, 0);
        let first = derive_facts(&closed, None, &context()).expect("an exact receipt");
        let second = derive_facts(&closed, None, &context()).expect("an exact receipt");
        assert_eq!(first, second);
        assert_eq!(first.len(), 2);
        assert_ne!(
            first[0].fact_id, first[1].fact_id,
            "compute and memory share an authority key but never a fact id"
        );
        for fact in &first {
            assert_eq!(fact.source_receipt.source.as_ref(), HANDS_SOURCE);
            assert_eq!(
                fact.idempotency.deduplication_id.as_ref(),
                fact.fact_id.to_hex()
            );
            assert!(fact.idempotency.business_key.contains(fact.meter.as_str()));
        }
    }

    #[test]
    fn a_different_intent_mints_different_facts_so_two_intervals_never_collapse() {
        let closed = receipt(ComputeSize::Gb1, HOUR_MS, 0);
        let first = derive_facts(&closed, None, &context()).expect("an exact receipt");
        let mut later = context();
        later.intent = LifecycleIntentId("lci_2".to_owned());
        let second = derive_facts(&closed, None, &later).expect("an exact receipt");
        assert_ne!(first[0].fact_id, second[0].fact_id);
    }
}
