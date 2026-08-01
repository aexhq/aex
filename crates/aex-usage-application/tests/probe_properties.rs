//! Property tests over the `METER-02` probe.
//!
//! The module tests pin named cases with scripted clocks. These pin the laws
//! that must hold for *any* schedule a real process can produce: awaited time is
//! never billed, the memory envelope is never over-committed, a resized
//! reservation integrates exactly, a crossing produces at most one fact, and a
//! suspended Hands generation is billed nothing.

use aex_usage_application::probe::{
    ActivationKey, ActivationMeter, ActivationScoped, BoundaryReceipt, BoundedFactSink, CpuInstant,
    CpuJob, EgressCounter, FactSink, LambdaReport, MAX_ATTRIBUTED_POLL_US, MemoryBudget,
    OverflowLedger, ProbeContext, ProbeError, RuntimeReceipt, ThreadCpuClock, WallClock,
    egress_from_receipt, hands_facts, lambda_facts,
};
use aex_usage_domain::fact::{
    Attribution, FactDraft, FactKind, ResourceGeneration, ResourceKind, SCHEMA_VERSION,
};
use aex_usage_domain::identity::{AuthorityId, AuthorityKey, AuthorityKind, SegmentOrdinal};
use aex_usage_domain::measurement::{
    BoundaryId, CounterEpoch, Evidence, FactBasis, LambdaVcpuConvention, Measurement, ReceiptKind,
    ReservationClass, ServiceTime, SourceReceipt,
};
use aex_usage_domain::meter::{Category, Meter};
use aex_usage_domain::shape::HandsShape;
use aex_usage_domain::wire_pending::{
    ActivationId, AgentId, OrganizationId, PricingVersion, RegionId, ServiceId, SessionId,
    Timestamp, WorkspaceId,
};
use proptest::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// fixtures
// ---------------------------------------------------------------------------

fn at(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(millis).expect("representable")
}

fn context() -> ProbeContext {
    ProbeContext {
        organization: OrganizationId::parse("org-1").expect("org"),
        workspace: WorkspaceId::parse("ws-1").expect("workspace"),
        region: RegionId::parse("eu-west-1").expect("region"),
        service: ServiceId::parse("brain-mux").expect("service"),
        resource: ResourceGeneration {
            kind: ResourceKind::MuxTask,
            generation: Box::from("task-1"),
        },
        pricing_version: PricingVersion::parse("synthetic-zero-v1").expect("version"),
        reservation: None,
    }
}

fn meter() -> Arc<ActivationMeter> {
    ActivationMeter::new(
        ActivationKey {
            session: SessionId::parse("sess-1").expect("session"),
            agent: AgentId::parse("agent-1").expect("agent"),
            activation: ActivationId::parse("act-1").expect("activation"),
            fence: 1,
        },
        context(),
        Attribution::default(),
    )
}

fn sink(capacity: usize) -> Arc<BoundedFactSink> {
    Arc::new(BoundedFactSink::new(capacity, Arc::new(OverflowLedger::new())).expect("capacity"))
}

fn draft(seed: u64) -> FactDraft {
    let measurement = Measurement::new(
        Meter::DataTransferEgressByte,
        FactBasis::Consumed,
        ServiceTime::Instant { at: at(0) },
        SourceReceipt {
            kind: ReceiptKind::DeliveryLog,
            id: Box::from("fixture"),
            digest: None,
        },
        Evidence::DeliveryReceipt {
            boundary: BoundaryId::CONTENT_DOWNLOAD,
            receipt_id: Box::from("fixture"),
            bytes: seed,
        },
    )
    .expect("a fixture measurement is valid");
    let context = context();
    FactDraft {
        schema_version: SCHEMA_VERSION,
        organization: context.organization.clone(),
        workspace: context.workspace.clone(),
        region: context.region.clone(),
        attribution: Attribution::default(),
        service: context.service.clone(),
        resource: context.resource.clone(),
        authority: AuthorityKey {
            region: context.region,
            category: Category::Transfer,
            kind: AuthorityKind::EgressCrossing,
            authority_id: AuthorityId::parse(&format!("crossing-{seed}")).expect("id"),
            segment_ordinal: SegmentOrdinal::FIRST,
        },
        pricing_version: context.pricing_version,
        reservation: None,
        kind: FactKind::Measured(measurement),
    }
}

/// A thread CPU clock over a scripted reading sequence.
#[derive(Debug)]
struct ScriptedCpuClock {
    readings: Mutex<std::collections::VecDeque<u64>>,
    last: AtomicU64,
}

impl ScriptedCpuClock {
    fn new(readings: impl IntoIterator<Item = u64>) -> Arc<Self> {
        Arc::new(Self {
            readings: Mutex::new(readings.into_iter().collect()),
            last: AtomicU64::new(0),
        })
    }
}

impl ThreadCpuClock for ScriptedCpuClock {
    fn thread_cpu_now(&self) -> CpuInstant {
        let next = self
            .readings
            .lock()
            .expect("lock")
            .pop_front()
            .unwrap_or_else(|| self.last.load(Ordering::Relaxed));
        self.last.store(next, Ordering::Relaxed);
        CpuInstant::from_micros(next)
    }
}

/// A wall clock advancing a fixed step on every `instant()` read.
#[derive(Debug)]
struct SteppingClock {
    base: Instant,
    step_ms: u64,
    elapsed_ms: AtomicU64,
}

impl SteppingClock {
    fn new(step_ms: u64) -> Arc<Self> {
        Arc::new(Self {
            base: Instant::now(),
            step_ms,
            elapsed_ms: AtomicU64::new(0),
        })
    }
}

impl WallClock for SteppingClock {
    fn now(&self) -> time::OffsetDateTime {
        let elapsed = self.elapsed_ms.load(Ordering::Acquire);
        time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(elapsed) * 1_000_000)
            .expect("representable")
    }

    fn instant(&self) -> Instant {
        let elapsed = self.elapsed_ms.fetch_add(self.step_ms, Ordering::AcqRel);
        self.base + Duration::from_millis(elapsed)
    }
}

/// A future pending for `remaining` polls, doing no work of its own.
struct Pending {
    remaining: u32,
}

impl std::future::Future for Pending {
    type Output = ();

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<()> {
        if self.remaining == 0 {
            return std::task::Poll::Ready(());
        }
        self.remaining -= 1;
        context.waker().wake_by_ref();
        std::task::Poll::Pending
    }
}

fn drive<F: std::future::Future>(future: F) -> F::Output {
    let waker = std::task::Waker::noop();
    let mut context = std::task::Context::from_waker(waker);
    let mut future = Box::pin(future);
    loop {
        if let std::task::Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
    }
}

// ---------------------------------------------------------------------------
// U3 — activation CPU attribution
// ---------------------------------------------------------------------------

proptest! {
    /// For any schedule, a future that does no work inside its polls attributes
    /// zero — however long it spends awaiting between them. This is the property
    /// that keeps a customer from being billed for a slow provider.
    #[test]
    fn awaited_time_is_never_attributed(
        polls in 1u32..12,
        away_us in prop::collection::vec(0u64..50_000_000, 1..12),
    ) {
        // Each poll reads the clock twice and both reads return the same value:
        // no work happened inside the poll. Between polls the clock jumps by an
        // arbitrary amount, standing in for a provider round trip or a tool call.
        let mut readings = Vec::new();
        let mut elapsed = 0u64;
        for index in 0..=polls {
            readings.push(elapsed);
            readings.push(elapsed);
            elapsed = elapsed
                .saturating_add(away_us[usize::try_from(index).expect("bounded") % away_us.len()]);
        }

        let meter = meter();
        drive(
            Pending { remaining: polls }
                .metered(Arc::clone(&meter), ScriptedCpuClock::new(readings)),
        );

        prop_assert_eq!(
            meter.attributed_us(),
            0,
            "time spent awaiting is not time spent computing"
        );
        prop_assert_eq!(meter.poll_count(), u64::from(polls) + 1);
    }

    /// Whatever a poll claims, no single poll may attribute more than the cap. A
    /// longer poll is a blocking call on the reactor, which is a defect to
    /// report rather than a quantity to bill.
    #[test]
    fn no_single_poll_attributes_more_than_the_cap(work_us in 0u64..5_000_000) {
        let meter = meter();
        drive(
            Pending { remaining: 0 }
                .metered(Arc::clone(&meter), ScriptedCpuClock::new([0, work_us])),
        );

        prop_assert!(meter.attributed_us() <= MAX_ATTRIBUTED_POLL_US);
        prop_assert_eq!(meter.attributed_us(), work_us.min(MAX_ATTRIBUTED_POLL_US));
        prop_assert_eq!(
            meter.long_poll_violations(),
            u64::from(work_us > MAX_ATTRIBUTED_POLL_US)
        );
    }

    /// A sequence of bounded jobs attributes exactly the sum of their elapsed
    /// CPU.
    #[test]
    fn bounded_jobs_sum_exactly(jobs in prop::collection::vec(0u64..100_000, 0..12)) {
        let meter = meter();
        let mut expected = 0u64;
        for elapsed in &jobs {
            let clock: Arc<dyn ThreadCpuClock> = ScriptedCpuClock::new([0, *elapsed]);
            let measured = CpuJob::begin(&meter, &clock).finish();
            prop_assert_eq!(measured.get(), *elapsed);
            expected += elapsed;
        }
        prop_assert_eq!(meter.attributed_us(), expected);
        prop_assert_eq!(meter.dropped_jobs(), 0);
    }

    /// A thread clock that appears to move backwards — task migration between
    /// worker threads — attributes zero rather than underflowing into a huge
    /// charge.
    #[test]
    fn a_backwards_clock_attributes_zero(start in 1u64..1_000_000, back in 1u64..1_000_000) {
        let meter = meter();
        let clock: Arc<dyn ThreadCpuClock> =
            ScriptedCpuClock::new([start, start.saturating_sub(back)]);
        prop_assert_eq!(CpuJob::begin(&meter, &clock).finish().get(), 0);
        prop_assert_eq!(meter.attributed_us(), 0);
    }
}

// ---------------------------------------------------------------------------
// U3 — memory reservations
// ---------------------------------------------------------------------------

proptest! {
    /// The envelope is never over-committed, whatever sequence of grants is
    /// requested, and every granted byte is returned when its token is released.
    #[test]
    fn the_memory_envelope_is_never_over_committed(
        requests in prop::collection::vec(0u64..40_000_000, 0..16),
    ) {
        let clock: Arc<dyn WallClock> = SteppingClock::new(1);
        let budget = MemoryBudget::new(64_000_000, 8_000_000, sink(256), clock).expect("budget");
        let grantable = budget.grantable_bytes();

        let mut held = Vec::new();
        for bytes in &requests {
            match budget.try_reserve(
                &context(),
                &Attribution::default(),
                ReservationClass::Context,
                *bytes,
            ) {
                Ok(token) => held.push(token),
                Err(ProbeError::BudgetExhausted { .. }) => {}
                Err(other) => prop_assert!(false, "unexpected refusal: {}", other),
            }
            prop_assert!(
                budget.live_bytes() <= grantable,
                "live {} exceeded grantable {}",
                budget.live_bytes(),
                grantable
            );
        }

        drop(held);
        prop_assert_eq!(
            budget.live_bytes(),
            0,
            "every granted byte returns when its token is released"
        );
    }

    /// A resized reservation integrates to exactly the sum of its fixed
    /// segments, over any resize sequence. No rounding step exists, so the
    /// equality is exact.
    #[test]
    fn a_resize_sequence_integrates_exactly(
        sizes in prop::collection::vec(1u64..2_000_000, 1..8),
        step_ms in 1u64..500,
    ) {
        let clock: Arc<dyn WallClock> = SteppingClock::new(step_ms);
        let budget = MemoryBudget::new(64_000_000, 8_000_000, sink(256), clock).expect("budget");

        let mut token = budget
            .try_reserve(
                &context(),
                &Attribution::default(),
                ReservationClass::Context,
                sizes[0],
            )
            .expect("the first grant fits");

        let mut total = 0u128;
        let mut expected = 0u128;
        for size in sizes.iter().skip(1) {
            expected += u128::from(token.bytes()) * u128::from(step_ms);
            total += token.resize(*size).expect("resize fits").get();
        }
        expected += u128::from(token.bytes()) * u128::from(step_ms);
        total += token.release().expect("releases").get();

        prop_assert_eq!(total, expected, "the integral is the sum of the segments");
        prop_assert_eq!(budget.live_bytes(), 0);
    }

    /// Every closed interval emits exactly one fact — no more, no fewer.
    #[test]
    fn each_closed_interval_emits_exactly_one_fact(resizes in 0usize..8) {
        let clock: Arc<dyn WallClock> = SteppingClock::new(10);
        let facts = sink(256);
        let budget =
            MemoryBudget::new(64_000_000, 8_000_000, Arc::clone(&facts), clock).expect("budget");

        let mut token = budget
            .try_reserve(
                &context(),
                &Attribution::default(),
                ReservationClass::PreviewBuffer,
                1_024,
            )
            .expect("the grant fits");
        for step in 0..resizes {
            token
                .resize(1_024 + u64::try_from(step).expect("bounded"))
                .expect("resize fits");
        }
        token.release().expect("releases");

        // One per resize, plus one for the final release.
        prop_assert_eq!(facts.take_batch(1_024).len(), resizes + 1);
    }
}

// ---------------------------------------------------------------------------
// U3 — egress counting
// ---------------------------------------------------------------------------

proptest! {
    /// A closed crossing produces exactly one fact whose quantity is everything
    /// counted; a dropped crossing produces none at all.
    #[test]
    fn a_crossing_produces_at_most_one_fact(
        writes in prop::collection::vec(0u64..1_000_000, 0..24),
        close_it in any::<bool>(),
    ) {
        let facts = sink(64);
        let dropped = Arc::new(AtomicU64::new(0));
        let mut crossing = EgressCounter::open(
            BoundaryId::REGIONAL_STREAM,
            context(),
            Attribution::default(),
            CounterEpoch::new(1),
            42,
            Arc::clone(&facts),
            Arc::clone(&dropped),
        );

        let mut expected = 0u64;
        for bytes in &writes {
            crossing.count(*bytes).expect("bounded by the strategy");
            expected += bytes;
        }
        prop_assert_eq!(crossing.counted(), expected);

        if close_it {
            let quantity = crossing.close(at(0)).expect("closes");
            prop_assert_eq!(quantity.get(), u128::from(expected));
            prop_assert_eq!(facts.take_batch(64).len(), 1);
            prop_assert_eq!(dropped.load(Ordering::Relaxed), 0);
        } else {
            drop(crossing);
            prop_assert!(
                facts.take_batch(64).is_empty(),
                "an interrupted crossing has no authoritative byte count"
            );
            prop_assert_eq!(dropped.load(Ordering::Relaxed), 1);
        }
    }

    /// A Hands generation with no provider transmit receipt never yields a
    /// transfer fact, whatever else the receipt says.
    #[test]
    fn hands_transmit_without_a_receipt_never_becomes_a_fact(bytes in 0u64..1_000_000) {
        let refused = egress_from_receipt(
            &context(),
            &Attribution::default(),
            at(0),
            &BoundaryReceipt::HandsProviderTransmit {
                generation: Box::from("gen-1"),
                receipt_id: Box::from("rcpt-1"),
                transmit_bytes: None,
            },
        );
        prop_assert!(
            matches!(refused, Err(ProbeError::NoTransmitEvidence { .. })),
            "a generation with no transmit receipt must produce no transfer fact"
        );

        let settled = egress_from_receipt(
            &context(),
            &Attribution::default(),
            at(0),
            &BoundaryReceipt::HandsProviderTransmit {
                generation: Box::from("gen-1"),
                receipt_id: Box::from("rcpt-1"),
                transmit_bytes: Some(bytes),
            },
        )
        .expect("a real receipt settles");
        prop_assert_eq!(
            settled.kind.measurement().expect("measured").quantity().get(),
            u128::from(bytes)
        );
    }
}

// ---------------------------------------------------------------------------
// U3 — allocated shapes
// ---------------------------------------------------------------------------

fn hands_shape() -> impl Strategy<Value = HandsShape> {
    prop::sample::select(HandsShape::ALL.to_vec())
}

proptest! {
    /// Suspended time bills nothing on either meter, for any split of a
    /// generation's lifetime and any public shape token.
    #[test]
    fn suspended_time_is_never_billed(
        shape in hands_shape(),
        running_ms in 0u64..600_000,
        suspended_ms in 0u64..600_000,
    ) {
        let lifetime = i64::try_from(running_ms + suspended_ms).expect("bounded");
        let receipt = RuntimeReceipt {
            receipt_id: Box::from("rcpt-1"),
            generation: Box::from("gen-1"),
            shape,
            from: at(0),
            to: at(lifetime),
            running_ms,
            suspended_ms,
            transmit_bytes: None,
        };
        let drafts = hands_facts(&context(), &Attribution::default(), &receipt).expect("valid");

        let baseline = shape.baseline();
        prop_assert_eq!(
            drafts[0].kind.measurement().expect("measured").quantity().get(),
            u128::from(baseline.millicpu) * u128::from(running_ms)
        );
        prop_assert_eq!(
            drafts[1].kind.measurement().expect("measured").quantity().get(),
            u128::from(baseline.memory_bytes) * u128::from(running_ms)
        );
    }

    /// A receipt whose halves do not account for its lifetime is refused rather
    /// than billed on a guess.
    #[test]
    fn an_unexhausted_lifetime_is_always_refused(
        running_ms in 0u64..300_000,
        suspended_ms in 0u64..300_000,
        remainder in 1u64..300_000,
    ) {
        let lifetime = i64::try_from(running_ms + suspended_ms + remainder).expect("bounded");
        let receipt = RuntimeReceipt {
            receipt_id: Box::from("rcpt-1"),
            generation: Box::from("gen-1"),
            shape: HandsShape::Gb1,
            from: at(0),
            to: at(lifetime),
            running_ms,
            suspended_ms,
            transmit_bytes: None,
        };
        prop_assert!(
            matches!(
                hands_facts(&context(), &Attribution::default(), &receipt),
                Err(ProbeError::UnexhaustedLifetime { .. })
            ),
            "an unexplained remainder must be refused, not billed"
        );
    }

    /// The Lambda convention floors once over the whole product, so it is always
    /// at least as large as flooring the nominal millicpu first, and never more
    /// than one millicpu-millisecond per millisecond above it.
    #[test]
    fn the_lambda_convention_floors_once_not_twice(
        memory_mb in 128u32..10_241,
        billed_ms in 0u64..900_001,
    ) {
        let convention = LambdaVcpuConvention::LinearAt1769Mb { version: 1 };
        let drafts = lambda_facts(
            &context(),
            &Attribution::default(),
            &LambdaReport {
                request_id: Box::from("req-1"),
                memory_mb,
                billed_ms,
            },
            at(0),
            convention,
        )
        .expect("valid report");

        let charged = drafts[0]
            .kind
            .measurement()
            .expect("measured")
            .quantity()
            .get();
        let exact = u128::from(memory_mb) * 1_000 * u128::from(billed_ms) / 1_769;
        prop_assert_eq!(charged, exact);

        // Flooring the per-millisecond rate first would lose up to one millicpu
        // per millisecond.
        let floored_first =
            u128::from(convention.nominal_millicpu(memory_mb)) * u128::from(billed_ms);
        prop_assert!(charged >= floored_first);
        prop_assert!(charged <= floored_first + u128::from(billed_ms));

        prop_assert_eq!(
            drafts[1]
                .kind
                .measurement()
                .expect("measured")
                .quantity()
                .get(),
            u128::from(memory_mb) * 1_048_576 * u128::from(billed_ms)
        );
    }
}

// ---------------------------------------------------------------------------
// U3 — the sink never loses a draft
// ---------------------------------------------------------------------------

proptest! {
    /// Every draft offered is accounted for: queued, refused with a typed error,
    /// or parked in the overflow ledger. Nothing is dropped quietly, because a
    /// lost fact is lost money.
    #[test]
    fn every_offered_draft_is_accounted_for(
        capacity in 1usize..16,
        offers in 0usize..64,
        via_drop_path in any::<bool>(),
    ) {
        let overflow = Arc::new(OverflowLedger::new());
        let facts = BoundedFactSink::new(capacity, Arc::clone(&overflow)).expect("capacity");

        let mut accepted = 0usize;
        for seed in 0..offers {
            let seed = u64::try_from(seed).expect("bounded");
            if via_drop_path {
                facts.offer_or_park(draft(seed));
            } else if facts.offer(draft(seed)).is_ok() {
                accepted += 1;
            }
        }

        let queued = facts.queued();
        let shed = usize::try_from(facts.shed_total()).expect("bounded");
        let parked = overflow.len();

        if via_drop_path {
            prop_assert_eq!(
                queued + parked,
                offers,
                "queued plus parked must account for every offer"
            );
        } else {
            prop_assert_eq!(accepted, queued);
            prop_assert_eq!(
                accepted + shed,
                offers,
                "accepted plus shed must account for every offer"
            );
        }
        prop_assert!(queued <= capacity, "the bound is never exceeded");
    }
}
