//! The composed loop: one turn end to end, the drain order, and A11-MUX through the
//! composition.
//!
//! The engine's own crash boundaries are asserted in `aex_brain_application::activation`.
//! What this module asserts is that the *composition* preserves them: the same in-memory
//! ports are used here, so a divergence between what the engine promises and what the mux
//! wires it to shows up as a failure rather than as a difference nobody compared.

use crate::admission::{ActivationResources, Admission, AdmissionBounds};
use crate::drain::Stage;
use crate::measure::Measurement;
use crate::wake::{BindingState, Bindings, MuxAdmission};
use aex_brain_application::activation::memory::{
    AbsentHands, CountingIds, FixedCatalog, FixedClock, MemoryQueue, MemoryStore, ProviderScript,
    Recorder, ScriptedProvider, ScriptedTools, wake_for,
};
use aex_brain_application::activation::{
    Activation, ActivationPolicy, Outcome, Ports, Stop, WakeLoop,
};
use aex_brain_application::kernel::{ActivationRegistry, DrainGate, PermitKind, PermitSet};
use aex_brain_application::ports::{
    BoxFuture, CancelToken, ClockPort, DispatchTicket, PreviewSink, ProviderDispatchError,
    ProviderOutcome, ProviderPort, SteadyInstant, StreamBudget, UnknownResolution, WakeQueue as _,
};
use aex_brain_domain::budget::DimensionVector;
use aex_brain_domain::effect::{DispatchEvidence, DurableEffect};
use aex_brain_domain::ids::{
    AgentId, AgentKey, JournalSeq, ModelSlug, SessionId, Timestamp, WakeId, WorkShard,
};
use aex_brain_domain::journal::{FinishReason, JournalEntry, JournalRecord, MessageOrigin};
use aex_brain_domain::wire_pending::{
    AgentLimits, CanonicalBlock, CanonicalModelRequest, ContentBlockRef, NormalizedUsage,
    ProviderId, ResolvedAgentConfig, StopReason,
};
use aex_model_catalog::canonical::{CredentialBindingRef, ProviderReceipt, ReceiptBounds, seal};
use aex_model_catalog::document::CapabilitySet;
use aex_model_catalog::{BoundedString, QualifiedModel, fixture};
use aex_usage_application::probe::{
    ActivationKey, ActivationScoped, CpuInstant, PhysicalCpuSource, ProbeContext, ProbeError,
    ThreadCpuClock,
};
use aex_usage_domain::fact::{Attribution, ResourceGeneration, ResourceKind};
use aex_usage_domain::wire_pending::{
    ActivationId, AgentId as UsageAgentId, OrganizationId, PricingVersion, RegionId, ServiceId,
    SessionId as UsageSessionId, WorkspaceId,
};
use aex_wire::ids::{GenerationId, PrefixedId as _, ProviderCredentialId, Uuid7};
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

const START: i64 = 1_767_225_600_000;

fn key() -> AgentKey {
    AgentKey::new(
        SessionId(Uuid::from_u128(0x5e55_1000)),
        AgentId(Uuid::from_u128(0xa6e7_2000)),
    )
}

fn model() -> ModelSlug {
    ModelSlug::truncating("deepseek-chat")
}

fn config() -> ResolvedAgentConfig {
    ResolvedAgentConfig {
        catalog_pin: capability().catalog(),
        provider: ProviderId::Deepseek,
        credential: aex_brain_domain::wire_pending::SessionCredentialPin::new(
            ProviderCredentialId::from_uuid7(Uuid7::compose(1, [8; 10])),
            1,
            1,
            0,
        )
        .expect("non-zero fixture pin"),
        model: model(),
        system: None,
        tool_manifest_digests: Vec::new(),
        hands_generation: GenerationId::from_uuid7(Uuid7::compose(1, [9; 10])),
        limits: AgentLimits {
            max_turns: 4,
            max_steps_per_turn: 8,
            turn_deadline_ms: 600_000,
        },
    }
}

fn capability() -> QualifiedModel {
    let mut entry = fixture::entry(
        ProviderId::Deepseek,
        "deepseek-chat",
        CapabilitySet::default(),
    );
    entry.limits.context_window_tokens = 64_000;
    entry.limits.max_output_tokens = 4_096;
    entry.limits.min_cacheable_prefix_tokens = 0;
    fixture::qualified(entry)
}

fn history() -> Vec<JournalEntry> {
    vec![
        JournalEntry::seal(
            JournalSeq(0),
            Timestamp::from_millis(START),
            JournalRecord::AgentStarted {
                config: Box::new(config()),
                parent: None,
                join: None,
                depth: 0,
                budget: DimensionVector::uniform(1_000),
            },
        )
        .expect("the record canonicalizes"),
        JournalEntry::seal(
            JournalSeq(1),
            Timestamp::from_millis(START),
            JournalRecord::UserMessage {
                content: vec![ContentBlockRef::Inline {
                    block: CanonicalBlock::Text {
                        text: BoundedString::truncating("summarize this"),
                        annotations: Vec::new(),
                    },
                }],
                origin: MessageOrigin::Submission,
            },
        )
        .expect("the record canonicalizes"),
    ]
}

fn produced() -> ProviderOutcome {
    let usage = NormalizedUsage::default();
    let selected = capability();
    let message = seal(
        vec![CanonicalBlock::Text {
            text: BoundedString::truncating("here is the summary"),
            annotations: Vec::new(),
        }],
        StopReason::EndTurn,
        &usage,
        &selected,
    )
    .expect("a whole message");
    let at = fixture::at(START);
    let receipt = ProviderReceipt {
        provider: message.provider,
        model: message.model.clone(),
        catalog: message.catalog,
        dialect: selected.dialect(),
        dialect_revision: selected.dialect_revision(),
        credential: CredentialBindingRef {
            id: ProviderCredentialId::from_uuid7(Uuid7::compose(1, [4; 10])),
            revision: 1,
            generation: 1,
        },
        provider_request_id: None,
        gateway_route: None,
        http_status: 200,
        attempts: 1,
        started_at: at,
        first_frame_at: Some(at),
        completed_at: at,
        request_bytes: 1,
        response_bytes: 1,
        frames: 1,
        rate_limit: None,
        response_receipt: Some(message.proof.0),
        bounds: ReceiptBounds {
            max_frame_bytes: 1_024,
            max_response_bytes: 1_024,
            idle_frame_timeout_ms: 1_000,
            total_deadline_ms: 10_000,
        },
    };
    ProviderOutcome {
        message,
        usage,
        receipt,
    }
}

fn bound() -> Bindings {
    Bindings {
        store: BindingState::Ready,
        snapshots: BindingState::Ready,
        provider: BindingState::Ready,
        catalog: BindingState::Ready,
        tools: BindingState::Ready,
        hands: BindingState::Ready,
    }
}

fn activation_resources(policy: &ActivationPolicy) -> ActivationResources {
    ActivationResources {
        context_bytes: policy.restore_resident_bytes,
        stream_buffer_bytes: u64::try_from(policy.stream_buffer_bytes).unwrap_or(u64::MAX),
        provider_streams: 1,
        hands_rpcs: 1,
    }
}

fn admission(drain: &Arc<DrainGate>) -> Arc<Admission> {
    let policy = ActivationPolicy::default();
    Arc::new(Admission::new(
        AdmissionBounds {
            target: 4,
            safety_cap: 8,
            offered_ceiling: 16,
        },
        Arc::new(PermitSet::new(BTreeMap::from([
            (PermitKind::Activation, 8_u64),
            (PermitKind::ContextBytes, policy.restore_resident_bytes),
            (
                PermitKind::StreamBufferBytes,
                u64::try_from(policy.stream_buffer_bytes).unwrap_or(u64::MAX),
            ),
            (PermitKind::ProviderStream, 1_u64),
            (PermitKind::HandsRpc, 1_u64),
        ]))),
        Arc::clone(drain),
        activation_resources(&policy),
    ))
}

struct Composed {
    queue: Arc<MemoryQueue>,
    store: Arc<MemoryStore>,
    drain: Arc<DrainGate>,
    pump: WakeLoop,
}

fn compose_loop(provider: Arc<dyn ProviderPort>) -> Composed {
    let log = Arc::new(Recorder::default());
    let clock = Arc::new(FixedClock::at(START));
    let queue = Arc::new(MemoryQueue::new(Arc::clone(&log)));
    let store = Arc::new(MemoryStore::new(
        Arc::clone(&clock),
        Arc::clone(&queue),
        Arc::clone(&log),
    ));
    store.seed(key(), history());
    let drain = Arc::new(DrainGate::new());
    let ports = Ports {
        journal: Arc::clone(&store) as Arc<_>,
        snapshots: Arc::clone(&store) as Arc<_>,
        effects: Arc::clone(&store) as Arc<_>,
        leases: Arc::clone(&store) as Arc<_>,
        wakes: Arc::clone(&queue) as Arc<_>,
        provider,
        tools: Arc::new(ScriptedTools::new(Vec::new(), Vec::new())),
        hands: Arc::new(AbsentHands),
        catalog: Arc::new(FixedCatalog::with_model(capability())),
        clock,
        ids: Arc::new(CountingIds::new()),
    };
    let pump = WakeLoop::new(
        Activation::new(
            ports,
            ActivationPolicy::default(),
            Arc::new(ActivationRegistry::new()),
            Arc::clone(&drain),
        ),
        Arc::new(MuxAdmission::new(admission(&drain), bound())),
    );
    Composed {
        queue,
        store,
        drain,
        pump,
    }
}

/// The vertical run, through the composition the deployable actually builds: admission, the
/// local slot, the claim, the fold, the dispatch, the settlement and the ack.
#[tokio::test(flavor = "current_thread")]
async fn the_composed_loop_drives_a_turn_end_to_end() {
    let composed = compose_loop(Arc::new(ScriptedProvider::new(vec![
        ProviderScript::Produce(Box::new(produced())),
    ])));
    composed.queue.project(wake_for(key(), "wrk-1"));

    let report = composed.pump.poll_once().await.expect("the poll succeeds");
    assert_eq!(report.received, 1);
    assert_eq!(report.driven, 1);
    assert_eq!(report.refused, 0);
    assert_eq!(composed.store.finish(key()), Some(FinishReason::Completed));
    assert_eq!(composed.queue.acked().len(), 1);
}

#[derive(Debug)]
struct ConcurrentProvider {
    target: usize,
    active: AtomicUsize,
    maximum: AtomicUsize,
}

impl ConcurrentProvider {
    const fn new(target: usize) -> Self {
        Self {
            target,
            active: AtomicUsize::new(0),
            maximum: AtomicUsize::new(0),
        }
    }
}

impl ProviderPort for ConcurrentProvider {
    fn dispatch<'a>(
        &'a self,
        _ticket: &'a DispatchTicket,
        _credential: aex_brain_domain::wire_pending::SessionCredentialPin,
        _request: &'a CanonicalModelRequest,
        _budget: &'a StreamBudget,
        _preview: &'a dyn PreviewSink,
        _cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderDispatchError>> {
        let mut entered = false;
        Box::pin(core::future::poll_fn(move |context| {
            if !entered {
                entered = true;
                let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.maximum.fetch_max(active, Ordering::SeqCst);
            }
            if self.maximum.load(Ordering::SeqCst) >= self.target {
                self.active.fetch_sub(1, Ordering::SeqCst);
                return core::task::Poll::Ready(Ok(produced()));
            }
            context.waker().wake_by_ref();
            core::task::Poll::Pending
        }))
    }

    fn resolve_unknown<'a>(
        &'a self,
        _identity: &'a DurableEffect,
        _evidence: &'a DispatchEvidence,
    ) -> BoxFuture<'a, Result<UnknownResolution, ProviderDispatchError>> {
        Box::pin(async { Ok(UnknownResolution::NoDurableOperation) })
    }
}

/// One receive schedules all ten long-effect slots concurrently, bounded by the explicit
/// drive limit. If the loop regresses to serial execution, the first provider future never
/// observes the other nine and this test cannot complete.
#[tokio::test(flavor = "current_thread")]
async fn ten_long_effects_are_polled_concurrently_under_the_drive_bound() {
    const COUNT: usize = 10;
    const COUNT_U32: u32 = 10;
    const COUNT_U64: u64 = 10;
    let log = Arc::new(Recorder::default());
    let clock = Arc::new(FixedClock::at(START));
    let queue = Arc::new(MemoryQueue::new(Arc::clone(&log)));
    let store = Arc::new(MemoryStore::new(
        Arc::clone(&clock),
        Arc::clone(&queue),
        Arc::clone(&log),
    ));
    for ordinal in 0..COUNT {
        let ordinal = u128::try_from(ordinal).expect("fixture ordinal fits u128");
        let key = AgentKey::new(
            key().session,
            AgentId(Uuid::from_u128(0xa6e7_3000 + ordinal)),
        );
        store.seed(key, history());
        let mut wake = wake_for(key, &format!("wrk-concurrent-{ordinal}"));
        wake.id = WakeId(Uuid::from_u128(0x5000 + ordinal));
        queue.project(wake);
    }
    let provider = Arc::new(ConcurrentProvider::new(COUNT));
    let drain = Arc::new(DrainGate::new());
    let ports = Ports {
        journal: Arc::clone(&store) as Arc<_>,
        snapshots: Arc::clone(&store) as Arc<_>,
        effects: Arc::clone(&store) as Arc<_>,
        leases: Arc::clone(&store) as Arc<_>,
        wakes: Arc::clone(&queue) as Arc<_>,
        provider: Arc::clone(&provider) as Arc<_>,
        tools: Arc::new(ScriptedTools::new(Vec::new(), Vec::new())),
        hands: Arc::new(AbsentHands),
        catalog: Arc::new(FixedCatalog::with_model(capability())),
        clock,
        ids: Arc::new(CountingIds::new()),
    };
    let policy = ActivationPolicy {
        max_concurrent_drives: COUNT,
        ..ActivationPolicy::default()
    };
    let context_bytes = policy.restore_resident_bytes.saturating_mul(COUNT_U64);
    let stream_buffer_bytes = u64::try_from(policy.stream_buffer_bytes)
        .unwrap_or(u64::MAX)
        .saturating_mul(COUNT_U64);
    let permits = Arc::new(PermitSet::new(BTreeMap::from([
        (PermitKind::Activation, COUNT_U64),
        (PermitKind::ContextBytes, context_bytes),
        (PermitKind::StreamBufferBytes, stream_buffer_bytes),
        (PermitKind::ProviderStream, COUNT_U64),
        (PermitKind::HandsRpc, COUNT_U64),
    ])));
    let admission = Arc::new(Admission::new(
        AdmissionBounds {
            target: COUNT_U32,
            safety_cap: COUNT_U32,
            offered_ceiling: COUNT_U32 * 2,
        },
        permits,
        Arc::clone(&drain),
        activation_resources(&policy),
    ));
    let pump = WakeLoop::new(
        Activation::new(
            ports,
            policy,
            Arc::new(ActivationRegistry::new()),
            Arc::clone(&drain),
        ),
        Arc::new(MuxAdmission::new(admission, bound())),
    );

    let report = tokio::time::timeout(core::time::Duration::from_secs(1), pump.poll_once())
        .await
        .expect("a serial regression must fail promptly instead of waiting for a long effect")
        .expect("the bounded batch completes");
    assert_eq!(report.driven, COUNT);
    assert_eq!(provider.maximum.load(Ordering::SeqCst), COUNT);
    assert_eq!(provider.active.load(Ordering::SeqCst), 0);
}

#[derive(Debug)]
struct RefillProvider {
    slow: AgentKey,
    recovered: AgentKey,
    slow_started: AtomicBool,
    recovered_started: AtomicBool,
    active: AtomicUsize,
    maximum: AtomicUsize,
}

/// A scheduler test needs production-like sleeping without making durable timestamps depend on
/// the wall clock of the machine running the suite.
#[derive(Debug)]
struct ReactorClock {
    base: std::time::Instant,
}

impl ReactorClock {
    fn new() -> Self {
        Self {
            base: std::time::Instant::now(),
        }
    }
}

impl ClockPort for ReactorClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_millis(START)
    }

    fn steady(&self) -> SteadyInstant {
        SteadyInstant(u64::try_from(self.base.elapsed().as_millis()).unwrap_or(u64::MAX))
    }

    fn sleep(&self, duration: core::time::Duration) -> BoxFuture<'_, ()> {
        Box::pin(tokio::time::sleep(duration))
    }
}

impl ProviderPort for RefillProvider {
    fn dispatch<'a>(
        &'a self,
        ticket: &'a DispatchTicket,
        _credential: aex_brain_domain::wire_pending::SessionCredentialPin,
        _request: &'a CanonicalModelRequest,
        _budget: &'a StreamBudget,
        _preview: &'a dyn PreviewSink,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderDispatchError>> {
        let key = ticket.key();
        Box::pin(async move {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.maximum.fetch_max(active, Ordering::SeqCst);
            if key == self.slow {
                self.slow_started.store(true, Ordering::SeqCst);
            }
            if key == self.recovered {
                self.recovered_started.store(true, Ordering::SeqCst);
            }

            if key == self.slow && !cancel.is_cancelled() {
                while !cancel.is_cancelled() {
                    tokio::time::sleep(core::time::Duration::from_millis(5)).await;
                }
            }
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(produced())
        })
    }

    fn resolve_unknown<'a>(
        &'a self,
        _identity: &'a DurableEffect,
        _evidence: &'a DispatchEvidence,
    ) -> BoxFuture<'a, Result<UnknownResolution, ProviderDispatchError>> {
        Box::pin(async { Ok(UnknownResolution::NoDurableOperation) })
    }
}

/// A pending provider occupies one aggregate slot, not the whole receive loop. A completed
/// sibling refills the other slot and recovers a subsequently persisted lost hint while the
/// first effect is still live. Drain then cancels that structured child and joins the set.
#[tokio::test(flavor = "current_thread")]
#[allow(
    clippy::too_many_lines,
    reason = "the scheduler refill scenario keeps queue, recovery, and aggregate-cap evidence together"
)]
async fn the_scheduler_refills_below_the_aggregate_cap_and_keeps_due_recovery_live() {
    const CAP: usize = 2;
    let slow = AgentKey::new(key().session, AgentId(Uuid::from_u128(0xa6e7_4010)));
    let fast = AgentKey::new(key().session, AgentId(Uuid::from_u128(0xa6e7_4011)));
    let recovered = AgentKey::new(key().session, AgentId(Uuid::from_u128(0xa6e7_4012)));
    let log = Arc::new(Recorder::default());
    let store_clock = Arc::new(FixedClock::at(START));
    let queue = Arc::new(MemoryQueue::new(Arc::clone(&log)));
    let store = Arc::new(MemoryStore::new(
        Arc::clone(&store_clock),
        Arc::clone(&queue),
        Arc::clone(&log),
    ));
    for key in [slow, fast, recovered] {
        store.seed(key, history());
    }
    for (key, id, work) in [(slow, 0x6100, "wrk-slow"), (fast, 0x6101, "wrk-fast")] {
        let mut wake = wake_for(key, work);
        wake.id = WakeId(Uuid::from_u128(id));
        queue.project(wake);
    }

    let provider = Arc::new(RefillProvider {
        slow,
        recovered,
        slow_started: AtomicBool::new(false),
        recovered_started: AtomicBool::new(false),
        active: AtomicUsize::new(0),
        maximum: AtomicUsize::new(0),
    });
    let drain = Arc::new(DrainGate::new());
    let ports = Ports {
        journal: Arc::clone(&store) as Arc<_>,
        snapshots: Arc::clone(&store) as Arc<_>,
        effects: Arc::clone(&store) as Arc<_>,
        leases: Arc::clone(&store) as Arc<_>,
        wakes: Arc::clone(&queue) as Arc<_>,
        provider: Arc::clone(&provider) as Arc<_>,
        tools: Arc::new(ScriptedTools::new(Vec::new(), Vec::new())),
        hands: Arc::new(AbsentHands),
        catalog: Arc::new(FixedCatalog::with_model(capability())),
        clock: Arc::new(ReactorClock::new()),
        ids: Arc::new(CountingIds::new()),
    };
    let policy = ActivationPolicy {
        receive_batch: 1,
        max_concurrent_drives: 1,
        due_shards: 1,
        due_scan_shards_per_pass: 1,
        due_scan_page: 1,
        due_scan_interval: core::time::Duration::ZERO,
        renew_interval: core::time::Duration::from_millis(10),
        ..ActivationPolicy::default()
    };
    let context_bytes = policy
        .restore_resident_bytes
        .saturating_mul(u64::try_from(CAP).expect("the test cap fits u64"));
    let stream_buffer_bytes = u64::try_from(policy.stream_buffer_bytes)
        .unwrap_or(u64::MAX)
        .saturating_mul(u64::try_from(CAP).expect("the test cap fits u64"));
    let admission = Arc::new(Admission::new(
        AdmissionBounds {
            target: u32::try_from(CAP).expect("the test cap fits u32"),
            safety_cap: u32::try_from(CAP).expect("the test cap fits u32"),
            offered_ceiling: 4,
        },
        Arc::new(PermitSet::new(BTreeMap::from([
            (
                PermitKind::Activation,
                u64::try_from(CAP).expect("the test cap fits u64"),
            ),
            (PermitKind::ContextBytes, context_bytes),
            (PermitKind::StreamBufferBytes, stream_buffer_bytes),
            (
                PermitKind::ProviderStream,
                u64::try_from(CAP).expect("the test cap fits u64"),
            ),
            (
                PermitKind::HandsRpc,
                u64::try_from(CAP).expect("the test cap fits u64"),
            ),
        ]))),
        Arc::clone(&drain),
        activation_resources(&policy),
    ));
    let pump = WakeLoop::new(
        Activation::new(
            ports,
            policy,
            Arc::new(ActivationRegistry::new()),
            Arc::clone(&drain),
        ),
        Arc::new(MuxAdmission::new(admission, bound())),
    );
    let scheduler = tokio::spawn(crate::run_wake_scheduler(
        pump,
        Arc::clone(&drain),
        CAP,
        |_| {},
    ));

    tokio::time::timeout(core::time::Duration::from_secs(1), async {
        while !provider.slow_started.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the slow provider starts");

    let mut lost_hint = wake_for(recovered, "wrk-recovered");
    lost_hint.id = WakeId(Uuid::from_u128(0x6102));
    lost_hint.due = Some(Timestamp::from_millis(START));
    queue.persist(lost_hint, WorkShard(0));

    tokio::time::timeout(core::time::Duration::from_secs(1), async {
        while !provider.recovered_started.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("a refilled lane recovers the lost hint while its sibling is pending");
    assert_eq!(provider.maximum.load(Ordering::SeqCst), CAP);
    assert_eq!(
        provider.active.load(Ordering::SeqCst),
        1,
        "only the deliberately slow dispatch remains"
    );
    assert!(!scheduler.is_finished());

    drain.start_drain();
    tokio::time::timeout(core::time::Duration::from_secs(1), scheduler)
        .await
        .expect("drain settles and joins every scheduler lane")
        .expect("the scheduler task does not panic");
    assert_eq!(provider.active.load(Ordering::SeqCst), 0);
    assert!(drain.is_quiesced());
}

/// A draining task takes nothing off the queue, whatever else it is doing.
#[tokio::test(flavor = "current_thread")]
async fn a_draining_composition_receives_nothing() {
    let composed = compose_loop(Arc::new(ScriptedProvider::new(vec![
        ProviderScript::Produce(Box::new(produced())),
    ])));
    composed.queue.project(wake_for(key(), "wrk-1"));
    composed.drain.start_drain();

    let report = composed.pump.poll_once().await.expect("the poll succeeds");
    assert_eq!(report.received, 0, "the queue was never touched");
    assert_eq!(composed.queue.depth(), 1, "the wake is still outstanding");
    assert_eq!(composed.store.finish(key()), None);
}

/// The seven stages, in order, ending in exit. A shutdown that stalls is then reportable as
/// a stage rather than as a silence.
#[tokio::test(flavor = "current_thread")]
async fn the_drain_sequence_walks_every_stage_in_order() {
    let composition = Arc::new(
        crate::compose(&crate::Config {
            plane: "dev".to_owned(),
            region: "eu-west-1".to_owned(),
            resource: "table".to_owned(),
            wake_queue_url: "https://sqs.invalid/queue".to_owned(),
            work_table: "work".to_owned(),
            secret_custody_table: "secret-custody".to_owned(),
            secret_kms_key_arn: "arn:aws:kms:eu-west-1:123456789012:key/fixture".to_owned(),
            content_bucket: "content".to_owned(),
            content_expected_owner: "123456789012".to_owned(),
            content_kms_key_arn: "arn:aws:kms:eu-west-1:123456789012:key/content".to_owned(),
            runtime_activity_table: "runtime-activity".to_owned(),
            usage_compute_queue_url: "https://sqs.eu-west-1.amazonaws.com/1/compute".to_owned(),
            usage_storage_queue_url: "https://sqs.eu-west-1.amazonaws.com/1/storage".to_owned(),
            runtime_due_shards: 8,
            runtime_due_page: aex_runtime_control::store::PageBudget {
                max_items: 32,
                max_reads: 100,
            },
            pricing_version: "synthetic-zero-v1".to_owned(),
            budget: 4,
        })
        .expect("the candidate shape composes"),
    );
    let idle = tokio::spawn(async {});

    let performed = crate::drain_sequence(&composition, idle)
        .await
        .expect("an idle pump joins cleanly");
    assert_eq!(performed, Stage::ORDER.to_vec());
    assert!(composition.drain.is_draining());
    assert_eq!(
        composition
            .health
            .respond("GET", crate::health::READY_PATH)
            .status,
        503,
        "readiness fails first"
    );
    assert_eq!(
        composition
            .health
            .respond("GET", crate::health::LIVE_PATH)
            .status,
        200,
        "liveness is never touched: failing it kills the task along with its effects"
    );
}

struct PendingUntilDropped(Arc<AtomicBool>);

impl core::future::Future for PendingUntilDropped {
    type Output = ();

    fn poll(
        self: core::pin::Pin<&mut Self>,
        _context: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Self::Output> {
        core::task::Poll::Pending
    }
}

impl Drop for PendingUntilDropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Expiring the cooperative window aborts and joins the exact task. A dropped join handle
/// would detach this future and leave the flag false after the helper returned.
#[tokio::test(flavor = "current_thread")]
async fn an_expired_drain_window_aborts_and_joins_the_pump() {
    let dropped = Arc::new(AtomicBool::new(false));
    let pump = tokio::spawn(PendingUntilDropped(Arc::clone(&dropped)));

    crate::join_pump(pump, core::time::Duration::ZERO)
        .await
        .expect("explicit cancellation is a joined drain outcome");
    assert!(dropped.load(Ordering::SeqCst));
}

/// A provider that is pending exactly once. It stands in for provider HTTP, a tool call and
/// a durable wait alike: all three are pending between polls.
#[derive(Debug, Default)]
struct PendingProvider {
    polled: Mutex<bool>,
}

impl ProviderPort for PendingProvider {
    fn dispatch<'a>(
        &'a self,
        _ticket: &'a DispatchTicket,
        _credential: aex_brain_domain::wire_pending::SessionCredentialPin,
        _request: &'a CanonicalModelRequest,
        _budget: &'a StreamBudget,
        _preview: &'a dyn PreviewSink,
        _cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderDispatchError>> {
        Box::pin(core::future::poll_fn(move |context| {
            let mut polled = self.polled.lock().expect("not poisoned");
            if *polled {
                return core::task::Poll::Ready(Ok(produced()));
            }
            *polled = true;
            context.waker().wake_by_ref();
            core::task::Poll::Pending
        }))
    }

    fn resolve_unknown<'a>(
        &'a self,
        _identity: &'a DurableEffect,
        _evidence: &'a DispatchEvidence,
    ) -> BoxFuture<'a, Result<UnknownResolution, ProviderDispatchError>> {
        Box::pin(async { Ok(UnknownResolution::NoDurableOperation) })
    }
}

/// A thread CPU clock whose readings are a script, so the assertion is exact rather than a
/// statement about the test machine's scheduler.
#[derive(Debug)]
struct ScriptedCpu {
    readings: Mutex<VecDeque<u64>>,
    last: AtomicU64,
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
struct ScriptedPhysical;

impl PhysicalCpuSource for ScriptedPhysical {
    fn task_cpu_usec(&self) -> Result<u64, ProbeError> {
        Ok(1_000_000_000)
    }
}

fn probe_context() -> ProbeContext {
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

fn activation_key() -> ActivationKey {
    ActivationKey {
        session: UsageSessionId::parse("sess-1").expect("session"),
        agent: UsageAgentId::parse("agent-1").expect("agent"),
        activation: ActivationId::parse("act-1").expect("activation"),
        fence: 1,
    }
}

/// A11-MUX, held through the composition rather than only in the probe's own unit test.
///
/// The loop is metered as one future. The provider is pending across a scripted clock jump of
/// nearly a second, and none of that second may be attributed: the meter measures the inside
/// of a poll, and a pending future is by definition not inside one. It would be easy to
/// "improve" the composition into wall-clock accounting and never notice, which is exactly
/// why this is asserted where the composition is, not only where the meter is.
#[tokio::test(flavor = "current_thread")]
async fn time_pending_on_the_provider_creates_no_compute_fact_through_the_loop() {
    let composed = compose_loop(Arc::new(PendingProvider::default()));
    composed.queue.project(wake_for(key(), "wrk-1"));

    // Two polls of 30 and 20 microseconds with a 999-millisecond gap between them.
    let cpu = Arc::new(ScriptedCpu {
        readings: Mutex::new(VecDeque::from([100, 130, 1_000_000, 1_000_020])),
        last: AtomicU64::new(0),
    });
    let measurement = Measurement::new(1 << 20, 1 << 17, Arc::new(ScriptedPhysical))
        .expect("the envelope leaves grantable bytes")
        .with_clocks(Arc::clone(&cpu) as Arc<_>, {
            let wall: Arc<dyn aex_usage_application::probe::WallClock> =
                Arc::new(aex_usage_application::probe::SystemWallClock);
            wall
        });
    let meter = measurement.activation(activation_key(), probe_context(), Attribution::default());

    let report = composed
        .pump
        .poll_once()
        .metered(Arc::clone(&meter), Arc::clone(measurement.thread_clock()))
        .await
        .expect("the poll succeeds");

    assert_eq!(report.driven, 1, "the turn still completed");
    assert_eq!(composed.store.finish(key()), Some(FinishReason::Completed));
    assert_eq!(
        meter.attributed_us(),
        50,
        "only the two polls are charged; the 999 ms the provider was pending is not"
    );
    assert_eq!(meter.poll_count(), 2);
    assert_eq!(meter.long_poll_violations(), 0);
}

/// The loop hands an agent back rather than holding a lease indefinitely, and the wake that
/// brings it back rides inside the decision that created it.
#[tokio::test(flavor = "current_thread")]
async fn a_handed_back_agent_leaves_a_wake_the_decision_created() {
    let composed = compose_loop(Arc::new(ScriptedProvider::new(vec![
        ProviderScript::Produce(Box::new(produced())),
    ])));
    composed.queue.project(wake_for(key(), "wrk-1"));
    let outcome = composed
        .pump
        .drive(
            composed
                .queue
                .receive(1, core::time::Duration::ZERO)
                .await
                .expect("the queue answers")
                .deliveries
                .pop()
                .expect("a delivery"),
        )
        .await
        .expect("the activation runs");
    assert!(matches!(
        outcome,
        Outcome::Progressed {
            stop: Stop::Finished(FinishReason::Completed),
            ..
        }
    ));
}
