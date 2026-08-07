//! cgroup reconciliation: the point where attributed poll time becomes a
//! charge, bounded by what the machine actually consumed.
//!
//! Poll-elapsed attribution is an allocation *weight*, never billable truth. At
//! each interval close the reconciler reads the cgroup's own `usage_usec` delta,
//! scales every meter's attribution down proportionally if the meters together
//! claimed more than the task physically used, and books the unclaimed remainder
//! to the unbilled platform bucket. `charged <= physical` therefore holds by
//! construction rather than by trust.

use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use aex_usage_domain::fact::{FactDraft, FactKind, SCHEMA_VERSION};
use aex_usage_domain::identity::{AuthorityId, AuthorityKey, AuthorityKind, SegmentOrdinal};
use aex_usage_domain::interval::reconcile_cpu;
use aex_usage_domain::measurement::{
    Evidence, FactBasis, Measurement, ReceiptKind, ServiceTime, SourceReceipt,
};
use aex_usage_domain::meter::{Category, Meter};
use aex_usage_domain::wire_pending::Timestamp;

use super::clock::{PhysicalCpuSource, WallClock};
use super::cpu::ActivationMeter;
use super::sink::FactSink;
use super::{ProbeError, to_timestamp};

/// What one closed accounting interval accounted for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuIntervalReport {
    /// The cgroup's `usage_usec` delta over the interval.
    pub physical_us: u64,
    /// What the meters together claimed.
    pub attributed_us: u64,
    /// What was actually charged after any scaling.
    pub charged_us: u64,
    /// What no meter claimed; platform overhead, never billed.
    pub platform_us: u64,
    /// Whether proportional scaling was applied.
    pub scaled: bool,
    /// How many facts were emitted.
    pub facts_emitted: u32,
    /// The interval the facts cover.
    pub interval: ServiceTime,
}

/// Registered meters and the last physical reading.
#[derive(Debug)]
struct ReconcilerState {
    registry: Vec<Weak<ActivationMeter>>,
    last_physical_usec: Option<u64>,
    last_close: Option<Timestamp>,
    next_ordinal: u64,
    overattribution_total: u64,
}

/// Closes one accounting interval across every registered activation meter.
#[derive(Debug)]
pub struct CpuReconciler {
    source: Arc<dyn PhysicalCpuSource>,
    clock: Arc<dyn WallClock>,
    interval: Duration,
    state: Mutex<ReconcilerState>,
}

impl CpuReconciler {
    /// Builds a reconciler over one physical CPU source.
    #[must_use]
    pub fn new(
        source: Arc<dyn PhysicalCpuSource>,
        clock: Arc<dyn WallClock>,
        interval: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            source,
            clock,
            interval,
            state: Mutex::new(ReconcilerState {
                registry: Vec::new(),
                last_physical_usec: None,
                last_close: None,
                next_ordinal: 0,
                overattribution_total: 0,
            }),
        })
    }

    /// The tick length this reconciler expects to be driven at.
    #[must_use]
    pub const fn interval(&self) -> Duration {
        self.interval
    }

    /// How many closes needed proportional scaling.
    #[must_use]
    pub fn overattribution_total(&self) -> u64 {
        self.state
            .lock()
            .map_or(0, |state| state.overattribution_total)
    }

    /// Registers an activation meter. The registry holds weak references, so a
    /// finished activation drops out without an explicit deregister.
    pub fn register(&self, meter: &Arc<ActivationMeter>) {
        if let Ok(mut state) = self.state.lock() {
            state.registry.push(Arc::downgrade(meter));
        }
    }

    /// How many meters are still live.
    #[must_use]
    pub fn live_meters(&self) -> usize {
        self.state.lock().map_or(0, |state| {
            state
                .registry
                .iter()
                .filter(|weak| weak.strong_count() > 0)
                .count()
        })
    }

    /// Closes one interval: reads the physical delta, drains every live meter,
    /// scales if the meters over-claimed, and offers one fact per meter that
    /// ends up with a non-zero charge.
    ///
    /// # Errors
    ///
    /// Returns [`ProbeError::PhysicalSource`] when the cgroup reading is
    /// unavailable — never a zero reading — and [`ProbeError::Measurement`] when
    /// a derived measurement fails its own construction rules.
    pub fn close_interval(&self, sink: &dyn FactSink) -> Result<CpuIntervalReport, ProbeError> {
        let physical_now = self.source.task_cpu_usec()?;
        let closed_at = to_timestamp(self.clock.now())?;

        let mut state = self.state.lock().map_err(|_| ProbeError::Poisoned {
            what: "cpu reconciler state",
        })?;

        let physical_us = state.last_physical_usec.map_or(physical_now, |previous| {
            physical_now.saturating_sub(previous)
        });
        state.last_physical_usec = Some(physical_now);

        let opened_at = state.last_close.unwrap_or(closed_at);
        state.last_close = Some(closed_at);
        let interval = ServiceTime::Interval {
            start: opened_at,
            end: closed_at,
        };
        let interval_ms = interval.duration_ms()?;

        // Drop dead meters, then drain the live ones. Draining is destructive,
        // so it happens exactly once per meter per interval.
        state.registry.retain(|weak| weak.strong_count() > 0);
        let live: Vec<Arc<ActivationMeter>> =
            state.registry.iter().filter_map(Weak::upgrade).collect();
        let attributed: Vec<u64> = live.iter().map(|meter| meter.drain_us()).collect();
        let attributed_us: u64 = attributed.iter().copied().sum();

        let allocation = reconcile_cpu(physical_us, &attributed)?;
        if allocation.scaled {
            state.overattribution_total += 1;
        }
        let ordinal_base = state.next_ordinal;
        state.next_ordinal += 1;
        drop(state);

        let mut facts_emitted = 0u32;
        for ((meter, claimed), charged) in live
            .iter()
            .zip(attributed.iter())
            .zip(allocation.charged.iter())
        {
            if *charged == 0 {
                continue;
            }
            let draft = Self::draft(
                meter,
                interval,
                interval_ms,
                physical_us,
                *claimed,
                *charged,
                allocation.scaled,
                ordinal_base,
            )?;
            // A refused offer is a typed error the caller sees; the probe never
            // swallows one, because a dropped draft is a dropped charge.
            sink.offer(draft)?;
            facts_emitted += 1;
        }

        Ok(CpuIntervalReport {
            physical_us,
            attributed_us,
            charged_us: allocation.charged_us(),
            platform_us: allocation.platform_us,
            scaled: allocation.scaled,
            facts_emitted,
            interval,
        })
    }

    /// Builds one activation's closed-interval draft.
    #[allow(clippy::too_many_arguments, reason = "one evidence record, one draft")]
    fn draft(
        meter: &Arc<ActivationMeter>,
        interval: ServiceTime,
        interval_ms: u64,
        physical_us: u64,
        attributed_us: u64,
        charged_us: u64,
        scaled: bool,
        ordinal: u64,
    ) -> Result<FactDraft, ProbeError> {
        let context = meter.context();
        let key = meter.key();
        let authority_id = AuthorityId::parse(&format!(
            "{}:{}:{}",
            key.activation.as_str(),
            key.fence,
            ordinal
        ))?;
        let measurement = Measurement::new(
            Meter::ComputeMillicpuMs,
            FactBasis::Consumed,
            interval,
            SourceReceipt {
                kind: ReceiptKind::CgroupInterval,
                id: Box::from(authority_id.as_str()),
                digest: None,
            },
            Evidence::CgroupReconciled {
                physical_us,
                attributed_us,
                charged_us,
                scaled,
                interval_ms,
            },
        )?;
        Ok(FactDraft {
            schema_version: SCHEMA_VERSION,
            organization: context.organization.clone(),
            workspace: context.workspace.clone(),
            region: context.region.clone(),
            attribution: meter.attribution().clone(),
            service: context.service.clone(),
            resource: context.resource.clone(),
            authority: AuthorityKey {
                region: context.region.clone(),
                category: Category::Compute,
                kind: AuthorityKind::Activation,
                authority_id,
                segment_ordinal: SegmentOrdinal::new(ordinal),
            },
            pricing_version: context.pricing_version.clone(),
            reservation: context.reservation.clone(),
            kind: FactKind::Measured(measurement),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::CpuReconciler;
    use crate::probe::clock::{PhysicalCpuSource, ThreadCpuClock};
    use crate::probe::cpu::{ActivationMeter, CpuJob};
    use crate::probe::sink::{BoundedFactSink, OverflowLedger};
    use crate::probe::testing::{FixedClock, ScriptedCpuClock, activation_key, probe_context};
    use crate::probe::{ProbeError, WallClock};
    use aex_usage_domain::fact::{Attribution, FactKind};
    use aex_usage_domain::measurement::Evidence;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    /// A cgroup source returning scripted cumulative readings.
    #[derive(Debug)]
    struct FakeCgroup {
        readings: std::sync::Mutex<std::collections::VecDeque<u64>>,
        last: AtomicU64,
        fail: bool,
    }

    impl FakeCgroup {
        fn new(readings: impl IntoIterator<Item = u64>) -> Arc<Self> {
            Arc::new(Self {
                readings: std::sync::Mutex::new(readings.into_iter().collect()),
                last: AtomicU64::new(0),
                fail: false,
            })
        }

        fn failing() -> Arc<Self> {
            Arc::new(Self {
                readings: std::sync::Mutex::new(std::collections::VecDeque::new()),
                last: AtomicU64::new(0),
                fail: true,
            })
        }
    }

    impl PhysicalCpuSource for FakeCgroup {
        fn task_cpu_usec(&self) -> Result<u64, ProbeError> {
            if self.fail {
                return Err(ProbeError::PhysicalSource {
                    source_name: "cgroup cpu.stat",
                    path: "/sys/fs/cgroup/cpu.stat".to_owned(),
                    reason: "unreadable".to_owned(),
                });
            }
            let next = self
                .readings
                .lock()
                .expect("lock")
                .pop_front()
                .unwrap_or_else(|| self.last.load(Ordering::Relaxed));
            self.last.store(next, Ordering::Relaxed);
            Ok(next)
        }
    }

    fn sink() -> BoundedFactSink {
        BoundedFactSink::new(64, Arc::new(OverflowLedger::new())).expect("capacity")
    }

    fn meter(seed: u64) -> Arc<ActivationMeter> {
        let mut key = activation_key();
        key.fence = seed;
        ActivationMeter::new(key, probe_context(), Attribution::default())
    }

    fn attribute(meter: &Arc<ActivationMeter>, micros: u64) {
        let clock: Arc<dyn ThreadCpuClock> = ScriptedCpuClock::new([0, micros]);
        let _ = CpuJob::begin(meter, &clock).finish();
    }

    #[test]
    fn an_unsaturated_interval_charges_exactly_what_each_meter_attributed() {
        let cgroup = FakeCgroup::new([0, 10_000]);
        let clock: Arc<dyn WallClock> = FixedClock::new(0);
        let reconciler = CpuReconciler::new(cgroup, Arc::clone(&clock), Duration::from_secs(10));
        let sink = sink();

        let first = meter(1);
        let second = meter(2);
        reconciler.register(&first);
        reconciler.register(&second);
        attribute(&first, 3_000);
        attribute(&second, 2_000);

        // First close establishes the baseline reading.
        reconciler.close_interval(&sink).expect("baseline close");
        attribute(&first, 3_000);
        attribute(&second, 2_000);
        let report = reconciler.close_interval(&sink).expect("closes");

        assert_eq!(report.physical_us, 10_000);
        assert_eq!(report.attributed_us, 5_000);
        assert_eq!(report.charged_us, 5_000);
        assert_eq!(report.platform_us, 5_000);
        assert!(!report.scaled);
        assert_eq!(report.facts_emitted, 2);
    }

    #[test]
    fn over_attribution_is_scaled_down_and_never_exceeds_the_physical_reading() {
        let cgroup = FakeCgroup::new([0, 1_000]);
        let clock: Arc<dyn WallClock> = FixedClock::new(0);
        let reconciler = CpuReconciler::new(cgroup, clock, Duration::from_secs(10));
        let sink = sink();

        let first = meter(1);
        let second = meter(2);
        reconciler.register(&first);
        reconciler.register(&second);
        reconciler.close_interval(&sink).expect("baseline close");

        // Ten times the physical reading, the pathological case.
        attribute(&first, 5_000);
        attribute(&second, 5_000);
        let report = reconciler.close_interval(&sink).expect("closes");

        assert!(report.scaled);
        assert_eq!(report.attributed_us, 10_000);
        assert_eq!(report.charged_us, 1_000);
        assert!(report.charged_us <= report.physical_us);
        assert_eq!(report.charged_us + report.platform_us, report.physical_us);
        assert_eq!(reconciler.overattribution_total(), 1);
    }

    #[test]
    fn the_emitted_fact_carries_the_reconciled_evidence() {
        let cgroup = FakeCgroup::new([0, 4_000]);
        let clock: Arc<dyn WallClock> = FixedClock::new(0);
        let reconciler = CpuReconciler::new(cgroup, clock, Duration::from_secs(10));
        let sink = sink();

        let only = meter(7);
        reconciler.register(&only);
        reconciler.close_interval(&sink).expect("baseline");
        let _ = sink.take_batch(64);

        attribute(&only, 2_500);
        reconciler.close_interval(&sink).expect("closes");

        let batch = sink.take_batch(64);
        assert_eq!(batch.len(), 1);
        let FactKind::Measured(measurement) = &batch[0].kind else {
            panic!("a reconciled interval is a measured fact");
        };
        assert_eq!(measurement.quantity().get(), 2_500);
        match measurement.evidence() {
            Evidence::CgroupReconciled {
                physical_us,
                attributed_us,
                charged_us,
                scaled,
                ..
            } => {
                assert_eq!(*physical_us, 4_000);
                assert_eq!(*attributed_us, 2_500);
                assert_eq!(*charged_us, 2_500);
                assert!(!scaled);
            }
            other => panic!("expected cgroup evidence, got {other:?}"),
        }
    }

    #[test]
    fn a_meter_that_attributed_nothing_emits_no_fact() {
        let cgroup = FakeCgroup::new([0, 9_000]);
        let clock: Arc<dyn WallClock> = FixedClock::new(0);
        let reconciler = CpuReconciler::new(cgroup, clock, Duration::from_secs(10));
        let sink = sink();

        let idle = meter(1);
        reconciler.register(&idle);
        reconciler.close_interval(&sink).expect("baseline");
        let report = reconciler.close_interval(&sink).expect("closes");

        assert_eq!(report.facts_emitted, 0);
        assert_eq!(report.charged_us, 0);
        assert_eq!(
            report.platform_us, 9_000,
            "unclaimed physical CPU is platform overhead"
        );
    }

    #[test]
    fn an_unreadable_cgroup_is_an_error_not_a_zero_charge() {
        let clock: Arc<dyn WallClock> = FixedClock::new(0);
        let reconciler = CpuReconciler::new(FakeCgroup::failing(), clock, Duration::from_secs(10));
        let sink = sink();
        assert!(matches!(
            reconciler.close_interval(&sink),
            Err(ProbeError::PhysicalSource { .. })
        ));
    }

    #[test]
    fn a_finished_activation_drops_out_of_the_registry() {
        let cgroup = FakeCgroup::new([0, 1_000, 2_000]);
        let clock: Arc<dyn WallClock> = FixedClock::new(0);
        let reconciler = CpuReconciler::new(cgroup, clock, Duration::from_secs(10));
        let sink = sink();

        {
            let transient = meter(1);
            reconciler.register(&transient);
            assert_eq!(reconciler.live_meters(), 1);
        }
        reconciler.close_interval(&sink).expect("closes");
        assert_eq!(reconciler.live_meters(), 0);
    }

    #[test]
    fn draining_happens_once_per_interval_so_nothing_is_charged_twice() {
        let cgroup = FakeCgroup::new([0, 8_000, 16_000]);
        let clock: Arc<dyn WallClock> = FixedClock::new(0);
        let reconciler = CpuReconciler::new(cgroup, clock, Duration::from_secs(10));
        let sink = sink();

        let only = meter(1);
        reconciler.register(&only);
        reconciler.close_interval(&sink).expect("baseline");

        attribute(&only, 6_000);
        let first = reconciler.close_interval(&sink).expect("first close");
        assert_eq!(first.charged_us, 6_000);

        // Nothing further was attributed, so the second close charges nothing.
        let second = reconciler.close_interval(&sink).expect("second close");
        assert_eq!(second.attributed_us, 0);
        assert_eq!(second.charged_us, 0);
    }
}
