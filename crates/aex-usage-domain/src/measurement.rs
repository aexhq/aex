//! Measurements, and the evidence that derives every quantity.
//!
//! [`Measurement`] has exactly one constructor and that constructor recomputes
//! the quantity from the evidence. A fact therefore cannot claim a number its
//! evidence does not produce, and there is no constructor anywhere that turns a
//! guest-reported counter, a log line, a trace or a mutable summary row into a
//! measurement.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::intent::Blake3Digest;
use crate::interval::storage::{StorageClose, StorageOwner, StorageSource};
use crate::meter::{Meter, UnknownMeter};
use crate::quantity::{Quantity, QuantityError};
use crate::shape::{ComputeShape, ShapeUnit};
use crate::wire_pending::{ActorRef, CaseId, Timestamp};

/// Whether a quantity is measured use or a held allocation.
///
/// Both are billed at the same rate; the basis records provenance so a
/// reconciler can separate allocated from measured against a physical bill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactBasis {
    /// Measured actual use: thread CPU microseconds, counted egress bytes,
    /// resident storage bytes.
    Consumed,
    /// An allocation the customer holds regardless of utilisation: a Hands
    /// shape over its running interval, a Lambda's configured memory, an
    /// explicit mux memory reservation.
    Reserved,
}

impl FactBasis {
    /// Every basis, in a stable order.
    pub const ALL: [Self; 2] = [Self::Consumed, Self::Reserved];

    /// The stable identifier written to a row.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Consumed => "consumed",
            Self::Reserved => "reserved",
        }
    }
}

impl fmt::Display for FactBasis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

impl FromStr for FactBasis {
    type Err = UnknownMeter;

    fn from_str(value: &str) -> Result<Self, UnknownMeter> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.id() == value)
            .ok_or_else(|| UnknownMeter {
                kind: "fact basis",
                value: value.to_owned(),
            })
    }
}

/// When the measured service happened, in the producer's own clock.
///
/// Ordering and idempotency never depend on this value: the authority stamps
/// `accepted_at` and assigns `accepted_sequence` itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServiceTime {
    /// A single instant.
    Instant {
        /// When the crossing happened.
        at: Timestamp,
    },
    /// A half-open `[start, end)` interval.
    Interval {
        /// Inclusive start.
        start: Timestamp,
        /// Exclusive end.
        end: Timestamp,
    },
}

impl ServiceTime {
    /// The instant the service is considered to have ended.
    #[must_use]
    pub const fn end(self) -> Timestamp {
        match self {
            Self::Instant { at } | Self::Interval { end: at, .. } => at,
        }
    }

    /// The instant the service is considered to have started.
    #[must_use]
    pub const fn start(self) -> Timestamp {
        match self {
            Self::Instant { at } | Self::Interval { start: at, .. } => at,
        }
    }

    /// The interval's length in milliseconds; an instant has length zero.
    ///
    /// # Errors
    ///
    /// Returns [`MeasurementError::InvertedInterval`] when the end precedes the
    /// start.
    pub const fn duration_ms(self) -> Result<u64, MeasurementError> {
        match self {
            Self::Instant { .. } => Ok(0),
            Self::Interval { start, end } => match start.millis_until(end) {
                Some(delta) => Ok(delta),
                None => Err(MeasurementError::InvertedInterval),
            },
        }
    }
}

/// What kind of receipt backs a measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptKind {
    /// A provider lifecycle record for one Hands generation.
    ProviderLifecycle,
    /// A Lambda `REPORT` line.
    LambdaReport,
    /// A closed cgroup reconciliation interval.
    CgroupInterval,
    /// A held memory reservation token.
    ReservationToken,
    /// A counted physical boundary crossing.
    BoundaryCounter,
    /// A provider or CDN delivery log line.
    DeliveryLog,
    /// A durable storage commit.
    StorageCommit,
    /// A correction case record.
    CorrectionCase,
}

impl ReceiptKind {
    /// Every receipt kind, in a stable order.
    pub const ALL: [Self; 8] = [
        Self::ProviderLifecycle,
        Self::LambdaReport,
        Self::CgroupInterval,
        Self::ReservationToken,
        Self::BoundaryCounter,
        Self::DeliveryLog,
        Self::StorageCommit,
        Self::CorrectionCase,
    ];

    /// The stable identifier written to a row.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::ProviderLifecycle => "provider_lifecycle",
            Self::LambdaReport => "lambda_report",
            Self::CgroupInterval => "cgroup_interval",
            Self::ReservationToken => "reservation_token",
            Self::BoundaryCounter => "boundary_counter",
            Self::DeliveryLog => "delivery_log",
            Self::StorageCommit => "storage_commit",
            Self::CorrectionCase => "correction_case",
        }
    }
}

impl fmt::Display for ReceiptKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

impl FromStr for ReceiptKind {
    type Err = UnknownMeter;

    fn from_str(value: &str) -> Result<Self, UnknownMeter> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.id() == value)
            .ok_or_else(|| UnknownMeter {
                kind: "receipt kind",
                value: value.to_owned(),
            })
    }
}

/// The external record a measurement can be reconciled against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceReceipt {
    /// What kind of record it is.
    pub kind: ReceiptKind,
    /// The record's own identifier.
    pub id: Box<str>,
    /// An optional content digest of the record as observed.
    pub digest: Option<Blake3Digest>,
}

/// What a memory reservation is held for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReservationClass {
    /// Assembled agent context.
    Context,
    /// The canonical provider request body.
    CanonicalRequest,
    /// Streaming response parser buffers.
    ParserBuffer,
    /// Preview frame buffers.
    PreviewBuffer,
    /// A retained tool or model result.
    Result,
    /// A warm cache entry held between activations.
    WarmCacheEntry,
}

impl ReservationClass {
    /// Every reservation class, in a stable order.
    pub const ALL: [Self; 6] = [
        Self::Context,
        Self::CanonicalRequest,
        Self::ParserBuffer,
        Self::PreviewBuffer,
        Self::Result,
        Self::WarmCacheEntry,
    ];

    /// The stable identifier written to a row.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Context => "context",
            Self::CanonicalRequest => "canonical_request",
            Self::ParserBuffer => "parser_buffer",
            Self::PreviewBuffer => "preview_buffer",
            Self::Result => "result",
            Self::WarmCacheEntry => "warm_cache_entry",
        }
    }
}

impl fmt::Display for ReservationClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

/// One billed public egress boundary.
///
/// The set is closed: a boundary that is not listed cannot be counted, so an
/// adapter cannot invent a billing surface.
/// `Serialize`/`Deserialize` are hand-written rather than derived: the inner
/// `&'static str` makes serde's borrow analysis add a `'de: 'static` bound to
/// every enclosing type, which would make [`Evidence`] undeserializable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BoundaryId(&'static str);

impl Serialize for BoundaryId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

impl<'de> Deserialize<'de> for BoundaryId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = <std::borrow::Cow<'de, str> as Deserialize<'de>>::deserialize(deserializer)?;
        Self::from_str(&raw).map_err(serde::de::Error::custom)
    }
}

impl BoundaryId {
    /// Regional HTTP responses.
    pub const REGIONAL_HTTP: Self = Self("regional_http");
    /// Regional stream frames.
    pub const REGIONAL_STREAM: Self = Self("regional_stream");
    /// Content download delivery.
    pub const CONTENT_DOWNLOAD: Self = Self("content_download");
    /// Outbound provider HTTP from the mux.
    pub const PROVIDER_HTTP: Self = Self("provider_http");
    /// The Hands metered public egress boundary.
    pub const HANDS_EGRESS: Self = Self("hands_egress");
    /// Authenticated delivery through the hosting edge.
    pub const VERCEL_AUTHENTICATED: Self = Self("vercel_authenticated");

    /// Every boundary, in a stable order.
    pub const ALL: [Self; 6] = [
        Self::REGIONAL_HTTP,
        Self::REGIONAL_STREAM,
        Self::CONTENT_DOWNLOAD,
        Self::PROVIDER_HTTP,
        Self::HANDS_EGRESS,
        Self::VERCEL_AUTHENTICATED,
    ];

    /// The stable identifier written to a row and an authority key.
    #[must_use]
    pub const fn id(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for BoundaryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl FromStr for BoundaryId {
    type Err = UnknownMeter;

    fn from_str(value: &str) -> Result<Self, UnknownMeter> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.id() == value)
            .ok_or_else(|| UnknownMeter {
                kind: "egress boundary",
                value: value.to_owned(),
            })
    }
}

impl TryFrom<String> for BoundaryId {
    type Error = UnknownMeter;

    fn try_from(value: String) -> Result<Self, UnknownMeter> {
        Self::from_str(&value)
    }
}

impl From<BoundaryId> for String {
    fn from(value: BoundaryId) -> Self {
        value.0.to_owned()
    }
}

/// A counter epoch: one process generation of one boundary.
///
/// A restart opens a new crossing space, so a restarted adapter can never
/// reuse a crossing sequence a previous process already claimed.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct CounterEpoch(u64);

impl CounterEpoch {
    /// Builds a counter epoch.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// The underlying generation number.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for CounterEpoch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The public convention that turns a Lambda's configured memory into vCPU.
///
/// The convention is public and reconciled against actual Lambda spend.
/// Changing it is a new pricing version, never a silent recompute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LambdaVcpuConvention {
    /// vCPU scales linearly with configured memory, reaching one vCPU at
    /// 1769 MB.
    LinearAt1769Mb {
        /// The convention version pinned on the fact.
        version: u16,
    },
}

impl LambdaVcpuConvention {
    /// Millicpu granted to a function of `memory_mb` under this convention.
    #[must_use]
    pub const fn millicpu(self, memory_mb: u32) -> u64 {
        match self {
            Self::LinearAt1769Mb { .. } => memory_mb as u64 * 1_000 / 1_769,
        }
    }
}

/// Derives a reconciled CPU quantity, enforcing the physical upper bound.
///
/// Three bounds, all load-bearing: a charge never exceeds what the cgroup
/// physically consumed, never exceeds what the meter attributed to itself, and
/// equals the attribution exactly whenever no proportional scaling was applied.
fn derive_cgroup(
    physical_us: u64,
    attributed_us: u64,
    charged_us: u64,
    scaled: bool,
) -> Result<Quantity, MeasurementError> {
    if charged_us > physical_us {
        return Err(MeasurementError::ChargedAboveBound {
            charged_us,
            bound_us: physical_us,
            bound: "physical",
        });
    }
    if charged_us > attributed_us {
        return Err(MeasurementError::ChargedAboveBound {
            charged_us,
            bound_us: attributed_us,
            bound: "attributed",
        });
    }
    if !scaled && charged_us != attributed_us {
        return Err(MeasurementError::UnscaledMismatch {
            charged_us,
            attributed_us,
        });
    }
    Ok(Quantity::new(u128::from(charged_us))?)
}

/// The physical record a measurement is derived from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Evidence {
    /// A closed cgroup reconciliation interval for one attributed meter.
    CgroupReconciled {
        /// The cgroup's own `usage_usec` delta over the interval.
        physical_us: u64,
        /// Microseconds this meter attributed to itself.
        attributed_us: u64,
        /// Microseconds actually charged after proportional scaling.
        charged_us: u64,
        /// Whether proportional scaling was applied.
        scaled: bool,
        /// The interval's length in milliseconds.
        interval_ms: u64,
    },
    /// A held memory reservation.
    Reservation {
        /// What the reservation was held for.
        class: ReservationClass,
        /// Bytes reserved.
        bytes: u64,
        /// Milliseconds the reservation was held.
        held_ms: u64,
    },
    /// A provider-allocated shape over its running interval.
    ProviderShape {
        /// The provider lifecycle receipt identifier.
        receipt_id: Box<str>,
        /// The generation the shape was allocated to.
        generation: Box<str>,
        /// The allocated baseline capacity.
        shape: ComputeShape,
        /// Milliseconds the generation was running, excluding suspension.
        running_ms: u64,
        /// Which capacity of the shape this measurement bills.
        unit: ShapeUnit,
    },
    /// A Lambda `REPORT` line.
    LambdaReport {
        /// The Lambda request identifier.
        request_id: Box<str>,
        /// The function's configured memory in MB.
        memory_mb: u32,
        /// Billed milliseconds from the report.
        billed_ms: u64,
        /// The public vCPU convention applied.
        convention: LambdaVcpuConvention,
    },
    /// Retained bytes over whole minutes.
    StorageResidence {
        /// What holds the bytes.
        owner: StorageOwner,
        /// Which durable store holds them.
        source: StorageSource,
        /// Bytes retained across the interval.
        bytes: u64,
        /// Whole minutes of residence.
        minutes: u64,
        /// The durable commit that closed the interval.
        commit_id: Box<str>,
        /// Whether the close floored an interior transition or ceiled a
        /// terminal one.
        close: StorageClose,
    },
    /// One physical public-boundary crossing counted by an AEX adapter.
    BoundaryCounter {
        /// Which billed boundary.
        boundary: BoundaryId,
        /// The counting process generation.
        epoch: CounterEpoch,
        /// Counter value when the crossing opened.
        start: u64,
        /// Counter value when the crossing closed.
        end: u64,
    },
    /// A provider-authoritative delivery record.
    DeliveryReceipt {
        /// Which billed boundary.
        boundary: BoundaryId,
        /// The delivery record identifier.
        receipt_id: Box<str>,
        /// Bytes the provider reports delivering.
        bytes: u64,
    },
    /// A correction restating an earlier measurement.
    CorrectionCase {
        /// The case that authorised the restatement.
        case_id: CaseId,
        /// Who raised it.
        actor: ActorRef,
        /// The evidence that produces the restated quantity. Never itself a
        /// correction case: a restatement chain has depth one.
        restated: Box<Evidence>,
    },
}

/// Why a measurement was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MeasurementError {
    /// The evidence variant cannot produce the requested meter's base unit.
    #[error("evidence `{evidence}` cannot derive meter `{meter}`")]
    MeterMismatch {
        /// The evidence variant that was offered.
        evidence: &'static str,
        /// The meter that was requested.
        meter: &'static str,
    },
    /// A half-open interval ended before it started.
    #[error("service interval ends before it starts")]
    InvertedInterval,
    /// An interval meter was given an instant.
    #[error("meter `{meter}` requires a half-open interval, not an instant")]
    InstantUnderIntervalMeter {
        /// The meter that was requested.
        meter: &'static str,
    },
    /// The evidence's own interval disagrees with the declared service time.
    #[error(
        "evidence interval {evidence_ms} ms disagrees with the {service_ms} ms service interval"
    )]
    IntervalDisagreement {
        /// Milliseconds the evidence claims.
        evidence_ms: u64,
        /// Milliseconds the service time declares.
        service_ms: u64,
    },
    /// More CPU was charged than the cgroup physically consumed, or more than
    /// this meter attributed to itself.
    #[error("charged {charged_us} us exceeds the {bound_us} us {bound} bound")]
    ChargedAboveBound {
        /// Microseconds charged.
        charged_us: u64,
        /// The bound that was exceeded.
        bound_us: u64,
        /// Which bound it was.
        bound: &'static str,
    },
    /// `scaled = false` was declared while charged and attributed differ.
    #[error("unscaled interval charged {charged_us} us against {attributed_us} us attributed")]
    UnscaledMismatch {
        /// Microseconds charged.
        charged_us: u64,
        /// Microseconds attributed.
        attributed_us: u64,
    },
    /// A boundary counter went backwards.
    #[error("boundary counter closed at {end} below its {start} open")]
    CounterRegressed {
        /// Counter value at open.
        start: u64,
        /// Counter value at close.
        end: u64,
    },
    /// A correction case was nested inside another correction case.
    #[error("a correction case cannot restate another correction case")]
    NestedCorrection,
    /// The basis contradicts what the evidence describes.
    #[error("evidence `{evidence}` is always basis `{expected}`, not `{actual}`")]
    BasisMismatch {
        /// The evidence variant that was offered.
        evidence: &'static str,
        /// The basis the evidence implies.
        expected: &'static str,
        /// The basis that was declared.
        actual: &'static str,
    },
    /// The derived quantity did not fit.
    #[error(transparent)]
    Quantity(#[from] QuantityError),
}

impl Evidence {
    /// The stable variant name, used in error messages and on the row.
    #[must_use]
    pub const fn variant(&self) -> &'static str {
        match self {
            Self::CgroupReconciled { .. } => "cgroup_reconciled",
            Self::Reservation { .. } => "reservation",
            Self::ProviderShape { .. } => "provider_shape",
            Self::LambdaReport { .. } => "lambda_report",
            Self::StorageResidence { .. } => "storage_residence",
            Self::BoundaryCounter { .. } => "boundary_counter",
            Self::DeliveryReceipt { .. } => "delivery_receipt",
            Self::CorrectionCase { .. } => "correction_case",
        }
    }

    /// The basis this evidence always implies.
    #[must_use]
    pub const fn basis(&self) -> FactBasis {
        match self {
            Self::CgroupReconciled { .. }
            | Self::StorageResidence { .. }
            | Self::BoundaryCounter { .. }
            | Self::DeliveryReceipt { .. } => FactBasis::Consumed,
            Self::Reservation { .. } | Self::ProviderShape { .. } | Self::LambdaReport { .. } => {
                FactBasis::Reserved
            }
            Self::CorrectionCase { restated, .. } => restated.basis(),
        }
    }

    /// Recomputes the quantity this evidence produces for `meter`.
    ///
    /// # Errors
    ///
    /// Returns [`MeasurementError`] when the evidence cannot produce the
    /// meter's base unit, when an internal bound is violated, or when the
    /// product does not fit a [`Quantity`].
    pub fn derive(&self, meter: Meter) -> Result<Quantity, MeasurementError> {
        let mismatch = || MeasurementError::MeterMismatch {
            evidence: self.variant(),
            meter: meter.id(),
        };
        match self {
            Self::CgroupReconciled {
                physical_us,
                attributed_us,
                charged_us,
                scaled,
                ..
            } => {
                if meter != Meter::ComputeMillicpuMs {
                    return Err(mismatch());
                }
                derive_cgroup(*physical_us, *attributed_us, *charged_us, *scaled)
            }
            Self::Reservation { bytes, held_ms, .. } => {
                if meter != Meter::MemoryByteMs {
                    return Err(mismatch());
                }
                Ok(Quantity::product(*bytes, *held_ms)?)
            }
            Self::ProviderShape {
                shape,
                running_ms,
                unit,
                ..
            } => match (meter, unit) {
                (Meter::ComputeMillicpuMs, ShapeUnit::Millicpu) => {
                    Ok(Quantity::product(u64::from(shape.millicpu), *running_ms)?)
                }
                (Meter::MemoryByteMs, ShapeUnit::MemoryBytes) => {
                    Ok(Quantity::product(shape.memory_bytes, *running_ms)?)
                }
                _ => Err(mismatch()),
            },
            Self::LambdaReport {
                memory_mb,
                billed_ms,
                convention,
                ..
            } => match meter {
                Meter::ComputeMillicpuMs => Ok(Quantity::product(
                    convention.millicpu(*memory_mb),
                    *billed_ms,
                )?),
                Meter::MemoryByteMs => Ok(Quantity::product(
                    u64::from(*memory_mb) * 1_048_576,
                    *billed_ms,
                )?),
                _ => Err(mismatch()),
            },
            Self::StorageResidence { bytes, minutes, .. } => {
                if meter != Meter::StorageByteMin {
                    return Err(mismatch());
                }
                Ok(Quantity::product(*bytes, *minutes)?)
            }
            Self::BoundaryCounter { start, end, .. } => {
                if meter != Meter::DataTransferEgressByte {
                    return Err(mismatch());
                }
                let counted =
                    end.checked_sub(*start)
                        .ok_or(MeasurementError::CounterRegressed {
                            start: *start,
                            end: *end,
                        })?;
                Ok(Quantity::new(u128::from(counted))?)
            }
            Self::DeliveryReceipt { bytes, .. } => {
                if meter != Meter::DataTransferEgressByte {
                    return Err(mismatch());
                }
                Ok(Quantity::new(u128::from(*bytes))?)
            }
            Self::CorrectionCase { restated, .. } => {
                if matches!(**restated, Self::CorrectionCase { .. }) {
                    return Err(MeasurementError::NestedCorrection);
                }
                restated.derive(meter)
            }
        }
    }

    /// The evidence's own interval length in milliseconds, when it carries one.
    #[must_use]
    pub const fn evidence_interval_ms(&self) -> Option<u64> {
        match self {
            Self::CgroupReconciled { interval_ms, .. } => Some(*interval_ms),
            Self::Reservation { held_ms, .. } => Some(*held_ms),
            Self::StorageResidence { minutes, .. } => Some(*minutes * 60_000),
            Self::ProviderShape { .. }
            | Self::LambdaReport { .. }
            | Self::BoundaryCounter { .. }
            | Self::DeliveryReceipt { .. }
            | Self::CorrectionCase { .. } => None,
        }
    }

    /// The longest interval this evidence may occupy, when it declares one.
    ///
    /// A Hands generation may be suspended and a Lambda may be billed for less
    /// than the wall interval, so these bound the service time rather than
    /// matching it.
    #[must_use]
    pub const fn evidence_bounded_ms(&self) -> Option<u64> {
        match self {
            Self::ProviderShape { running_ms, .. } => Some(*running_ms),
            Self::LambdaReport { billed_ms, .. } => Some(*billed_ms),
            _ => None,
        }
    }
}

/// One immutable, evidence-derived measurement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Measurement {
    meter: Meter,
    basis: FactBasis,
    quantity: Quantity,
    service_time: ServiceTime,
    source_receipt: SourceReceipt,
    evidence: Evidence,
}

impl Measurement {
    /// The only constructor. It recomputes the quantity from the evidence.
    ///
    /// # Errors
    ///
    /// Returns [`MeasurementError`] when the evidence cannot derive the meter,
    /// when the basis contradicts the evidence, when the service time is
    /// inverted or is an instant under an interval meter, when the evidence's
    /// own interval disagrees with the service time, or when any bound or
    /// overflow check fails.
    pub fn new(
        meter: Meter,
        basis: FactBasis,
        service_time: ServiceTime,
        source_receipt: SourceReceipt,
        evidence: Evidence,
    ) -> Result<Self, MeasurementError> {
        if meter.requires_interval() && matches!(service_time, ServiceTime::Instant { .. }) {
            return Err(MeasurementError::InstantUnderIntervalMeter { meter: meter.id() });
        }
        let service_ms = service_time.duration_ms()?;
        if evidence.basis() != basis {
            return Err(MeasurementError::BasisMismatch {
                evidence: evidence.variant(),
                expected: evidence.basis().id(),
                actual: basis.id(),
            });
        }
        if let Some(evidence_ms) = evidence.evidence_interval_ms()
            && evidence_ms != service_ms
        {
            return Err(MeasurementError::IntervalDisagreement {
                evidence_ms,
                service_ms,
            });
        }
        if let Some(bounded_ms) = evidence.evidence_bounded_ms()
            && bounded_ms > service_ms
        {
            return Err(MeasurementError::IntervalDisagreement {
                evidence_ms: bounded_ms,
                service_ms,
            });
        }
        let quantity = evidence.derive(meter)?;
        Ok(Self {
            meter,
            basis,
            quantity,
            service_time,
            source_receipt,
            evidence,
        })
    }

    /// The meter this measurement counts.
    #[must_use]
    pub const fn meter(&self) -> Meter {
        self.meter
    }

    /// Whether this is measured use or a held allocation.
    #[must_use]
    pub const fn basis(&self) -> FactBasis {
        self.basis
    }

    /// The evidence-derived quantity.
    #[must_use]
    pub const fn quantity(&self) -> Quantity {
        self.quantity
    }

    /// Whether the derived quantity is exactly zero.
    #[must_use]
    pub const fn is_zero(&self) -> bool {
        self.quantity.is_zero()
    }

    /// When the measured service happened.
    #[must_use]
    pub const fn service_time(&self) -> ServiceTime {
        self.service_time
    }

    /// The external record this measurement reconciles against.
    #[must_use]
    pub const fn source_receipt(&self) -> &SourceReceipt {
        &self.source_receipt
    }

    /// The physical record the quantity was derived from.
    #[must_use]
    pub const fn evidence(&self) -> &Evidence {
        &self.evidence
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BoundaryId, CounterEpoch, Evidence, FactBasis, LambdaVcpuConvention, Measurement,
        MeasurementError, ReceiptKind, ReservationClass, ServiceTime, SourceReceipt,
    };
    use crate::meter::Meter;
    use crate::quantity::Quantity;
    use crate::shape::{HandsShape, ShapeUnit};
    use crate::wire_pending::Timestamp;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("representable")
    }

    fn interval(start: i64, end: i64) -> ServiceTime {
        ServiceTime::Interval {
            start: at(start),
            end: at(end),
        }
    }

    fn receipt(kind: ReceiptKind) -> SourceReceipt {
        SourceReceipt {
            kind,
            id: Box::from("r1"),
            digest: None,
        }
    }

    #[test]
    fn cgroup_evidence_charges_only_what_it_measured() {
        let evidence = Evidence::CgroupReconciled {
            physical_us: 1_000,
            attributed_us: 900,
            charged_us: 900,
            scaled: false,
            interval_ms: 10_000,
        };
        let measurement = Measurement::new(
            Meter::ComputeMillicpuMs,
            FactBasis::Consumed,
            interval(0, 10_000),
            receipt(ReceiptKind::CgroupInterval),
            evidence,
        )
        .expect("valid cgroup interval");
        assert_eq!(
            measurement.quantity(),
            Quantity::new(900).expect("representable")
        );
    }

    #[test]
    fn charging_above_the_physical_bound_is_refused() {
        let evidence = Evidence::CgroupReconciled {
            physical_us: 100,
            attributed_us: 900,
            charged_us: 900,
            scaled: true,
            interval_ms: 10_000,
        };
        assert!(matches!(
            evidence.derive(Meter::ComputeMillicpuMs),
            Err(MeasurementError::ChargedAboveBound {
                bound: "physical",
                ..
            })
        ));
    }

    #[test]
    fn an_unscaled_interval_must_charge_exactly_what_it_attributed() {
        let evidence = Evidence::CgroupReconciled {
            physical_us: 1_000,
            attributed_us: 900,
            charged_us: 500,
            scaled: false,
            interval_ms: 10_000,
        };
        assert!(matches!(
            evidence.derive(Meter::ComputeMillicpuMs),
            Err(MeasurementError::UnscaledMismatch { .. })
        ));
    }

    #[test]
    fn a_reservation_integrates_bytes_over_held_milliseconds() {
        let measurement = Measurement::new(
            Meter::MemoryByteMs,
            FactBasis::Reserved,
            interval(0, 2_000),
            receipt(ReceiptKind::ReservationToken),
            Evidence::Reservation {
                class: ReservationClass::Context,
                bytes: 4_096,
                held_ms: 2_000,
            },
        )
        .expect("valid reservation");
        assert_eq!(
            measurement.quantity(),
            Quantity::new(4_096 * 2_000).expect("representable")
        );
    }

    #[test]
    fn a_reservation_whose_hold_disagrees_with_service_time_is_refused() {
        let error = Measurement::new(
            Meter::MemoryByteMs,
            FactBasis::Reserved,
            interval(0, 2_000),
            receipt(ReceiptKind::ReservationToken),
            Evidence::Reservation {
                class: ReservationClass::Context,
                bytes: 4_096,
                held_ms: 3_000,
            },
        )
        .expect_err("mismatched hold is refused");
        assert!(matches!(
            error,
            MeasurementError::IntervalDisagreement {
                evidence_ms: 3_000,
                service_ms: 2_000
            }
        ));
    }

    #[test]
    fn a_hands_shape_bills_baseline_capacity_over_running_time() {
        let shape = HandsShape::Gb2.baseline();
        let measurement = Measurement::new(
            Meter::ComputeMillicpuMs,
            FactBasis::Reserved,
            interval(0, 60_000),
            receipt(ReceiptKind::ProviderLifecycle),
            Evidence::ProviderShape {
                receipt_id: Box::from("rcpt"),
                generation: Box::from("gen"),
                shape,
                running_ms: 45_000,
                unit: ShapeUnit::Millicpu,
            },
        )
        .expect("valid shape fact");
        assert_eq!(
            measurement.quantity(),
            Quantity::new(1_000 * 45_000).expect("representable")
        );
    }

    #[test]
    fn a_shape_unit_selects_exactly_one_meter() {
        let evidence = Evidence::ProviderShape {
            receipt_id: Box::from("rcpt"),
            generation: Box::from("gen"),
            shape: HandsShape::Gb1.baseline(),
            running_ms: 1_000,
            unit: ShapeUnit::Millicpu,
        };
        assert!(evidence.derive(Meter::MemoryByteMs).is_err());
        assert!(evidence.derive(Meter::StorageByteMin).is_err());
        assert!(evidence.derive(Meter::DataTransferEgressByte).is_err());
    }

    #[test]
    fn the_lambda_convention_floors_in_integer_arithmetic() {
        let convention = LambdaVcpuConvention::LinearAt1769Mb { version: 1 };
        assert_eq!(convention.millicpu(1_769), 1_000);
        assert_eq!(convention.millicpu(128), 72);
        let evidence = Evidence::LambdaReport {
            request_id: Box::from("req"),
            memory_mb: 128,
            billed_ms: 250,
            convention,
        };
        assert_eq!(
            evidence
                .derive(Meter::ComputeMillicpuMs)
                .expect("derives compute"),
            Quantity::new(72 * 250).expect("representable")
        );
        assert_eq!(
            evidence
                .derive(Meter::MemoryByteMs)
                .expect("derives memory"),
            Quantity::new(128 * 1_048_576 * 250).expect("representable")
        );
    }

    #[test]
    fn a_boundary_counter_derives_the_difference_and_refuses_regression() {
        let evidence = Evidence::BoundaryCounter {
            boundary: BoundaryId::REGIONAL_HTTP,
            epoch: CounterEpoch::new(3),
            start: 100,
            end: 480,
        };
        assert_eq!(
            evidence
                .derive(Meter::DataTransferEgressByte)
                .expect("derives"),
            Quantity::new(380).expect("representable")
        );
        let regressed = Evidence::BoundaryCounter {
            boundary: BoundaryId::REGIONAL_HTTP,
            epoch: CounterEpoch::new(3),
            start: 480,
            end: 100,
        };
        assert!(matches!(
            regressed.derive(Meter::DataTransferEgressByte),
            Err(MeasurementError::CounterRegressed { .. })
        ));
    }

    #[test]
    fn egress_is_the_only_meter_that_admits_an_instant() {
        let egress = Measurement::new(
            Meter::DataTransferEgressByte,
            FactBasis::Consumed,
            ServiceTime::Instant { at: at(5_000) },
            receipt(ReceiptKind::DeliveryLog),
            Evidence::DeliveryReceipt {
                boundary: BoundaryId::CONTENT_DOWNLOAD,
                receipt_id: Box::from("d1"),
                bytes: 4_096,
            },
        );
        assert!(egress.is_ok());
        let compute = Measurement::new(
            Meter::ComputeMillicpuMs,
            FactBasis::Consumed,
            ServiceTime::Instant { at: at(5_000) },
            receipt(ReceiptKind::CgroupInterval),
            Evidence::CgroupReconciled {
                physical_us: 1,
                attributed_us: 1,
                charged_us: 1,
                scaled: false,
                interval_ms: 0,
            },
        );
        assert!(matches!(
            compute,
            Err(MeasurementError::InstantUnderIntervalMeter { .. })
        ));
    }

    #[test]
    fn an_inverted_interval_is_refused() {
        let error = Measurement::new(
            Meter::MemoryByteMs,
            FactBasis::Reserved,
            interval(2_000, 1_000),
            receipt(ReceiptKind::ReservationToken),
            Evidence::Reservation {
                class: ReservationClass::Result,
                bytes: 1,
                held_ms: 1,
            },
        )
        .expect_err("inverted interval is refused");
        assert_eq!(error, MeasurementError::InvertedInterval);
    }

    #[test]
    fn the_basis_must_match_what_the_evidence_describes() {
        let error = Measurement::new(
            Meter::MemoryByteMs,
            FactBasis::Consumed,
            interval(0, 1_000),
            receipt(ReceiptKind::ReservationToken),
            Evidence::Reservation {
                class: ReservationClass::Result,
                bytes: 1,
                held_ms: 1_000,
            },
        )
        .expect_err("a reservation is never consumed use");
        assert!(matches!(error, MeasurementError::BasisMismatch { .. }));
    }

    #[test]
    fn a_correction_case_cannot_restate_another_correction_case() {
        let inner = Evidence::CorrectionCase {
            case_id: crate::wire_pending::CaseId::parse("case-1").expect("case"),
            actor: crate::wire_pending::ActorRef::Reconciler {
                service: crate::wire_pending::ServiceId::parse("finance-reconcile")
                    .expect("service"),
            },
            restated: Box::new(Evidence::DeliveryReceipt {
                boundary: BoundaryId::CONTENT_DOWNLOAD,
                receipt_id: Box::from("d1"),
                bytes: 1,
            }),
        };
        let nested = Evidence::CorrectionCase {
            case_id: crate::wire_pending::CaseId::parse("case-2").expect("case"),
            actor: crate::wire_pending::ActorRef::Reconciler {
                service: crate::wire_pending::ServiceId::parse("finance-reconcile")
                    .expect("service"),
            },
            restated: Box::new(inner),
        };
        assert_eq!(
            nested.derive(Meter::DataTransferEgressByte),
            Err(MeasurementError::NestedCorrection)
        );
    }
}
