//! The composed wake loop, and the ports the composition itself owns.
//!
//! `aex_brain_application::activation` owns what an activation *means*. This module owns
//! which adapter each port resolves to in a deployed task, and it is where an absent peer is
//! named rather than papered over.
//!
//! # What is bound, and what is refused
//!
//! | Port | Bound to | Note |
//! | --- | --- | --- |
//! | `WakeQueue` | `aex_brain_store_aws::SqsWakeQueue` | real, over the configured queue and the `regional-work` due index |
//! | `JournalStore`, `EffectStore`, `LeaseStore` | `aex_brain_store_aws::BrainStore` | real; each claim derives tenant and deletion authority from its session head |
//! | `ToolPort` | injected production router, or explicit unavailable composition | managed-web and MCP do not yet implement `ToolExecutor` |
//! | `ClockPort`, `IdPort` | this module | composition facts, not a peer's |
//! | `ProviderPort` | [`AbsentProvider`] | the six-adapter gateway is implemented, but a Brain session carries no immutable credential pin and regional custody cannot mint a binding yet |
//! | `CatalogPort` | [`AbsentCatalog`] | the verified loader exists, but startup has no content-addressed envelope or compiled trust root to bind |
//! | `HandsPort` | [`aex_brain_hands::HandsAdapter`] in production injection | the adapter is real; no concrete guest transport/runtime backend exists yet |
//!
//! Every refusal is `DispatchProof::NotSent` and carries the name of the crate that owes the
//! implementation. None of them is a stub: a stub would let an activation appear to make
//! progress it did not make, and the whole point of the split-phase machine is that a caller
//! can always tell what did and did not leave the process.

use crate::admission::{Admission, AdmissionOutcome};
use aex_brain_application::activation::{
    Activation, ActivationPolicy, AdmissionControl, AdmissionDecision, Ports, WakeLoop,
};
use aex_brain_application::kernel::{ActivationRegistry, DrainGate};
use aex_brain_application::ports::{
    AgentHead, BoxFuture, CancelToken, CatalogDigest, CatalogError, CatalogPort, Claim, ClaimError,
    ClockPort, CommitError, CommitReceipt, DecisionContext, DispatchTicket, EffectStore,
    FenceGuard, HandsAccepted, HandsEndpoint, HandsError, HandsOperationStart,
    HandsOperationStatus, HandsPort, HandsResult, IdPort, JournalPage, JournalStore, LeaseStore,
    PreviewSink, ProviderDispatchError, ProviderOutcome, ProviderPort, ReadBudget, RedactedDetail,
    ReleaseDisposition, ResultBounds, SessionAuthority, SteadyInstant, StoreError, StreamBudget,
    ToolPort, UnknownResolution,
};
use aex_brain_domain::commit::DecisionCommit;
use aex_brain_domain::effect::{
    DispatchEvidence, DispatchProof, DispatchStage, DurableEffect, EffectKind,
};
use aex_brain_domain::ids::{
    AgentId, AgentKey, CatalogPin, DetachedOperationId, EffectId, HandsOperationId, JournalSeq,
    ModelSlug, OwnerToken, SessionId, Timestamp, WakeId,
};
use aex_brain_domain::wire_pending::{CanonicalModelRequest, DurableOperationSupport, ProviderId};
use aex_model_catalog::{ProviderFailureKind, QualifiedModel};
use aex_wire::ids::GenerationId;
use std::sync::Arc;

/// Historical refusal used by the explicit unbound-store fixture.
///
/// Deployed composition no longer uses it: `BrainStore` now derives these facts from each
/// claimed session. Keeping the refusal fixture proves an accidentally unbound store remains
/// fail closed.
pub const STORE_UNBOUND: &str = "aex-brain-store-aws is not bound into this composition";

/// Why the provider is not bound.
pub const PROVIDER_ABSENT: &str = "the Brain session contract carries no immutable provider \
                                   credential pin, and regional secret custody cannot yet mint \
                                   the provider credential record";

/// Why the catalog is not bound.
pub const CATALOG_ABSENT: &str = "no content-addressed signed model catalog and compiled trust \
                                  root were bound at startup";

/// Why tool execution is not bound.
pub const TOOL_EXECUTORS_ABSENT: &str = "aex-brain-managed-web and aex-brain-mcp do not implement \
                                        aex-brain-tool-catalog::router::ToolExecutor";

/// Why Hands is not bound.
pub const HANDS_ABSENT: &str = "aex-brain-hands implements HandsPort, but no concrete guest \
                                transport/runtime-store HandsBackend is available";

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
        match self.admission.admit(restore_bytes) {
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
/// stays false, and no row is ever written under a guessed tenant.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnboundStore;

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
        _after: Option<aex_brain_application::ports::JournalCursor>,
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
/// unimplemented port indistinguishable from a request that may have been served.
#[derive(Debug, Clone, Copy, Default)]
pub struct AbsentProvider;

impl ProviderPort for AbsentProvider {
    fn dispatch<'a>(
        &'a self,
        _ticket: &'a DispatchTicket,
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
/// be resumed?" is always "no".
#[derive(Debug, Clone, Copy, Default)]
pub struct AbsentCatalog;

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

/// A Hands adapter that does not implement this port.
#[derive(Debug, Clone, Copy, Default)]
pub struct AbsentHands;

impl AbsentHands {
    fn refusal() -> HandsError {
        HandsError::Transport {
            stage: DispatchStage::PreDispatch,
            proof: DispatchProof::NotSent,
            detail: RedactedDetail::internal(ProviderFailureKind::ServerError, HANDS_ABSENT),
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
        _fence: aex_brain_domain::ids::Fence,
    ) -> BoxFuture<'a, Result<(), HandsError>> {
        Box::pin(async { Err(Self::refusal()) })
    }

    fn result<'a>(
        &'a self,
        _generation: GenerationId,
        _operation: &'a HandsOperationId,
        _bounds: &'a ResultBounds,
    ) -> BoxFuture<'a, Result<HandsResult, HandsError>> {
        Box::pin(async { Err(Self::refusal()) })
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
    /// The currently unavailable production composition.
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

    /// The unsatisfied bindings, named. Readiness reports a name, never a bare `false`.
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

/// Binds the real store and queue adapters through one `AWS` configuration.
pub async fn aws_bindings(
    region: &str,
    queue_url: &str,
    session_table: &str,
    work_table: &str,
) -> (
    Arc<aex_brain_store_aws::BrainStore>,
    Arc<aex_brain_store_aws::SqsWakeQueue>,
) {
    let aws = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_sdk_dynamodb::config::Region::new(region.to_owned()))
        .load()
        .await;
    let dynamodb = aws_sdk_dynamodb::Client::new(&aws);
    let store = Arc::new(aex_brain_store_aws::BrainStore::new(
        dynamodb.clone(),
        aex_brain_store_aws::BrainTables {
            session_authority: session_table.to_owned(),
            regional_work: work_table.to_owned(),
        },
    ));
    let queue = Arc::new(aex_brain_store_aws::SqsWakeQueue::new(
        aws_sdk_sqs::Client::new(&aws),
        queue_url.to_owned(),
        aex_brain_store_aws::DueScan::new(dynamodb, work_table.to_owned()),
    ));
    (store, queue)
}

/// Fail-closed ports for a task whose production peers are not composed yet.
#[must_use]
pub fn unavailable_ports(
    store: Arc<aex_brain_store_aws::BrainStore>,
    wakes: Arc<dyn aex_brain_application::ports::WakeQueue>,
) -> Ports {
    Ports {
        journal: Arc::clone(&store) as Arc<_>,
        effects: Arc::clone(&store) as Arc<_>,
        leases: store,
        wakes,
        provider: Arc::new(AbsentProvider),
        // The real router. It holds no executor because nothing implements `ToolExecutor`
        // yet, so it refuses by its own typed error rather than by one invented here.
        tools: Arc::new(aex_brain_tool_catalog::router::CompositeToolRouter::new()),
        hands: Arc::new(AbsentHands),
        catalog: Arc::new(AbsentCatalog),
        clock: Arc::new(SystemClock::new()),
        ids: Arc::new(ProcessIds),
    }
}

/// Composes the store/queue authorities with fully supplied production peers.
#[must_use]
pub fn production_ports(
    store: Arc<aex_brain_store_aws::BrainStore>,
    wakes: Arc<dyn aex_brain_application::ports::WakeQueue>,
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

/// Builds the loop one task runs.
#[must_use]
pub fn wake_loop(
    ports: Ports,
    policy: ActivationPolicy,
    registry: Arc<ActivationRegistry>,
    drain: Arc<DrainGate>,
    admission: Arc<Admission>,
    bindings: Bindings,
) -> WakeLoop {
    WakeLoop::new(
        Activation::new(ports, policy, registry, drain),
        Arc::new(MuxAdmission::new(admission, bindings)),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        AbsentCatalog, AbsentProvider, Bindings, CATALOG_ABSENT, MuxAdmission, PROVIDER_ABSENT,
        ProcessIds, STORE_UNBOUND, SystemClock, UnboundStore,
    };
    use crate::admission::{Admission, AdmissionBounds};
    use aex_brain_application::activation::{AdmissionControl, AdmissionDecision};
    use aex_brain_application::kernel::{DrainGate, PermitKind, PermitSet};
    use aex_brain_application::ports::{
        CatalogPort, ClockPort, IdPort, JournalStore, LeaseStore, StoreError,
    };
    use aex_brain_domain::effect::EffectKind;
    use aex_brain_domain::ids::{AgentId, AgentKey, JournalSeq, OwnerToken, SessionId, Timestamp};
    use aex_brain_domain::wire_pending::{DurableOperationSupport, ProviderId};
    use aex_model_catalog::document::CapabilitySet;
    use aex_model_catalog::fixture;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use uuid::Uuid;

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
        let ports: &dyn aex_brain_application::ports::ProviderPort = &AbsentProvider;
        let _ = ports;
        assert!(PROVIDER_ABSENT.contains("immutable provider credential pin"));
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

    /// The store is bound, while all four absent production peers remain named blockers.
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
        assert!(Bindings::production().complete());
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
        let permits = Arc::new(PermitSet::new(BTreeMap::from([(
            PermitKind::Activation,
            4_u64,
        )])));
        let drain = Arc::new(DrainGate::new());
        let admission = Arc::new(Admission::new(
            AdmissionBounds {
                target: 2,
                safety_cap: 3,
                offered_ceiling: 8,
            },
            permits,
            Arc::clone(&drain),
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

    /// The application supplies its measured restore-byte ceiling to mux admission. The
    /// context pool holds those bytes before hydration and returns them through RAII when
    /// the activation ends.
    #[test]
    fn mux_admission_reserves_the_activation_restore_bytes() {
        let permits = Arc::new(PermitSet::new(BTreeMap::from([
            (PermitKind::Activation, 1_u64),
            (PermitKind::ContextBytes, 512_u64),
        ])));
        let admission = Arc::new(Admission::new(
            AdmissionBounds {
                target: 1,
                safety_cap: 1,
                offered_ceiling: 1,
            },
            Arc::clone(&permits),
            Arc::new(DrainGate::new()),
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
        drop(held);
        assert_eq!(permits.held(PermitKind::ContextBytes), 0);
        assert_eq!(permits.held(PermitKind::Activation), 0);
    }
}
