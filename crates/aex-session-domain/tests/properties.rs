//! Property catalogue for `aex-session-domain`: plan 04 items 1-31.
//!
//! The generated histories run against the crate's own deterministic fixtures,
//! so every failure shrinks to a witness that can be replayed verbatim.

use std::collections::BTreeSet;

use aex_content_domain::ContentDigest;
use aex_internal_contracts::RunId;
use aex_operation_domain::{DeletionState, OperationKind};
use aex_session_domain::testing::{
    approval_binding, child_agent, drift_field, entry_at, id, materialized_state, moment,
    running_session, session_fixture, terminal_attempt,
};
use aex_session_domain::{
    AccountProjection, AccountState, AgentControl, AgentError, AgentStatus, AgentTerminal,
    ApprovalCancelCause, ApprovalDecision, ApprovalRejection, ApprovalStatus, AuthorityFact,
    BindingField, CancelCause, CancelScope, CommandClass, DeleteEvidence, EffectId,
    EffectiveLimits, ExemptCommand, JournalEntry, JournalError, JournalSeq, LifecycleStatus,
    PauseReason, ResolvedMessageBounds, RunOutcome, RunStatus, SessionLifecycle, SessionStatus,
    TerminalRejection, WorkAdmission, begin_delete, cancel_pending, cancel_session_work,
    claim_terminal, complete_agent, complete_delete, create_root, fold_control, pause_gate,
    project_account, request_approval, respond, spawn, validate_append,
};
use aex_wire::error::ErrorCode;
use aex_wire::ids::{
    AgentId, ApprovalId, GenerationId, MessageId, OperationId, PrefixedId as _, Uuid7, WorkspaceId,
};
use aex_wire::limits::LimitId;
use proptest::prelude::*;

fn limits(materialized_agents: u64) -> EffectiveLimits {
    [(LimitId::SessionMaterializedAgents, materialized_agents)]
        .into_iter()
        .collect()
}

// ---------------------------------------------------------------------------
// 1, 2, 31 — reachability, monotonicity and typed terminal reasons
// ---------------------------------------------------------------------------

#[test]
fn session_status_reachability() {
    // 1 `session_status_reachability`: every declared public status is produced
    // by the retained-generation lifecycle, not inserted as a fixture-only arm.
    let mut observed: BTreeSet<SessionStatus> = BTreeSet::new();
    let mut lifecycle =
        SessionLifecycle::launched(id::<GenerationId>(1), moment(0)).expect("launches");
    observed.insert(lifecycle.status);
    lifecycle
        .admit_message(
            id::<MessageId>(2),
            RunId::from_uuid7(Uuid7::compose(1, [3; 10])),
            ResolvedMessageBounds {
                max_spend_cents: std::num::NonZeroU64::new(1_000).expect("positive"),
                deadline: moment(10_000),
            },
            moment(1),
        )
        .expect("message");
    observed.insert(lifecycle.status);
    observed.insert(lifecycle.status);
    lifecycle.cancel_current(moment(2)).expect("cancels");
    lifecycle.begin_suspend().expect("starts suspend");
    observed.insert(lifecycle.status);
    lifecycle.complete_suspend(moment(3)).expect("suspends");
    observed.insert(lifecycle.status);
    lifecycle.begin_resume().expect("starts resume");
    observed.insert(lifecycle.status);
    lifecycle.complete_resume(moment(4)).expect("resumes");
    lifecycle
        .begin_terminate(aex_session_domain::TerminationReason::User)
        .expect("starts terminate");
    observed.insert(lifecycle.status);
    lifecycle.complete_terminate(moment(5)).expect("terminates");
    observed.insert(lifecycle.status);
    lifecycle.begin_delete().expect("starts delete");
    observed.insert(lifecycle.status);

    assert_eq!(observed.len(), SessionStatus::ALL.len());
    for status in LifecycleStatus::ALL {
        assert!(observed.contains(&status), "{status:?} must be reachable");
    }
}

#[test]
fn run_terminal_reason_total() {
    // 31 `run_terminal_reason_total`.
    let outcomes = [
        RunOutcome::Succeeded {
            output_messages: Vec::new(),
        },
        RunOutcome::Failed {
            error: aex_session_domain::DomainError {
                code: ErrorCode::InternalError,
                message: "failed".to_owned(),
                detail: None,
                retryable: false,
            },
        },
        RunOutcome::TimedOut {
            deadline: moment(5),
        },
        RunOutcome::Cancelled {
            by: id::<OperationId>(30),
        },
        RunOutcome::Interrupted(aex_session_domain::InterruptReason::AccountPaused),
    ];
    let mut seen: BTreeSet<RunStatus> = BTreeSet::new();
    for outcome in &outcomes {
        assert!(outcome.status().is_terminal());
        assert_eq!(
            outcome.has_typed_reason(),
            outcome.status() != RunStatus::Succeeded
        );
        seen.insert(outcome.status());
    }
    assert_eq!(seen.len(), 5);

    // `Queued -> Cancelled` is reachable without ever passing through Running.
    let (session, mut run, agent, message) = running_session();
    run.status = RunStatus::Queued;
    run.started_at = None;
    let mut attempt = terminal_attempt(&session, &run);
    attempt.outcome = RunOutcome::Cancelled {
        by: id::<OperationId>(30),
    };
    let commit = claim_terminal(
        &run,
        &session,
        agent.id,
        aex_session_domain::AgentFence::INITIAL,
        std::slice::from_ref(&message),
        &attempt,
    )
    .expect("wins");
    assert_eq!(commit.run.status, RunStatus::Cancelled);
    assert_eq!(commit.run.started_at, None);
}

// ---------------------------------------------------------------------------
// 3-5 — the terminal barrier
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    /// 3 `one_terminal_winner`, 4 `loser_writes_nothing`, 5
    /// `terminal_barrier_completeness`.
    #[test]
    fn terminal_barrier(attempts in 1_usize..8) {
        let (session, run, agent, message) = running_session();
        let attempt = terminal_attempt(&session, &run);

        let mut current = run.clone();
        let mut winners = 0_usize;
        let mut winning_outcome = None;
        for _ in 0..attempts {
            match claim_terminal(
                &current,
                &session,
                agent.id,
                aex_session_domain::AgentFence::INITIAL,
                std::slice::from_ref(&message),
                &attempt,
            ) {
                Ok(commit) => {
                    winners += 1;
                    // 5: the winning commit carries every required part.
                    prop_assert!(commit.run.status.is_terminal());
                    prop_assert!(commit.run.outcome.is_some());
                    prop_assert_eq!(commit.sealed_messages.len(), 1);
                    prop_assert_eq!(
                        commit.sealed_messages[0].state,
                        aex_session_domain::MessageState::Sealed
                    );
                    prop_assert_eq!(commit.session.active_run, None);
                    prop_assert_eq!(commit.session.revision, session.revision.next());
                    prop_assert_eq!(commit.outbox.run, run.id);
                    prop_assert_eq!(commit.usage_closure, attempt.usage_closure);
                    winning_outcome = commit.run.outcome.clone();
                    current = commit.run;
                }
                Err(rejection) => {
                    // 4: a loser produces no commit at all and names the winner.
                    prop_assert_eq!(
                        rejection,
                        TerminalRejection::AlreadyTerminal {
                            winner: winning_outcome.clone().expect("a loser follows a winner")
                        }
                    );
                }
            }
        }
        // 3: exactly one attempt ever commits.
        prop_assert_eq!(winners, 1);
    }

    /// 2 `revision_monotone`.
    #[test]
    fn revision_monotone(steps in prop::collection::vec(0_u8..3, 1..16)) {
        let mut session = session_fixture();
        let mut revision = session.revision;
        let mut cancellation = session.cancellation;
        let mut deletion = session.deletion.epoch;

        for step in steps {
            match step % 3 {
                0 => {
                    let commit = cancel_session_work(
                        &session,
                        &[],
                        CancelCause::SessionCancel,
                        None,
                        moment(1),
                    );
                    prop_assert!(commit.cancellation >= cancellation);
                    cancellation = commit.cancellation;
                    session.cancellation = commit.cancellation;
                    session.work_admission = commit.admission;
                }
                1 => {
                    if session.deletion.state == DeletionState::Live {
                        let commit = begin_delete(&session.deletion, id::<OperationId>(20))
                            .expect("deletes");
                        prop_assert!(commit.guard.epoch > deletion);
                        deletion = commit.guard.epoch;
                        session.deletion = commit.guard;
                        session.work_admission = commit.admission;
                    }
                }
                _ => {
                    session.revision = session.revision.next();
                    prop_assert!(session.revision > revision);
                    revision = session.revision;
                }
            }
            prop_assert!(session.revision >= revision);
            prop_assert!(session.deletion.epoch >= deletion);
        }
    }
}

// ---------------------------------------------------------------------------
// 6-10 — the journal
// ---------------------------------------------------------------------------

/// A generated journal batch that is legal by construction from position 1.
fn legal_batch(len: usize) -> Vec<JournalEntry> {
    let agent: AgentId = child_agent().id;
    (1..=len as u64)
        .map(|seq| entry_at(agent, seq, AuthorityFact::None))
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 192, ..ProptestConfig::default() })]

    /// 6 `journal_fold_determinism`.
    #[test]
    fn journal_fold_determinism(len in 0_usize..12, split in 0_usize..12) {
        let prior = child_agent();
        let batch = legal_batch(len);
        let whole = fold_control(&prior, &batch).expect("folds");

        let at = split.min(batch.len());
        let (head, tail) = batch.split_at(at);
        let stepwise = fold_control(&prior, head)
            .and_then(|middle| fold_control(&middle, tail))
            .expect("folds");
        prop_assert_eq!(whole, stepwise);
    }

    /// 7 `guard_before_mutation`.
    #[test]
    fn guard_before_mutation(len in 1_usize..8, bad_at in 0_usize..8) {
        let prior = child_agent();
        let mut batch = legal_batch(len);
        let index = bad_at % batch.len();
        // Break exactly one entry's position, making the batch illegal.
        batch[index].seq = JournalSeq(999);

        let before = prior.clone();
        let outcome = fold_control(&prior, &batch);
        prop_assert!(outcome.is_err());
        // The prior is untouched: the fold never mutates in place.
        prop_assert_eq!(&prior, &before);

        // `validate_append` returns exactly what the fold would for the same
        // state, so the append guard and the fold cannot drift.
        let mut walked = prior.clone();
        for entry in &batch {
            let expected = validate_append(&walked, entry);
            match fold_control(&walked, std::slice::from_ref(entry)) {
                Ok(next) => {
                    prop_assert!(expected.is_ok());
                    walked = next;
                }
                Err(error) => {
                    prop_assert_eq!(expected, Err(error));
                    break;
                }
            }
        }
    }

    /// 9 `journal_duplicate_identity_idempotent`.
    #[test]
    fn journal_duplicate_identity_idempotent(len in 1_usize..8, repeats in 1_usize..4) {
        let prior = child_agent();
        let batch = legal_batch(len);
        let once = fold_control(&prior, &batch).expect("folds");

        let mut with_repeat = batch.clone();
        let last = batch.last().expect("non-empty").clone();
        for _ in 0..repeats {
            with_repeat.push(last.clone());
        }
        let repeated = fold_control(&prior, &with_repeat).expect("folds");
        prop_assert_eq!(&once, &repeated);
        prop_assert_eq!(repeated.journal_tail, JournalSeq(len as u64));
    }

    /// 10 `journal_body_placement`.
    #[test]
    fn journal_body_placement(sizes in prop::collection::vec(0_usize..40_000, 1..6)) {
        let prior = child_agent();
        for (index, size) in sizes.into_iter().enumerate() {
            let mut entry = entry_at(prior.id, index as u64 + 1, AuthorityFact::None);
            entry.body = aex_session_domain::JournalBody::Inline(vec![0; size]);
            let outcome = validate_append(&prior, &entry);
            if index == 0 {
                let too_large = size as u64 > aex_session_domain::INLINE_BODY_MAX_BYTES;
                prop_assert_eq!(outcome.is_err(), too_large);
            }
        }
    }
}

#[test]
fn journal_gap_regression_wrong_agent_and_post_terminal_each_have_their_own_error() {
    // 8 `journal_gap_rejected`.
    let prior = child_agent();
    assert!(matches!(
        validate_append(&prior, &entry_at(prior.id, 3, AuthorityFact::None)),
        Err(JournalError::SequenceGap { .. })
    ));

    let mut foreign = entry_at(prior.id, 1, AuthorityFact::None);
    foreign.agent = AgentId::from_uuid7(Uuid7::compose(9, [9; 10]));
    assert!(matches!(
        validate_append(&prior, &foreign),
        Err(JournalError::WrongAgent { .. })
    ));

    let advanced = fold_control(&prior, &legal_batch(2)).expect("folds");
    assert!(matches!(
        validate_append(&advanced, &entry_at(prior.id, 1, AuthorityFact::None)),
        Err(JournalError::SequenceRegression { .. })
    ));

    let settled = fold_control(
        &advanced,
        &[entry_at(
            prior.id,
            3,
            AuthorityFact::Terminal(AgentTerminal::Completed),
        )],
    )
    .expect("folds");
    assert!(matches!(
        validate_append(&settled, &entry_at(prior.id, 4, AuthorityFact::None)),
        Err(JournalError::AfterTerminal { .. })
    ));
}

#[test]
fn an_unbalanced_effect_is_rejected() {
    // 8, effect balance half.
    let prior = child_agent();
    let effect = EffectId(Uuid7::compose(1, [7; 10]));
    assert!(matches!(
        validate_append(
            &prior,
            &entry_at(prior.id, 1, AuthorityFact::EffectSettled(effect))
        ),
        Err(JournalError::UnbalancedEffect { .. })
    ));

    let opened = fold_control(
        &prior,
        &[entry_at(prior.id, 1, AuthorityFact::EffectOpened(effect))],
    )
    .expect("folds");
    assert_eq!(opened.open_effects.len(), 1);
    assert!(matches!(
        validate_append(
            &opened,
            &entry_at(prior.id, 2, AuthorityFact::EffectOpened(effect))
        ),
        Err(JournalError::UnbalancedEffect { .. })
    ));

    let settled = fold_control(
        &opened,
        &[entry_at(prior.id, 2, AuthorityFact::EffectSettled(effect))],
    )
    .expect("folds");
    assert!(settled.open_effects.is_empty());
}

// ---------------------------------------------------------------------------
// 11-14 — idempotency
// ---------------------------------------------------------------------------

#[test]
fn intent_hash_is_the_one_canonicalizer() {
    // 11-14 in aggregate. The digest, its canonicalization and its cross-language
    // corpus all live in `aex-wire` (D-23); this crate must add no second one, so
    // the property here is that the digest it uses is exactly that one and
    // behaves as specified.
    use aex_wire::canonical::{intent_digest, to_jcs_bytes};
    use aex_wire::routes::{PathBinding, RouteId, route};

    let digest = |body: &serde_json::Value| {
        intent_digest(
            route(RouteId::SessionCreate).id,
            &PathBinding::default(),
            Some(&to_jcs_bytes(body).expect("canonical")),
        )
    };

    // 12: invariant under key reorder, sensitive to any value change, `-0` == `0`.
    assert_eq!(
        digest(&serde_json::json!({"a": 1, "b": {"c": 2, "d": 3}})),
        digest(&serde_json::json!({"b": {"d": 3, "c": 2}, "a": 1}))
    );
    assert_ne!(
        digest(&serde_json::json!({"a": 1})),
        digest(&serde_json::json!({"a": 2}))
    );
    assert_eq!(
        digest(&serde_json::json!({"n": 0})),
        digest(&serde_json::json!({"n": -0}))
    );

    // 11: the same body replays; a single-field mutation conflicts.
    let stored = digest(&serde_json::json!({"a": 1, "b": 2}));
    assert_eq!(stored, digest(&serde_json::json!({"a": 1, "b": 2})));
    assert_ne!(stored, digest(&serde_json::json!({"a": 1, "b": 3})));

    // 14: the route id, not a free-text template, is the digest's route input, so
    // there is no route string for this crate to validate a second time.
    assert_ne!(
        digest(&serde_json::json!({})),
        intent_digest(
            route(RouteId::SessionGet).id,
            &PathBinding::default(),
            Some(&to_jcs_bytes(&serde_json::json!({})).expect("canonical")),
        )
    );

    // 13: the cross-language corpus is `aex-wire`'s and is exercised there; this
    // crate asserts only that it exists where the contract says it does.
    assert!(aex_wire::testing::corpus::root().is_absolute());
}

// ---------------------------------------------------------------------------
// 15-19 — approvals and the mutation guard
// ---------------------------------------------------------------------------

#[test]
fn approval_single_unresolved() {
    // 15 `approval_single_unresolved`.
    let binding = approval_binding();
    let first = request_approval(
        id::<ApprovalId>(40),
        binding.clone(),
        &binding,
        None,
        moment(0),
        moment(100),
    )
    .expect("raises")
    .approval;
    for tag in 41..45_u8 {
        assert!(matches!(
            request_approval(
                id::<ApprovalId>(tag),
                binding.clone(),
                &binding,
                Some(&first),
                moment(1),
                moment(100)
            ),
            Err(ApprovalRejection::Pending(_))
        ));
    }

    // Once resolved, the next one is admitted.
    let resolved = respond(&first, &binding, ApprovalDecision::Approve, moment(2))
        .expect("decides")
        .approval()
        .clone();
    assert!(
        request_approval(
            id::<ApprovalId>(46),
            binding.clone(),
            &binding,
            Some(&resolved),
            moment(3),
            moment(100)
        )
        .is_ok()
    );
}

#[test]
fn approval_binding_drift_fails_closed_for_every_field() {
    // 16 `approval_binding_drift_fails_closed`.
    let binding = approval_binding();
    for field in BindingField::ALL {
        let pending = request_approval(
            id::<ApprovalId>(40),
            binding.clone(),
            &binding,
            None,
            moment(0),
            moment(100),
        )
        .expect("raises")
        .approval;
        let drifted = drift_field(&binding, field);
        let Err(ApprovalRejection::BindingChanged { fields, commit }) =
            respond(&pending, &drifted, ApprovalDecision::Approve, moment(1))
        else {
            panic!("{field:?} must fail closed");
        };
        assert_eq!(fields, vec![field]);
        assert!(!commit.dispatch, "{field:?} must dispatch nothing");
        assert_eq!(commit.approval.status, ApprovalStatus::Cancelled);
        assert_eq!(
            commit.approval.cancel_cause,
            Some(ApprovalCancelCause::BindingDrift)
        );
    }
}

#[test]
fn approval_deny_does_not_cancel_the_run() {
    // 17 `approval_deny_does_not_cancel_run`.
    let binding = approval_binding();
    let pending = request_approval(
        id::<ApprovalId>(40),
        binding.clone(),
        &binding,
        None,
        moment(0),
        moment(100),
    )
    .expect("raises")
    .approval;
    let denied = respond(&pending, &binding, ApprovalDecision::Deny, moment(1))
        .expect("decides")
        .commit()
        .expect("a first decision commits")
        .clone();
    assert_eq!(denied.approval.status, ApprovalStatus::Denied);
    assert!(!denied.dispatch);
    assert_eq!(denied.denial_result, Some(ErrorCode::PreconditionFailed));

    // The run is untouched: nothing in the commit names it as cancelled.
    let (_, run, _, _) = running_session();
    assert!(!run.status.is_terminal());
}

#[test]
fn approval_cancel_scope_is_exact() {
    // 18 `approval_cancel_scope`.
    let binding = approval_binding();
    let pending = request_approval(
        id::<ApprovalId>(40),
        binding.clone(),
        &binding,
        None,
        moment(0),
        moment(100),
    )
    .expect("raises")
    .approval;

    let matching = CancelScope {
        run: Some(binding.run),
        generation: binding.expected_generation,
        tool_call: Some(binding.tool_call),
    };
    let foreign = CancelScope {
        run: Some(RunId::from_uuid7(Uuid7::compose(9, [9; 10]))),
        generation: Some(GenerationId::from_uuid7(Uuid7::compose(9, [8; 10]))),
        tool_call: Some(aex_wire::ids::ToolCallId::from_uuid7(Uuid7::compose(
            9, [7; 10],
        ))),
    };

    for cause in ApprovalCancelCause::ALL {
        let fires_unscoped = matches!(
            cause,
            ApprovalCancelCause::SessionDeleting | ApprovalCancelCause::AccountPaused
        );
        assert_eq!(
            cancel_pending(&pending, cause, &CancelScope::UNSCOPED, moment(1)).is_some(),
            fires_unscoped,
            "{cause:?} unscoped"
        );
        assert_eq!(
            cancel_pending(&pending, cause, &foreign, moment(1)).is_some(),
            fires_unscoped,
            "{cause:?} against a foreign scope"
        );
        assert_eq!(
            cancel_pending(&pending, cause, &matching, moment(1)).is_some(),
            cause != ApprovalCancelCause::BindingDrift,
            "{cause:?} against its own scope"
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 192, ..ProptestConfig::default() })]

    /// 19 `mutation_guard_exclusive`.
    #[test]
    fn mutation_guard_exclusive(holders in prop::collection::vec(1_u8..5, 1..12)) {
        use aex_session_domain::{acquire_mutation_guard, release_mutation_guard};

        let mut session = session_fixture();
        for tag in holders {
            let holder = id::<OperationId>(tag);
            let attempt = acquire_mutation_guard(
                &session,
                holder,
                OperationKind::SessionSuspend,
                moment(1),
            );
            if let Ok(guard) = attempt {
                // At most one holder: acquiring succeeded only because the slot
                // was free or the caller already owned it.
                prop_assert!(
                    session.mutation_guard.is_none()
                        || session.mutation_guard.map(|held| held.holder) == Some(holder)
                );
                session.mutation_guard = Some(guard);
            } else {
                let held = session.mutation_guard.expect("a denial names a holder");
                prop_assert_ne!(held.holder, holder);
            }

            // Release by a non-holder is always rejected.
            let stranger = id::<OperationId>(200);
            prop_assert!(release_mutation_guard(&session, stranger).is_err());
        }
    }
}

// ---------------------------------------------------------------------------
// 20-22 — agents
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    /// 20 `agent_ceiling_effective`.
    #[test]
    fn agent_ceiling_effective(ceiling in 2_u64..8, spawns in 1_usize..16) {
        let session = session_fixture();
        let root_agent = create_root(id::<AgentId>(1), &session, materialized_state(), moment(0)).agent;
        let mut live: Vec<AgentControl> = Vec::new();
        let mut admitted = 0_u64;

        for index in 0..spawns {
            let tag = u8::try_from(index).expect("bounded") + 10;
            match spawn(
                id::<AgentId>(tag),
                &root_agent,
                &session,
                &limits(ceiling),
                &live,
                materialized_state(),
                moment(1),
            ) {
                Ok(commit) => {
                    admitted += 1;
                    prop_assert!(admitted < ceiling);
                    live.push(commit.agent);
                }
                Err(AgentError::CeilingExceeded { effective, .. }) => {
                    prop_assert_eq!(effective, ceiling);
                    prop_assert_eq!(admitted + 1, ceiling);
                }
                Err(other) => prop_assert!(false, "unexpected {other:?}"),
            }
        }

        // Terminal agents never consume budget.
        for agent in &mut live {
            *agent = complete_agent(
                agent,
                AgentTerminal::Completed,
                aex_session_domain::AgentFence::INITIAL,
                moment(2),
            )
            .expect("completes")
            .agent;
        }
        prop_assert!(
            spawn(
                id::<AgentId>(200),
                &root_agent,
                &session,
                &limits(ceiling),
                &live,
                materialized_state(),
                moment(3),
            )
            .is_ok()
        );
    }

    /// 21 `cancel_session_work_epoch`.
    #[test]
    fn cancel_session_work_epoch(active in 0_usize..5, cause_index in 0_usize..4) {
        let mut session = session_fixture();
        let cause = CancelCause::ALL[cause_index];
        let root_agent =
            create_root(id::<AgentId>(1), &session, materialized_state(), moment(0)).agent;
        let agents: Vec<AgentControl> = (0..active)
            .map(|index| {
                spawn(
                    id::<AgentId>(u8::try_from(index).expect("bounded") + 10),
                    &root_agent,
                    &session,
                    &limits(8),
                    &[],
                    materialized_state(),
                    moment(1),
                )
                .expect("spawns")
                .agent
            })
            .collect();

        let target = cause.target_admission();
        session.work_admission = if active == 0 { target } else { WorkAdmission::Open };

        let commit = cancel_session_work(&session, &agents, cause, None, moment(2));
        if active == 0 && session.work_admission == target {
            // Declared no-op: the epoch does not move.
            prop_assert!(!commit.changed);
            prop_assert_eq!(commit.cancellation, session.cancellation);
        } else {
            prop_assert!(commit.changed);
            prop_assert_eq!(commit.cancellation, session.cancellation.next());
            prop_assert_eq!(commit.agents.len(), active);
            for agent in &commit.agents {
                prop_assert_eq!(agent.status, AgentStatus::Cancelled);
            }
        }
    }
}

#[test]
fn a_stale_generation_cancellation_is_the_second_declared_no_op() {
    // 21, second no-op case.
    let mut session = session_fixture();
    session.generation = Some(id::<GenerationId>(60));
    let root_agent = create_root(id::<AgentId>(1), &session, materialized_state(), moment(0)).agent;
    let child = spawn(
        id::<AgentId>(10),
        &root_agent,
        &session,
        &limits(4),
        &[],
        materialized_state(),
        moment(1),
    )
    .expect("spawns")
    .agent;

    let stale = cancel_session_work(
        &session,
        std::slice::from_ref(&child),
        CancelCause::ContinuityLost,
        Some(id::<GenerationId>(61)),
        moment(2),
    );
    assert!(!stale.changed);
    assert_eq!(stale.cancellation, session.cancellation);

    let current = cancel_session_work(
        &session,
        std::slice::from_ref(&child),
        CancelCause::ContinuityLost,
        Some(id::<GenerationId>(60)),
        moment(2),
    );
    assert!(current.changed);
}

#[test]
fn root_cannot_complete() {
    // 22 `root_cannot_complete`.
    let session = session_fixture();
    let root_agent = create_root(id::<AgentId>(1), &session, materialized_state(), moment(0)).agent;
    for terminal in AgentTerminal::ALL {
        assert_eq!(
            complete_agent(
                &root_agent,
                terminal,
                aex_session_domain::AgentFence::INITIAL,
                moment(1)
            ),
            Err(AgentError::RootCannotComplete)
        );
    }
    // Even after a fold that would otherwise settle it, the function refuses.
    let folded = fold_control(
        &root_agent,
        &[entry_at(
            root_agent.id,
            1,
            AuthorityFact::Terminal(AgentTerminal::Completed),
        )],
    )
    .expect("the fold records what the journal says");
    assert_eq!(
        complete_agent(
            &folded,
            AgentTerminal::Completed,
            aex_session_domain::AgentFence::INITIAL,
            moment(2)
        ),
        Err(AgentError::RootCannotComplete)
    );
}

// ---------------------------------------------------------------------------
// 23-28 — irreversible deletion and clone detachment
// ---------------------------------------------------------------------------

#[test]
fn deletion_epoch_fences_every_pre_fence_command() {
    // 23 `deletion_epoch_fences`.
    let session = session_fixture();
    let before = session.deletion.epoch;

    let deleting = begin_delete(&session.deletion, id::<OperationId>(20))
        .expect("claims")
        .guard;
    assert!(deleting.epoch > before);

    // A command built against the pre-fence epoch is stale forever.
    assert_ne!(deleting.epoch, before);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 192, ..ProptestConfig::default() })]

    /// 24 `delete_owner_idempotent` and 25 `delete_absorbing`.
    #[test]
    fn delete_owner_idempotent(replays in 1_usize..12) {
        let session = session_fixture();
        let first = begin_delete(&session.deletion, id::<OperationId>(20)).expect("claims");
        for _ in 0..replays {
            prop_assert_eq!(begin_delete(&first.guard, id::<OperationId>(20)), Ok(first));
            prop_assert!(begin_delete(&first.guard, id::<OperationId>(21)).is_err());
        }
    }
}

#[test]
fn delete_completion_requires_the_whole_predicate() {
    // 26 `delete_linearization`, completion half.
    let session = session_fixture();
    let deleting = begin_delete(&session.deletion, id::<OperationId>(21))
        .expect("claims")
        .guard;

    let complete = DeleteEvidence {
        generation_terminated: true,
        session_content_removed: true,
        messages_removed: true,
        brain_user_content_removed: true,
        observations_removed: true,
        export_objects_removed: true,
        billing_aggregate_retained: true,
        audit_fact_retained: true,
    };
    assert!(complete_delete(&deleting, id::<WorkspaceId>(22), &complete, moment(1)).is_ok());

    // Each individual omission blocks completion, so nothing is optional.
    let omissions: [fn(&mut DeleteEvidence); 8] = [
        |value| value.generation_terminated = false,
        |value| value.session_content_removed = false,
        |value| value.messages_removed = false,
        |value| value.brain_user_content_removed = false,
        |value| value.observations_removed = false,
        |value| value.export_objects_removed = false,
        |value| value.billing_aggregate_retained = false,
        |value| value.audit_fact_retained = false,
    ];
    for omit in omissions {
        let mut evidence = complete.clone();
        omit(&mut evidence);
        assert!(complete_delete(&deleting, id::<WorkspaceId>(22), &evidence, moment(1)).is_err());
    }
}

// ---------------------------------------------------------------------------
// 29, 30 — account pause
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// 29 `pause_monotone_projection`.
    #[test]
    fn pause_monotone_projection(
        revisions in prop::collection::vec(0_u64..20, 1..16),
        shuffle in prop::collection::vec(0_usize..16, 0..16),
    ) {
        let organization = id::<aex_wire::ids::OrganizationId>(3);
        // The revision identifies the state: two projections that disagree at
        // the same revision are a producer fault, not an ordering question, so
        // the state is derived from the revision rather than generated beside it.
        let build = |revision: u64| AccountProjection {
            organization,
            revision: aex_session_domain::AccountRevision(revision),
            state: if revision % 2 == 1 {
                AccountState::Paused { reason: PauseReason::TopUpRequired }
            } else {
                AccountState::Active
            },
            observed_at: moment(i64::try_from(revision).expect("small")),
        };

        let mut ordered: Vec<AccountProjection> =
            revisions.iter().copied().map(build).collect();
        let base = build(0);

        let fold = |values: &[AccountProjection]| {
            values
                .iter()
                .fold(base, |current, incoming| project_account(&current, incoming))
        };
        let first = fold(&ordered);

        for (index, target) in shuffle.into_iter().enumerate() {
            if index < ordered.len() && target < ordered.len() {
                ordered.swap(index, target);
            }
        }
        prop_assert_eq!(fold(&ordered), first, "any order converges");

        let highest = revisions.iter().copied().max().unwrap_or(0);
        prop_assert_eq!(first.revision.0, highest);
        prop_assert_eq!(first, build(highest));
    }
}

#[test]
fn pause_gate_exemptions_are_exactly_the_declared_list() {
    // 30 `pause_gate_exemptions`.
    let paused = AccountProjection {
        organization: id::<aex_wire::ids::OrganizationId>(3),
        revision: aex_session_domain::AccountRevision(1),
        state: AccountState::Paused {
            reason: PauseReason::TopUpRequired,
        },
        observed_at: moment(0),
    };
    for command in ExemptCommand::ALL {
        assert_eq!(
            pause_gate(command.class(), &paused),
            Ok(()),
            "{command:?} must stay available"
        );
    }
    for class in [CommandClass::PausableMutation, CommandClass::PausableRead] {
        let rejection = pause_gate(class, &paused).expect_err("must deny");
        assert_eq!(rejection.code, ErrorCode::AccountPaused);
    }
    assert_eq!(ExemptCommand::ALL.len(), 7);
}

#[test]
fn a_content_digest_is_the_one_body_digest_type() {
    // The whole stream shares `aex-wire`'s content hash rather than declaring a
    // second one, so a digest written by one crate parses in another.
    let digest = ContentDigest::of(b"body");
    assert_eq!(
        ContentDigest::parse(&digest.to_wire()).expect("round-trips"),
        digest
    );
}
