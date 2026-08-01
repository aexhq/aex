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
//! Snapshot read/write I/O is **zero-dollar observability**, never a
//! `data_transfer` fact (OD-25). Hands Internet egress is not charged at launch
//! (OD-26), so no transfer fact is produced while the provider exposes no
//! per-generation transmit receipt.

use core::future::Future;
use core::pin::Pin;

use serde::{Deserialize, Serialize};

use crate::wire_pending::{
    GenerationId, LifecycleIntentId, OrganizationId, Timestamp, WorkspaceId,
};

/// A dyn-compatible boxed future, so a sink is selectable at composition time.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The usage authority a fact belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageCategory {
    /// `usage-compute-authority`. Compute and memory are discriminated facts here.
    Compute,
    /// `usage-storage-authority`.
    Storage,
    /// `usage-transfer-authority`. Hands writes nothing here at launch.
    Transfer,
}

/// A launch meter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Meter {
    /// Billable compute: baseline millicpu times running milliseconds.
    #[serde(rename = "compute.millicpu_ms.v1")]
    ComputeMillicpuMs,
    /// Billable memory: baseline bytes times running milliseconds.
    #[serde(rename = "memory.byte_ms.v1")]
    MemoryByteMs,
    /// Billable storage: retained snapshot bytes times retained minutes.
    #[serde(rename = "storage.byte_min.v1")]
    StorageByteMin,
    /// Billable public-Internet egress. Hands emits none at launch (OD-26).
    #[serde(rename = "data_transfer.egress_byte.v1")]
    DataTransferEgressByte,
    /// Zero-dollar observability: bytes written into a snapshot on suspend.
    #[serde(rename = "observability.snapshot_write_byte.v1")]
    ObservabilitySnapshotWriteByte,
    /// Zero-dollar observability: bytes read back from a snapshot on resume.
    #[serde(rename = "observability.snapshot_read_byte.v1")]
    ObservabilitySnapshotReadByte,
}

impl Meter {
    /// The meter's wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ComputeMillicpuMs => "compute.millicpu_ms.v1",
            Self::MemoryByteMs => "memory.byte_ms.v1",
            Self::StorageByteMin => "storage.byte_min.v1",
            Self::DataTransferEgressByte => "data_transfer.egress_byte.v1",
            Self::ObservabilitySnapshotWriteByte => "observability.snapshot_write_byte.v1",
            Self::ObservabilitySnapshotReadByte => "observability.snapshot_read_byte.v1",
        }
    }

    /// The authority this meter's facts belong to.
    #[must_use]
    pub const fn category(self) -> UsageCategory {
        match self {
            Self::ComputeMillicpuMs
            | Self::MemoryByteMs
            | Self::ObservabilitySnapshotWriteByte
            | Self::ObservabilitySnapshotReadByte => UsageCategory::Compute,
            Self::StorageByteMin => UsageCategory::Storage,
            Self::DataTransferEgressByte => UsageCategory::Transfer,
        }
    }

    /// Whether this meter can ever produce a charge.
    ///
    /// The two snapshot-I/O meters are observability: at the Area 11 egress rate
    /// they would overcharge roughly 74 times on read and 183 times on write
    /// against provider cost, and the compute/memory rate margin already covers
    /// snapshot I/O as platform COGS.
    #[must_use]
    pub const fn is_billable(self) -> bool {
        !matches!(
            self,
            Self::ObservabilitySnapshotWriteByte | Self::ObservabilitySnapshotReadByte
        )
    }
}

/// Who a fact is attributed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attribution {
    /// The billed organization.
    pub organization: OrganizationId,
    /// The attributed workspace.
    pub workspace: WorkspaceId,
    /// The generation the fact came from.
    pub generation: GenerationId,
}

/// The closed interval a fact covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceTime {
    /// Start of the interval, inclusive.
    pub from: Timestamp,
    /// End of the interval, exclusive.
    pub to: Timestamp,
}

/// Where a fact's quantity came from. Hands never accepts a guest-reported basis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactBasis {
    /// The provider's own lifecycle evidence.
    ProviderLifecycle,
    /// The signed image catalog's declared snapshot size, pending the A11-METER
    /// shadow reconciliation.
    DeclaredImageCatalog,
}

/// One usage fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageFact {
    /// Deterministic identity, `blake3(generation ‖ intent ‖ meter)`. Never random:
    /// a random id cannot make a producer retry idempotent.
    pub fact_id: FactId,
    /// The meter.
    pub meter: Meter,
    /// The quantity in the meter's own unit.
    pub quantity: u128,
    /// Where the quantity came from.
    pub basis: FactBasis,
    /// Who it is attributed to.
    pub attribution: Attribution,
    /// The interval it covers.
    pub service_time: ServiceTime,
    /// The lifecycle intent that closed the interval.
    pub intent: LifecycleIntentId,
}

/// A deterministic fact identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FactId([u8; 32]);

impl FactId {
    /// Derives the identity from the authority key: the generation, the lifecycle
    /// intent that closed the interval, and the meter.
    ///
    /// A redelivered lifecycle event therefore produces a byte-identical fact and
    /// cannot double-bill.
    #[must_use]
    pub fn derive(generation: GenerationId, intent: LifecycleIntentId, meter: Meter) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&generation.to_bytes());
        hasher.update(&intent.to_bytes());
        hasher.update(meter.as_str().as_bytes());
        Self(*hasher.finalize().as_bytes())
    }

    /// The raw identity bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Why a usage sink refused a fact.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SinkError {
    /// The sink is not bound to the fact's category.
    #[error("this sink writes `{bound:?}` and refuses a `{offered:?}` fact")]
    WrongCategory {
        /// The category the sink is bound to.
        bound: UsageCategory,
        /// The category the fact belongs to.
        offered: UsageCategory,
    },
    /// The downstream authority rejected the write.
    #[error("the usage authority rejected the fact: {detail}")]
    Rejected {
        /// Why, with no customer or credential material.
        detail: Box<str>,
    },
}

/// Where Hands facts go.
///
/// `runtime-control-worker` binds one sink per category and must not link an
/// adapter capable of writing a sibling category's authority.
pub trait UsageFactSink: Send + Sync + 'static {
    /// Writes one fact into the authority named by its meter's category.
    ///
    /// # Errors
    ///
    /// Returns [`SinkError`] when the sink is bound to a different category or the
    /// authority rejects the write.
    fn emit<'a>(
        &'a self,
        category: UsageCategory,
        fact: &'a UsageFact,
    ) -> BoxFuture<'a, Result<(), SinkError>>;
}

#[cfg(test)]
mod tests {
    use super::{FactId, Meter, UsageCategory};
    use crate::wire_pending::{GenerationId, LifecycleIntentId};
    use uuid::Uuid;

    #[test]
    fn the_four_launch_meters_are_billable_and_snapshot_io_is_not() {
        for meter in [
            Meter::ComputeMillicpuMs,
            Meter::MemoryByteMs,
            Meter::StorageByteMin,
            Meter::DataTransferEgressByte,
        ] {
            assert!(meter.is_billable(), "{meter:?}");
        }
        for meter in [
            Meter::ObservabilitySnapshotWriteByte,
            Meter::ObservabilitySnapshotReadByte,
        ] {
            assert!(
                !meter.is_billable(),
                "snapshot I/O is zero-dollar observability: {meter:?}"
            );
            assert_ne!(
                meter.category(),
                UsageCategory::Transfer,
                "snapshot I/O must never reach the transfer authority"
            );
        }
    }

    #[test]
    fn each_meter_names_its_authority() {
        assert_eq!(
            Meter::ComputeMillicpuMs.category(),
            UsageCategory::Compute,
            "compute and memory are discriminated facts inside one authority"
        );
        assert_eq!(Meter::MemoryByteMs.category(), UsageCategory::Compute);
        assert_eq!(Meter::StorageByteMin.category(), UsageCategory::Storage);
        assert_eq!(
            Meter::DataTransferEgressByte.category(),
            UsageCategory::Transfer
        );
    }

    #[test]
    fn meter_names_round_trip_through_json() {
        for (meter, name) in [
            (Meter::ComputeMillicpuMs, "compute.millicpu_ms.v1"),
            (Meter::MemoryByteMs, "memory.byte_ms.v1"),
            (Meter::StorageByteMin, "storage.byte_min.v1"),
            (Meter::DataTransferEgressByte, "data_transfer.egress_byte.v1"),
        ] {
            assert_eq!(meter.as_str(), name);
            assert_eq!(
                serde_json::to_string(&meter).expect("a meter serializes"),
                format!("\"{name}\"")
            );
        }
    }

    #[test]
    fn a_fact_id_is_deterministic_in_generation_intent_and_meter() {
        let generation = GenerationId::from_bytes([1; 16]);
        let intent = LifecycleIntentId::from_uuid(Uuid::from_bytes([2; 16]));
        let first = FactId::derive(generation, intent, Meter::ComputeMillicpuMs);
        assert_eq!(
            first,
            FactId::derive(generation, intent, Meter::ComputeMillicpuMs),
            "a redelivered lifecycle event must produce the same fact id"
        );
        assert_ne!(
            first,
            FactId::derive(generation, intent, Meter::MemoryByteMs)
        );
        assert_ne!(
            first,
            FactId::derive(
                GenerationId::from_bytes([3; 16]),
                intent,
                Meter::ComputeMillicpuMs
            )
        );
        assert_ne!(
            first,
            FactId::derive(
                generation,
                LifecycleIntentId::from_uuid(Uuid::from_bytes([4; 16])),
                Meter::ComputeMillicpuMs
            )
        );
    }
}
