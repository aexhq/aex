//! The asynchronous half: one delivery, driven from claim to ack.
//!
//! Nothing here names a runtime. Every wait is a port call, so the composition root decides
//! how the loop is scheduled and this code runs unchanged under a harness with no executor
//! beyond a single-threaded block-on.
//!
//! # Where each crash boundary is
//!
//! | Boundary | What survives it |
//! | --- | --- |
//! | after the pre-send write, before the socket | the effect is `dispatch_started`; the next owner classifies it with `recover` and settles `OutcomeUnknown`. It is never dispatched twice. |
//! | after the socket, before the settlement | identical, and deliberately so: the durable record cannot distinguish the two, which is exactly why the pre-send write exists. |
//! | after a decision commit, before source retirement | the wake stays authoritative; a new owner folds the committed record and retires the source without repeating the effect. |
//! | after source retirement, before lease release or queue ack | a redelivery strongly observes the retired source, acks its hint, and never claims the agent. |

use super::decide::{
    self, Draft, FailureSettlement, classify_provider_failure, classify_tool_failure, phase_tag,
    provider_call_reservation,
};
use super::{
    ActivationError, ActivationPolicy, AdmissionControl, AdmissionDecision, DispatchControl,
    DispatchDecision, DispatchLane, Outcome, Ports, Release, Stop,
};
use crate::kernel::{ActivationRegistry, DrainGate, FoldCache, RenewalOutcome, RenewalState};
use crate::ports::{
    BoxFuture, CancelToken, Claim, ClaimError, CommitError, ConditionFailure, ControlStateView,
    DecisionContext, DetachedStatus, DueRowIsolation, DueScanCursor, FenceGuard,
    MAX_DUE_ROW_ISOLATIONS, ModelUsageObservation, PreparedToolCall, PreviewScope,
    ReleaseDisposition, ScopedPreviewSink, SessionAuthority, StoreError, ToolAdvertisement,
    ToolDispatchError, ToolOutcome, ToolResultBody, ToolRoute, ToolRoutingError, WakeDelivery,
    WakeOrigin, WakeState,
};
use aex_brain_domain::budget::{Dimension, DimensionVector};
use aex_brain_domain::canonical::canonicalize_value;
use aex_brain_domain::child::{CancelCause, ChildOutcome, QueuedReason};
use aex_brain_domain::commit::{ChildBootstrap, ChildWrite, JoinWrite};
use aex_brain_domain::context;
use aex_brain_domain::effect::{
    DetachedOperationRef, DispatchEvidence, DispatchProof, DispatchStage, DurableEffect,
    EffectClass, EffectKind, EffectState, RecoveryDecision, recover,
};
use aex_brain_domain::fold::{FoldState, PendingCall, Phase, apply};
use aex_brain_domain::ids::{
    AgentId, AgentKey, ContentHash, EffectId, JoinId, JournalSeq, Timestamp, ToolCallId, WaitId,
    WakeId, child_agent_id,
};
use aex_brain_domain::journal::ExecutorRoute;
use aex_brain_domain::journal::{
    FinishReason, JournalEntry, JournalRecord, ParkReason, TypedFailure, WaitResolution,
};
use aex_brain_domain::planner::{OwedStep, PlanPolicy, model_effect_id, plan};
use aex_brain_domain::wire_pending::{
    CanonicalBlock, CanonicalMessage, CanonicalModelRequest, ContentBlockRef,
    DurableOperationSupport, JoinMode, ResolvedAgentConfig, Role,
};
use aex_model_catalog::canonical::{
    CanonicalToolDef, CorrelationId, ReasoningRequest, ToolChoice, ToolResultPart,
};
use aex_model_catalog::document::Capability;
use aex_model_catalog::{BoundedString, QualifiedModel};
use aex_wire::ids::{AgentId as PublicAgentId, PrefixedId as _, Uuid7};
use futures::stream::{self, StreamExt as _};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// Provider request fields derived only from the qualified model and immutable tool surface.
pub(super) struct ModelToolFields {
    pub(super) tools: Vec<CanonicalToolDef>,
    pub(super) choice: ToolChoice,
    pub(super) parallel: bool,
}

/// Qualifies one immutable advertisement against the selected model without truncation.
pub(super) fn model_tool_fields(
    model: &QualifiedModel,
    advertised: ToolAdvertisement,
) -> Result<ModelToolFields, ActivationError> {
    if !model.capabilities().has(Capability::Tools) {
        return Ok(ModelToolFields {
            tools: Vec::new(),
            choice: ToolChoice::None,
            parallel: false,
        });
    }
    let advertised_count = advertised.definitions.len();
    let max_tools = model.dialect().max_tools();
    if advertised_count > usize::from(max_tools) {
        return Err(ActivationError::ToolLimitExceeded {
            advertised: advertised_count,
            max: max_tools,
        });
    }
    let choice = if advertised.definitions.is_empty() {
        ToolChoice::None
    } else {
        ToolChoice::Auto
    };
    let parallel = advertised.allows_parallel_emission(model.capabilities());
    Ok(ModelToolFields {
        tools: advertised.definitions,
        choice,
        parallel,
    })
}

/// One activation: everything from claiming an agent to acking its wake.
#[derive(Debug, Clone)]
pub struct Activation {
    ports: Ports,
    policy: ActivationPolicy,
    registry: Arc<ActivationRegistry>,
    drain: Arc<DrainGate>,
    fold_cache: Option<Arc<dyn FoldCache>>,
    dispatch: Option<Arc<dyn DispatchControl>>,
}

impl Activation {
    /// Binds an activation to its ports.
    #[must_use]
    pub const fn new(
        ports: Ports,
        policy: ActivationPolicy,
        registry: Arc<ActivationRegistry>,
        drain: Arc<DrainGate>,
    ) -> Self {
        Self {
            ports,
            policy,
            registry,
            drain,
            fold_cache: None,
            dispatch: None,
        }
    }

    /// Installs a process-local exact-revision fold cache.
    #[must_use]
    pub fn with_fold_cache(mut self, fold_cache: Arc<dyn FoldCache>) -> Self {
        self.fold_cache = Some(fold_cache);
        self
    }

    /// Installs non-blocking phase-specific dispatch admission.
    #[must_use]
    pub fn with_dispatch_control(mut self, dispatch: Arc<dyn DispatchControl>) -> Self {
        self.dispatch = Some(dispatch);
        self
    }

    /// The ports this activation drives.
    #[must_use]
    pub const fn ports(&self) -> &Ports {
        &self.ports
    }

    /// The bounds it runs under.
    #[must_use]
    pub const fn policy(&self) -> &ActivationPolicy {
        &self.policy
    }

    /// Drives one delivery to its conclusion.
    ///
    /// # Errors
    ///
    /// Every [`ActivationError`] leaves the durable wake in place: the delivery is released
    /// rather than acked, and the lease is given up so a surviving task claims at once. A
    /// refusal therefore costs a redelivery, never a lost unit of work.
    #[allow(
        clippy::too_many_lines,
        reason = "claim, supervision, source retirement, cache publication, lease release and acknowledgement are one crash-ordered sequence"
    )]
    pub async fn run(&self, delivery: WakeDelivery) -> Result<Outcome, ActivationError> {
        let Some(_admitted) = self.drain.try_admit() else {
            // Drain has started. Releasing rather than abandoning is the difference between
            // a wake another task picks up in milliseconds and one that waits out a timeout.
            self.release(&delivery, core::time::Duration::ZERO).await;
            return Ok(Outcome::Released(Release::Draining));
        };
        let key = delivery.wake.key;
        let Ok(_slot) = self.registry.try_activate(key) else {
            self.release(&delivery, self.policy.requeue_after).await;
            return Ok(Outcome::Released(Release::LocallyBusy));
        };

        let owner = self.ports.ids.owner_token();
        if !self.source_is_pending(&delivery).await? {
            return Ok(Outcome::Idle);
        }
        let claim = match self
            .ports
            .leases
            .claim(&key, owner, self.policy.lease_ttl, self.ports.clock.now())
            .await
        {
            Ok(claim) => claim,
            Err(ClaimError::Terminal) => {
                // A terminal session does not mint an agent fence. Without one there is no
                // authority to retire this source row, so leave it durable for the owning
                // lifecycle transaction rather than performing unfenced cleanup.
                self.release(&delivery, self.policy.requeue_after).await;
                return Ok(Outcome::Released(Release::HeldByOther));
            }
            Err(ClaimError::HeldByOther { .. } | ClaimError::Fenced { .. }) => {
                self.release(&delivery, self.policy.requeue_after).await;
                return Ok(Outcome::Released(Release::HeldByOther));
            }
            Err(ClaimError::Store(error)) => {
                self.release(&delivery, self.policy.requeue_after).await;
                return Err(error.into());
            }
        };

        if delivery.wake.tenant != claim.authority.workspace.to_string() {
            let _ = self
                .ports
                .leases
                .release(claim, ReleaseDisposition::Abandoned)
                .await;
            self.release(&delivery, self.policy.requeue_after).await;
            return Err(StoreError::WakeTenantMismatch.into());
        }

        let guard = FenceGuard::new(
            key,
            claim.owner,
            claim.fence,
            claim.head.revision,
            claim.head.journal_tail,
            claim.head.cancel_epoch,
            crate::ports::CancelToken::new(),
        );
        let claim_state = Arc::new(Mutex::new(claim.clone()));
        let mut session = Session {
            ports: &self.ports,
            policy: &self.policy,
            drain: &self.drain,
            key,
            authority: claim.authority.clone(),
            claim: Arc::clone(&claim_state),
            stop_requested: claim.head.stop_requested,
            stop_reason: claim.head.stop_reason.unwrap_or(FinishReason::Cancelled),
            guard,
            state: FoldState::empty(),
            steps: 0,
            attempts: 1,
            resume: VecDeque::new(),
            checkpoint: claim.head.checkpoint.clone(),
            fold_cache: self.fold_cache.as_deref(),
            dispatch: self.dispatch.as_deref(),
            retained_bytes: 0,
        };

        let cancel = session.guard.cancel().clone();
        let work = async {
            let outcome = if claim.head.finish.is_some() {
                // Terminal is a control projection, not permission to skip journal
                // integrity. Source retirement is still a durable action, so even this
                // path proves the complete folded tail before it writes or acks.
                session.reload().await.map(|()| Outcome::Idle)
            } else {
                session.drive().await
            };
            let outcome = match outcome {
                Ok(outcome) => self
                    .retire_source(&mut session, &delivery.wake)
                    .await
                    .map(|()| outcome),
                Err(error) => Err(error),
            };
            if outcome.is_ok() {
                session.publish_fold_cache();
            }
            outcome
        };
        let outcome = self.supervise(&claim_state, &delivery, cancel, work).await;
        let disposition = match &outcome {
            Ok(Outcome::Progressed {
                stop: Stop::Parked, ..
            }) => ReleaseDisposition::Parked,
            Ok(_) => ReleaseDisposition::Committed,
            Err(_) => ReleaseDisposition::Abandoned,
        };
        // The lease goes back before the ack. A crash between them leaves a wake that
        // redelivers against an unowned agent, which is the cheap direction to fail in.
        let latest_claim = claim_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let _ = self.ports.leases.release(latest_claim, disposition).await;

        self.finish_delivery(delivery, outcome).await
    }

    async fn finish_delivery(
        &self,
        delivery: WakeDelivery,
        outcome: Result<Outcome, ActivationError>,
    ) -> Result<Outcome, ActivationError> {
        match outcome {
            Ok(outcome) => {
                if matches!(outcome, Outcome::Released(_)) {
                    self.release(&delivery, self.policy.requeue_after).await;
                    return Ok(outcome);
                }
                // The ack is the last thing that happens, so it is also the likeliest to be
                // interrupted. Consuming the delivery on a failed ack would strand an agent
                // that has already committed, so the failure puts it back and lets the
                // redelivery observe the state the commit wrote.
                if let Err(error) = self.ports.wakes.ack(delivery.clone()).await {
                    self.release(&delivery, core::time::Duration::ZERO).await;
                    return Err(error.into());
                }
                Ok(outcome)
            }
            Err(error) => {
                if delivery
                    .receive_count()
                    .is_some_and(|receives| receives >= self.policy.max_receives)
                {
                    // The same delivery has failed this many times. Redelivering forever
                    // hides the fault behind a queue depth nobody reads; acking it records
                    // the poison and lets the durable due scan re-arm the work.
                    let receives = delivery.receive_count().unwrap_or_default();
                    self.ports.wakes.ack(delivery).await?;
                    return Ok(Outcome::Poisoned { receives });
                }
                self.release(&delivery, self.policy.requeue_after).await;
                Err(error)
            }
        }
    }

    async fn release(&self, delivery: &WakeDelivery, after: core::time::Duration) {
        // A failed release costs one visibility timeout, never a lost wake, so it is
        // deliberately not escalated over the reason the caller is already reporting.
        let _ = self.ports.wakes.release(delivery.clone(), after).await;
    }

    /// Runs claimed work beside one timer-driven ownership supervisor.
    ///
    /// The work future and timer are polled by one structured parent, so neither is detached
    /// and dropping this scope drops both. A pending provider/tool/Hands future sleeps until
    /// either it is woken or the renewal timer fires; there is no per-effect polling loop.
    async fn supervise<F, T>(
        &self,
        claim: &Arc<Mutex<Claim>>,
        delivery: &WakeDelivery,
        cancel: CancelToken,
        work: F,
    ) -> Result<T, ActivationError>
    where
        F: core::future::Future<Output = Result<T, ActivationError>>,
    {
        enum Event<T> {
            Work(Result<T, ActivationError>),
            Renew,
            Visibility,
        }

        let renewal = RenewalState::new(
            claim
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .fence,
            cancel.clone(),
        );
        let mut work = Box::pin(work);
        let mut timer = self.ports.clock.sleep(self.policy.renew_interval);
        let mut visibility: Option<BoxFuture<'_, Result<(), StoreError>>> = None;
        loop {
            let event = core::future::poll_fn(|context| {
                if let core::task::Poll::Ready(output) = work.as_mut().poll(context) {
                    return core::task::Poll::Ready(Event::Work(output));
                }
                if visibility
                    .as_mut()
                    .is_some_and(|pending| pending.as_mut().poll(context).is_ready())
                {
                    return core::task::Poll::Ready(Event::Visibility);
                }
                timer.as_mut().poll(context).map(|()| Event::Renew)
            })
            .await;
            match event {
                Event::Work(output) => return output,
                Event::Visibility => {
                    visibility = None;
                    continue;
                }
                Event::Renew => {
                    // Arm the next deadline before any network call. A slow visibility
                    // extension remains one polled child below; it cannot postpone the next
                    // lease renewal or stop the claimed work being polled.
                    timer = self.ports.clock.sleep(self.policy.renew_interval);
                    if self.drain.is_draining() {
                        // The adapter receives an honest cancellation request before this
                        // scope can be aborted. The token cannot prove whether a byte left,
                        // so the effect's returned dispatch proof still decides settlement.
                        cancel.cancel();
                    }
                }
            }

            let current = claim
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            match self
                .ports
                .leases
                .renew(&current, self.policy.lease_ttl, self.ports.clock.now())
                .await
            {
                Ok(renewed) => {
                    if renewal.renewed(renewed.fence) == RenewalOutcome::Lost {
                        return Err(ClaimError::Fenced {
                            current: renewed.fence,
                        }
                        .into());
                    }
                    *claim
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = renewed;
                    // Visibility is extended only after the durable lease proves this owner
                    // is still live. A queue failure permits a duplicate hint; the fence then
                    // rejects its claim. Hiding a delivery after a failed renewal would be the
                    // unsafe direction.
                    if visibility.is_none() {
                        visibility = Some(
                            self.ports
                                .wakes
                                .extend_visibility(delivery, self.policy.visibility_timeout),
                        );
                    }
                }
                Err(ClaimError::Fenced { current }) => {
                    let _ = renewal.observe(current);
                    return Err(ClaimError::Fenced { current }.into());
                }
                Err(error @ (ClaimError::HeldByOther { .. } | ClaimError::Terminal)) => {
                    cancel.cancel();
                    return Err(error.into());
                }
                Err(ClaimError::Store(error)) => {
                    if renewal.renewal_failed() == RenewalOutcome::Lost {
                        return Err(ClaimError::Store(error).into());
                    }
                }
            }
        }
    }

    async fn source_is_pending(&self, delivery: &WakeDelivery) -> Result<bool, ActivationError> {
        match self.ports.wakes.state(&delivery.wake).await {
            Ok(WakeState::Pending) => Ok(true),
            Ok(WakeState::Retired) => {
                self.ports.wakes.ack(delivery.clone()).await?;
                Ok(false)
            }
            Err(error) => {
                self.release(delivery, self.policy.requeue_after).await;
                Err(error.into())
            }
        }
    }

    async fn retire_source(
        &self,
        session: &mut Session<'_>,
        wake: &crate::ports::DurableWake,
    ) -> Result<(), ActivationError> {
        match session.retire(wake.work_id.clone()).await {
            Ok(()) => Ok(()),
            Err(ActivationError::Commit(CommitError::Condition(
                ConditionFailure::WakeStateMoved,
            ))) => match self.ports.wakes.state(wake).await? {
                WakeState::Retired => Ok(()),
                WakeState::Pending => {
                    Err(CommitError::Condition(ConditionFailure::WakeStateMoved).into())
                }
            },
            Err(error) => Err(error),
        }
    }
}

/// The receive side: one batch, deduplicated, admitted and driven.
///
/// Separated from [`Activation`] because the two answer different questions. An activation
/// asks "what does this agent owe?"; the loop asks "may this task take more work at all?",
/// which is an admission and drain question the composition root parameterizes.
#[derive(Clone)]
pub struct WakeLoop {
    activation: Activation,
    admission: Arc<dyn AdmissionControl>,
    inflight: Arc<Mutex<BTreeSet<String>>>,
    due_shard: Arc<AtomicU16>,
    due_cursors: Arc<Mutex<BTreeMap<u16, DueScanCursor>>>,
    due_scan_due_at: Arc<AtomicU64>,
}

/// What one pass over the queue did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PollReport {
    /// How many deliveries the receive returned.
    pub received: usize,
    /// How many were driven to a decision.
    pub driven: usize,
    /// How many went straight back to the queue.
    pub released: usize,
    /// How many failed with a typed refusal.
    pub refused: usize,
    /// How many of the deliveries came from the durable due backstop.
    pub recovered: usize,
    /// How many malformed due rows were isolated while valid siblings continued.
    pub malformed: usize,
    /// How many malformed queue records were isolated while valid siblings continued.
    pub malformed_queue: usize,
    /// How many malformed queue records reached the configured poison threshold.
    pub poisoned: usize,
    /// A process-wide bounded, redacted sample of malformed due rows from this pass.
    pub isolations: Vec<DueRowIsolation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DriveClass {
    Stable,
    Transient,
}

struct ScheduledDelivery {
    delivery: WakeDelivery,
    due_pages: Vec<usize>,
}

struct DuePageProgress {
    shard: u16,
    next: Option<DueScanCursor>,
    advance: bool,
}

impl WakeLoop {
    /// Binds a loop over one activation and one admission controller.
    #[must_use]
    pub fn new(activation: Activation, admission: Arc<dyn AdmissionControl>) -> Self {
        Self {
            activation,
            admission,
            inflight: Arc::new(Mutex::new(BTreeSet::new())),
            due_shard: Arc::new(AtomicU16::new(0)),
            due_cursors: Arc::new(Mutex::new(BTreeMap::new())),
            due_scan_due_at: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The activation this loop drives.
    #[must_use]
    pub const fn activation(&self) -> &Activation {
        &self.activation
    }

    /// Whether the composition currently permits another receive scope.
    ///
    /// A process-level scheduler uses this before adding a long-poll lane. [`poll_once`](Self::poll_once)
    /// repeats the check, so a drain or admission change between this observation and the
    /// first poll still fails closed without touching the queue.
    #[must_use]
    pub fn receiving_allowed(&self) -> bool {
        self.admission.should_receive()
    }

    /// Receives one batch and drives it.
    ///
    /// Returns an empty report without touching the queue when the task is draining or is
    /// above its admission target: a task that keeps receiving while it cannot start
    /// anything simply hides work in its own memory where no other task can take it.
    ///
    /// # Errors
    ///
    /// Propagates the receive's typed refusal. A failure to receive costs one poll; every
    /// durable wake survives it.
    pub async fn poll_once(&self) -> Result<PollReport, ActivationError> {
        let mut report = PollReport::default();
        if !self.admission.should_receive() {
            return Ok(report);
        }
        let policy = self.activation.policy();
        let batch = self
            .activation
            .ports()
            .wakes
            .receive(policy.receive_batch, policy.long_poll)
            .await?;
        report.received = batch.deliveries.len().saturating_add(batch.malformed.len());

        // Receive remains first so a transport failure cannot move a due cursor. Recovery
        // is the first post-receive I/O: neither poison-record handling nor ten 600-second
        // provider calls may postpone the lost-hint backstop.
        let mut scheduled: Vec<ScheduledDelivery> = batch
            .deliveries
            .into_iter()
            .map(|delivery| ScheduledDelivery {
                delivery,
                due_pages: Vec::new(),
            })
            .collect();
        let mut due_pages = Vec::new();
        if self.claim_due_pass() {
            self.scan_due(&mut report, &mut scheduled, &mut due_pages)
                .await;
        }
        self.handle_malformed(&mut report, batch.malformed).await;
        self.drive_scheduled(&mut report, scheduled, &mut due_pages)
            .await;
        self.commit_due_cursors(due_pages);
        Ok(report)
    }

    async fn handle_malformed(
        &self,
        report: &mut PollReport,
        malformed: Vec<crate::ports::MalformedWakeDelivery>,
    ) {
        for delivery in malformed {
            report.malformed_queue = report.malformed_queue.saturating_add(1);
            if delivery.receive_count >= self.activation.policy().max_receives {
                match self.activation.ports().wakes.ack_malformed(delivery).await {
                    Ok(()) => report.poisoned = report.poisoned.saturating_add(1),
                    Err(_) => report.refused = report.refused.saturating_add(1),
                }
            } else {
                match self
                    .activation
                    .ports()
                    .wakes
                    .release_malformed(delivery, self.activation.policy().requeue_after)
                    .await
                {
                    Ok(()) => report.released = report.released.saturating_add(1),
                    Err(_) => report.refused = report.refused.saturating_add(1),
                }
            }
        }
    }

    fn claim_due_pass(&self) -> bool {
        let now = self.activation.ports().clock.steady().0;
        let interval = u64::try_from(self.activation.policy().due_scan_interval.as_millis())
            .unwrap_or(u64::MAX);
        loop {
            let due_at = self.due_scan_due_at.load(Ordering::Acquire);
            if now < due_at {
                return false;
            }
            let next = now.saturating_add(interval);
            if self
                .due_scan_due_at
                .compare_exchange(due_at, next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return true;
            }
        }
    }

    async fn scan_due(
        &self,
        report: &mut PollReport,
        scheduled: &mut Vec<ScheduledDelivery>,
        progress: &mut Vec<DuePageProgress>,
    ) {
        let policy = self.activation.policy();
        let shard_count = policy.due_shards.max(1);
        let shards = policy.due_scan_shards_per_pass.max(1).min(shard_count);
        for _ in 0..shards {
            let shard = self.due_shard.fetch_add(1, Ordering::Relaxed) % shard_count;
            let after = self
                .due_cursors
                .lock()
                .expect("not poisoned")
                .get(&shard)
                .cloned();
            let page = self
                .activation
                .ports()
                .wakes
                .due_scan(
                    aex_brain_domain::ids::WorkShard(shard),
                    self.activation.ports().clock.now(),
                    policy.due_scan_page.max(1),
                    after,
                )
                .await;
            let Ok(page) = page else {
                // A failed shard is retained at its prior cursor. Rotation continues so
                // one throttled partition cannot suppress recovery in later shards.
                report.refused = report.refused.saturating_add(1);
                continue;
            };
            report.malformed = report.malformed.saturating_add(page.malformed);
            let sample_room = MAX_DUE_ROW_ISOLATIONS.saturating_sub(report.isolations.len());
            report
                .isolations
                .extend(page.isolations.into_iter().take(sample_room));
            report.recovered = report.recovered.saturating_add(page.wakes.len());
            report.received = report.received.saturating_add(page.wakes.len());
            let page_index = progress.len();
            progress.push(DuePageProgress {
                shard,
                next: page.next,
                advance: true,
            });
            for wake in page.wakes {
                if let Some(existing) = scheduled
                    .iter_mut()
                    .find(|delivery| delivery.delivery.wake.dedup_key == wake.dedup_key)
                {
                    // The queue and due index are two hints for one durable row. Drive it
                    // once, but make cursor progress depend on that shared drive's outcome.
                    existing.due_pages.push(page_index);
                } else {
                    scheduled.push(ScheduledDelivery {
                        delivery: WakeDelivery {
                            wake,
                            origin: WakeOrigin::DueScan,
                        },
                        due_pages: vec![page_index],
                    });
                }
            }
        }
    }

    async fn drive_scheduled(
        &self,
        report: &mut PollReport,
        scheduled: Vec<ScheduledDelivery>,
        due_pages: &mut [DuePageProgress],
    ) {
        let concurrency = self.activation.policy().max_concurrent_drives.max(1);
        let mut outcomes = stream::iter(scheduled)
            .map(|scheduled| async move {
                let outcome = self.drive(scheduled.delivery).await;
                (scheduled.due_pages, outcome)
            })
            .buffer_unordered(concurrency);
        while let Some((due_page_indexes, outcome)) = outcomes.next().await {
            let class = match outcome {
                Ok(Outcome::Released(_)) => {
                    report.released = report.released.saturating_add(1);
                    DriveClass::Transient
                }
                Ok(Outcome::Poisoned { .. }) => {
                    // Queue poison handling consumes only the SQS projection. If the same
                    // durable row was also in this due page, it remains pending authority
                    // and the page must revisit it rather than advancing past it.
                    report.driven = report.driven.saturating_add(1);
                    DriveClass::Transient
                }
                Ok(_) => {
                    report.driven = report.driven.saturating_add(1);
                    DriveClass::Stable
                }
                Err(_) => {
                    report.refused = report.refused.saturating_add(1);
                    DriveClass::Transient
                }
            };
            if class == DriveClass::Transient {
                for index in due_page_indexes {
                    if let Some(page) = due_pages.get_mut(index) {
                        page.advance = false;
                    }
                }
            }
        }
    }

    fn commit_due_cursors(&self, pages: Vec<DuePageProgress>) {
        let mut cursors = self.due_cursors.lock().expect("not poisoned");
        for page in pages {
            if !page.advance {
                continue;
            }
            // A cursor is installed only after every valid wake in the page reached a
            // durable stable outcome. Transient releases and typed refusals revisit the old
            // position; shard rotation remains independent, so later shards still run.
            match page.next {
                Some(next) => {
                    cursors.insert(page.shard, next);
                }
                None => {
                    cursors.remove(&page.shard);
                }
            }
        }
    }

    /// Drives one delivery through admission and dedup.
    ///
    /// # Errors
    ///
    /// Propagates the activation's typed refusal. The delivery has already been released by
    /// then, so the caller reports rather than compensates.
    pub async fn drive(&self, delivery: WakeDelivery) -> Result<Outcome, ActivationError> {
        let dedup = delivery.wake.dedup_key.clone();
        let Some(_entry) = InflightKey::take(&self.inflight, dedup) else {
            // A duplicate of something already running. Releasing at zero visibility lets it
            // be re-offered the moment the first one finishes, which is when it will
            // correctly observe that there is nothing left to do.
            let _ = self
                .activation
                .ports()
                .wakes
                .release(delivery, core::time::Duration::ZERO)
                .await;
            return Ok(Outcome::Released(Release::LocallyBusy));
        };
        let restore_bytes = self.activation.policy().restore_resident_bytes;
        let _permits = match self.admission.admit(restore_bytes) {
            AdmissionDecision::Admitted(permits) => permits,
            AdmissionDecision::Deferred { requeue_after } => {
                let _ = self
                    .activation
                    .ports()
                    .wakes
                    .release(delivery, requeue_after)
                    .await;
                return Ok(Outcome::Released(Release::Deferred));
            }
            AdmissionDecision::Shed { retry_after } => {
                let _ = self
                    .activation
                    .ports()
                    .wakes
                    .release(delivery, retry_after)
                    .await;
                return Ok(Outcome::Released(Release::Draining));
            }
        };
        self.activation.run(delivery).await
    }
}

impl core::fmt::Debug for WakeLoop {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("WakeLoop")
            .field("activation", &self.activation)
            .finish_non_exhaustive()
    }
}

/// A dedup-set membership held for the life of one drive, released on drop.
struct InflightKey<'set> {
    set: &'set Mutex<BTreeSet<String>>,
    key: String,
}

impl<'set> InflightKey<'set> {
    fn take(set: &'set Mutex<BTreeSet<String>>, key: String) -> Option<Self> {
        let mut guard = set.lock().expect("the dedup lock is not poisoned");
        if !guard.insert(key.clone()) {
            return None;
        }
        drop(guard);
        Some(Self { set, key })
    }
}

impl Drop for InflightKey<'_> {
    fn drop(&mut self) {
        let mut guard = self.set.lock().expect("the dedup lock is not poisoned");
        guard.remove(&self.key);
    }
}

/// One claimed agent, for the life of one activation.
struct Session<'a> {
    ports: &'a Ports,
    policy: &'a ActivationPolicy,
    drain: &'a DrainGate,
    key: AgentKey,
    authority: SessionAuthority,
    claim: Arc<Mutex<Claim>>,
    stop_requested: bool,
    stop_reason: FinishReason,
    guard: FenceGuard,
    state: FoldState,
    steps: u32,
    attempts: u16,
    resume: VecDeque<Box<DurableEffect>>,
    checkpoint: Option<aex_brain_domain::checkpoint::CheckpointMetadata>,
    fold_cache: Option<&'a dyn FoldCache>,
    dispatch: Option<&'a dyn DispatchControl>,
    retained_bytes: usize,
}

impl Session<'_> {
    const fn agent(&self) -> AgentId {
        self.key.agent
    }

    async fn drive(&mut self) -> Result<Outcome, ActivationError> {
        // Restore and open-effect recovery are independent authoritative reads after the
        // claim. Polling them together removes one store round trip from the cold path while
        // preserving the order that matters: neither result reaches planning until both
        // completed and the fold proved the claimed tail.
        let restore = self.load_fold();
        let open = async {
            self.ports
                .effects
                .load_open(&self.key)
                .await
                .map_err(ActivationError::from)
        };
        let (restored, open) = futures::try_join!(restore, open)?;
        self.install_fold(restored);
        self.publish_pending_model_usage().await?;
        if let Some(stop) = self.recover_open(open).await? {
            return Ok(Outcome::Progressed {
                steps: self.steps,
                stop,
            });
        }
        loop {
            match self.step().await? {
                Step::Nothing => {
                    return Ok(if self.steps == 0 {
                        Outcome::Idle
                    } else {
                        Outcome::Progressed {
                            steps: self.steps,
                            stop: Stop::HandedBack,
                        }
                    });
                }
                Step::Continue => {
                    if self.steps >= self.policy.max_steps_per_activation {
                        self.hand_back().await?;
                        return Ok(Outcome::Progressed {
                            steps: self.steps,
                            stop: Stop::HandedBack,
                        });
                    }
                }
                Step::Stop(stop) => {
                    return Ok(Outcome::Progressed {
                        steps: self.steps,
                        stop,
                    });
                }
            }
        }
    }

    /// Rebuilds the fold from the bounded authoritative journal.
    ///
    /// A page that observes a gap returns no entries at all, so the agent never acts on a
    /// prefix of its own history: `read_page` refuses and this propagates the refusal. The
    /// complete restore must also equal the `(sequence, hash)` tail returned with the claim;
    /// neither an early EOF nor a same-sequence fork may reach recovery or planning.
    async fn reload(&mut self) -> Result<(), ActivationError> {
        let restored = self.load_fold().await?;
        self.install_fold(restored);
        Ok(())
    }

    async fn load_fold(&self) -> Result<super::restore::RestoredFold, ActivationError> {
        let (claimed_seq, claimed_hash) = {
            let claim = self
                .claim
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (claim.head.journal_tail, claim.head.journal_tail_hash)
        };
        let revision = self
            .claim
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .head
            .revision;
        if self.checkpoint.is_none()
            && let Some(cache) = self.fold_cache
        {
            let tick = self.ports.clock.steady().0;
            if let Some(entry) = cache.load(&self.key, revision, tick) {
                let folded_hash = entry.state.hashes.last().copied();
                if entry.state.tail == claimed_seq && folded_hash == claimed_hash {
                    return Ok(super::restore::RestoredFold {
                        state: entry.state,
                        source: super::restore::RestoreSource::WarmCache,
                        retained_bytes: entry.bytes,
                    });
                }
                // Exact revision plus a different tail can only be damaged process-local
                // state. Remove just what was observed and rebuild from authority.
                cache.remove(&self.key, revision);
            }
        }
        let restored = if let Some(metadata) = &self.checkpoint {
            let checkpoint = self.ports.checkpoints.load(self.key, metadata).await?;
            super::restore::restore_from_checkpoint(
                self.ports.journal.as_ref(),
                checkpoint,
                claimed_seq,
                claimed_hash,
                self.policy.read,
                self.policy.restore,
            )
            .await?
        } else {
            super::restore::restore(
                self.ports.journal.as_ref(),
                self.key,
                claimed_seq,
                claimed_hash,
                self.policy.read,
                self.policy.restore,
            )
            .await?
        };
        Ok(restored)
    }

    fn install_fold(&mut self, restored: super::restore::RestoredFold) {
        self.state = restored.state;
        self.retained_bytes = restored.retained_bytes;
    }

    fn publish_fold_cache(&self) {
        let Some(cache) = self.fold_cache else {
            return;
        };
        cache.store(
            self.key,
            self.guard.revision(),
            &self.state,
            self.retained_bytes,
            self.ports.clock.steady().0,
        );
    }

    /// Classifies whatever the previous owner left open.
    ///
    /// Runs **before** the planner, because the planner has no dispatch evidence. A
    /// dispatched non-replayable effect settles `OutcomeUnknown` here; nothing downstream
    /// ever gets the chance to treat it as retryable.
    async fn recover_open(
        &mut self,
        mut open: Vec<DurableEffect>,
    ) -> Result<Option<Stop>, ActivationError> {
        open.retain(|effect| !effect.state.is_settled());
        open.sort_by_key(|effect| {
            let order = self
                .pending_for_effect(effect.id)
                .map_or(u32::MAX, |call| call.order);
            (order, effect.id.0)
        });
        if open.is_empty() {
            return Ok(None);
        }
        if self.stop_requested && self.state.active_run.is_some() {
            // A session cancellation has already advanced both the session and root-agent
            // epochs. The predecessor was allowed to observe that fence and drop its
            // provider future before this successor claimed the root. Recovery must still
            // settle every open batch member honestly, but the customer's requested
            // terminal result is cancellation rather than an infrastructure interruption.
            let mut draft = self.draft("effecting");
            for effect in open {
                let pending = self.pending_for_effect(effect.id);
                match &effect.state {
                    EffectState::Prepared { .. } => {
                        // The dispatch marker never committed, so the upstream provably saw
                        // nothing. Closing it prevents dispatch after cancellation.
                        decide::settle_known_failure(
                            &mut draft,
                            effect.id,
                            DispatchStage::PreDispatch,
                            DispatchProof::NotSent,
                        );
                    }
                    EffectState::DispatchStarted { attempt } => {
                        let evidence = effect.evidence.clone().unwrap_or_else(|| {
                            DispatchEvidence::ambiguous(*attempt, DispatchStage::Dispatched)
                        });
                        if pending.is_some() {
                            decide::settle_tool_unknown(&mut draft, effect.id, evidence);
                        } else {
                            decide::settle_root_unknown(&mut draft, effect.id, evidence);
                        }
                    }
                    EffectState::ResponseStarted {
                        attempt,
                        provider_request_id,
                    } => {
                        let evidence =
                            effect.evidence.clone().unwrap_or_else(|| DispatchEvidence {
                                stage: DispatchStage::Streaming,
                                proof: DispatchProof::ResponseStarted,
                                attempt: *attempt,
                                provider_request_id: provider_request_id.clone(),
                                external_operation: None,
                                detached_tool: None,
                                receipt: None,
                                detail: None,
                            });
                        if pending.is_some() {
                            decide::settle_tool_unknown(&mut draft, effect.id, evidence);
                        } else {
                            decide::settle_root_unknown(&mut draft, effect.id, evidence);
                        }
                    }
                    EffectState::Complete { .. }
                    | EffectState::KnownFailure { .. }
                    | EffectState::OutcomeUnknown { .. } => {
                        unreachable!("load_open excludes settled effects")
                    }
                }
                if let Some(call) = pending {
                    let executed_on = self.tool_executor(&call)?;
                    self.append_tool_error(
                        &mut draft,
                        effect.id,
                        &call,
                        executed_on,
                        "tool execution was cancelled",
                    );
                }
            }
            self.finish_current(&mut draft, self.stop_reason, None, None)?;
            self.commit(draft).await?;
            return Ok(Some(Stop::Finished(self.stop_reason)));
        }
        let mut recovered = self.draft("effecting");
        for effect in open {
            if matches!(effect.state, EffectState::Prepared { .. }) {
                // Intent committed, nothing sent: queue every unambiguous member. Their
                // durable call links retain exact model order and prevent a second identity.
                self.resume.push_back(Box::new(effect));
                continue;
            }
            match recover(&effect, Self::support()) {
                RecoveryDecision::Interrupt { evidence } => {
                    if let Some(call) = self.pending_for_effect(effect.id) {
                        let executed_on = self.tool_executor(&call)?;
                        decide::settle_tool_unknown(&mut recovered, effect.id, evidence);
                        self.append_tool_error(
                            &mut recovered,
                            effect.id,
                            &call,
                            executed_on,
                            "tool outcome is unknown after worker recovery",
                        );
                    } else {
                        self.settle_unknown_current(&mut recovered, effect.id, evidence)?;
                        self.commit(recovered).await?;
                        return Ok(Some(Stop::Finished(FinishReason::Interrupted)));
                    }
                }
                RecoveryDecision::QueryDurableOperation { .. } => {
                    return Err(ActivationError::Unsupported {
                        step: "query_non_tool_durable_operation",
                        owed_by: "the provider or Hands recovery adapter selected by the effect kind; a tool executor route cannot safely answer this identity",
                    });
                }
                RecoveryDecision::QueryDetachedTool { operation } => {
                    self.commit(recovered).await?;
                    return self.resolve_detached(&effect, &operation).await.map(Some);
                }
                RecoveryDecision::ReconstructFromReceipt { .. } => {
                    return Err(ActivationError::Unsupported {
                        step: "reconstruct_from_receipt",
                        owed_by: "aex-content-aws, which holds the committed response body Brain would rebuild the outcome from",
                    });
                }
                RecoveryDecision::RetrySameEffect { .. } => {
                    return Err(ActivationError::Unsupported {
                        step: "retry_dispatched_effect",
                        owed_by: "the effect driver's recovery controller (plan 07 S-5.3), which is what moves a dispatched effect back to prepared",
                    });
                }
            }
        }
        self.commit(recovered).await?;
        Ok(None)
    }

    fn pending_for_effect(&self, effect: EffectId) -> Option<PendingCall> {
        let call = self
            .state
            .tool_effects
            .iter()
            .find_map(|(call, linked)| (*linked == effect).then(|| call.clone()))?;
        self.state.pending_calls.get(&call).cloned()
    }

    fn tool_executor(&self, call: &PendingCall) -> Result<ExecutorRoute, ActivationError> {
        let config = self.config()?;
        Ok(self
            .ports
            .tools
            .route(&config.catalog_pin, &call.name)?
            .executor)
    }

    const fn support() -> DurableOperationSupport {
        // The launch answer for every compiled provider: none exposes a
        // proven generation-resume or result-lookup operation.
        DurableOperationSupport::None
    }

    /// One plan-and-act cycle.
    async fn step(&mut self) -> Result<Step, ActivationError> {
        if let Some(effect) = self.resume.pop_front() {
            return match effect.kind {
                EffectKind::ModelCall => self.model_call(Some(&effect)).await,
                EffectKind::ToolCall => self.tool_call(Some(&effect)).await,
                other => Err(ActivationError::Unsupported {
                    step: kind_name(other),
                    owed_by: "the effect driver; this build prepares only model and tool effects",
                }),
            };
        }
        // An agent with no `AgentStarted` record does not exist yet. Brain never creates
        // one: the session authority does, and until it has there is nothing to plan.
        if self.state.config.is_none() {
            return Ok(Step::Nothing);
        }
        if let Phase::Parked { reason } = &self.state.phase
            && let ParkReason::AwaitingChildren { join, deadline } = reason.as_ref()
        {
            return self.resume_subagent_wait(*join, *deadline).await;
        }
        let owed = plan(&self.state, &self.plan_policy());
        match owed {
            OwedStep::Finished { reason } => Ok(Step::Stop(Stop::Finished(reason))),
            OwedStep::Finish { reason } => {
                let reason = if reason == FinishReason::Cancelled && self.stop_requested {
                    self.stop_reason
                } else {
                    reason
                };
                let mut draft = self.draft(phase_tag(&self.state.phase));
                self.finish_current(&mut draft, reason, self.state.failure.clone(), None)?;
                self.commit(draft).await?;
                Ok(Step::Stop(Stop::Finished(reason)))
            }
            OwedStep::Park { reason } => self.park(*reason).await,
            OwedStep::ModelCall { .. } => self.model_call(None).await,
            OwedStep::ToolCalls { calls } => {
                if calls.is_empty() {
                    return Ok(Step::Nothing);
                }
                if calls
                    .iter()
                    .any(|call| !is_native_subagent_tool(call.name.as_str()))
                {
                    return self.tool_batch(calls).await;
                }
                if let Some(call) = first_pending(&self.state) {
                    return self.native_subagent_tool(call).await;
                }
                Ok(Step::Nothing)
            }
            OwedStep::SpawnChildren { .. } => Err(ActivationError::Unsupported {
                step: "spawn_children",
                owed_by: "the `create_subagent` tool, which is the only thing that produces a fanout request for `subagent::plan_spawn`",
            }),
        }
    }

    fn plan_policy(&self) -> PlanPolicy {
        let limits = self
            .state
            .config
            .as_ref()
            .map_or(DEFAULT_LIMITS, |config| config.limits);
        PlanPolicy {
            cancel_requested: self.stop_requested,
            ..PlanPolicy::from_limits(self.ports.clock.now(), limits)
        }
    }

    fn bounded_effect_deadline(&self, proposed: Timestamp) -> Timestamp {
        self.state
            .active_deadline
            .map_or(proposed, |run_deadline| run_deadline.min(proposed))
    }

    /// Opens a durable wait, unless one with the same reason is already open.
    ///
    /// The second half matters: without it every redelivered wake against a parked agent
    /// would append another `WaitOpened`, and the journal would grow without the agent ever
    /// doing anything.
    async fn park(&mut self, reason: ParkReason) -> Result<Step, ActivationError> {
        if self.state.waits.values().any(|open| *open == reason) {
            return Ok(Step::Nothing);
        }
        let mut draft = self.draft("parked");
        let wait = wait_id(self.agent(), draft.next_seq());
        let due = match &reason {
            ParkReason::AwaitingTimer { due } => Some(*due),
            _ => None,
        };
        draft.append(JournalRecord::WaitOpened { wait, reason, due });
        self.commit(draft).await?;
        Ok(Step::Stop(Stop::Parked))
    }

    /// Commits a continuation wake and hands the agent back.
    ///
    /// The wake rides inside the decision, which is the only place one may be created. A
    /// hand-back that merely stopped would leave the work durable and unscheduled.
    async fn hand_back(&mut self) -> Result<(), ActivationError> {
        self.hand_back_for(QueuedReason::RegionalCapacity).await
    }

    async fn hand_back_for(&mut self, reason: QueuedReason) -> Result<(), ActivationError> {
        let mut draft = self.draft(phase_tag(&self.state.phase));
        self.wake_continuation_for(&mut draft, reason);
        self.commit(draft).await
    }

    fn wake_continuation(&self, draft: &mut Draft) {
        self.wake_continuation_for(draft, QueuedReason::RegionalCapacity);
    }

    fn wake_continuation_for(&self, draft: &mut Draft, reason: QueuedReason) {
        draft.wake(
            self.ports.ids.wake_id(),
            ParkReason::AwaitingCapacity { reason },
            self.ports.clock.now(),
            self.authority.workspace.to_string(),
            self.policy.shard_for(self.agent()),
        );
    }

    fn dispatch_admission(&self, lane: DispatchLane, weight: u16) -> DispatchDecision {
        self.dispatch.map_or_else(
            || DispatchDecision::Admitted(None),
            |control| control.admit(lane, weight),
        )
    }

    fn cancellation_requested(&self) -> bool {
        if self.drain.is_draining() {
            // The supervisor normally propagates drain on its renewal tick, but a ready
            // downstream future may start drain and return without yielding back to that
            // supervisor. Observe the gate inside the session as well so the same poll
            // cannot dispatch a replacement after shutdown has begun.
            self.guard.cancel().cancel();
        }
        self.guard.cancel().is_cancelled()
    }

    /// Retires the source wake under the same session and agent fence as every decision.
    ///
    /// This decision carries no journal mutation: the control item is a condition check,
    /// while the work update moves the exact pending row to done and removes its due keys.
    async fn retire(&mut self, work_id: String) -> Result<(), ActivationError> {
        let mut draft = self.draft(phase_tag(&self.state.phase));
        draft.retire_wake(work_id);
        self.commit(draft).await
    }

    fn draft(&self, phase: &'static str) -> Draft {
        Draft::new(&self.guard, self.ports.clock.now(), phase)
    }

    /// Drains journal-authoritative provider usage through the idempotent FIFO handoff.
    ///
    /// The assistant record always commits first. The publication marker always commits
    /// second. A crash on either side of FIFO acceptance therefore republishes the same
    /// deterministic fact ids and never loses or double-counts a message.
    async fn publish_pending_model_usage(&mut self) -> Result<(), ActivationError> {
        while let Some((effect, pending)) = self
            .state
            .pending_model_usage
            .first_key_value()
            .map(|(effect, pending)| (*effect, pending.clone()))
        {
            let publication = self
                .ports
                .model_usage
                .publish(&ModelUsageObservation {
                    organization: self.authority.organization,
                    workspace: self.authority.workspace,
                    session: self.key.session,
                    effect,
                    provider: pending.message.provider,
                    model: pending.message.model,
                    usage: pending.usage,
                    observed_at: pending.observed_at,
                })
                .await?;
            #[cfg(any(test, feature = "testing"))]
            if publication == crate::ports::ModelUsagePublication::Disabled {
                self.state.pending_model_usage.remove(&effect);
                continue;
            }
            let _ = publication;
            let mut draft = self.draft(phase_tag(&self.state.phase));
            draft.append(JournalRecord::ModelUsagePublished { effect });
            self.commit(draft).await?;
        }
        Ok(())
    }

    fn finish_current(
        &self,
        draft: &mut Draft,
        reason: FinishReason,
        failure: Option<TypedFailure>,
        ambiguous_effect: Option<EffectId>,
    ) -> Result<(), ActivationError> {
        let Some(run) = self.state.active_run else {
            decide::finish(draft, reason, failure);
            if let Some(parent) = self.state.parent {
                // The child's absorbing terminal and the parent's runnable fact are one
                // decision. The wake reason itself is only scheduling metadata: once the
                // parent is claimed, its folded `AwaitingChildren` wait remains authority
                // for which join (and deadline) is open. A stable per-child dedup key makes
                // an ambiguous transaction replay collapse rather than wake twice.
                draft.wake_agent(
                    AgentKey::new(self.key.session, parent),
                    self.ports.ids.wake_id(),
                    ParkReason::AwaitingUserMessage,
                    self.ports.clock.now(),
                    self.authority.workspace.to_string(),
                    self.policy.shard_for(parent),
                    format!("child:{}:terminal", self.agent().0.as_hyphenated()),
                );
            }
            return Ok(());
        };
        let message = self
            .state
            .active_message
            .ok_or(ActivationError::Unsupported {
                step: "finish_root_run_without_public_message",
                owed_by: "RunAdmitted must bind the public message that owns the run",
            })?;
        let active = self
            .authority
            .active
            .as_deref()
            .ok_or(ActivationError::Unsupported {
                step: "finish_root_run_without_boundary_authority",
                owed_by: "the production claimant must strongly bind the active session and run",
            })?;
        if active.run.id != run || active.session.active_run != Some(run) {
            return Err(ActivationError::Unsupported {
                step: "finish_root_run_with_mismatched_authority",
                owed_by: "the session head and Brain fold must name the same active run",
            });
        }
        let cancellation = if reason == FinishReason::Cancelled {
            active
                .session
                .mutation_guard
                .filter(|guard| guard.kind == aex_operation_domain::OperationKind::SessionCancel)
                .map(|guard| guard.holder)
        } else {
            None
        };
        if reason == FinishReason::Cancelled && cancellation.is_none() {
            return Err(ActivationError::Unsupported {
                step: "finish_cancelled_run_without_operation",
                owed_by: "the session cancellation authority must carry its operation guard",
            });
        }
        let ambiguous_effect = ambiguous_effect.or(self.state.ambiguous_effect);
        if reason == FinishReason::Interrupted && ambiguous_effect.is_none() {
            return Err(ActivationError::Unsupported {
                step: "finish_interrupted_run_without_effect",
                owed_by: "the outcome-unknown settlement must carry its exact effect",
            });
        }
        decide::finish_run(
            draft,
            run,
            message,
            active.session.revision.0,
            reason,
            failure,
            self.state.active_output_messages.clone(),
            cancellation,
            ambiguous_effect,
        );
        Ok(())
    }

    fn settle_unknown_current(
        &self,
        draft: &mut Draft,
        effect: EffectId,
        evidence: DispatchEvidence,
    ) -> Result<(), ActivationError> {
        if self.state.active_run.is_some() {
            decide::settle_root_unknown(draft, effect, evidence);
            self.finish_current(draft, FinishReason::Interrupted, None, Some(effect))
        } else {
            decide::settle_unknown(draft, effect, evidence);
            Ok(())
        }
    }

    /// Commits one decision and folds its own records into the in-memory state.
    ///
    /// Folding locally rather than re-reading is what makes an activation that owes several
    /// steps cost one page read: the records it just wrote are the ones it would read back.
    async fn commit(&mut self, draft: Draft) -> Result<(), ActivationError> {
        if draft.is_empty() {
            return Ok(());
        }
        let recorded_at = draft.now();
        let first_seq = self.guard.tail().map_or(JournalSeq::ZERO, JournalSeq::next);
        let records = draft.records().to_vec();
        let commit = draft.into_commit();
        let retirement_only = commit.is_retirement_only();
        let context = DecisionContext {
            authority: self.authority.clone(),
            lease_expires_at: self
                .claim
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .expires_at,
            now: recorded_at,
        };
        match self.ports.journal.commit(&context, &commit).await {
            Ok(receipt) => {
                let mut seq = first_seq;
                for record in records {
                    self.retained_bytes = self
                        .retained_bytes
                        .saturating_add(record.canonical_bytes()?.len());
                    let entry = JournalEntry::seal(seq, recorded_at, record)?;
                    apply(&mut self.state, &entry)?;
                    seq = seq.next();
                }
                if !retirement_only {
                    self.guard = self.guard.advanced(receipt.revision, receipt.tail);
                    self.steps = self.steps.saturating_add(1);
                }
                Ok(())
            }
            Err(CommitError::Condition(ConditionFailure::IdempotentReplay(_))) => {
                // The identical decision already committed. Re-reading is the honest way to
                // learn what it wrote: the receipt a replay returns describes the write this
                // attempt did not perform.
                if !retirement_only {
                    self.refresh_guard().await?;
                    self.reload().await?;
                    self.steps = self.steps.saturating_add(1);
                }
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn refresh_guard(&mut self) -> Result<(), ActivationError> {
        let head =
            self.ports
                .journal
                .load_head(&self.key)
                .await?
                .ok_or(ActivationError::Unsupported {
                    step: "reload_head",
                    owed_by: "the session authority, which owns the control row this agent lost",
                })?;
        self.guard = self
            .guard
            .advanced(head.revision, head.journal_tail.unwrap_or(JournalSeq::ZERO));
        self.claim
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .head = head;
        Ok(())
    }

    fn config(&self) -> Result<ResolvedAgentConfig, ActivationError> {
        self.state.config.as_ref().map_or(
            Err(ActivationError::Unsupported {
                step: "agent_started",
                owed_by: "the session authority, which appends `agent_started` before a wake is ever projected",
            }),
            |config| Ok((**config).clone()),
        )
    }

    async fn checkpoint_context_if_owed(
        &mut self,
        capability: &QualifiedModel,
    ) -> Result<(), ActivationError> {
        let decision = context::decide(
            &self.state,
            capability,
            &self.policy.context,
            self.retained_bytes,
        );
        if !decision.compaction_owed {
            return Ok(());
        }
        let created_at = self
            .state
            .turn_started_at
            .unwrap_or_else(|| self.ports.clock.now());
        let Some(candidate) = context::build_checkpoint(
            self.key,
            &self.state,
            self.checkpoint.as_ref(),
            decision.target_tokens,
            created_at,
        ) else {
            return Ok(());
        };
        let saved = self
            .ports
            .checkpoints
            .save(&self.guard, &self.authority, &candidate)
            .await;
        let metadata = match saved {
            Ok(metadata) => metadata,
            Err(_) if !decision.compaction_mandatory => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        self.retained_bytes = serde_json::to_vec(&candidate)
            .map_err(|_| crate::ports::CheckpointError::Corrupt)?
            .len();
        self.state = candidate.state;
        self.checkpoint = Some(metadata.clone());
        self.claim
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .head
            .checkpoint = Some(metadata);
        Ok(())
    }

    /// The effect whose intent this agent has already committed, if any.
    ///
    /// The planner cannot answer this: it reports `ModelCall` for a prepared effect and for
    /// a fresh one alike. Preparing a second effect for one step would charge the reservation
    /// twice and leave the first identity open for the rest of the agent's life.
    fn open_prepared(&self) -> Option<Prepared> {
        let Phase::Effecting { effect } = &self.state.phase else {
            return None;
        };
        match self.state.open_effects.get(effect)? {
            EffectState::Prepared { attempt } => Some(Prepared {
                id: *effect,
                attempt: *attempt,
                request_hash: None,
            }),
            _ => None,
        }
    }

    /// Prepares, dispatches and settles one model call.
    #[allow(
        clippy::too_many_lines,
        reason = "the pre-send write, the dispatch and the settlement are one ordered sequence; splitting it would put the order in two places and let them drift"
    )]
    async fn model_call(
        &mut self,
        resumed: Option<&DurableEffect>,
    ) -> Result<Step, ActivationError> {
        let config = self.config()?;
        let capability =
            self.ports
                .catalog
                .model(&config.catalog_pin, config.provider, &config.model)?;
        let now = self.ports.clock.now();
        let deadline =
            self.bounded_effect_deadline(now.plus_millis(self.policy.effect_deadline_ms));

        let existing = resumed.map(Prepared::from).or_else(|| self.open_prepared());
        let effect = existing.as_ref().map_or_else(
            || model_effect_id(self.agent(), &self.state),
            |open| open.id,
        );
        if config.system.is_some() {
            return Err(ActivationError::UnhydratedSystem);
        }
        if self
            .state
            .open_user
            .iter()
            .any(|reference| matches!(reference, ContentBlockRef::Placed { .. }))
        {
            return Err(ActivationError::UnhydratedContent);
        }
        self.checkpoint_context_if_owed(&capability).await?;
        let mut messages = context::render(&self.state, &self.policy.context);
        let open_user = self
            .state
            .open_user
            .iter()
            .filter_map(|reference| match reference {
                ContentBlockRef::Inline { block } => Some(block.clone()),
                ContentBlockRef::Placed { .. } => None,
            })
            .collect::<Vec<_>>();
        if !open_user.is_empty() {
            messages.push(CanonicalMessage {
                role: Role::User,
                blocks: open_user,
            });
        }
        let max_output_tokens = capability.limits().max_output_tokens;
        let tool_fields = model_tool_fields(
            &capability,
            self.ports.tools.advertise(&config.catalog_pin)?,
        )?;
        let mut request = CanonicalModelRequest {
            selection: capability,
            system: Vec::new(),
            messages,
            tools: tool_fields.tools,
            tool_choice: tool_fields.choice,
            parallel_tools: tool_fields.parallel,
            max_output_tokens,
            temperature_milli: None,
            top_p_milli: None,
            stop_sequences: Vec::new(),
            reasoning: ReasoningRequest::ProviderDefault,
            structured_output: None,
            cache_breakpoints: Vec::new(),
            correlation: CorrelationId::from_effect(effect.0),
            request_hash: aex_wire::ContentHash::of(b"pending"),
        };
        request.request_hash = request.digest()?;
        // The canonical provider request owns a wire SHA-256 digest; Brain's
        // effect identity owns a blake3 `ContentHash`. Commit to the exact wire
        // digest bytes without relabelling one algorithm as the other.
        let request_hash = ContentHash::of(request.request_hash.as_bytes());
        // A prepared effect has never had its pre-send write, so this *is* its attempt.
        // Opening a second effect instead would leave the first dangling for the rest of the
        // agent's life and charge its reservation twice.
        let (effect, attempt) = if let Some(open) = existing {
            if open
                .request_hash
                .is_some_and(|stored| stored != request_hash)
            {
                return Err(ActivationError::RequestConflict { effect: open.id });
            }
            (open.id, open.attempt)
        } else {
            let id = effect;
            let mut draft = self.draft(phase_tag(&self.state.phase));
            decide::prepare(
                &mut draft,
                id,
                None,
                EffectKind::ModelCall,
                EffectClass::NonReplayable,
                request_hash,
                1,
                deadline,
                provider_call_reservation(&self.state),
            );
            self.commit(draft).await?;
            (id, 1)
        };

        if self.cancellation_requested() {
            self.hand_back().await?;
            return Ok(Step::Stop(Stop::HandedBack));
        }

        let _dispatch_permits = match self.dispatch_admission(DispatchLane::Provider, 1) {
            DispatchDecision::Admitted(permits) => permits,
            DispatchDecision::Deferred(reason) => {
                // The effect is still prepared: no pre-send ticket exists and no byte may
                // have left. Persist the exact pressure reason and release the claim rather
                // than waiting locally under its lease.
                self.hand_back_for(reason).await?;
                return Ok(Step::Stop(Stop::HandedBack));
            }
        };

        // The durable pre-send write. Nothing below this line may run without the ticket it
        // mints, which is why `dispatch` accepts nothing else.
        let ticket = self
            .ports
            .effects
            .mark_dispatch_started(
                &self.guard,
                &self.authority,
                &effect,
                attempt,
                self.ports.clock.now(),
            )
            .await?;
        let preview_port = Arc::clone(&self.ports.previews);
        let public_message = self
            .state
            .active_run
            .map(|run| decide::public_message_id(run, effect, 0x01));
        let preview = ScopedPreviewSink::new(
            preview_port.as_ref(),
            PreviewScope {
                session: self.key.session,
                agent: self.agent(),
                effect,
                attempt,
                message: public_message,
            },
        );
        let outcome = self
            .ports
            .provider
            .dispatch(
                &ticket,
                config.credential,
                &request,
                &preview,
                self.guard.cancel(),
            )
            .await;

        let mut draft = self.draft("effecting");
        match outcome {
            Ok(produced) => {
                if !produced.is_consistent() {
                    return Err(ActivationError::InvalidProviderOutcome);
                }
                let committed_journal_sequence = if let Some(run) = self.state.active_run {
                    let sequence = draft.next_seq().get().saturating_add(1);
                    decide::settle_root_model_call(&mut draft, run, effect, &produced);
                    Some(sequence)
                } else {
                    decide::settle_model_call(&mut draft, effect, &produced);
                    None
                };
                self.commit(draft).await?;
                if let Some(sequence) = committed_journal_sequence {
                    let _ = preview.committed(sequence);
                }
                self.publish_pending_model_usage().await?;
                Ok(Step::Continue)
            }
            Err(error) => match classify_provider_failure(&error, attempt) {
                FailureSettlement::NotSent { stage, retryable } => {
                    decide::settle_known_failure(&mut draft, effect, stage, DispatchProof::NotSent);
                    if retryable && self.attempts < self.policy.max_provider_attempts {
                        self.attempts = self.attempts.saturating_add(1);
                        let next = self.ports.ids.effect_id(
                            &self.agent(),
                            draft.next_seq(),
                            EffectKind::ModelCall,
                        );
                        let mut next_request = request.clone();
                        next_request.correlation = CorrelationId::from_effect(next.0);
                        next_request.request_hash = next_request.digest()?;
                        let next_request_hash =
                            ContentHash::of(next_request.request_hash.as_bytes());
                        decide::prepare(
                            &mut draft,
                            next,
                            None,
                            EffectKind::ModelCall,
                            EffectClass::NonReplayable,
                            next_request_hash,
                            attempt.saturating_add(1),
                            deadline,
                            provider_call_reservation(&self.state),
                        );
                        if self.cancellation_requested() {
                            // Drain may re-arm only because the adapter proved no byte left.
                            // The replacement and its continuation wake share this commit,
                            // so shutdown cannot strand a prepared identity between them.
                            self.wake_continuation(&mut draft);
                            self.commit(draft).await?;
                            return Ok(Step::Stop(Stop::HandedBack));
                        }
                        self.commit(draft).await?;
                        return Ok(Step::Continue);
                    }
                    self.finish_current(
                        &mut draft,
                        FinishReason::Failed,
                        Some(decide::refusal(
                            "provider_dispatch_failed",
                            error.detail.as_str(),
                        )),
                        None,
                    )?;
                    self.commit(draft).await?;
                    Ok(Step::Stop(Stop::Finished(FinishReason::Failed)))
                }
                FailureSettlement::KnownFailure { stage, proof } => {
                    decide::settle_known_failure(&mut draft, effect, stage, proof);
                    self.finish_current(
                        &mut draft,
                        FinishReason::Failed,
                        Some(decide::refusal(
                            "provider_dispatch_failed",
                            error.detail.as_str(),
                        )),
                        None,
                    )?;
                    self.commit(draft).await?;
                    Ok(Step::Stop(Stop::Finished(FinishReason::Failed)))
                }
                FailureSettlement::Unknown(evidence) => {
                    self.settle_unknown_current(&mut draft, effect, *evidence)?;
                    self.commit(draft).await?;
                    Ok(Step::Stop(Stop::Finished(FinishReason::Interrupted)))
                }
            },
        }
    }

    /// Starts one bounded slice of the immutable pending-call batch concurrently.
    ///
    /// Every intent is committed before any start, each start still crosses its
    /// own durable pre-send fence, and every terminal result is appended in the
    /// assistant's original call order. The fold withholds the model-visible
    /// tool-result turn until all pending calls (including later slices and
    /// native children) are terminal.
    async fn tool_batch(&mut self, calls: Vec<PendingCall>) -> Result<Step, ActivationError> {
        let config = self.config()?;
        let mut jobs = Vec::new();
        let mut draft = self.draft(phase_tag(&self.state.phase));

        for call in calls
            .into_iter()
            .filter(|call| !is_native_subagent_tool(call.name.as_str()))
            .take(MAX_CONCURRENT_TOOL_STARTS)
        {
            // A successor routes from the immutable catalog pin again, but it
            // must reuse the call-linked prepared effect rather than opening a
            // second identity.
            if self.state.tool_effects.contains_key(&call.call) {
                return self.tool_call(None).await;
            }
            let route = self.ports.tools.route(&config.catalog_pin, &call.name)?;
            let permit = match route.executor {
                ExecutorRoute::ToolMux => {
                    if route.concurrency_weight == 0 {
                        return Err(ToolRoutingError::InvalidConcurrencyWeight {
                            name: route.name.as_str().to_owned(),
                        }
                        .into());
                    }
                    match self.dispatch_admission(DispatchLane::Network, route.concurrency_weight) {
                        DispatchDecision::Admitted(permit) => permit,
                        DispatchDecision::Deferred(reason) => {
                            drop(jobs);
                            self.hand_back_for(reason).await?;
                            return Ok(Step::Stop(Stop::HandedBack));
                        }
                    }
                }
                ExecutorRoute::BrainInline => None,
            };
            let request_hash = ContentHash::of(&canonicalize_value(&call.input)?);
            let now = self.ports.clock.now();
            let deadline =
                self.bounded_effect_deadline(now.plus_millis(i64::from(route.timeout_ms)));
            let effect =
                self.ports
                    .ids
                    .effect_id(&self.agent(), draft.next_seq(), EffectKind::ToolCall);
            decide::prepare(
                &mut draft,
                effect,
                Some(call.call.clone()),
                EffectKind::ToolCall,
                route.class,
                request_hash,
                1,
                deadline,
                Vec::new(),
            );
            jobs.push(ToolBatchJob {
                call,
                route,
                effect,
                attempt: 1,
                deadline,
                _permit: permit,
            });
        }

        if jobs.is_empty() {
            return Ok(Step::Nothing);
        }
        self.commit(draft).await?;
        if self.cancellation_requested() {
            self.hand_back().await?;
            return Ok(Step::Stop(Stop::HandedBack));
        }

        let control = ControlStateView {
            todo_state: self.state.todo_state.clone(),
            assistant_turns: self.state.assistant_turns,
            depth: self.state.budget.depth,
        };
        let max_result_bytes = self.policy.context.tool_result_bytes;
        let hands_generation = config.hands_generation;
        let mcp_servers = config.mcp_servers;
        let effects = Arc::clone(&self.ports.effects);
        let tools = Arc::clone(&self.ports.tools);
        let clock = Arc::clone(&self.ports.clock);
        let guard = &self.guard;
        let authority = &self.authority;
        let cancel = self.guard.cancel();
        let invocations = stream::iter(jobs.into_iter().map(|job| {
            let effects = Arc::clone(&effects);
            let tools = Arc::clone(&tools);
            let clock = Arc::clone(&clock);
            let control = control.clone();
            let mcp_servers = mcp_servers.clone();
            async move {
                let ticket = effects
                    .mark_dispatch_started(guard, authority, &job.effect, job.attempt, clock.now())
                    .await?;
                let prepared = PreparedToolCall {
                    call: job.call.call.clone(),
                    route: job.route.clone(),
                    input: job.call.input.clone(),
                    max_result_bytes,
                    hands_generation,
                    mcp_servers,
                    control,
                };
                let outcome = tools.invoke(&ticket, &prepared, cancel).await;
                if let Ok(ToolOutcome::Detached { operation, .. }) = &outcome {
                    let evidence = DispatchEvidence {
                        stage: DispatchStage::Streaming,
                        proof: DispatchProof::ResponseStarted,
                        attempt: job.attempt,
                        provider_request_id: None,
                        external_operation: None,
                        detached_tool: Some(DetachedOperationRef {
                            id: operation.clone(),
                            executor: job.route.executor,
                        }),
                        receipt: None,
                        detail: None,
                    };
                    effects.mark_response_started(&ticket, &evidence).await?;
                }
                Ok::<_, ActivationError>(ToolBatchInvocation { job, outcome })
            }
        }))
        .buffer_unordered(MAX_CONCURRENT_TOOL_STARTS)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;

        let mut invocations = invocations;
        invocations.sort_by_key(|invocation| invocation.job.call.order);
        let now = self.ports.clock.now();
        let mut draft = self.draft("effecting");
        let mut parked = false;
        for invocation in invocations {
            let ToolBatchInvocation { job, outcome } = invocation;
            match outcome {
                Ok(ToolOutcome::Completed(body)) if body.executed_on == job.route.executor => {
                    self.append_completed_tool(&mut draft, job.effect, &job.call, body);
                }
                Ok(ToolOutcome::Completed(_)) => {
                    let evidence = DispatchEvidence {
                        stage: DispatchStage::Terminal,
                        proof: DispatchProof::ResponseStarted,
                        attempt: job.attempt,
                        provider_request_id: None,
                        external_operation: None,
                        detached_tool: None,
                        receipt: None,
                        detail: Some(
                            "tool executor receipt route does not match the pinned route"
                                .to_owned(),
                        ),
                    };
                    decide::settle_tool_unknown(&mut draft, job.effect, evidence);
                    self.append_tool_error(
                        &mut draft,
                        job.effect,
                        &job.call,
                        job.route.executor,
                        "tool result route mismatch",
                    );
                }
                Ok(ToolOutcome::Detached {
                    operation,
                    poll_after,
                }) => {
                    let operation = DetachedOperationRef {
                        id: operation,
                        executor: job.route.executor,
                    };
                    let due = detached_poll_due(now, job.deadline, poll_after);
                    let wait = wait_id(self.agent(), draft.next_seq());
                    let reason = ParkReason::AwaitingToolResult {
                        call: job.call.call,
                        operation,
                    };
                    draft.append(JournalRecord::WaitOpened {
                        wait,
                        reason: reason.clone(),
                        due: Some(due),
                    });
                    draft.wake(
                        self.ports.ids.wake_id(),
                        reason,
                        due,
                        self.authority.workspace.to_string(),
                        self.policy.shard_for(self.agent()),
                    );
                    parked = true;
                }
                Err(error) => {
                    match classify_tool_failure(&error, job.attempt) {
                        FailureSettlement::NotSent { stage, .. } => {
                            decide::settle_known_failure(
                                &mut draft,
                                job.effect,
                                stage,
                                DispatchProof::NotSent,
                            );
                        }
                        FailureSettlement::KnownFailure { stage, proof } => {
                            decide::settle_known_failure(&mut draft, job.effect, stage, proof);
                        }
                        FailureSettlement::Unknown(evidence) => {
                            decide::settle_tool_unknown(&mut draft, job.effect, *evidence);
                        }
                    }
                    self.append_tool_error(
                        &mut draft,
                        job.effect,
                        &job.call,
                        job.route.executor,
                        error.detail.as_str(),
                    );
                }
            }
        }
        if parked {
            draft.phase("parked");
        }
        self.commit(draft).await?;
        Ok(if parked {
            Step::Stop(Stop::Parked)
        } else {
            Step::Continue
        })
    }

    fn append_completed_tool(
        &self,
        draft: &mut Draft,
        effect: EffectId,
        call: &PendingCall,
        body: ToolResultBody,
    ) {
        if let Some(run) = self.state.active_run {
            decide::settle_root_tool_call(
                draft,
                run,
                effect,
                call.call.clone(),
                body.content,
                body.is_error,
                body.executed_on,
                body.duration_ms,
                body.checksum,
            );
        } else {
            decide::settle_tool_call(
                draft,
                effect,
                call.call.clone(),
                body.content,
                body.is_error,
                body.executed_on,
                body.duration_ms,
                body.checksum,
            );
        }
    }

    fn append_tool_error(
        &self,
        draft: &mut Draft,
        effect: EffectId,
        call: &PendingCall,
        executed_on: ExecutorRoute,
        detail: &str,
    ) {
        let content = vec![ToolResultPart::Text {
            text: BoundedString::truncating(detail),
        }];
        if let Some(run) = self.state.active_run {
            decide::append_root_tool_result(
                draft,
                run,
                effect,
                call.call.clone(),
                content,
                true,
                executed_on,
                0,
            );
        } else {
            draft.append(JournalRecord::ToolResult {
                public_message: None,
                call: call.call.clone(),
                content,
                is_error: true,
                executed_on,
                duration_ms: 0,
                effect,
            });
        }
    }

    /// Prepares, dispatches and settles the first outstanding tool call.
    #[allow(
        clippy::too_many_lines,
        reason = "same ordered sequence as the model call: the pre-send write, the invocation and the settlement belong in one place"
    )]
    async fn tool_call(
        &mut self,
        resumed: Option<&DurableEffect>,
    ) -> Result<Step, ActivationError> {
        let config = self.config()?;
        let call = resumed
            .and_then(|effect| {
                self.state
                    .tool_effects
                    .iter()
                    .find_map(|(call, linked)| (*linked == effect.id).then(|| call.clone()))
            })
            .and_then(|call| self.state.pending_calls.get(&call).cloned())
            .or_else(|| first_pending(&self.state));
        let Some(call) = call else {
            return Ok(Step::Nothing);
        };
        let route = self.ports.tools.route(&config.catalog_pin, &call.name)?;
        let request_hash = ContentHash::of(&canonicalize_value(&call.input)?);
        let now = self.ports.clock.now();
        let deadline = self.bounded_effect_deadline(now.plus_millis(i64::from(route.timeout_ms)));

        let existing = resumed.map(Prepared::from).or_else(|| {
            self.state.tool_effects.get(&call.call).and_then(|effect| {
                match self.state.open_effects.get(effect) {
                    Some(EffectState::Prepared { attempt }) => Some(Prepared {
                        id: *effect,
                        attempt: *attempt,
                        request_hash: None,
                    }),
                    _ => None,
                }
            })
        });
        let (effect, attempt) = if let Some(open) = existing {
            if open
                .request_hash
                .is_some_and(|stored| stored != request_hash)
            {
                return Err(ActivationError::RequestConflict { effect: open.id });
            }
            (open.id, open.attempt)
        } else {
            let id = self.ports.ids.effect_id(
                &self.agent(),
                self.state.expected_seq(),
                EffectKind::ToolCall,
            );
            let mut draft = self.draft(phase_tag(&self.state.phase));
            decide::prepare(
                &mut draft,
                id,
                Some(call.call.clone()),
                EffectKind::ToolCall,
                route.class,
                request_hash,
                1,
                deadline,
                Vec::new(),
            );
            self.commit(draft).await?;
            (id, 1)
        };

        if self.cancellation_requested() {
            self.hand_back().await?;
            return Ok(Step::Stop(Stop::HandedBack));
        }

        let dispatch_lane = match route.executor {
            ExecutorRoute::ToolMux => Some(DispatchLane::Network),
            ExecutorRoute::BrainInline => None,
        };
        let _dispatch_permits = if let Some(lane) = dispatch_lane {
            if route.concurrency_weight == 0 {
                return Err(ToolRoutingError::InvalidConcurrencyWeight {
                    name: route.name.as_str().to_owned(),
                }
                .into());
            }
            match self.dispatch_admission(lane, route.concurrency_weight) {
                DispatchDecision::Admitted(permits) => permits,
                DispatchDecision::Deferred(reason) => {
                    self.hand_back_for(reason).await?;
                    return Ok(Step::Stop(Stop::HandedBack));
                }
            }
        } else {
            None
        };

        let ticket = self
            .ports
            .effects
            .mark_dispatch_started(
                &self.guard,
                &self.authority,
                &effect,
                attempt,
                self.ports.clock.now(),
            )
            .await?;
        let prepared = PreparedToolCall {
            call: call.call.clone(),
            route: route.clone(),
            input: call.input.clone(),
            max_result_bytes: self.policy.context.tool_result_bytes,
            hands_generation: config.hands_generation,
            mcp_servers: config.mcp_servers.clone(),
            control: ControlStateView {
                todo_state: self.state.todo_state.clone(),
                assistant_turns: self.state.assistant_turns,
                depth: self.state.budget.depth,
            },
        };
        let outcome = self
            .ports
            .tools
            .invoke(&ticket, &prepared, self.guard.cancel())
            .await;

        let mut draft = self.draft("effecting");
        match outcome {
            Ok(ToolOutcome::Completed(body)) => {
                if body.executed_on != route.executor {
                    let evidence = DispatchEvidence {
                        stage: DispatchStage::Terminal,
                        proof: DispatchProof::ResponseStarted,
                        attempt,
                        provider_request_id: None,
                        external_operation: None,
                        detached_tool: None,
                        receipt: None,
                        detail: Some(
                            "tool executor receipt route does not match the pinned route"
                                .to_owned(),
                        ),
                    };
                    self.settle_unknown_current(&mut draft, effect, evidence)?;
                    self.commit(draft).await?;
                    return Ok(Step::Stop(Stop::Finished(FinishReason::Interrupted)));
                }
                if let Some(run) = self.state.active_run {
                    decide::settle_root_tool_call(
                        &mut draft,
                        run,
                        effect,
                        call.call.clone(),
                        body.content,
                        body.is_error,
                        body.executed_on,
                        body.duration_ms,
                        body.checksum,
                    );
                } else {
                    decide::settle_tool_call(
                        &mut draft,
                        effect,
                        call.call.clone(),
                        body.content,
                        body.is_error,
                        body.executed_on,
                        body.duration_ms,
                        body.checksum,
                    );
                }
                self.commit(draft).await?;
                Ok(Step::Continue)
            }
            Ok(ToolOutcome::Detached {
                operation,
                poll_after,
            }) => {
                // The connection is released and the result queried later, so a long tool
                // never holds a socket or a lease. The operation id goes into the durable
                // evidence first: without it the effect would be unresolvable rather than
                // detached.
                let operation = DetachedOperationRef {
                    id: operation,
                    executor: route.executor,
                };
                let evidence = DispatchEvidence {
                    stage: DispatchStage::Streaming,
                    proof: DispatchProof::ResponseStarted,
                    attempt,
                    provider_request_id: None,
                    external_operation: None,
                    detached_tool: Some(operation.clone()),
                    receipt: None,
                    detail: None,
                };
                self.ports
                    .effects
                    .mark_response_started(&ticket, &evidence)
                    .await?;
                let due = detached_poll_due(now, deadline, poll_after);
                let wait = wait_id(self.agent(), draft.next_seq());
                let reason = ParkReason::AwaitingToolResult {
                    call: call.call.clone(),
                    operation,
                };
                draft.append(JournalRecord::WaitOpened {
                    wait,
                    reason: reason.clone(),
                    due: Some(due),
                });
                draft.phase("parked");
                draft.wake(
                    self.ports.ids.wake_id(),
                    reason,
                    due,
                    self.authority.workspace.to_string(),
                    self.policy.shard_for(self.agent()),
                );
                self.commit(draft).await?;
                Ok(Step::Stop(Stop::Parked))
            }
            Err(error) => match classify_tool_failure(&error, attempt) {
                FailureSettlement::NotSent { stage, retryable } => {
                    decide::settle_known_failure(&mut draft, effect, stage, DispatchProof::NotSent);
                    if retryable && self.cancellation_requested() {
                        let next = self.ports.ids.effect_id(
                            &self.agent(),
                            draft.next_seq(),
                            EffectKind::ToolCall,
                        );
                        decide::prepare(
                            &mut draft,
                            next,
                            Some(call.call.clone()),
                            EffectKind::ToolCall,
                            route.class,
                            request_hash,
                            attempt.saturating_add(1),
                            deadline,
                            Vec::new(),
                        );
                        self.wake_continuation(&mut draft);
                        self.commit(draft).await?;
                        return Ok(Step::Stop(Stop::HandedBack));
                    }
                    // A tool that could not be dispatched is a *result* the model decides
                    // about, not a reason to end the run: the alternative is terminalizing a
                    // whole session because one optional tool was unavailable.
                    let content = vec![ToolResultPart::Text {
                        text: BoundedString::truncating(error.detail.as_str()),
                    }];
                    if let Some(run) = self.state.active_run {
                        decide::append_root_tool_result(
                            &mut draft,
                            run,
                            effect,
                            call.call.clone(),
                            content,
                            true,
                            route.executor,
                            0,
                        );
                    } else {
                        draft.append(JournalRecord::ToolResult {
                            public_message: None,
                            call: call.call.clone(),
                            content,
                            is_error: true,
                            executed_on: route.executor,
                            duration_ms: 0,
                            effect,
                        });
                    }
                    self.commit(draft).await?;
                    Ok(Step::Continue)
                }
                FailureSettlement::KnownFailure { stage, proof } => {
                    decide::settle_known_failure(&mut draft, effect, stage, proof);
                    let content = vec![ToolResultPart::Text {
                        text: BoundedString::truncating(error.detail.as_str()),
                    }];
                    if let Some(run) = self.state.active_run {
                        decide::append_root_tool_result(
                            &mut draft,
                            run,
                            effect,
                            call.call.clone(),
                            content,
                            true,
                            route.executor,
                            0,
                        );
                    } else {
                        draft.append(JournalRecord::ToolResult {
                            public_message: None,
                            call: call.call.clone(),
                            content,
                            is_error: true,
                            executed_on: route.executor,
                            duration_ms: 0,
                            effect,
                        });
                    }
                    self.commit(draft).await?;
                    Ok(Step::Continue)
                }
                FailureSettlement::Unknown(evidence) => {
                    self.settle_unknown_current(&mut draft, effect, *evidence)?;
                    self.commit(draft).await?;
                    Ok(Step::Stop(Stop::Finished(FinishReason::Interrupted)))
                }
            },
        }
    }

    /// Executes Brain-owned structured-concurrency tools inside the same
    /// fenced decision as their journal result. No generic executor is called,
    /// so a retry cannot duplicate a child or stop request after ambiguity.
    async fn native_subagent_tool(&mut self, call: PendingCall) -> Result<Step, ActivationError> {
        match call.name.as_str() {
            "create_subagent" => self.create_subagent(call).await,
            "stop_subagent" => self.stop_subagent(call).await,
            "wait_subagents" => self.wait_subagents(call).await,
            _ => unreachable!("native tool predicate is a closed set"),
        }
    }

    async fn create_subagent(&mut self, call: PendingCall) -> Result<Step, ActivationError> {
        let arguments = call.input.to_value();
        let Some(prompt) = arguments.get("prompt").and_then(serde_json::Value::as_str) else {
            return self
                .native_refusal(call, "create_subagent requires a prompt")
                .await;
        };
        let depth = self.state.budget.depth.saturating_add(1);
        if depth > aex_brain_domain::budget::MAX_SUBAGENT_DEPTH {
            return self
                .native_refusal(call, "subagent depth limit (3) reached")
                .await;
        }

        let ordinal = self.state.spawn_ordinal;
        let child = child_agent_id(self.agent(), ordinal);
        let join = native_join_id(self.agent(), self.state.expected_seq(), 0x31);
        let config = self.config()?;
        let mut budget = self.state.budget.limit;
        budget.set(
            Dimension::TotalChildrenCreated,
            u64::from(aex_brain_domain::budget::MAX_SUBAGENTS_PER_SESSION),
        );
        let grant = DimensionVector::ZERO;
        let input = vec![ContentBlockRef::Inline {
            block: CanonicalBlock::Text {
                text: BoundedString::truncating(prompt),
                annotations: Vec::new(),
            },
        }];

        let mut draft = self.draft("awaiting_tools");
        draft.append(JournalRecord::JoinOpened {
            join,
            mode: JoinMode::All,
            members: vec![child],
            shards: 1,
        });
        draft.append(JournalRecord::ChildSpawned {
            child,
            ordinal,
            grant,
            join,
            queued_reason: None,
        });
        draft.child(ChildWrite::Spawn {
            child,
            ordinal,
            grant,
            join,
            queued_reason: None,
            bootstrap: Box::new(ChildBootstrap {
                config: Box::new(config),
                input,
                depth,
                budget,
            }),
        });
        draft.join(JoinWrite::Open {
            join,
            mode: JoinMode::All,
            members: vec![child],
            shards: 1,
        });
        draft.charge_session(Dimension::TotalChildrenCreated, 1);
        draft.wake_agent(
            AgentKey::new(self.key.session, child),
            self.ports.ids.wake_id(),
            ParkReason::AwaitingUserMessage,
            self.ports.clock.now(),
            self.authority.workspace.to_string(),
            self.policy.shard_for(child),
            format!("child:{}:start", child.0.as_hyphenated()),
        );
        let public = PublicAgentId::from_uuid7(
            Uuid7::from_bytes(*child.0.as_bytes())
                .expect("child_agent_id always constructs UUIDv7"),
        );
        self.append_native_result(
            &mut draft,
            &call,
            serde_json::json!({
                "agentId": public.to_string(),
                "state": "starting",
                "depth": depth,
                "ordinal": ordinal,
            }),
            false,
        )?;
        self.commit(draft).await?;
        Ok(Step::Continue)
    }

    async fn stop_subagent(&mut self, call: PendingCall) -> Result<Step, ActivationError> {
        let arguments = call.input.to_value();
        let Some(raw) = arguments.get("agentId").and_then(serde_json::Value::as_str) else {
            return self
                .native_refusal(call, "stop_subagent requires agentId")
                .await;
        };
        let Ok(public) = raw.parse::<PublicAgentId>() else {
            return self.native_refusal(call, "agentId is malformed").await;
        };
        let child = AgentId(Uuid::from_bytes(*public.uuid7().as_bytes()));
        let Some(record) = self.state.children.get(&child) else {
            return self
                .native_refusal(call, "agentId is not a direct child")
                .await;
        };
        if record.state.is_terminal() {
            return self
                .native_refusal(call, "subagent is already terminal")
                .await;
        }
        let mut draft = self.draft("awaiting_tools");
        draft.child(ChildWrite::RequestStop { child });
        draft.wake_agent(
            AgentKey::new(self.key.session, child),
            self.ports.ids.wake_id(),
            ParkReason::AwaitingUserMessage,
            self.ports.clock.now(),
            self.authority.workspace.to_string(),
            self.policy.shard_for(child),
            format!("child:{}:stop", child.0.as_hyphenated()),
        );
        self.append_native_result(
            &mut draft,
            &call,
            serde_json::json!({"requested": true, "state": "stopping"}),
            false,
        )?;
        self.commit(draft).await?;
        Ok(Step::Continue)
    }

    async fn wait_subagents(&mut self, call: PendingCall) -> Result<Step, ActivationError> {
        let arguments = call.input.to_value();
        let mode = match arguments.get("mode").and_then(serde_json::Value::as_str) {
            Some("any") => JoinMode::Any,
            Some("all") => JoinMode::All,
            _ => return self.native_refusal(call, "mode must be any or all").await,
        };
        let Some(ids) = arguments
            .get("agentIds")
            .and_then(serde_json::Value::as_array)
        else {
            return self.native_refusal(call, "agentIds is required").await;
        };
        if ids.is_empty()
            || ids.len()
                > usize::try_from(aex_brain_domain::budget::MAX_SUBAGENTS_PER_SESSION)
                    .expect("the subagent limit fits usize")
        {
            return self
                .native_refusal(call, "agentIds must contain 1 to 12 direct children")
                .await;
        }
        let mut members = Vec::with_capacity(ids.len());
        for value in ids {
            let Some(raw) = value.as_str() else {
                return self
                    .native_refusal(call, "agentIds must contain strings")
                    .await;
            };
            let Ok(public) = raw.parse::<PublicAgentId>() else {
                return self
                    .native_refusal(call, "agentIds contains a malformed id")
                    .await;
            };
            let child = AgentId(Uuid::from_bytes(*public.uuid7().as_bytes()));
            if !self.state.children.contains_key(&child) || members.contains(&child) {
                return self
                    .native_refusal(call, "agentIds must be unique direct children")
                    .await;
            }
            members.push(child);
        }
        let mut draft = self.draft("awaiting_tools");
        let terminal = self.observe_terminal_children(&members, &mut draft).await?;
        if join_satisfied(mode, terminal.len(), members.len()) {
            self.append_wait_result(&mut draft, &call, mode, &members, &terminal, false)?;
            self.commit(draft).await?;
            return Ok(Step::Continue);
        }

        let join = native_join_id(self.agent(), self.state.expected_seq(), 0x32);
        let shards = aex_brain_domain::wire_pending::join_shards(members.len());
        draft.append(JournalRecord::JoinOpened {
            join,
            mode,
            members: members.clone(),
            shards,
        });
        draft.join(JoinWrite::Open {
            join,
            mode,
            members,
            shards,
        });
        let timeout_seconds = arguments
            .get("timeoutSeconds")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(3_600);
        let deadline =
            self.ports.clock.now().plus_millis(
                i64::try_from(timeout_seconds.saturating_mul(1_000)).unwrap_or(i64::MAX),
            );
        let wait = wait_id(self.agent(), draft.next_seq());
        let reason = ParkReason::AwaitingChildren { join, deadline };
        draft.append(JournalRecord::WaitOpened {
            wait,
            reason: reason.clone(),
            due: Some(deadline),
        });
        draft.phase("parked");
        draft.wake(
            WakeId(wait.0),
            reason,
            deadline,
            self.authority.workspace.to_string(),
            self.policy.shard_for(self.agent()),
        );
        self.commit(draft).await?;
        Ok(Step::Stop(Stop::Parked))
    }

    async fn resume_subagent_wait(
        &mut self,
        join: JoinId,
        deadline: Timestamp,
    ) -> Result<Step, ActivationError> {
        let Some(group) = self.state.joins.get(&join).cloned() else {
            return Err(ActivationError::Unsupported {
                step: "resume_subagent_wait",
                owed_by: "the JoinOpened record paired with every AwaitingChildren wait",
            });
        };
        let Some((wait, _)) = self.state.waits.iter().find(|(_, reason)| {
            matches!(reason, ParkReason::AwaitingChildren { join: open, .. } if *open == join)
        }) else {
            return Err(ActivationError::Unsupported {
                step: "resume_subagent_wait",
                owed_by: "the open durable child wait",
            });
        };
        let Some(call) =
            first_pending(&self.state).filter(|call| call.name.as_str() == "wait_subagents")
        else {
            return Err(ActivationError::Unsupported {
                step: "resume_subagent_wait",
                owed_by: "the unresolved wait_subagents tool call",
            });
        };
        let mut draft = self.draft("parked");
        let terminal = self
            .observe_terminal_children(&group.members, &mut draft)
            .await?;
        let timed_out = self.ports.clock.now() >= deadline;
        if join_satisfied(group.mode, terminal.len(), group.members.len()) || timed_out {
            draft.append(JournalRecord::WaitResolved {
                wait: *wait,
                resolution: if timed_out {
                    WaitResolution::Expired
                } else {
                    WaitResolution::Delivered
                },
            });
            // `wait` also deterministically names its one deadline wake. Retiring it in
            // the resolution transaction prevents an early child completion from leaving
            // a future timeout row behind. When the deadline wake is itself the source,
            // outer source retirement observes this already-retired row idempotently.
            draft.retire_wake(format!("wrk_{}", wait.0.as_simple()));
            self.append_wait_result(
                &mut draft,
                &call,
                group.mode,
                &group.members,
                &terminal,
                timed_out,
            )?;
            self.commit(draft).await?;
            return Ok(Step::Continue);
        }

        // A child-terminal wake can arrive before every member is done. The parent remains
        // parked without manufacturing a polling wake: later child terminals each carry
        // their own durable wake, while the single deadline wake created with `WaitOpened`
        // remains the timeout backstop.
        if !draft.is_empty() {
            self.commit(draft).await?;
        }
        Ok(Step::Stop(Stop::Parked))
    }

    async fn observe_terminal_children(
        &self,
        members: &[AgentId],
        draft: &mut Draft,
    ) -> Result<Vec<(AgentId, ChildOutcome)>, ActivationError> {
        let keys = members
            .iter()
            .map(|child| AgentKey::new(self.key.session, *child))
            .collect::<Vec<_>>();
        let heads = self.ports.journal.load_heads(&keys).await?;
        let mut terminal = Vec::new();
        for (child, head) in members.iter().zip(heads) {
            let Some(head) = head else {
                return Err(ActivationError::Unsupported {
                    step: "observe_subagent",
                    owed_by: "the child control created atomically with ChildSpawned",
                });
            };
            let Some(reason) = head.finish else {
                continue;
            };
            let outcome = match reason {
                FinishReason::Completed => ChildOutcome::Completed,
                FinishReason::Cancelled => ChildOutcome::Cancelled {
                    cause: CancelCause::Stopped,
                },
                _ => ChildOutcome::Failed,
            };
            terminal.push((*child, outcome));
            if self
                .state
                .children
                .get(child)
                .is_some_and(|record| !record.state.is_terminal())
            {
                draft.append(JournalRecord::ChildTerminal {
                    child: *child,
                    outcome,
                    rolled_up: Vec::new(),
                    result: None,
                });
                draft.child(ChildWrite::Terminal {
                    child: *child,
                    outcome,
                });
            }
        }
        Ok(terminal)
    }

    fn append_wait_result(
        &self,
        draft: &mut Draft,
        call: &PendingCall,
        mode: JoinMode,
        members: &[AgentId],
        terminal: &[(AgentId, ChildOutcome)],
        timed_out: bool,
    ) -> Result<(), ActivationError> {
        let terminal_ids = terminal
            .iter()
            .map(|(child, outcome)| {
                serde_json::json!({
                    "agentId": public_agent(*child),
                    "state": match outcome {
                        ChildOutcome::Completed => "completed",
                        ChildOutcome::Failed => "failed",
                        ChildOutcome::Cancelled { .. } => "cancelled",
                    }
                })
            })
            .collect::<Vec<_>>();
        let pending = members
            .iter()
            .filter(|member| !terminal.iter().any(|(child, _)| child == *member))
            .map(|child| public_agent(*child))
            .collect::<Vec<_>>();
        self.append_native_result(
            draft,
            call,
            serde_json::json!({
                "mode": match mode { JoinMode::Any => "any", JoinMode::All => "all" },
                "satisfied": join_satisfied(mode, terminal.len(), members.len()),
                "terminal": terminal_ids,
                "pending": pending,
                "timedOut": timed_out,
            }),
            false,
        )
    }

    async fn native_refusal(
        &mut self,
        call: PendingCall,
        message: &'static str,
    ) -> Result<Step, ActivationError> {
        let mut draft = self.draft("awaiting_tools");
        self.append_native_result(
            &mut draft,
            &call,
            serde_json::json!({"error": message}),
            true,
        )?;
        self.commit(draft).await?;
        Ok(Step::Continue)
    }

    fn append_native_result(
        &self,
        draft: &mut Draft,
        call: &PendingCall,
        value: serde_json::Value,
        is_error: bool,
    ) -> Result<(), ActivationError> {
        let effect = EffectId::derive(self.agent(), draft.next_seq(), 0x7f);
        let content = vec![ToolResultPart::Json {
            value: aex_wire::CanonicalJson::from_value(&value)
                .expect("the closed native subagent result is canonical JSON"),
        }];
        if let Some(run) = self.state.active_run {
            decide::append_root_tool_result(
                draft,
                run,
                effect,
                call.call.clone(),
                content,
                is_error,
                ExecutorRoute::BrainInline,
                0,
            );
        } else {
            draft.append(JournalRecord::ToolResult {
                public_message: None,
                call: call.call.clone(),
                content,
                is_error,
                executed_on: ExecutorRoute::BrainInline,
                duration_ms: 0,
                effect,
            });
        }
        Ok(())
    }

    /// Asks about a detached operation and settles whatever it says.
    #[allow(
        clippy::too_many_lines,
        reason = "one function per durable-operation status keeps the four settlements next to the statuses they answer"
    )]
    async fn resolve_detached(
        &mut self,
        effect: &DurableEffect,
        operation: &DetachedOperationRef,
    ) -> Result<Stop, ActivationError> {
        let Some((wait, call, waiting_on)) = self.open_tool_wait() else {
            return self.repair_detached_wait(effect, operation).await;
        };
        if waiting_on != *operation {
            return Err(ActivationError::Unsupported {
                step: "detached_operation_ref_mismatch",
                owed_by: "nothing: the effect evidence and journal wait must bind the same operation id and executor",
            });
        }
        let now = self.ports.clock.now();
        if now >= effect.deadline {
            let mut draft = self.draft("effecting");
            self.settle_detached_unknown(&mut draft, effect, wait)?;
            self.commit(draft).await?;
            return Ok(Stop::Finished(FinishReason::Interrupted));
        }
        let status = self.ports.tools.query(operation).await;
        let query_now = self.ports.clock.now();
        // The query may have taken a material fraction of the effect lifetime. Build the
        // decision only after it returns so its timestamp describes the commit attempt,
        // while `query_now` independently enforces and bounds the persisted deadline.
        let mut draft = self.draft("effecting");
        match status {
            Ok(DetachedStatus::Running { poll_after }) => {
                if query_now >= effect.deadline {
                    self.settle_detached_unknown(&mut draft, effect, wait)?;
                    self.commit(draft).await?;
                    return Ok(Stop::Finished(FinishReason::Interrupted));
                }
                // Still running. The poll timer is a wake, and a wake exists only inside a
                // decision, so re-arming it is a commit rather than a queue call.
                let due = detached_poll_due(query_now, effect.deadline, poll_after);
                draft.wake(
                    self.ports.ids.wake_id(),
                    ParkReason::AwaitingToolResult {
                        call,
                        operation: operation.clone(),
                    },
                    due,
                    self.authority.workspace.to_string(),
                    self.policy.shard_for(self.agent()),
                );
                self.commit(draft).await?;
                Ok(Stop::Parked)
            }
            Ok(DetachedStatus::Completed(body)) => {
                if body.executed_on != operation.executor {
                    self.settle_detached_unknown(&mut draft, effect, wait)?;
                    self.commit(draft).await?;
                    return Ok(Stop::Finished(FinishReason::Interrupted));
                }
                self.settle_detached(
                    &mut draft,
                    effect.id,
                    wait,
                    call,
                    body.content,
                    body.is_error,
                    body.executed_on,
                    body.duration_ms,
                    body.checksum,
                );
                self.wake_continuation(&mut draft);
                self.commit(draft).await?;
                Ok(Stop::HandedBack)
            }
            Ok(DetachedStatus::Failed { reason }) => {
                let content = vec![ToolResultPart::Text {
                    text: BoundedString::truncating(&reason),
                }];
                let checksum = ContentHash::of(&canonicalize_value(&content)?);
                self.settle_detached(
                    &mut draft,
                    effect.id,
                    wait,
                    call,
                    content,
                    true,
                    operation.executor,
                    0,
                    checksum,
                );
                self.wake_continuation(&mut draft);
                self.commit(draft).await?;
                Ok(Stop::HandedBack)
            }
            Err(error) if error.retryable => {
                if query_now >= effect.deadline {
                    self.settle_detached_unknown(&mut draft, effect, wait)?;
                    self.commit(draft).await?;
                    return Ok(Stop::Finished(FinishReason::Interrupted));
                }
                let due = detached_query_retry_due(query_now, effect.deadline);
                draft.wake(
                    self.ports.ids.wake_id(),
                    ParkReason::AwaitingToolResult {
                        call,
                        operation: operation.clone(),
                    },
                    due,
                    self.authority.workspace.to_string(),
                    self.policy.shard_for(self.agent()),
                );
                self.commit(draft).await?;
                Ok(Stop::Parked)
            }
            Ok(DetachedStatus::Unknown) | Err(_) => {
                self.settle_detached_unknown(&mut draft, effect, wait)?;
                self.commit(draft).await?;
                Ok(Stop::Finished(FinishReason::Interrupted))
            }
        }
    }

    /// Repairs the sole valid inter-write state of a detached tool dispatch.
    ///
    /// `mark_response_started` must persist the operation identity before `WaitOpened`:
    /// reversing them would allow a crash to leave an unresolvable wait. A crash between
    /// those writes therefore leaves an effect row with the operation and a journal whose
    /// phase is still `Effecting`. The journal is serial, so its first pending call is the
    /// exact call this one open effect dispatched. Re-checking the request hash and pinned
    /// executor makes that inference fail closed before the missing wait is reconstructed.
    ///
    /// This activation never queries or dispatches the operation. It only restores the
    /// durable wait (or atomically opens and expires it), after which ordinary recovery owns
    /// the external lookup.
    async fn repair_detached_wait(
        &mut self,
        effect: &DurableEffect,
        operation: &DetachedOperationRef,
    ) -> Result<Stop, ActivationError> {
        if effect.kind != EffectKind::ToolCall
            || !matches!(self.state.phase, Phase::Effecting { effect: open } if open == effect.id)
        {
            return Err(ActivationError::Unsupported {
                step: "detached_result_without_wait",
                owed_by: "nothing: only the response-started/tool-effecting inter-write state may reconstruct a missing detached wait",
            });
        }
        let Some(call) = first_pending(&self.state) else {
            return Err(ActivationError::Unsupported {
                step: "detached_result_without_pending_call",
                owed_by: "nothing: an open detached tool effect must still have its ordered pending call",
            });
        };
        let request_hash = ContentHash::of(&canonicalize_value(&call.input)?);
        let config = self.config()?;
        let route = self.ports.tools.route(&config.catalog_pin, &call.name)?;
        if effect.request_hash != request_hash
            || effect.class != route.class
            || operation.executor != route.executor
        {
            return Err(ActivationError::RequestConflict { effect: effect.id });
        }

        let now = self.ports.clock.now();
        let mut draft = self.draft("effecting");
        let wait = wait_id(self.agent(), draft.next_seq());
        let due = detached_query_retry_due(now, effect.deadline);
        let reason = ParkReason::AwaitingToolResult {
            call: call.call,
            operation: operation.clone(),
        };
        draft.append(JournalRecord::WaitOpened {
            wait,
            reason: reason.clone(),
            due: Some(due),
        });
        if now >= effect.deadline {
            self.settle_detached_unknown(&mut draft, effect, wait)?;
            self.commit(draft).await?;
            return Ok(Stop::Finished(FinishReason::Interrupted));
        }
        draft.phase("parked");
        draft.wake(
            self.ports.ids.wake_id(),
            reason,
            due,
            self.authority.workspace.to_string(),
            self.policy.shard_for(self.agent()),
        );
        self.commit(draft).await?;
        Ok(Stop::Parked)
    }

    /// Appends the settlement, the wait resolution and the result, in the order the fold
    /// needs them.
    ///
    /// The order is load-bearing. `WaitResolved` reads the pending calls to decide the phase,
    /// so resolving *after* the result would leave the agent awaiting input it already has.
    #[allow(
        clippy::too_many_arguments,
        reason = "these are exactly the fields of the three records this writes; bundling them would create a struct with one construction site"
    )]
    fn settle_detached(
        &self,
        draft: &mut Draft,
        effect: EffectId,
        wait: WaitId,
        call: ToolCallId,
        content: Vec<ToolResultPart>,
        is_error: bool,
        executed_on: ExecutorRoute,
        duration_ms: u32,
        checksum: ContentHash,
    ) {
        draft.append(JournalRecord::EffectSettled {
            effect,
            outcome: aex_brain_domain::effect::SettledOutcome::Complete { receipt: checksum },
            charged: Vec::new(),
        });
        draft.append(JournalRecord::WaitResolved {
            wait,
            resolution: WaitResolution::Delivered,
        });
        if let Some(run) = self.state.active_run {
            decide::append_root_tool_result(
                draft,
                run,
                effect,
                call,
                content,
                is_error,
                executed_on,
                duration_ms,
            );
        } else {
            draft.append(JournalRecord::ToolResult {
                public_message: None,
                call,
                content,
                is_error,
                executed_on,
                duration_ms,
                effect,
            });
        }
    }

    fn settle_detached_unknown(
        &self,
        draft: &mut Draft,
        effect: &DurableEffect,
        wait: WaitId,
    ) -> Result<(), ActivationError> {
        let evidence = effect.evidence.clone().unwrap_or_else(|| {
            DispatchEvidence::ambiguous(
                effect.state.attempt().unwrap_or(1),
                DispatchStage::Streaming,
            )
        });
        // Close the wait before the settlement. Children become absorbing at the
        // outcome-unknown record; roots append their non-absorbing run boundary next.
        draft.append(JournalRecord::WaitResolved {
            wait,
            resolution: WaitResolution::Cancelled,
        });
        self.settle_unknown_current(draft, effect.id, evidence)
    }

    fn open_tool_wait(&self) -> Option<(WaitId, ToolCallId, DetachedOperationRef)> {
        self.state
            .waits
            .iter()
            .find_map(|(wait, reason)| match reason {
                ParkReason::AwaitingToolResult { call, operation } => {
                    Some((*wait, call.clone(), operation.clone()))
                }
                _ => None,
            })
    }
}

const DETACHED_POLL_FLOOR: core::time::Duration = core::time::Duration::from_millis(250);
const DETACHED_QUERY_RETRY_AFTER: core::time::Duration = core::time::Duration::from_secs(5);
/// Local fan-out quantum. Global/tenant/target admission can be tighter.
const MAX_CONCURRENT_TOOL_STARTS: usize = 8;

struct ToolBatchJob {
    call: PendingCall,
    route: ToolRoute,
    effect: EffectId,
    attempt: u16,
    deadline: Timestamp,
    _permit: Option<crate::kernel::Reservation>,
}

struct ToolBatchInvocation {
    job: ToolBatchJob,
    outcome: Result<ToolOutcome, ToolDispatchError>,
}

fn detached_poll_due(
    now: Timestamp,
    deadline: Timestamp,
    requested: core::time::Duration,
) -> Timestamp {
    let delay = requested.max(DETACHED_POLL_FLOOR);
    bounded_due(now, deadline, delay)
}

fn detached_query_retry_due(now: Timestamp, deadline: Timestamp) -> Timestamp {
    bounded_due(now, deadline, DETACHED_QUERY_RETRY_AFTER)
}

fn bounded_due(now: Timestamp, deadline: Timestamp, delay: core::time::Duration) -> Timestamp {
    let proposed = now.plus_millis(millis(delay));
    proposed.min(deadline)
}

/// An effect whose intent is committed and whose pre-send write has not happened.
struct Prepared {
    id: EffectId,
    attempt: u16,
    /// The hash the durable record carries, where the caller read one.
    ///
    /// `None` when the identity came from the fold: the phase records which effect is open,
    /// not what it was going to send, and the recomputed request is the same function of the
    /// same fold either way.
    request_hash: Option<ContentHash>,
}

impl From<&DurableEffect> for Prepared {
    fn from(effect: &DurableEffect) -> Self {
        Self {
            id: effect.id,
            attempt: effect.state.attempt().unwrap_or(1),
            request_hash: Some(effect.request_hash),
        }
    }
}

/// What one plan-and-act cycle concluded.
enum Step {
    /// Nothing is owed. The wake is satisfied.
    Nothing,
    /// A decision committed and the agent still owes work.
    Continue,
    /// The activation ends.
    Stop(Stop),
}

/// The limits an agent runs under when its configuration has not been read.
///
/// Only reachable before `agent_started`, where the planner cannot do anything anyway. It
/// exists so the policy is total rather than optional at every call site.
const DEFAULT_LIMITS: aex_brain_domain::wire_pending::AgentLimits =
    aex_brain_domain::wire_pending::AgentLimits {
        turn_deadline_ms: 1,
        max_run_duration_ms: 1,
        max_depth: 0,
        max_fanout: 0,
    };

/// The first unresolved call, in the assistant message's tool-use order.
///
/// Order comes from the message that asked for the calls, never from a map's iteration
/// order, so a slow tool cannot reorder a turn.
fn first_pending(state: &FoldState) -> Option<PendingCall> {
    state
        .pending_in_order()
        .into_iter()
        .min_by_key(|call| call.order)
}

/// The deterministic identity of the wait opened at `seq`.
///
/// Derived rather than minted so a redelivered decision produces the identical record and
/// the journal put collapses it instead of opening a second wait.
fn wait_id(agent: AgentId, seq: JournalSeq) -> WaitId {
    WaitId(Uuid::from_bytes(
        EffectId::derive(agent, seq, EffectKind::DurableWait.tag()).0,
    ))
}

fn is_native_subagent_tool(name: &str) -> bool {
    matches!(name, "create_subagent" | "stop_subagent" | "wait_subagents")
}

fn native_join_id(agent: AgentId, seq: JournalSeq, tag: u8) -> JoinId {
    let mut bytes = EffectId::derive(agent, seq, tag).0;
    bytes[6] = (bytes[6] & 0x0f) | 0x70;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    JoinId(Uuid::from_bytes(bytes))
}

fn public_agent(agent: AgentId) -> String {
    PublicAgentId::from_uuid7(
        Uuid7::from_bytes(*agent.0.as_bytes()).expect("subagent identities are UUIDv7"),
    )
    .to_string()
}

const fn join_satisfied(mode: JoinMode, terminal: usize, members: usize) -> bool {
    match mode {
        JoinMode::Any => terminal > 0,
        JoinMode::All => terminal == members,
    }
}

const fn kind_name(kind: EffectKind) -> &'static str {
    match kind {
        EffectKind::ModelCall => "model_call",
        EffectKind::ToolCall => "tool_call",
        EffectKind::HandsOperation => "hands_operation",
        EffectKind::DurableWait => "durable_wait",
        EffectKind::ChildSpawn => "child_spawn",
    }
}

fn millis(duration: core::time::Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}
