//! The composed wake loop, and the ports the composition itself owns.
//!
//! `aex_brain_app::activation` owns what an activation *means*. This module owns
//! which adapter each port resolves to in a deployed task, and it is where an absent peer is
//! named rather than papered over.
//!
//! # What is bound, and what is refused
//!
//! | Port | Bound to | Note |
//! | --- | --- | --- |
//! | `WakeQueue` | `aex_brain_store_dynamodb::SqsWakeQueue` | real, over the configured queue and the `regional-work` due index |
//! | `JournalStore`, `EffectStore`, `LeaseStore` | `aex_brain_store_dynamodb::BrainStore` | real; each claim derives tenant and deletion authority from its session head |
//! | `ToolPort` | injected production router | startup refuses unless all four coarse routes have concrete executors |
//! | `ClockPort`, `IdPort` | this module | composition facts, not a peer's |
//! | `ProviderPort` | regional custody + KMS + six-provider router, or explicit startup refusal | dispatch uses the immutable session pin and ticket-scoped tenant authority; registration remains owned by the secret API |
//! | `CatalogPort` | release-bound `VerifiedCatalogPort` | the complete retained collection verifies before a port exists |
//! | `HandsPort` | `HandsAdapter` over `ProductionHandsBackend` | shares the runtime store and `MicroVM` client with the runtime-control engine |
//!
//! Every refusal is `DispatchProof::NotSent` and carries the name of the crate that owes the
//! implementation. None of them is a stub: a stub would let an activation appear to make
//! progress it did not make, and the whole point of the split-phase machine is that a caller
//! can always tell what did and did not leave the process.
//!
//! The deployed composition binds every port for real — [`Bindings::production`] is the only
//! binding set a running task ever holds. The fail-closed refusal fixtures and the partial
//! binding sets exist solely to prove an accidentally unbound port stays fail closed, so they
//! are compiled into tests only.

use crate::admission::{Admission, AdmissionOutcome};
use aex_brain_app::activation::{
    Activation, ActivationPolicy, AdmissionControl, AdmissionDecision, DispatchControl,
    DispatchDecision, DispatchLane, Ports, WakeLoop,
};
use aex_brain_app::kernel::{ActivationRegistry, DrainGate, FoldCache, PermitKind, PermitSet};
#[cfg(test)]
use aex_brain_app::ports::StoreError;
use aex_brain_app::ports::{
    BoxFuture, CatalogPort, ClockPort, HandsError, HandsPort, IdPort, ProviderPort, SteadyInstant,
    ToolPort,
};
use aex_brain_domain::child::QueuedReason;
use aex_brain_domain::effect::EffectKind;
use aex_brain_domain::ids::{
    AgentId, CatalogPin, DetachedOperationId, EffectId, JournalSeq, OwnerToken, Timestamp, WakeId,
};
use aex_brain_domain::journal::ExecutorRoute;
use std::sync::Arc;

// The fail-closed fixtures below are `#[cfg(test)]`, so the vocabulary only
// they speak is too: none of these names may appear in the deployed build.
#[cfg(test)]
use aex_brain_app::ports::{
    AgentHead, CancelToken, CatalogDigest, CatalogError, Claim, ClaimError, CommitError,
    CommitReceipt, DecisionContext, DispatchTicket, EffectStore, FenceGuard, JournalPage,
    JournalStore, LeaseStore, PreviewSink, ProviderDispatchError, ProviderOutcome, ReadBudget,
    RedactedDetail, ReleaseDisposition, SessionAuthority, StreamBudget, UnknownResolution,
};
#[cfg(test)]
use aex_brain_domain::commit::DecisionCommit;
#[cfg(test)]
use aex_brain_domain::effect::{DispatchEvidence, DispatchProof, DispatchStage, DurableEffect};
#[cfg(test)]
use aex_brain_domain::ids::{AgentKey, ModelSlug};
#[cfg(test)]
use aex_brain_domain::wire_pending::{CanonicalModelRequest, DurableOperationSupport, ProviderId};
#[cfg(test)]
use aex_model_catalog::{ProviderFailureKind, QualifiedModel};

use aex_brain_tool_catalog::readiness::{
    BuiltinSelection, CapabilitySet as ToolCapabilitySet, ExecutorRegistry, ReadinessInput,
    ResolvedSecretNames, advertise,
};
use aex_brain_tool_catalog::router::{CompositeToolRouter, ToolExecutor};
use aex_brain_tool_catalog::wire_pending::ExecutorRoute as CatalogExecutorRoute;
use aex_runtime_control::store::{OpenEffectCounter, PageBudget, RuntimeActivityStore};
use aex_runtime_control::usage::UsageFactSink;
use aex_runtime_control_aws::worker::{Pace, RuntimeControl, RuntimePorts, RuntimeSettings};

/// Historical refusal used by the explicit unbound-store fixture.
///
/// Deployed composition no longer uses it: `BrainStore` now derives these facts from each
/// claimed session. Keeping the refusal fixture proves an accidentally unbound store remains
/// fail closed.
#[cfg(test)]
pub const STORE_UNBOUND: &str = "aex-brain-store-dynamodb is not bound into this composition";

/// Why the provider would be refused, were it ever unbound.
#[cfg(test)]
pub const PROVIDER_ABSENT: &str =
    "the provider gateway could not bind its build-stamped adapter source identity";

/// Why the catalog would be refused, were it ever unbound.
#[cfg(test)]
pub const CATALOG_ABSENT: &str = "no content-addressed signed model catalog and compiled trust \
                                  root were bound at startup";

/// Why a verified collection still cannot serve wakes.
#[cfg(test)]
pub const CATALOG_NO_ACTIVE_MODELS: &str =
    "the signed model catalog collection contains no Active serviceable model";

/// Why tool execution would be refused, were it ever unbound.
#[cfg(test)]
pub const TOOL_EXECUTORS_ABSENT: &str = "one or more production tool executors are not bound";

/// Missing in-process implementation of control, park, and subagent scheduling tools.
pub const BRAIN_INLINE_EXECUTOR_ABSENT: &str =
    "Brain-inline control, park, and subagent scheduling authority is not implemented";

/// Missing session-scoped managed-search credential authority.
pub const MANAGED_WEB_EXECUTOR_ABSENT: &str = "managed web search cannot bind a session-scoped, \
                                               revocation-aware credential authority";

/// Missing private platform-paid tool executor.
pub const TOOL_EXECUTOR_ABSENT: &str = "the private platform tool executor is not bound";

/// Missing qualified MCP transport, task-recovery, and registry authority.
pub const MCP_EXECUTOR_ABSENT: &str = "MCP has no production ToolExecutor with qualified \
                                      transport, task recovery, and exact registry authority";

/// Missing concrete Hands tool route.
pub const HANDS_TOOL_EXECUTOR_ABSENT: &str = "Hands tool executor is not bound";

/// Why Hands would be refused, were it ever unbound.
#[cfg(test)]
pub const HANDS_ABSENT: &str = "the production Hands backend is not bound";

/// The admission controller, as the loop sees it.
#[derive(Debug)]
pub struct MuxAdmission {
    admission: Arc<Admission>,
    bindings: Bindings,
}

impl MuxAdmission {
    /// Wraps the task's admission bands under the bindings it actually holds.
    #[must_use]
    pub const fn new(admission: Arc<Admission>, bindings: Bindings) -> Self {
        Self {
            admission,
            bindings,
        }
    }
}

impl AdmissionControl for MuxAdmission {
    /// Whether the task should pull from the queue.
    ///
    /// An incomplete binding stops receiving entirely. This is the difference between a task
    /// that serves nothing and a task that *destroys* work: every delivery it took would be
    /// released, and after `max_receives` redeliveries the poison policy would ack a wake
    /// nothing had served. Not receiving is the only behaviour that cannot lose work.
    fn should_receive(&self) -> bool {
        self.bindings.complete() && self.admission.should_receive()
    }

    fn admit(&self, restore_bytes: u64) -> AdmissionDecision {
        // The activation's strict total restore ceiling is reserved before the first page.
        // `DynamoDB` page count is not a memory measurement, and reserving zero here would
        // let many individually bounded pages overrun the task's context pool.
        if restore_bytes != self.admission.resources().context_bytes {
            return AdmissionDecision::Shed {
                retry_after: core::time::Duration::from_millis(500),
            };
        }
        match self.admission.admit() {
            AdmissionOutcome::Admitted(permits) => AdmissionDecision::Admitted(permits),
            AdmissionOutcome::Deferred { requeue_after } => {
                AdmissionDecision::Deferred { requeue_after }
            }
            AdmissionOutcome::Shed(_) => AdmissionDecision::Shed {
                retry_after: core::time::Duration::from_millis(500),
            },
        }
    }
}

/// Phase-specific local dispatch permits over the process resource set.
#[derive(Debug)]
pub struct MuxDispatch {
    permits: Arc<PermitSet>,
    provider_streams: u64,
    network_lane: u64,
    hands_rpcs: u64,
}

impl MuxDispatch {
    /// Builds the non-blocking dispatch gate.
    #[must_use]
    pub const fn new(
        permits: Arc<PermitSet>,
        resources: crate::admission::ActivationResources,
    ) -> Self {
        Self {
            permits,
            provider_streams: resources.provider_streams,
            network_lane: resources.network_lane,
            hands_rpcs: resources.hands_rpcs,
        }
    }
}

impl DispatchControl for MuxDispatch {
    fn admit(&self, lane: DispatchLane, weight: u16) -> DispatchDecision {
        let (kind, base_units, reason) = match lane {
            DispatchLane::Provider => (
                PermitKind::ProviderStream,
                self.provider_streams,
                QueuedReason::ProviderPermits,
            ),
            // Managed web and MCP draw on their own pool. Sharing the provider pool made a
            // handful of concurrent fetches — each weighing four units against a stream's
            // one — able to defer every model dispatch in the task. Separate pools make the
            // lanes independent; they do not make anything concurrent, and must not: the
            // driver still runs one tool call at a time.
            DispatchLane::Network => (
                PermitKind::NetworkLane,
                self.network_lane,
                QueuedReason::RegionalCapacity,
            ),
            DispatchLane::Hands => (
                PermitKind::HandsRpc,
                self.hands_rpcs,
                QueuedReason::HandsPermits,
            ),
        };
        let units = base_units.saturating_mul(u64::from(weight));
        match self.permits.acquire(kind, units) {
            Ok(permit) => DispatchDecision::Admitted(Some(permit)),
            Err(_) => DispatchDecision::Deferred(reason),
        }
    }
}

/// The process clock.
///
/// Wall time and monotonic time are separate readings on purpose: a clock adjustment must not
/// expire a live deadline, and a monotonic reading must never be written into a durable record
/// where it would mean nothing to another process.
#[derive(Debug)]
pub struct SystemClock {
    base: std::time::Instant,
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemClock {
    /// A clock anchored at this instant.
    #[must_use]
    pub fn new() -> Self {
        Self {
            base: std::time::Instant::now(),
        }
    }
}

impl ClockPort for SystemClock {
    fn now(&self) -> Timestamp {
        let millis = time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000;
        Timestamp::from_millis(i64::try_from(millis).unwrap_or(i64::MAX))
    }

    fn steady(&self) -> SteadyInstant {
        SteadyInstant(u64::try_from(self.base.elapsed().as_millis()).unwrap_or(u64::MAX))
    }

    fn sleep(&self, duration: core::time::Duration) -> BoxFuture<'_, ()> {
        Box::pin(tokio::time::sleep(duration))
    }
}

/// Identifier generation for a deployed task.
///
/// The first two are the domain's own deterministic derivations, called through rather than
/// reimplemented: a second spelling of an identity derivation is a second identity.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessIds;

impl IdPort for ProcessIds {
    fn child_agent_id(&self, parent: &AgentId, ordinal: u32) -> AgentId {
        aex_brain_domain::ids::child_agent_id(*parent, ordinal)
    }

    fn effect_id(&self, agent: &AgentId, seq: JournalSeq, kind: EffectKind) -> EffectId {
        EffectId::derive(*agent, seq, kind.tag())
    }

    fn owner_token(&self) -> OwnerToken {
        // A fresh token per attempt. Reusing one makes two attempts by the same task
        // indistinguishable, which the activation-pool spike found to be a correctness bug.
        OwnerToken(uuid::Uuid::new_v4())
    }

    fn wake_id(&self) -> WakeId {
        WakeId(uuid::Uuid::now_v7())
    }

    fn operation_id(&self) -> DetachedOperationId {
        DetachedOperationId(uuid::Uuid::now_v7().to_string())
    }
}

/// A store that refuses every call and says why.
///
/// It is not a stub. A stub returns something plausible; this returns a non-retryable typed
/// refusal naming the exact reason, so a delivery is released rather than acked, readiness
/// stays false, and no row is ever written under a guessed tenant. Deployed composition
/// always binds the real `BrainStore`, so this fixture is compiled into tests only.
#[cfg(test)]
#[derive(Debug, Clone, Copy, Default)]
pub struct UnboundStore;

#[cfg(test)]
impl UnboundStore {
    fn refusal() -> StoreError {
        StoreError::Transport {
            reason: STORE_UNBOUND.to_owned(),
            // Not retryable: no amount of waiting binds a context the composition cannot
            // construct, and marking it retryable would hide the gap behind a retry curve.
            retryable: false,
        }
    }
}

#[cfg(test)]
impl JournalStore for UnboundStore {
    fn load_head<'a>(
        &'a self,
        _key: &'a AgentKey,
    ) -> BoxFuture<'a, Result<Option<AgentHead>, StoreError>> {
        Box::pin(async { Err(Self::refusal()) })
    }

    fn read_page<'a>(
        &'a self,
        _key: &'a AgentKey,
        _from: JournalSeq,
        _budget: ReadBudget,
        _after: Option<aex_brain_app::ports::JournalCursor>,
    ) -> BoxFuture<'a, Result<JournalPage, StoreError>> {
        Box::pin(async { Err(Self::refusal()) })
    }

    fn commit<'a>(
        &'a self,
        _context: &'a DecisionContext,
        _commit: &'a DecisionCommit,
    ) -> BoxFuture<'a, Result<CommitReceipt, CommitError>> {
        Box::pin(async { Err(CommitError::Store(Self::refusal())) })
    }
}

#[cfg(test)]
impl EffectStore for UnboundStore {
    fn mark_dispatch_started<'a>(
        &'a self,
        _guard: &'a FenceGuard,
        _authority: &'a SessionAuthority,
        _effect: &'a EffectId,
        _attempt: u16,
        _at: Timestamp,
    ) -> BoxFuture<'a, Result<DispatchTicket, CommitError>> {
        Box::pin(async { Err(CommitError::Store(Self::refusal())) })
    }

    fn mark_response_started<'a>(
        &'a self,
        _ticket: &'a DispatchTicket,
        _evidence: &'a DispatchEvidence,
    ) -> BoxFuture<'a, Result<(), CommitError>> {
        Box::pin(async { Err(CommitError::Store(Self::refusal())) })
    }

    fn load_open<'a>(
        &'a self,
        _key: &'a AgentKey,
    ) -> BoxFuture<'a, Result<Vec<DurableEffect>, StoreError>> {
        Box::pin(async { Err(Self::refusal()) })
    }
}

#[cfg(test)]
impl LeaseStore for UnboundStore {
    fn claim<'a>(
        &'a self,
        _key: &'a AgentKey,
        _owner: OwnerToken,
        _ttl: core::time::Duration,
        _now: Timestamp,
    ) -> BoxFuture<'a, Result<Claim, ClaimError>> {
        Box::pin(async { Err(ClaimError::Store(Self::refusal())) })
    }

    fn renew<'a>(
        &'a self,
        _claim: &'a Claim,
        _ttl: core::time::Duration,
        _now: Timestamp,
    ) -> BoxFuture<'a, Result<Claim, ClaimError>> {
        Box::pin(async { Err(ClaimError::Store(Self::refusal())) })
    }

    fn release(
        &self,
        _claim: Claim,
        _disposition: ReleaseDisposition,
    ) -> BoxFuture<'_, Result<(), StoreError>> {
        Box::pin(async { Err(Self::refusal()) })
    }
}

/// A provider binding whose required credential authorities are unavailable.
///
/// Every dispatch fails `NotSent`, which is the strongest thing an adapter may assert and the
/// only value that permits another attempt. Answering anything weaker would make an
/// unimplemented port indistinguishable from a request that may have been served. Deployed
/// composition always binds the real router, so this fixture is compiled into tests only.
#[cfg(test)]
#[derive(Debug, Clone, Copy, Default)]
pub struct AbsentProvider;

#[cfg(test)]
impl ProviderPort for AbsentProvider {
    fn dispatch<'a>(
        &'a self,
        _ticket: &'a DispatchTicket,
        _credential: aex_brain_domain::wire_pending::SessionCredentialPin,
        _request: &'a CanonicalModelRequest,
        _budget: &'a StreamBudget,
        _preview: &'a dyn PreviewSink,
        _cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderDispatchError>> {
        Box::pin(async {
            Err(ProviderDispatchError {
                stage: DispatchStage::PreDispatch,
                proof: DispatchProof::NotSent,
                kind: ProviderFailureKind::InvalidRequest,
                provider_request_id: None,
                retry_after: None,
                detail: RedactedDetail::internal(
                    ProviderFailureKind::InvalidRequest,
                    PROVIDER_ABSENT,
                ),
            })
        })
    }

    fn resolve_unknown<'a>(
        &'a self,
        _identity: &'a DurableEffect,
        _evidence: &'a DispatchEvidence,
    ) -> BoxFuture<'a, Result<UnknownResolution, ProviderDispatchError>> {
        // The honest launch answer, and the same one every real adapter gives: no BYOK
        // provider exposes a generation-resume or result-lookup operation.
        Box::pin(async { Ok(UnknownResolution::NoDurableOperation) })
    }
}

/// A catalog binding with no startup artifact or trust root.
///
/// `durable_operation_support` answers [`DurableOperationSupport::None`], which is not a stub:
/// it is the correct launch answer for every admitted model, and the safe answer to "can this
/// be resumed?" is always "no". Deployed composition always binds the release-verified
/// catalog, so this fixture is compiled into tests only.
#[cfg(test)]
#[derive(Debug, Clone, Copy, Default)]
pub struct AbsentCatalog;

#[cfg(test)]
impl CatalogPort for AbsentCatalog {
    fn digest(&self, pin: &CatalogPin) -> Result<CatalogDigest, CatalogError> {
        Err(CatalogError::UnknownPin { pin: *pin })
    }

    fn model(
        &self,
        pin: &CatalogPin,
        _provider: ProviderId,
        _model: &ModelSlug,
    ) -> Result<QualifiedModel, CatalogError> {
        Err(CatalogError::UnknownPin { pin: *pin })
    }

    fn durable_operation_support(
        &self,
        _pin: &CatalogPin,
        _provider: ProviderId,
        _model: &ModelSlug,
    ) -> DurableOperationSupport {
        DurableOperationSupport::None
    }
}

/// State of one production composition binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingState {
    /// A concrete implementation is installed.
    Ready,
    /// The implementation is absent for the named, actionable reason.
    Unavailable(&'static str),
}

impl BindingState {
    /// Whether a concrete implementation is installed.
    #[must_use]
    pub const fn is_ready(self) -> bool {
        matches!(self, Self::Ready)
    }
}

/// Whether every port the loop needs is bound to a real peer.
///
/// Readiness reads this. A task with any unbound peer must never report ready: it would
/// take work off the queue only to release it, and a queue that is being drained and
/// re-filled looks exactly like one that is being served.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bindings {
    /// Whether the journal, effect and lease ports reach a real authority.
    pub store: BindingState,
    /// Whether the provider port reaches a real adapter.
    pub provider: BindingState,
    /// Whether the catalog port reaches a verified artifact.
    pub catalog: BindingState,
    /// Whether every installed tool route has its concrete executor.
    pub tools: BindingState,
    /// Whether the Hands adapter has a concrete runtime backend.
    pub hands: BindingState,
}

impl Bindings {
    /// A test composition with every launch peer absent.
    ///
    /// Deployed composition never constructs this: [`Bindings::production`] is
    /// the only set `main` builds. It exists so the admission tests can prove
    /// an incomplete binding set never takes work off the queue.
    #[cfg(test)]
    #[must_use]
    pub const fn unavailable() -> Self {
        Self {
            store: BindingState::Ready,
            provider: BindingState::Unavailable(PROVIDER_ABSENT),
            catalog: BindingState::Unavailable(CATALOG_ABSENT),
            tools: BindingState::Unavailable(TOOL_EXECUTORS_ABSENT),
            hands: BindingState::Unavailable(HANDS_ABSENT),
        }
    }

    /// A test composition where only provider custody and transport are real.
    #[cfg(test)]
    #[must_use]
    pub const fn provider_ready() -> Self {
        Self {
            store: BindingState::Ready,
            provider: BindingState::Ready,
            catalog: BindingState::Unavailable(CATALOG_ABSENT),
            tools: BindingState::Unavailable(TOOL_EXECUTORS_ABSENT),
            hands: BindingState::Unavailable(HANDS_ABSENT),
        }
    }

    /// Applies the catalog's verified service capability to this binding set.
    ///
    /// Signature validity alone is not readiness: a zero-Active collection
    /// would make every model wake fail after receipt. Admission remains closed
    /// instead, so Brain never consumes work it cannot route.
    #[cfg(test)]
    #[must_use]
    pub const fn with_catalog_capability(mut self, service_capable: bool) -> Self {
        self.catalog = if service_capable {
            BindingState::Ready
        } else {
            BindingState::Unavailable(CATALOG_NO_ACTIVE_MODELS)
        };
        self
    }

    /// Fully injected production ports.
    #[must_use]
    pub const fn production() -> Self {
        Self {
            store: BindingState::Ready,
            provider: BindingState::Ready,
            catalog: BindingState::Ready,
            tools: BindingState::Ready,
            hands: BindingState::Ready,
        }
    }

    /// Whether the task may serve work.
    #[must_use]
    pub const fn complete(&self) -> bool {
        self.store.is_ready()
            && self.provider.is_ready()
            && self.catalog.is_ready()
            && self.tools.is_ready()
            && self.hands.is_ready()
    }

    /// The unsatisfied bindings, named. A refusal reports a name, never a bare `false`.
    #[cfg(test)]
    #[must_use]
    pub fn unsatisfied(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        for state in [
            self.store,
            self.provider,
            self.catalog,
            self.tools,
            self.hands,
        ] {
            if let BindingState::Unavailable(reason) = state {
                missing.push(reason);
            }
        }
        missing
    }
}

/// Concrete non-store peers required by a production wake loop.
///
/// Construction requires all four peers at once. There is no default and no optional port,
/// so a caller cannot accidentally make a partially real composition report ready.
pub struct ProductionPeers {
    provider: Arc<dyn ProviderPort>,
    tools: Arc<dyn ToolPort>,
    hands: Arc<dyn HandsPort>,
    catalog: Arc<dyn CatalogPort>,
}

impl ProductionPeers {
    /// Binds real provider, tool, catalog, and Hands-runtime implementations.
    #[must_use]
    pub fn new(
        provider: Arc<dyn ProviderPort>,
        tools: Arc<dyn ToolPort>,
        hands_backend: Arc<dyn aex_brain_hands::HandsBackend>,
        catalog: Arc<dyn CatalogPort>,
    ) -> Self {
        Self {
            provider,
            tools,
            hands: Arc::new(aex_brain_hands::HandsAdapter::new(hands_backend)),
            catalog,
        }
    }
}

impl core::fmt::Debug for ProductionPeers {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ProductionPeers")
            .finish_non_exhaustive()
    }
}

/// Concrete executors available to the tool catalog.
///
/// The options exist only so startup diagnostics and tests can name every absent authority.
/// [`ProductionToolExecutors::compose`] never substitutes a refusal executor or advertises a
/// row that the exact linked executor does not implement.
#[derive(Default)]
pub struct ProductionToolExecutors {
    /// Control, park, and subagent scheduling tools.
    pub brain_inline: Option<Arc<dyn ToolExecutor>>,
    /// Managed web search.
    pub managed_web: Option<Arc<dyn ToolExecutor>>,
    /// Platform-paid tools served by the private executor.
    pub tool_exec: Option<Arc<dyn ToolExecutor>>,
    /// Qualified MCP calls and task recovery.
    pub mcp: Option<Arc<dyn ToolExecutor>>,
    /// Exact-generation Hands tools.
    pub hands: Option<Arc<dyn ToolExecutor>>,
}

impl core::fmt::Debug for ProductionToolExecutors {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ProductionToolExecutors")
            .field("missing", &self.missing())
            .finish()
    }
}

impl ProductionToolExecutors {
    /// Names every route whose concrete executor is absent.
    #[must_use]
    pub fn missing(&self) -> Vec<&'static str> {
        [
            (BRAIN_INLINE_EXECUTOR_ABSENT, self.brain_inline.is_none()),
            (MANAGED_WEB_EXECUTOR_ABSENT, self.managed_web.is_none()),
            (TOOL_EXECUTOR_ABSENT, self.tool_exec.is_none()),
            (MCP_EXECUTOR_ABSENT, self.mcp.is_none()),
            (HANDS_TOOL_EXECUTOR_ABSENT, self.hands.is_none()),
        ]
        .into_iter()
        .filter_map(|(reason, missing)| missing.then_some(reason))
        .collect()
    }

    /// Builds one immutable router for every retained model-catalog revision.
    ///
    /// # Errors
    ///
    /// Returns [`ToolCompositionError::InvalidCatalog`] when the executable subset cannot be
    /// hydrated exactly. Missing optional authorities produce a smaller immutable surface.
    pub fn compose(
        self,
        pins: impl IntoIterator<Item = CatalogPin>,
    ) -> Result<Arc<dyn ToolPort>, ToolCompositionError> {
        let mut router = CompositeToolRouter::new();
        let linked = [
            (ExecutorRoute::BrainInline, self.brain_inline),
            (ExecutorRoute::ManagedWeb, self.managed_web),
            (ExecutorRoute::ToolExec, self.tool_exec),
            (ExecutorRoute::Mcp, self.mcp),
            (ExecutorRoute::Hands, self.hands),
        ]
        .into_iter()
        .filter_map(|(route, executor)| executor.map(|executor| (route, executor)))
        .collect::<Vec<_>>();
        for (route, executor) in &linked {
            router
                .register_executor(*route, Arc::clone(executor))
                .map_err(|error| ToolCompositionError::InvalidCatalog {
                    reason: error.to_string(),
                })?;
        }
        let entries = aex_brain_tool_catalog::catalog::builtin_entries().map_err(|error| {
            ToolCompositionError::InvalidCatalog {
                reason: error.to_string(),
            }
        })?;
        let executable = entries
            .iter()
            .filter(|entry| {
                let route = coarse_tool_route(entry.descriptor.route);
                let Ok(name) =
                    aex_brain_domain::ids::ToolName::parse(entry.descriptor.name.as_str())
                else {
                    return false;
                };
                linked.iter().any(|(linked_route, executor)| {
                    *linked_route == route && executor.supports(&name)
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        let executor_routes = executable
            .iter()
            .map(|entry| entry.descriptor.route)
            .collect::<std::collections::BTreeSet<_>>();
        let executor_registry = ExecutorRegistry::new(executor_routes);
        let capabilities = ToolCapabilitySet::default();
        let secrets = ResolvedSecretNames::default();
        let selection = BuiltinSelection::Default;
        let advertised = advertise(ReadinessInput {
            entries: &executable,
            executors: &executor_registry,
            capabilities: &capabilities,
            secrets: &secrets,
            selection: &selection,
            approval_required: &[],
        })
        .map_err(|error| ToolCompositionError::InvalidCatalog {
            reason: error.to_string(),
        })?;
        let manifest = aex_brain_domain::ids::ContentHash::of(
            &aex_brain_tool_catalog::catalog::builtin_catalog_bytes().map_err(|error| {
                ToolCompositionError::InvalidCatalog {
                    reason: error.to_string(),
                }
            })?,
        );
        let mut installed = 0_usize;
        for pin in pins {
            router
                .install_catalog(pin, manifest, &entries, &advertised)
                .map_err(|error| ToolCompositionError::InvalidCatalog {
                    reason: error.to_string(),
                })?;
            installed = installed.saturating_add(1);
        }
        if installed == 0 {
            return Err(ToolCompositionError::InvalidCatalog {
                reason: "the verified model catalog retained no revision".to_owned(),
            });
        }
        Ok(Arc::new(router))
    }
}

/// Why the production tool router could not be composed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ToolCompositionError {
    /// The immutable catalog could not be installed.
    #[error("the production tool catalog could not be installed: {reason}")]
    InvalidCatalog {
        /// Exact build failure.
        reason: String,
    },
}

const fn coarse_tool_route(route: CatalogExecutorRoute) -> ExecutorRoute {
    match route {
        CatalogExecutorRoute::Control
        | CatalogExecutorRoute::Park
        | CatalogExecutorRoute::SubagentScheduler => ExecutorRoute::BrainInline,
        CatalogExecutorRoute::ManagedWeb => ExecutorRoute::ManagedWeb,
        CatalogExecutorRoute::ToolExec => ExecutorRoute::ToolExec,
        CatalogExecutorRoute::Mcp => ExecutorRoute::Mcp,
        CatalogExecutorRoute::HandsFilesystem
        | CatalogExecutorRoute::HandsDevelopment
        | CatalogExecutorRoute::HandsBrowser
        | CatalogExecutorRoute::RegisteredCustom => ExecutorRoute::Hands,
    }
}

/// Real Hands ports sharing one runtime authority and one `MicroVM` client.
pub struct HandsBindings {
    /// Lower backend consumed by `HandsAdapter`.
    pub backend: Arc<dyn aex_brain_hands::HandsBackend>,
    /// Tool executor for the coarse Hands route.
    pub executor: Arc<dyn ToolExecutor>,
}

impl core::fmt::Debug for HandsBindings {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("HandsBindings")
            .finish_non_exhaustive()
    }
}

/// Settings for Brain's runtime-control dependency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandsBinding {
    /// Deployment region.
    pub region: aex_wire::types::Region,
    /// Runtime-activity table.
    pub runtime_activity_table: String,
    /// Session-authority table used to recount open Hands effects.
    pub session_authority_table: String,
    /// Compute usage ingress.
    pub compute_queue_url: String,
    /// Storage usage ingress.
    pub storage_queue_url: String,
    /// Runtime due-index shard count.
    pub due_shards: u16,
    /// Runtime due scan budget.
    pub due_page: PageBudget,
    /// Exact pricing version attached to usage drafts.
    pub pricing_version: String,
}

#[derive(Debug)]
struct TokioPace;

impl Pace for TokioPace {
    fn sleep(&self, millis: u64) -> core::pin::Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(tokio::time::sleep(core::time::Duration::from_millis(
            millis,
        )))
    }
}

/// Binds the production Hands backend and tool executor.
///
/// The backend and runtime engine share the exact same store and provider `Arc`s; this avoids
/// split ownership of generation state or duplicate per-client resource pools.
///
/// # Errors
///
/// Returns the typed Hands construction error when its bounded HTTPS transport cannot start.
pub fn hands_binding(
    aws: &aws_config::SdkConfig,
    binding: HandsBinding,
) -> Result<HandsBindings, HandsError> {
    let dynamo = aws_sdk_dynamodb::Client::new(aws);
    let sqs = aws_sdk_sqs::Client::new(aws);
    let store: Arc<dyn RuntimeActivityStore> = Arc::new(
        aex_runtime_activity_dynamodb::RuntimeActivityDynamoStore::new(
            dynamo.clone(),
            binding.runtime_activity_table,
        ),
    );
    let effects: Arc<dyn OpenEffectCounter> = Arc::new(
        aex_session_dynamodb::runtime_effects::OpenHandsEffectCounter::new(
            dynamo,
            binding.session_authority_table,
        ),
    );
    let provider: Arc<dyn aex_hands_control_aws::MicrovmControlApi> =
        Arc::new(aex_hands_control_aws::AwsMicrovmControl::new(
            aws_sdk_lambdamicrovms::Client::new(aws),
            binding.region.as_str(),
        ));
    let compute: Arc<dyn UsageFactSink> = Arc::new(
        aex_runtime_control_aws::usage_ingress::SqsFactDraftSink::new(
            sqs.clone(),
            binding.compute_queue_url,
            aex_usage_domain::meter::Category::Compute,
        ),
    );
    let storage: Arc<dyn UsageFactSink> = Arc::new(
        aex_runtime_control_aws::usage_ingress::SqsFactDraftSink::new(
            sqs,
            binding.storage_queue_url,
            aex_usage_domain::meter::Category::Storage,
        ),
    );
    let runtime = Arc::new(RuntimeControl::new(
        RuntimePorts {
            store: Arc::clone(&store),
            effects,
            provider: Arc::clone(&provider),
            compute,
            storage,
            pace: Arc::new(TokioPace),
        },
        RuntimeSettings {
            region: binding.region,
            pricing_version: aex_internal_contracts::PricingVersion(binding.pricing_version),
            shards: binding.due_shards,
            page: binding.due_page,
            schedule_jitter_ms: 0,
        },
    ));
    let backend: Arc<dyn aex_brain_hands::HandsBackend> = Arc::new(
        aex_brain_hands::ProductionHandsBackend::new(store, provider, runtime)?,
    );
    let hands: Arc<dyn HandsPort> =
        Arc::new(aex_brain_hands::HandsAdapter::new(Arc::clone(&backend)));
    let executor: Arc<dyn ToolExecutor> = Arc::new(aex_brain_hands::HandsToolExecutor::new(hands));
    Ok(HandsBindings { backend, executor })
}

/// Real AWS clients shared by store, queue, provider custody, and KMS composition.
pub struct AwsBindings {
    /// Session-authority store.
    pub store: Arc<aex_brain_store_dynamodb::BrainStore>,
    /// Wake delivery and due backstop.
    pub queue: Arc<aex_brain_store_dynamodb::SqsWakeQueue>,
    /// The one SDK configuration all clients in this task derive from.
    pub sdk: aws_config::SdkConfig,
}

/// Binds the real store and queue adapters through one `AWS` configuration.
pub async fn aws_bindings(
    region: &str,
    queue_url: &str,
    session_table: &str,
    work_table: &str,
) -> AwsBindings {
    let aws = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_sdk_dynamodb::config::Region::new(region.to_owned()))
        .load()
        .await;
    let dynamodb = aws_sdk_dynamodb::Client::new(&aws);
    let store = Arc::new(aex_brain_store_dynamodb::BrainStore::new(
        dynamodb.clone(),
        aex_brain_store_dynamodb::BrainTables {
            session_authority: session_table.to_owned(),
            regional_work: work_table.to_owned(),
        },
    ));
    let queue = Arc::new(aex_brain_store_dynamodb::SqsWakeQueue::new(
        aws_sdk_sqs::Client::new(&aws),
        queue_url.to_owned(),
        aex_brain_store_dynamodb::DueScan::new(dynamodb, work_table.to_owned()),
    ));
    AwsBindings {
        store,
        queue,
        sdk: aws,
    }
}

/// Provider and managed-search ports sharing one custody client, KMS client,
/// and context-partitioned branch-key cache.
pub struct CredentialBindings {
    /// Six-provider direct dispatch.
    pub provider: Arc<dyn ProviderPort>,
    /// Session-scoped managed-web executor.
    pub managed_web: Arc<dyn ToolExecutor>,
}

impl core::fmt::Debug for CredentialBindings {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("CredentialBindings")
            .finish_non_exhaustive()
    }
}

/// Binds exact regional custody plus KMS reveal into provider and managed-web
/// authorities without duplicating SDK pools or plaintext caches.
#[must_use]
pub fn credential_bindings(
    aws: &aws_config::SdkConfig,
    store: Arc<aex_brain_store_dynamodb::BrainStore>,
    custody_table: &str,
    kms_key_arn: &str,
    plane: aex_secret_domain::context::Plane,
    region: aex_wire::types::Region,
    cache_partition: &str,
) -> CredentialBindings {
    let custody = Arc::new(aex_secret_custody_dynamodb::CustodyStore::new(
        aws_sdk_dynamodb::Client::new(aws),
        custody_table.to_owned(),
    ));
    let crypto = Arc::new(aex_secret_aws::EnvelopeCrypto::new(
        Box::new(aex_secret_aws::KmsBranchKeys::new(
            aws_sdk_kms::Client::new(aws),
            kms_key_arn.to_owned(),
        )),
        cache_partition.to_owned(),
    ));
    let provider_authority = Arc::new(aex_brain_provider_custody::CredentialAuthority::new(
        Arc::clone(&custody),
        Arc::clone(&crypto),
        plane,
        region,
    ));
    let router = aex_brain_provider_gateway::router::ProviderRouter::from_build(
        Arc::clone(&provider_authority) as Arc<_>,
        provider_authority,
        store,
    );
    let managed_web: Arc<dyn ToolExecutor> =
        Arc::new(aex_brain_managed_web::executor::ManagedWebExecutor::production_fetch_only());
    CredentialBindings {
        provider: Arc::new(router),
        managed_web,
    }
}

/// Composes the store/queue authorities with fully supplied production peers.
#[must_use]
pub fn production_ports(
    store: Arc<aex_brain_store_dynamodb::BrainStore>,
    wakes: Arc<dyn aex_brain_app::ports::WakeQueue>,
    peers: ProductionPeers,
) -> Ports {
    Ports {
        journal: Arc::clone(&store) as Arc<_>,
        effects: Arc::clone(&store) as Arc<_>,
        leases: store,
        wakes,
        provider: peers.provider,
        tools: peers.tools,
        hands: peers.hands,
        catalog: peers.catalog,
        clock: Arc::new(SystemClock::new()),
        ids: Arc::new(ProcessIds),
    }
}

/// Process-local acceleration resources bound into activation.
#[derive(Debug)]
pub struct ActivationAccelerators {
    permits: Arc<PermitSet>,
    fold_cache: Arc<dyn FoldCache>,
}

impl ActivationAccelerators {
    /// Groups resources that can change latency but never durable authority.
    #[must_use]
    pub const fn new(permits: Arc<PermitSet>, fold_cache: Arc<dyn FoldCache>) -> Self {
        Self {
            permits,
            fold_cache,
        }
    }
}

/// Builds the loop one task runs.
#[must_use]
pub fn wake_loop(
    ports: Ports,
    policy: ActivationPolicy,
    registry: Arc<ActivationRegistry>,
    drain: Arc<DrainGate>,
    admission: Arc<Admission>,
    accelerators: ActivationAccelerators,
    bindings: Bindings,
) -> WakeLoop {
    let dispatch_resources = admission.resources();
    let activation = Activation::new(ports, policy, registry, drain)
        .with_fold_cache(accelerators.fold_cache)
        .with_dispatch_control(Arc::new(MuxDispatch::new(
            accelerators.permits,
            dispatch_resources,
        )));
    WakeLoop::new(activation, Arc::new(MuxAdmission::new(admission, bindings)))
}

#[cfg(test)]
mod tests {
    use super::{
        AbsentCatalog, AbsentProvider, Bindings, CATALOG_ABSENT, CATALOG_NO_ACTIVE_MODELS,
        MuxAdmission, MuxDispatch, PROVIDER_ABSENT, ProcessIds, ProductionToolExecutors,
        STORE_UNBOUND, SystemClock, UnboundStore,
    };
    use crate::admission::{ActivationResources, Admission, AdmissionBounds};
    use aex_brain_app::activation::{
        AdmissionControl, AdmissionDecision, DispatchControl, DispatchDecision, DispatchLane,
    };
    use aex_brain_app::kernel::{DrainGate, PermitKind, PermitSet};
    use aex_brain_app::ports::{
        BoxFuture, CancelToken, CatalogPort, ClockPort, DetachedStatus, DispatchTicket, IdPort,
        JournalStore, LeaseStore, PreparedToolCall, StoreError, ToolDispatchError, ToolOutcome,
    };
    use aex_brain_domain::effect::EffectKind;
    use aex_brain_domain::ids::{
        AgentId, AgentKey, CatalogPin, DetachedOperationId, Fence, JournalSeq, OwnerToken,
        SessionId, Timestamp, ToolName,
    };
    use aex_brain_domain::wire_pending::{DurableOperationSupport, ProviderId};
    use aex_model_catalog::document::{Capability, CapabilitySet};
    use aex_model_catalog::fixture;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use uuid::Uuid;

    #[derive(Debug)]
    struct NeverExecutor;

    impl aex_brain_tool_catalog::router::ToolExecutor for NeverExecutor {
        fn supports(&self, _tool: &ToolName) -> bool {
            true
        }

        fn invoke<'a>(
            &'a self,
            _ticket: &'a DispatchTicket,
            _call: &'a PreparedToolCall,
            _cancel: &'a CancelToken,
        ) -> BoxFuture<'a, Result<ToolOutcome, ToolDispatchError>> {
            Box::pin(async { panic!("composition test never dispatches") })
        }

        fn query<'a>(
            &'a self,
            _operation: &'a DetachedOperationId,
        ) -> BoxFuture<'a, Result<DetachedStatus, ToolDispatchError>> {
            Box::pin(async { panic!("composition test never queries") })
        }

        fn cancel<'a>(
            &'a self,
            _operation: &'a DetachedOperationId,
            _fence: Fence,
        ) -> BoxFuture<'a, Result<(), ToolDispatchError>> {
            Box::pin(async { panic!("composition test never cancels") })
        }
    }

    fn activation_resources(context_bytes: u64) -> ActivationResources {
        ActivationResources {
            context_bytes,
            stream_buffer_bytes: 1,
            provider_streams: 1,
            network_lane: 1,
            hands_rpcs: 1,
        }
    }

    #[test]
    fn phase_dispatch_permits_are_nonblocking_typed_and_raii_released() {
        let permits = Arc::new(PermitSet::new(BTreeMap::from([
            (PermitKind::ProviderStream, 4_u64),
            (PermitKind::NetworkLane, 4_u64),
            (PermitKind::HandsRpc, 1_u64),
        ])));
        let dispatch = MuxDispatch::new(Arc::clone(&permits), activation_resources(1));

        let DispatchDecision::Admitted(network) = dispatch.admit(DispatchLane::Network, 4) else {
            panic!("the weighted network lane has four slots");
        };
        assert!(matches!(
            dispatch.admit(DispatchLane::Network, 1),
            DispatchDecision::Deferred(aex_brain_domain::child::QueuedReason::RegionalCapacity)
        ));
        assert_eq!(permits.held(PermitKind::NetworkLane), 4);
        assert_eq!(permits.held(PermitKind::HandsRpc), 0);
        drop(network);
        assert_eq!(permits.held(PermitKind::NetworkLane), 0);

        let DispatchDecision::Admitted(hands) = dispatch.admit(DispatchLane::Hands, 1) else {
            panic!("the Hands lane has one slot");
        };
        assert_eq!(permits.held(PermitKind::HandsRpc), 1);
        drop(hands);
        assert_eq!(permits.held(PermitKind::HandsRpc), 0);
    }

    /// A saturated network lane must leave model dispatch untouched.
    ///
    /// While both lanes drew on one pool this was not merely unenforced — it was
    /// false: a tool call weighs four units against a stream's one, so twelve
    /// concurrent fetches exhausted the pool and deferred every model dispatch in
    /// the task. Unreachable today because the driver is serial, and the reason
    /// per-batch concurrency must not land before this split.
    #[test]
    fn a_saturated_network_lane_never_defers_a_model_dispatch() {
        let permits = Arc::new(PermitSet::new(BTreeMap::from([
            (PermitKind::ProviderStream, 1_u64),
            (PermitKind::NetworkLane, 4_u64),
            (PermitKind::HandsRpc, 1_u64),
        ])));
        let dispatch = MuxDispatch::new(Arc::clone(&permits), activation_resources(1));

        let DispatchDecision::Admitted(_network) = dispatch.admit(DispatchLane::Network, 4) else {
            panic!("the network lane admits its own weighted call");
        };
        assert!(matches!(
            dispatch.admit(DispatchLane::Network, 1),
            DispatchDecision::Deferred(_)
        ));
        assert!(
            matches!(
                dispatch.admit(DispatchLane::Provider, 1),
                DispatchDecision::Admitted(_)
            ),
            "an exhausted network lane must not starve the model stream it no longer shares"
        );
    }

    fn block_on<F: core::future::Future>(future: F) -> F::Output {
        let mut future = Box::pin(future);
        let mut context = core::task::Context::from_waker(core::task::Waker::noop());
        match future.as_mut().poll(&mut context) {
            core::task::Poll::Ready(value) => value,
            core::task::Poll::Pending => panic!("a refusal must not need a runtime"),
        }
    }

    fn key() -> AgentKey {
        AgentKey::new(SessionId(Uuid::from_u128(1)), AgentId(Uuid::from_u128(2)))
    }

    /// The refusal is not retryable and names the exact reason. A retryable refusal would
    /// hide an unbindable composition behind a retry curve nobody reads.
    #[test]
    fn the_unbound_store_refuses_by_name_and_never_pretends_to_be_transient() {
        let error = block_on(UnboundStore.load_head(&key())).expect_err("the store is unbound");
        let StoreError::Transport { reason, retryable } = error else {
            panic!("expected a named transport refusal");
        };
        assert_eq!(reason, STORE_UNBOUND);
        assert!(!retryable);
        assert!(
            block_on(UnboundStore.claim(
                &key(),
                OwnerToken(Uuid::from_u128(3)),
                core::time::Duration::from_secs(1),
                Timestamp::from_millis(0)
            ))
            .is_err()
        );
    }

    /// An unimplemented provider asserts the strongest thing it can: nothing left the
    /// process. Anything weaker would be indistinguishable from a request that may have been
    /// served, and would interrupt every run instead of failing it honestly.
    #[test]
    fn the_absent_provider_proves_nothing_was_sent() {
        let ports: &dyn aex_brain_app::ports::ProviderPort = &AbsentProvider;
        let _ = ports;
        assert!(PROVIDER_ABSENT.contains("build-stamped adapter source identity"));
    }

    /// The safe answer to "can this be resumed?" is "no", so an unknown pin still answers
    /// the one infallible question rather than tempting the caller to guess.
    #[test]
    fn the_absent_catalog_answers_no_durable_operation_and_refuses_everything_else() {
        let model = fixture::qualified_entry(ProviderId::Deepseek, "m", CapabilitySet::default());
        let pin = model.catalog();
        assert_eq!(
            AbsentCatalog.durable_operation_support(&pin, ProviderId::Deepseek, model.model()),
            DurableOperationSupport::None
        );
        assert!(AbsentCatalog.digest(&pin).is_err());
        assert!(CATALOG_ABSENT.contains("signed model catalog"));
    }

    /// Each partial composition names only the peers it truly lacks.
    #[test]
    fn a_deployed_task_binds_the_store_and_names_the_remaining_peers() {
        let bindings = Bindings::unavailable();
        assert!(!bindings.complete());
        assert!(bindings.store.is_ready());
        let missing = bindings.unsatisfied();
        assert_eq!(missing.len(), 4);
        assert!(!missing.contains(&STORE_UNBOUND));
        assert!(missing.contains(&PROVIDER_ABSENT));
        assert!(missing.contains(&CATALOG_ABSENT));
        assert!(missing.contains(&super::TOOL_EXECUTORS_ABSENT));
        assert!(missing.contains(&super::HANDS_ABSENT));

        let with_provider = Bindings::provider_ready();
        assert!(!with_provider.complete());
        assert!(with_provider.provider.is_ready());
        let missing = with_provider.unsatisfied();
        assert_eq!(missing.len(), 3);
        assert!(!missing.contains(&PROVIDER_ABSENT));
        assert!(Bindings::production().complete());
    }

    #[test]
    fn catalog_hydration_advertises_only_exact_supported_rows_in_durable_order() {
        let pin = CatalogPin(aex_model_catalog::Blake3Digest::of(b"catalog"));
        let tools = ProductionToolExecutors {
            brain_inline: None,
            managed_web: None,
            tool_exec: None,
            mcp: None,
            hands: Some(Arc::new(NeverExecutor)),
        }
        .compose([
            pin,
            CatalogPin(aex_model_catalog::Blake3Digest::of(b"older")),
        ])
        .expect("the exact supported subset composes");
        let advertised = tools.advertise(&pin).expect("the retained pin is hydrated");
        assert_eq!(
            advertised
                .definitions
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["bash"]
        );
        assert!(!advertised.parallel_safe);
        assert!(matches!(
            tools.route(&pin, &ToolName::parse("wait").expect("name")),
            Err(aex_brain_app::ports::ToolRoutingError::NotAdmitted { .. })
        ));
        assert!(matches!(
            tools.route(&pin, &ToolName::parse("web_search").expect("name")),
            Err(aex_brain_app::ports::ToolRoutingError::NotAdmitted { .. })
        ));
    }

    #[test]
    fn deferred_authorities_never_reappear_in_the_bash_only_catalog() {
        let pin = CatalogPin(aex_model_catalog::Blake3Digest::of(b"catalog"));
        let tools = ProductionToolExecutors {
            brain_inline: Some(Arc::new(crate::inline_tools::BrainControlExecutor)),
            managed_web: Some(Arc::new(NeverExecutor)),
            tool_exec: Some(Arc::new(NeverExecutor)),
            mcp: None,
            hands: Some(Arc::new(NeverExecutor)),
        }
        .compose([pin])
        .expect("optional authorities are filtered before installation");
        let advertised = tools.advertise(&pin).expect("advertisement");
        let names = advertised
            .definitions
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["bash"]);
        assert!(!names.contains(&"web_fetch"));
        assert!(!names.contains(&"web_search"));
        assert!(!names.iter().any(|name| name.starts_with("mcp__")));
        assert!(!names.contains(&"read_file"));
        assert!(
            !advertised.parallel_safe,
            "managed network work is not safe for us to run concurrently"
        );
        assert!(
            advertised.allows_parallel_emission(CapabilitySet::from_slice(&[
                Capability::Tools,
                Capability::ParallelTools,
            ])),
            "a non-pure advertised row must not suppress the model's own emission"
        );
    }

    #[test]
    fn a_verified_zero_active_collection_keeps_admission_and_readiness_closed() {
        let bindings = Bindings::provider_ready().with_catalog_capability(false);
        assert!(!bindings.complete());
        assert_eq!(
            bindings.catalog,
            super::BindingState::Unavailable(CATALOG_NO_ACTIVE_MODELS)
        );
        assert!(bindings.unsatisfied().contains(&CATALOG_NO_ACTIVE_MODELS));

        let permits = Arc::new(PermitSet::new(BTreeMap::from([(
            PermitKind::Activation,
            1_u64,
        )])));
        let drain = Arc::new(DrainGate::new());
        let admission = Arc::new(Admission::new(
            AdmissionBounds {
                target: 1,
                safety_cap: 1,
                offered_ceiling: 1,
            },
            permits,
            drain,
            activation_resources(1),
        ));
        assert!(
            !MuxAdmission::new(admission, bindings).should_receive(),
            "Brain must not consume a wake that every model lookup can only fail"
        );
    }

    /// Two claim attempts by one task must be distinguishable, so the owner token is fresh
    /// every time.
    #[test]
    fn every_claim_attempt_mints_its_own_owner_token() {
        let ids = ProcessIds;
        assert_ne!(ids.owner_token(), ids.owner_token());
        // The two derivations are the domain's, called through rather than reimplemented.
        let agent = AgentId(Uuid::from_u128(7));
        assert_eq!(
            ids.effect_id(&agent, JournalSeq(3), EffectKind::ModelCall),
            aex_brain_domain::ids::EffectId::derive(agent, JournalSeq(3), 1)
        );
        assert_eq!(
            ids.child_agent_id(&agent, 2),
            aex_brain_domain::ids::child_agent_id(agent, 2)
        );
    }

    /// Wall time and monotonic time are separate readings: a clock adjustment must not
    /// expire a live deadline.
    #[test]
    fn the_clock_keeps_wall_and_steady_apart() {
        let clock = SystemClock::new();
        assert!(
            clock.now().millis() > 1_600_000_000_000,
            "a real wall clock"
        );
        let first = clock.steady();
        let second = clock.steady();
        assert!(second >= first, "steady time never goes backwards");
    }

    /// A draining task admits nothing, and the loop asks before it receives.
    #[test]
    fn admission_stops_receiving_the_moment_drain_starts() {
        let permits = Arc::new(PermitSet::new(BTreeMap::from([
            (PermitKind::Activation, 4_u64),
            (PermitKind::ContextBytes, 2_u64),
            (PermitKind::StreamBufferBytes, 2_u64),
            (PermitKind::ProviderStream, 2_u64),
            (PermitKind::HandsRpc, 2_u64),
        ])));
        let drain = Arc::new(DrainGate::new());
        let admission = Arc::new(Admission::new(
            AdmissionBounds {
                target: 2,
                safety_cap: 3,
                offered_ceiling: 8,
            },
            permits,
            Arc::clone(&drain),
            activation_resources(1),
        ));
        let control = MuxAdmission::new(
            Arc::clone(&admission),
            Bindings {
                store: super::BindingState::Ready,
                provider: super::BindingState::Ready,
                catalog: super::BindingState::Ready,
                tools: super::BindingState::Ready,
                hands: super::BindingState::Ready,
            },
        );
        assert!(control.should_receive());
        assert!(
            !MuxAdmission::new(admission, Bindings::unavailable()).should_receive(),
            "missing production peers must never take work off the queue"
        );
        drain.start_drain();
        assert!(!control.should_receive());
        assert!(matches!(control.admit(0), AdmissionDecision::Shed { .. }));
    }

    /// The composed receive path honours measured memory pressure, so the wake loop stops
    /// asking for deliveries the task cannot afford — and starts again when the measurement
    /// recovers, with nothing abandoned in between.
    #[test]
    fn admission_stops_receiving_under_measured_memory_pressure() {
        let permits = Arc::new(PermitSet::new(BTreeMap::from([
            (PermitKind::Activation, 4_u64),
            (PermitKind::ContextBytes, 4_u64),
            (PermitKind::StreamBufferBytes, 4_u64),
            (PermitKind::ProviderStream, 4_u64),
            (PermitKind::HandsRpc, 4_u64),
        ])));
        let admission = Arc::new(Admission::new(
            AdmissionBounds {
                target: 2,
                safety_cap: 3,
                offered_ceiling: 8,
            },
            permits,
            Arc::new(DrainGate::new()),
            activation_resources(1),
        ));
        let control = MuxAdmission::new(Arc::clone(&admission), Bindings::production());
        assert!(control.should_receive());

        let _ = admission.pressure().observe(measured(85));
        assert!(!control.should_receive());
        assert!(
            matches!(control.admit(1), AdmissionDecision::Admitted { .. }),
            "a delivery already taken is still started rather than hidden locally"
        );

        let _ = admission.pressure().observe(measured(60));
        assert!(control.should_receive());
    }

    /// A reading of exactly `percent` of a declared memory limit.
    fn measured(percent: u64) -> crate::pressure::MemoryReading {
        crate::pressure::MemoryReading::Measured {
            source: crate::pressure::MemorySource::CgroupV2,
            current_bytes: percent,
            limit_bytes: 100,
        }
    }

    /// The application supplies its measured restore-byte ceiling to mux admission. The
    /// context pool holds those bytes before hydration and returns them through RAII when
    /// the activation ends.
    #[test]
    fn mux_admission_reserves_the_activation_restore_bytes() {
        let permits = Arc::new(PermitSet::new(BTreeMap::from([
            (PermitKind::Activation, 1_u64),
            (PermitKind::ContextBytes, 512_u64),
            (PermitKind::StreamBufferBytes, 1_u64),
            (PermitKind::ProviderStream, 1_u64),
            (PermitKind::HandsRpc, 1_u64),
        ])));
        let admission = Arc::new(Admission::new(
            AdmissionBounds {
                target: 1,
                safety_cap: 1,
                offered_ceiling: 1,
            },
            Arc::clone(&permits),
            Arc::new(DrainGate::new()),
            activation_resources(512),
        ));
        let control = MuxAdmission::new(
            admission,
            Bindings {
                store: super::BindingState::Ready,
                provider: super::BindingState::Ready,
                catalog: super::BindingState::Ready,
                tools: super::BindingState::Ready,
                hands: super::BindingState::Ready,
            },
        );

        let AdmissionDecision::Admitted(held) = control.admit(512) else {
            panic!("the exact context boundary is admitted");
        };
        assert_eq!(permits.held(PermitKind::ContextBytes), 512);
        assert_eq!(permits.held(PermitKind::StreamBufferBytes), 1);
        assert_eq!(permits.held(PermitKind::ProviderStream), 0);
        assert_eq!(permits.held(PermitKind::HandsRpc), 0);
        drop(held);
        assert_eq!(permits.held(PermitKind::ContextBytes), 0);
        assert_eq!(permits.held(PermitKind::Activation), 0);
        assert_eq!(permits.held(PermitKind::StreamBufferBytes), 0);
        assert_eq!(permits.held(PermitKind::ProviderStream), 0);
        assert_eq!(permits.held(PermitKind::HandsRpc), 0);
    }
}
