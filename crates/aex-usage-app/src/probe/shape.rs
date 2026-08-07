//! Allocated-shape producers: Hands generations and Lambda invocations.
//!
//! Both bill an allocation the customer holds rather than measured use, so both
//! carry `FactBasis::Reserved`. Two rules do the load-bearing work:
//!
//! - a Hands generation's `suspended_ms` bills **nothing** — neither compute nor
//!   memory — because a suspended `MicroVM` holds no capacity; and
//! - an unexplained remainder in `to - from` is a hard error, not a guess. If
//!   the running and suspended halves do not account for the generation's whole
//!   lifetime, the receipt is wrong and billing it would be inventing a number.

use aex_usage_domain::fact::{Attribution, FactDraft, FactKind, SCHEMA_VERSION};
use aex_usage_domain::identity::{AuthorityId, AuthorityKey, AuthorityKind, SegmentOrdinal};
use aex_usage_domain::measurement::{
    Evidence, FactBasis, LambdaVcpuConvention, Measurement, ReceiptKind, ServiceTime, SourceReceipt,
};
use aex_usage_domain::meter::{Category, Meter};
use aex_usage_domain::shape::{ComputeShape, HandsShape, ShapeUnit};
use aex_usage_domain::wire_pending::Timestamp;

use super::{ProbeContext, ProbeError};

/// One Hands generation's provider lifecycle receipt.
///
/// `TODO(cross-stream)`: `aex_hands_protocol::lifecycle::RuntimeReceipt` exists and
/// differs: it identifies the generation with a `GenerationId` and carries no receipt
/// id, its shape is an `aex_wire::types::ComputeSize` rather than a `HandsShape`, its
/// byte counts are `DecimalU128` rather than `u64`, and it adds `snapshot_bytes`.
/// Adopting it means this module derives its facts from the peer's units, so it is a
/// measurement change rather than an import. The fields below are exactly the ones the
/// compute, memory and transfer facts are derived from; nothing here interprets guest
/// state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeReceipt {
    /// The provider receipt identifier.
    pub receipt_id: Box<str>,
    /// The generation this receipt closes.
    pub generation: Box<str>,
    /// The public baseline token the generation ran at.
    pub shape: HandsShape,
    /// When the generation started.
    pub from: Timestamp,
    /// When the generation ended.
    pub to: Timestamp,
    /// Milliseconds the generation was running.
    pub running_ms: u64,
    /// Milliseconds the generation was suspended. Bills nothing.
    pub suspended_ms: u64,
    /// Bytes the provider reports transmitting, when it reports any at all.
    pub transmit_bytes: Option<u64>,
}

impl RuntimeReceipt {
    /// Proves the receipt accounts for its own lifetime.
    ///
    /// # Errors
    ///
    /// Returns [`ProbeError::UnexhaustedLifetime`] when `running_ms` and
    /// `suspended_ms` do not sum to `to - from`. A remainder means some of the
    /// generation's life is unexplained, and billing an unexplained interval is
    /// exactly the guess this refuses to make.
    pub fn validate(&self) -> Result<u64, ProbeError> {
        let lifetime_ms =
            self.from
                .millis_until(self.to)
                .ok_or_else(|| ProbeError::InvertedLifetime {
                    generation: self.generation.to_string(),
                })?;
        let accounted =
            self.running_ms
                .checked_add(self.suspended_ms)
                .ok_or(ProbeError::CounterOverflow {
                    boundary: "hands lifetime",
                })?;
        if accounted != lifetime_ms {
            return Err(ProbeError::UnexhaustedLifetime {
                generation: self.generation.to_string(),
                lifetime_ms,
                accounted_ms: accounted,
            });
        }
        Ok(lifetime_ms)
    }
}

/// Builds the compute and memory facts for one Hands generation.
///
/// Returns exactly two drafts: `compute.millicpu_ms.v1` and `memory.byte_ms.v1`,
/// both over `running_ms` and both `FactBasis::Reserved`. Snapshot storage and
/// any transfer fact are produced separately — snapshot read/write I/O is
/// zero-dollar observability, not a transfer fact, and Hands Internet egress is
/// not charged at launch.
///
/// # Errors
///
/// Returns [`ProbeError::UnexhaustedLifetime`] when the receipt does not account
/// for its own lifetime, and [`ProbeError::Measurement`] when a derived quantity
/// fails its construction rules.
pub fn hands_facts(
    context: &ProbeContext,
    attribution: &Attribution,
    receipt: &RuntimeReceipt,
) -> Result<Vec<FactDraft>, ProbeError> {
    receipt.validate()?;

    // A suspended generation holds no capacity, so the billed interval is the
    // running half only.
    let service_time = ServiceTime::Interval {
        start: receipt.from,
        end: receipt.from.plus_millis(receipt.running_ms)?,
    };
    let baseline = receipt.shape.baseline();

    let mut drafts = Vec::with_capacity(2);
    for (index, (meter, unit)) in [
        (Meter::ComputeMillicpuMs, ShapeUnit::Millicpu),
        (Meter::MemoryByteMs, ShapeUnit::MemoryBytes),
    ]
    .into_iter()
    .enumerate()
    {
        let measurement = Measurement::new(
            meter,
            FactBasis::Reserved,
            service_time,
            SourceReceipt {
                kind: ReceiptKind::ProviderLifecycle,
                id: receipt.receipt_id.clone(),
                digest: None,
            },
            Evidence::ProviderShape {
                receipt_id: receipt.receipt_id.clone(),
                generation: receipt.generation.clone(),
                shape: baseline,
                running_ms: receipt.running_ms,
                unit,
            },
        )?;
        drafts.push(FactDraft {
            schema_version: SCHEMA_VERSION,
            organization: context.organization.clone(),
            workspace: context.workspace.clone(),
            region: context.region.clone(),
            attribution: attribution.clone(),
            service: context.service.clone(),
            resource: context.resource.clone(),
            authority: AuthorityKey {
                region: context.region.clone(),
                category: Category::Compute,
                kind: AuthorityKind::HandsGeneration,
                authority_id: AuthorityId::parse(&receipt.generation)?,
                // The two meters share one authority, so they are distinguished
                // by ordinal rather than by colliding on one identity.
                segment_ordinal: SegmentOrdinal::new(index as u64),
            },
            pricing_version: context.pricing_version.clone(),
            reservation: context.reservation.clone(),
            kind: FactKind::Measured(measurement),
        });
    }
    Ok(drafts)
}

/// One Lambda `REPORT` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LambdaReport {
    /// The Lambda request identifier.
    pub request_id: Box<str>,
    /// The function's configured memory in MB.
    pub memory_mb: u32,
    /// Billed milliseconds from the report.
    pub billed_ms: u64,
}

/// Builds the compute and memory facts for one Lambda invocation.
///
/// `quantity_compute = memory_mb * 1000 * billed_ms / 1769` (floor, in `u128`)
/// and `quantity_memory = memory_mb * 1_048_576 * billed_ms`. The convention is
/// public and reconciled against actual Lambda spend; changing it is a new
/// pricing version, never a silent recompute.
///
/// # Errors
///
/// Returns [`ProbeError::Measurement`] when a derived quantity fails its
/// construction rules.
pub fn lambda_facts(
    context: &ProbeContext,
    attribution: &Attribution,
    report: &LambdaReport,
    at: Timestamp,
    convention: LambdaVcpuConvention,
) -> Result<[FactDraft; 2], ProbeError> {
    let service_time = ServiceTime::Interval {
        start: at,
        end: at.plus_millis(report.billed_ms)?,
    };

    let build = |meter: Meter, ordinal: u64| -> Result<FactDraft, ProbeError> {
        let measurement = Measurement::new(
            meter,
            FactBasis::Reserved,
            service_time,
            SourceReceipt {
                kind: ReceiptKind::LambdaReport,
                id: report.request_id.clone(),
                digest: None,
            },
            Evidence::LambdaReport {
                request_id: report.request_id.clone(),
                memory_mb: report.memory_mb,
                billed_ms: report.billed_ms,
                convention,
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
                category: Category::Compute,
                kind: AuthorityKind::LambdaInvocation,
                authority_id: AuthorityId::parse(&report.request_id)?,
                segment_ordinal: SegmentOrdinal::new(ordinal),
            },
            pricing_version: context.pricing_version.clone(),
            reservation: context.reservation.clone(),
            kind: FactKind::Measured(measurement),
        })
    };

    Ok([
        build(Meter::ComputeMillicpuMs, 0)?,
        build(Meter::MemoryByteMs, 1)?,
    ])
}

/// The baseline capacity a public Hands token allocates.
#[must_use]
pub const fn hands_baseline(shape: HandsShape) -> ComputeShape {
    shape.baseline()
}

#[cfg(test)]
mod tests {
    use super::{LambdaReport, RuntimeReceipt, hands_facts, lambda_facts};
    use crate::probe::ProbeError;
    use crate::probe::testing::{at, probe_context};
    use aex_usage_domain::fact::Attribution;
    use aex_usage_domain::measurement::LambdaVcpuConvention;
    use aex_usage_domain::meter::Meter;
    use aex_usage_domain::shape::HandsShape;

    const GIB: u128 = 1024 * 1024 * 1024;

    fn receipt(running_ms: u64, suspended_ms: u64, lifetime_ms: i64) -> RuntimeReceipt {
        RuntimeReceipt {
            receipt_id: Box::from("rcpt-1"),
            generation: Box::from("gen-1"),
            shape: HandsShape::Gb2,
            from: at(0),
            to: at(lifetime_ms),
            running_ms,
            suspended_ms,
            transmit_bytes: None,
        }
    }

    #[test]
    fn a_generation_bills_its_running_half_at_the_baseline_shape() {
        // 2gb: baseline 1 vCPU (1000 millicpu) and 2 GiB.
        let drafts = hands_facts(
            &probe_context(),
            &Attribution::default(),
            &receipt(30_000, 0, 30_000),
        )
        .expect("valid receipt");

        assert_eq!(drafts.len(), 2);
        let compute = drafts[0].kind.measurement().expect("measured");
        let memory = drafts[1].kind.measurement().expect("measured");
        assert_eq!(compute.meter(), Meter::ComputeMillicpuMs);
        assert_eq!(compute.quantity().get(), 1_000 * 30_000);
        assert_eq!(memory.meter(), Meter::MemoryByteMs);
        assert_eq!(memory.quantity().get(), 2 * GIB * 30_000);
    }

    #[test]
    fn suspended_time_bills_nothing_on_either_meter() {
        // 10 s running, 50 s suspended, 60 s of lifetime.
        let drafts = hands_facts(
            &probe_context(),
            &Attribution::default(),
            &receipt(10_000, 50_000, 60_000),
        )
        .expect("valid receipt");

        let compute = drafts[0].kind.measurement().expect("measured");
        let memory = drafts[1].kind.measurement().expect("measured");
        assert_eq!(
            compute.quantity().get(),
            1_000 * 10_000,
            "a suspended MicroVM holds no CPU"
        );
        assert_eq!(
            memory.quantity().get(),
            2 * GIB * 10_000,
            "a suspended MicroVM holds no memory"
        );
    }

    #[test]
    fn a_fully_suspended_generation_bills_zero_rather_than_its_lifetime() {
        let drafts = hands_facts(
            &probe_context(),
            &Attribution::default(),
            &receipt(0, 60_000, 60_000),
        )
        .expect("valid receipt");
        assert_eq!(
            drafts[0]
                .kind
                .measurement()
                .expect("measured")
                .quantity()
                .get(),
            0
        );
        assert_eq!(
            drafts[1]
                .kind
                .measurement()
                .expect("measured")
                .quantity()
                .get(),
            0
        );
    }

    #[test]
    fn an_unexhausted_lifetime_is_a_hard_error_not_a_guess() {
        // 40 s accounted for out of a 60 s lifetime: 20 s is unexplained.
        let refused = hands_facts(
            &probe_context(),
            &Attribution::default(),
            &receipt(30_000, 10_000, 60_000),
        );
        assert!(matches!(
            refused,
            Err(ProbeError::UnexhaustedLifetime {
                lifetime_ms: 60_000,
                accounted_ms: 40_000,
                ..
            })
        ));

        // Over-accounting is refused just as firmly.
        assert!(
            hands_facts(
                &probe_context(),
                &Attribution::default(),
                &receipt(50_000, 50_000, 60_000),
            )
            .is_err()
        );
    }

    #[test]
    fn the_two_hands_meters_get_distinct_identities() {
        let drafts = hands_facts(
            &probe_context(),
            &Attribution::default(),
            &receipt(1_000, 0, 1_000),
        )
        .expect("valid receipt");
        assert_ne!(
            drafts[0].fact_id(),
            drafts[1].fact_id(),
            "compute and memory are two facts, not one"
        );
    }

    #[test]
    fn every_public_shape_token_produces_its_own_baseline_quantity() {
        let expected = [
            (HandsShape::Mb512, 250u128, GIB / 2),
            (HandsShape::Gb1, 500, GIB),
            (HandsShape::Gb2, 1_000, 2 * GIB),
            (HandsShape::Gb4, 2_000, 4 * GIB),
            (HandsShape::Gb8, 4_000, 8 * GIB),
        ];
        assert_eq!(expected.len(), HandsShape::ALL.len(), "the arity is five");

        for (shape, millicpu, memory_bytes) in expected {
            let mut base = receipt(1_000, 0, 1_000);
            base.shape = shape;
            let drafts =
                hands_facts(&probe_context(), &Attribution::default(), &base).expect("valid");
            assert_eq!(
                drafts[0]
                    .kind
                    .measurement()
                    .expect("measured")
                    .quantity()
                    .get(),
                millicpu * 1_000,
                "{shape} compute"
            );
            assert_eq!(
                drafts[1]
                    .kind
                    .measurement()
                    .expect("measured")
                    .quantity()
                    .get(),
                memory_bytes * 1_000,
                "{shape} memory"
            );
        }
    }

    #[test]
    fn the_lambda_convention_floors_in_u128_at_the_pinned_ratio() {
        // 1769 MB is exactly one vCPU under the published convention.
        let drafts = lambda_facts(
            &probe_context(),
            &Attribution::default(),
            &LambdaReport {
                request_id: Box::from("req-1"),
                memory_mb: 1_769,
                billed_ms: 1_000,
            },
            at(0),
            LambdaVcpuConvention::LinearAt1769Mb { version: 1 },
        )
        .expect("valid report");

        let compute = drafts[0].kind.measurement().expect("measured");
        let memory = drafts[1].kind.measurement().expect("measured");
        assert_eq!(compute.quantity().get(), 1_000 * 1_000, "exactly one vCPU");
        assert_eq!(memory.quantity().get(), 1_769 * 1_048_576 * 1_000);
    }

    #[test]
    fn the_lambda_golden_vectors_hold_across_the_memory_range() {
        // quantity = memory_mb * 1000 * billed_ms / 1769, floored.
        let expected = [
            (128u32, 100u64, 128u128 * 1_000 * 100 / 1_769),
            (512, 250, 512u128 * 1_000 * 250 / 1_769),
            (1_024, 1_000, 1_024u128 * 1_000 * 1_000 / 1_769),
            (1_769, 1, 1_769u128 * 1_000 / 1_769),
            (3_538, 500, 3_538u128 * 1_000 * 500 / 1_769),
            (10_240, 900_000, 10_240u128 * 1_000 * 900_000 / 1_769),
        ];
        for (memory_mb, billed_ms, compute_quantity) in expected {
            let drafts = lambda_facts(
                &probe_context(),
                &Attribution::default(),
                &LambdaReport {
                    request_id: Box::from("req-1"),
                    memory_mb,
                    billed_ms,
                },
                at(0),
                LambdaVcpuConvention::LinearAt1769Mb { version: 1 },
            )
            .expect("valid report");
            assert_eq!(
                drafts[0]
                    .kind
                    .measurement()
                    .expect("measured")
                    .quantity()
                    .get(),
                compute_quantity,
                "{memory_mb} MB for {billed_ms} ms"
            );
            assert_eq!(
                drafts[1]
                    .kind
                    .measurement()
                    .expect("measured")
                    .quantity()
                    .get(),
                u128::from(memory_mb) * 1_048_576 * u128::from(billed_ms),
                "{memory_mb} MB memory"
            );
        }
    }
}
