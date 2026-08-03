//! The composed loop: one turn end to end, the drain order, and A11-MUX through the
//! composition.
//!
//! The engine's own crash boundaries are asserted in `aex_brain_application::activation`.
//! What this module asserts is that the *composition* preserves them: the same in-memory
//! ports are used here, so a divergence between what the engine promises and what the mux
//! wires it to shows up as a failure rather than as a difference nobody compared.

use crate::admission::{Admission, AdmissionBounds};
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
    BoxFuture, CancelToken, DispatchTicket, PreviewSink, ProviderDispatchError, ProviderOutcome,
    ProviderPort, StreamBudget, UnknownResolution, WakeQueue as _,
};
use aex_brain_domain::budget::DimensionVector;
use aex_brain_domain::effect::{DispatchEvidence, DurableEffect};
use aex_brain_domain::ids::{
    AgentId, AgentKey, CatalogPin, ContentHash, JournalSeq, ModelSlug, SessionId, Timestamp,
};
use aex_brain_domain::journal::{FinishReason, JournalEntry, JournalRecord, MessageOrigin};
use aex_brain_domain::wire_pending::{
    AgentLimits, CanonicalBlock, CanonicalModelRequest, CompleteAssistantMessage, CompleteProof,
    ContentBlockRef, ModelCapability, NormalizedUsage, ProviderId, ProviderReceipt,
    ResolvedAgentConfig, StopReason,
};
use aex_usage_application::probe::{
    ActivationKey, ActivationScoped, CpuInstant, PhysicalCpuSource, ProbeContext, ProbeError,
    ThreadCpuClock,
};
use aex_usage_domain::fact::{Attribution, ResourceGeneration, ResourceKind};
use aex_usage_domain::wire_pending::{
    ActivationId, AgentId as UsageAgentId, OrganizationId, PricingVersion, RegionId, ServiceId,
    SessionId as UsageSessionId, WorkspaceId,
};
use aex_wire::ids::{GenerationId, PrefixedId as _, Uuid7};
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
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
    ModelSlug("deepseek-chat".to_owned())
}

fn config() -> ResolvedAgentConfig {
    ResolvedAgentConfig {
        catalog_pin: CatalogPin(ContentHash::of(b"catalog")),
        provider: ProviderId::Deepseek,
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

fn capability() -> ModelCapability {
    ModelCapability {
        provider: ProviderId::Deepseek,
        model: model(),
        context_window_tokens: 64_000,
        max_output_tokens: 4_096,
        min_cacheable_prefix_tokens: None,
        supports_tools: true,
        admitted: true,
    }
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
                        text: "summarize this".to_owned(),
                    },
                }],
                origin: MessageOrigin::Submission,
            },
        )
        .expect("the record canonicalizes"),
    ]
}

fn produced() -> ProviderOutcome {
    let blocks = vec![CanonicalBlock::Text {
        text: "here is the summary".to_owned(),
    }];
    ProviderOutcome {
        message: CompleteAssistantMessage {
            complete: CompleteProof::mint(StopReason::EndTurn, &blocks).expect("a whole message"),
            blocks,
            stop_reason: StopReason::EndTurn,
        },
        usage: NormalizedUsage::default(),
        receipt: ProviderReceipt {
            provider: ProviderId::Deepseek,
            model: model(),
            request_id: None,
            route_revision: 1,
        },
    }
}

fn bound() -> Bindings {
    Bindings {
        store: BindingState::Ready,
        provider: BindingState::Ready,
        catalog: BindingState::Ready,
        tools: BindingState::Ready,
        hands: BindingState::Ready,
    }
}

fn admission(drain: &Arc<DrainGate>) -> Arc<Admission> {
    Arc::new(Admission::new(
        AdmissionBounds {
            target: 4,
            safety_cap: 8,
            offered_ceiling: 16,
        },
        Arc::new(PermitSet::new(BTreeMap::from([
            (PermitKind::Activation, 8_u64),
            (PermitKind::ContextBytes, 64 * 1_024 * 1_024),
        ]))),
        Arc::clone(drain),
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
            budget: 4,
        })
        .expect("the candidate shape composes"),
    );
    let idle = tokio::spawn(async {});

    let performed = crate::drain_sequence(&composition, idle).await;
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
