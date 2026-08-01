//! A11-MUX measurement assertions.
//!
//! Every clock here is scripted rather than sampled, so each assertion about an attributed
//! quantity is exact. A probe test that measured real elapsed time would be asserting the
//! test machine's scheduler rather than the accounting rule.

use super::{Measurement, RECONCILE_INTERVAL, SINK_CAPACITY, byte_ms, millicpu_ms_from_cpu_us};
use aex_usage_application::probe::{
    ActivationKey, ActivationScoped, CpuInstant, CpuJob, MAX_ATTRIBUTED_POLL_US, PhysicalCpuSource,
    ProbeContext, ProbeError, ThreadCpuClock, WallClock,
};
use aex_usage_domain::fact::{Attribution, ResourceGeneration, ResourceKind};
use aex_usage_domain::measurement::ReservationClass;
use aex_usage_domain::wire_pending::{
    ActivationId, AgentId, OrganizationId, PricingVersion, RegionId, ServiceId, SessionId,
    WorkspaceId,
};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// A clock whose readings are a script.
#[derive(Debug)]
struct ScriptedCpu {
    readings: Mutex<VecDeque<u64>>,
    last: AtomicU64,
}

impl ScriptedCpu {
    fn new(readings: impl IntoIterator<Item = u64>) -> Arc<Self> {
        Arc::new(Self {
            readings: Mutex::new(readings.into_iter().collect()),
            last: AtomicU64::new(0),
        })
    }
}

impl ThreadCpuClock for ScriptedCpu {
    fn thread_cpu_now(&self) -> CpuInstant {
        let next = self
            .readings
            .lock()
            .expect("not poisoned")
            .pop_front()
            .unwrap_or_else(|| self.last.load(Ordering::SeqCst));
        self.last.store(next, Ordering::SeqCst);
        CpuInstant::from_micros(next)
    }
}

#[derive(Debug)]
struct ScriptedPhysical(AtomicU64);

impl PhysicalCpuSource for ScriptedPhysical {
    fn task_cpu_usec(&self) -> Result<u64, ProbeError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}

#[derive(Debug)]
struct FixedWall(AtomicU64);

impl WallClock for FixedWall {
    fn now(&self) -> time::OffsetDateTime {
        time::OffsetDateTime::from_unix_timestamp_nanos(
            i128::from(self.0.load(Ordering::SeqCst)) * 1_000_000,
        )
        .expect("in range")
    }

    fn instant(&self) -> std::time::Instant {
        std::time::Instant::now()
    }
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

fn key() -> ActivationKey {
    ActivationKey {
        session: SessionId::parse("sess-1").expect("session"),
        agent: AgentId::parse("agent-1").expect("agent"),
        activation: ActivationId::parse("act-1").expect("activation"),
        fence: 1,
    }
}

fn measurement(
    physical: Arc<ScriptedPhysical>,
    cpu: Arc<ScriptedCpu>,
    envelope: u64,
) -> Measurement {
    Measurement::new(envelope, envelope / 8, physical)
        .expect("the envelope leaves grantable bytes")
        .with_clocks(cpu, Arc::new(FixedWall(AtomicU64::new(0))))
}

/// A future that is pending exactly once, then ready. It stands in for a provider stream, a
/// tool call and a durable wait alike: all three are pending between polls.
struct PendingOnce(bool);

impl core::future::Future for PendingOnce {
    type Output = ();

    fn poll(
        mut self: core::pin::Pin<&mut Self>,
        context: &mut core::task::Context<'_>,
    ) -> core::task::Poll<()> {
        if self.0 {
            return core::task::Poll::Ready(());
        }
        self.0 = true;
        context.waker().wake_by_ref();
        core::task::Poll::Pending
    }
}

#[test]
fn the_unit_identity_removes_rounding_from_the_billing_path() {
    assert_eq!(millicpu_ms_from_cpu_us(1), 1);
    assert_eq!(millicpu_ms_from_cpu_us(1_000_000), 1_000_000);
    assert_eq!(byte_ms(1_024, 1_000), 1_024_000);
    assert_eq!(byte_ms(u64::MAX, 2), u64::MAX, "saturating, never wrapping");
}

#[test]
fn the_reconciliation_interval_and_sink_capacity_are_pinned() {
    assert_eq!(RECONCILE_INTERVAL.as_secs(), 10);
    assert_eq!(SINK_CAPACITY, 4_096);
}

/// The per-poll cap is the usage stream's constant, not a second copy of it.
#[test]
fn the_per_poll_cap_is_the_shared_one() {
    assert_eq!(MAX_ATTRIBUTED_POLL_US, 50_000);
}

/// The load-bearing assertion of A11-MUX. Time spent **pending** on provider HTTP, a tool
/// call or a durable wait is not inside a poll, so it produces no compute fact. The scripted
/// clock jumps by nearly a second across the pending gap and none of it is attributed.
#[tokio::test(flavor = "current_thread")]
async fn time_pending_on_an_external_wait_creates_no_compute_fact() {
    let cpu = ScriptedCpu::new([100, 150, 1_000_000, 1_000_010]);
    let physical = Arc::new(ScriptedPhysical(AtomicU64::new(10_000_000)));
    let measurement = measurement(physical, Arc::clone(&cpu), 1 << 20);
    let meter = measurement.activation(key(), context(), Attribution::default());

    PendingOnce(false)
        .metered(Arc::clone(&meter), Arc::clone(measurement.thread_clock()))
        .await;

    assert_eq!(
        meter.attributed_us(),
        60,
        "only the two polls are charged; the gap between them is not"
    );
    assert_eq!(meter.poll_count(), 2);
    assert_eq!(meter.long_poll_violations(), 0);
}

/// A poll above the cap is a defect — long work belongs on the compute lane — so it is
/// counted as a violation rather than turned into a larger charge.
#[tokio::test(flavor = "current_thread")]
async fn a_poll_above_the_cap_is_counted_as_a_defect_not_billed_in_full() {
    let over = MAX_ATTRIBUTED_POLL_US + 25_000;
    let cpu = ScriptedCpu::new([0, over]);
    let physical = Arc::new(ScriptedPhysical(AtomicU64::new(10_000_000)));
    let measurement = measurement(physical, Arc::clone(&cpu), 1 << 20);
    let meter = measurement.activation(key(), context(), Attribution::default());

    core::future::ready(())
        .metered(Arc::clone(&meter), Arc::clone(measurement.thread_clock()))
        .await;

    assert_eq!(meter.attributed_us(), MAX_ATTRIBUTED_POLL_US);
    assert_eq!(meter.long_poll_violations(), 1);
}

/// A compute-lane job reads the thread clock around the whole job, so bounded CPU that is
/// deliberately moved off the reactor is still attributed to the activation that caused it.
#[test]
fn a_compute_lane_job_attributes_its_own_cpu() {
    let cpu = ScriptedCpu::new([1_000, 1_400]);
    let physical = Arc::new(ScriptedPhysical(AtomicU64::new(10_000_000)));
    let measurement = measurement(physical, Arc::clone(&cpu), 1 << 20);
    let meter = measurement.activation(key(), context(), Attribution::default());

    let job = CpuJob::begin(&meter, measurement.thread_clock());
    let charged = job.finish();
    assert_eq!(charged.get(), 400);
    assert_eq!(meter.attributed_us(), 400);
}

/// The cap that makes the whole scheme safe: whatever the meters claim, the sum charged can
/// never exceed the physical CPU the task actually used.
#[test]
fn charged_cpu_never_exceeds_the_cgroup_delta() {
    let cpu = ScriptedCpu::new([0, 1_000]);
    let physical = Arc::new(ScriptedPhysical(AtomicU64::new(100)));
    let measurement = measurement(physical, Arc::clone(&cpu), 1 << 20);
    let meter = measurement.activation(key(), context(), Attribution::default());
    let job = CpuJob::begin(&meter, measurement.thread_clock());
    let _ = job.finish();

    let report = measurement.close_interval().expect("the source reads");
    assert!(
        report.charged_us <= report.physical_us,
        "charged {} exceeds physical {}",
        report.charged_us,
        report.physical_us
    );
    assert_eq!(
        report.charged_us + report.platform_us,
        report.physical_us,
        "the remainder is an unbilled platform bucket, never lost"
    );
    assert!(report.scaled, "an over-attributing interval scales down");
    assert_eq!(measurement.overattribution_total(), 1);
}

/// A memory reservation is an RAII token, so the interval closes in the same code path that
/// finishes the work rather than at some later sweep.
#[test]
fn every_reservation_interval_closes_when_the_token_drops() {
    let cpu = ScriptedCpu::new([0]);
    let physical = Arc::new(ScriptedPhysical(AtomicU64::new(1_000)));
    let measurement = measurement(physical, cpu, 1 << 20);
    let context = context();
    let attribution = Attribution::default();

    assert_eq!(measurement.live_bytes(), 0);
    {
        let reservation = measurement
            .reserve(&context, &attribution, ReservationClass::Context, 4_096)
            .expect("inside the envelope");
        assert_eq!(reservation.bytes(), 4_096);
        assert_eq!(measurement.live_bytes(), 4_096);
    }
    assert_eq!(
        measurement.live_bytes(),
        0,
        "a dropped token releases its bytes in the same code path"
    );
}

/// A reservation the envelope cannot grant is refused. The caller defers or sheds; it never
/// proceeds, because an unreserved buffer is memory nobody accounted for and nobody frees.
#[test]
fn a_reservation_over_the_envelope_is_refused_rather_than_granted() {
    let cpu = ScriptedCpu::new([0]);
    let physical = Arc::new(ScriptedPhysical(AtomicU64::new(1_000)));
    let measurement = measurement(physical, cpu, 4_096);
    let error = measurement
        .reserve(
            &context(),
            &Attribution::default(),
            ReservationClass::Context,
            1 << 30,
        )
        .expect_err("a gigabyte does not fit four kilobytes");
    assert!(
        matches!(error, ProbeError::BudgetExhausted { .. }),
        "{error:?}"
    );
    assert_eq!(measurement.live_bytes(), 0);
}

/// A finished activation drops out of the registry on its own, so a forgotten deregister
/// cannot leak a meter for the life of the task.
#[test]
fn a_finished_activation_leaves_the_registry_on_its_own() {
    let cpu = ScriptedCpu::new([0]);
    let physical = Arc::new(ScriptedPhysical(AtomicU64::new(1_000)));
    let measurement = measurement(physical, cpu, 1 << 20);
    {
        let _meter = measurement.activation(key(), context(), Attribution::default());
        assert_eq!(measurement.live_meters(), 1);
    }
    assert_eq!(measurement.live_meters(), 0);
}
