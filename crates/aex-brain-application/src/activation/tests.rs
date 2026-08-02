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
    ProviderScript, Recorder, ScriptedProvider, ScriptedTools, wake_for,
};
use super::{
    Activation, ActivationError, ActivationPolicy, Outcome, Ports, Release, Stop, WakeLoop,
};
use crate::kernel::{ActivationRegistry, DrainGate};
use crate::ports::{
    CancelToken, ClaimError, ClockPort as _, CommitError, ConditionFailure, EffectStore as _,
    FenceGuard, LeaseStore as _, ProviderDispatchError, ProviderFailureClass, ProviderOutcome,
    RedactedDetail, ReleaseDisposition, StoreError, WakeQueue as _,
};
use aex_brain_domain::budget::DimensionVector;
use aex_brain_domain::effect::{
    DispatchProof, DispatchStage, DurableEffect, EffectClass, EffectKind, EffectState,
};
use aex_brain_domain::ids::{
    AgentId, AgentKey, CatalogPin, ContentHash, EffectId, JournalSeq, ModelSlug, OwnerToken,
    SessionId, Timestamp, WakeId, WorkShard,
};
use aex_brain_domain::journal::{
    FinishReason, JournalEntry, JournalRecord, MessageOrigin, ParkReason,
};
use aex_brain_domain::wire_pending::{
    AgentLimits, CanonicalBlock, CompleteAssistantMessage, CompleteProof, ContentBlockRef,
    ModelCapability, NormalizedUsage, ProviderId, ProviderReceipt, ResolvedAgentConfig, StopReason,
};
use aex_wire::ids::{GenerationId, PrefixedId as _, Uuid7};
use std::sync::Arc;
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

const START: i64 = 1_767_225_600_000;

fn key() -> AgentKey {
    AgentKey::new(
        SessionId(Uuid::from_u128(0x5e55_1000)),
        AgentId(Uuid::from_u128(0xa6e7_2000)),
    )
}

fn pin() -> CatalogPin {
    CatalogPin(ContentHash::of(b"catalog"))
}

fn model() -> ModelSlug {
    ModelSlug("deepseek-chat".to_owned())
}

fn config() -> ResolvedAgentConfig {
    ResolvedAgentConfig {
        catalog_pin: pin(),
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
                    text: "summarize this".to_owned(),
                },
            }],
            origin: MessageOrigin::Submission,
        },
    )
    .expect("the record canonicalizes");
    vec![started, message]
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
        usage: NormalizedUsage {
            input_tokens: 12,
            output_tokens: 34,
            ..NormalizedUsage::default()
        },
        receipt: ProviderReceipt {
            provider: ProviderId::Deepseek,
            model: model(),
            request_id: None,
            route_revision: 1,
        },
    }
}

fn failure(proof: DispatchProof, class: ProviderFailureClass) -> ProviderDispatchError {
    ProviderDispatchError {
        stage: match proof {
            DispatchProof::NotSent => DispatchStage::PreDispatch,
            _ => DispatchStage::Dispatched,
        },
        proof,
        class,
        provider_request_id: None,
        retry_after: None,
        detail: RedactedDetail::new("the fixture refuses"),
    }
}

/// Everything one test drives.
struct Harness {
    clock: Arc<FixedClock>,
    queue: Arc<MemoryQueue>,
    store: Arc<MemoryStore>,
    provider: Arc<ScriptedProvider>,
    catalog: Arc<FixedCatalog>,
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
            catalog: Arc::new(FixedCatalog::with_model(capability())),
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
            tools: Arc::new(ScriptedTools::new(Vec::new(), Vec::new())) as Arc<_>,
            hands: Arc::new(AbsentHands) as Arc<_>,
            catalog: Arc::clone(&self.catalog) as Arc<_>,
            clock: Arc::clone(&self.clock) as Arc<_>,
            ids: Arc::new(CountingIds::new()) as Arc<_>,
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

    /// Projects the wake the session authority would have created, and drives it.
    fn wake(&self) {
        self.queue.project(wake_for(key(), "wrk-1"));
    }

    fn run_next(&self) -> Result<Outcome, ActivationError> {
        let delivery = block_on(self.queue.receive(1, core::time::Duration::from_secs(0)))
            .expect("the queue answers")
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
    assert_eq!(harness.queue.acked().len(), 1);
    assert_eq!(harness.queue.depth(), 0, "nothing was left outstanding");
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

/// The matrix is the `proof` field and nothing else. A `PossiblySent` failure is
/// indistinguishable from a served request, so it settles unknown and the run interrupts.
#[test]
fn a_possibly_sent_request_settles_unknown_and_is_never_attempted_again() {
    let harness = Harness::new(vec![ProviderScript::Fail(Box::new(failure(
        DispatchProof::PossiblySent,
        ProviderFailureClass::Transient,
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

/// `NotSent` is the one value that permits another attempt, because it is the only one that
/// says the upstream cannot have seen the request.
#[test]
fn only_a_provably_unsent_request_is_attempted_again() {
    let harness = Harness::new(vec![
        ProviderScript::Fail(Box::new(failure(
            DispatchProof::NotSent,
            ProviderFailureClass::Transient,
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
        ProviderFailureClass::Permanent,
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

/// One malformed oldest row and nine permanently held rows fill the first page. The cursor
/// must still advance so the younger eleventh row runs, then wrap only after the shard end.
#[test]
fn bad_and_held_oldest_rows_cannot_starve_younger_due_work() {
    let mut harness = Harness::new(vec![ProviderScript::Produce(Box::new(produced()))]);
    harness.policy.due_shards = 1;
    harness.policy.receive_batch = 10;

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
    harness.queue.persist(younger_wake, WorkShard(0));

    let pump = WakeLoop::new(harness.activation(), Arc::new(AlwaysAdmit));
    let first = block_on(pump.poll_once()).expect("the malformed row is isolated");
    assert_eq!(first.malformed, 1);
    assert_eq!(first.recovered, 9);
    assert_eq!(first.released, 9, "all nine valid old rows remain held");
    assert!(harness.provider.dispatched().is_empty());

    let second = block_on(pump.poll_once()).expect("the scan resumes after page one");
    assert_eq!(second.recovered, 1);
    assert_eq!(second.driven, 1);
    assert_eq!(harness.provider.dispatched().len(), 1);

    let wrapped = block_on(pump.poll_once()).expect("the completed pass wraps");
    assert_eq!(wrapped.malformed, 1);
    assert_eq!(wrapped.recovered, 9);
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
            2,
            harness.clock.now(),
        ))
        .expect_err("only current control ownership may mint a ticket");
        assert!(
            matches!(error, CommitError::Condition(ConditionFailure::StaleFence)),
            "{error:?}"
        );
    }

    let ticket = block_on(harness.store.mark_dispatch_started(
        &live,
        &successor.authority,
        &effect,
        2,
        harness.clock.now(),
    ))
    .expect("the live successor takes over the prepared identity");
    assert_eq!(ticket.effect(), effect);
    assert_eq!(ticket.fence(), successor.fence);
    assert!(matches!(
        harness.store.effects(key())[0].state,
        EffectState::DispatchStarted { attempt: 2 }
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
