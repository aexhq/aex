//! The runtime-control engine: the queue and schedule entry points, the
//! exact-generation fence, the true-idle suspend transition and the actively
//! enforced eight-hour lifetime.
//!
//! This is the sole ordinary `MicroVM` control role. Nothing else suspends,
//! resumes or terminates a generation, which is what makes the single suspend
//! fence sufficient and what makes a worker outage safe: it delays cost saving and
//! raises an alarm, and it can never pause an authority-open background job.
//!
//! Every decision here is taken by the pure model in `aex-runtime-control`. What
//! this module adds is the ordering: atomically take the fence and record the
//! intent, then recount, then call the provider, then settle, then emit facts. A
//! different order is what produces a snapshot of a running job or a charge with
//! no intent.

use std::sync::Arc;

use aex_hands_control_aws::lifecycle::{AwaitVerdict, await_step, poll_interval_ms};
use aex_hands_control_aws::provider::{MicrovmControlApi, MicrovmDescription};
use aex_hands_protocol::lifecycle::{ProviderRequestId, RuntimeReceipt};
use aex_hands_protocol::rpc::Fence;
use aex_internal_contracts::PricingVersion;
use aex_runtime_control::clock::millis_between;
use aex_runtime_control::clock::plus_millis;
use aex_runtime_control::generation::{GenerationHead, GenerationState, next_fence};
use aex_runtime_control::idle::IdleAssessment;
use aex_runtime_control::lifecycle::{
    IntentRecord, IntentState, LifecycleAction, LifecycleIntentId, MicrovmId, ProviderCall,
    ProviderState, RECONCILE_ATTEMPTS, ReconcileStep, client_token, snapshot_lifecycle_id,
};
use aex_runtime_control::store::{
    CommandBinding, GenerationAccountingPlan, GenerationCommit, GenerationPlan, GenerationView,
    IdleProbe, LifecycleIntentPlan, LifecycleReceiptPlan, LifecycleReconcilePlan,
    LifecycleRequestPlan, OpenEffectCounter, PageBudget, RuntimeActivityStore, RuntimeShard,
    RuntimeStoreError, UsageOutboxPlan, bind_command,
};
use aex_runtime_control::usage::{
    FactContext, SinkError, SnapshotIo, SnapshotResidence, UsageCategory, UsageFactSink,
    derive_facts,
};
use aex_usage_domain::fact::FactDraft;
use aex_usage_domain::meter::Category;
use aex_wire::ids::{GenerationId, SessionId};
use aex_wire::types::{Region, Timestamp};
use futures::{StreamExt as _, stream};
use serde::{Deserialize, Serialize};

use crate::composition::{HoldReason, SuspendDecision, evaluate_suspend, recount};
use crate::queue::{BatchItem, BatchResult, ItemOutcome, fold_batch};

/// How long a lifecycle await may take before it is treated as indeterminate.
pub const AWAIT_BUDGET_MS: u64 = 30_000;

/// Maximum independent lifecycle items evaluated concurrently in one Lambda.
///
/// A provider await may consume the full 30-second budget. Serially processing a
/// ten-record SQS batch or a fifty-item due page would exceed ordinary Lambda
/// timeouts; this bound keeps one invocation within one await window while
/// capping provider and `DynamoDB` pressure.
pub const ITEM_CONCURRENCY: usize = 32;

/// A cooperative pause between provider polls.
///
/// A port rather than a direct `tokio::time::sleep`, so a lifecycle await is
/// deterministic in test. A real clock in an await turns the timeout case into a
/// timing race, and a timing race in a lifecycle test is a test that eventually
/// passes for the wrong reason.
pub trait Pace: Send + Sync + 'static {
    /// Pauses for `millis`.
    fn sleep(&self, millis: u64) -> core::pin::Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

/// One message as the queue delivered it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueRecord {
    /// The message identity the partial-batch response names.
    pub message_id: String,
    /// How many times it has been delivered.
    pub receive_count: u32,
    /// The raw body.
    pub body: String,
}

/// The work domains this worker is the sole handler for.
///
/// `runtime.evaluate` is the due-index reaper's own redelivery, not a public work
/// domain; the two customer-visible domains are the other arms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "domain", rename_all_fields = "camelCase")]
pub enum RuntimeCommand {
    /// Evaluate one generation against the lifecycle model.
    #[serde(rename = "runtime.evaluate")]
    Evaluate {
        /// The session that owns the generation.
        session: SessionId,
        /// The generation the sender observed.
        generation: GenerationId,
    },
    /// A live workspace read asked for a retained workspace to be woken.
    #[serde(rename = "runtime.live_workspace_wake")]
    LiveWorkspaceWake {
        /// The session that owns the generation.
        session: SessionId,
        /// The generation the sender observed.
        generation: GenerationId,
    },
    /// The workspace was discarded; the generation and its snapshot go with it.
    #[serde(rename = "runtime.workspace_discard")]
    WorkspaceDiscard {
        /// The session that owns the generation.
        session: SessionId,
        /// The generation the sender observed.
        generation: GenerationId,
    },
}

impl RuntimeCommand {
    /// The work-domain identifier this command belongs to.
    #[must_use]
    pub const fn domain(self) -> &'static str {
        match self {
            Self::Evaluate { .. } => "runtime.evaluate",
            Self::LiveWorkspaceWake { .. } => "runtime.live_workspace_wake",
            Self::WorkspaceDiscard { .. } => "runtime.workspace_discard",
        }
    }

    /// The session the command acts on.
    #[must_use]
    pub const fn session(self) -> SessionId {
        match self {
            Self::Evaluate { session, .. }
            | Self::LiveWorkspaceWake { session, .. }
            | Self::WorkspaceDiscard { session, .. } => session,
        }
    }

    /// The generation the sender observed.
    #[must_use]
    pub const fn generation(self) -> GenerationId {
        match self {
            Self::Evaluate { generation, .. }
            | Self::LiveWorkspaceWake { generation, .. }
            | Self::WorkspaceDiscard { generation, .. } => generation,
        }
    }
}

/// Why one command needed nothing further.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settled {
    /// The session points at no generation at all.
    NoGeneration,
    /// The command named a generation the session has moved past.
    Superseded {
        /// What the session points at now.
        current: GenerationId,
    },
    /// The generation is terminal; terminal states are absorbing.
    AlreadyTerminal {
        /// Which terminal state.
        state: GenerationState,
    },
    /// H-LAZY: the generation holds no provider compute, so no provider call is
    /// made and nothing is charged.
    NotMaterialized,
    /// The evaluation held, and the boundary was rearmed.
    Held {
        /// Why.
        reason: HoldReason,
    },
    /// Another writer moved the head between the read and the conditional write.
    Raced,
    /// The counter disagreed with the authority. It was repaired and nothing was
    /// suspended on this pass.
    CounterRepaired {
        /// What the authority says is really open.
        authoritative_open: u32,
        /// What the head claimed.
        recorded_open: u32,
    },
    /// The generation entered its lifetime drain margin: no new admissions.
    Draining {
        /// Remaining provider lifetime.
        remaining_ms: u64,
    },
    /// The generation was suspended and its running interval was closed.
    Suspended,
    /// The generation was resumed and its snapshot residence was closed.
    Resumed,
    /// The generation was terminated.
    Terminated {
        /// Whether the customer lost continuity without asking to.
        continuity_lost: bool,
        /// Remaining provider lifetime at the moment of termination.
        remaining_ms: u64,
    },
    /// A resume was refused because too little provider lifetime remains. The
    /// session authority allocates a new generation instead.
    ResumeRefused {
        /// Remaining provider lifetime.
        remaining_ms: u64,
    },
    /// The provider is gone. `lost` is absorbing.
    Lost,
    /// A provider outcome was indeterminate. The intent is recorded `unknown` and
    /// reconciliation owns it; no second effect is dispatched.
    Reconciling,
}

/// What one command produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutcome {
    /// Nothing further is needed.
    Settled(Settled),
    /// A redrive can fix it.
    Retry {
        /// Why.
        reason: String,
    },
    /// A redrive cannot fix it. Quarantine with an operator record and an alarm.
    Poison {
        /// Why.
        reason: String,
    },
}

impl CommandOutcome {
    /// The queue outcome this maps onto.
    #[must_use]
    pub fn item_outcome(&self) -> ItemOutcome {
        match self {
            Self::Settled(_) => ItemOutcome::Succeeded,
            Self::Retry { reason } => ItemOutcome::Retryable {
                reason: reason.clone(),
            },
            Self::Poison { reason } => ItemOutcome::Poison {
                reason: reason.clone(),
            },
        }
    }
}

/// What one schedule pass over one shard did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SchedulePass {
    /// How many due generations the scan returned.
    pub scanned: u32,
    /// What each evaluation produced, in scan order.
    pub outcomes: Vec<(GenerationId, CommandOutcome)>,
    /// The cursor for the next page, absent when the shard is exhausted.
    pub cursor: Option<String>,
    /// Why the scan itself failed, when it did. A failed scan is reported rather
    /// than folded into an empty page, because an empty page and an unreadable
    /// index mean opposite things to the alarm that watches this worker.
    pub scan_failure: Option<String>,
}

/// The bindings the engine holds.
///
/// There is deliberately **no transfer sink**. Hands Internet egress is not
/// charged at launch (OD-26) and snapshot I/O is zero-dollar observability (OD-25),
/// so the worker has no reason to hold a transfer-authority binding at all. Not
/// holding one is stronger than holding one and not using it.
pub struct RuntimePorts {
    /// The runtime-activity authority.
    pub store: Arc<dyn RuntimeActivityStore>,
    /// The authoritative open-Hands-effect count.
    pub effects: Arc<dyn OpenEffectCounter>,
    /// The trusted provider control plane.
    pub provider: Arc<dyn MicrovmControlApi>,
    /// The compute-authority ingress.
    pub compute: Arc<dyn UsageFactSink>,
    /// The storage-authority ingress.
    pub storage: Arc<dyn UsageFactSink>,
    /// The await pacing port.
    pub pace: Arc<dyn Pace>,
}

impl core::fmt::Debug for RuntimePorts {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("RuntimePorts")
            .finish_non_exhaustive()
    }
}

/// The settings one deployment of the engine runs with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSettings {
    /// The region every fact is attributed to.
    pub region: Region,
    /// The rate book facts are priced against.
    pub pricing_version: PricingVersion,
    /// How many shards the due index is spread over.
    pub shards: u16,
    /// How much one due scan may read.
    pub page: PageBudget,
    /// Jitter added to an evaluation schedule, drawn by the caller.
    pub schedule_jitter_ms: u64,
}

/// Everything one settled transition lands together.
///
/// Bundled rather than passed as ten arguments, because the fields have to move
/// as one: settling the intent without landing the head, or landing the head
/// without closing the accounting interval, is exactly the partial state the
/// crash matrix has to be able to reconcile.
struct Closure<'a> {
    /// The generation being settled.
    view: &'a GenerationView,
    /// The head as it stands after the fence was taken.
    from: &'a GenerationHead,
    /// The provider `MicroVM`.
    microvm: &'a MicrovmId,
    /// The intent this settles.
    intent_id: &'a LifecycleIntentId,
    /// Provider action the intent performed.
    action: LifecycleAction,
    /// Where the head lands.
    next_state: GenerationState,
    /// What the provider was observed to be.
    observed: ProviderState,
    /// The provider request id, the only evidence an empty body carries.
    request: Option<ProviderRequestId>,
    /// The snapshot residence this transition opened or closed.
    residence: Option<SnapshotResidence>,
    /// Whether the residence is a *closed* one, and therefore chargeable.
    charge_residence: bool,
    /// Running milliseconds in the closed interval.
    running_ms: u64,
    /// Suspended milliseconds in the closed interval.
    suspended_ms: u64,
}

/// Exact identities needed to settle one bounded provider await.
struct TransitionAwait<'a> {
    view: &'a GenerationView,
    current: &'a GenerationHead,
    microvm: &'a MicrovmId,
    intent_id: &'a LifecycleIntentId,
    request: &'a ProviderRequestId,
    action: LifecycleAction,
    now: Timestamp,
}

/// The runtime-control engine.
#[derive(Debug)]
pub struct RuntimeControl {
    ports: RuntimePorts,
    settings: RuntimeSettings,
}

impl RuntimeControl {
    /// Composes the engine.
    #[must_use]
    pub const fn new(ports: RuntimePorts, settings: RuntimeSettings) -> Self {
        Self { ports, settings }
    }

    /// The settings this engine runs with.
    #[must_use]
    pub const fn settings(&self) -> &RuntimeSettings {
        &self.settings
    }

    /// The queue entry point.
    ///
    /// The response names **exactly** the identifiers that must be redriven.
    /// Reporting the whole batch would redrive the ones that already succeeded,
    /// which for a lifecycle effect means dispatching it twice.
    pub async fn handle_queue(&self, records: &[QueueRecord], now: Timestamp) -> BatchResult {
        let items = stream::iter(records)
            .map(|record| async move {
                let outcome = match serde_json::from_str::<RuntimeCommand>(&record.body) {
                    Ok(command) => self.handle_command(command, now).await,
                    Err(error) => CommandOutcome::Poison {
                        reason: format!("undecodable runtime command: {error}"),
                    },
                };
                BatchItem {
                    message_id: record.message_id.clone(),
                    receive_count: record.receive_count,
                    outcome: outcome.item_outcome(),
                }
            })
            .buffer_unordered(ITEM_CONCURRENCY)
            .collect::<Vec<_>>()
            .await;
        fold_batch(&items)
    }

    /// The schedule entry point: one bounded ordered pass over one shard of the
    /// due index.
    ///
    /// The scan is bounded and ordered, so a reaper sweep costs one query per
    /// shard rather than a table walk, and the evaluation schedule carries the
    /// jitter that the 180000 ms decision must never carry.
    pub async fn handle_schedule(&self, shard: RuntimeShard, now: Timestamp) -> SchedulePass {
        let page = match self
            .ports
            .store
            .scan_due(shard, now, self.settings.page)
            .await
        {
            Ok(page) => page,
            Err(error) => {
                return SchedulePass {
                    scan_failure: Some(format!("the due scan failed: {error}")),
                    ..SchedulePass::default()
                };
            }
        };
        let outcomes = stream::iter(&page.due)
            .map(|pointer| async move {
                let outcome = self
                    .handle_command(
                        RuntimeCommand::Evaluate {
                            session: pointer.session,
                            generation: pointer.generation,
                        },
                        now,
                    )
                    .await;
                (pointer.generation, outcome)
            })
            .buffer_unordered(ITEM_CONCURRENCY)
            .collect::<Vec<_>>()
            .await;
        SchedulePass {
            scanned: u32::try_from(page.due.len()).unwrap_or(u32::MAX),
            outcomes,
            cursor: page.cursor,
            scan_failure: None,
        }
    }

    /// Runs one command under the exact-generation fence.
    pub async fn handle_command(&self, command: RuntimeCommand, now: Timestamp) -> CommandOutcome {
        let pointer = match self
            .ports
            .store
            .load_current_generation(command.session())
            .await
        {
            Ok(pointer) => pointer,
            Err(error) => return store_outcome(&error),
        };
        let fence = match bind_command(pointer.as_ref(), command.generation()) {
            CommandBinding::Proceed { fence, .. } => fence,
            CommandBinding::NoGeneration => {
                return CommandOutcome::Settled(Settled::NoGeneration);
            }
            CommandBinding::Superseded { current } => {
                return CommandOutcome::Settled(Settled::Superseded { current });
            }
            CommandBinding::Unallocated { named, current } => {
                return CommandOutcome::Poison {
                    reason: format!(
                        "command names generation {named}, which the session authority never \
                         allocated; it points at {current}"
                    ),
                };
            }
        };
        let view = match self
            .ports
            .store
            .load_generation_view(command.generation())
            .await
        {
            Ok(Some(view)) => view,
            Ok(None) => {
                return CommandOutcome::Poison {
                    reason: format!(
                        "the session pointer names generation {} but no head exists",
                        command.generation()
                    ),
                };
            }
            Err(error) => return store_outcome(&error),
        };
        if let Err(outcome) = self.flush_usage(view.head.generation).await {
            return outcome;
        }
        if view.head.fence != fence {
            return CommandOutcome::Retry {
                reason: format!(
                    "the pointer is at fence {} and the head at fence {}; a transition is in \
                     flight",
                    fence.0, view.head.fence.0
                ),
            };
        }
        if let Some(intent) = &view.open_intent
            && !intent.permits_new_effect()
        {
            return self.reconcile_intent(&view, intent, now).await;
        }
        match command {
            RuntimeCommand::Evaluate { .. } => self.evaluate(&view, now).await,
            RuntimeCommand::LiveWorkspaceWake { .. } => self.wake(&view, now).await,
            RuntimeCommand::WorkspaceDiscard { .. } => {
                self.discard(&view, now, LifecycleAction::Terminate, false)
                    .await
            }
        }
    }

    /// Advances one unresolved lifecycle intent without ever repeating its
    /// provider effect.
    async fn reconcile_intent(
        &self,
        view: &GenerationView,
        intent: &IntentRecord,
        now: Timestamp,
    ) -> CommandOutcome {
        if intent.generation != view.head.generation || intent.fence != view.head.fence {
            return CommandOutcome::Poison {
                reason: format!(
                    "open intent {} is bound to generation {}/fence {}, but the head is {}/{}",
                    intent.intent_id,
                    intent.generation,
                    intent.fence.0,
                    view.head.generation,
                    view.head.fence.0
                ),
            };
        }
        if intent.state == IntentState::Quarantined {
            return CommandOutcome::Poison {
                reason: format!(
                    "lifecycle intent {} is quarantined after {} reconciliation attempts",
                    intent.intent_id, intent.attempts
                ),
            };
        }
        match intent.next_reconcile_step(&client_token(view.head.generation)) {
            ReconcileStep::Quarantine { attempts } => {
                self.defer_reconcile(
                    intent,
                    now,
                    format!("reconciliation budget exhausted after {attempts} attempts"),
                )
                .await
            }
            ReconcileStep::ReissueLaunch { .. } => {
                // This worker deliberately holds no signed image/run-hook plan.
                // Reissuing with a fabricated request would violate the launch
                // authority and could create a second VM if any field drifted.
                self.defer_reconcile(
                    intent,
                    now,
                    "launch reconciliation requires the original signed RunMicrovm request"
                        .to_owned(),
                )
                .await
            }
            ReconcileStep::Probe { microvm } => match self.ports.provider.get(&microvm).await {
                Ok(description) => {
                    self.reconcile_observed(view, intent, &microvm, description.state, now)
                        .await
                }
                Err(ProviderCall::NotFound) if intent.action == LifecycleAction::Terminate => {
                    self.close_reconciled(view, intent, &microvm, ProviderState::Terminated, now)
                        .await
                }
                Err(ProviderCall::NotFound) => {
                    self.mark_lost(view, &view.head, &intent.intent_id, now)
                        .await
                }
                Err(call) => {
                    self.defer_reconcile(
                        intent,
                        now,
                        format!("GetMicrovm did not resolve the intent: {call}"),
                    )
                    .await
                }
            },
        }
    }

    /// Resolves a provider observation through the same transition table as the
    /// ordinary await path.
    async fn reconcile_observed(
        &self,
        view: &GenerationView,
        intent: &IntentRecord,
        microvm: &MicrovmId,
        observed: ProviderState,
        now: Timestamp,
    ) -> CommandOutcome {
        if intent.action == LifecycleAction::Launch {
            return self
                .defer_reconcile(
                    intent,
                    now,
                    format!(
                        "launch intent observed {observed:?}, but this worker lacks the original launch receipt authority"
                    ),
                )
                .await;
        }
        match await_step(intent.action, observed, view.head.state, AWAIT_BUDGET_MS) {
            AwaitVerdict::Reached(_) => {
                if matches!(
                    intent.action,
                    LifecycleAction::Suspend | LifecycleAction::Resume
                ) && intent.provider_request_id.is_none()
                {
                    return self
                        .defer_reconcile(
                            intent,
                            now,
                            "the provider reached the target but the suspend/resume request id is absent"
                                .to_owned(),
                        )
                        .await;
                }
                self.close_reconciled(view, intent, microvm, observed, now)
                    .await
            }
            AwaitVerdict::Lost => {
                self.mark_lost(view, &view.head, &intent.intent_id, now)
                    .await
            }
            AwaitVerdict::Poll | AwaitVerdict::TimedOut => {
                self.defer_reconcile(
                    intent,
                    now,
                    format!(
                        "provider remains {observed:?} while reconciling {}",
                        intent.intent_id
                    ),
                )
                .await
            }
        }
    }

    /// Closes an intent whose exact provider state is now known.
    async fn close_reconciled(
        &self,
        view: &GenerationView,
        intent: &IntentRecord,
        microvm: &MicrovmId,
        observed: ProviderState,
        now: Timestamp,
    ) -> CommandOutcome {
        let elapsed = millis_between(view.accounted_from, now);
        let (next_state, residence, charge_residence, running_ms, suspended_ms) =
            match intent.action {
                LifecycleAction::Suspend => (
                    GenerationState::Suspended,
                    Some(SnapshotResidence {
                        lifecycle_id: snapshot_lifecycle_id(microvm, view.snapshot_ordinal),
                        generation: u64::from(view.snapshot_ordinal),
                        bytes: view.snapshot_bytes,
                        suspended_at: now,
                        released_at: now,
                        terminal: false,
                        io: SnapshotIo::default(),
                    }),
                    false,
                    elapsed,
                    0,
                ),
                LifecycleAction::Resume => {
                    let Some(suspended_at) = view.suspended_at else {
                        return CommandOutcome::Poison {
                            reason: format!(
                                "resume intent {} has no retained-snapshot start",
                                intent.intent_id
                            ),
                        };
                    };
                    (
                        GenerationState::Running,
                        Some(SnapshotResidence {
                            lifecycle_id: snapshot_lifecycle_id(microvm, view.snapshot_ordinal),
                            generation: u64::from(view.snapshot_ordinal),
                            bytes: view.snapshot_bytes,
                            suspended_at,
                            released_at: now,
                            terminal: false,
                            io: SnapshotIo::default(),
                        }),
                        true,
                        0,
                        elapsed,
                    )
                }
                LifecycleAction::Terminate => {
                    let was_suspended = view.suspended_at.is_some();
                    (
                        GenerationState::Terminated,
                        view.suspended_at.map(|suspended_at| SnapshotResidence {
                            lifecycle_id: snapshot_lifecycle_id(microvm, view.snapshot_ordinal),
                            generation: u64::from(view.snapshot_ordinal),
                            bytes: view.snapshot_bytes,
                            suspended_at,
                            released_at: now,
                            terminal: true,
                            io: SnapshotIo::default(),
                        }),
                        true,
                        if was_suspended { 0 } else { elapsed },
                        if was_suspended { elapsed } else { 0 },
                    )
                }
                LifecycleAction::Launch => unreachable!("launch is refused before settlement"),
            };
        let closure = Closure {
            view,
            from: &view.head,
            microvm,
            intent_id: &intent.intent_id,
            action: intent.action,
            next_state,
            observed,
            request: intent.provider_request_id.clone(),
            residence,
            charge_residence,
            running_ms,
            suspended_ms,
        };
        match self.close(closure, now).await {
            Ok(()) => match intent.action {
                LifecycleAction::Suspend => CommandOutcome::Settled(Settled::Suspended),
                LifecycleAction::Resume => CommandOutcome::Settled(Settled::Resumed),
                LifecycleAction::Terminate => CommandOutcome::Settled(Settled::Terminated {
                    continuity_lost: false,
                    remaining_ms: view
                        .lifetime
                        .map_or(0, |lifetime| lifetime.remaining_ms(now)),
                }),
                LifecycleAction::Launch => unreachable!("launch is refused before settlement"),
            },
            Err(outcome) => outcome,
        }
    }

    /// Persists one bounded probe attempt and yields the due index so a failed
    /// prefix cannot monopolize a shard page.
    async fn defer_reconcile(
        &self,
        intent: &IntentRecord,
        now: Timestamp,
        reason: String,
    ) -> CommandOutcome {
        let next_evaluate_at = plus_millis(now, 1);
        match self
            .ports
            .store
            .record_reconcile_attempt(&LifecycleReconcilePlan {
                intent_id: intent.intent_id.clone(),
                generation: intent.generation,
                expected_attempts: intent.attempts,
                next_evaluate_at,
                reconciled_at: now,
            })
            .await
        {
            Ok(updated) if updated.state == IntentState::Quarantined => CommandOutcome::Poison {
                reason: format!(
                    "lifecycle intent {} quarantined after {} attempts: {reason}",
                    updated.intent_id, updated.attempts
                ),
            },
            Ok(updated) => CommandOutcome::Retry {
                reason: format!(
                    "lifecycle intent {} remains unresolved after {}/{} attempts: {reason}",
                    updated.intent_id, updated.attempts, RECONCILE_ATTEMPTS
                ),
            },
            Err(error) => store_outcome(&error),
        }
    }

    /// One lifecycle evaluation.
    async fn evaluate(&self, view: &GenerationView, now: Timestamp) -> CommandOutcome {
        if view.head.state.is_terminal() {
            return CommandOutcome::Settled(Settled::AlreadyTerminal {
                state: view.head.state,
            });
        }
        if !view.permits_new_effect() {
            return CommandOutcome::Settled(Settled::Reconciling);
        }
        let Some(microvm) = view.microvm.clone() else {
            return CommandOutcome::Settled(Settled::NotMaterialized);
        };
        let Some(lifetime) = view.lifetime else {
            return CommandOutcome::Poison {
                reason: format!(
                    "generation {} holds MicroVM {microvm} but records no launch instant; the \
                     eight-hour lifetime cannot be enforced without one",
                    view.head.generation
                ),
            };
        };
        let observed = match self.probe(&microvm).await {
            Ok(description) => description.state,
            Err(outcome) => return outcome,
        };
        let assessment = IdleAssessment::from_head(&view.head, now);
        match evaluate_suspend(&view.head, &assessment, lifetime, observed, now) {
            SuspendDecision::Terminate { remaining_ms } => {
                match self
                    .discard(view, now, LifecycleAction::Terminate, true)
                    .await
                {
                    CommandOutcome::Settled(Settled::Terminated {
                        continuity_lost, ..
                    }) => CommandOutcome::Settled(Settled::Terminated {
                        continuity_lost,
                        remaining_ms,
                    }),
                    other => other,
                }
            }
            SuspendDecision::Drain { remaining_ms } => self.drain(view, now, remaining_ms).await,
            SuspendDecision::Hold { reason } => {
                if let Err(error) = self.rearm(view, &assessment, now).await {
                    return store_outcome(&error);
                }
                CommandOutcome::Settled(Settled::Held { reason })
            }
            SuspendDecision::TakeLock { next_fence, .. } => {
                self.suspend(view, &microvm, next_fence, now).await
            }
            SuspendDecision::RepairCounter {
                authoritative_open,
                recorded_open,
            } => CommandOutcome::Settled(Settled::CounterRepaired {
                authoritative_open,
                recorded_open,
            }),
        }
    }

    /// Enters the lifetime drain margin: no new admissions, open work finishes.
    async fn drain(
        &self,
        view: &GenerationView,
        now: Timestamp,
        remaining_ms: u64,
    ) -> CommandOutcome {
        if view.head.state == GenerationState::LifetimeDraining {
            return CommandOutcome::Settled(Settled::Draining { remaining_ms });
        }
        if !view.head.state.permits(GenerationState::LifetimeDraining) {
            return CommandOutcome::Settled(Settled::Draining { remaining_ms });
        }
        let plan = GenerationPlan {
            generation: view.head.generation,
            expected_state: view.head.state,
            expected_fence: view.head.fence,
            expected_revision: view.head.revision,
            next_state: GenerationState::LifetimeDraining,
            next_fence: next_fence(view.head.fence),
            microvm: view.microvm.clone(),
            transport_mode: view.head.transport_mode,
            accounting: None,
            at: now,
        };
        match self.ports.store.commit_generation(&plan).await {
            Ok(_) => CommandOutcome::Settled(Settled::Draining { remaining_ms }),
            Err(RuntimeStoreError::RevisionConflict { .. }) => {
                CommandOutcome::Settled(Settled::Raced)
            }
            Err(error) => store_outcome(&error),
        }
    }

    /// Records the intent that must exist before any provider effect.
    async fn open_intent(
        &self,
        head: &GenerationHead,
        microvm: &MicrovmId,
        action: LifecycleAction,
        next_state: GenerationState,
        fence: Fence,
        now: Timestamp,
    ) -> Result<(GenerationCommit, LifecycleIntentId), CommandOutcome> {
        let intent_id = intent_id(head.generation, action, fence);
        let plan = LifecycleIntentPlan {
            intent_id: intent_id.clone(),
            generation: head.generation,
            microvm: Some(microvm.clone()),
            action,
            fence,
            expected_state: head.state,
            expected_fence: head.fence,
            expected_revision: head.revision,
            next_state,
            dispatched_at: now,
        };
        match self.ports.store.record_intent(&plan).await {
            Ok(commit) => Ok((commit.generation, intent_id)),
            Err(error) => Err(store_outcome(&error)),
        }
    }

    /// Persists the provider's request identity before any transition await.
    async fn remember_request(
        &self,
        generation: GenerationId,
        intent_id: &LifecycleIntentId,
        request: &ProviderRequestId,
    ) -> Result<(), CommandOutcome> {
        self.ports
            .store
            .record_provider_request(&LifecycleRequestPlan {
                intent_id: intent_id.clone(),
                generation,
                provider_request_id: request.clone(),
            })
            .await
            .map(|_| ())
            .map_err(|error| store_outcome(&error))
    }

    /// Everything one settled transition has to land together.
    async fn close(&self, closure: Closure<'_>, now: Timestamp) -> Result<(), CommandOutcome> {
        let source_receipt_id = closure.request.as_ref().map_or_else(
            || {
                format!(
                    "aex-runtime:{}:{}",
                    closure.from.generation, closure.intent_id.0
                )
            },
            |request| {
                format!(
                    "lambda-microvm:{}:{}:{}",
                    closure.microvm,
                    closure.action.as_str(),
                    request.0
                )
            },
        );
        let runtime_receipt = RuntimeReceipt {
            generation: closure.from.generation,
            shape: closure.from.size,
            running_ms: closure.running_ms,
            suspended_ms: closure.suspended_ms,
            from: closure.view.accounted_from,
            to: now,
            snapshot_bytes: None,
            transmit_bytes: None,
        };
        let charged = closure
            .charge_residence
            .then_some(closure.residence.as_ref())
            .flatten();
        let drafts = self.derive_drafts(
            closure.view,
            &runtime_receipt,
            charged,
            closure.intent_id,
            &source_receipt_id,
        )?;
        let snapshot_ordinal = closure
            .view
            .snapshot_ordinal
            .checked_add(u32::from(
                closure.residence.is_some() && closure.charge_residence,
            ))
            .ok_or_else(|| CommandOutcome::Poison {
                reason: "snapshot generation ordinal overflowed".to_owned(),
            })?;
        let generation_commit = GenerationPlan {
            generation: closure.from.generation,
            expected_state: closure.from.state,
            expected_fence: closure.from.fence,
            expected_revision: closure.from.revision,
            next_state: closure.next_state,
            next_fence: closure.from.fence,
            microvm: Some(closure.microvm.clone()),
            transport_mode: closure.from.transport_mode,
            accounting: Some(GenerationAccountingPlan {
                accounted_from: now,
                suspended_at: (closure.next_state == GenerationState::Suspended).then_some(now),
                snapshot_ordinal,
            }),
            at: now,
        };
        let receipt_plan = LifecycleReceiptPlan {
            intent_id: closure.intent_id.clone(),
            generation: closure.from.generation,
            next_intent_state: IntentState::Settled,
            provider_request_id: closure.request.clone(),
            observed_state: Some(closure.observed),
            snapshot: closure.residence.clone(),
            usage: drafts
                .iter()
                .cloned()
                .map(|draft| UsageOutboxPlan {
                    category: draft.authority.category,
                    draft,
                })
                .collect(),
            generation_commit: Some(generation_commit),
            settled_at: now,
        };
        let receipt = self
            .ports
            .store
            .settle_intent(&receipt_plan)
            .await
            .map_err(|error| store_outcome(&error))?;
        if receipt.receipt_id != source_receipt_id {
            return Err(CommandOutcome::Poison {
                reason: format!(
                    "runtime receipt identity `{}` disagrees with transactional usage identity `{source_receipt_id}`",
                    receipt.receipt_id
                ),
            });
        }
        self.flush_usage(closure.from.generation).await
    }

    /// Recounts open effects after the suspend fence and closes the intent when
    /// the cached counter was stale.
    async fn recount_before_suspend(
        &self,
        view: &GenerationView,
        current: &GenerationHead,
        intent_id: &LifecycleIntentId,
        now: Timestamp,
    ) -> Result<Option<Settled>, CommandOutcome> {
        let authoritative = self
            .ports
            .effects
            .count_open_hands_effects(view.session, view.head.generation)
            .await
            .map_err(|error| store_outcome(&error))?;
        let SuspendDecision::RepairCounter {
            authoritative_open,
            recorded_open,
        } = recount(current, authoritative)
        else {
            return Ok(None);
        };
        self.settle_without_effect(view, current, intent_id, GenerationState::Running, now)
            .await?;
        Ok(Some(Settled::CounterRepaired {
            authoritative_open,
            recorded_open,
        }))
    }

    /// The suspend transition.
    async fn suspend(
        &self,
        view: &GenerationView,
        microvm: &MicrovmId,
        fence: Fence,
        now: Timestamp,
    ) -> CommandOutcome {
        // 1. Take the fence and record the blocking intent atomically. Brain can
        // no longer admit, and a crash can never leave a transitional head with
        // no durable explanation.
        let (locked, intent_id) = match self
            .open_intent(
                &view.head,
                microvm,
                LifecycleAction::Suspend,
                GenerationState::Suspending,
                fence,
                now,
            )
            .await
        {
            Ok(commit) => commit,
            Err(outcome) => return outcome,
        };

        // 2. The authoritative recount, before any provider call.
        match self
            .recount_before_suspend(view, &locked.head, &intent_id, now)
            .await
        {
            Ok(Some(settled)) => return CommandOutcome::Settled(settled),
            Ok(None) => {}
            Err(outcome) => return outcome,
        }

        // 3. Dispatch.
        let request = match self.ports.provider.suspend(microvm).await {
            Ok(request) => request,
            Err(call) => {
                return self
                    .settle_failed_dispatch(
                        view,
                        &locked.head,
                        &intent_id,
                        &call,
                        GenerationState::Running,
                        now,
                    )
                    .await;
            }
        };

        // 4. Persist the response identity before waiting. A crash after this
        // point leaves enough evidence for exact-identity reconciliation.
        if let Err(outcome) = self
            .remember_request(view.head.generation, &intent_id, &request)
            .await
        {
            return outcome;
        }

        // 5. Await SUSPENDED. Never assume success.
        if let Some(outcome) = self
            .settle_await(TransitionAwait {
                view,
                current: &locked.head,
                microvm,
                action: LifecycleAction::Suspend,
                intent_id: &intent_id,
                request: &request,
                now,
            })
            .await
        {
            return outcome;
        }

        // 6. Settle the intent, land the state, and close the running interval.
        let closure = Closure {
            view,
            from: &locked.head,
            microvm,
            intent_id: &intent_id,
            action: LifecycleAction::Suspend,
            next_state: GenerationState::Suspended,
            observed: ProviderState::Suspended,
            request: Some(request),
            residence: Some(SnapshotResidence {
                lifecycle_id: snapshot_lifecycle_id(microvm, view.snapshot_ordinal),
                generation: u64::from(view.snapshot_ordinal),
                bytes: view.snapshot_bytes,
                suspended_at: now,
                released_at: now,
                terminal: false,
                io: SnapshotIo::default(),
            }),
            charge_residence: false,
            running_ms: millis_between(view.accounted_from, now),
            suspended_ms: 0,
        };
        match self.close(closure, now).await {
            Ok(()) => CommandOutcome::Settled(Settled::Suspended),
            Err(outcome) => outcome,
        }
    }

    /// Whether a wake may resume this generation, and the `MicroVM` it would act on.
    fn resume_guard(
        view: &GenerationView,
        now: Timestamp,
    ) -> Result<(MicrovmId, Timestamp), CommandOutcome> {
        if view.head.state.is_terminal() {
            return Err(CommandOutcome::Settled(Settled::AlreadyTerminal {
                state: view.head.state,
            }));
        }
        if !view.permits_new_effect() {
            return Err(CommandOutcome::Settled(Settled::Reconciling));
        }
        let Some(microvm) = view.microvm.clone() else {
            // H-LAZY: materialization collapses through `aex_brain_hands`, which
            // owns the three-layer single flight. The worker never launches, so it
            // cannot become a second launch path racing the first.
            return Err(CommandOutcome::Settled(Settled::NotMaterialized));
        };
        if view.head.state == GenerationState::Running {
            return Err(CommandOutcome::Settled(Settled::Resumed));
        }
        if view.head.state != GenerationState::Suspended {
            return Err(CommandOutcome::Retry {
                reason: format!(
                    "generation {} is {:?}; a wake resumes only a suspended generation",
                    view.head.generation, view.head.state
                ),
            });
        }
        let (Some(lifetime), Some(suspended_at)) = (view.lifetime, view.suspended_at) else {
            return Err(CommandOutcome::Poison {
                reason: format!(
                    "generation {} is suspended but records no launch or suspension instant; \
                     neither the lifetime nor the retained snapshot interval can be closed",
                    view.head.generation
                ),
            });
        };
        // A resume is refused near expiry; the session authority allocates a new
        // generation rather than reviving one about to be hard-stopped.
        lifetime
            .permit_resume(now)
            .map(|_| (microvm, suspended_at))
            .map_err(|refused| {
                CommandOutcome::Settled(Settled::ResumeRefused {
                    remaining_ms: refused.remaining_ms,
                })
            })
    }

    /// The resume transition, driven by `runtime.live_workspace_wake`.
    async fn wake(&self, view: &GenerationView, now: Timestamp) -> CommandOutcome {
        let (microvm, suspended_at) = match Self::resume_guard(view, now) {
            Ok(resolved) => resolved,
            Err(outcome) => return outcome,
        };
        let fence = next_fence(view.head.fence);
        let (moved, intent_id) = match self
            .open_intent(
                &view.head,
                &microvm,
                LifecycleAction::Resume,
                GenerationState::Resuming,
                fence,
                now,
            )
            .await
        {
            Ok(commit) => commit,
            Err(outcome) => return outcome,
        };
        let request = match self.ports.provider.resume(&microvm).await {
            Ok(request) => request,
            Err(call) => {
                return self
                    .settle_failed_dispatch(
                        view,
                        &moved.head,
                        &intent_id,
                        &call,
                        GenerationState::Suspended,
                        now,
                    )
                    .await;
            }
        };
        if let Err(outcome) = self
            .remember_request(view.head.generation, &intent_id, &request)
            .await
        {
            return outcome;
        }
        if let Some(outcome) = self
            .settle_await(TransitionAwait {
                view,
                current: &moved.head,
                microvm: &microvm,
                action: LifecycleAction::Resume,
                intent_id: &intent_id,
                request: &request,
                now,
            })
            .await
        {
            return outcome;
        }
        let closure = Closure {
            view,
            from: &moved.head,
            microvm: &microvm,
            intent_id: &intent_id,
            action: LifecycleAction::Resume,
            next_state: GenerationState::Running,
            observed: ProviderState::Running,
            request: Some(request),
            residence: Some(SnapshotResidence {
                lifecycle_id: snapshot_lifecycle_id(&microvm, view.snapshot_ordinal),
                generation: u64::from(view.snapshot_ordinal),
                bytes: view.snapshot_bytes,
                suspended_at,
                released_at: now,
                terminal: false,
                io: SnapshotIo::default(),
            }),
            charge_residence: true,
            running_ms: 0,
            suspended_ms: millis_between(view.accounted_from, now),
        };
        match self.close(closure, now).await {
            Ok(()) => CommandOutcome::Settled(Settled::Resumed),
            Err(outcome) => outcome,
        }
    }

    /// Terminates a generation and closes its receipts.
    async fn discard(
        &self,
        view: &GenerationView,
        now: Timestamp,
        action: LifecycleAction,
        continuity_lost: bool,
    ) -> CommandOutcome {
        if view.head.state.is_terminal() {
            return CommandOutcome::Settled(Settled::AlreadyTerminal {
                state: view.head.state,
            });
        }
        if !view.permits_new_effect() {
            return CommandOutcome::Settled(Settled::Reconciling);
        }
        let Some(microvm) = view.microvm.clone() else {
            return CommandOutcome::Settled(Settled::NotMaterialized);
        };
        let was_suspended = view.head.state == GenerationState::Suspended;
        if was_suspended && view.suspended_at.is_none() {
            return CommandOutcome::Poison {
                reason: format!(
                    "generation {} is suspended but records no suspension instant",
                    view.head.generation
                ),
            };
        }
        let fence = next_fence(view.head.fence);
        let (moved, intent_id) = match self
            .open_intent(
                &view.head,
                &microvm,
                action,
                GenerationState::Terminating,
                fence,
                now,
            )
            .await
        {
            Ok(commit) => commit,
            Err(outcome) => return outcome,
        };
        let request = match self.ports.provider.terminate(&microvm).await {
            // A terminate is done when the provider says so, and `NotFound`
            // counts as done: there is nothing left to terminate.
            Err(ProviderCall::NotFound) => None,
            Ok(request) => Some(request),
            Err(call) => {
                return self
                    .settle_failed_dispatch(
                        view,
                        &moved.head,
                        &intent_id,
                        &call,
                        view.head.state,
                        now,
                    )
                    .await;
            }
        };
        if let Some(request) = &request
            && let Err(outcome) = self
                .remember_request(view.head.generation, &intent_id, request)
                .await
        {
            return outcome;
        }
        let elapsed = millis_between(view.accounted_from, now);
        let closure = Closure {
            view,
            from: &moved.head,
            microvm: &microvm,
            intent_id: &intent_id,
            action,
            next_state: GenerationState::Terminated,
            observed: ProviderState::Terminated,
            request,
            residence: view
                .suspended_at
                .filter(|_| was_suspended)
                .map(|suspended_at| SnapshotResidence {
                    lifecycle_id: snapshot_lifecycle_id(&microvm, view.snapshot_ordinal),
                    generation: u64::from(view.snapshot_ordinal),
                    bytes: view.snapshot_bytes,
                    suspended_at,
                    released_at: now,
                    terminal: true,
                    io: SnapshotIo::default(),
                }),
            charge_residence: true,
            running_ms: if was_suspended { 0 } else { elapsed },
            suspended_ms: if was_suspended { elapsed } else { 0 },
        };
        match self.close(closure, now).await {
            Ok(()) => CommandOutcome::Settled(Settled::Terminated {
                continuity_lost,
                remaining_ms: view
                    .lifetime
                    .map_or(0, |lifetime| lifetime.remaining_ms(now)),
            }),
            Err(outcome) => outcome,
        }
    }

    /// Awaits a dispatched transition, returning the outcome when it did not reach
    /// its target and `None` when it did.
    async fn settle_await(&self, transition: TransitionAwait<'_>) -> Option<CommandOutcome> {
        let TransitionAwait {
            view,
            current,
            microvm,
            intent_id,
            request,
            action,
            now,
        } = transition;
        match self
            .await_transition(microvm, action, view.head.state)
            .await
        {
            Ok(AwaitVerdict::Reached(_)) => None,
            Ok(AwaitVerdict::Lost) => Some(self.mark_lost(view, current, intent_id, now).await),
            Ok(AwaitVerdict::TimedOut | AwaitVerdict::Poll) => Some(
                self.mark_unknown(view, current, intent_id, Some(request.clone()), now)
                    .await,
            ),
            Err(outcome) => Some(outcome),
        }
    }

    /// One `GetMicrovm`, classified.
    async fn probe(&self, microvm: &MicrovmId) -> Result<MicrovmDescription, CommandOutcome> {
        match self.ports.provider.get(microvm).await {
            Ok(description) => Ok(description),
            Err(call) => Err(provider_outcome(&call, "GetMicrovm")),
        }
    }

    /// Polls the provider until the transition settles or the budget runs out.
    async fn await_transition(
        &self,
        microvm: &MicrovmId,
        action: LifecycleAction,
        recorded: GenerationState,
    ) -> Result<AwaitVerdict, CommandOutcome> {
        let interval = poll_interval_ms(action);
        let mut elapsed = 0_u64;
        loop {
            let observed = self.probe(microvm).await;
            let observed = match observed {
                Ok(description) => description.state,
                // A `NotFound` while awaiting a terminate is the terminate
                // succeeding; anywhere else it is the generation being lost.
                Err(CommandOutcome::Settled(Settled::Lost)) => {
                    return Ok(if action == LifecycleAction::Terminate {
                        AwaitVerdict::Reached(GenerationState::Terminated)
                    } else {
                        AwaitVerdict::Lost
                    });
                }
                Err(outcome) => return Err(outcome),
            };
            match await_step(action, observed, recorded, elapsed) {
                AwaitVerdict::Poll => {
                    if elapsed >= AWAIT_BUDGET_MS {
                        return Ok(AwaitVerdict::TimedOut);
                    }
                    self.ports.pace.sleep(interval).await;
                    elapsed = elapsed.saturating_add(interval);
                }
                settled => return Ok(settled),
            }
        }
    }

    /// Rearms the evaluation schedule without changing anything else.
    async fn rearm(
        &self,
        view: &GenerationView,
        assessment: &IdleAssessment,
        _now: Timestamp,
    ) -> Result<(), RuntimeStoreError> {
        let probe = IdleProbe {
            generation: view.head.generation,
            assessment: assessment.clone(),
            next_evaluate_at: assessment.next_evaluate_at(self.settings.schedule_jitter_ms),
            expected_revision: view.head.revision,
        };
        self.ports.store.record_probe(&probe).await
    }

    /// Settles an intent whose dispatch the provider refused outright.
    ///
    /// A refused dispatch had no effect, so the head goes back where it was and the
    /// intent settles rather than blocking every later effect behind a `dispatched`
    /// record nothing will ever close.
    async fn settle_failed_dispatch(
        &self,
        view: &GenerationView,
        current: &GenerationHead,
        intent_id: &LifecycleIntentId,
        call: &ProviderCall,
        restore_to: GenerationState,
        now: Timestamp,
    ) -> CommandOutcome {
        if let ProviderCall::Unknown { request } = call {
            return self
                .mark_unknown(view, current, intent_id, request.clone(), now)
                .await;
        }
        if matches!(call, ProviderCall::NotFound) {
            return self.mark_lost(view, current, intent_id, now).await;
        }
        if let Err(outcome) = self
            .settle_without_effect(view, current, intent_id, restore_to, now)
            .await
        {
            return outcome;
        }
        provider_outcome(call, "the lifecycle dispatch")
    }

    /// Settles an intent proven to have produced no provider effect.
    ///
    /// The restore is conditional on the head as it stands **after** the fence
    /// was taken, not on the caller's stale read. It is committed with the
    /// receipt so a lost response cannot leave a settled intent in a
    /// transitional generation.
    async fn settle_without_effect(
        &self,
        view: &GenerationView,
        current: &GenerationHead,
        intent_id: &LifecycleIntentId,
        restore_to: GenerationState,
        now: Timestamp,
    ) -> Result<(), CommandOutcome> {
        let restore = GenerationPlan {
            generation: current.generation,
            expected_state: current.state,
            expected_fence: current.fence,
            expected_revision: current.revision,
            next_state: restore_to,
            next_fence: current.fence,
            microvm: view.microvm.clone(),
            transport_mode: current.transport_mode,
            accounting: None,
            at: now,
        };
        let plan = LifecycleReceiptPlan {
            intent_id: intent_id.clone(),
            generation: view.head.generation,
            next_intent_state: IntentState::Settled,
            provider_request_id: None,
            observed_state: None,
            snapshot: None,
            usage: Vec::new(),
            generation_commit: Some(restore),
            settled_at: now,
        };
        self.ports
            .store
            .settle_intent(&plan)
            .await
            .map(|_| ())
            .map_err(|error| store_outcome(&error))
    }

    /// Records an indeterminate outcome. No second effect is dispatched.
    async fn mark_unknown(
        &self,
        view: &GenerationView,
        current: &GenerationHead,
        intent_id: &LifecycleIntentId,
        request: Option<ProviderRequestId>,
        now: Timestamp,
    ) -> CommandOutcome {
        let generation_commit = GenerationPlan {
            generation: current.generation,
            expected_state: current.state,
            expected_fence: current.fence,
            expected_revision: current.revision,
            next_state: GenerationState::Unknown,
            next_fence: current.fence,
            microvm: view.microvm.clone(),
            transport_mode: current.transport_mode,
            accounting: None,
            at: now,
        };
        let plan = LifecycleReceiptPlan {
            intent_id: intent_id.clone(),
            generation: view.head.generation,
            next_intent_state: IntentState::Unknown,
            provider_request_id: request,
            observed_state: None,
            snapshot: None,
            usage: Vec::new(),
            generation_commit: Some(generation_commit),
            settled_at: now,
        };
        if let Err(error) = self.ports.store.settle_intent(&plan).await {
            return store_outcome(&error);
        }
        CommandOutcome::Settled(Settled::Reconciling)
    }

    /// Records that the provider no longer has the generation. `lost` is absorbing.
    async fn mark_lost(
        &self,
        view: &GenerationView,
        current: &GenerationHead,
        intent_id: &LifecycleIntentId,
        now: Timestamp,
    ) -> CommandOutcome {
        let generation_commit = GenerationPlan {
            generation: current.generation,
            expected_state: current.state,
            expected_fence: current.fence,
            expected_revision: current.revision,
            next_state: GenerationState::Lost,
            next_fence: current.fence,
            microvm: view.microvm.clone(),
            transport_mode: current.transport_mode,
            accounting: None,
            at: now,
        };
        let plan = LifecycleReceiptPlan {
            intent_id: intent_id.clone(),
            generation: view.head.generation,
            next_intent_state: IntentState::Settled,
            provider_request_id: None,
            observed_state: Some(ProviderState::Terminated),
            snapshot: None,
            usage: Vec::new(),
            generation_commit: Some(generation_commit),
            settled_at: now,
        };
        if let Err(error) = self.ports.store.settle_intent(&plan).await {
            return store_outcome(&error);
        }
        CommandOutcome::Settled(Settled::Lost)
    }

    /// Derives the canonical drafts persisted in the lifecycle transaction.
    fn derive_drafts(
        &self,
        view: &GenerationView,
        receipt: &RuntimeReceipt,
        residence: Option<&SnapshotResidence>,
        intent_id: &LifecycleIntentId,
        source_receipt_id: &str,
    ) -> Result<Vec<FactDraft>, CommandOutcome> {
        let context = FactContext {
            organization: view.organization,
            workspace: view.workspace,
            region: self.settings.region,
            session: view.session,
            pricing_version: self.settings.pricing_version.clone(),
            intent: intent_id.clone(),
            source_receipt_id: source_receipt_id.to_owned(),
        };
        derive_facts(receipt, residence, &context).map_err(|error| CommandOutcome::Poison {
            reason: format!("the runtime receipt does not account for its own interval: {error}"),
        })
    }

    /// Delivers and acknowledges the generation's transactional usage outbox.
    async fn flush_usage(&self, generation: GenerationId) -> Result<(), CommandOutcome> {
        let pending = self
            .ports
            .store
            .load_usage_outbox(generation)
            .await
            .map_err(|error| store_outcome(&error))?;
        for entry in pending {
            let category = entry.category;
            let sink = self
                .sink_for(category)
                .ok_or_else(|| CommandOutcome::Poison {
                    reason: format!(
                        "meter {} routes to the {category:?} authority, which this worker holds no \
                     binding for",
                        entry
                            .draft
                            .kind
                            .meter()
                            .map_or("correction", |meter| meter.id())
                    ),
                })?;
            sink.emit(category, entry.draft.clone())
                .await
                .map_err(|error| match error {
                    SinkError::Refused { .. } => CommandOutcome::Poison {
                        reason: error.to_string(),
                    },
                    SinkError::Unavailable { .. } => CommandOutcome::Retry {
                        reason: error.to_string(),
                    },
                })?;
            self.ports
                .store
                .mark_usage_emitted(generation, &entry.draft)
                .await
                .map_err(|error| store_outcome(&error))?;
        }
        Ok(())
    }

    /// The ingress a category's facts go to.
    ///
    /// `Transfer` deliberately resolves to nothing: the worker holds no
    /// transfer-authority binding, so a transfer fact cannot be written even by
    /// accident.
    fn sink_for(&self, category: UsageCategory) -> Option<&Arc<dyn UsageFactSink>> {
        match category {
            Category::Compute => Some(&self.ports.compute),
            Category::Storage => Some(&self.ports.storage),
            Category::Transfer => None,
        }
    }
}

/// The deterministic lifecycle intent identity.
///
/// Derived from the generation, the action and the fence, so a redelivered command
/// mints the same intent and the store's conditional write — not a timer — is what
/// stops a second effect.
#[must_use]
pub fn intent_id(
    generation: GenerationId,
    action: LifecycleAction,
    fence: Fence,
) -> LifecycleIntentId {
    LifecycleIntentId(format!("{generation}:{}:{}", action.as_str(), fence.0))
}

/// How a store failure is reported.
fn store_outcome(error: &RuntimeStoreError) -> CommandOutcome {
    match error {
        RuntimeStoreError::Unavailable { .. }
        | RuntimeStoreError::RevisionConflict { .. }
        | RuntimeStoreError::ReconcileConflict { .. } => CommandOutcome::Retry {
            reason: error.to_string(),
        },
        RuntimeStoreError::IntentOpen { .. } => CommandOutcome::Settled(Settled::Reconciling),
        RuntimeStoreError::Malformed { .. } | RuntimeStoreError::NoSuchGeneration { .. } => {
            CommandOutcome::Poison {
                reason: error.to_string(),
            }
        }
    }
}

/// How a provider answer is reported.
fn provider_outcome(call: &ProviderCall, what: &str) -> CommandOutcome {
    match call {
        ProviderCall::Throttled { retry_after } => CommandOutcome::Retry {
            reason: format!("{what} was throttled; retry after {retry_after:?}"),
        },
        ProviderCall::Capacity { quota } => CommandOutcome::Retry {
            reason: format!("{what} hit provider quota {quota}; take the long durable backoff"),
        },
        ProviderCall::Transient { class } => CommandOutcome::Retry {
            reason: format!("{what} failed transiently ({class:?})"),
        },
        ProviderCall::NotFound => CommandOutcome::Settled(Settled::Lost),
        ProviderCall::Unknown { .. } => CommandOutcome::Settled(Settled::Reconciling),
        ProviderCall::Invalid { field, detail } => CommandOutcome::Poison {
            reason: format!("{what} was rejected on `{field}`: {detail}"),
        },
        ProviderCall::Fatal { code } => CommandOutcome::Poison {
            reason: format!("{what} failed fatally: {code}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CommandOutcome, Pace, QueueRecord, RuntimeCommand, RuntimeControl, RuntimePorts,
        RuntimeSettings, Settled,
    };
    use crate::composition::HoldReason;
    use aex_hands_control_aws::provider::{
        EndpointToken, MicrovmControlApi, MicrovmDescription, MicrovmPage, ProviderFuture,
        RunRequest,
    };
    use aex_hands_protocol::lifecycle::ProviderRequestId;
    use aex_hands_protocol::rpc::Fence;
    use aex_internal_contracts::PricingVersion;
    use aex_runtime_control::clock::minus_millis;
    use aex_runtime_control::generation::{
        GenerationHead, GenerationState, ImageIdentifier, Revision,
    };
    use aex_runtime_control::idle::TRUE_IDLE_THRESHOLD_MS;
    use aex_runtime_control::lifecycle::{
        IntentRecord, IntentState, LIFETIME_DRAIN_MARGIN_MS, LIFETIME_TERMINATE_MARGIN_MS,
        LifecycleAction, Lifetime, MicrovmId, ProviderCall, ProviderState, TransientClass,
    };
    use aex_runtime_control::store::{
        GenerationCommit, GenerationPlan, GenerationPointer, GenerationView, IdleProbe,
        LifecycleIntentCommit, LifecycleIntentPlan, LifecycleReceipt, LifecycleReceiptPlan,
        LifecycleReconcilePlan, LifecycleRequestPlan, OpenEffectCounter, PageBudget,
        RuntimeActivityStore, RuntimeDuePage, RuntimeShard, RuntimeStoreError, StoreFuture,
        UsageOutboxEntry,
    };
    use aex_runtime_control::usage::{SinkError, UsageCategory, UsageFactSink};
    use aex_usage_domain::fact::{FactDraft, FactKind};
    use aex_usage_domain::meter::{Category, Meter};
    use aex_wire::ids::{
        GenerationId, OrganizationId, PrefixedId as _, SessionId, Uuid7, WorkspaceId,
    };
    use aex_wire::types::{ComputeSize, Region, Timestamp};
    use std::sync::{Arc, Mutex};

    const LAUNCHED_AT: i64 = 1_000_000;
    const BUSY_AT: i64 = 1_000_000;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("a bounded instant")
    }

    fn generation() -> GenerationId {
        GenerationId::from_uuid7(Uuid7::compose(10, [1; 10]))
    }

    fn session() -> SessionId {
        SessionId::from_uuid7(Uuid7::compose(1, [2; 10]))
    }

    fn microvm() -> MicrovmId {
        MicrovmId("mvm-1".to_owned())
    }

    fn head(state: GenerationState, open: u32) -> GenerationHead {
        GenerationHead {
            generation: generation(),
            size: ComputeSize::Gb1,
            state,
            fence: Fence(3),
            revision: Revision::new(11),
            open_operations: open,
            last_busy_at: at(BUSY_AT),
            idle_since: None,
            suspend_lock_expires_at: None,
            keepalive_lease: None,
            transport_mode: None,
        }
    }

    fn view(state: GenerationState, open: u32) -> GenerationView {
        GenerationView {
            head: head(state, open),
            session: session(),
            workspace: WorkspaceId::from_uuid7(Uuid7::compose(3, [3; 10])),
            organization: OrganizationId::from_uuid7(Uuid7::compose(2, [2; 10])),
            microvm: Some(microvm()),
            lifetime: Some(Lifetime {
                launched_at: at(LAUNCHED_AT),
            }),
            accounted_from: at(LAUNCHED_AT),
            open_intent: None,
            suspended_at: None,
            snapshot_ordinal: 0,
            snapshot_bytes: 445_644_800,
        }
    }

    fn pointer() -> GenerationPointer {
        GenerationPointer {
            session: session(),
            generation: generation(),
            fence: Fence(3),
            revision: Revision::new(11),
        }
    }

    #[derive(Default)]
    struct StoreState {
        pointer: Option<GenerationPointer>,
        view: Option<GenerationView>,
        due: Vec<GenerationPointer>,
        cursor: Option<String>,
        commits: Vec<GenerationPlan>,
        intents: Vec<LifecycleIntentPlan>,
        receipts: Vec<LifecycleReceiptPlan>,
        outbox: Vec<UsageOutboxEntry>,
        probes: Vec<IdleProbe>,
        scan_failure: Option<RuntimeStoreError>,
        lose_next_settle_response: bool,
    }

    #[derive(Default)]
    struct FakeStore {
        state: Mutex<StoreState>,
    }

    impl FakeStore {
        fn with(view: GenerationView) -> Arc<Self> {
            Arc::new(Self {
                state: Mutex::new(StoreState {
                    pointer: Some(pointer()),
                    view: Some(view),
                    ..StoreState::default()
                }),
            })
        }

        fn lock(&self) -> std::sync::MutexGuard<'_, StoreState> {
            self.state.lock().expect("the fixture lock is not poisoned")
        }
    }

    impl RuntimeActivityStore for FakeStore {
        fn load_current_generation(
            &self,
            _session: SessionId,
        ) -> StoreFuture<'_, Option<GenerationPointer>> {
            let pointer = self.lock().pointer;
            Box::pin(async move { Ok(pointer) })
        }

        fn load_generation_view(
            &self,
            _generation: GenerationId,
        ) -> StoreFuture<'_, Option<GenerationView>> {
            let view = self.lock().view.clone();
            Box::pin(async move { Ok(view) })
        }

        fn commit_generation<'a>(
            &'a self,
            plan: &'a GenerationPlan,
        ) -> StoreFuture<'a, GenerationCommit> {
            let mut state = self.lock();
            let mut head = state
                .view
                .as_ref()
                .expect("the fixture has a view")
                .head
                .clone();
            if head.revision != plan.expected_revision {
                let found = Some(head.revision);
                let expected = plan.expected_revision;
                drop(state);
                return Box::pin(async move {
                    Err(RuntimeStoreError::RevisionConflict { expected, found })
                });
            }
            head.state = plan.next_state;
            head.fence = plan.next_fence;
            head.revision = head.revision.next();
            if let Some(pointer) = state.pointer.as_mut() {
                pointer.fence = head.fence;
                pointer.revision = head.revision;
            }
            state.commits.push(plan.clone());
            if let Some(view) = state.view.as_mut() {
                view.head = head.clone();
                if let Some(accounting) = plan.accounting {
                    view.accounted_from = accounting.accounted_from;
                    view.suspended_at = accounting.suspended_at;
                    view.snapshot_ordinal = accounting.snapshot_ordinal;
                }
            }
            let revision = head.revision;
            drop(state);
            Box::pin(async move { Ok(GenerationCommit { head, revision }) })
        }

        fn record_intent<'a>(
            &'a self,
            plan: &'a LifecycleIntentPlan,
        ) -> StoreFuture<'a, LifecycleIntentCommit> {
            let mut state = self.lock();
            let record = IntentRecord {
                intent_id: plan.intent_id.clone(),
                generation: plan.generation,
                microvm: plan.microvm.clone(),
                action: plan.action,
                fence: plan.fence,
                state: IntentState::Dispatched,
                provider_request_id: None,
                attempts: 0,
                dispatched_at: plan.dispatched_at,
            };
            let mut head = state
                .view
                .as_ref()
                .expect("the fixture has a view")
                .head
                .clone();
            if head.state != plan.expected_state
                || head.fence != plan.expected_fence
                || head.revision != plan.expected_revision
            {
                let found = Some(head.revision);
                let expected = plan.expected_revision;
                drop(state);
                return Box::pin(async move {
                    Err(RuntimeStoreError::RevisionConflict { expected, found })
                });
            }
            head.state = plan.next_state;
            head.fence = plan.fence;
            head.revision = head.revision.next();
            state.intents.push(plan.clone());
            state.commits.push(GenerationPlan {
                generation: plan.generation,
                expected_state: plan.expected_state,
                expected_fence: plan.expected_fence,
                expected_revision: plan.expected_revision,
                next_state: plan.next_state,
                next_fence: plan.fence,
                microvm: plan.microvm.clone(),
                transport_mode: head.transport_mode,
                accounting: None,
                at: plan.dispatched_at,
            });
            if let Some(pointer) = state.pointer.as_mut() {
                pointer.fence = head.fence;
                pointer.revision = head.revision;
            }
            if let Some(view) = state.view.as_mut() {
                view.head = head.clone();
                view.open_intent = Some(record.clone());
            }
            let revision = head.revision;
            drop(state);
            Box::pin(async move {
                Ok(LifecycleIntentCommit {
                    generation: GenerationCommit { head, revision },
                    intent: record,
                })
            })
        }

        fn record_provider_request<'a>(
            &'a self,
            plan: &'a LifecycleRequestPlan,
        ) -> StoreFuture<'a, IntentRecord> {
            let mut state = self.lock();
            let intent = state
                .view
                .as_mut()
                .and_then(|view| view.open_intent.as_mut())
                .expect("the fixture has an open intent");
            if intent.intent_id != plan.intent_id {
                let open = intent.clone();
                drop(state);
                return Box::pin(async move {
                    Err(RuntimeStoreError::IntentOpen {
                        intent_id: open.intent_id,
                        state: open.state,
                    })
                });
            }
            intent.provider_request_id = Some(plan.provider_request_id.clone());
            let intent = intent.clone();
            drop(state);
            Box::pin(async move { Ok(intent) })
        }

        fn record_reconcile_attempt<'a>(
            &'a self,
            plan: &'a LifecycleReconcilePlan,
        ) -> StoreFuture<'a, IntentRecord> {
            let mut state = self.lock();
            let intent = state
                .view
                .as_mut()
                .and_then(|view| view.open_intent.as_mut())
                .expect("the fixture has an open intent");
            if intent.attempts != plan.expected_attempts {
                let found = intent.attempts;
                drop(state);
                return Box::pin(async move {
                    Err(RuntimeStoreError::ReconcileConflict {
                        expected: plan.expected_attempts,
                        found,
                    })
                });
            }
            intent.attempts = intent
                .attempts
                .saturating_add(1)
                .min(aex_runtime_control::lifecycle::RECONCILE_ATTEMPTS);
            if intent.attempts >= aex_runtime_control::lifecycle::RECONCILE_ATTEMPTS {
                intent.state = IntentState::Quarantined;
            }
            let intent = intent.clone();
            drop(state);
            Box::pin(async move { Ok(intent) })
        }

        fn settle_intent<'a>(
            &'a self,
            plan: &'a LifecycleReceiptPlan,
        ) -> StoreFuture<'a, LifecycleReceipt> {
            let mut state = self.lock();
            if let Some(generation) = &plan.generation_commit {
                let head = &state.view.as_ref().expect("the fixture has a view").head;
                if head.state != generation.expected_state
                    || head.fence != generation.expected_fence
                    || head.revision != generation.expected_revision
                {
                    let expected = generation.expected_revision;
                    let found = Some(head.revision);
                    drop(state);
                    return Box::pin(async move {
                        Err(RuntimeStoreError::RevisionConflict { expected, found })
                    });
                }
            }
            state.receipts.push(plan.clone());
            state
                .outbox
                .extend(plan.usage.iter().cloned().map(|pending| UsageOutboxEntry {
                    generation: plan.generation,
                    category: pending.category,
                    draft: pending.draft,
                }));
            if let Some(generation) = &plan.generation_commit {
                let mut head = state
                    .view
                    .as_ref()
                    .expect("the fixture has a view")
                    .head
                    .clone();
                head.state = generation.next_state;
                head.fence = generation.next_fence;
                head.revision = head.revision.next();
                state.commits.push(generation.clone());
                if let Some(pointer) = state.pointer.as_mut() {
                    pointer.fence = head.fence;
                    pointer.revision = head.revision;
                }
                if let Some(view) = state.view.as_mut() {
                    view.head = head;
                    if let Some(accounting) = generation.accounting {
                        view.accounted_from = accounting.accounted_from;
                        view.suspended_at = accounting.suspended_at;
                        view.snapshot_ordinal = accounting.snapshot_ordinal;
                    }
                    if plan.next_intent_state == IntentState::Unknown {
                        if let Some(intent) = view.open_intent.as_mut() {
                            intent.state = IntentState::Unknown;
                            intent.provider_request_id = plan.provider_request_id.clone();
                        }
                    } else {
                        view.open_intent = None;
                    }
                }
            }
            let lose_response = state.lose_next_settle_response;
            state.lose_next_settle_response = false;
            drop(state);
            if lose_response {
                return Box::pin(async move {
                    Err(RuntimeStoreError::Unavailable {
                        reason: "the committed settlement response was lost".to_owned(),
                    })
                });
            }
            let action = if plan.intent_id.0.contains(":resume:") {
                LifecycleAction::Resume
            } else if plan.intent_id.0.contains(":terminate:") {
                LifecycleAction::Terminate
            } else {
                LifecycleAction::Suspend
            };
            let receipt_id = plan.provider_request_id.as_ref().map_or_else(
                || format!("aex-runtime:{}:{}", plan.generation, plan.intent_id.0),
                |request| format!("lambda-microvm:mvm-1:{}:{}", action.as_str(), request.0),
            );
            let receipt = LifecycleReceipt {
                intent: IntentRecord {
                    intent_id: plan.intent_id.clone(),
                    generation: plan.generation,
                    microvm: Some(microvm()),
                    action,
                    fence: Fence(4),
                    state: plan.next_intent_state,
                    provider_request_id: plan.provider_request_id.clone(),
                    attempts: 0,
                    dispatched_at: plan.settled_at,
                },
                receipt_id,
                snapshot: plan.snapshot.clone(),
            };
            Box::pin(async move { Ok(receipt) })
        }

        fn load_usage_outbox(
            &self,
            _generation: GenerationId,
        ) -> StoreFuture<'_, Vec<UsageOutboxEntry>> {
            let outbox = self.lock().outbox.clone();
            Box::pin(async move { Ok(outbox) })
        }

        fn mark_usage_emitted<'a>(
            &'a self,
            _generation: GenerationId,
            draft: &'a FactDraft,
        ) -> StoreFuture<'a, ()> {
            let fact_id = draft.fact_id();
            self.lock()
                .outbox
                .retain(|entry| entry.draft.fact_id() != fact_id);
            Box::pin(async move { Ok(()) })
        }

        fn record_probe<'a>(&'a self, probe: &'a IdleProbe) -> StoreFuture<'a, ()> {
            self.lock().probes.push(probe.clone());
            Box::pin(async move { Ok(()) })
        }

        fn scan_due(
            &self,
            _shard: RuntimeShard,
            _now: Timestamp,
            _budget: PageBudget,
        ) -> StoreFuture<'_, RuntimeDuePage> {
            let mut state = self.lock();
            if let Some(failure) = state.scan_failure.clone() {
                drop(state);
                return Box::pin(async move { Err(failure) });
            }
            let page = RuntimeDuePage {
                due: core::mem::take(&mut state.due),
                cursor: state.cursor.clone(),
            };
            drop(state);
            Box::pin(async move { Ok(page) })
        }
    }

    struct FakeEffects {
        open: u32,
    }

    impl OpenEffectCounter for FakeEffects {
        fn count_open_hands_effects(
            &self,
            _session: SessionId,
            _generation: GenerationId,
        ) -> StoreFuture<'_, u32> {
            let open = self.open;
            Box::pin(async move { Ok(open) })
        }
    }

    #[derive(Default)]
    struct ProviderScript {
        state: Option<ProviderState>,
        suspend: Option<ProviderCall>,
        resume: Option<ProviderCall>,
        terminate: Option<ProviderCall>,
        hold_transition: bool,
        calls: Vec<&'static str>,
    }

    #[derive(Default)]
    struct FakeProvider {
        script: Mutex<ProviderScript>,
    }

    impl FakeProvider {
        fn running() -> Arc<Self> {
            Arc::new(Self {
                script: Mutex::new(ProviderScript {
                    state: Some(ProviderState::Running),
                    ..ProviderScript::default()
                }),
            })
        }

        fn suspended() -> Arc<Self> {
            Arc::new(Self {
                script: Mutex::new(ProviderScript {
                    state: Some(ProviderState::Suspended),
                    ..ProviderScript::default()
                }),
            })
        }

        fn lock(&self) -> std::sync::MutexGuard<'_, ProviderScript> {
            self.script
                .lock()
                .expect("the fixture lock is not poisoned")
        }

        fn calls(&self) -> Vec<&'static str> {
            self.lock().calls.clone()
        }
    }

    impl MicrovmControlApi for FakeProvider {
        fn run<'a>(&'a self, _request: &'a RunRequest) -> ProviderFuture<'a, MicrovmDescription> {
            self.lock().calls.push("run");
            Box::pin(async move {
                Err(ProviderCall::Fatal {
                    code: "the control worker never launches".into(),
                })
            })
        }

        fn get<'a>(&'a self, _microvm: &'a MicrovmId) -> ProviderFuture<'a, MicrovmDescription> {
            let mut script = self.lock();
            script.calls.push("get");
            let state = script.state;
            drop(script);
            Box::pin(async move {
                match state {
                    Some(state) => Ok(MicrovmDescription {
                        microvm: microvm(),
                        state,
                        endpoint: None,
                        launched_at: Some(at(LAUNCHED_AT)),
                    }),
                    None => Err(ProviderCall::NotFound),
                }
            })
        }

        fn suspend<'a>(&'a self, _microvm: &'a MicrovmId) -> ProviderFuture<'a, ProviderRequestId> {
            let mut script = self.lock();
            script.calls.push("suspend");
            if let Some(failure) = script.suspend.clone() {
                drop(script);
                return Box::pin(async move { Err(failure) });
            }
            if !script.hold_transition {
                script.state = Some(ProviderState::Suspended);
            }
            drop(script);
            Box::pin(async move { Ok(ProviderRequestId("req-1".to_owned())) })
        }

        fn resume<'a>(&'a self, _microvm: &'a MicrovmId) -> ProviderFuture<'a, ProviderRequestId> {
            let mut script = self.lock();
            script.calls.push("resume");
            if let Some(failure) = script.resume.clone() {
                drop(script);
                return Box::pin(async move { Err(failure) });
            }
            script.state = Some(ProviderState::Running);
            drop(script);
            Box::pin(async move { Ok(ProviderRequestId("req-2".to_owned())) })
        }

        fn terminate<'a>(
            &'a self,
            _microvm: &'a MicrovmId,
        ) -> ProviderFuture<'a, ProviderRequestId> {
            let mut script = self.lock();
            script.calls.push("terminate");
            if let Some(failure) = script.terminate.clone() {
                drop(script);
                return Box::pin(async move { Err(failure) });
            }
            script.state = Some(ProviderState::Terminated);
            drop(script);
            Box::pin(async move { Ok(ProviderRequestId("req-3".to_owned())) })
        }

        fn auth_token<'a>(
            &'a self,
            _microvm: &'a MicrovmId,
            _ttl_seconds: u64,
            _ports: &'a [u16],
        ) -> ProviderFuture<'a, EndpointToken> {
            self.lock().calls.push("auth_token");
            Box::pin(async move {
                Err(ProviderCall::Fatal {
                    code: "the control worker mints no endpoint token".into(),
                })
            })
        }

        fn list<'a>(
            &'a self,
            _image: Option<&'a ImageIdentifier>,
            _page: Option<&'a str>,
        ) -> ProviderFuture<'a, MicrovmPage> {
            self.lock().calls.push("list");
            Box::pin(async move {
                Ok(MicrovmPage {
                    microvms: Vec::new(),
                    next: None,
                })
            })
        }
    }

    #[derive(Default)]
    struct FakeSink {
        facts: Mutex<Vec<(UsageCategory, FactDraft)>>,
        failure: Mutex<Option<SinkError>>,
    }

    impl FakeSink {
        fn meters(&self) -> Vec<Meter> {
            self.facts
                .lock()
                .expect("the fixture lock is not poisoned")
                .iter()
                .filter_map(|(_, fact)| fact.kind.meter())
                .collect()
        }

        fn quantities(&self) -> Vec<u128> {
            self.facts
                .lock()
                .expect("the fixture lock is not poisoned")
                .iter()
                .filter_map(|(_, fact)| match &fact.kind {
                    FactKind::Measured(measurement) => Some(measurement.quantity().get()),
                    _ => None,
                })
                .collect()
        }

        fn categories(&self) -> Vec<UsageCategory> {
            self.facts
                .lock()
                .expect("the fixture lock is not poisoned")
                .iter()
                .map(|(category, _)| *category)
                .collect()
        }

        fn fail_with(&self, failure: Option<SinkError>) {
            *self
                .failure
                .lock()
                .expect("the fixture lock is not poisoned") = failure;
        }
    }

    impl UsageFactSink for FakeSink {
        fn emit<'a>(
            &'a self,
            category: UsageCategory,
            fact: FactDraft,
        ) -> core::pin::Pin<Box<dyn Future<Output = Result<(), SinkError>> + Send + 'a>> {
            if let Some(error) = self
                .failure
                .lock()
                .expect("the fixture lock is not poisoned")
                .clone()
            {
                return Box::pin(async move { Err(error) });
            }
            self.facts
                .lock()
                .expect("the fixture lock is not poisoned")
                .push((category, fact));
            Box::pin(async move { Ok(()) })
        }
    }

    struct NoPace;

    impl Pace for NoPace {
        fn sleep(&self, _millis: u64) -> core::pin::Pin<Box<dyn Future<Output = ()> + Send + '_>> {
            Box::pin(async move {})
        }
    }

    struct Fixture {
        control: RuntimeControl,
        store: Arc<FakeStore>,
        provider: Arc<FakeProvider>,
        compute: Arc<FakeSink>,
        storage: Arc<FakeSink>,
    }

    fn fixture(store: Arc<FakeStore>, provider: Arc<FakeProvider>, open: u32) -> Fixture {
        let compute = Arc::new(FakeSink::default());
        let storage = Arc::new(FakeSink::default());
        let control = RuntimeControl::new(
            RuntimePorts {
                store: store.clone(),
                effects: Arc::new(FakeEffects { open }),
                provider: provider.clone(),
                compute: compute.clone(),
                storage: storage.clone(),
                pace: Arc::new(NoPace),
            },
            RuntimeSettings {
                region: Region::EuWest1,
                pricing_version: PricingVersion("synthetic-zero-v1".to_owned()),
                shards: 8,
                page: PageBudget {
                    max_items: 50,
                    max_reads: 100,
                },
                schedule_jitter_ms: 0,
            },
        );
        Fixture {
            control,
            store,
            provider,
            compute,
            storage,
        }
    }

    fn evaluate() -> RuntimeCommand {
        RuntimeCommand::Evaluate {
            session: session(),
            generation: generation(),
        }
    }

    fn wake() -> RuntimeCommand {
        RuntimeCommand::LiveWorkspaceWake {
            session: session(),
            generation: generation(),
        }
    }

    #[tokio::test]
    async fn the_exact_180000_ms_boundary_decides_whether_a_generation_is_suspended() {
        let idle = fixture(
            FakeStore::with(view(GenerationState::Running, 0)),
            FakeProvider::running(),
            0,
        );
        assert_eq!(
            idle.control
                .handle_command(evaluate(), at(BUSY_AT + 179_999))
                .await,
            CommandOutcome::Settled(Settled::Held {
                reason: HoldReason::NotYetIdle
            }),
            "179999 ms is not idle"
        );
        assert!(
            !idle.provider.calls().contains(&"suspend"),
            "nothing is suspended one millisecond early"
        );
        assert_eq!(
            idle.store.lock().probes.len(),
            1,
            "a hold rearms the evaluation schedule"
        );

        let boundary = fixture(
            FakeStore::with(view(GenerationState::Running, 0)),
            FakeProvider::running(),
            0,
        );
        assert_eq!(
            boundary
                .control
                .handle_command(evaluate(), at(BUSY_AT + 180_000))
                .await,
            CommandOutcome::Settled(Settled::Suspended),
            "180000 ms is"
        );
        assert!(boundary.provider.calls().contains(&"suspend"));
    }

    #[tokio::test]
    async fn an_open_background_operation_blocks_suspension_indefinitely() {
        let busy = fixture(
            FakeStore::with(view(GenerationState::Running, 1)),
            FakeProvider::running(),
            1,
        );
        for elapsed in [180_000_i64, 3_600_000, 20_000_000] {
            assert_eq!(
                busy.control
                    .handle_command(evaluate(), at(BUSY_AT + elapsed))
                    .await,
                CommandOutcome::Settled(Settled::Held {
                    reason: HoldReason::Busy
                }),
                "elapsed {elapsed} ms"
            );
        }
        assert!(!busy.provider.calls().contains(&"suspend"));
    }

    #[tokio::test]
    async fn the_authoritative_recount_repairs_a_stale_over_count_and_suspends_nothing() {
        // The head says nothing is open; the session authority says three are.
        let raced = fixture(
            FakeStore::with(view(GenerationState::Running, 0)),
            FakeProvider::running(),
            3,
        );
        assert_eq!(
            raced
                .control
                .handle_command(evaluate(), at(BUSY_AT + 180_000))
                .await,
            CommandOutcome::Settled(Settled::CounterRepaired {
                authoritative_open: 3,
                recorded_open: 0
            })
        );
        assert!(
            !raced.provider.calls().contains(&"suspend"),
            "acting on a counter that was just proven wrong is how a running job gets snapshotted"
        );
        let state = raced.store.lock();
        assert_eq!(
            state.commits.len(),
            2,
            "the lock is taken and then given back"
        );
        assert_eq!(state.commits[0].next_state, GenerationState::Suspending);
        assert_eq!(state.commits[1].next_state, GenerationState::Running);
        assert_eq!(
            state.intents.len(),
            1,
            "the blocking intent and fence land atomically before the recount"
        );
        assert_eq!(
            state.receipts.len(),
            1,
            "a recount refusal settles that intent while restoring running"
        );
    }

    #[tokio::test]
    async fn a_suspend_closes_the_running_interval_into_compute_and_memory_facts_only() {
        let closing = fixture(
            FakeStore::with(view(GenerationState::Running, 0)),
            FakeProvider::running(),
            0,
        );
        let now = at(LAUNCHED_AT + 3_600_000);
        assert_eq!(
            closing.control.handle_command(evaluate(), now).await,
            CommandOutcome::Settled(Settled::Suspended)
        );
        assert_eq!(
            closing.compute.meters(),
            vec![Meter::ComputeMillicpuMs, Meter::MemoryByteMs],
            "memory is a discriminated fact inside the compute authority"
        );
        assert!(
            closing.storage.meters().is_empty(),
            "a suspend opens a residence; it does not close one"
        );
        assert_eq!(
            closing.compute.quantities(),
            vec![500 * 3_600_000, 1_073_741_824 * 3_600_000]
        );
        assert!(
            closing
                .compute
                .categories()
                .iter()
                .all(|category| *category == Category::Compute),
            "no fact reaches an authority this worker holds no binding for"
        );
        let state = closing.store.lock();
        let accounting = state.commits[1]
            .accounting
            .expect("the settled transition advances accounting");
        assert_eq!(accounting.accounted_from, now);
        assert_eq!(accounting.suspended_at, Some(now));
        assert_eq!(accounting.snapshot_ordinal, 0);
        assert_eq!(state.receipts[0].usage.len(), 2);
        assert!(
            state.outbox.is_empty(),
            "successful enqueue acknowledges the outbox"
        );
    }

    #[tokio::test]
    async fn a_usage_outage_cannot_lose_a_committed_lifecycle_interval() {
        let closing = fixture(
            FakeStore::with(view(GenerationState::Running, 0)),
            FakeProvider::running(),
            0,
        );
        closing.compute.fail_with(Some(SinkError::Unavailable {
            category: Category::Compute,
            reason: "queue unavailable".to_owned(),
        }));
        let now = at(LAUNCHED_AT + 3_600_000);
        assert!(matches!(
            closing.control.handle_command(evaluate(), now).await,
            CommandOutcome::Retry { .. }
        ));
        {
            let state = closing.store.lock();
            assert_eq!(
                state.view.as_ref().expect("view").head.state,
                GenerationState::Suspended
            );
            assert_eq!(state.outbox.len(), 2, "both compute drafts remain durable");
        }

        closing.compute.fail_with(None);
        let outcome = closing.control.handle_command(evaluate(), now).await;
        assert!(matches!(
            outcome,
            CommandOutcome::Settled(
                Settled::Held { .. } | Settled::AlreadyTerminal { .. } | Settled::Raced
            )
        ));
        assert!(
            closing.store.lock().outbox.is_empty(),
            "redelivery flushes and acknowledges the transactional outbox"
        );
        assert_eq!(closing.compute.meters().len(), 2);
    }

    #[tokio::test]
    async fn a_lost_settlement_response_cannot_repeat_the_provider_effect() {
        let store = FakeStore::with(view(GenerationState::Running, 0));
        store.lock().lose_next_settle_response = true;
        let closing = fixture(Arc::clone(&store), FakeProvider::running(), 0);
        let now = at(LAUNCHED_AT + 3_600_000);

        assert!(matches!(
            closing.control.handle_command(evaluate(), now).await,
            CommandOutcome::Retry { .. }
        ));
        {
            let state = store.lock();
            assert_eq!(
                state.view.as_ref().expect("view").head.state,
                GenerationState::Suspended,
                "the receipt, final state, accounting, and outbox committed together"
            );
            assert_eq!(state.outbox.len(), 2);
        }
        assert_eq!(
            closing
                .provider
                .calls()
                .into_iter()
                .filter(|call| *call == "suspend")
                .count(),
            1
        );

        let outcome = closing.control.handle_command(evaluate(), now).await;
        assert!(matches!(outcome, CommandOutcome::Settled(_)));
        assert!(store.lock().outbox.is_empty());
        assert_eq!(closing.compute.meters().len(), 2);
        assert_eq!(
            closing
                .provider
                .calls()
                .into_iter()
                .filter(|call| *call == "suspend")
                .count(),
            1,
            "redelivery observes the final head and never repeats the effect"
        );
    }

    #[tokio::test]
    async fn the_fence_and_intent_land_atomically_before_recount_provider_and_receipt() {
        let ordered = fixture(
            FakeStore::with(view(GenerationState::Running, 0)),
            FakeProvider::running(),
            0,
        );
        ordered
            .control
            .handle_command(evaluate(), at(BUSY_AT + 180_000))
            .await;
        let state = ordered.store.lock();
        assert_eq!(state.commits[0].next_state, GenerationState::Suspending);
        assert_eq!(
            state.commits[0].next_fence,
            Fence(4),
            "the transition advances the fence, so Brain can no longer admit"
        );
        assert_eq!(
            state.intents.len(),
            1,
            "one intent, recorded before the call"
        );
        assert_eq!(state.intents[0].action, LifecycleAction::Suspend);
        assert_eq!(state.receipts.len(), 1);
        assert_eq!(state.receipts[0].next_intent_state, IntentState::Settled);
        assert_eq!(
            state.receipts[0].provider_request_id,
            Some(ProviderRequestId("req-1".to_owned())),
            "a missing request id is never invented"
        );
        assert_eq!(
            state.commits[1].next_state,
            GenerationState::Suspended,
            "the head lands only after the provider settled"
        );
        let pointer = state.pointer.as_ref().expect("the fixture has CURRENT");
        let head = &state.view.as_ref().expect("the fixture has HEAD").head;
        assert_eq!(pointer.fence, head.fence, "CURRENT converges with HEAD");
        assert_eq!(
            pointer.revision, head.revision,
            "CURRENT and HEAD advance in the same lifecycle transaction"
        );
    }

    #[tokio::test]
    async fn the_eight_hour_lifetime_drains_at_minus_300_s_and_terminates_at_minus_60_s() {
        let expiry = Lifetime {
            launched_at: at(LAUNCHED_AT),
        }
        .expires_at();

        let draining = fixture(
            FakeStore::with(view(GenerationState::Running, 0)),
            FakeProvider::running(),
            0,
        );
        assert_eq!(
            draining
                .control
                .handle_command(evaluate(), minus_millis(expiry, LIFETIME_DRAIN_MARGIN_MS))
                .await,
            CommandOutcome::Settled(Settled::Draining {
                remaining_ms: LIFETIME_DRAIN_MARGIN_MS
            })
        );
        assert_eq!(
            draining.store.lock().commits[0].next_state,
            GenerationState::LifetimeDraining,
            "draining is an actual fenced transition, not an observation"
        );
        assert!(!draining.provider.calls().contains(&"terminate"));

        let terminating = fixture(
            FakeStore::with(view(GenerationState::Running, 0)),
            FakeProvider::running(),
            0,
        );
        let outcome = terminating
            .control
            .handle_command(
                evaluate(),
                minus_millis(expiry, LIFETIME_TERMINATE_MARGIN_MS),
            )
            .await;
        assert_eq!(
            outcome,
            CommandOutcome::Settled(Settled::Terminated {
                continuity_lost: true,
                remaining_ms: LIFETIME_TERMINATE_MARGIN_MS
            }),
            "the customer is told continuity was lost, with the exact remaining number"
        );
        assert!(terminating.provider.calls().contains(&"terminate"));
        assert_eq!(
            terminating.compute.meters(),
            vec![Meter::ComputeMillicpuMs, Meter::MemoryByteMs],
            "the receipt is closed rather than stranded"
        );
    }

    #[tokio::test]
    async fn a_resume_is_refused_inside_the_terminate_margin() {
        let mut suspended = view(GenerationState::Suspended, 0);
        suspended.suspended_at = Some(at(LAUNCHED_AT + 1_000));
        let refusing = fixture(FakeStore::with(suspended), FakeProvider::suspended(), 0);
        let expiry = Lifetime {
            launched_at: at(LAUNCHED_AT),
        }
        .expires_at();
        assert_eq!(
            refusing
                .control
                .handle_command(wake(), minus_millis(expiry, LIFETIME_TERMINATE_MARGIN_MS))
                .await,
            CommandOutcome::Settled(Settled::ResumeRefused {
                remaining_ms: LIFETIME_TERMINATE_MARGIN_MS
            })
        );
        assert!(
            !refusing.provider.calls().contains(&"resume"),
            "a new generation is allocated instead"
        );
    }

    #[tokio::test]
    async fn a_wake_resumes_and_closes_the_retained_snapshot_into_a_storage_fact() {
        let mut suspended = view(GenerationState::Suspended, 0);
        suspended.suspended_at = Some(at(LAUNCHED_AT));
        let waking = fixture(FakeStore::with(suspended), FakeProvider::suspended(), 0);
        assert_eq!(
            waking
                .control
                .handle_command(wake(), at(LAUNCHED_AT + 600_000))
                .await,
            CommandOutcome::Settled(Settled::Resumed)
        );
        assert!(waking.provider.calls().contains(&"resume"));
        assert_eq!(
            waking.storage.meters(),
            vec![Meter::StorageByteMin],
            "retained snapshot bytes are the only thing a suspension charges"
        );
        assert_eq!(waking.storage.quantities(), vec![445_644_800 * 10]);
        assert_eq!(
            waking.compute.meters(),
            vec![Meter::ComputeMillicpuMs, Meter::MemoryByteMs],
            "the interval is accounted rather than silently dropped"
        );
        assert_eq!(
            waking.compute.quantities(),
            vec![0, 0],
            "suspended time is not running time, and saying so with a zero is not the same as              saying nothing"
        );
        let state = waking.store.lock();
        let accounting = state.commits[1].accounting.expect("resume accounting");
        assert_eq!(accounting.accounted_from, at(LAUNCHED_AT + 600_000));
        assert_eq!(accounting.suspended_at, None);
        assert_eq!(accounting.snapshot_ordinal, 1);
    }

    #[tokio::test]
    async fn a_workspace_discard_terminates_and_releases_the_snapshot() {
        let mut suspended = view(GenerationState::Suspended, 0);
        suspended.suspended_at = Some(at(LAUNCHED_AT));
        let discarding = fixture(FakeStore::with(suspended), FakeProvider::suspended(), 0);
        let outcome = discarding
            .control
            .handle_command(
                RuntimeCommand::WorkspaceDiscard {
                    session: session(),
                    generation: generation(),
                },
                at(LAUNCHED_AT + 600_000),
            )
            .await;
        assert!(
            matches!(
                outcome,
                CommandOutcome::Settled(Settled::Terminated {
                    continuity_lost: false,
                    ..
                })
            ),
            "a discard the customer asked for is not a continuity loss: {outcome:?}"
        );
        assert!(discarding.provider.calls().contains(&"terminate"));
        assert_eq!(discarding.storage.meters(), vec![Meter::StorageByteMin]);
        let state = discarding.store.lock();
        let accounting = state.commits[1].accounting.expect("terminal accounting");
        assert_eq!(accounting.suspended_at, None);
        assert_eq!(accounting.snapshot_ordinal, 1);
    }

    #[tokio::test]
    async fn an_indeterminate_provider_outcome_records_unknown_and_dispatches_no_second_effect() {
        let store = FakeStore::with(view(GenerationState::Running, 0));
        let provider = FakeProvider::running();
        provider.lock().suspend = Some(ProviderCall::Unknown {
            request: Some(ProviderRequestId("req-lost".to_owned())),
        });
        let ambiguous = fixture(store, provider, 0);
        assert_eq!(
            ambiguous
                .control
                .handle_command(evaluate(), at(BUSY_AT + 180_000))
                .await,
            CommandOutcome::Settled(Settled::Reconciling)
        );
        assert_eq!(
            ambiguous.store.lock().receipts[0].next_intent_state,
            IntentState::Unknown
        );
        assert_eq!(
            ambiguous.store.lock().receipts[0].provider_request_id,
            Some(ProviderRequestId("req-lost".to_owned())),
            "the request id carried by an ambiguous SDK answer remains durable"
        );
        assert_eq!(
            ambiguous
                .provider
                .calls()
                .iter()
                .filter(|call| **call == "suspend")
                .count(),
            1,
            "an ambiguous outcome is never retried blindly"
        );

        // A later pass probes the exact VM, advances the durable budget, and
        // dispatches no second effect while the provider still reports RUNNING.
        assert!(matches!(
            ambiguous
                .control
                .handle_command(evaluate(), at(BUSY_AT + 180_001))
                .await,
            CommandOutcome::Retry { .. }
        ));
        assert_eq!(
            ambiguous
                .provider
                .calls()
                .iter()
                .filter(|call| **call == "suspend")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn a_timeout_persists_the_request_id_and_a_later_probe_settles_without_redispatch() {
        let store = FakeStore::with(view(GenerationState::Running, 0));
        let provider = FakeProvider::running();
        provider.lock().hold_transition = true;
        let control = fixture(Arc::clone(&store), Arc::clone(&provider), 0);

        assert_eq!(
            control
                .control
                .handle_command(evaluate(), at(BUSY_AT + 180_000))
                .await,
            CommandOutcome::Settled(Settled::Reconciling)
        );
        {
            let state = store.lock();
            let open = state
                .view
                .as_ref()
                .and_then(|view| view.open_intent.as_ref())
                .expect("the timed-out intent remains open");
            assert_eq!(open.state, IntentState::Unknown);
            assert_eq!(
                open.provider_request_id,
                Some(ProviderRequestId("req-1".to_owned())),
                "the request identity is persisted before the await can time out"
            );
        }

        provider.lock().state = Some(ProviderState::Suspended);
        assert_eq!(
            control
                .control
                .handle_command(evaluate(), at(BUSY_AT + 180_001))
                .await,
            CommandOutcome::Settled(Settled::Suspended)
        );
        assert_eq!(
            provider
                .calls()
                .into_iter()
                .filter(|call| *call == "suspend")
                .count(),
            1,
            "reconciliation probes and never repeats the provider effect"
        );
    }

    #[tokio::test]
    async fn the_eighth_unresolved_probe_quarantines_durably_and_never_redrives_hot() {
        let mut unknown = view(GenerationState::Unknown, 0);
        unknown.open_intent = Some(IntentRecord {
            intent_id: super::intent_id(generation(), LifecycleAction::Suspend, Fence(3)),
            generation: generation(),
            microvm: Some(microvm()),
            action: LifecycleAction::Suspend,
            fence: Fence(3),
            state: IntentState::Unknown,
            provider_request_id: None,
            attempts: 7,
            dispatched_at: at(BUSY_AT),
        });
        let store = FakeStore::with(unknown);
        let provider = FakeProvider::running();
        let quarantining = fixture(Arc::clone(&store), Arc::clone(&provider), 0);
        let outcome = quarantining
            .control
            .handle_command(evaluate(), at(BUSY_AT + 1))
            .await;
        assert!(matches!(outcome, CommandOutcome::Poison { .. }));
        let state = store.lock();
        let open = state
            .view
            .as_ref()
            .and_then(|view| view.open_intent.as_ref())
            .expect("quarantine remains operator-visible");
        assert_eq!(open.state, IntentState::Quarantined);
        assert_eq!(open.attempts, 8);
        assert!(!provider.calls().contains(&"suspend"));
    }

    #[tokio::test]
    async fn a_refused_dispatch_settles_its_intent_and_redrives() {
        let store = FakeStore::with(view(GenerationState::Running, 0));
        let provider = FakeProvider::running();
        provider.lock().suspend = Some(ProviderCall::Transient {
            class: TransientClass::ServerFault,
        });
        let refused = fixture(store, provider, 0);
        let outcome = refused
            .control
            .handle_command(evaluate(), at(BUSY_AT + 180_000))
            .await;
        assert!(
            matches!(outcome, CommandOutcome::Retry { .. }),
            "{outcome:?}"
        );
        let state = refused.store.lock();
        assert_eq!(
            state.receipts[0].next_intent_state,
            IntentState::Settled,
            "a refused dispatch had no effect, so its intent must not block every later one"
        );
        assert_eq!(
            state.commits.last().expect("a restore").next_state,
            GenerationState::Running
        );
    }

    #[tokio::test]
    async fn a_lost_generation_is_absorbing() {
        let store = FakeStore::with(view(GenerationState::Running, 0));
        let provider = FakeProvider::running();
        provider.lock().state = None;
        let lost = fixture(store, provider, 0);
        assert_eq!(
            lost.control
                .handle_command(evaluate(), at(BUSY_AT + 180_000))
                .await,
            CommandOutcome::Settled(Settled::Lost)
        );
    }

    #[tokio::test]
    async fn an_unmaterialized_generation_costs_no_provider_call_at_all() {
        let mut lazy = view(GenerationState::Requested, 0);
        lazy.microvm = None;
        lazy.lifetime = None;
        let unmaterialized = fixture(FakeStore::with(lazy), FakeProvider::running(), 0);
        assert_eq!(
            unmaterialized
                .control
                .handle_command(evaluate(), at(BUSY_AT + 180_000))
                .await,
            CommandOutcome::Settled(Settled::NotMaterialized)
        );
        assert!(
            unmaterialized.provider.calls().is_empty(),
            "H-LAZY: a model-only session consumes no lifecycle API rate"
        );
    }

    #[tokio::test]
    async fn the_fence_settles_a_superseded_command_and_quarantines_an_invented_one() {
        let bound = fixture(
            FakeStore::with(view(GenerationState::Running, 0)),
            FakeProvider::running(),
            0,
        );
        let older = GenerationId::from_uuid7(Uuid7::compose(1, [1; 10]));
        assert_eq!(
            bound
                .control
                .handle_command(
                    RuntimeCommand::Evaluate {
                        session: session(),
                        generation: older,
                    },
                    at(BUSY_AT + 180_000),
                )
                .await,
            CommandOutcome::Settled(Settled::Superseded {
                current: generation()
            })
        );

        let invented = GenerationId::from_uuid7(Uuid7::compose(99, [1; 10]));
        let outcome = bound
            .control
            .handle_command(
                RuntimeCommand::Evaluate {
                    session: session(),
                    generation: invented,
                },
                at(BUSY_AT + 180_000),
            )
            .await;
        assert!(
            matches!(outcome, CommandOutcome::Poison { .. }),
            "{outcome:?}"
        );
        assert!(
            !bound.provider.calls().contains(&"suspend"),
            "neither command reaches the provider"
        );
    }

    #[tokio::test]
    async fn the_queue_reports_exactly_the_failed_identifiers() {
        let queued = fixture(
            FakeStore::with(view(GenerationState::Running, 1)),
            FakeProvider::running(),
            1,
        );
        let good = serde_json::to_string(&evaluate()).expect("the command serializes");
        let records = [
            QueueRecord {
                message_id: "a".to_owned(),
                receive_count: 1,
                body: good.clone(),
            },
            QueueRecord {
                message_id: "b".to_owned(),
                receive_count: 1,
                body: "{\"domain\":\"runtime.evaluate\"}".to_owned(),
            },
            QueueRecord {
                message_id: "c".to_owned(),
                receive_count: 1,
                body: good,
            },
        ];
        let result = queued.control.handle_queue(&records, at(BUSY_AT)).await;
        assert!(
            result.response.batch_item_failures.is_empty(),
            "a hold and a poison are both settled for the queue"
        );
        assert_eq!(result.quarantined.len(), 1);
        assert_eq!(result.quarantined[0].message_id, "b");
    }

    #[tokio::test]
    async fn a_retryable_item_is_the_only_identifier_reported() {
        let store = FakeStore::with(view(GenerationState::Running, 0));
        let provider = FakeProvider::running();
        provider.lock().suspend = Some(ProviderCall::Throttled {
            retry_after: core::time::Duration::from_secs(2),
        });
        let throttled = fixture(store, provider, 0);
        let body = serde_json::to_string(&evaluate()).expect("the command serializes");
        let records = [QueueRecord {
            message_id: "only".to_owned(),
            receive_count: 1,
            body,
        }];
        let result = throttled
            .control
            .handle_queue(&records, at(BUSY_AT + 180_000))
            .await;
        assert_eq!(result.response.batch_item_failures.len(), 1);
        assert_eq!(
            result.response.batch_item_failures[0].item_identifier,
            "only"
        );
        assert!(result.quarantined.is_empty());
    }

    #[tokio::test]
    async fn the_schedule_pass_evaluates_every_due_generation_and_carries_the_cursor() {
        let store = FakeStore::with(view(GenerationState::Running, 1));
        {
            let mut state = store.lock();
            state.due = vec![pointer()];
            state.cursor = Some("next".to_owned());
        }
        let scheduled = fixture(store, FakeProvider::running(), 1);
        let pass = scheduled
            .control
            .handle_schedule(RuntimeShard(0), at(BUSY_AT))
            .await;
        assert_eq!(pass.scanned, 1);
        assert_eq!(pass.cursor.as_deref(), Some("next"));
        assert_eq!(pass.scan_failure, None);
        assert_eq!(pass.outcomes.len(), 1);
        assert_eq!(pass.outcomes[0].0, generation());
    }

    #[tokio::test]
    async fn an_unreadable_due_index_is_reported_and_never_read_as_an_empty_page() {
        let store = FakeStore::with(view(GenerationState::Running, 0));
        store.lock().scan_failure = Some(RuntimeStoreError::Unavailable {
            reason: "throttled".to_owned(),
        });
        let broken = fixture(store, FakeProvider::running(), 0);
        let pass = broken
            .control
            .handle_schedule(RuntimeShard(0), at(BUSY_AT))
            .await;
        assert_eq!(pass.scanned, 0);
        assert!(pass.scan_failure.is_some());
        assert!(pass.outcomes.is_empty());
    }

    #[test]
    fn the_true_idle_threshold_is_a_compiled_constant_and_not_a_setting() {
        assert_eq!(TRUE_IDLE_THRESHOLD_MS, 180_000);
        // The settings type is destructured exhaustively on purpose: adding a
        // threshold field would fail to compile rather than quietly becoming
        // deployment-tunable, which is what HR-21 forbids.
        let RuntimeSettings {
            region: _,
            pricing_version: _,
            shards: _,
            page: _,
            schedule_jitter_ms: _,
        } = RuntimeSettings {
            region: Region::EuWest1,
            pricing_version: PricingVersion("synthetic-zero-v1".to_owned()),
            shards: 1,
            page: PageBudget {
                max_items: 1,
                max_reads: 1,
            },
            schedule_jitter_ms: 0,
        };
    }

    #[test]
    fn every_work_domain_round_trips_through_its_wire_name() {
        for (command, domain) in [
            (evaluate(), "runtime.evaluate"),
            (wake(), "runtime.live_workspace_wake"),
            (
                RuntimeCommand::WorkspaceDiscard {
                    session: session(),
                    generation: generation(),
                },
                "runtime.workspace_discard",
            ),
        ] {
            assert_eq!(command.domain(), domain);
            let json = serde_json::to_value(command).expect("it serializes");
            assert_eq!(json["domain"], domain);
            let restored: RuntimeCommand = serde_json::from_value(json).expect("it deserializes");
            assert_eq!(restored, command);
        }
        // An unknown field is refused, so a sender cannot smuggle one past the
        // fence by decorating a known domain.
        assert!(
            serde_json::from_str::<RuntimeCommand>(
                r#"{"domain":"runtime.evaluate","session":"ses_x","generation":"gen_x","fence":9}"#
            )
            .is_err()
        );
    }
}
