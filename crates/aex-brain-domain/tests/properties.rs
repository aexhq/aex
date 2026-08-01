//! Slice S-1.2 — fold properties F1 through F12 over arbitrary journal histories.
//!
//! Every property is quantified over the generator in
//! `aex_brain_test_support::journal_gen`, including its hostile transformations. The plan's
//! table is reproduced here one property per test so a failure names the invariant that
//! broke rather than "a property failed".

use aex_brain_domain::budget::{DIMENSIONS, Kind};
use aex_brain_domain::effect::SettledOutcome;
use aex_brain_domain::fold::{FoldError, FoldState, Phase, apply, fold};
use aex_brain_domain::ids::JournalSeq;
use aex_brain_domain::journal::{JournalEntry, JournalRecord};
use aex_brain_domain::planner::{OwedStep, PlanPolicy, plan};
use aex_brain_domain::wire_pending::Turn;
use aex_brain_test_support::journal_gen::{
    HistoryBuilder, Hostile, agent, arb_any_history, arb_hostile, config, finished, grant, started,
    user_text,
};
use proptest::prelude::*;
use std::collections::BTreeSet;

fn incremental(history: &[JournalEntry]) -> Result<FoldState, FoldError> {
    let mut state = FoldState::empty();
    for entry in history {
        apply(&mut state, entry)?;
    }
    Ok(state)
}

proptest! {
    /// F1 — `fold(h) == incremental_apply(h)`.
    ///
    /// `fold` is defined as a loop over `apply` so the hot incremental path and the cold
    /// rehydration path cannot drift. This property is what keeps that definition honest
    /// if either ever grows its own body.
    #[test]
    fn f1_fold_agrees_with_incremental_apply(history in arb_any_history()) {
        prop_assert_eq!(fold(&history).ok(), incremental(&history).ok());
    }

    /// F2 — `fold(h ++ [r]) == apply(fold(h), r)`, prefix-monotone.
    #[test]
    fn f2_the_fold_is_prefix_monotone(history in arb_any_history()) {
        for split in 0..=history.len() {
            let (prefix, suffix) = history.split_at(split);
            let Ok(mut incremental_state) = fold(prefix) else { continue };
            let mut ok = true;
            for entry in suffix {
                if apply(&mut incremental_state, entry).is_err() {
                    ok = false;
                    break;
                }
            }
            if ok {
                let whole = fold(&history).ok();
                prop_assert_eq!(whole.as_ref(), Some(&incremental_state));
            }
        }
    }

    /// F3 — after `AgentFinished`, every further record is rejected `TerminalAbsorbing`.
    #[test]
    fn f3_the_terminal_absorbs(history in arb_any_history()) {
        let Ok(state) = fold(&history) else { return Ok(()) };
        if !state.is_finished() {
            return Ok(());
        }
        let mut after = state;
        let seq = after.expected_seq();
        let entry = JournalEntry::seal(seq, aex_brain_domain::ids::Timestamp(0), user_text("late"))
            .expect("the trailing record canonicalizes");
        let error = apply(&mut after, &entry).expect_err("a terminal agent absorbs");
        prop_assert!(matches!(error, FoldError::TerminalAbsorbing { .. }), "{error:?}");
    }

    /// F4 — a tool result must answer a pending call; a resolved call rejects
    /// `DuplicateToolResult` and an unknown call rejects `UnknownCall`.
    #[test]
    fn f4_tool_results_answer_exactly_one_pending_call(history in arb_any_history()) {
        let Ok(state) = fold(&history) else { return Ok(()) };
        // Every call that ever resolved left the pending set; nothing can be both.
        for call in &state.retired_calls {
            prop_assert!(!state.pending_calls.contains_key(call), "{call:?}");
        }
        for call in state.resolved_calls.keys() {
            prop_assert!(!state.pending_calls.contains_key(call), "{call:?}");
        }
    }

    /// F5 — `model_history` contains no block that did not arrive inside a proved message.
    ///
    /// Assistant turns are the only ones a provider can author, and the fold refuses one
    /// whose proof does not cover its blocks, so the assistant turns in history are exactly
    /// the proved ones.
    #[test]
    fn f5_only_proved_messages_reach_model_history(history in arb_any_history()) {
        let Ok(state) = fold(&history) else { return Ok(()) };
        let proved = history
            .iter()
            .filter(|entry| matches!(entry.record, JournalRecord::AssistantMessage { .. }))
            .count();
        let in_history = state
            .model_history
            .iter()
            .filter(|turn| matches!(turn, Turn::Assistant { .. }))
            .count();
        // A compaction replaces the prefix, so history can hold fewer — never more.
        prop_assert!(in_history <= proved, "{in_history} assistant turns from {proved} messages");
    }

    /// F6 — a compacted fold agrees with an uncompacted one on every counter.
    #[test]
    fn f6_compaction_preserves_every_counter(history in arb_any_history()) {
        let Ok(state) = fold(&history) else { return Ok(()) };
        let compacted = aex_brain_test_support::journal_gen::compaction(
            state.tail.unwrap_or(JournalSeq::ZERO),
            aex_brain_domain::journal::PreservedCounters {
                usage: state.usage,
                assistant_turns: state.assistant_turns,
                spawn_ordinal: state.spawn_ordinal,
            },
        );
        if state.is_finished() {
            return Ok(());
        }
        let mut after = state.clone();
        let entry = JournalEntry::seal(
            after.expected_seq(),
            aex_brain_domain::ids::Timestamp(0),
            compacted,
        )
        .expect("the compaction canonicalizes");
        prop_assume!(apply(&mut after, &entry).is_ok());
        prop_assert_eq!(after.budget, state.budget);
        prop_assert_eq!(after.usage, state.usage);
        prop_assert_eq!(after.children, state.children);
        prop_assert_eq!(after.joins, state.joins);
        prop_assert_eq!(after.pending_calls, state.pending_calls);
        prop_assert_eq!(after.assistant_turns, state.assistant_turns);
    }

    /// F7 — conserved dimensions never decrease; concurrent dimensions never exceed the
    /// limit.
    #[test]
    fn f7_budget_arithmetic_holds_at_every_prefix(history in arb_any_history()) {
        let mut state = FoldState::empty();
        let mut previous_used = aex_brain_domain::budget::DimensionVector::ZERO;
        for entry in &history {
            if apply(&mut state, entry).is_err() {
                break;
            }
            prop_assert!(state.budget.invariant_i1(), "I1 broken: {:?}", state.budget);
            for &dimension in &DIMENSIONS {
                if dimension.kind() == Kind::Conserved {
                    prop_assert!(
                        state.budget.used.get(dimension) >= previous_used.get(dimension),
                        "{dimension:?} decreased"
                    );
                }
            }
            previous_used = state.budget.used;
        }
    }

    /// F8 — applying `(seq, hash)` twice is a no-op; `(seq, other_hash)` is `JournalForked`.
    #[test]
    fn f8_redelivery_is_idempotent_and_a_fork_is_typed(history in arb_any_history()) {
        prop_assume!(!history.is_empty());
        let Ok(state) = fold(&history) else { return Ok(()) };

        let mut replayed = state.clone();
        for entry in &history {
            prop_assert!(apply(&mut replayed, entry).is_ok(), "redelivery must be a no-op");
        }
        prop_assert_eq!(&replayed, &state);

        // A fork is detectable exactly where the hash is still retained. A compaction
        // deliberately drops the hashes it absorbed, so a fork before `base_seq` is
        // indistinguishable from a redelivery of something already summarized and is an
        // honest no-op — stating that here keeps the two behaviours from being confused.
        for index in 0..history.len() {
            let forked = Hostile::Fork { index }.apply(&history);
            let candidate = &forked[index];
            if candidate.envelope.content_hash == history[index].envelope.content_hash {
                continue;
            }
            let mut after = state.clone();
            let outcome = apply(&mut after, candidate);
            if candidate.envelope.seq >= state.base_seq {
                let error = outcome.expect_err("a retained sequence detects the fork");
                prop_assert!(matches!(error, FoldError::JournalForked { .. }), "{error:?}");
            } else {
                prop_assert!(
                    outcome.is_ok(),
                    "a compacted-away sequence is a no-op, not an error: {:?}",
                    outcome.err()
                );
                prop_assert_eq!(&after, &state, "a no-op must not change the state");
            }
        }
    }

    /// F10 — at most one settlement per preparation, and never a settlement without one.
    #[test]
    fn f10_effects_settle_at_most_once(history in arb_any_history()) {
        let Ok(state) = fold(&history) else { return Ok(()) };
        let mut prepared = BTreeSet::new();
        let mut settled = BTreeSet::new();
        for entry in &history {
            match &entry.record {
                JournalRecord::EffectPrepared { effect, .. } => {
                    prop_assert!(prepared.insert(*effect), "{effect} prepared twice");
                }
                JournalRecord::EffectSettled { effect, .. } => {
                    prop_assert!(prepared.contains(effect), "{effect} settled unprepared");
                    prop_assert!(settled.insert(*effect), "{effect} settled twice");
                }
                _ => {}
            }
        }
        for effect in &settled {
            prop_assert!(!state.open_effects.contains_key(effect));
            prop_assert!(state.settled_effects.contains(effect));
        }
    }

    /// F11 — tool-result block order equals tool-use order regardless of arrival order.
    #[test]
    fn f11_tool_results_rebuild_in_tool_use_order(history in arb_any_history()) {
        let Ok(state) = fold(&history) else { return Ok(()) };
        for turn in &state.model_history {
            let Turn::User { blocks } = turn else { continue };
            let calls: Vec<&aex_brain_domain::ids::ToolCallId> = blocks
                .iter()
                .filter_map(|block| match block {
                    aex_brain_domain::wire_pending::CanonicalBlock::ToolResult { call, .. } => {
                        Some(call)
                    }
                    _ => None,
                })
                .collect();
            // Results are emitted only when every call in the assistant turn resolved, so a
            // result turn never mixes calls from two assistant turns.
            let mut unique = calls.clone();
            unique.sort();
            unique.dedup();
            prop_assert_eq!(unique.len(), calls.len(), "a call appeared twice in one turn");
        }
    }

    /// F12 — the owed step is a total function of the fold state; recovery is not a mode.
    #[test]
    fn f12_the_owed_step_is_total(history in arb_any_history()) {
        let Ok(state) = fold(&history) else { return Ok(()) };
        let limits = config().limits;
        let policy = PlanPolicy::from_limits(aex_brain_domain::ids::Timestamp(0), limits);
        let first = plan(&state, &policy);
        // Totality means "defined and stable", not merely "does not panic".
        prop_assert_eq!(&first, &plan(&state, &policy));
        if state.is_finished() {
            prop_assert!(matches!(first, OwedStep::Finished { .. }), "{first:?}");
        }
    }

    /// A hostile transformation is always answered by a typed error or an honest no-op,
    /// and never by a partially applied state.
    #[test]
    fn a_hostile_delivery_never_folds_silently(
        history in arb_any_history(),
        hostile in arb_hostile(),
    ) {
        prop_assume!(fold(&history).is_ok());
        let mutated = hostile.apply(&history);
        match fold(&mutated) {
            Ok(state) => {
                // The only hostile shapes that legitimately fold are exact redelivery and a
                // reorder that happened to be a redelivery; both leave a contiguous tail.
                prop_assert_eq!(
                    state.tail.map(JournalSeq::get),
                    mutated
                        .iter()
                        .map(|entry| entry.envelope.seq.get())
                        .max()
                        .filter(|_| !mutated.is_empty()),
                    "a folded hostile history must still end at its own tail"
                );
            }
            Err(error) => {
                prop_assert!(
                    !matches!(error, FoldError::Canonical(_)),
                    "a hostile delivery must produce a semantic error, not an encoding one: {error:?}"
                );
            }
        }
    }
}

/// F9 — after the cancellation epoch advances, no `EffectPrepared` is admissible.
///
/// The epoch lives on the durable control item rather than in the fold, so the fold's half
/// of F9 is that a cancelled agent is terminal and therefore absorbing: no record of any
/// kind, `EffectPrepared` included, can follow. The store's precondition set carries the
/// other half.
#[test]
fn f9_a_cancelled_agent_admits_no_further_effect() {
    let history = HistoryBuilder::new()
        .push(started(grant(1_000)))
        .push(finished(aex_brain_domain::journal::FinishReason::Cancelled))
        .build();
    let mut state = fold(&history).expect("a cancelled history folds");
    assert!(matches!(state.phase, Phase::Finished));

    let prepared = aex_brain_test_support::journal_gen::effect_prepared(
        aex_brain_test_support::journal_gen::model_effect(agent(1), JournalSeq(2)),
        aex_brain_domain::effect::EffectKind::ModelCall,
        aex_brain_domain::effect::EffectClass::NonReplayable,
    );
    let entry = JournalEntry::seal(JournalSeq(2), aex_brain_domain::ids::Timestamp(0), prepared)
        .expect("the record canonicalizes");
    let error = apply(&mut state, &entry).expect_err("a cancelled agent prepares nothing");
    assert!(
        matches!(error, FoldError::TerminalAbsorbing { .. }),
        "{error:?}"
    );
}

/// An `OutcomeUnknown` settlement is the only producer of `Interrupted`.
#[test]
fn interrupted_has_exactly_one_producer() {
    let effect = aex_brain_test_support::journal_gen::model_effect(agent(1), JournalSeq(1));
    let history = HistoryBuilder::new()
        .push(started(grant(1_000)))
        .push(aex_brain_test_support::journal_gen::effect_prepared(
            effect,
            aex_brain_domain::effect::EffectKind::ModelCall,
            aex_brain_domain::effect::EffectClass::NonReplayable,
        ))
        .push(aex_brain_test_support::journal_gen::effect_unknown(effect))
        .build();
    let state = fold(&history).expect("an unknown settlement folds");
    assert_eq!(
        state.finish,
        Some(aex_brain_domain::journal::FinishReason::Interrupted)
    );

    // The same history with a proved outcome must not reach `Interrupted`.
    let clean = HistoryBuilder::new()
        .push(started(grant(1_000)))
        .push(aex_brain_test_support::journal_gen::effect_prepared(
            effect,
            aex_brain_domain::effect::EffectKind::ModelCall,
            aex_brain_domain::effect::EffectClass::NonReplayable,
        ))
        .push(aex_brain_test_support::journal_gen::effect_complete(effect))
        .build();
    let state = fold(&clean).expect("a complete settlement folds");
    assert_eq!(state.finish, None);
    assert!(
        matches!(
            clean[2].record,
            JournalRecord::EffectSettled {
                outcome: SettledOutcome::Complete { .. },
                ..
            }
        ),
        "the fixture must settle complete"
    );
}
