//! Runtime receipts and the facts derived from them.
//!
//! A [`RuntimeReceipt`] can only be constructed from a [`ProviderLifecycleEvidence`],
//! and that value can only be built from a provider control-plane response. Nothing
//! reachable inside the guest can produce either: the guest crates do not depend on
//! this crate at all, and the constructor takes a value they cannot name (B8).

use serde::{Deserialize, Serialize};

use crate::lifecycle::LifecycleAction;
use crate::shape::ComputeSize;
use crate::usage::{
    Attribution, FactBasis, FactId, Meter, ServiceTime, UsageCategory, UsageFact,
};
use crate::wire_pending::{
    GenerationId, LifecycleIntentId, MicrovmId, OrganizationId, ProviderRequestId, Timestamp,
    WorkspaceId,
};

/// One millisecond-resolution provider lifecycle observation.
///
/// The only constructor takes a provider request id, which is produced by an AWS
/// SDK response and by nothing else in the workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderLifecycleEvidence {
    microvm: MicrovmId,
    request: ProviderRequestId,
    action: LifecycleAction,
    from: Timestamp,
    to: Timestamp,
    running_ms: u64,
    suspended_ms: u64,
}

impl ProviderLifecycleEvidence {
    /// Records what the provider observed over one closed interval.
    ///
    /// The provider request id is mandatory: a lifecycle response with no request
    /// id carries no evidence, and AEX never invents one.
    #[must_use]
    pub const fn from_provider_response(
        microvm: MicrovmId,
        request: ProviderRequestId,
        action: LifecycleAction,
        from: Timestamp,
        to: Timestamp,
        running_ms: u64,
        suspended_ms: u64,
    ) -> Self {
        Self {
            microvm,
            request,
            action,
            from,
            to,
            running_ms,
            suspended_ms,
        }
    }

    /// The MicroVM the provider observed.
    #[must_use]
    pub const fn microvm(&self) -> &MicrovmId {
        &self.microvm
    }

    /// The provider request id.
    #[must_use]
    pub const fn request(&self) -> &ProviderRequestId {
        &self.request
    }
}

/// Retained snapshot bytes over one suspension interval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotResidence {
    /// The AEX-minted snapshot lifecycle identity.
    pub lifecycle_id: String,
    /// Retained bytes. Declared by the signed image catalog until the provider
    /// exposes an actual size.
    pub bytes: u64,
    /// Start of retention, inclusive.
    pub suspended_at: Timestamp,
    /// End of retention, exclusive.
    pub released_at: Timestamp,
    /// Bytes written into the snapshot on suspend. Zero-dollar observability.
    pub write_bytes: u64,
    /// Bytes read back from the snapshot on resume. Zero-dollar observability.
    pub read_bytes: u64,
}

/// A closed runtime interval with everything needed to derive facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeReceipt {
    evidence: ProviderLifecycleEvidence,
    generation: GenerationId,
    intent: LifecycleIntentId,
    organization: OrganizationId,
    workspace: WorkspaceId,
    size: ComputeSize,
    snapshot: Option<SnapshotResidence>,
    /// Provider-authoritative transmit bytes for this generation.
    ///
    /// `None` means no Hands transfer fact. The provider exposes no per-generation
    /// transmit receipt today (OD-26), and a guest counter can never substitute
    /// because customer root can falsify it.
    transmit_bytes: Option<u128>,
}

/// Why a receipt was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ReceiptInvalid {
    /// The interval runs backwards.
    #[error("the interval ends {to} ms before it starts at {from} ms")]
    BackwardsInterval {
        /// Interval start, epoch milliseconds.
        from: i64,
        /// Interval end, epoch milliseconds.
        to: i64,
    },
    /// The running and suspended split does not exhaust the interval.
    ///
    /// An unexplained remainder over-charges if billed as running and under-charges
    /// if dropped, so it is a hard error either way.
    #[error("running {running_ms} ms + suspended {suspended_ms} ms != the {span_ms} ms interval")]
    UnexplainedRemainder {
        /// Running milliseconds claimed.
        running_ms: u64,
        /// Suspended milliseconds claimed.
        suspended_ms: u64,
        /// The interval length.
        span_ms: u64,
    },
    /// A snapshot residence lies outside the receipt's interval.
    #[error("the snapshot residence is not contained in the receipt interval")]
    SnapshotOutsideInterval,
    /// A snapshot residence runs backwards.
    #[error("the snapshot residence ends before it starts")]
    BackwardsSnapshot,
}

impl RuntimeReceipt {
    /// Closes an interval from provider evidence.
    ///
    /// # Errors
    ///
    /// See [`ReceiptInvalid`].
    #[allow(clippy::too_many_arguments)]
    pub fn close(
        evidence: ProviderLifecycleEvidence,
        generation: GenerationId,
        intent: LifecycleIntentId,
        organization: OrganizationId,
        workspace: WorkspaceId,
        size: ComputeSize,
        snapshot: Option<SnapshotResidence>,
        transmit_bytes: Option<u128>,
    ) -> Result<Self, ReceiptInvalid> {
        let receipt = Self {
            evidence,
            generation,
            intent,
            organization,
            workspace,
            size,
            snapshot,
            transmit_bytes,
        };
        receipt.validate()?;
        Ok(receipt)
    }

    /// The interval start.
    #[must_use]
    pub const fn from(&self) -> Timestamp {
        self.evidence.from
    }

    /// The interval end.
    #[must_use]
    pub const fn to(&self) -> Timestamp {
        self.evidence.to
    }

    /// Provider-observed running milliseconds, including retained-running idle
    /// time until suspension.
    #[must_use]
    pub const fn running_ms(&self) -> u64 {
        self.evidence.running_ms
    }

    /// Provider-observed suspended milliseconds.
    #[must_use]
    pub const fn suspended_ms(&self) -> u64 {
        self.evidence.suspended_ms
    }

    /// The shape the interval was billed at.
    #[must_use]
    pub const fn size(&self) -> ComputeSize {
        self.size
    }

    /// The retained snapshot, if the interval had one.
    #[must_use]
    pub const fn snapshot(&self) -> Option<&SnapshotResidence> {
        self.snapshot.as_ref()
    }

    /// Provider-authoritative transmit bytes, or `None`.
    #[must_use]
    pub const fn transmit_bytes(&self) -> Option<u128> {
        self.transmit_bytes
    }

    /// The exact-accounting check.
    ///
    /// # Errors
    ///
    /// See [`ReceiptInvalid`].
    pub fn validate(&self) -> Result<(), ReceiptInvalid> {
        let from = self.evidence.from;
        let to = self.evidence.to;
        if to < from {
            return Err(ReceiptInvalid::BackwardsInterval {
                from: from.millis(),
                to: to.millis(),
            });
        }
        let span_ms = to.saturating_millis_since(from);
        let claimed = self
            .evidence
            .running_ms
            .checked_add(self.evidence.suspended_ms);
        if claimed != Some(span_ms) {
            return Err(ReceiptInvalid::UnexplainedRemainder {
                running_ms: self.evidence.running_ms,
                suspended_ms: self.evidence.suspended_ms,
                span_ms,
            });
        }
        if let Some(snapshot) = &self.snapshot {
            if snapshot.released_at < snapshot.suspended_at {
                return Err(ReceiptInvalid::BackwardsSnapshot);
            }
            if snapshot.suspended_at < from || snapshot.released_at > to {
                return Err(ReceiptInvalid::SnapshotOutsideInterval);
            }
        }
        Ok(())
    }

    fn attribution(&self) -> Attribution {
        Attribution {
            organization: self.organization,
            workspace: self.workspace,
            generation: self.generation,
        }
    }

    fn fact(&self, meter: Meter, quantity: u128, basis: FactBasis, when: ServiceTime) -> UsageFact {
        UsageFact {
            fact_id: FactId::derive(self.generation, self.intent, meter),
            meter,
            quantity,
            basis,
            attribution: self.attribution(),
            service_time: when,
            intent: self.intent,
        }
    }

    /// Every fact this closed interval produces, in a stable order.
    ///
    /// Compute and memory come from the exact provider shape times the
    /// provider-authoritative running interval. Storage comes from retained
    /// snapshot bytes. Snapshot I/O is emitted as zero-dollar observability. No
    /// transfer fact is produced unless the provider supplied transmit bytes.
    #[must_use]
    pub fn facts(&self) -> Vec<UsageFact> {
        let whole = ServiceTime {
            from: self.evidence.from,
            to: self.evidence.to,
        };
        let running = u128::from(self.evidence.running_ms);
        let mut facts = vec![
            self.fact(
                Meter::ComputeMillicpuMs,
                u128::from(self.size.baseline_millicpu()) * running,
                FactBasis::ProviderLifecycle,
                whole,
            ),
            self.fact(
                Meter::MemoryByteMs,
                u128::from(self.size.baseline_memory_bytes()) * running,
                FactBasis::ProviderLifecycle,
                whole,
            ),
        ];
        if let Some(snapshot) = &self.snapshot {
            let retained = ServiceTime {
                from: snapshot.suspended_at,
                to: snapshot.released_at,
            };
            let minutes = u128::from(
                snapshot
                    .released_at
                    .saturating_millis_since(snapshot.suspended_at)
                    / 60_000,
            );
            facts.push(self.fact(
                Meter::StorageByteMin,
                u128::from(snapshot.bytes) * minutes,
                FactBasis::DeclaredImageCatalog,
                retained,
            ));
            facts.push(self.fact(
                Meter::ObservabilitySnapshotWriteByte,
                u128::from(snapshot.write_bytes),
                FactBasis::ProviderLifecycle,
                retained,
            ));
            facts.push(self.fact(
                Meter::ObservabilitySnapshotReadByte,
                u128::from(snapshot.read_bytes),
                FactBasis::ProviderLifecycle,
                retained,
            ));
        }
        if let Some(bytes) = self.transmit_bytes {
            facts.push(self.fact(
                Meter::DataTransferEgressByte,
                bytes,
                FactBasis::ProviderLifecycle,
                whole,
            ));
        }
        facts
    }

    /// The facts this receipt sends to `category`.
    #[must_use]
    pub fn facts_for(&self, category: UsageCategory) -> Vec<UsageFact> {
        self.facts()
            .into_iter()
            .filter(|fact| fact.meter.category() == category)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ProviderLifecycleEvidence, ReceiptInvalid, RuntimeReceipt, SnapshotResidence,
    };
    use crate::lifecycle::LifecycleAction;
    use crate::shape::ComputeSize;
    use crate::usage::{Meter, UsageCategory};
    use crate::wire_pending::{
        GenerationId, LifecycleIntentId, MicrovmId, OrganizationId, ProviderRequestId, Timestamp,
        WorkspaceId,
    };
    use uuid::Uuid;

    const FROM: i64 = 1_000_000;

    fn receipt_of(
        running_ms: u64,
        suspended_ms: u64,
        span_ms: u64,
        size: ComputeSize,
        snapshot: Option<SnapshotResidence>,
        transmit_bytes: Option<u128>,
    ) -> Result<RuntimeReceipt, ReceiptInvalid> {
        RuntimeReceipt::close(
            ProviderLifecycleEvidence::from_provider_response(
                MicrovmId::new("mvm-1"),
                ProviderRequestId::new("req-1"),
                LifecycleAction::Suspend,
                Timestamp::from_millis(FROM),
                Timestamp::from_millis(FROM).saturating_add_millis(span_ms),
                running_ms,
                suspended_ms,
            ),
            GenerationId::from_bytes([1; 16]),
            LifecycleIntentId::from_uuid(Uuid::from_bytes([2; 16])),
            OrganizationId::from_bytes([3; 16]),
            WorkspaceId::from_bytes([4; 16]),
            size,
            snapshot,
            transmit_bytes,
        )
    }

    #[test]
    fn any_partition_of_the_interval_validates_and_any_remainder_fails() {
        let span = 600_000_u64;
        for running in [0_u64, 1, 59_999, 300_000, span - 1, span] {
            assert!(
                receipt_of(running, span - running, span, ComputeSize::Gb1, None, None).is_ok(),
                "running {running} of {span}"
            );
        }
        for (running, suspended) in [(0_u64, 0_u64), (span, 1), (span - 1, 0), (span + 1, 0)] {
            let error = receipt_of(running, suspended, span, ComputeSize::Gb1, None, None)
                .expect_err("an unexplained remainder is a hard error");
            assert_eq!(
                error,
                ReceiptInvalid::UnexplainedRemainder {
                    running_ms: running,
                    suspended_ms: suspended,
                    span_ms: span
                }
            );
        }
    }

    #[test]
    fn a_backwards_interval_is_refused() {
        let error = RuntimeReceipt::close(
            ProviderLifecycleEvidence::from_provider_response(
                MicrovmId::new("mvm-1"),
                ProviderRequestId::new("req-1"),
                LifecycleAction::Suspend,
                Timestamp::from_millis(FROM),
                Timestamp::from_millis(FROM - 1),
                0,
                0,
            ),
            GenerationId::from_bytes([1; 16]),
            LifecycleIntentId::from_uuid(Uuid::from_bytes([2; 16])),
            OrganizationId::from_bytes([3; 16]),
            WorkspaceId::from_bytes([4; 16]),
            ComputeSize::Gb1,
            None,
            None,
        )
        .expect_err("time must not run backwards");
        assert_eq!(
            error,
            ReceiptInvalid::BackwardsInterval {
                from: FROM,
                to: FROM - 1
            }
        );
    }

    #[test]
    fn compute_and_memory_quantities_are_exact_for_all_five_shapes() {
        let running = 600_000_u64;
        let expected = [
            (ComputeSize::Mb512, 250_u128, 536_870_912_u128),
            (ComputeSize::Gb1, 500, 1_073_741_824),
            (ComputeSize::Gb2, 1_000, 2_147_483_648),
            (ComputeSize::Gb4, 2_000, 4_294_967_296),
            (ComputeSize::Gb8, 4_000, 8_589_934_592),
        ];
        for (size, millicpu, bytes) in expected {
            let receipt = receipt_of(running, 0, running, size, None, None).expect("valid");
            let facts = receipt.facts();
            assert_eq!(facts.len(), 2, "{size}");
            assert_eq!(facts[0].meter, Meter::ComputeMillicpuMs);
            assert_eq!(facts[0].quantity, millicpu * u128::from(running), "{size}");
            assert_eq!(facts[1].meter, Meter::MemoryByteMs);
            assert_eq!(facts[1].quantity, bytes * u128::from(running), "{size}");
        }
    }

    #[test]
    fn retained_running_idle_time_is_billed_until_suspension() {
        // 60 s of work then 180 s of retained-running idle before suspension: all
        // 240 s are running milliseconds, because the provider held the shape.
        let receipt = receipt_of(240_000, 360_000, 600_000, ComputeSize::Gb1, None, None)
            .expect("valid");
        assert_eq!(receipt.running_ms(), 240_000);
        assert_eq!(
            receipt.facts()[0].quantity,
            500 * 240_000,
            "the whole retained-running interval is metered"
        );
    }

    #[test]
    fn snapshot_io_is_zero_dollar_observability_and_never_a_transfer_fact() {
        let snapshot = SnapshotResidence {
            lifecycle_id: "snapshot-lifecycle:mvm-1:0".to_owned(),
            bytes: 445_644_800,
            suspended_at: Timestamp::from_millis(FROM + 300_000),
            released_at: Timestamp::from_millis(FROM + 600_000),
            write_bytes: 445_644_800,
            read_bytes: 445_644_800,
        };
        let receipt = receipt_of(
            300_000,
            300_000,
            600_000,
            ComputeSize::Gb1,
            Some(snapshot),
            None,
        )
        .expect("valid");
        let facts = receipt.facts();
        assert_eq!(facts.len(), 5);
        assert_eq!(facts[2].meter, Meter::StorageByteMin);
        assert_eq!(facts[2].quantity, 445_644_800 * 5, "300 s is five minutes");
        assert_eq!(facts[3].meter, Meter::ObservabilitySnapshotWriteByte);
        assert_eq!(facts[4].meter, Meter::ObservabilitySnapshotReadByte);
        for fact in &facts {
            assert_ne!(
                fact.meter,
                Meter::DataTransferEgressByte,
                "snapshot I/O must never become an egress fact"
            );
        }
        assert!(receipt.facts_for(UsageCategory::Transfer).is_empty());
        assert_eq!(receipt.facts_for(UsageCategory::Storage).len(), 1);
        assert_eq!(receipt.facts_for(UsageCategory::Compute).len(), 4);
    }

    #[test]
    fn no_transfer_fact_exists_while_the_provider_exposes_no_transmit_receipt() {
        let receipt = receipt_of(600_000, 0, 600_000, ComputeSize::Gb1, None, None).expect("valid");
        assert_eq!(receipt.transmit_bytes(), None);
        assert!(receipt.facts_for(UsageCategory::Transfer).is_empty());

        // If a provider receipt ever appears, the meter turns on with no redesign.
        let metered =
            receipt_of(600_000, 0, 600_000, ComputeSize::Gb1, None, Some(4_096)).expect("valid");
        let transfer = metered.facts_for(UsageCategory::Transfer);
        assert_eq!(transfer.len(), 1);
        assert_eq!(transfer[0].quantity, 4_096);
    }

    #[test]
    fn a_snapshot_outside_the_receipt_interval_is_refused() {
        let outside = SnapshotResidence {
            lifecycle_id: "snapshot-lifecycle:mvm-1:0".to_owned(),
            bytes: 1,
            suspended_at: Timestamp::from_millis(FROM - 1),
            released_at: Timestamp::from_millis(FROM + 100),
            write_bytes: 0,
            read_bytes: 0,
        };
        assert_eq!(
            receipt_of(600_000, 0, 600_000, ComputeSize::Gb1, Some(outside), None),
            Err(ReceiptInvalid::SnapshotOutsideInterval)
        );
        let backwards = SnapshotResidence {
            lifecycle_id: "snapshot-lifecycle:mvm-1:0".to_owned(),
            bytes: 1,
            suspended_at: Timestamp::from_millis(FROM + 200),
            released_at: Timestamp::from_millis(FROM + 100),
            write_bytes: 0,
            read_bytes: 0,
        };
        assert_eq!(
            receipt_of(600_000, 0, 600_000, ComputeSize::Gb1, Some(backwards), None),
            Err(ReceiptInvalid::BackwardsSnapshot)
        );
    }

    #[test]
    fn redelivery_produces_byte_identical_facts() {
        let first = receipt_of(600_000, 0, 600_000, ComputeSize::Gb1, None, None).expect("valid");
        let replay = receipt_of(600_000, 0, 600_000, ComputeSize::Gb1, None, None).expect("valid");
        assert_eq!(first.facts(), replay.facts());
        let ids: Vec<_> = first.facts().into_iter().map(|fact| fact.fact_id).collect();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1], "two meters never share a fact id");
    }
}
