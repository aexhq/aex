//! Egress counting: one physical crossing, one fact.
//!
//! [`EgressCounter::close`] consumes the counter, so emitting a second fact for
//! the same crossing does not compile. A crash before `close` leaves no fact at
//! all — the crossing is simply unbilled, which is the right way round: an
//! unbilled byte is a cost, a double-billed byte is a refund and an apology.
//!
//! Bytes are counted **after** compression, exactly as written to the socket.
//! TLS and IP framing stay provider overhead inside the rate margin.
//!
//! Receipt-mode crossings settle from a provider-authoritative delivery record
//! and never from a client callback. There is no constructor anywhere here that
//! turns a guest-reported or client-reported number into a fact.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use aex_usage_domain::fact::{Attribution, FactDraft, FactKind, SCHEMA_VERSION};
use aex_usage_domain::identity::{AuthorityId, AuthorityKey, AuthorityKind, SegmentOrdinal};
use aex_usage_domain::measurement::{
    BoundaryId, CounterEpoch, Evidence, FactBasis, Measurement, ReceiptKind, ServiceTime,
    SourceReceipt,
};
use aex_usage_domain::meter::{Category, Meter};
use aex_usage_domain::quantity::Quantity;
use aex_usage_domain::wire_pending::Timestamp;

use super::sink::{BoundedFactSink, FactSink};
use super::{ProbeContext, ProbeError};

/// A counter for exactly one physical crossing.
#[must_use = "an EgressCounter must be closed; dropping it emits nothing and is counted"]
#[derive(Debug)]
pub struct EgressCounter {
    boundary: BoundaryId,
    context: ProbeContext,
    attribution: Attribution,
    epoch: CounterEpoch,
    crossing_seq: u64,
    bytes: u64,
    sink: Arc<BoundedFactSink>,
    dropped: Arc<AtomicU64>,
    closed: bool,
}

impl EgressCounter {
    /// Opens a counter for one crossing.
    pub fn open(
        boundary: BoundaryId,
        context: ProbeContext,
        attribution: Attribution,
        epoch: CounterEpoch,
        crossing_seq: u64,
        sink: Arc<BoundedFactSink>,
        dropped: Arc<AtomicU64>,
    ) -> Self {
        Self {
            boundary,
            context,
            attribution,
            epoch,
            crossing_seq,
            bytes: 0,
            sink,
            dropped,
            closed: false,
        }
    }

    /// Counts encoded bytes written to the socket, after compression.
    ///
    /// # Errors
    ///
    /// Returns [`ProbeError::CounterOverflow`] on `u64` overflow, which no real
    /// crossing reaches and which must not silently wrap into a small charge.
    pub fn count(&mut self, bytes: u64) -> Result<(), ProbeError> {
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or(ProbeError::CounterOverflow {
                boundary: self.boundary.id(),
            })?;
        Ok(())
    }

    /// Bytes counted so far.
    #[must_use]
    pub const fn counted(&self) -> u64 {
        self.bytes
    }

    /// Which boundary this crossing is on.
    #[must_use]
    pub const fn boundary(&self) -> BoundaryId {
        self.boundary
    }

    /// Closes the crossing and offers exactly one fact.
    ///
    /// Consumes the counter, so a second fact for the same crossing is a
    /// compile error rather than a runtime duplicate.
    ///
    /// # Errors
    ///
    /// Returns [`ProbeError::Measurement`] when the counted bytes fail the
    /// measurement rules, and [`ProbeError::Sink`] when the sink refuses.
    pub fn close(mut self, at: Timestamp) -> Result<Quantity, ProbeError> {
        let authority_id = AuthorityId::parse(&format!(
            "{}:{}:{}",
            self.boundary.id(),
            self.epoch.get(),
            self.crossing_seq
        ))?;
        let measurement = Measurement::new(
            Meter::DataTransferEgressByte,
            FactBasis::Consumed,
            ServiceTime::Instant { at },
            SourceReceipt {
                kind: ReceiptKind::BoundaryCounter,
                id: Box::from(authority_id.as_str()),
                digest: None,
            },
            Evidence::BoundaryCounter {
                boundary: self.boundary,
                epoch: self.epoch,
                start: 0,
                end: self.bytes,
            },
        )?;
        let quantity = measurement.quantity();
        let draft = FactDraft {
            schema_version: SCHEMA_VERSION,
            organization: self.context.organization.clone(),
            workspace: self.context.workspace.clone(),
            region: self.context.region.clone(),
            attribution: self.attribution.clone(),
            service: self.context.service.clone(),
            resource: self.context.resource.clone(),
            authority: AuthorityKey {
                region: self.context.region.clone(),
                category: Category::Transfer,
                kind: AuthorityKind::EgressCrossing,
                authority_id,
                segment_ordinal: SegmentOrdinal::FIRST,
            },
            pricing_version: self.context.pricing_version.clone(),
            reservation: self.context.reservation.clone(),
            kind: FactKind::Measured(measurement),
        };
        self.sink.offer(draft)?;
        self.closed = true;
        Ok(quantity)
    }
}

impl Drop for EgressCounter {
    fn drop(&mut self) {
        if self.closed {
            return;
        }
        // Emitting nothing is deliberate. A crossing that was interrupted has
        // no authoritative byte count, and inventing one would bill an
        // unmeasured quantity. The drop is counted so the gap is visible.
        self.dropped.fetch_add(1, Ordering::Relaxed);
    }
}

/// A provider-authoritative delivery record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoundaryReceipt {
    /// An S3 access-log delivery line.
    S3Delivery {
        /// Which boundary delivered it.
        boundary: BoundaryId,
        /// The provider request identifier.
        request_id: Box<str>,
        /// Bytes the provider reports delivering.
        bytes: u64,
    },
    /// A CDN delivery line.
    CdnDelivery {
        /// Which boundary delivered it.
        boundary: BoundaryId,
        /// The provider request identifier.
        request_id: Box<str>,
        /// Bytes the provider reports delivering.
        bytes: u64,
    },
    /// An authenticated delivery through the hosting edge.
    VercelAuthenticated {
        /// The provider request identifier.
        request_id: Box<str>,
        /// Bytes the provider reports delivering.
        bytes: u64,
    },
    /// A Hands generation's provider transmit receipt.
    ///
    /// `transmit_bytes: None` means the provider exposes no per-generation
    /// transmit receipt, so **no transfer fact exists** for that generation.
    HandsProviderTransmit {
        /// The generation.
        generation: Box<str>,
        /// The provider receipt identifier.
        receipt_id: Box<str>,
        /// Bytes the provider reports transmitting, when it reports any.
        transmit_bytes: Option<u64>,
    },
}

impl BoundaryReceipt {
    /// Which boundary the delivery crossed.
    #[must_use]
    pub const fn boundary(&self) -> BoundaryId {
        match self {
            Self::S3Delivery { boundary, .. } | Self::CdnDelivery { boundary, .. } => *boundary,
            // Both are fixed boundaries rather than caller-supplied ones, so
            // neither carries a `boundary` field to read back.
            Self::VercelAuthenticated { .. } => BoundaryId::VERCEL_AUTHENTICATED,
            Self::HandsProviderTransmit { .. } => BoundaryId::HANDS_EGRESS,
        }
    }

    /// The provider's own record identifier.
    #[must_use]
    pub fn receipt_id(&self) -> &str {
        match self {
            Self::S3Delivery { request_id, .. }
            | Self::CdnDelivery { request_id, .. }
            | Self::VercelAuthenticated { request_id, .. } => request_id,
            Self::HandsProviderTransmit { receipt_id, .. } => receipt_id,
        }
    }
}

/// Builds a receipt-settled egress draft.
///
/// # Errors
///
/// Returns [`ProbeError::NoTransmitEvidence`] for
/// `HandsProviderTransmit { transmit_bytes: None }`. Estimating would bill an
/// unmeasured quantity, so the refusal is the whole point.
pub fn egress_from_receipt(
    context: &ProbeContext,
    attribution: &Attribution,
    at: Timestamp,
    receipt: &BoundaryReceipt,
) -> Result<FactDraft, ProbeError> {
    let bytes = match receipt {
        BoundaryReceipt::S3Delivery { bytes, .. }
        | BoundaryReceipt::CdnDelivery { bytes, .. }
        | BoundaryReceipt::VercelAuthenticated { bytes, .. }
        | BoundaryReceipt::HandsProviderTransmit {
            transmit_bytes: Some(bytes),
            ..
        } => *bytes,
        BoundaryReceipt::HandsProviderTransmit {
            generation,
            transmit_bytes: None,
            ..
        } => {
            return Err(ProbeError::NoTransmitEvidence {
                generation: generation.to_string(),
            });
        }
    };

    let boundary = receipt.boundary();
    let authority_id = AuthorityId::parse(&format!("{}:{}", boundary.id(), receipt.receipt_id()))?;
    let measurement = Measurement::new(
        Meter::DataTransferEgressByte,
        FactBasis::Consumed,
        ServiceTime::Instant { at },
        SourceReceipt {
            kind: ReceiptKind::DeliveryLog,
            id: Box::from(receipt.receipt_id()),
            digest: None,
        },
        Evidence::DeliveryReceipt {
            boundary,
            receipt_id: Box::from(receipt.receipt_id()),
            bytes,
        },
    )?;
    Ok(FactDraft {
        schema_version: SCHEMA_VERSION,
        organization: context.organization.clone(),
        workspace: context.workspace.clone(),
        region: context.region.clone(),
        attribution: attribution.clone(),
        service: context.service.clone(),
        resource: context.resource.clone(),
        authority: AuthorityKey {
            region: context.region.clone(),
            category: Category::Transfer,
            kind: AuthorityKind::DownloadGrant,
            authority_id,
            segment_ordinal: SegmentOrdinal::FIRST,
        },
        pricing_version: context.pricing_version.clone(),
        reservation: context.reservation.clone(),
        kind: FactKind::Measured(measurement),
    })
}

#[cfg(test)]
mod tests {
    use super::{BoundaryReceipt, CounterEpoch, EgressCounter, egress_from_receipt};
    use crate::probe::ProbeError;
    use crate::probe::sink::{BoundedFactSink, OverflowLedger};
    use crate::probe::testing::{at, probe_context};
    use aex_usage_domain::fact::{Attribution, FactKind};
    use aex_usage_domain::measurement::{BoundaryId, Evidence};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn sink() -> Arc<BoundedFactSink> {
        Arc::new(BoundedFactSink::new(32, Arc::new(OverflowLedger::new())).expect("capacity"))
    }

    fn counter(sink: &Arc<BoundedFactSink>, dropped: &Arc<AtomicU64>) -> EgressCounter {
        EgressCounter::open(
            BoundaryId::REGIONAL_HTTP,
            probe_context(),
            Attribution::default(),
            CounterEpoch::new(3),
            17,
            Arc::clone(sink),
            Arc::clone(dropped),
        )
    }

    #[test]
    fn one_crossing_produces_exactly_one_fact() {
        let sink = sink();
        let dropped = Arc::new(AtomicU64::new(0));
        let mut crossing = counter(&sink, &dropped);

        crossing.count(1_024).expect("counts");
        crossing.count(2_048).expect("counts");
        assert_eq!(crossing.counted(), 3_072);

        let quantity = crossing.close(at(1_000)).expect("closes");
        assert_eq!(quantity.get(), 3_072);

        let batch = sink.take_batch(32);
        assert_eq!(batch.len(), 1);
        let FactKind::Measured(measurement) = &batch[0].kind else {
            panic!("a closed crossing is a measured fact");
        };
        match measurement.evidence() {
            Evidence::BoundaryCounter {
                boundary,
                epoch,
                start,
                end,
            } => {
                assert_eq!(*boundary, BoundaryId::REGIONAL_HTTP);
                assert_eq!(epoch.get(), 3);
                assert_eq!(*start, 0);
                assert_eq!(*end, 3_072);
            }
            other => panic!("expected a counter, got {other:?}"),
        }
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn dropping_a_counter_emits_nothing_and_is_counted() {
        let sink = sink();
        let dropped = Arc::new(AtomicU64::new(0));
        {
            let mut crossing = counter(&sink, &dropped);
            crossing.count(4_096).expect("counts");
        }
        assert!(
            sink.take_batch(32).is_empty(),
            "an interrupted crossing has no authoritative byte count"
        );
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn a_counter_that_would_overflow_is_refused_rather_than_wrapped() {
        let sink = sink();
        let dropped = Arc::new(AtomicU64::new(0));
        let mut crossing = counter(&sink, &dropped);
        crossing.count(u64::MAX).expect("counts");
        assert!(matches!(
            crossing.count(1),
            Err(ProbeError::CounterOverflow { .. })
        ));
    }

    #[test]
    fn two_crossings_on_one_boundary_get_distinct_identities() {
        let sink = sink();
        let dropped = Arc::new(AtomicU64::new(0));

        let mut first = counter(&sink, &dropped);
        first.count(10).expect("counts");
        first.close(at(0)).expect("closes");

        let mut second = EgressCounter::open(
            BoundaryId::REGIONAL_HTTP,
            probe_context(),
            Attribution::default(),
            CounterEpoch::new(3),
            18,
            Arc::clone(&sink),
            Arc::clone(&dropped),
        );
        second.count(20).expect("counts");
        second.close(at(0)).expect("closes");

        let batch = sink.take_batch(32);
        assert_eq!(batch.len(), 2);
        assert_ne!(
            batch[0].fact_id(),
            batch[1].fact_id(),
            "two crossings are two facts"
        );
    }

    #[test]
    fn a_restart_opens_a_new_crossing_space() {
        let sink = sink();
        let dropped = Arc::new(AtomicU64::new(0));
        let build = |epoch| {
            EgressCounter::open(
                BoundaryId::REGIONAL_STREAM,
                probe_context(),
                Attribution::default(),
                CounterEpoch::new(epoch),
                1,
                Arc::clone(&sink),
                Arc::clone(&dropped),
            )
        };
        build(1).close(at(0)).expect("closes");
        build(2).close(at(0)).expect("closes");

        let batch = sink.take_batch(32);
        assert_ne!(
            batch[0].fact_id(),
            batch[1].fact_id(),
            "the same crossing sequence in a new epoch is a different crossing"
        );
    }

    #[test]
    fn a_delivery_receipt_settles_from_the_provider_record() {
        let draft = egress_from_receipt(
            &probe_context(),
            &Attribution::default(),
            at(500),
            &BoundaryReceipt::S3Delivery {
                boundary: BoundaryId::CONTENT_DOWNLOAD,
                request_id: Box::from("REQ-1"),
                bytes: 8_192,
            },
        )
        .expect("settles");

        let FactKind::Measured(measurement) = &draft.kind else {
            panic!("a delivery receipt is a measured fact");
        };
        assert_eq!(measurement.quantity().get(), 8_192);
    }

    #[test]
    fn a_hands_generation_without_a_transmit_receipt_produces_no_fact() {
        // The provider exposes no per-generation transmit receipt, so there is
        // nothing to bill. Estimating would charge an unmeasured quantity.
        let refused = egress_from_receipt(
            &probe_context(),
            &Attribution::default(),
            at(0),
            &BoundaryReceipt::HandsProviderTransmit {
                generation: Box::from("gen-1"),
                receipt_id: Box::from("rcpt-1"),
                transmit_bytes: None,
            },
        );
        assert!(matches!(
            refused,
            Err(ProbeError::NoTransmitEvidence { .. })
        ));

        // With a real receipt it settles normally.
        let settled = egress_from_receipt(
            &probe_context(),
            &Attribution::default(),
            at(0),
            &BoundaryReceipt::HandsProviderTransmit {
                generation: Box::from("gen-1"),
                receipt_id: Box::from("rcpt-1"),
                transmit_bytes: Some(4_096),
            },
        )
        .expect("settles");
        assert_eq!(
            settled
                .kind
                .measurement()
                .expect("measured")
                .quantity()
                .get(),
            4_096
        );
    }
}
