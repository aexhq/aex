//! In-memory ports, for asserting the loop without a service.
//!
//! These are **fixtures, not stubs**. Every one refuses rather than invents: the provider
//! never fabricates a generation, the store enforces the whole precondition set, and the
//! queue is at-least-once with a real receive count. A fixture that answered optimistically
//! would make the crash-boundary assertions vacuous, which is the one thing they cannot be.
//!
//! Available to this crate's own tests and, behind the `testing` feature, to the composition
//! root's — so the mux's wiring is asserted against the same behaviour the engine is.

#![allow(
    clippy::missing_panics_doc,
    reason = "every panic here is a poisoned fixture lock, which can only happen when a test has already failed while holding it; documenting each one would be thirty copies of that sentence"
)]

use super::{AdmissionControl, AdmissionDecision};
use crate::ports::{
    AgentHead, BoxFuture, CancelToken, CatalogDigest, CatalogError, CatalogPort, Claim, ClaimError,
    ClockPort, CommitError, CommitReceipt, ConditionFailure, DecisionContext, DetachedStatus,
    DispatchTicket, DueRowIsolation, DueRowIsolationReason, DueScanCursor, DueScanPage,
    DurableWake, EffectStore, FenceGuard, HandsAccepted, HandsEndpoint, HandsError,
    HandsOperationStart, HandsOperationStatus, HandsPort, IdPort, JournalCursor, JournalPage,
    JournalStore, LeaseStore, MAX_DUE_ROW_ISOLATIONS, MalformedWakeDelivery, MalformedWakeReason,
    PreparedToolCall, PreviewSink, ProviderDispatchError, ProviderOutcome, ProviderPort,
    ReadBudget, RedactedDetail, ReleaseDisposition, ResultBounds, SessionAuthority, SteadyInstant,
    StoreError, StreamBudget, ToolAdvertisement, ToolDispatchError, ToolOutcome, ToolPort,
    ToolRoute, ToolRoutingError, WakeBatch, WakeDelivery, WakeOrigin, WakeQueue, WakeState,
};
use aex_brain_domain::budget::BudgetNode;
use aex_brain_domain::commit::{DecisionCommit, EffectWrite};
use aex_brain_domain::effect::{
    DetachedOperationRef, DispatchEvidence, DurableEffect, EffectState, SettledOutcome,
};
use aex_brain_domain::ids::{
    AgentId, AgentKey, AgentRevision, CancelEpoch, CatalogPin, ContentHash, DetachedOperationId,
    EffectId, Fence, HandsOperationId, JournalSeq, ModelSlug, OwnerToken, SessionId, Timestamp,
    ToolName, WakeId, WorkShard,
};
use aex_brain_domain::journal::{FinishReason, JournalEntry, ParkReason};
use aex_brain_domain::wire_pending::{CanonicalModelRequest, DurableOperationSupport, ProviderId};
use aex_model_catalog::{CatalogError as ModelCatalogError, QualifiedModel};
use aex_wire::ids::{GenerationId, PrefixedId, Uuid7};
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicI64, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// An ordered record of what the ports were asked to do.
///
/// The order is itself the assertion in several tests: the ack has to come strictly after
/// the commit, and a log is the only way to state that without reaching inside the loop.
#[derive(Debug, Default)]
pub struct Recorder {
    entries: Mutex<Vec<String>>,
}

impl Recorder {
    /// Records one call.
    pub fn note(&self, what: impl Into<String>) {
        self.entries
            .lock()
            .expect("the recorder lock is not poisoned")
            .push(what.into());
    }

    /// Everything recorded, in order.
    #[must_use]
    pub fn entries(&self) -> Vec<String> {
        self.entries
            .lock()
            .expect("the recorder lock is not poisoned")
            .clone()
    }

    /// How many calls of `what` were recorded.
    #[must_use]
    pub fn count(&self, what: &str) -> usize {
        self.entries()
            .iter()
            .filter(|entry| entry.as_str() == what)
            .count()
    }

    /// The index of the first `what`, if any.
    #[must_use]
    pub fn first(&self, what: &str) -> Option<usize> {
        self.entries().iter().position(|entry| entry == what)
    }
}

/// A clock that only moves when a test moves it.
///
/// A sampled clock would make every deadline assertion a statement about the test machine's
/// scheduler rather than about the rule under test.
#[derive(Debug)]
pub struct FixedClock {
    millis: AtomicI64,
}

impl FixedClock {
    /// A clock reading `millis`.
    #[must_use]
    pub const fn at(millis: i64) -> Self {
        Self {
            millis: AtomicI64::new(millis),
        }
    }

    /// Moves the clock forward.
    pub fn advance(&self, millis: i64) {
        self.millis.fetch_add(millis, Ordering::SeqCst);
    }
}

impl ClockPort for FixedClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_millis(self.millis.load(Ordering::SeqCst))
    }

    fn steady(&self) -> SteadyInstant {
        SteadyInstant(u64::try_from(self.millis.load(Ordering::SeqCst)).unwrap_or(0))
    }

    fn sleep(&self, duration: core::time::Duration) -> BoxFuture<'_, ()> {
        Box::pin(FixedSleep {
            clock: self,
            duration,
            armed: false,
        })
    }
}

struct FixedSleep<'clock> {
    clock: &'clock FixedClock,
    duration: core::time::Duration,
    armed: bool,
}

impl core::future::Future for FixedSleep<'_> {
    type Output = ();

    fn poll(
        mut self: core::pin::Pin<&mut Self>,
        context: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Self::Output> {
        if self.armed {
            self.clock
                .advance(i64::try_from(self.duration.as_millis()).unwrap_or(i64::MAX));
            return core::task::Poll::Ready(());
        }
        self.armed = true;
        context.waker().wake_by_ref();
        core::task::Poll::Pending
    }
}

/// Identifiers that are deterministic where the contract says so and counted where it does
/// not, so a test can name every value the loop produced.
#[derive(Debug, Default)]
pub struct CountingIds {
    next: AtomicU64,
}

impl CountingIds {
    /// A fresh generator.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn take(&self) -> u128 {
        u128::from(self.next.fetch_add(1, Ordering::SeqCst)) + 1
    }
}

impl IdPort for CountingIds {
    fn child_agent_id(&self, parent: &AgentId, ordinal: u32) -> AgentId {
        aex_brain_domain::ids::child_agent_id(*parent, ordinal)
    }

    fn effect_id(
        &self,
        agent: &AgentId,
        seq: JournalSeq,
        kind: aex_brain_domain::EffectKind,
    ) -> EffectId {
        EffectId::derive(*agent, seq, kind.tag())
    }

    fn owner_token(&self) -> OwnerToken {
        OwnerToken(Uuid::from_u128(self.take()))
    }

    fn wake_id(&self) -> WakeId {
        WakeId(Uuid::from_u128(self.take()))
    }

    fn operation_id(&self) -> DetachedOperationId {
        DetachedOperationId(format!("op-{}", self.take()))
    }
}

/// A catalog holding exactly the entries a test installed.
#[derive(Debug)]
pub struct FixedCatalog {
    models: Mutex<BTreeMap<(ProviderId, String), QualifiedModel>>,
    support: Mutex<DurableOperationSupport>,
}

impl FixedCatalog {
    /// A catalog with one admitted model.
    #[must_use]
    pub fn with_model(model: QualifiedModel) -> Self {
        let catalog = Self {
            models: Mutex::new(BTreeMap::new()),
            support: Mutex::new(DurableOperationSupport::None),
        };
        catalog
            .models
            .lock()
            .expect("not poisoned")
            .insert((model.provider(), model.model().as_str().to_owned()), model);
        catalog
    }

    /// Declares that this catalog's models expose a proven durable operation.
    pub fn prove_durable_operations(&self) {
        *self.support.lock().expect("not poisoned") =
            DurableOperationSupport::ResultLookup { ttl_ms: 60_000 };
    }
}

impl CatalogPort for FixedCatalog {
    fn digest(&self, pin: &CatalogPin) -> Result<CatalogDigest, CatalogError> {
        Ok(CatalogDigest(pin.0))
    }

    fn model(
        &self,
        pin: &CatalogPin,
        provider: ProviderId,
        model: &ModelSlug,
    ) -> Result<QualifiedModel, CatalogError> {
        self.models
            .lock()
            .expect("not poisoned")
            .get(&(provider, model.as_str().to_owned()))
            .cloned()
            .filter(|qualified| qualified.catalog() == *pin)
            .ok_or_else(|| {
                CatalogError::Lookup(ModelCatalogError::UnknownModel {
                    provider,
                    model: model.clone(),
                })
            })
    }

    fn durable_operation_support(
        &self,
        _pin: &CatalogPin,
        _provider: ProviderId,
        _model: &ModelSlug,
    ) -> DurableOperationSupport {
        *self.support.lock().expect("not poisoned")
    }
}

/// What a scripted provider does on its next dispatch.
#[derive(Debug)]
pub enum ProviderScript {
    /// Produce this outcome.
    Produce(Box<ProviderOutcome>),
    /// Fail with this typed error.
    Fail(Box<ProviderDispatchError>),
}

/// A provider that answers from a script and records every ticket it was handed.
///
/// It has no default answer: an exhausted script fails `PossiblySent`, because a fixture
/// that quietly produced another generation would let a double-dispatch pass unnoticed.
#[derive(Debug, Default)]
pub struct ScriptedProvider {
    script: Mutex<VecDeque<ProviderScript>>,
    dispatched: Mutex<Vec<(EffectId, Fence, u16)>>,
    credentials: Mutex<Vec<aex_brain_domain::wire_pending::SessionCredentialPin>>,
    requests: Mutex<Vec<CanonicalModelRequest>>,
}

impl ScriptedProvider {
    /// A provider that will answer `script`, in order.
    #[must_use]
    pub fn new(script: impl IntoIterator<Item = ProviderScript>) -> Self {
        Self {
            script: Mutex::new(script.into_iter().collect()),
            dispatched: Mutex::new(Vec::new()),
            credentials: Mutex::new(Vec::new()),
            requests: Mutex::new(Vec::new()),
        }
    }

    /// Every dispatch it was asked to perform.
    #[must_use]
    pub fn dispatched(&self) -> Vec<(EffectId, Fence, u16)> {
        self.dispatched.lock().expect("not poisoned").clone()
    }

    /// Every canonical request it was asked to perform.
    #[must_use]
    pub fn requests(&self) -> Vec<CanonicalModelRequest> {
        self.requests.lock().expect("not poisoned").clone()
    }

    /// Every immutable credential pin it was asked to dispatch under.
    #[must_use]
    pub fn credentials(&self) -> Vec<aex_brain_domain::wire_pending::SessionCredentialPin> {
        self.credentials.lock().expect("not poisoned").clone()
    }
}

impl ProviderPort for ScriptedProvider {
    fn dispatch<'a>(
        &'a self,
        ticket: &'a DispatchTicket,
        credential: aex_brain_domain::wire_pending::SessionCredentialPin,
        request: &'a CanonicalModelRequest,
        _budget: &'a StreamBudget,
        _preview: &'a dyn PreviewSink,
        _cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderDispatchError>> {
        Box::pin(async move {
            self.credentials
                .lock()
                .expect("not poisoned")
                .push(credential);
            self.requests
                .lock()
                .expect("not poisoned")
                .push(request.clone());
            self.dispatched.lock().expect("not poisoned").push((
                ticket.effect(),
                ticket.fence(),
                ticket.attempt(),
            ));
            match self.script.lock().expect("not poisoned").pop_front() {
                Some(ProviderScript::Produce(outcome)) => Ok(*outcome),
                Some(ProviderScript::Fail(error)) => Err(*error),
                None => Err(ProviderDispatchError {
                    stage: aex_brain_domain::effect::DispatchStage::Dispatched,
                    proof: aex_brain_domain::effect::DispatchProof::PossiblySent,
                    kind: crate::ports::ProviderFailureKind::ServerError,
                    provider_request_id: None,
                    retry_after: None,
                    detail: RedactedDetail::internal(
                        crate::ports::ProviderFailureKind::ServerError,
                        "the script is exhausted",
                    ),
                }),
            }
        })
    }

    fn resolve_unknown<'a>(
        &'a self,
        _identity: &'a DurableEffect,
        _evidence: &'a DispatchEvidence,
    ) -> BoxFuture<'a, Result<crate::ports::UnknownResolution, ProviderDispatchError>> {
        Box::pin(async { Ok(crate::ports::UnknownResolution::NoDurableOperation) })
    }
}

/// A tool port with a fixed route table and a scripted answer per invocation.
#[derive(Debug, Default)]
pub struct ScriptedTools {
    routes: Mutex<BTreeMap<String, ToolRoute>>,
    advertisement: ToolAdvertisement,
    invocations: Mutex<VecDeque<Result<ToolOutcome, ToolDispatchError>>>,
    queries: Mutex<VecDeque<Result<DetachedStatus, ToolDispatchError>>>,
    invoked: Mutex<Vec<String>>,
    queried: Mutex<Vec<DetachedOperationRef>>,
    cancelled: Mutex<Vec<(DetachedOperationRef, Fence)>>,
}

impl ScriptedTools {
    /// A router that knows `route` and will answer `script`, in order.
    #[must_use]
    pub fn new(
        routes: impl IntoIterator<Item = ToolRoute>,
        script: impl IntoIterator<Item = Result<ToolOutcome, ToolDispatchError>>,
    ) -> Self {
        let routes = routes.into_iter().collect::<Vec<_>>();
        let mut definitions = routes
            .iter()
            .map(|route| aex_model_catalog::canonical::CanonicalToolDef {
                name: route.name.clone(),
                description: aex_model_catalog::BoundedString::new("activation fixture tool")
                    .expect("bounded fixture description"),
                input_schema: aex_wire::CanonicalJson::parse(
                    r#"{"type":"object","additionalProperties":true}"#,
                )
                .expect("canonical fixture schema"),
                strict: false,
            })
            .collect::<Vec<_>>();
        definitions.sort_by(|left, right| left.name.cmp(&right.name));
        Self {
            routes: Mutex::new(
                routes
                    .into_iter()
                    .map(|route| (route.name.as_str().to_owned(), route))
                    .collect(),
            ),
            advertisement: ToolAdvertisement {
                definitions,
                parallel_safe: false,
            },
            invocations: Mutex::new(script.into_iter().collect()),
            queries: Mutex::new(VecDeque::new()),
            invoked: Mutex::new(Vec::new()),
            queried: Mutex::new(Vec::new()),
            cancelled: Mutex::new(Vec::new()),
        }
    }

    /// Scripts what a durable-operation query answers, in order.
    pub fn script_queries(
        &self,
        script: impl IntoIterator<Item = Result<DetachedStatus, ToolDispatchError>>,
    ) {
        *self.queries.lock().expect("not poisoned") = script.into_iter().collect();
    }

    /// Every call it was asked to invoke.
    #[must_use]
    pub fn invoked(&self) -> Vec<String> {
        self.invoked.lock().expect("not poisoned").clone()
    }

    /// Every executor-bound durable operation it was asked to query.
    #[must_use]
    pub fn queried(&self) -> Vec<DetachedOperationRef> {
        self.queried.lock().expect("not poisoned").clone()
    }

    /// Every executor-bound durable operation it was asked to cancel.
    #[must_use]
    pub fn cancelled(&self) -> Vec<(DetachedOperationRef, Fence)> {
        self.cancelled.lock().expect("not poisoned").clone()
    }
}

impl ToolPort for ScriptedTools {
    fn advertise(&self, _pin: &CatalogPin) -> Result<ToolAdvertisement, ToolRoutingError> {
        Ok(self.advertisement.clone())
    }

    fn route(&self, pin: &CatalogPin, name: &ToolName) -> Result<ToolRoute, ToolRoutingError> {
        self.routes
            .lock()
            .expect("not poisoned")
            .get(name.as_str())
            .cloned()
            .ok_or_else(|| {
                let _ = pin;
                ToolRoutingError::Unknown {
                    name: name.as_str().to_owned(),
                }
            })
    }

    fn invoke<'a>(
        &'a self,
        _ticket: &'a DispatchTicket,
        call: &'a PreparedToolCall,
        _cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<ToolOutcome, ToolDispatchError>> {
        Box::pin(async move {
            self.invoked
                .lock()
                .expect("not poisoned")
                .push(call.call.as_str().to_owned());
            self.invocations
                .lock()
                .expect("not poisoned")
                .pop_front()
                .unwrap_or_else(|| {
                    Err(ToolDispatchError {
                        stage: aex_brain_domain::effect::DispatchStage::Dispatched,
                        proof: aex_brain_domain::effect::DispatchProof::PossiblySent,
                        retryable: false,
                        detail: RedactedDetail::internal(
                            crate::ports::ProviderFailureKind::ServerError,
                            "the script is exhausted",
                        ),
                    })
                })
        })
    }

    fn query<'a>(
        &'a self,
        operation: &'a DetachedOperationRef,
    ) -> BoxFuture<'a, Result<DetachedStatus, ToolDispatchError>> {
        Box::pin(async move {
            self.queried
                .lock()
                .expect("not poisoned")
                .push(operation.clone());
            self.queries
                .lock()
                .expect("not poisoned")
                .pop_front()
                .unwrap_or(Ok(DetachedStatus::Unknown))
        })
    }

    fn cancel<'a>(
        &'a self,
        operation: &'a DetachedOperationRef,
        fence: Fence,
    ) -> BoxFuture<'a, Result<(), ToolDispatchError>> {
        Box::pin(async move {
            self.cancelled
                .lock()
                .expect("not poisoned")
                .push((operation.clone(), fence));
            Ok(())
        })
    }
}

/// A deliberately unavailable Hands port for activation tests.
///
/// Every method fails closed and names the missing implementation. It is not a stub: it
/// answers nothing, proves nothing, and a run that needs it interrupts honestly rather than
/// proceeding on a fabricated result.
#[derive(Debug, Clone, Copy, Default)]
pub struct AbsentHands;

impl AbsentHands {
    fn refusal() -> HandsError {
        HandsError::Transport {
            stage: aex_brain_domain::effect::DispatchStage::PreDispatch,
            proof: aex_brain_domain::effect::DispatchProof::NotSent,
            detail: RedactedDetail::internal(
                crate::ports::ProviderFailureKind::ServerError,
                "the activation test did not bind a Hands peer",
            ),
        }
    }
}

impl HandsPort for AbsentHands {
    fn ensure_generation<'a>(
        &'a self,
        _session: &'a SessionId,
        _generation: GenerationId,
    ) -> BoxFuture<'a, Result<HandsEndpoint, HandsError>> {
        Box::pin(async { Err(Self::refusal()) })
    }

    fn start<'a>(
        &'a self,
        _ticket: &'a DispatchTicket,
        _generation: GenerationId,
        _start: &'a HandsOperationStart,
    ) -> BoxFuture<'a, Result<HandsAccepted, HandsError>> {
        Box::pin(async { Err(Self::refusal()) })
    }

    fn status<'a>(
        &'a self,
        _generation: GenerationId,
        _operation: &'a HandsOperationId,
    ) -> BoxFuture<'a, Result<HandsOperationStatus, HandsError>> {
        Box::pin(async { Err(Self::refusal()) })
    }

    fn cancel<'a>(
        &'a self,
        _generation: GenerationId,
        _operation: &'a HandsOperationId,
        _fence: Fence,
    ) -> BoxFuture<'a, Result<(), HandsError>> {
        Box::pin(async { Err(Self::refusal()) })
    }

    fn result<'a>(
        &'a self,
        _generation: GenerationId,
        _operation: &'a HandsOperationId,
        _bounds: &'a ResultBounds,
    ) -> BoxFuture<'a, Result<crate::ports::HandsResult, HandsError>> {
        Box::pin(async { Err(Self::refusal()) })
    }
}

/// An admission controller that admits everything, for tests about the loop rather than
/// about the bands.
#[derive(Debug, Clone, Copy, Default)]
pub struct AlwaysAdmit;

impl AdmissionControl for AlwaysAdmit {
    fn should_receive(&self) -> bool {
        true
    }

    fn admit(&self, _restore_bytes: u64) -> AdmissionDecision {
        AdmissionDecision::Admitted(Vec::new())
    }
}

/// One agent as the in-memory authority holds it.
#[derive(Debug, Clone)]
struct AgentRow {
    generation: GenerationId,
    revision: AgentRevision,
    fence: Fence,
    tail: Option<JournalSeq>,
    tail_hash: Option<ContentHash>,
    cancel_epoch: CancelEpoch,
    finish: Option<FinishReason>,
    phase: String,
    budget: BudgetNode,
    stop_requested: bool,
    lease_owner: Option<OwnerToken>,
    lease_expires_at: Timestamp,
    entries: Vec<JournalEntry>,
    effects: BTreeMap<EffectId, DurableEffect>,
}

#[derive(Debug, Clone, Copy)]
enum DispatchHeadRace {
    Cancel,
    Delete,
}

/// The in-memory `DynamoDB` stand-in: journal, effects and leases over one map.
///
/// It enforces every precondition the real transaction does. A fixture that accepted a stale
/// fence would make the fence tests assert nothing.
#[derive(Debug)]
pub struct MemoryStore {
    agents: Mutex<BTreeMap<AgentKey, AgentRow>>,
    authorities: Mutex<BTreeMap<SessionId, SessionAuthority>>,
    commit_faults: Mutex<VecDeque<Option<CommitError>>>,
    read_faults: Mutex<VecDeque<StoreError>>,
    read_pages: Mutex<VecDeque<JournalPage>>,
    dispatch_head_races: Mutex<VecDeque<DispatchHeadRace>>,
    clock: Arc<FixedClock>,
    queue: Arc<MemoryQueue>,
    log: Arc<Recorder>,
    restore_effect_overlap: AtomicU8,
}

impl MemoryStore {
    /// An empty authority.
    #[must_use]
    pub fn new(clock: Arc<FixedClock>, queue: Arc<MemoryQueue>, log: Arc<Recorder>) -> Self {
        Self {
            agents: Mutex::new(BTreeMap::new()),
            authorities: Mutex::new(BTreeMap::new()),
            commit_faults: Mutex::new(VecDeque::new()),
            read_faults: Mutex::new(VecDeque::new()),
            read_pages: Mutex::new(VecDeque::new()),
            dispatch_head_races: Mutex::new(VecDeque::new()),
            clock,
            queue,
            log,
            restore_effect_overlap: AtomicU8::new(0),
        }
    }

    /// Makes journal hydration and open-effect loading rendezvous on their first poll.
    ///
    /// This is opt-in test instrumentation. Without concurrent polling neither future can
    /// complete, so the activation regression test proves overlap rather than merely call
    /// order.
    pub fn require_restore_effect_overlap(&self) {
        self.restore_effect_overlap
            .store(0b1000_0000, Ordering::SeqCst);
    }

    /// Whether both independent reads reached the rendezvous.
    #[must_use]
    pub fn restore_effect_overlap_observed(&self) -> bool {
        self.restore_effect_overlap.load(Ordering::SeqCst) & 0b11 == 0b11
    }

    async fn rendezvous_restore_effect(&self, bit: u8) {
        if self.restore_effect_overlap.load(Ordering::SeqCst) & 0b1000_0000 == 0 {
            return;
        }
        core::future::poll_fn(|context| {
            let observed = self.restore_effect_overlap.fetch_or(bit, Ordering::SeqCst) | bit;
            if observed & 0b11 == 0b11 {
                core::task::Poll::Ready(())
            } else {
                context.waker().wake_by_ref();
                core::task::Poll::Pending
            }
        })
        .await;
    }

    /// Creates an agent whose journal is `entries`.
    ///
    /// The entries are sealed by the caller, so a test states the exact history it means
    /// rather than a sequence of mutations that happen to produce one.
    ///
    /// # Panics
    ///
    /// Panics if the lock is poisoned.
    pub fn seed(&self, key: AgentKey, entries: Vec<JournalEntry>) {
        let tail = entries.last().map(|entry| entry.envelope.seq);
        let tail_hash = entries.last().map(|entry| entry.envelope.content_hash);
        self.agents.lock().expect("not poisoned").insert(
            key,
            AgentRow {
                generation: GenerationId::from_uuid7(Uuid7::compose(1, [9; 10])),
                revision: AgentRevision::ZERO,
                fence: Fence::ZERO,
                tail,
                tail_hash,
                cancel_epoch: CancelEpoch::ZERO,
                finish: None,
                phase: "awaiting_model".to_owned(),
                budget: BudgetNode::default(),
                stop_requested: false,
                lease_owner: None,
                lease_expires_at: Timestamp::from_millis(0),
                entries,
                effects: BTreeMap::new(),
            },
        );
        self.authorities
            .lock()
            .expect("not poisoned")
            .entry(key.session)
            .or_insert_with(fixture_authority);
    }

    /// Forces a successor ownership generation, modelling another mux task after expiry.
    ///
    /// This is intentionally stronger than editing only the expiry: the returned claim is a
    /// real `(owner, fence)` pair the predecessor's next conditional renewal must observe.
    #[must_use]
    pub fn force_takeover(
        &self,
        key: AgentKey,
        owner: OwnerToken,
        ttl: core::time::Duration,
    ) -> Claim {
        let authority = self
            .authorities
            .lock()
            .expect("not poisoned")
            .get(&key.session)
            .cloned()
            .expect("the seeded session has authority");
        let mut agents = self.agents.lock().expect("not poisoned");
        let row = agents.get_mut(&key).expect("the seeded agent exists");
        row.fence = row.fence.advance();
        row.lease_owner = Some(owner);
        row.lease_expires_at = self
            .clock
            .now()
            .plus_millis(i64::try_from(ttl.as_millis()).unwrap_or(i64::MAX));
        Claim {
            key,
            owner,
            fence: row.fence,
            expires_at: row.lease_expires_at,
            authority,
            head: Self::head_of(row, key),
        }
    }

    /// Replaces the session-head authority a subsequent claim observes.
    pub fn set_authority(&self, session: SessionId, authority: SessionAuthority) {
        self.authorities
            .lock()
            .expect("not poisoned")
            .insert(session, authority);
    }

    /// Advances the session cancellation epoch observed by the next renewal.
    pub fn cancel_session(&self, key: AgentKey) {
        let mut agents = self.agents.lock().expect("not poisoned");
        let row = agents.get_mut(&key).expect("the seeded agent exists");
        row.cancel_epoch = CancelEpoch(row.cancel_epoch.0.saturating_add(1));
    }

    /// Advances cancellation after the next effect preparation commits but before its
    /// pre-dispatch transaction checks the session head.
    pub fn cancel_before_next_dispatch(&self) {
        self.dispatch_head_races
            .lock()
            .expect("not poisoned")
            .push_back(DispatchHeadRace::Cancel);
    }

    /// Advances deletion after the next effect preparation commits but before its
    /// pre-dispatch transaction checks the session head.
    pub fn delete_before_next_dispatch(&self) {
        self.dispatch_head_races
            .lock()
            .expect("not poisoned")
            .push_back(DispatchHeadRace::Delete);
    }

    /// Places `effect` in the agent's effect partition, as a dead owner would have left it.
    ///
    /// # Panics
    ///
    /// Panics if the agent was never seeded.
    pub fn seed_effect(&self, key: AgentKey, effect: DurableEffect) {
        let mut agents = self.agents.lock().expect("not poisoned");
        let row = agents.get_mut(&key).expect("the agent was seeded");
        row.effects.insert(effect.id, effect);
    }

    /// Scripts the next commit to fail with `error`.
    pub fn fail_next_commit(&self, error: CommitError) {
        self.commit_faults
            .lock()
            .expect("not poisoned")
            .push_back(Some(error));
    }

    /// Lets the next `count` commits through before the scripted failure.
    ///
    /// This is how a test names *which* durable point the process died at: a fault that
    /// always hit the first commit could only ever describe one boundary.
    pub fn pass_commits(&self, count: usize) {
        let mut faults = self.commit_faults.lock().expect("not poisoned");
        for _ in 0..count {
            faults.push_back(None);
        }
    }

    /// Scripts the next page read to fail with `error`.
    pub fn fail_next_read(&self, error: StoreError) {
        self.read_faults
            .lock()
            .expect("not poisoned")
            .push_back(error);
    }

    /// Supplies one exact page for the next journal read.
    ///
    /// Used for continuation/EOF boundary tests where the authoritative head and the
    /// service page must deliberately disagree.
    pub fn page_next_read(&self, page: JournalPage) {
        self.read_pages
            .lock()
            .expect("not poisoned")
            .push_back(page);
    }

    /// Replaces the tail pair returned with the next claim without changing stored pages.
    pub fn set_claimed_tail(
        &self,
        key: AgentKey,
        tail: Option<JournalSeq>,
        tail_hash: Option<ContentHash>,
    ) {
        let mut agents = self.agents.lock().expect("not poisoned");
        let row = agents.get_mut(&key).expect("the agent was seeded");
        row.tail = tail;
        row.tail_hash = tail_hash;
    }

    /// The agent's journal, as the authority holds it.
    ///
    /// # Panics
    ///
    /// Panics if the agent was never seeded.
    #[must_use]
    pub fn entries(&self, key: AgentKey) -> Vec<JournalEntry> {
        self.agents
            .lock()
            .expect("not poisoned")
            .get(&key)
            .expect("the agent was seeded")
            .entries
            .clone()
    }

    /// The agent's durable effects.
    ///
    /// # Panics
    ///
    /// Panics if the agent was never seeded.
    #[must_use]
    pub fn effects(&self, key: AgentKey) -> Vec<DurableEffect> {
        self.agents
            .lock()
            .expect("not poisoned")
            .get(&key)
            .expect("the agent was seeded")
            .effects
            .values()
            .cloned()
            .collect()
    }

    /// The agent's terminal reason, once it has one.
    ///
    /// # Panics
    ///
    /// Panics if the agent was never seeded.
    #[must_use]
    pub fn finish(&self, key: AgentKey) -> Option<FinishReason> {
        self.agents
            .lock()
            .expect("not poisoned")
            .get(&key)
            .expect("the agent was seeded")
            .finish
    }

    fn head_of(row: &AgentRow, key: AgentKey) -> AgentHead {
        AgentHead {
            key,
            generation: row.generation,
            revision: row.revision,
            fence: row.fence,
            journal_tail: row.tail,
            journal_tail_hash: row.tail_hash,
            cancel_epoch: row.cancel_epoch,
            finish: row.finish,
            phase: row.phase.clone(),
            budget: row.budget,
            stop_requested: row.stop_requested,
            open_effects: row
                .effects
                .values()
                .filter(|effect| !effect.state.is_settled())
                .map(|effect| effect.id)
                .collect(),
            lease_expires_at: row.lease_expires_at,
        }
    }
}

impl JournalStore for MemoryStore {
    fn load_head<'a>(
        &'a self,
        key: &'a AgentKey,
    ) -> BoxFuture<'a, Result<Option<AgentHead>, StoreError>> {
        Box::pin(async move {
            self.log.note("load_head");
            Ok(self
                .agents
                .lock()
                .expect("not poisoned")
                .get(key)
                .map(|row| Self::head_of(row, *key)))
        })
    }

    fn read_page<'a>(
        &'a self,
        key: &'a AgentKey,
        from: JournalSeq,
        budget: ReadBudget,
        after: Option<JournalCursor>,
    ) -> BoxFuture<'a, Result<JournalPage, StoreError>> {
        Box::pin(async move {
            self.log.note("read_page");
            self.rendezvous_restore_effect(0b01).await;
            if let Some(fault) = self.read_faults.lock().expect("not poisoned").pop_front() {
                return Err(fault);
            }
            if let Some(page) = self.read_pages.lock().expect("not poisoned").pop_front() {
                return Ok(page);
            }
            if after.as_ref().is_some_and(|cursor| cursor.next() != from) {
                return Err(StoreError::Undecodable {
                    location: "memory journal continuation".to_owned(),
                    reason: "the continuation sequence does not match the requested page"
                        .to_owned(),
                });
            }
            let agents = self.agents.lock().expect("not poisoned");
            let Some(row) = agents.get(key) else {
                return Ok(JournalPage {
                    entries: Vec::new(),
                    hydrated_bytes: 0,
                    next: None,
                });
            };
            let mut entries = Vec::new();
            let mut hydrated_bytes = 0_usize;
            for entry in row
                .entries
                .iter()
                .filter(|entry| entry.envelope.seq >= from)
                .take(budget.max_entries)
            {
                let body_bytes =
                    entry
                        .record
                        .canonical_bytes()
                        .map_err(|error| StoreError::Undecodable {
                            location: "memory journal entry".to_owned(),
                            reason: error.to_string(),
                        })?;
                hydrated_bytes = hydrated_bytes.saturating_add(body_bytes.len());
                if hydrated_bytes > budget.max_bytes {
                    return Err(StoreError::ReadBudgetExhausted {
                        entries: entries.len(),
                        bytes: hydrated_bytes,
                    });
                }
                entries.push(entry.clone());
            }
            let next = entries
                .last()
                .map(|entry| entry.envelope.seq.next())
                .filter(|next| row.entries.iter().any(|entry| entry.envelope.seq >= *next))
                .map(|next| {
                    JournalCursor::new(
                        [("offset", next.get().saturating_sub(1).to_string())],
                        after.as_ref().map_or(from, JournalCursor::start),
                        next,
                    )
                });
            Ok(JournalPage {
                entries,
                hydrated_bytes,
                next,
            })
        })
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the fixture enforces the whole precondition set in one place; splitting it would let a reader believe a precondition is checked somewhere it is not"
    )]
    fn commit<'a>(
        &'a self,
        context: &'a DecisionContext,
        commit: &'a DecisionCommit,
    ) -> BoxFuture<'a, Result<CommitReceipt, CommitError>> {
        Box::pin(async move {
            self.log.note("commit");
            commit.validate()?;
            if let Some(Some(fault)) = self.commit_faults.lock().expect("not poisoned").pop_front()
            {
                return Err(fault);
            }
            let now = self.clock.now();
            let authority = self
                .authorities
                .lock()
                .expect("not poisoned")
                .get(&commit.guard.key.session)
                .cloned()
                .ok_or_else(|| {
                    CommitError::Store(StoreError::Undecodable {
                        location: "session head".to_owned(),
                        reason: "the session does not exist".to_owned(),
                    })
                })?;
            if authority != context.authority {
                return Err(ConditionFailure::CancelEpochAdvanced.into());
            }
            let mut agents = self.agents.lock().expect("not poisoned");
            let key = commit.guard.key;
            let row = agents.get_mut(&key).ok_or_else(|| {
                CommitError::Store(StoreError::Undecodable {
                    location: "agent control".to_owned(),
                    reason: "the agent does not exist".to_owned(),
                })
            })?;
            if row.fence != commit.guard.fence || row.lease_owner != Some(commit.guard.owner) {
                return Err(ConditionFailure::StaleFence.into());
            }
            if row.revision != commit.guard.revision {
                return Err(ConditionFailure::StaleRevision.into());
            }
            if row.tail != commit.guard.tail {
                return Err(ConditionFailure::UnexpectedTail.into());
            }
            if row.cancel_epoch != commit.guard.cancel_epoch {
                return Err(ConditionFailure::CancelEpochAdvanced.into());
            }
            if let Some(retired) = &commit.retired_wake
                && !self
                    .queue
                    .can_retire(retired, key, &context.authority.workspace.to_string())
            {
                return Err(ConditionFailure::WakeStateMoved.into());
            }

            let mut seq = commit.guard.tail.map_or(JournalSeq::ZERO, JournalSeq::next);
            for record in &commit.appends {
                let entry = JournalEntry::seal(seq, now, record.clone())
                    .map_err(aex_brain_domain::commit::EnvelopeViolation::from)?;
                // The journal put is conditional on `attribute_not_exists`, so a redelivered
                // identical decision loses that action and comes back as a replay.
                if row
                    .entries
                    .iter()
                    .any(|stored| stored.envelope.seq == entry.envelope.seq)
                {
                    return Err(ConditionFailure::IdempotentReplay(Box::new(CommitReceipt {
                        revision: row.revision,
                        tail: row.tail.unwrap_or(JournalSeq::ZERO),
                        wakes: Vec::new(),
                        committed_at: now,
                    }))
                    .into());
                }
                row.entries.push(entry);
                seq = seq.next();
            }
            for write in &commit.effects {
                match write {
                    EffectWrite::Prepare {
                        id,
                        kind,
                        generation,
                        class,
                        request_hash,
                        deadline,
                        attempt,
                    } => {
                        row.effects.insert(
                            *id,
                            DurableEffect {
                                id: *id,
                                kind: *kind,
                                generation: *generation,
                                class: *class,
                                request_hash: *request_hash,
                                state: EffectState::Prepared { attempt: *attempt },
                                deadline: *deadline,
                                evidence: None,
                            },
                        );
                    }
                    EffectWrite::Settle { id, outcome } => {
                        if let Some(effect) = row.effects.get_mut(id) {
                            effect.state = match outcome {
                                SettledOutcome::Complete { receipt } => {
                                    EffectState::Complete { receipt: *receipt }
                                }
                                SettledOutcome::KnownFailure { stage, proof } => {
                                    EffectState::KnownFailure {
                                        stage: *stage,
                                        proof: *proof,
                                    }
                                }
                                SettledOutcome::OutcomeUnknown { evidence } => {
                                    EffectState::OutcomeUnknown {
                                        evidence: evidence.clone(),
                                    }
                                }
                            };
                        }
                    }
                }
            }
            if !commit.is_retirement_only() {
                row.revision = commit.control.next_revision;
                row.tail = Some(commit.control.next_tail);
                if let Some(last) = row.entries.last() {
                    row.tail_hash = Some(last.envelope.content_hash);
                }
                row.phase.clone_from(&commit.control.phase);
                if let Some(finish) = commit.control.finish {
                    row.finish = Some(finish);
                }
            }
            let wakes: Vec<WakeId> = commit.wakes.iter().map(|wake| wake.id).collect();
            // The durable wake row is projected onto the queue, exactly as the
            // `regional-work` stream does. Nothing else ever puts a message there.
            for wake in &commit.wakes {
                self.queue.project_in_shard(
                    DurableWake {
                        id: wake.id,
                        work_id: format!("wrk_{}", wake.id.0.as_simple()),
                        key: wake.key,
                        dedup_key: wake.dedup_key.clone(),
                        reason: wake.reason.clone(),
                        due: wake.due,
                        priority: wake.priority,
                        tenant: wake.tenant.clone(),
                    },
                    wake.shard,
                );
            }
            if let Some(retired) = &commit.retired_wake {
                self.queue.retire(retired);
            }
            Ok(CommitReceipt {
                revision: row.revision,
                tail: commit.control.next_tail,
                wakes,
                committed_at: now,
            })
        })
    }
}

impl EffectStore for MemoryStore {
    fn mark_dispatch_started<'a>(
        &'a self,
        guard: &'a FenceGuard,
        authority: &'a SessionAuthority,
        effect: &'a EffectId,
        attempt: u16,
        at: Timestamp,
    ) -> BoxFuture<'a, Result<DispatchTicket, CommitError>> {
        Box::pin(async move {
            self.log.note("mark_dispatch_started");
            match self
                .dispatch_head_races
                .lock()
                .expect("not poisoned")
                .pop_front()
            {
                Some(DispatchHeadRace::Cancel) => {
                    let mut agents = self.agents.lock().expect("not poisoned");
                    let row = agents
                        .get_mut(&guard.key())
                        .ok_or(CommitError::Condition(ConditionFailure::StaleFence))?;
                    row.cancel_epoch = CancelEpoch(row.cancel_epoch.0.saturating_add(1));
                }
                Some(DispatchHeadRace::Delete) => {
                    let mut authorities = self.authorities.lock().expect("not poisoned");
                    let current = authorities.get_mut(&guard.key().session).ok_or_else(|| {
                        CommitError::Store(StoreError::Undecodable {
                            location: "session head".to_owned(),
                            reason: "the session does not exist".to_owned(),
                        })
                    })?;
                    current.deletion_epoch = current.deletion_epoch.saturating_add(1);
                }
                None => {}
            }
            let current_authority = self
                .authorities
                .lock()
                .expect("not poisoned")
                .get(&guard.key().session)
                .cloned()
                .ok_or_else(|| {
                    CommitError::Store(StoreError::Undecodable {
                        location: "session head".to_owned(),
                        reason: "the session does not exist".to_owned(),
                    })
                })?;
            if &current_authority != authority {
                return Err(ConditionFailure::CancelEpochAdvanced.into());
            }
            let mut agents = self.agents.lock().expect("not poisoned");
            let row = agents
                .get_mut(&guard.key())
                .ok_or(CommitError::Condition(ConditionFailure::StaleFence))?;
            if row.cancel_epoch != guard.cancel_epoch() {
                return Err(ConditionFailure::CancelEpochAdvanced.into());
            }
            if row.fence != guard.fence() || row.lease_owner != Some(guard.as_ref().owner) {
                return Err(ConditionFailure::StaleFence.into());
            }
            let durable = row.effects.get_mut(effect).ok_or(CommitError::Condition(
                ConditionFailure::EffectStateMismatch { effect: *effect },
            ))?;
            // Ownership takeover is not a retry: no byte left the predecessor. The
            // prepared attempt is immutable and the successor must mint its ticket for
            // that exact attempt rather than rewriting durable history.
            if !matches!(
                durable.state,
                EffectState::Prepared {
                    attempt: prepared_attempt
                } if prepared_attempt == attempt
            ) {
                return Err(ConditionFailure::EffectStateMismatch { effect: *effect }.into());
            }
            durable.state = EffectState::DispatchStarted { attempt };
            durable.evidence = Some(DispatchEvidence::ambiguous(
                attempt,
                aex_brain_domain::effect::DispatchStage::Dispatched,
            ));
            Ok(DispatchTicket::mint(
                guard,
                authority.workspace,
                authority.organization,
                *effect,
                attempt,
                at,
            ))
        })
    }

    fn mark_response_started<'a>(
        &'a self,
        ticket: &'a DispatchTicket,
        evidence: &'a DispatchEvidence,
    ) -> BoxFuture<'a, Result<(), CommitError>> {
        Box::pin(async move {
            self.log.note("mark_response_started");
            let mut agents = self.agents.lock().expect("not poisoned");
            let row = agents
                .get_mut(&ticket.key())
                .ok_or(CommitError::Condition(ConditionFailure::StaleFence))?;
            let effect = ticket.effect();
            let durable = row.effects.get_mut(&effect).ok_or(CommitError::Condition(
                ConditionFailure::EffectStateMismatch { effect },
            ))?;
            durable
                .mark_response_started(evidence.clone())
                .map_err(|_| {
                    CommitError::Condition(ConditionFailure::EffectStateMismatch { effect })
                })
        })
    }

    fn load_open<'a>(
        &'a self,
        key: &'a AgentKey,
    ) -> BoxFuture<'a, Result<Vec<DurableEffect>, StoreError>> {
        Box::pin(async move {
            self.log.note("load_open");
            self.rendezvous_restore_effect(0b10).await;
            Ok(self
                .agents
                .lock()
                .expect("not poisoned")
                .get(key)
                .map(|row| {
                    row.effects
                        .values()
                        .filter(|effect| !effect.state.is_settled())
                        .cloned()
                        .collect()
                })
                .unwrap_or_default())
        })
    }
}

impl LeaseStore for MemoryStore {
    fn claim<'a>(
        &'a self,
        key: &'a AgentKey,
        owner: OwnerToken,
        ttl: core::time::Duration,
        now: Timestamp,
    ) -> BoxFuture<'a, Result<Claim, ClaimError>> {
        Box::pin(async move {
            self.log.note("claim");
            let authority = self
                .authorities
                .lock()
                .expect("not poisoned")
                .get(&key.session)
                .cloned()
                .ok_or_else(|| {
                    ClaimError::Store(StoreError::Undecodable {
                        location: "session head".to_owned(),
                        reason: "the session does not exist".to_owned(),
                    })
                })?;
            let mut agents = self.agents.lock().expect("not poisoned");
            let row = agents.get_mut(key).ok_or(ClaimError::HeldByOther {
                expires_at: Timestamp::from_millis(0),
            })?;
            if row.lease_expires_at.millis() > now.millis() && row.lease_owner != Some(owner) {
                return Err(ClaimError::HeldByOther {
                    expires_at: row.lease_expires_at,
                });
            }
            row.fence = row.fence.advance();
            row.lease_owner = Some(owner);
            row.lease_expires_at =
                now.plus_millis(i64::try_from(ttl.as_millis()).unwrap_or(i64::MAX));
            Ok(Claim {
                key: *key,
                owner,
                fence: row.fence,
                expires_at: row.lease_expires_at,
                authority,
                head: Self::head_of(row, *key),
            })
        })
    }

    fn renew<'a>(
        &'a self,
        claim: &'a Claim,
        ttl: core::time::Duration,
        now: Timestamp,
    ) -> BoxFuture<'a, Result<Claim, ClaimError>> {
        Box::pin(async move {
            self.log.note("renew_lease");
            let authority = self
                .authorities
                .lock()
                .expect("not poisoned")
                .get(&claim.key.session)
                .cloned()
                .ok_or(ClaimError::Terminal)?;
            if authority != claim.authority {
                return Err(ClaimError::Terminal);
            }
            let mut agents = self.agents.lock().expect("not poisoned");
            let row = agents.get_mut(&claim.key).ok_or(ClaimError::Terminal)?;
            if row.cancel_epoch != claim.head.cancel_epoch {
                return Err(ClaimError::Terminal);
            }
            if row.fence != claim.fence || row.lease_owner != Some(claim.owner) {
                return Err(ClaimError::Fenced { current: row.fence });
            }
            row.lease_expires_at =
                now.plus_millis(i64::try_from(ttl.as_millis()).unwrap_or(i64::MAX));
            Ok(Claim {
                expires_at: row.lease_expires_at,
                ..claim.clone()
            })
        })
    }

    fn release(
        &self,
        claim: Claim,
        _disposition: ReleaseDisposition,
    ) -> BoxFuture<'_, Result<(), StoreError>> {
        Box::pin(async move {
            self.log.note("release_lease");
            let mut agents = self.agents.lock().expect("not poisoned");
            if let Some(row) = agents.get_mut(&claim.key)
                && row.fence == claim.fence
                && row.lease_owner == Some(claim.owner)
            {
                row.lease_owner = None;
                row.lease_expires_at = Timestamp::from_millis(0);
            }
            Ok(())
        })
    }
}

/// An at-least-once in-memory queue.
///
/// It has no `enqueue`: [`MemoryQueue::project`] is how a committed wake reaches it, which
/// is the same shape as the `regional-work` stream projection. Nothing else can create a
/// delivery, so a test cannot accidentally prove the loop works on invented work.
#[derive(Debug, Default)]
pub struct MemoryQueue {
    durable: Mutex<BTreeMap<WakeId, (DurableWake, WorkShard)>>,
    malformed_due: Mutex<BTreeMap<WakeId, WorkShard>>,
    visible: Mutex<VecDeque<WakeDelivery>>,
    malformed_visible: Mutex<VecDeque<MalformedWakeDelivery>>,
    acked: Mutex<Vec<WakeDelivery>>,
    receive_faults: Mutex<VecDeque<StoreError>>,
    ack_faults: Mutex<VecDeque<StoreError>>,
    receipts: AtomicU64,
    visibility_extensions: AtomicU64,
    malformed_released: AtomicU64,
    malformed_acked: AtomicU64,
    log: Mutex<Option<Arc<Recorder>>>,
}

impl MemoryQueue {
    /// An empty queue that records into `log`.
    #[must_use]
    pub fn new(log: Arc<Recorder>) -> Self {
        Self {
            durable: Mutex::new(BTreeMap::new()),
            malformed_due: Mutex::new(BTreeMap::new()),
            visible: Mutex::new(VecDeque::new()),
            malformed_visible: Mutex::new(VecDeque::new()),
            acked: Mutex::new(Vec::new()),
            receive_faults: Mutex::new(VecDeque::new()),
            ack_faults: Mutex::new(VecDeque::new()),
            receipts: AtomicU64::new(0),
            visibility_extensions: AtomicU64::new(0),
            malformed_released: AtomicU64::new(0),
            malformed_acked: AtomicU64::new(0),
            log: Mutex::new(Some(log)),
        }
    }

    /// Projects one durable wake onto the queue, as the work stream does.
    pub fn project(&self, wake: DurableWake) {
        self.project_in_shard(wake, WorkShard(0));
    }

    /// Projects one malformed queue record beside valid siblings.
    pub fn project_malformed(&self, receive_count: u32) {
        let receipt = self.receipts.fetch_add(1, Ordering::SeqCst);
        self.malformed_visible
            .lock()
            .expect("not poisoned")
            .push_back(MalformedWakeDelivery {
                receipt: Some(format!("bad-rh-{receipt}")),
                receive_count,
                reason: MalformedWakeReason::InvalidProjection,
                fingerprint: format!("{receipt:016x}"),
            });
    }

    /// Persists a durable wake without projecting its stream hint.
    ///
    /// This is the lost-hint fixture: only [`WakeQueue::due_scan`] can recover it.
    pub fn persist(&self, wake: DurableWake, shard: WorkShard) {
        self.durable
            .lock()
            .expect("not poisoned")
            .insert(wake.id, (wake, shard));
    }

    /// Persists one malformed due-index projection for isolation/starvation tests.
    ///
    /// It has an index identity and shard but no decodable wake body, matching a malformed
    /// projected row closely enough to assert that the page cursor advances past it.
    pub fn persist_malformed_due(&self, id: WakeId, shard: WorkShard) {
        self.malformed_due
            .lock()
            .expect("not poisoned")
            .insert(id, shard);
    }

    fn project_in_shard(&self, wake: DurableWake, shard: WorkShard) {
        self.persist(wake.clone(), shard);
        let receipt = self.receipts.fetch_add(1, Ordering::SeqCst);
        self.visible
            .lock()
            .expect("not poisoned")
            .push_back(WakeDelivery {
                wake,
                origin: WakeOrigin::Queue {
                    receipt: format!("rh-{receipt}"),
                    receive_count: 1,
                },
            });
    }

    /// How many authoritative wake rows remain in the sparse due set.
    #[must_use]
    pub fn durable_depth(&self) -> usize {
        self.durable.lock().expect("not poisoned").len()
    }

    fn can_retire(
        &self,
        wake: &aex_brain_domain::commit::WakeRetirement,
        key: AgentKey,
        tenant: &str,
    ) -> bool {
        self.durable
            .lock()
            .expect("not poisoned")
            .values()
            .any(|(stored, _)| {
                stored.work_id == wake.work_id && stored.key == key && stored.tenant == tenant
            })
    }

    fn retire(&self, wake: &aex_brain_domain::commit::WakeRetirement) {
        self.durable
            .lock()
            .expect("not poisoned")
            .retain(|_, (stored, _)| stored.work_id != wake.work_id);
    }

    /// Scripts the next queue receive to fail before the due backstop may advance.
    pub fn fail_next_receive(&self, error: StoreError) {
        self.receive_faults
            .lock()
            .expect("not poisoned")
            .push_back(error);
    }

    /// Scripts the next ack to fail, which is the crash between the commit and the ack.
    pub fn fail_next_ack(&self, error: StoreError) {
        self.ack_faults
            .lock()
            .expect("not poisoned")
            .push_back(error);
    }

    /// How many deliveries are waiting.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.visible.lock().expect("not poisoned").len()
    }

    /// Every delivery that was acknowledged.
    #[must_use]
    pub fn acked(&self) -> Vec<WakeDelivery> {
        self.acked.lock().expect("not poisoned").clone()
    }

    /// How many times a live queue delivery had its visibility extended.
    #[must_use]
    pub fn visibility_extensions(&self) -> u64 {
        self.visibility_extensions.load(Ordering::SeqCst)
    }

    /// How many malformed records were explicitly returned for another receive.
    #[must_use]
    pub fn malformed_released(&self) -> u64 {
        self.malformed_released.load(Ordering::SeqCst)
    }

    /// How many malformed records reached the poison threshold and were acknowledged.
    #[must_use]
    pub fn malformed_acked(&self) -> u64 {
        self.malformed_acked.load(Ordering::SeqCst)
    }

    fn note(&self, what: &str) {
        if let Some(log) = self.log.lock().expect("not poisoned").as_ref() {
            log.note(what);
        }
    }
}

impl WakeQueue for MemoryQueue {
    fn receive(
        &self,
        max: usize,
        _wait: core::time::Duration,
    ) -> BoxFuture<'_, Result<WakeBatch, StoreError>> {
        Box::pin(async move {
            self.note("receive");
            if let Some(fault) = self
                .receive_faults
                .lock()
                .expect("not poisoned")
                .pop_front()
            {
                return Err(fault);
            }
            let mut visible = self.visible.lock().expect("not poisoned");
            let mut malformed = self.malformed_visible.lock().expect("not poisoned");
            let valid_taken = visible.len().min(max);
            let remaining = max.saturating_sub(valid_taken);
            let malformed_taken = malformed.len().min(remaining);
            Ok(WakeBatch {
                deliveries: visible.drain(..valid_taken).collect(),
                malformed: malformed.drain(..malformed_taken).collect(),
            })
        })
    }

    fn release_malformed(
        &self,
        mut delivery: MalformedWakeDelivery,
        _after: core::time::Duration,
    ) -> BoxFuture<'_, Result<(), StoreError>> {
        Box::pin(async move {
            self.note("release_malformed");
            self.malformed_released.fetch_add(1, Ordering::SeqCst);
            delivery.receive_count = delivery.receive_count.saturating_add(1);
            self.malformed_visible
                .lock()
                .expect("not poisoned")
                .push_back(delivery);
            Ok(())
        })
    }

    fn ack_malformed(
        &self,
        _delivery: MalformedWakeDelivery,
    ) -> BoxFuture<'_, Result<(), StoreError>> {
        Box::pin(async move {
            self.note("ack_malformed");
            self.malformed_acked.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }

    fn state<'a>(&'a self, wake: &'a DurableWake) -> BoxFuture<'a, Result<WakeState, StoreError>> {
        Box::pin(async move {
            let durable = self.durable.lock().expect("not poisoned");
            match durable.get(&wake.id) {
                Some((stored, _))
                    if stored.work_id == wake.work_id
                        && stored.key == wake.key
                        && stored.tenant == wake.tenant
                        && stored.dedup_key == wake.dedup_key =>
                {
                    Ok(WakeState::Pending)
                }
                Some(_) => Err(StoreError::Undecodable {
                    location: "regional-work/source wake".to_owned(),
                    reason: "the delivery does not match the authoritative wake".to_owned(),
                }),
                None => Ok(WakeState::Retired),
            }
        })
    }

    fn extend_visibility<'a>(
        &'a self,
        delivery: &'a WakeDelivery,
        _by: core::time::Duration,
    ) -> BoxFuture<'a, Result<(), StoreError>> {
        Box::pin(async move {
            if matches!(delivery.origin, WakeOrigin::Queue { .. }) {
                self.note("extend_visibility");
                self.visibility_extensions.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        })
    }

    fn release(
        &self,
        delivery: WakeDelivery,
        _after: core::time::Duration,
    ) -> BoxFuture<'_, Result<(), StoreError>> {
        Box::pin(async move {
            self.note("release_delivery");
            let mut returned = delivery;
            if let WakeOrigin::Queue { receive_count, .. } = &mut returned.origin {
                *receive_count = receive_count.saturating_add(1);
                self.visible
                    .lock()
                    .expect("not poisoned")
                    .push_back(returned);
            }
            Ok(())
        })
    }

    fn ack(&self, delivery: WakeDelivery) -> BoxFuture<'_, Result<(), StoreError>> {
        Box::pin(async move {
            self.note("ack");
            if matches!(delivery.origin, WakeOrigin::DueScan) {
                return Ok(());
            }
            if let Some(fault) = self.ack_faults.lock().expect("not poisoned").pop_front() {
                return Err(fault);
            }
            self.acked.lock().expect("not poisoned").push(delivery);
            Ok(())
        })
    }

    fn due_scan(
        &self,
        shard: WorkShard,
        now: Timestamp,
        max: usize,
        after: Option<DueScanCursor>,
    ) -> BoxFuture<'_, Result<DueScanPage, StoreError>> {
        Box::pin(async move {
            self.note("due_scan");
            let after = after
                .as_ref()
                .map(|cursor| {
                    cursor
                        .parts()
                        .get("wakeId")
                        .ok_or_else(|| StoreError::Undecodable {
                            location: "memory due cursor".to_owned(),
                            reason: "`wakeId` is absent".to_owned(),
                        })?
                        .parse::<Uuid>()
                        .map(WakeId)
                        .map_err(|_| StoreError::Undecodable {
                            location: "memory due cursor".to_owned(),
                            reason: "`wakeId` is malformed".to_owned(),
                        })
                })
                .transpose()?;
            let mut rows: Vec<(WakeId, Option<DurableWake>)> = self
                .durable
                .lock()
                .expect("not poisoned")
                .iter()
                .filter(|(id, (wake, stored_shard))| {
                    after.is_none_or(|after| **id > after)
                        && *stored_shard == shard
                        && wake.due.is_some_and(|due| due.millis() <= now.millis())
                })
                .map(|(id, (wake, _))| (*id, Some(wake.clone())))
                .collect();
            rows.extend(
                self.malformed_due
                    .lock()
                    .expect("not poisoned")
                    .iter()
                    .filter(|(id, stored_shard)| {
                        after.is_none_or(|after| **id > after) && **stored_shard == shard
                    })
                    .map(|(id, _)| (*id, None)),
            );
            rows.sort_by_key(|(id, _)| *id);
            let has_more = rows.len() > max;
            rows.truncate(max);
            let last = rows.last().map(|(id, _)| *id);
            let malformed = rows.iter().filter(|(_, wake)| wake.is_none()).count();
            let isolations = rows
                .iter()
                .filter(|(_, wake)| wake.is_none())
                .take(MAX_DUE_ROW_ISOLATIONS)
                .map(|(id, _)| {
                    let digest =
                        id.0.as_bytes()
                            .iter()
                            .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
                                (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3)
                            });
                    DueRowIsolation {
                        reason: DueRowIsolationReason::MalformedProjection,
                        fingerprint: format!("{digest:016x}"),
                    }
                })
                .collect();
            let wakes = rows.into_iter().filter_map(|(_, wake)| wake).collect();
            let next = has_more.then(|| {
                DueScanCursor::new([(
                    "wakeId",
                    last.expect("a truncated page has a last row")
                        .0
                        .as_hyphenated()
                        .to_string(),
                )])
            });
            Ok(DueScanPage {
                wakes,
                next,
                malformed,
                isolations,
            })
        })
    }
}

/// The wake a test offers to start an activation.
///
/// Built through the queue's projection so no test can start work the durable authority did
/// not create.
#[must_use]
pub fn wake_for(key: AgentKey, dedup: &str) -> DurableWake {
    DurableWake {
        id: WakeId(Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_00a1)),
        work_id: dedup.to_owned(),
        key,
        dedup_key: dedup.to_owned(),
        reason: ParkReason::AwaitingUserMessage,
        due: None,
        priority: 1,
        tenant: fixture_authority().workspace.to_string(),
    }
}

/// The tenant authority used by in-memory sessions unless a test replaces it explicitly.
#[must_use]
pub fn fixture_authority() -> SessionAuthority {
    SessionAuthority {
        workspace: aex_wire::ids::WorkspaceId::from_uuid7(Uuid7::compose(
            1_767_225_600_002,
            [3; 10],
        )),
        organization: aex_wire::ids::OrganizationId::from_uuid7(Uuid7::compose(
            1_767_225_600_003,
            [4; 10],
        )),
        deletion_epoch: 0,
    }
}
