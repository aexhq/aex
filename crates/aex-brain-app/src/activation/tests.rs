//! The wake loop's crash boundaries.
//!
//! Every test here names one way the process can die and asserts what survives it. The
//! shared shape is: run an activation, interrupt it at a specific durable point, run a second
//! activation against the state the first one left, and assert what the second one did **and
//! did not** do. "Did not" is the load-bearing half — a second dispatch of a possibly-served
//! request is invisible until the bill arrives.
//!
//! Nothing here needs an executor. Every port fixture is ready on its first poll, so
//! [`block_on`] panics rather than parking: a fixture that grew a real await point would mean
//! this module had quietly acquired a runtime it does not configure.

use super::memory::{
    AbsentHands, AlwaysAdmit, CountingIds, FixedCatalog, FixedClock, MemoryQueue, MemoryStore,
    ProviderScript, Recorder, ScriptedProvider, ScriptedTools, fixture_authority, wake_for,
};
use super::{
    Activation, ActivationError, ActivationPolicy, AdmissionControl, AdmissionDecision,
    DispatchControl, DispatchDecision, DispatchLane, Outcome, Ports, Release, RestoreBudget,
    RestoreSource, Stop, WakeLoop,
};
use crate::kernel::{
    ActivationRegistry, DrainGate, FoldCache, PermitKind, PermitSet, WarmCacheShard, WarmEntry,
};
use crate::ports::{
    BoxFuture, CancelToken, ClaimError, ClockPort as _, CommitError, ConditionFailure,
    DetachedStatus, DispatchTicket, EffectStore as _, FenceGuard, JournalCursor, JournalPage,
    LeaseStore as _, PreviewSink, ProviderDispatchError, ProviderFailureKind, ProviderOutcome,
    ProviderPort, RedactedDetail, ReleaseDisposition, StoreError, StreamBudget, ToolAdvertisement,
    ToolDispatchError, ToolOutcome, ToolResultBody, ToolRoute, UnknownResolution, WakeQueue as _,
};
use aex_brain_domain::budget::DimensionVector;
use aex_brain_domain::child::QueuedReason;
use aex_brain_domain::effect::{
    DetachedOperationRef, DispatchProof, DispatchStage, DurableEffect, EffectClass, EffectKind,
    EffectState,
};
use aex_brain_domain::fold::fold;
use aex_brain_domain::ids::{
    AgentId, AgentKey, CatalogPin, ContentHash, DetachedOperationId, EffectId, JournalSeq,
    ModelSlug, OwnerToken, SessionId, Timestamp, ToolCallId, ToolName, WakeId, WorkShard,
};
use aex_brain_domain::journal::{
    ExecutorRoute, FinishReason, JournalEntry, JournalRecord, MessageOrigin, ParkReason,
};
use aex_brain_domain::wire_pending::{
    AgentLimits, CanonicalBlock, CanonicalModelRequest, ContentBlockRef, NormalizedUsage,
    ProviderId, ResolvedAgentConfig, Role, StopReason,
};
use aex_model_catalog::canonical::{
    CanonicalToolDef, CredentialBindingRef, ProviderReceipt, ReceiptBounds, ToolChoice, seal,
};
use aex_model_catalog::document::{Capability, CapabilitySet};
use aex_model_catalog::{BoundedString, QualifiedModel, fixture};
use aex_wire::CanonicalJson;
use aex_wire::ids::{GenerationId, PrefixedId as _, ProviderCredentialId, Uuid7};
use core::future::Future as _;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use uuid::Uuid;

/// Drives a fixture future to completion.
///
/// Deliberately runtime-free: a pending poll means a fixture grew a real await point, and
/// this module must fail loudly rather than quietly depend on an executor it does not
/// configure.
fn block_on<F: core::future::Future>(future: F) -> F::Output {
    let mut future = Box::pin(future);
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
    match future.as_mut().poll(&mut context) {
        core::task::Poll::Ready(value) => value,
        core::task::Poll::Pending => panic!("an activation fixture must not need a runtime"),
    }
}

fn poll_until_ready<F: core::future::Future>(future: F) -> F::Output {
    let mut future = Box::pin(future);
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
    for _ in 0..64 {
        if let core::task::Poll::Ready(value) = future.as_mut().poll(&mut context) {
            return value;
        }
    }
    panic!("the instrumented activation did not complete after bounded cooperative polls");
}

const START: i64 = 1_767_225_600_000;

fn key() -> AgentKey {
    AgentKey::new(
        SessionId(Uuid::from_u128(0x5e55_1000)),
        AgentId(Uuid::from_u128(0xa6e7_2000)),
    )
}

fn pin() -> CatalogPin {
    capability().catalog()
}

fn model() -> ModelSlug {
    ModelSlug::truncating("deepseek-chat")
}

fn config() -> ResolvedAgentConfig {
    ResolvedAgentConfig {
        catalog_pin: pin(),
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
        limits_revision: 1,
        limits: AgentLimits {
            max_turns: 4,
            max_steps_per_turn: 8,
            turn_deadline_ms: 600_000,
            max_run_duration_ms: 3_600_000,
            max_depth: 4,
            max_fanout: 32,
        },
    }
}

fn capability() -> QualifiedModel {
    let mut entry = fixture::entry(
        ProviderId::Deepseek,
        "deepseek-chat",
        CapabilitySet::from_slice(&[Capability::Tools]),
    );
    entry.limits.context_window_tokens = 64_000;
    entry.limits.max_output_tokens = 4_096;
    entry.limits.min_cacheable_prefix_tokens = 0;
    fixture::qualified(entry)
}

/// An agent that has been started and given one user message, so a model call is owed.
fn history() -> Vec<JournalEntry> {
    let started = JournalEntry::seal(
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
    .expect("the record canonicalizes");
    let message = JournalEntry::seal(
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
    .expect("the record canonicalizes");
    vec![started, message]
}

fn journal_bytes(entries: &[JournalEntry]) -> usize {
    entries
        .iter()
        .map(|entry| {
            entry
                .record
                .canonical_bytes()
                .expect("the fixture record remains canonical")
                .len()
        })
        .sum()
}

fn journal_page(entries: Vec<JournalEntry>, next: Option<JournalSeq>) -> JournalPage {
    let hydrated_bytes = journal_bytes(&entries);
    JournalPage {
        entries,
        hydrated_bytes,
        next: next.map(|seq| {
            JournalCursor::new([("offset", seq.get().to_string())], JournalSeq::ZERO, seq)
        }),
    }
}

fn produced() -> ProviderOutcome {
    let usage = NormalizedUsage {
        input_tokens: 12,
        output_tokens: 34,
        ..NormalizedUsage::default()
    };
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

fn produced_tool_use() -> ProviderOutcome {
    let mut outcome = produced();
    let selected = capability();
    let message = seal(
        vec![CanonicalBlock::ToolUse {
            id: ToolCallId::truncating("call-1"),
            name: ToolName::parse("web_fetch").expect("tool name"),
            input: CanonicalJson::parse("{}").expect("canonical tool input"),
        }],
        StopReason::ToolUse,
        &outcome.usage,
        &selected,
    )
    .expect("a whole tool-use message");
    outcome.receipt.response_receipt = Some(message.proof.0);
    outcome.message = message;
    outcome
}

fn detached_route() -> ToolRoute {
    ToolRoute {
        name: ToolName::parse("web_fetch").expect("tool name"),
        executor: ExecutorRoute::ManagedWeb,
        class: EffectClass::DurableDetached,
        timeout_ms: 60_000,
        concurrency_weight: 1,
        manifest_digest: ContentHash::of(b"tool manifest"),
    }
}

fn detached_ref() -> DetachedOperationRef {
    DetachedOperationRef {
        id: DetachedOperationId("shared-operation-id".to_owned()),
        executor: ExecutorRoute::ManagedWeb,
    }
}

fn completed_detached_result() -> DetachedStatus {
    DetachedStatus::Completed(Box::new(ToolResultBody {
        content: Vec::new(),
        is_error: false,
        duration_ms: 10,
        executed_on: ExecutorRoute::ManagedWeb,
        checksum: ContentHash::of(b"detached result"),
    }))
}

fn retryable_query_error() -> ToolDispatchError {
    ToolDispatchError {
        stage: DispatchStage::PreDispatch,
        proof: DispatchProof::NotSent,
        retryable: true,
        detail: RedactedDetail::internal(
            ProviderFailureKind::Transport,
            "durable operation lookup is temporarily unavailable",
        ),
    }
}

fn failure(proof: DispatchProof, kind: ProviderFailureKind) -> ProviderDispatchError {
    ProviderDispatchError {
        stage: match proof {
            DispatchProof::NotSent => DispatchStage::PreDispatch,
            _ => DispatchStage::Dispatched,
        },
        proof,
        kind,
        provider_request_id: None,
        retry_after: None,
        detail: RedactedDetail::internal(kind, "the fixture refuses"),
    }
}

fn terminal_failure(kind: ProviderFailureKind) -> ProviderDispatchError {
    ProviderDispatchError {
        stage: DispatchStage::Terminal,
        proof: DispatchProof::ResponseStarted,
        kind,
        provider_request_id: None,
        retry_after: None,
        detail: RedactedDetail::internal(kind, "the provider refused definitively"),
    }
}

/// Everything one test drives.
struct Harness {
    clock: Arc<FixedClock>,
    queue: Arc<MemoryQueue>,
    store: Arc<MemoryStore>,
    provider: Arc<ScriptedProvider>,
    tools: Arc<ScriptedTools>,
    catalog: Arc<FixedCatalog>,
    ids: Arc<CountingIds>,
    log: Arc<Recorder>,
    drain: Arc<DrainGate>,
    registry: Arc<ActivationRegistry>,
    policy: ActivationPolicy,
}

impl Harness {
    fn new(script: Vec<ProviderScript>) -> Self {
        let log = Arc::new(Recorder::default());
        let clock = Arc::new(FixedClock::at(START));
        let queue = Arc::new(MemoryQueue::new(Arc::clone(&log)));
        let store = Arc::new(MemoryStore::new(
            Arc::clone(&clock),
            Arc::clone(&queue),
            Arc::clone(&log),
        ));
        store.seed(key(), history());
        Self {
            clock,
            queue,
            store,
            provider: Arc::new(ScriptedProvider::new(script)),
            tools: Arc::new(ScriptedTools::new(Vec::new(), Vec::new())),
            catalog: Arc::new(FixedCatalog::with_model(capability())),
            ids: Arc::new(CountingIds::new()),
            log,
            drain: Arc::new(DrainGate::new()),
            registry: Arc::new(ActivationRegistry::new()),
            policy: ActivationPolicy::default(),
        }
    }

    fn ports(&self) -> Ports {
        Ports {
            journal: Arc::clone(&self.store) as Arc<_>,
            effects: Arc::clone(&self.store) as Arc<_>,
            leases: Arc::clone(&self.store) as Arc<_>,
            wakes: Arc::clone(&self.queue) as Arc<_>,
            provider: Arc::clone(&self.provider) as Arc<_>,
            tools: Arc::clone(&self.tools) as Arc<_>,
            hands: Arc::new(AbsentHands) as Arc<_>,
            catalog: Arc::clone(&self.catalog) as Arc<_>,
            clock: Arc::clone(&self.clock) as Arc<_>,
            ids: Arc::clone(&self.ids) as Arc<_>,
        }
    }

    fn activation(&self) -> Activation {
        Activation::new(
            self.ports(),
            self.policy.clone(),
            Arc::clone(&self.registry),
            Arc::clone(&self.drain),
        )
    }

    fn with_tools(
        mut self,
        routes: impl IntoIterator<Item = ToolRoute>,
        script: impl IntoIterator<Item = Result<ToolOutcome, ToolDispatchError>>,
    ) -> Self {
        self.tools = Arc::new(ScriptedTools::new(routes, script));
        self
    }

    /// Projects the wake the session authority would have created, and drives it.
    fn wake(&self) {
        self.queue.project(wake_for(key(), "wrk-1"));
    }

    fn run_next(&self) -> Result<Outcome, ActivationError> {
        let delivery = block_on(self.queue.receive(1, core::time::Duration::from_secs(0)))
            .expect("the queue answers")
            .deliveries
            .pop()
            .expect("a delivery is waiting");
        block_on(self.activation().run(delivery))
    }

    fn records(&self) -> Vec<&'static str> {
        self.store
            .entries(key())
            .iter()
            .map(|entry| entry.record.kind_name())
            .collect()
    }
}

#[derive(Debug)]
struct DeferredDispatch {
    lane: DispatchLane,
    reason: QueuedReason,
}

#[derive(Debug)]
struct DisabledFoldCache;

impl FoldCache for DisabledFoldCache {
    fn load(
        &self,
        _key: &AgentKey,
        _revision: aex_brain_domain::ids::AgentRevision,
        _tick: u64,
    ) -> Option<WarmEntry> {
        None
    }

    fn store(
        &self,
        _key: AgentKey,
        _revision: aex_brain_domain::ids::AgentRevision,
        _state: &aex_brain_domain::fold::FoldState,
        _bytes: usize,
        _tick: u64,
    ) -> bool {
        false
    }

    fn remove(&self, _key: &AgentKey, _revision: aex_brain_domain::ids::AgentRevision) {}
}

impl DispatchControl for DeferredDispatch {
    fn admit(&self, lane: DispatchLane, _weight: u16) -> DispatchDecision {
        assert_eq!(lane, self.lane);
        DispatchDecision::Deferred(self.reason)
    }
}

/// The two independent post-claim reads must both be polled before either fixture can
/// complete. A sequential implementation deadlocks inside the bounded poll loop.
#[test]
fn restore_and_open_effect_reads_overlap_after_claim() {
    let harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    harness.store.require_restore_effect_overlap();
    harness.wake();
    let delivery = block_on(harness.queue.receive(1, core::time::Duration::ZERO))
        .expect("the queue answers")
        .deliveries
        .pop()
        .expect("a delivery is waiting");

    poll_until_ready(harness.activation().run(delivery)).expect("the activation runs");
    assert!(harness.store.restore_effect_overlap_observed());
}

/// A cache hit is usable only at the exact claimed revision and still proves the claimed
/// tail/hash. The second terminal delivery therefore performs no journal page read.
#[test]
fn exact_revision_fold_cache_skips_replay_without_changing_outcome() {
    let harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    let cache = Arc::new(WarmCacheShard::new(64 * 1_024 * 1_024, 16 * 1_024 * 1_024));
    let activation = harness
        .activation()
        .with_fold_cache(Arc::clone(&cache) as Arc<_>);
    harness.wake();
    let first = block_on(harness.queue.receive(1, core::time::Duration::ZERO))
        .expect("the queue answers")
        .deliveries
        .pop()
        .expect("a delivery is waiting");
    block_on(activation.run(first)).expect("the first activation populates the cache");
    let reads = harness.log.count("read_page");
    assert_eq!(cache.len(), 1);

    harness.queue.project(wake_for(key(), "wrk-cache-hit"));
    let second = block_on(harness.queue.receive(1, core::time::Duration::ZERO))
        .expect("the queue answers")
        .deliveries
        .pop()
        .expect("a second delivery is waiting");
    let outcome = block_on(activation.run(second)).expect("the cached activation runs");

    assert_eq!(outcome, Outcome::Idle);
    assert_eq!(harness.log.count("read_page"), reads);
}

/// Disabling the derived accelerator changes only latency. The same wake, authority and
/// provider result produce the same durable records and activation outcome as no cache at all.
#[test]
fn disabled_fold_cache_is_semantically_equivalent_to_no_cache() {
    let uncached = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    uncached.wake();
    let uncached_outcome = uncached.run_next().expect("the uncached activation runs");

    let disabled = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    let activation = disabled
        .activation()
        .with_fold_cache(Arc::new(DisabledFoldCache));
    disabled.wake();
    let delivery = block_on(disabled.queue.receive(1, core::time::Duration::ZERO))
        .expect("the queue answers")
        .deliveries
        .pop()
        .expect("a delivery is waiting");
    let disabled_outcome =
        block_on(activation.run(delivery)).expect("the disabled-cache activation runs");

    assert_eq!(disabled_outcome, uncached_outcome);
    assert_eq!(disabled.records(), uncached.records());
    assert_eq!(
        disabled.log.count("read_page"),
        uncached.log.count("read_page"),
        "the disabled cache follows the same authority path"
    );
}

/// Dispatch pressure is observed only after durable preparation. The activation writes a
/// typed continuation, never mints a pre-send ticket, and returns ownership immediately.
#[test]
fn provider_capacity_deferral_is_durable_and_never_crosses_pre_send() {
    let harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    let activation = harness
        .activation()
        .with_dispatch_control(Arc::new(DeferredDispatch {
            lane: DispatchLane::Provider,
            reason: QueuedReason::ProviderPermits,
        }));
    harness.wake();
    let delivery = block_on(harness.queue.receive(1, core::time::Duration::ZERO))
        .expect("the queue answers")
        .deliveries
        .pop()
        .expect("a delivery is waiting");

    let outcome = block_on(activation.run(delivery)).expect("capacity deferral is progress");
    assert_eq!(
        outcome,
        Outcome::Progressed {
            steps: 2,
            stop: Stop::HandedBack,
        }
    );
    assert_eq!(harness.log.count("mark_dispatch_started"), 0);
    assert!(harness.provider.requests().is_empty());

    let continuation = block_on(harness.queue.receive(1, core::time::Duration::ZERO))
        .expect("the queue answers")
        .deliveries
        .pop()
        .expect("the capacity continuation is projected");
    assert_eq!(
        continuation.wake.reason,
        ParkReason::AwaitingCapacity {
            reason: QueuedReason::ProviderPermits,
        }
    );
}

/// The whole vertical: one wake, one claim, one fold, one model call, one settlement, one
/// terminal, one ack.
#[test]
fn one_wake_drives_a_turn_from_claim_to_ack() {
    let harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    harness.wake();

    let outcome = harness.run_next().expect("the activation runs");
    assert_eq!(
        outcome,
        Outcome::Progressed {
            steps: 3,
            stop: Stop::Finished(FinishReason::Completed)
        }
    );
    assert_eq!(
        harness.records(),
        vec![
            "agent_started",
            "user_message",
            "effect_prepared",
            "effect_settled",
            "assistant_message",
            "agent_finished",
        ],
        "the journal records the prepare, the settlement, the message and the terminal"
    );
    assert_eq!(harness.store.finish(key()), Some(FinishReason::Completed));
    assert_eq!(harness.provider.dispatched().len(), 1);
    assert_eq!(harness.provider.credentials(), vec![config().credential]);
    let requests = harness.provider.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].hash_is_consistent().expect("canonical request"));
    assert_eq!(requests[0].messages.len(), 1);
    assert!(requests[0].tools.is_empty());
    assert_eq!(requests[0].tool_choice, ToolChoice::None);
    assert!(!requests[0].parallel_tools);
    assert_eq!(requests[0].messages[0].role, Role::User);
    assert!(matches!(
        &requests[0].messages[0].blocks[..],
        [CanonicalBlock::Text { text, .. }] if text.as_str() == "summarize this"
    ));
    assert_eq!(harness.queue.acked().len(), 1);
    assert_eq!(harness.queue.depth(), 0, "nothing was left outstanding");
}

#[test]
fn model_tool_fields_refuse_truncation_and_follow_the_model_on_parallel_emission() {
    let definition = CanonicalToolDef {
        name: aex_wire::ids::ResourceName::parse("todo_read").expect("name"),
        description: BoundedString::new("Read todo state.").expect("description"),
        input_schema: CanonicalJson::parse(
            r#"{"type":"object","additionalProperties":false,"properties":{}}"#,
        )
        .expect("schema"),
        strict: false,
    };
    // `parallel_safe: false` is the live production shape: `web_fetch` is
    // advertised on every deployed task and is neither pure nor zero-weight.
    // It bounds our own concurrency and must not reach the provider request,
    // because four of the six dialects cannot encode "one tool at a time" and
    // refuse the whole request rather than send something else.
    let advertised = ToolAdvertisement {
        definitions: vec![definition],
        parallel_safe: false,
    };

    let mut capable_entry = fixture::entry(
        ProviderId::Deepseek,
        "deepseek-chat",
        CapabilitySet::from_slice(&[Capability::Tools, Capability::ParallelTools]),
    );
    capable_entry.limits.max_tools = 1;
    let capable = fixture::qualified(capable_entry);
    let fields = super::run::model_tool_fields(&capable, advertised.clone())
        .expect("one declared tool is within the model limit");
    assert_eq!(fields.tools.len(), 1);
    assert_eq!(fields.choice, ToolChoice::Auto);
    assert!(fields.parallel);

    // The model's own declaration is the only thing that withholds it.
    let mut without_parallel_entry = fixture::entry(
        ProviderId::Deepseek,
        "deepseek-chat",
        CapabilitySet::from_slice(&[Capability::Tools]),
    );
    without_parallel_entry.limits.max_tools = 1;
    let without_parallel = fixture::qualified(without_parallel_entry);
    let fields = super::run::model_tool_fields(&without_parallel, advertised.clone())
        .expect("one declared tool is within the model limit");
    assert!(!fields.parallel);

    let mut bounded_entry = fixture::entry(
        ProviderId::Deepseek,
        "deepseek-chat",
        CapabilitySet::from_slice(&[Capability::Tools]),
    );
    bounded_entry.limits.max_tools = 0;
    let bounded = fixture::qualified(bounded_entry);
    assert!(matches!(
        super::run::model_tool_fields(&bounded, advertised),
        Err(ActivationError::ToolLimitExceeded {
            advertised: 1,
            max: 0
        })
    ));
}

#[test]
fn a_new_activation_recovers_the_executor_from_durable_effect_and_journal_state() {
    let harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced_tool_use()))])
        .with_tools(
            [detached_route()],
            [Ok(ToolOutcome::Detached {
                operation: detached_ref().id,
                poll_after: core::time::Duration::from_secs(1),
            })],
        );
    harness.tools.script_queries([Ok(DetachedStatus::Failed {
        reason: "the managed fetch failed".to_owned(),
    })]);
    harness.wake();

    let first = harness.run_next().expect("the invocation detaches");
    assert!(matches!(
        first,
        Outcome::Progressed {
            stop: Stop::Parked,
            ..
        }
    ));
    let tool_effect = harness
        .store
        .effects(key())
        .into_iter()
        .find(|effect| effect.kind == EffectKind::ToolCall)
        .expect("the detached effect is durable");
    assert_eq!(
        tool_effect
            .evidence
            .as_ref()
            .and_then(|evidence| evidence.detached_tool.clone()),
        Some(detached_ref())
    );
    assert!(harness.store.entries(key()).iter().any(|entry| {
        matches!(
            &entry.record,
            JournalRecord::WaitOpened {
                reason: ParkReason::AwaitingToolResult { operation, .. },
                ..
            } if *operation == detached_ref()
        )
    }));

    // `run_next` constructs a new Activation, modeling another mux owner after restart.
    let second = harness
        .run_next()
        .expect("the successor resolves the operation");
    assert!(matches!(
        second,
        Outcome::Progressed {
            stop: Stop::HandedBack,
            ..
        }
    ));
    assert_eq!(harness.tools.queried(), vec![detached_ref()]);
    assert!(harness.store.entries(key()).iter().any(|entry| {
        matches!(
            &entry.record,
            JournalRecord::ToolResult {
                is_error: true,
                executed_on: ExecutorRoute::ManagedWeb,
                ..
            }
        )
    }));
    assert_eq!(
        harness.queue.depth(),
        1,
        "settlement atomically creates the continuation the next model call needs"
    );
}

/// The detached operation identity and its wait are intentionally separate durable writes.
/// If the process dies between them, the next owner must reconstruct the wait from the
/// response-started effect rather than query early, dispatch twice, or wedge forever.
#[test]
fn a_crash_between_detached_operation_and_wait_is_repaired_without_redispatch() {
    let harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced_tool_use()))])
        .with_tools(
            [detached_route()],
            [Ok(ToolOutcome::Detached {
                operation: detached_ref().id,
                poll_after: core::time::Duration::from_secs(1),
            })],
        );
    harness
        .tools
        .script_queries([Ok(completed_detached_result())]);
    harness.wake();
    // Model prepare, model settlement and tool prepare commit. The following wait commit
    // dies after `mark_response_started` has already persisted the operation reference.
    harness.store.pass_commits(3);
    harness
        .store
        .fail_next_commit(CommitError::Store(StoreError::Transport {
            reason: "the task died after persisting the detached operation".to_owned(),
            retryable: true,
        }));

    let error = harness.run_next().expect_err("the wait did not commit");
    assert!(
        matches!(error, ActivationError::Commit(CommitError::Store(_))),
        "{error:?}"
    );
    assert_eq!(harness.tools.invoked().len(), 1);
    assert!(harness.tools.queried().is_empty());
    assert!(
        !harness
            .store
            .entries(key())
            .iter()
            .any(|entry| { matches!(entry.record, JournalRecord::WaitOpened { .. }) })
    );
    let tool_effect = harness
        .store
        .effects(key())
        .into_iter()
        .find(|effect| effect.kind == EffectKind::ToolCall)
        .expect("the detached tool effect is durable");
    assert_eq!(
        tool_effect
            .evidence
            .as_ref()
            .and_then(|evidence| evidence.detached_tool.clone()),
        Some(detached_ref())
    );

    let repaired = harness
        .run_next()
        .expect("the next owner restores the wait");
    assert!(matches!(
        repaired,
        Outcome::Progressed {
            stop: Stop::Parked,
            ..
        }
    ));
    assert_eq!(
        harness.tools.invoked().len(),
        1,
        "repair never dispatches a second tool call"
    );
    assert!(
        harness.tools.queried().is_empty(),
        "repair commits authority before external lookup"
    );
    assert_eq!(
        harness
            .store
            .entries(key())
            .iter()
            .filter(|entry| matches!(entry.record, JournalRecord::WaitOpened { .. }))
            .count(),
        1
    );

    harness.clock.advance(5_000);
    harness
        .run_next()
        .expect("ordinary recovery resolves the exact operation");
    assert_eq!(harness.tools.queried(), vec![detached_ref()]);
    assert_eq!(harness.tools.invoked().len(), 1);
}

#[test]
fn an_expired_detached_inter_write_repair_opens_then_closes_the_wait_atomically() {
    let harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced_tool_use()))])
        .with_tools(
            [detached_route()],
            [Ok(ToolOutcome::Detached {
                operation: detached_ref().id,
                poll_after: core::time::Duration::from_secs(1),
            })],
        );
    harness.wake();
    harness.store.pass_commits(3);
    harness
        .store
        .fail_next_commit(CommitError::Store(StoreError::Transport {
            reason: "the task died after persisting the detached operation".to_owned(),
            retryable: true,
        }));
    harness.run_next().expect_err("the wait did not commit");
    harness.clock.advance(60_000);

    let recovered = harness
        .run_next()
        .expect("expired repair settles without upstream I/O");
    assert!(matches!(
        recovered,
        Outcome::Progressed {
            stop: Stop::Finished(FinishReason::Interrupted),
            ..
        }
    ));
    assert!(harness.tools.queried().is_empty());
    assert_eq!(harness.tools.invoked().len(), 1);
    let entries = harness.store.entries(key());
    assert!(entries.windows(3).any(|records| {
        matches!(records[0].record, JournalRecord::WaitOpened { .. })
            && matches!(
                records[1].record,
                JournalRecord::WaitResolved {
                    resolution: aex_brain_domain::journal::WaitResolution::Cancelled,
                    ..
                }
            )
            && matches!(
                records[2].record,
                JournalRecord::EffectSettled {
                    outcome: aex_brain_domain::effect::SettledOutcome::OutcomeUnknown { .. },
                    ..
                }
            )
    }));
}

#[test]
fn retryable_detached_query_failure_rearms_without_settling_then_completes() {
    let harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced_tool_use()))])
        .with_tools(
            [detached_route()],
            [Ok(ToolOutcome::Detached {
                operation: detached_ref().id,
                poll_after: core::time::Duration::ZERO,
            })],
        );
    harness.tools.script_queries([
        Err(retryable_query_error()),
        Ok(completed_detached_result()),
    ]);
    harness.wake();
    harness.run_next().expect("the invocation detaches");

    harness.clock.advance(250);
    let retry = harness
        .run_next()
        .expect("propagation lag is retryable rather than unknown");
    assert!(matches!(
        retry,
        Outcome::Progressed {
            stop: Stop::Parked,
            ..
        }
    ));
    let entries = harness.store.entries(key());
    assert_eq!(
        entries
            .iter()
            .filter(|entry| matches!(entry.record, JournalRecord::WaitResolved { .. }))
            .count(),
        0,
        "the original wait remains the sole authority while lookup is retryable"
    );
    let tool_effect = harness
        .store
        .effects(key())
        .into_iter()
        .find(|effect| effect.kind == EffectKind::ToolCall)
        .expect("tool effect");
    assert!(!tool_effect.state.is_settled());

    harness.clock.advance(5_000);
    harness
        .run_next()
        .expect("the next bounded query completes");
    assert_eq!(
        harness.tools.queried(),
        vec![detached_ref(), detached_ref()]
    );
    assert!(
        harness
            .store
            .entries(key())
            .iter()
            .any(|entry| { matches!(entry.record, JournalRecord::WaitResolved { .. }) })
    );
}

#[test]
fn detached_query_stops_at_the_persisted_effect_deadline_without_network_io() {
    let harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced_tool_use()))])
        .with_tools(
            [detached_route()],
            [Ok(ToolOutcome::Detached {
                operation: detached_ref().id,
                poll_after: core::time::Duration::from_secs(1),
            })],
        );
    harness.tools.script_queries([Err(retryable_query_error())]);
    harness.wake();
    harness.run_next().expect("the invocation detaches");
    harness.clock.advance(60_000);

    let terminal = harness
        .run_next()
        .expect("the exact persisted deadline settles honestly unknown");
    assert!(matches!(
        terminal,
        Outcome::Progressed {
            stop: Stop::Finished(FinishReason::Interrupted),
            ..
        }
    ));
    assert!(
        harness.tools.queried().is_empty(),
        "expiry is checked before issuing another upstream query"
    );
    let entries = harness.store.entries(key());
    assert!(entries.windows(2).any(|pair| {
        matches!(
            pair[0].record,
            JournalRecord::WaitResolved {
                resolution: aex_brain_domain::journal::WaitResolution::Cancelled,
                ..
            }
        ) && matches!(
            pair[1].record,
            JournalRecord::EffectSettled {
                outcome: aex_brain_domain::effect::SettledOutcome::OutcomeUnknown { .. },
                ..
            }
        )
    }));
}

#[test]
fn authoritative_detached_absence_settles_unknown_immediately() {
    let harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced_tool_use()))])
        .with_tools(
            [detached_route()],
            [Ok(ToolOutcome::Detached {
                operation: detached_ref().id,
                poll_after: core::time::Duration::from_secs(1),
            })],
        );
    harness.tools.script_queries([Ok(DetachedStatus::Unknown)]);
    harness.wake();
    harness.run_next().expect("the invocation detaches");

    harness.clock.advance(1_000);
    let terminal = harness
        .run_next()
        .expect("authoritative absence settles unknown");
    assert!(matches!(
        terminal,
        Outcome::Progressed {
            stop: Stop::Finished(FinishReason::Interrupted),
            ..
        }
    ));
    assert_eq!(harness.tools.queried(), vec![detached_ref()]);
}

/// A wake's tenant is a projection hint, never authority. A forged or stale projection is
/// released before any journal read or write; the session-head workspace returned by claim
/// is the only value an activation may propagate.
#[test]
fn a_wake_for_another_tenant_is_refused_before_any_decision() {
    let harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    let mut wake = wake_for(key(), "wrk-wrong-tenant");
    wake.tenant = "ws_forged".to_owned();
    harness.queue.project(wake);

    let error = harness
        .run_next()
        .expect_err("the tenant assertion is refused");
    assert!(
        matches!(
            error,
            ActivationError::Store(StoreError::WakeTenantMismatch)
        ),
        "{error:?}"
    );
    assert_eq!(harness.log.count("commit"), 0);
    assert_eq!(harness.log.count("read_page"), 0);
    assert!(harness.provider.dispatched().is_empty());
    assert_eq!(harness.queue.depth(), 1, "the wake remains retryable");
    assert!(harness.queue.acked().is_empty());
}

/// Deleting a message is not a commit. If the ack came first, a crash between them would
/// lose the wake and the agent would sit with work owed and nothing to wake it.
#[test]
fn the_ack_happens_strictly_after_the_last_commit() {
    let harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    harness.wake();
    harness.run_next().expect("the activation runs");

    let entries = harness.log.entries();
    let ack = entries
        .iter()
        .position(|entry| entry == "ack")
        .expect("the delivery was acked");
    let last_commit = entries
        .iter()
        .rposition(|entry| entry == "commit")
        .expect("a decision committed");
    assert!(ack > last_commit, "{entries:?}");
}

/// The pre-send write is what makes this boundary observable at all. A crash after it and
/// before the settlement leaves an effect that may have been served, and the only honest
/// settlement is `OutcomeUnknown`.
#[test]
fn a_crash_after_the_pre_send_write_never_dispatches_a_second_time() {
    let harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    harness.wake();
    // The settlement commit is the one that dies: the intent committed, the request was
    // sent, the response arrived, and the process was lost before it could record either.
    harness.store.pass_commits(1);
    harness
        .store
        .fail_next_commit(CommitError::Store(StoreError::Transport {
            reason: "the task died between the dispatch and the settlement".to_owned(),
            retryable: true,
        }));

    let error = harness
        .run_next()
        .expect_err("the settlement did not commit");
    assert!(
        matches!(error, ActivationError::Commit(CommitError::Store(_))),
        "{error:?}"
    );
    assert_eq!(harness.provider.dispatched().len(), 1);
    assert_eq!(
        harness.queue.acked().len(),
        0,
        "an uncommitted decision is never acked"
    );

    // A surviving task claims the same agent and finds the effect dispatched.
    let outcome = harness.run_next().expect("recovery runs");
    assert_eq!(
        outcome,
        Outcome::Progressed {
            steps: 1,
            stop: Stop::Finished(FinishReason::Interrupted)
        }
    );
    assert_eq!(
        harness.provider.dispatched().len(),
        1,
        "a possibly-served request is never sent again"
    );
    assert_eq!(harness.store.finish(key()), Some(FinishReason::Interrupted));
    let settled = harness
        .store
        .effects(key())
        .into_iter()
        .find(|effect| matches!(effect.state, EffectState::OutcomeUnknown { .. }));
    assert!(
        settled.is_some(),
        "the effect settled unknown, not complete"
    );
}

/// The redelivery after a lost ack must not repeat the last step. It folds the journal the
/// commit already wrote, sees a terminal agent, and simply acks.
#[test]
fn a_crash_between_the_commit_and_the_ack_replays_without_a_second_generation() {
    let harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    harness.wake();
    harness.queue.fail_next_ack(StoreError::Transport {
        reason: "the task died between the commit and the ack".to_owned(),
        retryable: true,
    });

    let error = harness.run_next().expect_err("the ack did not land");
    assert!(matches!(error, ActivationError::Store(_)), "{error:?}");
    assert_eq!(harness.store.finish(key()), Some(FinishReason::Completed));
    assert_eq!(
        harness.queue.durable_depth(),
        0,
        "the source row retired in the terminal decision before the failed hint ack"
    );

    let outcome = harness.run_next().expect("the redelivery runs");
    assert_eq!(outcome, Outcome::Idle, "a terminal agent owes nothing");
    assert_eq!(
        harness.provider.dispatched().len(),
        1,
        "the turn is not run again"
    );
    assert_eq!(harness.queue.acked().len(), 1);
}

/// The final durable point is source-row retirement, not the queue ack. If that decision
/// fails, the pending due row survives and a second owner can retire it without repeating
/// any effect the journal already settled.
#[test]
fn a_crash_before_source_retirement_is_recovered_without_a_second_generation() {
    let harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    harness.wake();
    harness.store.pass_commits(3);
    harness
        .store
        .fail_next_commit(CommitError::Store(StoreError::Transport {
            reason: "the task died before retiring regional-work".to_owned(),
            retryable: true,
        }));

    let error = harness
        .run_next()
        .expect_err("the retirement decision did not commit");
    assert!(matches!(error, ActivationError::Commit(_)), "{error:?}");
    assert_eq!(harness.queue.durable_depth(), 1, "the due row survives");
    assert!(harness.queue.acked().is_empty(), "no hint was acked early");

    let outcome = harness.run_next().expect("a surviving owner retires it");
    assert_eq!(outcome, Outcome::Idle);
    assert_eq!(harness.provider.dispatched().len(), 1);
    assert_eq!(harness.queue.durable_depth(), 0);
    assert_eq!(harness.queue.acked().len(), 1);
}

/// A losing owner publishes nothing. The whole precondition set is on the commit, so the
/// loser learns it lost at exactly the moment it tries to write.
#[test]
fn a_stolen_fence_publishes_nothing_and_does_not_ack() {
    let harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    harness.wake();
    harness
        .store
        .fail_next_commit(CommitError::Condition(ConditionFailure::StaleFence));

    let error = harness.run_next().expect_err("the commit was fenced out");
    assert!(
        matches!(
            error,
            ActivationError::Commit(CommitError::Condition(ConditionFailure::StaleFence))
        ),
        "{error:?}"
    );
    assert_eq!(
        harness.records(),
        vec!["agent_started", "user_message"],
        "nothing was appended"
    );
    assert_eq!(harness.provider.dispatched().len(), 0, "no byte left");
    assert_eq!(harness.queue.acked().len(), 0);
    assert_eq!(harness.queue.depth(), 1, "the wake went back");
}

fn assert_session_head_race_blocks_dispatch(script_race: impl FnOnce(&MemoryStore)) {
    let harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    script_race(&harness.store);
    harness.wake();

    let error = harness
        .run_next()
        .expect_err("the session head moved before ticket minting");
    assert!(
        matches!(
            error,
            ActivationError::Commit(CommitError::Condition(
                ConditionFailure::CancelEpochAdvanced
            ))
        ),
        "{error:?}"
    );
    assert_eq!(
        harness.records(),
        vec!["agent_started", "user_message", "effect_prepared"],
        "the preparation committed before the race"
    );
    assert!(
        matches!(
            harness.store.effects(key())[0].state,
            EffectState::Prepared { .. }
        ),
        "a refused transaction must not move the effect"
    );
    assert!(
        harness.provider.dispatched().is_empty(),
        "no external byte may leave after the session loses authority"
    );
    assert!(
        harness.queue.acked().is_empty(),
        "the source remains durable"
    );
}

/// Cancellation can commit after `EffectPrepared`; the ticket transaction must observe
/// the advanced epoch and refuse before the provider sees a request.
#[test]
fn cancellation_between_effect_preparation_and_ticket_mint_dispatches_nothing() {
    assert_session_head_race_blocks_dispatch(MemoryStore::cancel_before_next_dispatch);
}

/// Trash/purge advances deletion authority after `EffectPrepared`; the same ticket
/// transaction must refuse before the provider sees a request.
#[test]
fn deletion_between_effect_preparation_and_ticket_mint_dispatches_nothing() {
    assert_session_head_race_blocks_dispatch(MemoryStore::delete_before_next_dispatch);
}

/// A journal with a gap does not fold, so the agent does not plan and does not act. Acting
/// on a prefix of one's own history is worse than not acting at all.
#[test]
fn a_journal_gap_does_not_fold_does_not_act_and_does_not_ack() {
    let harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    harness.wake();
    harness.store.fail_next_read(StoreError::JournalGap {
        missing: JournalSeq(1),
    });

    let error = harness.run_next().expect_err("the fold refused");
    assert!(
        matches!(error, ActivationError::Store(StoreError::JournalGap { .. })),
        "{error:?}"
    );
    assert_eq!(harness.provider.dispatched().len(), 0);
    assert_eq!(harness.queue.acked().len(), 0);
    assert_eq!(harness.queue.depth(), 1);
}

/// A service page can reach EOF before the control head it was claimed with. That is not a
/// shorter valid history: recovery and planning must see the whole authoritative journal or
/// neither may run.
#[test]
fn early_eof_is_refused_before_recovery_or_planning() {
    let harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    let entries = history();
    harness
        .store
        .page_next_read(journal_page(vec![entries[0].clone()], None));
    harness.wake();

    let error = harness.run_next().expect_err("EOF before the claimed tail");
    assert!(
        matches!(
            error,
            ActivationError::Store(StoreError::JournalTailMismatch { .. })
        ),
        "{error:?}"
    );
    assert_eq!(harness.log.count("load_open"), 0, "recovery never ran");
    assert_eq!(harness.log.count("commit"), 0, "planning never wrote");
    assert!(harness.provider.dispatched().is_empty());
    assert!(harness.queue.acked().is_empty());
}

/// Equal sequence counts are insufficient: the claimed head hash is the exact history
/// identity, so a same-sequence fork is refused before any effect recovery.
#[test]
fn an_exact_sequence_with_the_wrong_tail_hash_is_refused() {
    let harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    harness.store.set_claimed_tail(
        key(),
        Some(JournalSeq(1)),
        Some(ContentHash::of(b"another tail at the same sequence")),
    );
    harness.wake();

    let error = harness
        .run_next()
        .expect_err("the hash must equal the claimed tail");
    assert!(matches!(
        error,
        ActivationError::Store(StoreError::JournalTailMismatch {
            claimed_seq: Some(JournalSeq(1)),
            folded_seq: Some(JournalSeq(1)),
            ..
        })
    ));
    assert_eq!(harness.log.count("load_open"), 0);
    assert_eq!(harness.log.count("commit"), 0);
    assert!(harness.provider.dispatched().is_empty());
}

/// Native continuations, rather than a page's item count, carry a valid restore across as
/// many short pages as the total budget permits.
#[test]
fn a_valid_multipage_restore_reaches_the_exact_claimed_tail() {
    let mut harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    harness.policy.read.max_entries = 1;
    harness.wake();

    harness
        .run_next()
        .expect("both pages restore and the turn runs");
    assert_eq!(harness.log.count("read_page"), 2);
    assert_eq!(harness.provider.dispatched().len(), 1);
}

/// Per-page limits are not an activation limit. Three one-entry pages exceed a two-entry
/// restore ceiling even though each page is individually valid, and nothing downstream may
/// plan or dispatch from the retained prefix.
#[test]
fn multiple_pages_cannot_exceed_the_total_restore_budget() {
    let mut harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    let id = EffectId([9; 16]);
    let mut entries = history();
    entries.push(
        JournalEntry::seal(
            JournalSeq(2),
            Timestamp::from_millis(START),
            JournalRecord::EffectPrepared {
                effect: id,
                kind: EffectKind::ModelCall,
                class: EffectClass::NonReplayable,
                request_hash: ContentHash::of(b"prepared but not recoverable under this cap"),
                deadline: Timestamp::from_millis(START + 60_000),
                attempt: 1,
                reservation: Vec::new(),
            },
        )
        .expect("the third record canonicalizes"),
    );
    harness.store.seed(key(), entries);
    harness.policy.read.max_entries = 1;
    harness.policy.restore = RestoreBudget {
        max_entries: 2,
        max_bytes: usize::MAX,
    };
    harness.wake();

    let error = harness
        .run_next()
        .expect_err("the third page is beyond the activation ceiling");
    assert!(matches!(
        error,
        ActivationError::Store(StoreError::RestoreBudgetExhausted { .. })
    ));
    assert_eq!(
        harness.log.count("read_page"),
        2,
        "two bounded pages were read"
    );
    assert_eq!(harness.log.count("load_open"), 0);
    assert_eq!(harness.log.count("commit"), 0);
    assert!(harness.provider.dispatched().is_empty());
    assert!(harness.queue.acked().is_empty());
}

/// The byte dimension is cumulative too. The second page is refused against the bytes left
/// by the first, even though either page fits the ordinary per-page limit by itself.
#[test]
fn multiple_pages_cannot_exceed_the_total_restore_byte_budget() {
    let mut harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    let entries = history();
    harness.policy.read.max_entries = 1;
    harness.policy.restore = RestoreBudget {
        max_entries: entries.len(),
        max_bytes: journal_bytes(&entries).saturating_sub(1),
    };
    harness.wake();

    let error = harness
        .run_next()
        .expect_err("the second page is one byte beyond the activation ceiling");
    assert!(matches!(
        error,
        ActivationError::Store(StoreError::RestoreBudgetExhausted { .. })
    ));
    assert_eq!(harness.log.count("read_page"), 2);
    assert_eq!(harness.log.count("load_open"), 0);
    assert_eq!(harness.log.count("commit"), 0);
    assert!(harness.provider.dispatched().is_empty());
}

/// The ceiling is inclusive. A two-page history whose measured bytes and entry count equal
/// both limits is valid; using `>=` here would reject the largest safe restore.
#[test]
fn the_exact_total_restore_boundary_succeeds() {
    let mut harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    let entries = history();
    harness.policy.read.max_entries = 1;
    harness.policy.restore = RestoreBudget {
        max_entries: entries.len(),
        max_bytes: journal_bytes(&entries),
    };
    harness.wake();

    harness.run_next().expect("the inclusive boundary restores");
    assert_eq!(harness.log.count("read_page"), 2);
    assert_eq!(harness.provider.dispatched().len(), 1);
}

#[derive(Debug)]
struct ReservingAdmission {
    permits: Arc<PermitSet>,
    requested: AtomicU64,
}

impl AdmissionControl for ReservingAdmission {
    fn should_receive(&self) -> bool {
        true
    }

    fn admit(&self, restore_bytes: u64) -> AdmissionDecision {
        self.requested.store(restore_bytes, Ordering::SeqCst);
        let activation = self
            .permits
            .acquire(PermitKind::Activation, 1)
            .expect("the activation permit is available");
        let context = self
            .permits
            .acquire(PermitKind::ContextBytes, restore_bytes)
            .expect("the restore reservation is available");
        AdmissionDecision::Admitted(vec![activation, context])
    }
}

/// Restore memory is an admission reservation, not a counter callers must remember to
/// decrement. Both the activation and context permits return on every completed drive.
#[test]
fn the_restore_reservation_releases_with_the_activation() {
    let harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    let restore_bytes = harness.policy.restore_resident_bytes;
    let permits = Arc::new(PermitSet::new(BTreeMap::from([
        (PermitKind::Activation, 1),
        (PermitKind::ContextBytes, restore_bytes),
    ])));
    let admission = Arc::new(ReservingAdmission {
        permits: Arc::clone(&permits),
        requested: AtomicU64::new(0),
    });
    harness.wake();
    let delivery = block_on(harness.queue.receive(1, core::time::Duration::from_secs(0)))
        .expect("the queue answers")
        .deliveries
        .pop()
        .expect("a delivery is waiting");
    let loop_ = WakeLoop::new(harness.activation(), Arc::clone(&admission) as Arc<_>);

    block_on(loop_.drive(delivery)).expect("the activation runs");
    assert_eq!(admission.requested.load(Ordering::SeqCst), restore_bytes);
    assert_eq!(permits.held(PermitKind::Activation), 0);
    assert_eq!(permits.held(PermitKind::ContextBytes), 0);
}

/// Drain stops the loop taking new work. The delivery is released at zero visibility so a
/// surviving task takes it immediately, rather than abandoned to a timeout.
#[test]
fn a_draining_task_releases_the_delivery_rather_than_starting_it() {
    let harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    harness.wake();
    harness.drain.start_drain();

    let outcome = harness.run_next().expect("the release succeeds");
    assert_eq!(outcome, Outcome::Released(Release::Draining));
    assert_eq!(harness.log.count("claim"), 0, "the agent was never claimed");
    assert_eq!(harness.provider.dispatched().len(), 0);
    assert_eq!(harness.queue.depth(), 1);
}

/// The local slot removes duplicate work in this process. It is explicitly *not* the
/// correctness mechanism — the fence is — so the loser returns promptly instead of waiting.
#[test]
fn a_second_local_activation_for_one_agent_releases_rather_than_racing() {
    let harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    let held = harness
        .registry
        .try_activate(key())
        .expect("the first slot is free");
    harness.wake();

    let outcome = harness.run_next().expect("the release succeeds");
    assert_eq!(outcome, Outcome::Released(Release::LocallyBusy));
    assert_eq!(harness.log.count("claim"), 0);
    drop(held);
}

/// A non-terminal `PossiblySent` failure is indistinguishable from a served
/// request, so it settles unknown and the run interrupts.
#[test]
fn a_possibly_sent_request_settles_unknown_and_is_never_attempted_again() {
    let harness = Harness::new(vec![ProviderScript::Fail(Box::new(failure(
        DispatchProof::PossiblySent,
        ProviderFailureKind::ServerError,
    )))]);
    harness.wake();

    let outcome = harness.run_next().expect("the activation runs");
    assert_eq!(
        outcome,
        Outcome::Progressed {
            steps: 2,
            stop: Stop::Finished(FinishReason::Interrupted)
        }
    );
    assert_eq!(
        harness.provider.dispatched().len(),
        1,
        "a transient class does not license a retry when the proof is PossiblySent"
    );
    assert_eq!(harness.store.finish(key()), Some(FinishReason::Interrupted));
}

/// A complete provider error response proves failure even though the request
/// reached the upstream. It is terminal and non-retryable, not ambiguous.
#[test]
fn a_definitive_provider_refusal_settles_known_failure() {
    let harness = Harness::new(vec![ProviderScript::Fail(Box::new(terminal_failure(
        ProviderFailureKind::Authentication,
    )))]);
    harness.wake();

    let outcome = harness.run_next().expect("the activation runs");
    assert_eq!(
        outcome,
        Outcome::Progressed {
            steps: 2,
            stop: Stop::Finished(FinishReason::Failed)
        }
    );
    assert_eq!(harness.provider.dispatched().len(), 1);
    assert!(matches!(
        harness.store.effects(key())[0].state,
        EffectState::KnownFailure {
            stage: DispatchStage::Terminal,
            proof: DispatchProof::ResponseStarted,
        }
    ));
}

/// `NotSent` is the one value that permits another attempt, because it is the only one that
/// says the upstream cannot have seen the request.
#[test]
fn only_a_provably_unsent_request_is_attempted_again() {
    let harness = Harness::new(vec![
        ProviderScript::Fail(Box::new(failure(
            DispatchProof::NotSent,
            ProviderFailureKind::ServerError,
        ))),
        ProviderScript::Produce(Box::new(produced())),
    ]);
    harness.wake();

    let outcome = harness.run_next().expect("the activation runs");
    assert_eq!(
        outcome,
        Outcome::Progressed {
            steps: 4,
            stop: Stop::Finished(FinishReason::Completed)
        },
        "prepare, settle-and-re-prepare, settle-with-message, finish"
    );
    let dispatched = harness.provider.dispatched();
    assert_eq!(dispatched.len(), 2, "the second attempt ran");
    assert_ne!(
        dispatched[0].0, dispatched[1].0,
        "a second attempt opens a second effect rather than reusing a settled identity"
    );
    assert_eq!(harness.store.finish(key()), Some(FinishReason::Completed));
}

/// A permanent refusal ends the run `Failed` rather than being retried into a loop the
/// customer pays for.
#[test]
fn a_permanent_refusal_finishes_failed_rather_than_looping() {
    let harness = Harness::new(vec![ProviderScript::Fail(Box::new(failure(
        DispatchProof::NotSent,
        ProviderFailureKind::InvalidRequest,
    )))]);
    harness.wake();

    let outcome = harness.run_next().expect("the activation runs");
    assert_eq!(
        outcome,
        Outcome::Progressed {
            steps: 2,
            stop: Stop::Finished(FinishReason::Failed)
        }
    );
    assert_eq!(harness.provider.dispatched().len(), 1);
    assert_eq!(harness.store.finish(key()), Some(FinishReason::Failed));
}

/// Brain never creates an agent. A wake that arrives before the session authority has
/// appended `agent_started` finds nothing owed and says so, rather than inventing a journal.
#[test]
fn a_wake_for_an_unstarted_agent_is_idle_rather_than_creative() {
    let harness = Harness::new(Vec::new());
    harness.store.seed(key(), Vec::new());
    harness.wake();

    let outcome = harness.run_next().expect("the activation runs");
    assert_eq!(outcome, Outcome::Idle);
    assert!(harness.store.entries(key()).is_empty());
    assert_eq!(harness.provider.dispatched().len(), 0);
    assert_eq!(harness.queue.acked().len(), 1, "the wake is satisfied");
}

/// A prepared effect is the one unambiguous recovery case: the intent committed and nothing
/// was sent, so the same identity is dispatched under the new fence rather than a second one
/// being opened.
#[test]
fn a_prepared_effect_left_by_a_dead_owner_is_dispatched_once_under_the_new_fence() {
    let harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    harness.wake();
    let first = harness.run_next().expect("the activation runs");
    assert!(matches!(first, Outcome::Progressed { .. }));

    // Every effect the run opened is settled; none was left dangling.
    let open: Vec<DurableEffect> = harness
        .store
        .effects(key())
        .into_iter()
        .filter(|effect| !effect.state.is_settled())
        .collect();
    assert!(open.is_empty(), "{open:?}");
}

/// A dispatched effect belonging to a dead owner is classified before the planner ever runs.
/// The planner has no dispatch evidence, so letting it decide would be a guess.
#[test]
fn recovery_runs_before_the_planner_and_interrupts_a_dispatched_effect() {
    let harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    // Exactly the state a task that died mid-dispatch leaves: the intent is in the journal
    // and the effect row says a byte may have left.
    let id = aex_brain_domain::ids::EffectId([9; 16]);
    let mut journal = history();
    journal.push(
        JournalEntry::seal(
            JournalSeq(2),
            Timestamp::from_millis(START),
            JournalRecord::EffectPrepared {
                effect: id,
                kind: EffectKind::ModelCall,
                class: EffectClass::NonReplayable,
                request_hash: ContentHash::of(b"whatever the dead owner sent"),
                deadline: Timestamp::from_millis(START + 60_000),
                attempt: 1,
                reservation: Vec::new(),
            },
        )
        .expect("the record canonicalizes"),
    );
    harness.store.seed(key(), journal);
    harness.store.seed_effect(
        key(),
        DurableEffect {
            id,
            kind: EffectKind::ModelCall,
            generation: None,
            class: EffectClass::NonReplayable,
            request_hash: ContentHash::of(b"whatever the dead owner sent"),
            state: EffectState::DispatchStarted { attempt: 1 },
            deadline: Timestamp::from_millis(START + 60_000),
            evidence: None,
        },
    );
    harness.wake();

    let outcome = harness.run_next().expect("the activation runs");
    assert_eq!(
        outcome,
        Outcome::Progressed {
            steps: 1,
            stop: Stop::Finished(FinishReason::Interrupted)
        }
    );
    assert_eq!(
        harness.provider.dispatched().len(),
        0,
        "the planner never ran, so nothing was dispatched"
    );
}

/// The step bound hands the agent back rather than holding a lease indefinitely, and the
/// hand-back rides inside the decision — the only place a wake may be created.
#[test]
fn a_handed_back_activation_commits_the_wake_that_brings_it_back() {
    let mut harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    harness.policy.max_steps_per_activation = 1;
    harness.wake();

    let outcome = harness.run_next().expect("the activation runs");
    assert_eq!(
        outcome,
        Outcome::Progressed {
            steps: 3,
            stop: Stop::HandedBack
        },
        "the prepare, the settlement and the hand-back are three decisions"
    );
    assert_eq!(
        harness.queue.depth(),
        1,
        "the continuation wake was projected from the decision that created it"
    );
    assert_eq!(harness.queue.acked().len(), 1, "the delivered wake is done");
}

/// The queue is at-least-once by contract, so a batch carrying one wake twice is the normal
/// case. The second delivery finds a terminal agent and acks; it does not run the turn again.
///
/// The *concurrent* duplicate is the local slot's job and is asserted separately, because a
/// sequential loop cannot exhibit it: the first activation has already released its slot.
#[test]
fn a_batch_carrying_one_wake_twice_produces_one_generation() {
    let harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    harness.wake();
    harness.wake();
    let pump = WakeLoop::new(harness.activation(), Arc::new(AlwaysAdmit));

    let report = block_on(pump.poll_once()).expect("the poll succeeds");
    assert_eq!(report.received, 2);
    assert_eq!(report.refused, 0);
    assert_eq!(
        harness.provider.dispatched().len(),
        1,
        "one agent, one dispatch"
    );
    assert_eq!(harness.queue.acked().len(), 2, "both deliveries are done");
    assert_eq!(harness.queue.durable_depth(), 0, "one source row retired");
}

/// Decode failure belongs to one queue record, never the whole receive. Valid siblings run;
/// the malformed record is released below the threshold and acknowledged exactly when it
/// reaches the configured poison count.
#[test]
fn malformed_queue_records_are_isolated_and_obey_the_poison_threshold() {
    let mut harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    harness.policy.max_receives = 3;
    harness.wake();
    harness.queue.project_malformed(1);
    let pump = WakeLoop::new(harness.activation(), Arc::new(AlwaysAdmit));

    let first = block_on(pump.poll_once()).expect("valid siblings still run");
    assert_eq!(first.received, 2);
    assert_eq!(first.driven, 1);
    assert_eq!(first.malformed_queue, 1);
    assert_eq!(first.released, 1);
    assert_eq!(first.poisoned, 0);
    assert_eq!(harness.provider.dispatched().len(), 1);
    assert_eq!(harness.queue.malformed_released(), 1);

    let second = block_on(pump.poll_once()).expect("the second receive is still retryable");
    assert_eq!(second.malformed_queue, 1);
    assert_eq!(second.released, 1);
    assert_eq!(second.poisoned, 0);

    let third = block_on(pump.poll_once()).expect("the threshold poisons only that record");
    assert_eq!(third.malformed_queue, 1);
    assert_eq!(third.poisoned, 1);
    assert_eq!(third.released, 0);
    assert_eq!(harness.queue.malformed_acked(), 1);
}

#[derive(Debug, Default)]
struct GatedProvider {
    released: AtomicBool,
    dispatches: AtomicUsize,
}

impl ProviderPort for GatedProvider {
    fn dispatch<'a>(
        &'a self,
        _ticket: &'a DispatchTicket,
        _credential: aex_brain_domain::wire_pending::SessionCredentialPin,
        _request: &'a CanonicalModelRequest,
        _budget: &'a StreamBudget,
        _preview: &'a dyn PreviewSink,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderDispatchError>> {
        self.dispatches.fetch_add(1, Ordering::SeqCst);
        Box::pin(core::future::poll_fn(move |context| {
            if cancel.is_cancelled() {
                return core::task::Poll::Ready(Err(failure(
                    DispatchProof::PossiblySent,
                    ProviderFailureKind::ServerError,
                )));
            }
            if self.released.load(Ordering::SeqCst) {
                return core::task::Poll::Ready(Ok(produced()));
            }
            context.waker().wake_by_ref();
            core::task::Poll::Pending
        }))
    }

    fn resolve_unknown<'a>(
        &'a self,
        _identity: &'a DurableEffect,
        _evidence: &'a aex_brain_domain::effect::DispatchEvidence,
    ) -> BoxFuture<'a, Result<UnknownResolution, ProviderDispatchError>> {
        Box::pin(async { Ok(UnknownResolution::NoDurableOperation) })
    }
}

#[derive(Debug)]
struct CancelAwareProvider {
    proof: DispatchProof,
    dispatches: AtomicUsize,
}

impl CancelAwareProvider {
    const fn new(proof: DispatchProof) -> Self {
        Self {
            proof,
            dispatches: AtomicUsize::new(0),
        }
    }
}

impl ProviderPort for CancelAwareProvider {
    fn dispatch<'a>(
        &'a self,
        _ticket: &'a DispatchTicket,
        _credential: aex_brain_domain::wire_pending::SessionCredentialPin,
        _request: &'a CanonicalModelRequest,
        _budget: &'a StreamBudget,
        _preview: &'a dyn PreviewSink,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderDispatchError>> {
        self.dispatches.fetch_add(1, Ordering::SeqCst);
        Box::pin(core::future::poll_fn(move |context| {
            if cancel.is_cancelled() {
                return core::task::Poll::Ready(Err(failure(
                    self.proof,
                    ProviderFailureKind::Transport,
                )));
            }
            context.waker().wake_by_ref();
            core::task::Poll::Pending
        }))
    }

    fn resolve_unknown<'a>(
        &'a self,
        _identity: &'a DurableEffect,
        _evidence: &'a aex_brain_domain::effect::DispatchEvidence,
    ) -> BoxFuture<'a, Result<UnknownResolution, ProviderDispatchError>> {
        Box::pin(async { Ok(UnknownResolution::NoDurableOperation) })
    }
}

#[derive(Debug)]
struct DrainOnFirstNotSent {
    drain: Arc<DrainGate>,
    dispatches: AtomicUsize,
}

impl ProviderPort for DrainOnFirstNotSent {
    fn dispatch<'a>(
        &'a self,
        _ticket: &'a DispatchTicket,
        _credential: aex_brain_domain::wire_pending::SessionCredentialPin,
        _request: &'a CanonicalModelRequest,
        _budget: &'a StreamBudget,
        _preview: &'a dyn PreviewSink,
        _cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderDispatchError>> {
        if self.dispatches.fetch_add(1, Ordering::SeqCst) == 0 {
            self.drain.start_drain();
        }
        Box::pin(async {
            Err(failure(
                DispatchProof::NotSent,
                ProviderFailureKind::Transport,
            ))
        })
    }

    fn resolve_unknown<'a>(
        &'a self,
        _identity: &'a DurableEffect,
        _evidence: &'a aex_brain_domain::effect::DispatchEvidence,
    ) -> BoxFuture<'a, Result<UnknownResolution, ProviderDispatchError>> {
        Box::pin(async { Ok(UnknownResolution::NoDurableOperation) })
    }
}

/// Drain reaches an already-dispatched provider through its cancellation token. A
/// possibly-sent request settles unknown and interrupts; it is never described as a clean
/// cancellation or dispatched a second time.
#[test]
fn drain_cancels_a_pending_ambiguous_effect_and_settles_it_honestly() {
    let mut harness = Harness::new(Vec::new());
    harness.policy.renew_interval = core::time::Duration::from_secs(1);
    let provider = Arc::new(CancelAwareProvider::new(DispatchProof::PossiblySent));
    let mut ports = harness.ports();
    ports.provider = Arc::clone(&provider) as Arc<_>;
    let activation = Activation::new(
        ports,
        harness.policy.clone(),
        Arc::clone(&harness.registry),
        Arc::clone(&harness.drain),
    );
    harness.wake();
    let delivery = block_on(harness.queue.receive(1, core::time::Duration::ZERO))
        .expect("the queue answers")
        .deliveries
        .pop()
        .expect("one delivery exists");
    let mut drive = Box::pin(activation.run(delivery));
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
    assert!(drive.as_mut().poll(&mut context).is_pending());

    harness.drain.start_drain();
    let core::task::Poll::Ready(outcome) = drive.as_mut().poll(&mut context) else {
        panic!("the renewal supervisor observes drain and cancels the provider");
    };
    assert!(matches!(
        outcome,
        Ok(Outcome::Progressed {
            stop: Stop::Finished(FinishReason::Interrupted),
            ..
        })
    ));
    assert_eq!(provider.dispatches.load(Ordering::SeqCst), 1);
    assert!(harness.drain.is_quiesced());
}

/// A downstream adapter may re-arm work during drain only with `NotSent` proof. The
/// replacement and continuation wake commit together, and the draining activation never
/// dispatches that replacement itself.
#[test]
fn drain_rearms_only_a_proven_not_sent_effect_without_dispatching_the_replacement() {
    let mut harness = Harness::new(Vec::new());
    harness.policy.renew_interval = core::time::Duration::from_secs(1);
    let provider = Arc::new(CancelAwareProvider::new(DispatchProof::NotSent));
    let mut ports = harness.ports();
    ports.provider = Arc::clone(&provider) as Arc<_>;
    let activation = Activation::new(
        ports,
        harness.policy.clone(),
        Arc::clone(&harness.registry),
        Arc::clone(&harness.drain),
    );
    harness.wake();
    let delivery = block_on(harness.queue.receive(1, core::time::Duration::ZERO))
        .expect("the queue answers")
        .deliveries
        .pop()
        .expect("one delivery exists");
    let mut drive = Box::pin(activation.run(delivery));
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
    assert!(drive.as_mut().poll(&mut context).is_pending());

    harness.drain.start_drain();
    let core::task::Poll::Ready(outcome) = drive.as_mut().poll(&mut context) else {
        panic!("drain cancels and the provider proves the request was not sent");
    };
    assert!(matches!(
        outcome,
        Ok(Outcome::Progressed {
            stop: Stop::HandedBack,
            ..
        })
    ));
    assert_eq!(provider.dispatches.load(Ordering::SeqCst), 1);
    assert!(
        harness
            .store
            .effects(key())
            .iter()
            .any(|effect| matches!(effect.state, EffectState::Prepared { attempt: 2 }))
    );
    assert_eq!(harness.queue.durable_depth(), 1);
}

/// A ready adapter can observe shutdown and return in the same poll, before the renewal
/// supervisor gets control again. The session must observe the drain gate itself or it will
/// immediately dispatch the replacement it just prepared during shutdown.
#[test]
fn drain_starting_inside_a_not_sent_dispatch_never_dispatches_its_replacement() {
    let harness = Harness::new(Vec::new());
    let provider = Arc::new(DrainOnFirstNotSent {
        drain: Arc::clone(&harness.drain),
        dispatches: AtomicUsize::new(0),
    });
    let mut ports = harness.ports();
    ports.provider = Arc::clone(&provider) as Arc<_>;
    let activation = Activation::new(
        ports,
        harness.policy.clone(),
        Arc::clone(&harness.registry),
        Arc::clone(&harness.drain),
    );
    harness.wake();
    let delivery = block_on(harness.queue.receive(1, core::time::Duration::ZERO))
        .expect("the queue answers")
        .deliveries
        .pop()
        .expect("one delivery exists");

    let outcome = block_on(activation.run(delivery)).expect("NotSent can be re-armed safely");
    assert!(matches!(
        outcome,
        Outcome::Progressed {
            stop: Stop::HandedBack,
            ..
        }
    ));
    assert_eq!(provider.dispatches.load(Ordering::SeqCst), 1);
    assert!(
        harness
            .store
            .effects(key())
            .iter()
            .any(|effect| matches!(effect.state, EffectState::Prepared { attempt: 2 }))
    );
    assert_eq!(harness.queue.durable_depth(), 1);
    assert!(harness.drain.is_quiesced());
}

/// Two independent loops overlap for longer than both the original 15-second lease and the
/// test visibility window. The supervisor renews both authorities while the provider is
/// healthy, so the second loop is rejected by the durable owner and no second dispatch exists.
#[test]
fn a_long_effect_renews_lease_and_visibility_while_a_second_loop_cannot_take_ownership() {
    let mut harness = Harness::new(Vec::new());
    harness.policy.lease_ttl = core::time::Duration::from_secs(15);
    harness.policy.renew_interval = core::time::Duration::from_secs(5);
    harness.policy.visibility_timeout = core::time::Duration::from_secs(10);
    let provider = Arc::new(GatedProvider::default());
    let mut ports = harness.ports();
    ports.provider = Arc::clone(&provider) as Arc<_>;
    let first = WakeLoop::new(
        Activation::new(
            ports.clone(),
            harness.policy.clone(),
            Arc::new(ActivationRegistry::new()),
            Arc::new(DrainGate::new()),
        ),
        Arc::new(AlwaysAdmit),
    );
    let second = WakeLoop::new(
        Activation::new(
            ports,
            harness.policy.clone(),
            Arc::new(ActivationRegistry::new()),
            Arc::new(DrainGate::new()),
        ),
        Arc::new(AlwaysAdmit),
    );
    harness.wake();
    let delivery = block_on(harness.queue.receive(1, core::time::Duration::ZERO))
        .expect("the queue answers")
        .deliveries
        .pop()
        .expect("one delivery exists");
    let mut first_drive = Box::pin(first.drive(delivery.clone()));
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());

    assert!(first_drive.as_mut().poll(&mut context).is_pending());
    for _ in 0..4 {
        assert!(first_drive.as_mut().poll(&mut context).is_pending());
    }
    assert_eq!(
        harness.clock.now().millis(),
        START + 20_000,
        "the effect has crossed both the original lease TTL and visibility window"
    );
    assert_eq!(harness.log.count("renew_lease"), 4);
    assert_eq!(harness.queue.visibility_extensions(), 4);

    assert_eq!(
        block_on(second.drive(delivery.clone())).expect("the duplicate is safely released"),
        Outcome::Released(Release::HeldByOther),
        "the second process-local loop still loses at the durable lease"
    );
    assert_eq!(provider.dispatches.load(Ordering::SeqCst), 1);

    provider.released.store(true, Ordering::SeqCst);
    let core::task::Poll::Ready(first_outcome) = first_drive.as_mut().poll(&mut context) else {
        panic!("the released provider completes on its next poll");
    };
    assert!(matches!(first_outcome, Ok(Outcome::Progressed { .. })));
    assert_eq!(provider.dispatches.load(Ordering::SeqCst), 1);
}

/// If another owner advances the fence, the timer branch cancels and drops the pending
/// effect future immediately. The successor recovers the ambiguous dispatch as interrupted;
/// it never calls the provider a second time.
#[test]
fn ownership_loss_stops_the_pending_effect_and_the_successor_never_redispatches_it() {
    let mut harness = Harness::new(Vec::new());
    harness.policy.renew_interval = core::time::Duration::from_secs(5);
    let provider = Arc::new(GatedProvider::default());
    let mut ports = harness.ports();
    ports.provider = Arc::clone(&provider) as Arc<_>;
    let first = WakeLoop::new(
        Activation::new(
            ports.clone(),
            harness.policy.clone(),
            Arc::new(ActivationRegistry::new()),
            Arc::new(DrainGate::new()),
        ),
        Arc::new(AlwaysAdmit),
    );
    let second = WakeLoop::new(
        Activation::new(
            ports,
            harness.policy.clone(),
            Arc::new(ActivationRegistry::new()),
            Arc::new(DrainGate::new()),
        ),
        Arc::new(AlwaysAdmit),
    );
    harness.wake();
    let delivery = block_on(harness.queue.receive(1, core::time::Duration::ZERO))
        .expect("the queue answers")
        .deliveries
        .pop()
        .expect("one delivery exists");
    let mut first_drive = Box::pin(first.drive(delivery.clone()));
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
    assert!(first_drive.as_mut().poll(&mut context).is_pending());

    let successor = harness.store.force_takeover(
        key(),
        OwnerToken(Uuid::from_u128(0xbeef)),
        harness.policy.lease_ttl,
    );
    let core::task::Poll::Ready(lost) = first_drive.as_mut().poll(&mut context) else {
        panic!("the next renewal observes the successor fence");
    };
    assert!(matches!(
        lost,
        Err(ActivationError::Claim(ClaimError::Fenced { current }))
            if current == successor.fence
    ));
    assert_eq!(provider.dispatches.load(Ordering::SeqCst), 1);
    block_on(
        harness
            .store
            .release(successor, ReleaseDisposition::Committed),
    )
    .expect("the injected successor hands ownership to the real recovery loop");

    provider.released.store(true, Ordering::SeqCst);
    let recovered = block_on(second.drive(delivery)).expect("the successor recovers durably");
    assert_eq!(
        recovered,
        Outcome::Progressed {
            steps: 1,
            stop: Stop::Finished(FinishReason::Interrupted),
        }
    );
    assert_eq!(
        provider.dispatches.load(Ordering::SeqCst),
        1,
        "an ambiguous provider dispatch is never repeated"
    );
}

fn assert_session_authority_loss_stops_mid_effect(delete: bool) {
    let mut harness = Harness::new(Vec::new());
    harness.policy.renew_interval = core::time::Duration::from_secs(1);
    let provider = Arc::new(GatedProvider::default());
    let mut ports = harness.ports();
    ports.provider = Arc::clone(&provider) as Arc<_>;
    let activation = Activation::new(
        ports,
        harness.policy.clone(),
        Arc::new(ActivationRegistry::new()),
        Arc::new(DrainGate::new()),
    );
    harness.wake();
    let delivery = block_on(harness.queue.receive(1, core::time::Duration::ZERO))
        .expect("the queue answers")
        .deliveries
        .pop()
        .expect("one delivery exists");
    let mut first = Box::pin(activation.run(delivery));
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
    assert!(first.as_mut().poll(&mut context).is_pending());

    if delete {
        let mut authority = fixture_authority();
        authority.deletion_epoch = authority.deletion_epoch.saturating_add(1);
        harness.store.set_authority(key().session, authority);
    } else {
        harness.store.cancel_session(key());
    }
    let core::task::Poll::Ready(lost) = first.as_mut().poll(&mut context) else {
        panic!("renewal observes the moved session authority");
    };
    assert!(matches!(
        lost,
        Err(ActivationError::Claim(ClaimError::Terminal))
    ));
    assert_eq!(provider.dispatches.load(Ordering::SeqCst), 1);

    provider.released.store(true, Ordering::SeqCst);
    let successor = harness
        .run_next()
        .expect("a new claim recovers the open effect");
    assert!(matches!(
        successor,
        Outcome::Progressed {
            stop: Stop::Finished(FinishReason::Interrupted),
            ..
        }
    ));
    assert_eq!(provider.dispatches.load(Ordering::SeqCst), 1);
}

#[test]
fn renewal_observes_session_cancellation_during_a_pending_effect() {
    assert_session_authority_loss_stops_mid_effect(false);
}

#[test]
fn renewal_observes_session_deletion_during_a_pending_effect() {
    assert_session_authority_loss_stops_mid_effect(true);
}

/// A stream projection is only a hint. The bounded due-shard pass must recover the same
/// authoritative row when that hint never arrived, without fabricating an SQS receipt.
#[test]
fn a_lost_stream_hint_is_recovered_by_the_due_scan() {
    let mut harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    harness.policy.due_shards = 1;
    let mut wake = wake_for(key(), "wrk-lost-hint");
    wake.due = Some(harness.clock.now());
    harness.queue.persist(wake, WorkShard(0));
    let pump = WakeLoop::new(harness.activation(), Arc::new(AlwaysAdmit));

    let report = block_on(pump.poll_once()).expect("the backstop poll succeeds");
    assert_eq!(report.recovered, 1);
    assert_eq!(report.received, 1);
    assert_eq!(report.driven, 1);
    assert_eq!(harness.provider.dispatched().len(), 1);
    assert!(
        harness.queue.acked().is_empty(),
        "a due-scan delivery has no queue receipt to acknowledge"
    );
    assert_eq!(harness.queue.durable_depth(), 0);
}

/// The queue receive is the primary path. If it fails, no due page has been fetched and no
/// continuation can skip that page on the next pass.
#[test]
fn a_failed_sqs_receive_cannot_advance_past_a_recovered_due_page() {
    let mut harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    harness.policy.due_shards = 1;
    harness.policy.due_scan_shards_per_pass = 1;
    let mut wake = wake_for(key(), "wrk-receive-fault");
    wake.due = Some(harness.clock.now());
    harness.queue.persist(wake, WorkShard(0));
    harness.queue.fail_next_receive(StoreError::Transport {
        reason: "injected receive failure".to_owned(),
        retryable: true,
    });
    let pump = WakeLoop::new(harness.activation(), Arc::new(AlwaysAdmit));

    assert!(
        block_on(pump.poll_once()).is_err(),
        "the receive fails closed"
    );
    assert_eq!(
        harness.log.count("due_scan"),
        0,
        "a page is not read until the primary receive has succeeded"
    );
    assert_eq!(harness.queue.durable_depth(), 1);

    let recovered = block_on(pump.poll_once()).expect("the same due page remains recoverable");
    assert_eq!(recovered.recovered, 1);
    assert_eq!(recovered.driven, 1);
    assert_eq!(harness.queue.durable_depth(), 0);
}

/// A due backstop burst never consumes the SQS receive batch. Ready queue work is driven
/// first, then one bounded recovery page is admitted.
#[test]
fn a_ready_sqs_delivery_is_never_suppressed_by_the_due_backstop() {
    let mut harness = Harness::new(vec![
        ProviderScript::Produce(Box::new(produced())),
        ProviderScript::Produce(Box::new(produced())),
    ]);
    harness.policy.due_shards = 1;
    harness.policy.due_scan_shards_per_pass = 1;
    harness.wake();

    let recovered_key = AgentKey::new(key().session, AgentId(Uuid::from_u128(0xa6e7_20ff)));
    harness.store.seed(recovered_key, history());
    let mut recovered = wake_for(recovered_key, "wrk-due-beside-sqs");
    recovered.id = WakeId(Uuid::from_u128(0xff));
    recovered.due = Some(harness.clock.now());
    harness.queue.persist(recovered, WorkShard(0));
    let pump = WakeLoop::new(harness.activation(), Arc::new(AlwaysAdmit));

    let report = block_on(pump.poll_once()).expect("both delivery paths make progress");
    assert_eq!(report.received, 2);
    assert_eq!(report.recovered, 1);
    assert_eq!(report.driven, 2);
    assert!(
        harness.log.first("receive") < harness.log.first("due_scan"),
        "SQS is observed before the backstop"
    );
    assert!(
        harness.log.first("due_scan") < harness.log.first("mark_dispatch_started"),
        "due recovery is completed before either long external effect starts"
    );
    assert_eq!(harness.provider.dispatched().len(), 2);
}

/// Sixteen rotating shards every twenty seconds is a measured bound, not a scan on every
/// fast queue poll: all 64 shards are covered in four passes (80 seconds worst-case), while
/// an immediate fifth poll performs no `DynamoDB` query.
#[test]
fn due_recovery_covers_sixty_four_shards_in_four_cadenced_bursts_without_hot_polling() {
    let mut harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    harness.policy.due_shards = 64;
    harness.policy.due_scan_shards_per_pass = 16;
    harness.policy.due_scan_page = 1;
    harness.policy.due_scan_interval = core::time::Duration::from_secs(20);
    let mut wake = wake_for(key(), "wrk-last-shard");
    wake.due = Some(harness.clock.now());
    harness.queue.persist(wake, WorkShard(63));
    let pump = WakeLoop::new(harness.activation(), Arc::new(AlwaysAdmit));

    for pass in 0..4 {
        let report = block_on(pump.poll_once()).expect("the bounded burst succeeds");
        assert_eq!(harness.log.count("due_scan"), (pass + 1) * 16);
        if pass < 3 {
            assert_eq!(report.recovered, 0, "shard 63 has not been reached yet");
            harness.clock.advance(20_000);
        } else {
            assert_eq!(report.recovered, 1, "the fourth burst reaches shard 63");
        }
    }
    assert_eq!(harness.provider.dispatched().len(), 1);

    let immediate = block_on(pump.poll_once()).expect("an SQS poll still runs");
    assert_eq!(immediate.recovered, 0);
    assert_eq!(
        harness.log.count("due_scan"),
        64,
        "no time elapsed, so the backstop does not poll again"
    );
}

/// A malformed row may be isolated, but a page containing held work must retain its cursor.
/// Shard rotation is independent of that cursor, so a later shard still runs on the next
/// pass instead of one hot partition starving the whole backstop.
#[test]
fn transient_due_rows_retain_the_cursor_without_starving_later_shards() {
    let mut harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    harness.policy.due_shards = 2;
    harness.policy.due_scan_shards_per_pass = 1;
    harness.policy.receive_batch = 10;
    harness.policy.due_scan_page = 10;
    harness.policy.due_scan_interval = core::time::Duration::ZERO;

    let held = block_on(harness.store.claim(
        &key(),
        OwnerToken(Uuid::from_u128(0x1111)),
        harness.policy.lease_ttl,
        harness.clock.now(),
    ))
    .expect("the oldest agent is held for the whole test");
    assert_eq!(held.fence.0, 1);

    harness
        .queue
        .persist_malformed_due(WakeId(Uuid::from_u128(1)), WorkShard(0));
    for ordinal in 2_u128..=10 {
        let mut wake = wake_for(key(), &format!("wrk-held-{ordinal:02}"));
        wake.id = WakeId(Uuid::from_u128(ordinal));
        wake.due = Some(harness.clock.now());
        harness.queue.persist(wake, WorkShard(0));
    }

    let younger = AgentKey::new(key().session, AgentId(Uuid::from_u128(0xa6e7_2001)));
    harness.store.seed(younger, history());
    let mut younger_wake = wake_for(younger, "wrk-younger");
    younger_wake.id = WakeId(Uuid::from_u128(11));
    younger_wake.due = Some(harness.clock.now());
    harness.queue.persist(younger_wake, WorkShard(1));

    let pump = WakeLoop::new(harness.activation(), Arc::new(AlwaysAdmit));
    let first = block_on(pump.poll_once()).expect("the malformed row is isolated");
    assert_eq!(first.malformed, 1);
    assert_eq!(first.isolations.len(), 1);
    assert_eq!(first.isolations[0].fingerprint.len(), 16);
    assert_eq!(first.recovered, 9);
    assert_eq!(first.released, 9, "all nine valid old rows remain held");
    assert!(harness.provider.dispatched().is_empty());

    let second = block_on(pump.poll_once()).expect("the next shard is still scanned");
    assert_eq!(second.recovered, 1);
    assert_eq!(second.driven, 1);
    assert_eq!(harness.provider.dispatched().len(), 1);

    let revisited = block_on(pump.poll_once()).expect("the failed page is revisited");
    assert_eq!(revisited.malformed, 1);
    assert_eq!(revisited.recovered, 9);
    assert_eq!(revisited.released, 9);
}

/// `Committed`, `Parked` and `Abandoned` all end this ownership scope. A successor must be
/// able to claim at the same wall-clock instant, and a delayed release from the predecessor
/// must not clear that successor's exact fence/owner.
#[test]
fn every_completed_release_is_immediately_claimable_and_stale_release_is_harmless() {
    for disposition in [
        ReleaseDisposition::Committed,
        ReleaseDisposition::Parked,
        ReleaseDisposition::Abandoned,
    ] {
        let harness = Harness::new(Vec::new());
        let first_owner = OwnerToken(Uuid::from_u128(0x21));
        let first = block_on(harness.store.claim(
            &key(),
            first_owner,
            harness.policy.lease_ttl,
            harness.clock.now(),
        ))
        .expect("the predecessor claims");
        block_on(harness.store.release(first.clone(), disposition))
            .expect("the predecessor releases");

        let successor_owner = OwnerToken(Uuid::from_u128(0x22));
        let successor = block_on(harness.store.claim(
            &key(),
            successor_owner,
            harness.policy.lease_ttl,
            harness.clock.now(),
        ))
        .unwrap_or_else(|error| panic!("{disposition:?} was not immediately claimable: {error}"));
        assert!(successor.fence.0 > first.fence.0);

        block_on(harness.store.release(first, disposition))
            .expect("the delayed stale release is an idempotent no-op");
        let third = block_on(harness.store.claim(
            &key(),
            OwnerToken(Uuid::from_u128(0x23)),
            harness.policy.lease_ttl,
            harness.clock.now(),
        ));
        assert!(
            matches!(third, Err(ClaimError::HeldByOther { .. })),
            "a stale release cleared the successor under {disposition:?}: {third:?}"
        );
    }
}

/// A successor owns the current control row even though the prepared effect was written by
/// its predecessor. The stale and forged-owner guards fail; the live guard takes over the
/// same effect identity and mints the only dispatch ticket.
#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the takeover test keeps every stale/current authority assertion in one scenario"
)]
fn prepared_effect_takeover_requires_the_current_fence_and_owner() {
    let harness = Harness::new(Vec::new());
    let effect = EffectId([0x33; 16]);
    harness.store.seed_effect(
        key(),
        DurableEffect {
            id: effect,
            kind: EffectKind::ModelCall,
            generation: None,
            class: EffectClass::NonReplayable,
            request_hash: ContentHash::of(b"prepared by predecessor"),
            state: EffectState::Prepared { attempt: 1 },
            deadline: Timestamp::from_millis(START + 60_000),
            evidence: None,
        },
    );

    let predecessor = block_on(harness.store.claim(
        &key(),
        OwnerToken(Uuid::from_u128(0x31)),
        harness.policy.lease_ttl,
        harness.clock.now(),
    ))
    .expect("the predecessor claims");
    let stale = FenceGuard::new(
        key(),
        predecessor.owner,
        predecessor.fence,
        predecessor.head.revision,
        predecessor.head.journal_tail,
        predecessor.head.cancel_epoch,
        CancelToken::new(),
    );
    block_on(
        harness
            .store
            .release(predecessor, ReleaseDisposition::Abandoned),
    )
    .expect("the predecessor dies before dispatch");

    let successor = block_on(harness.store.claim(
        &key(),
        OwnerToken(Uuid::from_u128(0x32)),
        harness.policy.lease_ttl,
        harness.clock.now(),
    ))
    .expect("the successor takes ownership");
    let live = FenceGuard::new(
        key(),
        successor.owner,
        successor.fence,
        successor.head.revision,
        successor.head.journal_tail,
        successor.head.cancel_epoch,
        CancelToken::new(),
    );
    let forged_owner = FenceGuard::new(
        key(),
        OwnerToken(Uuid::from_u128(0x31)),
        successor.fence,
        successor.head.revision,
        successor.head.journal_tail,
        successor.head.cancel_epoch,
        CancelToken::new(),
    );

    for losing in [&stale, &forged_owner] {
        let error = block_on(harness.store.mark_dispatch_started(
            losing,
            &successor.authority,
            &effect,
            1,
            harness.clock.now(),
        ))
        .expect_err("only current control ownership may mint a ticket");
        assert!(
            matches!(error, CommitError::Condition(ConditionFailure::StaleFence)),
            "{error:?}"
        );
    }

    let mismatch = block_on(harness.store.mark_dispatch_started(
        &live,
        &successor.authority,
        &effect,
        2,
        harness.clock.now(),
    ))
    .expect_err("takeover cannot rewrite the prepared attempt");
    assert!(matches!(
        mismatch,
        CommitError::Condition(ConditionFailure::EffectStateMismatch { effect: moved })
            if moved == effect
    ));
    assert!(matches!(
        harness.store.effects(key())[0].state,
        EffectState::Prepared { attempt: 1 }
    ));

    let ticket = block_on(harness.store.mark_dispatch_started(
        &live,
        &successor.authority,
        &effect,
        1,
        harness.clock.now(),
    ))
    .expect("the live successor takes over the exact prepared attempt");
    assert_eq!(ticket.effect(), effect);
    assert_eq!(ticket.fence(), successor.fence);
    assert!(matches!(
        harness.store.effects(key())[0].state,
        EffectState::DispatchStarted { attempt: 1 }
    ));
}

/// Removing both sparse-index keys in the fenced retirement is what prevents a completed
/// source row from being rediscovered forever by the recovery sweep.
#[test]
fn a_retired_wake_never_resurrects_from_the_due_index() {
    let mut harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    harness.policy.due_shards = 1;
    let mut wake = wake_for(key(), "wrk-no-resurrection");
    wake.due = Some(harness.clock.now());
    harness.queue.project(wake);
    harness
        .run_next()
        .expect("the activation retires its source");
    let pump = WakeLoop::new(harness.activation(), Arc::new(AlwaysAdmit));

    let report = block_on(pump.poll_once()).expect("the next sweep succeeds");
    assert_eq!(report.recovered, 0);
    assert_eq!(report.received, 0);
    assert_eq!(harness.queue.durable_depth(), 0);
    assert_eq!(harness.provider.dispatched().len(), 1);
}

/// A parked agent does not append a second `wait_opened` every time it is woken. Without
/// this the journal would grow on every redelivery while the agent did nothing.
#[test]
fn a_redelivered_wake_against_a_parked_agent_appends_nothing() {
    let harness = Harness::new(Vec::new());
    let started = JournalEntry::seal(
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
    .expect("the record canonicalizes");
    harness.store.seed(key(), vec![started]);
    harness.wake();

    let first = harness.run_next().expect("the activation runs");
    assert_eq!(
        first,
        Outcome::Progressed {
            steps: 1,
            stop: Stop::Parked
        }
    );
    assert_eq!(harness.records().len(), 2, "one `wait_opened`");
    let opened = harness.store.entries(key()).into_iter().any(|entry| {
        matches!(entry.record, JournalRecord::WaitOpened { ref reason, .. }
            if *reason == ParkReason::AwaitingUserMessage)
    });
    assert!(opened);

    harness.wake();
    let second = harness.run_next().expect("the redelivery runs");
    assert_eq!(second, Outcome::Idle);
    assert_eq!(harness.records().len(), 2, "nothing more was appended");
}

fn restore_fixture(
    store: &MemoryStore,
    entries: &[JournalEntry],
    budget: RestoreBudget,
) -> Result<super::RestoredFold, ActivationError> {
    let tail = entries.last().map(|entry| entry.envelope.seq);
    let tail_hash = entries.last().map(|entry| entry.envelope.content_hash);
    block_on(super::restore::restore(
        store,
        key(),
        tail,
        tail_hash,
        crate::ports::ReadBudget {
            max_entries: 64,
            max_bytes: usize::MAX,
        },
        budget,
    ))
}

/// A cold activation has one authority: the complete bounded journal from sequence zero.
#[test]
fn cold_restore_replays_the_complete_journal_from_zero() {
    let harness = Harness::new(Vec::new());
    let entries = history();
    let restored = restore_fixture(
        &harness.store,
        &entries,
        RestoreBudget {
            max_entries: entries.len(),
            max_bytes: journal_bytes(&entries),
        },
    )
    .expect("the complete journal fits");

    assert_eq!(restored.state, fold(&entries).expect("full history folds"));
    assert_eq!(restored.source, RestoreSource::JournalFromZero);
    assert_eq!(restored.retained_bytes, journal_bytes(&entries));
}

/// Brain cannot hide an oversized cold restore behind a filesystem or object-store image.
#[test]
fn cold_restore_refuses_when_the_complete_journal_exceeds_either_bound() {
    let harness = Harness::new(Vec::new());
    let entries = history();
    assert!(matches!(
        restore_fixture(
            &harness.store,
            &entries,
            RestoreBudget {
                max_entries: 1,
                max_bytes: usize::MAX,
            },
        ),
        Err(ActivationError::Store(
            StoreError::RestoreBudgetExhausted { .. }
        ))
    ));
    assert!(matches!(
        restore_fixture(
            &harness.store,
            &entries,
            RestoreBudget {
                max_entries: entries.len(),
                max_bytes: journal_bytes(&entries).saturating_sub(1),
            },
        ),
        Err(ActivationError::Store(
            StoreError::RestoreBudgetExhausted { .. }
        ))
    ));
}

/// A historical claim cannot make later authoritative journal rows disappear.
#[test]
fn journal_rows_beyond_the_claim_never_reach_planning() {
    let harness = Harness::new(Vec::new());
    let entries = history();
    let claimed = &entries[..1];

    let error = block_on(super::restore::restore(
        harness.store.as_ref(),
        key(),
        claimed.last().map(|entry| entry.envelope.seq),
        claimed.last().map(|entry| entry.envelope.content_hash),
        crate::ports::ReadBudget {
            max_entries: 64,
            max_bytes: usize::MAX,
        },
        RestoreBudget {
            max_entries: entries.len(),
            max_bytes: usize::MAX,
        },
    ))
    .expect_err("the complete journal disagrees with the historical claim");
    assert!(matches!(
        error,
        ActivationError::Store(StoreError::JournalTailMismatch { .. })
    ));
}

/// Same sequence is insufficient authority: the claimed hash must equal the journal hash.
#[test]
fn claimed_hash_fork_never_reaches_planning() {
    let harness = Harness::new(Vec::new());
    let entries = history();
    let tail = entries.last().expect("history is non-empty");

    let error = block_on(super::restore::restore(
        harness.store.as_ref(),
        key(),
        Some(tail.envelope.seq),
        Some(ContentHash([0xaa; 32])),
        crate::ports::ReadBudget {
            max_entries: 64,
            max_bytes: usize::MAX,
        },
        RestoreBudget {
            max_entries: entries.len(),
            max_bytes: usize::MAX,
        },
    ))
    .expect_err("same-sequence different-hash authority must refuse");
    assert!(matches!(
        error,
        ActivationError::Store(StoreError::JournalTailMismatch { .. })
    ));
}
