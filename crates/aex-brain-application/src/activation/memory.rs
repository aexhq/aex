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
    DispatchTicket, DurableWake, EffectStore, FenceGuard, HandsAccepted, HandsEndpoint, HandsError,
    HandsOperationStart, HandsOperationStatus, HandsPort, IdPort, JournalPage, JournalStore,
    LeaseStore, PreparedToolCall, PreviewSink, ProviderDispatchError, ProviderOutcome,
    ProviderPort, ReadBudget, RedactedDetail, ReleaseDisposition, ResultBounds, SessionAuthority,
    SteadyInstant, StoreError, StreamBudget, ToolDispatchError, ToolOutcome, ToolPort, ToolRoute,
    ToolRoutingError, WakeDelivery, WakeQueue,
};
use aex_brain_domain::budget::BudgetNode;
use aex_brain_domain::commit::{DecisionCommit, EffectWrite};
use aex_brain_domain::effect::{DispatchEvidence, DurableEffect, EffectState, SettledOutcome};
use aex_brain_domain::ids::{
    AgentId, AgentKey, AgentRevision, CancelEpoch, CatalogPin, DetachedOperationId, EffectId,
    Fence, HandsGeneration, HandsOperationId, JournalSeq, ModelSlug, OwnerToken, SessionId,
    Timestamp, ToolName, WakeId, WorkShard,
};
use aex_brain_domain::journal::{FinishReason, JournalEntry, ParkReason};
use aex_brain_domain::wire_pending::{
    CanonicalModelRequest, DurableOperationSupport, ModelCapability, ProviderId, ToolManifestEntry,
};
use aex_wire::ids::{PrefixedId, Uuid7};
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
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
    models: Mutex<BTreeMap<(ProviderId, String), ModelCapability>>,
    support: Mutex<DurableOperationSupport>,
}

impl FixedCatalog {
    /// A catalog with one admitted model.
    #[must_use]
    pub fn with_model(capability: ModelCapability) -> Self {
        let catalog = Self {
            models: Mutex::new(BTreeMap::new()),
            support: Mutex::new(DurableOperationSupport::None),
        };
        catalog.models.lock().expect("not poisoned").insert(
            (capability.provider, capability.model.0.clone()),
            capability,
        );
        catalog
    }

    /// Declares that this catalog's models expose a proven durable operation.
    pub fn prove_durable_operations(&self) {
        *self.support.lock().expect("not poisoned") = DurableOperationSupport::Proven;
    }
}

impl CatalogPort for FixedCatalog {
    fn digest(&self, pin: &CatalogPin) -> Result<CatalogDigest, CatalogError> {
        Ok(CatalogDigest {
            digest: pin.0,
            revision: 1,
            signature_verified: true,
        })
    }

    fn model(
        &self,
        pin: &CatalogPin,
        provider: ProviderId,
        model: &ModelSlug,
    ) -> Result<ModelCapability, CatalogError> {
        self.models
            .lock()
            .expect("not poisoned")
            .get(&(provider, model.0.clone()))
            .cloned()
            .ok_or_else(|| CatalogError::UnknownModel {
                pin: pin.0,
                provider,
                model: model.clone(),
            })
    }

    fn tool(&self, pin: &CatalogPin, name: &ToolName) -> Result<ToolManifestEntry, CatalogError> {
        Err(CatalogError::UnknownTool {
            pin: pin.0,
            name: name.clone(),
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
}

impl ScriptedProvider {
    /// A provider that will answer `script`, in order.
    #[must_use]
    pub fn new(script: impl IntoIterator<Item = ProviderScript>) -> Self {
        Self {
            script: Mutex::new(script.into_iter().collect()),
            dispatched: Mutex::new(Vec::new()),
        }
    }

    /// Every dispatch it was asked to perform.
    #[must_use]
    pub fn dispatched(&self) -> Vec<(EffectId, Fence, u16)> {
        self.dispatched.lock().expect("not poisoned").clone()
    }
}

impl ProviderPort for ScriptedProvider {
    fn dispatch<'a>(
        &'a self,
        ticket: &'a DispatchTicket,
        _request: &'a CanonicalModelRequest,
        _budget: &'a StreamBudget,
        _preview: &'a dyn PreviewSink,
        _cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderDispatchError>> {
        Box::pin(async move {
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
                    class: crate::ports::ProviderFailureClass::Transient,
                    provider_request_id: None,
                    retry_after: None,
                    detail: RedactedDetail::new("the script is exhausted"),
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
    invocations: Mutex<VecDeque<Result<ToolOutcome, ToolDispatchError>>>,
    queries: Mutex<VecDeque<DetachedStatus>>,
    invoked: Mutex<Vec<String>>,
}

impl ScriptedTools {
    /// A router that knows `route` and will answer `script`, in order.
    #[must_use]
    pub fn new(
        routes: impl IntoIterator<Item = ToolRoute>,
        script: impl IntoIterator<Item = Result<ToolOutcome, ToolDispatchError>>,
    ) -> Self {
        Self {
            routes: Mutex::new(
                routes
                    .into_iter()
                    .map(|route| (route.name.0.clone(), route))
                    .collect(),
            ),
            invocations: Mutex::new(script.into_iter().collect()),
            queries: Mutex::new(VecDeque::new()),
            invoked: Mutex::new(Vec::new()),
        }
    }

    /// Scripts what a durable-operation query answers, in order.
    pub fn script_queries(&self, script: impl IntoIterator<Item = DetachedStatus>) {
        *self.queries.lock().expect("not poisoned") = script.into_iter().collect();
    }

    /// Every call it was asked to invoke.
    #[must_use]
    pub fn invoked(&self) -> Vec<String> {
        self.invoked.lock().expect("not poisoned").clone()
    }
}

impl ToolPort for ScriptedTools {
    fn route(&self, pin: &CatalogPin, name: &ToolName) -> Result<ToolRoute, ToolRoutingError> {
        self.routes
            .lock()
            .expect("not poisoned")
            .get(&name.0)
            .cloned()
            .ok_or_else(|| {
                let _ = pin;
                ToolRoutingError::Unknown {
                    name: name.0.clone(),
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
                .push(call.call.0.clone());
            self.invocations
                .lock()
                .expect("not poisoned")
                .pop_front()
                .unwrap_or_else(|| {
                    Err(ToolDispatchError {
                        stage: aex_brain_domain::effect::DispatchStage::Dispatched,
                        proof: aex_brain_domain::effect::DispatchProof::PossiblySent,
                        retryable: false,
                        detail: RedactedDetail::new("the script is exhausted"),
                    })
                })
        })
    }

    fn query<'a>(
        &'a self,
        _operation: &'a DetachedOperationId,
    ) -> BoxFuture<'a, Result<DetachedStatus, ToolDispatchError>> {
        Box::pin(async move {
            Ok(self
                .queries
                .lock()
                .expect("not poisoned")
                .pop_front()
                .unwrap_or(DetachedStatus::Unknown))
        })
    }

    fn cancel<'a>(
        &'a self,
        _operation: &'a DetachedOperationId,
        _fence: Fence,
    ) -> BoxFuture<'a, Result<(), ToolDispatchError>> {
        Box::pin(async { Ok(()) })
    }
}

/// A Hands port whose peer does not exist yet.
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
            detail: RedactedDetail::new(
                "aex-brain-hands does not implement HandsPort; it binds aex-hands-protocol \
                 and takes no dependency on aex-brain-application",
            ),
        }
    }
}

impl HandsPort for AbsentHands {
    fn ensure_generation<'a>(
        &'a self,
        _session: &'a SessionId,
        _generation: HandsGeneration,
    ) -> BoxFuture<'a, Result<HandsEndpoint, HandsError>> {
        Box::pin(async { Err(Self::refusal()) })
    }

    fn start<'a>(
        &'a self,
        _ticket: &'a DispatchTicket,
        _generation: HandsGeneration,
        _start: &'a HandsOperationStart,
    ) -> BoxFuture<'a, Result<HandsAccepted, HandsError>> {
        Box::pin(async { Err(Self::refusal()) })
    }

    fn status<'a>(
        &'a self,
        _generation: HandsGeneration,
        _operation: &'a HandsOperationId,
    ) -> BoxFuture<'a, Result<HandsOperationStatus, HandsError>> {
        Box::pin(async { Err(Self::refusal()) })
    }

    fn cancel<'a>(
        &'a self,
        _generation: HandsGeneration,
        _operation: &'a HandsOperationId,
        _fence: Fence,
    ) -> BoxFuture<'a, Result<(), HandsError>> {
        Box::pin(async { Err(Self::refusal()) })
    }

    fn result<'a>(
        &'a self,
        _generation: HandsGeneration,
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

    fn admit(&self) -> AdmissionDecision {
        AdmissionDecision::Admitted(Vec::new())
    }
}

/// One agent as the in-memory authority holds it.
#[derive(Debug, Clone)]
struct AgentRow {
    revision: AgentRevision,
    fence: Fence,
    tail: Option<JournalSeq>,
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
    clock: Arc<FixedClock>,
    queue: Arc<MemoryQueue>,
    log: Arc<Recorder>,
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
            clock,
            queue,
            log,
        }
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
        self.agents.lock().expect("not poisoned").insert(
            key,
            AgentRow {
                revision: AgentRevision::ZERO,
                fence: Fence::ZERO,
                tail,
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

    /// Replaces the session-head authority a subsequent claim observes.
    pub fn set_authority(&self, session: SessionId, authority: SessionAuthority) {
        self.authorities
            .lock()
            .expect("not poisoned")
            .insert(session, authority);
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
            revision: row.revision,
            fence: row.fence,
            journal_tail: row.tail,
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
    ) -> BoxFuture<'a, Result<JournalPage, StoreError>> {
        Box::pin(async move {
            self.log.note("read_page");
            if let Some(fault) = self.read_faults.lock().expect("not poisoned").pop_front() {
                return Err(fault);
            }
            let agents = self.agents.lock().expect("not poisoned");
            let Some(row) = agents.get(key) else {
                return Ok(JournalPage {
                    entries: Vec::new(),
                    next: None,
                });
            };
            let entries: Vec<JournalEntry> = row
                .entries
                .iter()
                .filter(|entry| entry.envelope.seq >= from)
                .take(budget.max_entries)
                .cloned()
                .collect();
            let next = entries
                .last()
                .map(|entry| entry.envelope.seq.next())
                .filter(|_| entries.len() >= budget.max_entries);
            Ok(JournalPage { entries, next })
        })
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the fixture enforces the whole precondition set in one place; splitting it would let a reader believe a precondition is checked somewhere it is not"
    )]
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
            row.revision = commit.control.next_revision;
            row.tail = Some(commit.control.next_tail);
            row.phase.clone_from(&commit.control.phase);
            if let Some(finish) = commit.control.finish {
                row.finish = Some(finish);
            }
            let wakes: Vec<WakeId> = commit.wakes.iter().map(|wake| wake.id).collect();
            // The durable wake row is projected onto the queue, exactly as the
            // `regional-work` stream does. Nothing else ever puts a message there.
            for wake in &commit.wakes {
                self.queue.project(DurableWake {
                    id: wake.id,
                    key: wake.key,
                    dedup_key: wake.dedup_key.clone(),
                    reason: wake.reason.clone(),
                    due: wake.due,
                    priority: wake.priority,
                    tenant: wake.tenant.clone(),
                });
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
        effect: &'a EffectId,
        attempt: u16,
        at: Timestamp,
    ) -> BoxFuture<'a, Result<DispatchTicket, CommitError>> {
        Box::pin(async move {
            self.log.note("mark_dispatch_started");
            let mut agents = self.agents.lock().expect("not poisoned");
            let row = agents
                .get_mut(&guard.key())
                .ok_or(CommitError::Condition(ConditionFailure::StaleFence))?;
            if row.fence != guard.fence() {
                return Err(ConditionFailure::StaleFence.into());
            }
            let durable = row.effects.get_mut(effect).ok_or(CommitError::Condition(
                ConditionFailure::EffectStateMismatch { effect: *effect },
            ))?;
            // One durable pre-send write authorizes exactly one attempt.
            if !matches!(durable.state, EffectState::Prepared { .. }) {
                return Err(ConditionFailure::EffectStateMismatch { effect: *effect }.into());
            }
            durable.state = EffectState::DispatchStarted { attempt };
            durable.evidence = Some(DispatchEvidence::ambiguous(
                attempt,
                aex_brain_domain::effect::DispatchStage::Dispatched,
            ));
            Ok(DispatchTicket::mint(guard, *effect, attempt, at))
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
            if row.finish.is_some() {
                return Err(ClaimError::Terminal);
            }
            if row.lease_owner.is_some() && row.lease_expires_at.millis() > now.millis() {
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
            let mut agents = self.agents.lock().expect("not poisoned");
            let row = agents.get_mut(&claim.key).ok_or(ClaimError::Terminal)?;
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
        disposition: ReleaseDisposition,
    ) -> BoxFuture<'_, Result<(), StoreError>> {
        Box::pin(async move {
            self.log.note("release_lease");
            let mut agents = self.agents.lock().expect("not poisoned");
            if let Some(row) = agents.get_mut(&claim.key)
                && row.fence == claim.fence
                && row.lease_owner == Some(claim.owner)
            {
                row.lease_owner = None;
                row.lease_expires_at = if matches!(disposition, ReleaseDisposition::Drain) {
                    Timestamp::from_millis(0)
                } else {
                    row.lease_expires_at
                };
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
    visible: Mutex<VecDeque<WakeDelivery>>,
    acked: Mutex<Vec<WakeDelivery>>,
    ack_faults: Mutex<VecDeque<StoreError>>,
    receipts: AtomicU64,
    log: Mutex<Option<Arc<Recorder>>>,
}

impl MemoryQueue {
    /// An empty queue that records into `log`.
    #[must_use]
    pub fn new(log: Arc<Recorder>) -> Self {
        Self {
            visible: Mutex::new(VecDeque::new()),
            acked: Mutex::new(Vec::new()),
            ack_faults: Mutex::new(VecDeque::new()),
            receipts: AtomicU64::new(0),
            log: Mutex::new(Some(log)),
        }
    }

    /// Projects one durable wake onto the queue, as the work stream does.
    pub fn project(&self, wake: DurableWake) {
        let receipt = self.receipts.fetch_add(1, Ordering::SeqCst);
        self.visible
            .lock()
            .expect("not poisoned")
            .push_back(WakeDelivery {
                wake,
                receipt: format!("rh-{receipt}"),
                receive_count: 1,
            });
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
    ) -> BoxFuture<'_, Result<Vec<WakeDelivery>, StoreError>> {
        Box::pin(async move {
            self.note("receive");
            let mut visible = self.visible.lock().expect("not poisoned");
            let taken = visible.len().min(max);
            Ok(visible.drain(..taken).collect())
        })
    }

    fn extend_visibility<'a>(
        &'a self,
        _delivery: &'a WakeDelivery,
        _by: core::time::Duration,
    ) -> BoxFuture<'a, Result<(), StoreError>> {
        Box::pin(async { Ok(()) })
    }

    fn release(
        &self,
        delivery: WakeDelivery,
        _after: core::time::Duration,
    ) -> BoxFuture<'_, Result<(), StoreError>> {
        Box::pin(async move {
            self.note("release_delivery");
            let mut returned = delivery;
            returned.receive_count = returned.receive_count.saturating_add(1);
            self.visible
                .lock()
                .expect("not poisoned")
                .push_back(returned);
            Ok(())
        })
    }

    fn ack(&self, delivery: WakeDelivery) -> BoxFuture<'_, Result<(), StoreError>> {
        Box::pin(async move {
            self.note("ack");
            if let Some(fault) = self.ack_faults.lock().expect("not poisoned").pop_front() {
                return Err(fault);
            }
            self.acked.lock().expect("not poisoned").push(delivery);
            Ok(())
        })
    }

    fn due_scan(
        &self,
        _shard: WorkShard,
        _now: Timestamp,
        _max: usize,
    ) -> BoxFuture<'_, Result<Vec<DurableWake>, StoreError>> {
        Box::pin(async { Ok(Vec::new()) })
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
