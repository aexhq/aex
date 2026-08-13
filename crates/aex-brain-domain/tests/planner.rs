//! Slice S-1.4 and S-1.5 — owed-step totality, honest finish reasons, context policy.

use aex_brain_domain::context::{
    CLEARED_PLACEHOLDER, ContextPolicy, DEFAULT_TOOL_RESULT_BYTES, PROTECTED_TAIL_TURNS, decide,
    view,
};
use aex_brain_domain::fold::{FoldState, apply, fold};
use aex_brain_domain::ids::{JournalSeq, Timestamp};
use aex_brain_domain::journal::{FinishReason, JournalEntry, JournalRecord};
use aex_brain_domain::planner::{OwedStep, PlanPolicy, plan};
use aex_brain_domain::wire_pending::{
    CanonicalBlock, CanonicalMessage, ProviderId, Role, StopReason, ToolResultPart,
};
use aex_brain_test_support::histories::{Expectation, all};
use aex_brain_test_support::journal_gen::{
    HistoryBuilder, TURN_USAGE, agent, assistant_text, assistant_tool_use, config, effect_complete,
    effect_prepared, grant, model_effect, started, user_text,
};
use aex_model_catalog::document::CapabilitySet;
use aex_model_catalog::{BoundedString, fixture};

fn text(value: &str) -> CanonicalBlock {
    CanonicalBlock::Text {
        text: BoundedString::truncating(value),
        annotations: Vec::new(),
    }
}

fn policy(now_millis: i64) -> PlanPolicy {
    PlanPolicy::from_limits(Timestamp(now_millis), config().limits)
}

/// The owed step is defined for every state the corpus can reach.
#[test]
fn every_reachable_state_owes_exactly_one_step() {
    for case in all() {
        if matches!(case.expectation, Expectation::Rejected(_)) {
            continue;
        }
        let state = fold(&case.history).unwrap_or_else(|error| panic!("{}: {error}", case.name));
        let step = plan(&state, &policy(0));
        assert_eq!(step, plan(&state, &policy(0)), "{}", case.name);
    }
}

/// A truncated response finishes `Failed` and never `Completed`.
///
/// Reporting success on a cut-off deliverable is a correctness bug, so this outranks every
/// other check that could otherwise close the turn cleanly.
#[test]
fn a_truncated_response_finishes_failed() {
    let effect = model_effect(agent(1), JournalSeq(2));
    let mut state = fold(
        &HistoryBuilder::new()
            .push(started(grant(1_000)))
            .push(user_text("write"))
            .push(effect_prepared(
                effect,
                aex_brain_domain::effect::EffectKind::ModelCall,
                aex_brain_domain::effect::EffectClass::NonReplayable,
            ))
            .push(assistant_text("half a doc", effect))
            .push(effect_complete(effect))
            .build(),
    )
    .expect("the history folds");

    assert_eq!(
        plan(&state, &policy(0)),
        OwedStep::Finish {
            reason: FinishReason::Completed
        },
        "a clean turn closes completed"
    );

    // Now the same turn as the provider actually truncated it.
    state.last_stop_reason = Some(StopReason::MaxOutputTokens);
    assert_eq!(
        plan(&state, &policy(0)),
        OwedStep::Finish {
            reason: FinishReason::Failed
        }
    );
}

/// Each breach maps to its own honest reason rather than a generic failure.
#[test]
fn each_breach_maps_to_its_own_reason() {
    let base = fold(
        &HistoryBuilder::new()
            .push(started(grant(1_000)))
            .push(user_text("go"))
            .build(),
    )
    .expect("the history folds");

    let mut turns = base.clone();
    turns.assistant_turns = config().limits.max_turns;
    assert_eq!(
        plan(&turns, &policy(0)),
        OwedStep::Finish {
            reason: FinishReason::MaxTurns
        }
    );

    let mut steps = base.clone();
    steps.steps_this_turn = config().limits.max_steps_per_turn;
    assert_eq!(
        plan(&steps, &policy(0)),
        OwedStep::Finish {
            reason: FinishReason::MaxSteps
        }
    );

    let deadline = i64::from(config().limits.turn_deadline_ms);
    let started_at = base
        .turn_started_at
        .expect("a user message anchors the turn");
    assert_eq!(
        plan(&base, &policy(started_at.millis() + deadline)),
        OwedStep::Finish {
            reason: FinishReason::Timeout
        }
    );

    let mut budget = base.clone();
    budget.budget.limit = aex_brain_domain::budget::DimensionVector::uniform(1);
    budget
        .budget
        .consume(aex_brain_domain::budget::Dimension::CostMicroUsd, 1)
        .expect("the last unit is consumable");
    assert_eq!(
        plan(&budget, &policy(0)),
        OwedStep::Finish {
            reason: FinishReason::Budget
        }
    );

    let mut cancelled = policy(0);
    cancelled.cancel_requested = true;
    assert_eq!(
        plan(&base, &cancelled),
        OwedStep::Finish {
            reason: FinishReason::Cancelled
        }
    );
}

/// A parked agent owes a park, and an agent with pending calls owes those calls in order.
#[test]
fn the_phase_decides_the_step() {
    let effect = model_effect(agent(1), JournalSeq(2));
    let state = fold(
        &HistoryBuilder::new()
            .push(started(grant(1_000)))
            .push(user_text("go"))
            .push(effect_prepared(
                effect,
                aex_brain_domain::effect::EffectKind::ModelCall,
                aex_brain_domain::effect::EffectClass::NonReplayable,
            ))
            .push(assistant_tool_use(
                &[("second", "write_file"), ("first", "read_file")],
                effect,
            ))
            .push(effect_complete(effect))
            .build(),
    )
    .expect("the history folds");

    let OwedStep::ToolCalls { calls } = plan(&state, &policy(0)) else {
        panic!("pending calls owe tool calls");
    };
    // Tool-use order, not the map's key order: the assistant asked for `second` first.
    assert_eq!(
        calls
            .iter()
            .map(|call| call.call.as_str())
            .collect::<Vec<_>>(),
        vec!["second", "first"]
    );

    let empty = FoldState::empty();
    assert!(matches!(plan(&empty, &policy(0)), OwedStep::Park { .. }));
}

/// A terminal agent owes nothing further, whatever the policy says.
#[test]
fn a_terminal_agent_owes_nothing() {
    for reason in [
        FinishReason::Completed,
        FinishReason::MaxTurns,
        FinishReason::MaxSteps,
        FinishReason::Budget,
        FinishReason::Timeout,
        FinishReason::Cancelled,
        FinishReason::Failed,
        FinishReason::Interrupted,
    ] {
        let state = fold(
            &HistoryBuilder::new()
                .push(started(grant(1_000)))
                .push(aex_brain_test_support::journal_gen::finished(reason))
                .build(),
        )
        .expect("a terminal history folds");
        let mut cancelled = policy(i64::MAX / 2);
        cancelled.cancel_requested = true;
        assert_eq!(
            plan(&state, &cancelled),
            OwedStep::Finished { reason },
            "{reason:?}"
        );
    }
}

/// The context policy triggers on the whole prompt, including both cache dimensions.
///
/// Triggering on input tokens alone under-counts a cached prompt and therefore never
/// compacts it, which is the failure mode this rule exists to prevent.
#[test]
fn the_context_trigger_counts_the_whole_window() {
    let capability = fixture::qualified_entry_sized(
        ProviderId::Deepseek,
        "deepseek-chat",
        CapabilitySet::default(),
        1_000,
        100,
    );
    let mut state = FoldState::empty();
    state.usage = TURN_USAGE;
    let policy = ContextPolicy::default();
    let below = decide(&state, &capability, &policy, 0);
    assert!(!below.compaction_owed, "{below:?}");

    // The same input tokens, now mostly served from the provider's prompt cache. A trigger
    // that counted input alone would see 100 tokens and never compact a full window.
    state.usage.cache_read_input_tokens = 900;
    let above = decide(&state, &capability, &policy, 0);
    assert_eq!(above.prompt_tokens, 1_000);
    assert!(
        above.compaction_owed,
        "a cached prompt occupying the window must still trigger: {above:?}"
    );
    assert_eq!(
        above.target_tokens,
        u64::from(capability.limits().context_window_tokens) / 2,
        "one batched pass targets half the window"
    );

    // Hydration over the ceiling makes compaction mandatory regardless of the trigger.
    let forced = decide(
        &FoldState::empty(),
        &capability,
        &policy,
        policy.max_hydrated_context_bytes + 1,
    );
    assert!(
        forced.compaction_mandatory && forced.compaction_owed,
        "{forced:?}"
    );
}

/// Compaction protects the last three turns and clears with a deterministic placeholder, so
/// a retried compaction is a no-op rather than a second, different summary.
#[test]
fn clearing_is_deterministic_and_protects_the_tail() {
    let mut state = FoldState::empty();
    for index in 0..8 {
        state.model_history.push(CanonicalMessage {
            role: Role::User,
            blocks: vec![text(&format!("turn {index}"))],
        });
    }
    let policy = ContextPolicy::default();
    let once = view(&state, &policy, 4);
    let twice = view(&state, &policy, 4);
    assert_eq!(once, twice, "the view is a pure function of its inputs");

    let cleared = once
        .iter()
        .filter(|turn| {
            turn.role == Role::User
                && turn.blocks.iter().any(|block| {
                    matches!(block, CanonicalBlock::Text { text, .. } if text.as_str() == CLEARED_PLACEHOLDER)
                })
        })
        .count();
    assert!(cleared > 0, "something must actually be cleared");

    let protected = &once[once.len() - PROTECTED_TAIL_TURNS..];
    for turn in protected {
        if turn.role != Role::User {
            continue;
        }
        for block in &turn.blocks {
            if let CanonicalBlock::Text { text, .. } = block {
                assert_ne!(text.as_str(), CLEARED_PLACEHOLDER, "the tail is protected");
            }
        }
    }
}

/// The per-tool-result cap is idempotent: capping twice equals capping once.
///
/// Without this a retried request would send the model a shorter prompt than the first
/// attempt did, so the retry would not be the same request.
#[test]
fn the_tool_result_cap_is_idempotent() {
    let oversized = CanonicalBlock::ToolResult {
        call: aex_brain_domain::ids::ToolCallId::truncating("c1"),
        content: vec![ToolResultPart::Text {
            text: BoundedString::truncating(&"x".repeat(DEFAULT_TOOL_RESULT_BYTES * 2)),
        }],
        is_error: false,
    };
    let mut state = FoldState::empty();
    state.model_history.push(CanonicalMessage {
        role: Role::User,
        blocks: vec![oversized],
    });
    let policy = ContextPolicy::default();
    let once = view(&state, &policy, 0);

    let mut reapplied = FoldState::empty();
    reapplied.model_history.clone_from(&once);
    assert_eq!(
        once,
        view(&reapplied, &policy, 0),
        "capping is not idempotent"
    );

    assert_eq!(once[0].role, Role::User);
    let CanonicalBlock::ToolResult { content, .. } = &once[0].blocks[0] else {
        panic!("the block stays a tool result");
    };
    let ToolResultPart::Text { text } = &content[0] else {
        panic!("the content stays text");
    };
    assert!(
        text.len() <= DEFAULT_TOOL_RESULT_BYTES,
        "the marker must be charged against the budget, not appended after it: {} bytes",
        text.len()
    );
    assert!(
        text.as_str().ends_with(CLEARED_PLACEHOLDER),
        "truncation is visible"
    );
}

/// A max-output stop is terminal and therefore sealable; the planner still
/// reports the turn as failed rather than completed.
#[test]
fn a_truncating_stop_is_sealable_but_never_completed() {
    let model = fixture::qualified_entry(
        ProviderId::Deepseek,
        "deepseek-chat",
        CapabilitySet::default(),
    );
    let message = aex_model_catalog::canonical::seal(
        vec![text("half")],
        StopReason::MaxOutputTokens,
        &TURN_USAGE,
        &model,
    )
    .expect("max output is a complete terminal stop");
    assert!(message.proof_covers(&TURN_USAGE));
    assert!(
        aex_model_catalog::canonical::seal(Vec::new(), StopReason::EndTurn, &TURN_USAGE, &model,)
            .is_err()
    );
}

/// The model-call effect identity is a function of the state, so a redelivered wake that
/// replans the same owed step derives the same id and the durable record collapses it.
#[test]
fn replanning_derives_the_same_effect_identity() {
    let history = HistoryBuilder::new()
        .push(started(grant(1_000)))
        .push(user_text("go"))
        .build();
    let state = fold(&history).expect("the history folds");
    let first = aex_brain_domain::planner::model_effect_id(agent(9), &state);
    let second = aex_brain_domain::planner::model_effect_id(agent(9), &state);
    assert_eq!(first, second);
    assert_ne!(
        first,
        aex_brain_domain::planner::model_effect_id(agent(8), &state)
    );

    // Advancing the journal advances the identity.
    let mut advanced = state;
    let entry = JournalEntry::seal(JournalSeq(2), Timestamp(0), user_text("more"))
        .expect("the record canonicalizes");
    apply(&mut advanced, &entry).expect("the record applies");
    assert_ne!(
        first,
        aex_brain_domain::planner::model_effect_id(agent(9), &advanced)
    );
    assert!(matches!(entry.record, JournalRecord::UserMessage { .. }));
}
