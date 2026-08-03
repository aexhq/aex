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
    ActivationError, ActivationPolicy, AdmissionControl, AdmissionDecision, Outcome, Ports,
    Release, Stop,
};
use crate::kernel::{ActivationRegistry, DrainGate, RenewalOutcome, RenewalState};
use crate::ports::{
    BoxFuture, CancelToken, Claim, ClaimError, CommitError, ConditionFailure, ControlStateView,
    DecisionContext, DetachedStatus, DueRowIsolation, DueScanCursor, FenceGuard,
    MAX_DUE_ROW_ISOLATIONS, NullPreviewSink, PreparedToolCall, ReleaseDisposition,
    SessionAuthority, StoreError, StreamBudget, ToolOutcome, WakeDelivery, WakeOrigin, WakeState,
};
use aex_brain_domain::canonical::canonicalize_value;
use aex_brain_domain::child::QueuedReason;
use aex_brain_domain::context;
use aex_brain_domain::effect::{
    DetachedOperationRef, DispatchEvidence, DispatchProof, DispatchStage, DurableEffect,
    EffectClass, EffectKind, EffectState, RecoveryDecision, recover,
};
use aex_brain_domain::fold::{FoldState, PendingCall, Phase, apply};
use aex_brain_domain::ids::{
    AgentId, AgentKey, ContentHash, EffectId, JournalSeq, Timestamp, ToolCallId, WaitId,
};
use aex_brain_domain::journal::ExecutorRoute;
use aex_brain_domain::journal::{
    FinishReason, JournalEntry, JournalRecord, ParkReason, WaitResolution,
};
use aex_brain_domain::planner::{OwedStep, PlanPolicy, model_effect_id, plan};
use aex_brain_domain::wire_pending::{
    CanonicalMessage, CanonicalModelRequest, ContentBlockRef, DurableOperationSupport,
    ResolvedAgentConfig, Role,
};
use aex_model_catalog::BoundedString;
use aex_model_catalog::canonical::{CorrelationId, ReasoningRequest, ToolChoice, ToolResultPart};
use futures::stream::{self, StreamExt as _};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// One activation: everything from claiming an agent to acking its wake.
#[derive(Debug, Clone)]
pub struct Activation {
    ports: Ports,
    policy: ActivationPolicy,
    registry: Arc<ActivationRegistry>,
    drain: Arc<DrainGate>,
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
        }
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
            guard,
            state: FoldState::empty(),
            steps: 0,
            attempts: 1,
            resume: None,
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
            match outcome {
                Ok(outcome) => self
                    .retire_source(&mut session, &delivery.wake)
                    .await
                    .map(|()| outcome),
                Err(error) => Err(error),
            }
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
        let restore_bytes =
            u64::try_from(self.activation.policy().restore.max_bytes).unwrap_or(u64::MAX);
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
    guard: FenceGuard,
    state: FoldState,
    steps: u32,
    attempts: u16,
    resume: Option<Box<DurableEffect>>,
}

impl Session<'_> {
    const fn agent(&self) -> AgentId {
        self.key.agent
    }

    async fn drive(&mut self) -> Result<Outcome, ActivationError> {
        self.reload().await?;
        if let Some(stop) = self.recover_open().await? {
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

    /// Rebuilds the fold from the journal.
    ///
    /// A page that observes a gap returns no entries at all, so the agent never acts on a
    /// prefix of its own history: `read_page` refuses and this propagates the refusal. The
    /// complete restore must also equal the `(sequence, hash)` tail returned with the claim;
    /// neither an early EOF nor a same-sequence fork may reach recovery or planning.
    async fn reload(&mut self) -> Result<(), ActivationError> {
        let mut state = FoldState::empty();
        let mut from = JournalSeq::ZERO;
        let mut cursor = None;
        let mut restored_entries = 0_usize;
        let mut restored_bytes = 0_usize;
        loop {
            let remaining_entries = self
                .policy
                .restore
                .max_entries
                .saturating_sub(restored_entries);
            let remaining_bytes = self.policy.restore.max_bytes.saturating_sub(restored_bytes);
            if remaining_entries == 0 || remaining_bytes == 0 {
                return Err(StoreError::RestoreBudgetExhausted {
                    entries: restored_entries.saturating_add(usize::from(remaining_entries == 0)),
                    bytes: restored_bytes.saturating_add(usize::from(remaining_bytes == 0)),
                    max_entries: self.policy.restore.max_entries,
                    max_bytes: self.policy.restore.max_bytes,
                }
                .into());
            }
            let page_budget = crate::ports::ReadBudget {
                max_entries: self.policy.read.max_entries.min(remaining_entries),
                max_bytes: self.policy.read.max_bytes.min(remaining_bytes),
            };
            let page = match self
                .ports
                .journal
                .read_page(&self.key, from, page_budget, cursor.take())
                .await
            {
                Ok(page) => page,
                Err(StoreError::ReadBudgetExhausted { entries, bytes })
                    if remaining_bytes <= self.policy.read.max_bytes =>
                {
                    return Err(StoreError::RestoreBudgetExhausted {
                        entries: restored_entries.saturating_add(entries),
                        bytes: restored_bytes.saturating_add(bytes),
                        max_entries: self.policy.restore.max_entries,
                        max_bytes: self.policy.restore.max_bytes,
                    }
                    .into());
                }
                Err(error) => return Err(error.into()),
            };
            let next_entries = restored_entries.saturating_add(page.entries.len());
            let next_bytes = restored_bytes.saturating_add(page.hydrated_bytes);
            if next_entries > self.policy.restore.max_entries
                || next_bytes > self.policy.restore.max_bytes
            {
                return Err(StoreError::RestoreBudgetExhausted {
                    entries: next_entries,
                    bytes: next_bytes,
                    max_entries: self.policy.restore.max_entries,
                    max_bytes: self.policy.restore.max_bytes,
                }
                .into());
            }
            for entry in &page.entries {
                apply(&mut state, entry)?;
            }
            restored_entries = next_entries;
            restored_bytes = next_bytes;
            match page.next {
                Some(next) => {
                    if page.entries.is_empty() {
                        return Err(StoreError::Undecodable {
                            location: "journal continuation".to_owned(),
                            reason: "a continuation followed a page with no journal entries"
                                .to_owned(),
                        }
                        .into());
                    }
                    from = next.next();
                    cursor = Some(next);
                }
                None => break,
            }
        }
        let claim = self
            .claim
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let claimed_seq = claim.head.journal_tail;
        let claimed_hash = claim.head.journal_tail_hash;
        drop(claim);
        let folded_hash = state.hashes.last().copied();
        if state.tail != claimed_seq || folded_hash != claimed_hash {
            return Err(StoreError::JournalTailMismatch {
                claimed_seq,
                claimed_hash,
                folded_seq: state.tail,
                folded_hash,
            }
            .into());
        }
        self.state = state;
        Ok(())
    }

    /// Classifies whatever the previous owner left open.
    ///
    /// Runs **before** the planner, because the planner has no dispatch evidence. A
    /// dispatched non-replayable effect settles `OutcomeUnknown` here; nothing downstream
    /// ever gets the chance to treat it as retryable.
    async fn recover_open(&mut self) -> Result<Option<Stop>, ActivationError> {
        let open = self.ports.effects.load_open(&self.key).await?;
        let Some(effect) = open.into_iter().find(|effect| !effect.state.is_settled()) else {
            return Ok(None);
        };
        if matches!(effect.state, EffectState::Prepared { .. }) {
            // Intent committed, nothing sent: the one unambiguous case. The step loop
            // re-dispatches it under this fence rather than opening a second effect.
            self.resume = Some(Box::new(effect));
            return Ok(None);
        }
        match recover(&effect, self.support()) {
            RecoveryDecision::Interrupt { evidence } => {
                let mut draft = self.draft("effecting");
                decide::settle_unknown(&mut draft, effect.id, evidence);
                self.commit(draft).await?;
                Ok(Some(Stop::Finished(FinishReason::Interrupted)))
            }
            RecoveryDecision::QueryDurableOperation { .. } => Err(ActivationError::Unsupported {
                step: "query_non_tool_durable_operation",
                owed_by: "the provider or Hands recovery adapter selected by the effect kind; a tool executor route cannot safely answer this identity",
            }),
            RecoveryDecision::QueryDetachedTool { operation } => {
                self.resolve_detached(&effect, &operation).await.map(Some)
            }
            RecoveryDecision::ReconstructFromReceipt { .. } => Err(ActivationError::Unsupported {
                step: "reconstruct_from_receipt",
                owed_by: "aex-content-aws, which holds the committed response body Brain would rebuild the outcome from",
            }),
            RecoveryDecision::RetrySameEffect { .. } => Err(ActivationError::Unsupported {
                step: "retry_dispatched_effect",
                owed_by: "the effect driver's recovery controller (plan 07 S-5.3), which is what moves a dispatched effect back to prepared",
            }),
        }
    }

    fn support(&self) -> DurableOperationSupport {
        self.state
            .config
            .as_ref()
            .map_or(DurableOperationSupport::None, |config| {
                self.ports.catalog.durable_operation_support(
                    &config.catalog_pin,
                    config.provider,
                    &config.model,
                )
            })
    }

    /// One plan-and-act cycle.
    async fn step(&mut self) -> Result<Step, ActivationError> {
        if let Some(effect) = self.resume.take() {
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
        let owed = plan(&self.state, &self.plan_policy());
        match owed {
            OwedStep::Finished { reason } => Ok(Step::Stop(Stop::Finished(reason))),
            OwedStep::Finish { reason } => {
                let mut draft = self.draft(phase_tag(&self.state.phase));
                decide::finish(&mut draft, reason, self.state.failure.clone());
                self.commit(draft).await?;
                Ok(Step::Stop(Stop::Finished(reason)))
            }
            OwedStep::Park { reason } => self.park(*reason).await,
            OwedStep::ModelCall { .. } => self.model_call(None).await,
            OwedStep::ToolCalls { calls } => {
                if calls.is_empty() {
                    return Ok(Step::Nothing);
                }
                self.tool_call(None).await
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
        let mut draft = self.draft(phase_tag(&self.state.phase));
        self.wake_continuation(&mut draft);
        self.commit(draft).await
    }

    fn wake_continuation(&self, draft: &mut Draft) {
        draft.wake(
            self.ports.ids.wake_id(),
            ParkReason::AwaitingCapacity {
                reason: QueuedReason::RegionalCapacity,
            },
            self.ports.clock.now(),
            self.authority.workspace.to_string(),
            self.policy.shard_for(self.agent()),
        );
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

    fn stream_budget(&self, deadline: Timestamp) -> StreamBudget {
        StreamBudget {
            buffer_bytes: self.policy.stream_buffer_bytes,
            response_bytes: self.policy.stream_response_bytes,
            deadline,
            idle_timeout_ms: self.policy.stream_idle_timeout_ms,
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
        let deadline = now.plus_millis(self.policy.effect_deadline_ms);

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
        let mut messages = context::view(&self.state, &self.policy.context, 0);
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
        let mut request = CanonicalModelRequest {
            selection: capability,
            system: Vec::new(),
            messages,
            tools: Vec::new(),
            tool_choice: ToolChoice::None,
            parallel_tools: false,
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
        let outcome = self
            .ports
            .provider
            .dispatch(
                &ticket,
                config.credential,
                &request,
                &self.stream_budget(deadline),
                &NullPreviewSink,
                self.guard.cancel(),
            )
            .await;

        let mut draft = self.draft("effecting");
        match outcome {
            Ok(produced) => {
                if !produced.is_consistent() {
                    return Err(ActivationError::InvalidProviderOutcome);
                }
                decide::settle_model_call(&mut draft, effect, &produced);
                self.commit(draft).await?;
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
                    decide::finish(
                        &mut draft,
                        FinishReason::Failed,
                        Some(decide::refusal(
                            "provider_dispatch_failed",
                            error.detail.as_str(),
                        )),
                    );
                    self.commit(draft).await?;
                    Ok(Step::Stop(Stop::Finished(FinishReason::Failed)))
                }
                FailureSettlement::KnownFailure { stage, proof } => {
                    decide::settle_known_failure(&mut draft, effect, stage, proof);
                    decide::finish(
                        &mut draft,
                        FinishReason::Failed,
                        Some(decide::refusal(
                            "provider_dispatch_failed",
                            error.detail.as_str(),
                        )),
                    );
                    self.commit(draft).await?;
                    Ok(Step::Stop(Stop::Finished(FinishReason::Failed)))
                }
                FailureSettlement::Unknown(evidence) => {
                    decide::settle_unknown(&mut draft, effect, *evidence);
                    self.commit(draft).await?;
                    Ok(Step::Stop(Stop::Finished(FinishReason::Interrupted)))
                }
            },
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
        let Some(call) = first_pending(&self.state) else {
            return Ok(Step::Nothing);
        };
        let route = self.ports.tools.route(&config.catalog_pin, &call.name)?;
        let request_hash = ContentHash::of(&canonicalize_value(&call.input)?);
        let now = self.ports.clock.now();
        let deadline = now.plus_millis(i64::from(route.timeout_ms));

        let existing = resumed.map(Prepared::from).or_else(|| self.open_prepared());
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
            control: ControlStateView {
                todos: Vec::new(),
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
                    decide::settle_unknown(&mut draft, effect, evidence);
                    self.commit(draft).await?;
                    return Ok(Step::Stop(Stop::Finished(FinishReason::Interrupted)));
                }
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
                    draft.append(JournalRecord::ToolResult {
                        call: call.call.clone(),
                        content: vec![ToolResultPart::Text {
                            text: BoundedString::truncating(error.detail.as_str()),
                        }],
                        is_error: true,
                        executed_on: route.executor,
                        duration_ms: 0,
                        effect,
                    });
                    self.commit(draft).await?;
                    Ok(Step::Continue)
                }
                FailureSettlement::KnownFailure { stage, proof } => {
                    decide::settle_known_failure(&mut draft, effect, stage, proof);
                    draft.append(JournalRecord::ToolResult {
                        call: call.call.clone(),
                        content: vec![ToolResultPart::Text {
                            text: BoundedString::truncating(error.detail.as_str()),
                        }],
                        is_error: true,
                        executed_on: route.executor,
                        duration_ms: 0,
                        effect,
                    });
                    self.commit(draft).await?;
                    Ok(Step::Continue)
                }
                FailureSettlement::Unknown(evidence) => {
                    decide::settle_unknown(&mut draft, effect, *evidence);
                    self.commit(draft).await?;
                    Ok(Step::Stop(Stop::Finished(FinishReason::Interrupted)))
                }
            },
        }
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
            Self::settle_detached_unknown(&mut draft, effect, wait);
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
                    Self::settle_detached_unknown(&mut draft, effect, wait);
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
                    Self::settle_detached_unknown(&mut draft, effect, wait);
                    self.commit(draft).await?;
                    return Ok(Stop::Finished(FinishReason::Interrupted));
                }
                Self::settle_detached(
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
                Self::settle_detached(
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
                    Self::settle_detached_unknown(&mut draft, effect, wait);
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
                Self::settle_detached_unknown(&mut draft, effect, wait);
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
            Self::settle_detached_unknown(&mut draft, effect, wait);
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
        draft.append(JournalRecord::ToolResult {
            call,
            content,
            is_error,
            executed_on,
            duration_ms,
            effect,
        });
    }

    fn settle_detached_unknown(draft: &mut Draft, effect: &DurableEffect, wait: WaitId) {
        let evidence = effect.evidence.clone().unwrap_or_else(|| {
            DispatchEvidence::ambiguous(
                effect.state.attempt().unwrap_or(1),
                DispatchStage::Streaming,
            )
        });
        // `OutcomeUnknown` is terminal in the pure fold. Close the wait first so every
        // record in this atomic decision remains foldable; reversing these two appends
        // would place `WaitResolved` after an absorbing terminal.
        draft.append(JournalRecord::WaitResolved {
            wait,
            resolution: WaitResolution::Cancelled,
        });
        decide::settle_unknown(draft, effect.id, evidence);
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
        max_turns: 1,
        max_steps_per_turn: 1,
        turn_deadline_ms: 1,
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
