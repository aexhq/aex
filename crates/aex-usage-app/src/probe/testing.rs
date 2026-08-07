//! Deterministic fixtures and fake seams shared by the probe's own tests.
//!
//! Every clock here is scripted rather than sampled, so each assertion about an
//! attributed quantity is exact. A probe test that measured real elapsed time
//! would be asserting the test machine's scheduler, not the accounting rule.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aex_usage_domain::fact::{
    Attribution, FactDraft, FactKind, ResourceGeneration, ResourceKind, SCHEMA_VERSION,
};
use aex_usage_domain::identity::{AuthorityId, AuthorityKey, AuthorityKind, SegmentOrdinal};
use aex_usage_domain::measurement::{
    BoundaryId, Evidence, FactBasis, Measurement, ReceiptKind, ServiceTime, SourceReceipt,
};
use aex_usage_domain::meter::{Category, Meter};
use aex_usage_domain::wire_pending::{
    ActivationId, AgentId, OrganizationId, PricingVersion, RegionId, ServiceId, SessionId,
    Timestamp, WorkspaceId,
};
use time::OffsetDateTime;

use super::ProbeContext;
use super::clock::{CpuInstant, ThreadCpuClock, WallClock};
use super::cpu::ActivationKey;

/// A canonical instant from whole milliseconds.
pub fn at(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(millis).expect("representable")
}

/// The probe context every fixture fact carries.
pub fn probe_context() -> ProbeContext {
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

/// A fixture activation key.
pub fn activation_key() -> ActivationKey {
    ActivationKey {
        session: SessionId::parse("sess-1").expect("session"),
        agent: AgentId::parse("agent-1").expect("agent"),
        activation: ActivationId::parse("act-1").expect("activation"),
        fence: 1,
    }
}

/// A distinct valid draft per seed, for sink and drain tests.
pub fn draft_fixture(seed: u64) -> FactDraft {
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

    let context = probe_context();
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

/// A thread CPU clock returning a scripted sequence of readings.
#[derive(Debug)]
pub struct ScriptedCpuClock {
    readings: Mutex<VecDeque<u64>>,
    last: AtomicU64,
}

impl ScriptedCpuClock {
    /// Builds a clock over a scripted reading sequence.
    ///
    /// Once the script is exhausted the clock holds its last value, so a test
    /// that polls more than it scripted attributes zero rather than drifting.
    pub fn new(readings: impl IntoIterator<Item = u64>) -> Arc<Self> {
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
            .expect("clock lock")
            .pop_front()
            .unwrap_or_else(|| self.last.load(Ordering::Relaxed));
        self.last.store(next, Ordering::Relaxed);
        CpuInstant::from_micros(next)
    }
}

/// A wall clock frozen at one instant.
#[derive(Debug)]
pub struct FixedClock {
    wall: OffsetDateTime,
    instant: Instant,
}

impl FixedClock {
    /// Freezes the clock at `millis` since the Unix epoch.
    pub fn new(millis: i64) -> Arc<Self> {
        Arc::new(Self {
            wall: OffsetDateTime::from_unix_timestamp_nanos(i128::from(millis) * 1_000_000)
                .expect("representable"),
            instant: Instant::now(),
        })
    }
}

impl WallClock for FixedClock {
    fn now(&self) -> OffsetDateTime {
        self.wall
    }

    fn instant(&self) -> Instant {
        self.instant
    }
}

/// A wall clock that advances by a fixed step on every `instant()` read.
///
/// Each read returns the current value and then advances, so a hold measured
/// between two reads is exactly one step long.
#[derive(Debug)]
pub struct SteppingClock {
    base: Instant,
    base_millis: i64,
    step_ms: u64,
    elapsed_ms: AtomicU64,
}

impl SteppingClock {
    /// Builds a clock starting at `base_millis` and advancing `step_ms` per read.
    pub fn new(base_millis: i64, step_ms: u64) -> Arc<Self> {
        Arc::new(Self {
            base: Instant::now(),
            base_millis,
            step_ms,
            elapsed_ms: AtomicU64::new(0),
        })
    }
}

impl WallClock for SteppingClock {
    fn now(&self) -> OffsetDateTime {
        let elapsed = self.elapsed_ms.load(Ordering::Acquire);
        let millis = self.base_millis + i64::try_from(elapsed).unwrap_or(i64::MAX);
        OffsetDateTime::from_unix_timestamp_nanos(i128::from(millis) * 1_000_000)
            .expect("representable")
    }

    fn instant(&self) -> Instant {
        let elapsed = self.elapsed_ms.fetch_add(self.step_ms, Ordering::AcqRel);
        self.base + Duration::from_millis(elapsed)
    }
}
