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
use crate::kernel::{ActivationRegistry, DrainGate};
use crate::ports::{
    ClaimError, CommitError, ConditionFailure, ControlStateView, DecisionContext, DetachedStatus,
    DueScanCursor, FenceGuard, NullPreviewSink, PreparedToolCall, ReleaseDisposition,
    SessionAuthority, StoreError, StreamBudget, ToolOutcome, WakeDelivery, WakeOrigin, WakeState,
};
use aex_brain_domain::canonical::canonicalize_value;
use aex_brain_domain::child::QueuedReason;
use aex_brain_domain::context;
use aex_brain_domain::effect::{
    DispatchEvidence, DispatchProof, DispatchStage, DurableEffect, EffectClass, EffectKind,
    EffectState, RecoveryDecision, recover,
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
    CanonicalBlock, CanonicalModelRequest, DurableOperationSupport, ResolvedAgentConfig,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU16, Ordering};
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
        let mut session = Session {
            ports: &self.ports,
            policy: &self.policy,
            key,
            authority: claim.authority.clone(),
            lease_expires_at: claim.expires_at,
            stop_requested: claim.head.stop_requested,
            guard,
            state: FoldState::empty(),
            steps: 0,
            attempts: 1,
            resume: None,
        };

        let outcome = if claim.head.finish.is_some() {
            Ok(Outcome::Idle)
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
        let disposition = match &outcome {
            Ok(Outcome::Progressed {
                stop: Stop::Parked, ..
            }) => ReleaseDisposition::Parked,
            Ok(_) => ReleaseDisposition::Committed,
            Err(_) => ReleaseDisposition::Abandoned,
        };
        // The lease goes back before the ack. A crash between them leaves a wake that
        // redelivers against an unowned agent, which is the cheap direction to fail in.
        let _ = self.ports.leases.release(claim, disposition).await;

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
}

/// What one pass over the queue did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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
        }
    }

    /// The activation this loop drives.
    #[must_use]
    pub const fn activation(&self) -> &Activation {
        &self.activation
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
        let shard_count = policy.due_shards.max(1);
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
                policy.receive_batch,
                after,
            )
            .await?;
        {
            let mut cursors = self.due_cursors.lock().expect("not poisoned");
            match page.next.clone() {
                Some(next) => {
                    cursors.insert(shard, next);
                }
                None => {
                    cursors.remove(&shard);
                }
            }
        }
        let recovered = page.wakes;
        report.recovered = recovered.len();
        report.malformed = page.malformed;
        let remaining = policy.receive_batch.saturating_sub(recovered.len());
        let wait = if recovered.is_empty() {
            policy.long_poll
        } else {
            core::time::Duration::ZERO
        };
        let mut deliveries: Vec<WakeDelivery> = recovered
            .into_iter()
            .map(|wake| WakeDelivery {
                wake,
                origin: WakeOrigin::DueScan,
            })
            .collect();
        if remaining > 0 {
            deliveries.extend(
                self.activation
                    .ports()
                    .wakes
                    .receive(remaining, wait)
                    .await?,
            );
        }
        report.received = deliveries.len();
        for delivery in deliveries {
            match self.drive(delivery).await {
                Ok(Outcome::Released(_)) => report.released += 1,
                Ok(_) => report.driven += 1,
                Err(_) => report.refused += 1,
            }
        }
        Ok(report)
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
        let _permits = match self.admission.admit() {
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
    key: AgentKey,
    authority: SessionAuthority,
    lease_expires_at: Timestamp,
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
    /// prefix of its own history: `read_page` refuses and this propagates the refusal.
    async fn reload(&mut self) -> Result<(), ActivationError> {
        let mut state = FoldState::empty();
        let mut from = JournalSeq::ZERO;
        loop {
            let page = self
                .ports
                .journal
                .read_page(&self.key, from, self.policy.read)
                .await?;
            let empty = page.entries.is_empty();
            for entry in &page.entries {
                apply(&mut state, entry)?;
            }
            match page.next {
                Some(next) if !empty => from = next,
                _ => break,
            }
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
            RecoveryDecision::QueryDurableOperation { id } => {
                self.resolve_detached(&effect, &id).await.map(Some)
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
        draft.wake(
            self.ports.ids.wake_id(),
            ParkReason::AwaitingCapacity {
                reason: QueuedReason::RegionalCapacity,
            },
            self.ports.clock.now(),
            self.authority.workspace.to_string(),
            self.policy.shard_for(self.agent()),
        );
        self.commit(draft).await
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
            lease_expires_at: self.lease_expires_at,
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
                    self.reload().await?;
                    self.refresh_guard().await?;
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
        let request = CanonicalModelRequest {
            provider: config.provider,
            model: config.model.clone(),
            system: config.system.clone(),
            turns: context::view(&self.state, &self.policy.context, 0),
            tools: Vec::new(),
            max_output_tokens: capability.max_output_tokens,
        };
        let request_hash = ContentHash::of(&canonicalize_value(&request)?);
        let now = self.ports.clock.now();
        let deadline = now.plus_millis(self.policy.effect_deadline_ms);

        let existing = resumed.map(Prepared::from).or_else(|| self.open_prepared());
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
            let id = model_effect_id(self.agent(), &self.state);
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

        // The durable pre-send write. Nothing below this line may run without the ticket it
        // mints, which is why `dispatch` accepts nothing else.
        let ticket = self
            .ports
            .effects
            .mark_dispatch_started(&self.guard, &effect, attempt, self.ports.clock.now())
            .await?;
        let outcome = self
            .ports
            .provider
            .dispatch(
                &ticket,
                &request,
                &self.stream_budget(deadline),
                &NullPreviewSink,
                self.guard.cancel(),
            )
            .await;

        let mut draft = self.draft("effecting");
        match outcome {
            Ok(produced) => {
                let (provider, model, usage) = decide::model_settlement(&produced);
                decide::settle_model_call(
                    &mut draft,
                    effect,
                    provider,
                    model,
                    &produced.message,
                    usage,
                )?;
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
                        decide::prepare(
                            &mut draft,
                            next,
                            EffectKind::ModelCall,
                            EffectClass::NonReplayable,
                            request_hash,
                            attempt.saturating_add(1),
                            deadline,
                            provider_call_reservation(&self.state),
                        );
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

        let ticket = self
            .ports
            .effects
            .mark_dispatch_started(&self.guard, &effect, attempt, self.ports.clock.now())
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
                decide::settle_tool_call(
                    &mut draft,
                    effect,
                    call.call.clone(),
                    body.blocks,
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
                let evidence = DispatchEvidence {
                    stage: DispatchStage::Streaming,
                    proof: DispatchProof::ResponseStarted,
                    attempt,
                    provider_request_id: None,
                    operation: Some(operation),
                    receipt: None,
                    detail: None,
                };
                self.ports
                    .effects
                    .mark_response_started(&ticket, &evidence)
                    .await?;
                let due = now.plus_millis(millis(poll_after));
                let wait = wait_id(self.agent(), draft.next_seq());
                let reason = ParkReason::AwaitingToolResult {
                    call: call.call.clone(),
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
                FailureSettlement::NotSent { stage, .. } => {
                    decide::settle_known_failure(&mut draft, effect, stage, DispatchProof::NotSent);
                    // A tool that could not be dispatched is a *result* the model decides
                    // about, not a reason to end the run: the alternative is terminalizing a
                    // whole session because one optional tool was unavailable.
                    draft.append(JournalRecord::ToolResult {
                        call: call.call.clone(),
                        blocks: vec![CanonicalBlock::Text {
                            text: error.detail.as_str().to_owned(),
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
        operation: &aex_brain_domain::ids::DetachedOperationId,
    ) -> Result<Stop, ActivationError> {
        let status = self.ports.tools.query(operation).await;
        let Some((wait, call)) = self.open_tool_wait() else {
            return Err(ActivationError::Unsupported {
                step: "detached_result_without_wait",
                owed_by: "nothing: a detached effect is always committed with the wait it parks on, so this state means the journal and the effect row disagree",
            });
        };
        let mut draft = self.draft("effecting");
        match status {
            Ok(DetachedStatus::Running { poll_after }) => {
                // Still running. The poll timer is a wake, and a wake exists only inside a
                // decision, so re-arming it is a commit rather than a queue call.
                let due = self.ports.clock.now().plus_millis(millis(poll_after));
                draft.wake(
                    self.ports.ids.wake_id(),
                    ParkReason::AwaitingToolResult { call },
                    due,
                    self.authority.workspace.to_string(),
                    self.policy.shard_for(self.agent()),
                );
                self.commit(draft).await?;
                Ok(Stop::Parked)
            }
            Ok(DetachedStatus::Completed(body)) => {
                Self::settle_detached(
                    &mut draft,
                    effect.id,
                    wait,
                    call,
                    body.blocks,
                    body.is_error,
                    body.executed_on,
                    body.duration_ms,
                    body.checksum,
                );
                self.commit(draft).await?;
                Ok(Stop::HandedBack)
            }
            Ok(DetachedStatus::Failed { reason }) => {
                let blocks = vec![CanonicalBlock::Text { text: reason }];
                let checksum = ContentHash::of(&canonicalize_value(&blocks)?);
                Self::settle_detached(
                    &mut draft,
                    effect.id,
                    wait,
                    call,
                    blocks,
                    true,
                    // A failed detached operation carries no executor on its durable record,
                    // and inventing one would attribute the failure to an executor that may
                    // never have run.
                    ExecutorRoute::BrainInline,
                    0,
                    checksum,
                );
                self.commit(draft).await?;
                Ok(Stop::HandedBack)
            }
            Ok(DetachedStatus::Unknown) | Err(_) => {
                let evidence = effect.evidence.clone().unwrap_or_else(|| {
                    DispatchEvidence::ambiguous(
                        effect.state.attempt().unwrap_or(1),
                        DispatchStage::Streaming,
                    )
                });
                decide::settle_unknown(&mut draft, effect.id, evidence);
                self.commit(draft).await?;
                Ok(Stop::Finished(FinishReason::Interrupted))
            }
        }
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
        blocks: Vec<CanonicalBlock>,
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
            blocks,
            is_error,
            executed_on,
            duration_ms,
            effect,
        });
    }

    fn open_tool_wait(&self) -> Option<(WaitId, ToolCallId)> {
        self.state
            .waits
            .iter()
            .find_map(|(wait, reason)| match reason {
                ParkReason::AwaitingToolResult { call } => Some((*wait, call.clone())),
                _ => None,
            })
    }
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
