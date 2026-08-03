//! Slice S-1.7 — the mutation gate.
//!
//! Each case names one guard and proves it is *load-bearing*: the guarded input produces
//! the named typed error **and** leaves the state untouched. Deleting the guard would make
//! the second half observable — the record would fold and the state would move — so a case
//! here fails the moment its guard is removed.
//!
//! "The state did not move" is asserted rather than assumed, because a guard that rejects
//! after a partial mutation is exactly as broken as no guard at all: the caller sees an
//! error, reloads, and finds a journal that already absorbed the record it was told was
//! refused.

use aex_brain_domain::budget::{BudgetNode, Dimension, DimensionVector, StructuralLimits};
use aex_brain_domain::fold::{FoldError, FoldState, apply, fold};
use aex_brain_domain::ids::{ContentHash, JournalSeq, Timestamp};
use aex_brain_domain::journal::{FinishReason, JournalEntry, JournalRecord};
use aex_brain_test_support::journal_gen::{
    HistoryBuilder, agent, assistant_text, assistant_tool_use, effect_complete, effect_prepared,
    finished, grant, model_effect, only, started, tool_result, user_text,
};

/// Applies `intruder` to the fold of `history` and returns the state before, the state
/// after the refused application, and the error.
fn probe(
    history: &[JournalEntry],
    intruder: JournalRecord,
    seq: JournalSeq,
) -> (FoldState, FoldState, FoldError) {
    let before = fold(history).expect("the setup history folds");
    let mut after = before.clone();
    let entry =
        JournalEntry::seal(seq, Timestamp(0), intruder).expect("the intruder canonicalizes");
    let error = apply(&mut after, &entry).expect_err("the guard fires");
    (before, after, error)
}

/// The sequence guard: a gap is typed and the state does not move.
#[test]
fn the_sequence_guard_is_load_bearing() {
    let history = HistoryBuilder::new()
        .push(started(grant(1_000)))
        .push(user_text("one"))
        .build();
    let (before, after, error) = probe(&history, user_text("skipped ahead"), JournalSeq(5));
    assert!(matches!(error, FoldError::JournalGap { .. }), "{error:?}");
    assert_eq!(before, after, "a gap must not advance the fold");
}

/// The fork guard: the same sequence under a different hash quarantines and does not fold.
#[test]
fn the_fork_guard_is_load_bearing() {
    let history = HistoryBuilder::new()
        .push(started(grant(1_000)))
        .push(user_text("one"))
        .build();
    let (before, after, error) = probe(&history, user_text("a torn read"), JournalSeq(1));
    assert!(
        matches!(error, FoldError::JournalForked { .. }),
        "{error:?}"
    );
    assert_eq!(before, after, "a fork must not fold");
}

/// The terminal guard: nothing follows the absorbing terminal.
#[test]
fn the_terminal_guard_is_load_bearing() {
    let history = HistoryBuilder::new()
        .push(started(grant(1_000)))
        .push(finished(FinishReason::Completed))
        .build();
    let (before, after, error) = probe(&history, user_text("too late"), JournalSeq(2));
    assert!(
        matches!(error, FoldError::TerminalAbsorbing { .. }),
        "{error:?}"
    );
    assert_eq!(before, after, "a terminal agent must not move");
    assert_eq!(after.finish, Some(FinishReason::Completed));
}

/// The envelope-hash guard: a record its envelope does not cover is refused.
#[test]
fn the_envelope_hash_guard_is_load_bearing() {
    let history = HistoryBuilder::new().push(started(grant(1_000))).build();
    let before = fold(&history).expect("the setup history folds");
    let mut after = before.clone();
    let mut entry = JournalEntry::seal(JournalSeq(1), Timestamp(0), user_text("one"))
        .expect("the record canonicalizes");
    entry.envelope.content_hash = ContentHash::of(b"a different record entirely");
    let error = apply(&mut after, &entry).expect_err("the guard fires");
    assert!(
        matches!(error, FoldError::EnvelopeHashMismatch { .. }),
        "{error:?}"
    );
    assert_eq!(before, after);
}

/// The completeness guard: an assistant message whose proof does not cover its blocks never
/// enters model-visible history.
#[test]
fn the_completeness_guard_is_load_bearing() {
    use aex_brain_domain::wire_pending::CanonicalBlock;
    use aex_model_catalog::BoundedString;

    let history = HistoryBuilder::new()
        .push(started(grant(1_000)))
        .push(user_text("go"))
        .build();
    let mut intruder = assistant_text(
        "what the proof covers",
        model_effect(agent(1), JournalSeq(2)),
    );
    let JournalRecord::AssistantMessage { message, .. } = &mut intruder else {
        unreachable!("assistant_text always returns an assistant message")
    };
    message.blocks = vec![CanonicalBlock::Text {
        text: BoundedString::truncating("what actually arrived"),
        annotations: Vec::new(),
    }];
    let (before, after, error) = probe(&history, intruder, JournalSeq(2));
    assert!(
        matches!(error, FoldError::UnprovenAssistantMessage { .. }),
        "{error:?}"
    );
    assert_eq!(before, after);
    assert!(after.model_history.is_empty(), "nothing may have leaked in");
}

/// A receipt for another response cannot authorize an otherwise valid
/// assistant message, and the refusal happens before model history moves.
#[test]
fn the_provider_receipt_guard_is_load_bearing() {
    let history = HistoryBuilder::new()
        .push(started(grant(1_000)))
        .push(user_text("go"))
        .build();
    let mut intruder = assistant_text("a complete response", model_effect(agent(1), JournalSeq(2)));
    let JournalRecord::AssistantMessage { receipt, .. } = &mut intruder else {
        unreachable!("assistant_text always returns an assistant message")
    };
    receipt.response_receipt = Some(aex_wire::ContentHash::of(b"another response"));
    let (before, after, error) = probe(&history, intruder, JournalSeq(2));
    assert!(
        matches!(error, FoldError::ReceiptMismatch { .. }),
        "{error:?}"
    );
    assert_eq!(before, after);
    assert!(after.model_history.is_empty(), "nothing may have leaked in");
}

/// A response proof inside a receipt does not make a non-success HTTP response
/// admissible as an assistant message.
#[test]
fn the_provider_receipt_success_shape_is_load_bearing() {
    let history = HistoryBuilder::new()
        .push(started(grant(1_000)))
        .push(user_text("go"))
        .build();
    let mut intruder = assistant_text("a complete response", model_effect(agent(1), JournalSeq(2)));
    let JournalRecord::AssistantMessage { receipt, .. } = &mut intruder else {
        unreachable!("assistant_text always returns an assistant message")
    };
    receipt.http_status = 500;
    let (before, after, error) = probe(&history, intruder, JournalSeq(2));
    assert!(
        matches!(error, FoldError::ReceiptMismatch { .. }),
        "{error:?}"
    );
    assert_eq!(before, after);
}

/// The pending-call guard: a result for a call the model never made is refused.
#[test]
fn the_pending_call_guard_is_load_bearing() {
    let effect = model_effect(agent(1), JournalSeq(2));
    let history = HistoryBuilder::new()
        .push(started(grant(1_000)))
        .push(user_text("go"))
        .push(effect_prepared(
            effect,
            aex_brain_domain::effect::EffectKind::ModelCall,
            aex_brain_domain::effect::EffectClass::NonReplayable,
        ))
        .push(assistant_tool_use(&[("c1", "read_file")], effect))
        .push(effect_complete(effect))
        .build();
    let (before, after, error) = probe(
        &history,
        tool_result("phantom", "result", effect),
        JournalSeq(5),
    );
    assert!(matches!(error, FoldError::UnknownCall { .. }), "{error:?}");
    assert_eq!(before, after);
    assert_eq!(after.pending_calls.len(), 1, "the real call is untouched");
}

/// The duplicate-result guard: a resolved call cannot be answered twice.
#[test]
fn the_duplicate_result_guard_is_load_bearing() {
    let effect = model_effect(agent(1), JournalSeq(2));
    let history = HistoryBuilder::new()
        .push(started(grant(1_000)))
        .push(user_text("go"))
        .push(effect_prepared(
            effect,
            aex_brain_domain::effect::EffectKind::ModelCall,
            aex_brain_domain::effect::EffectClass::NonReplayable,
        ))
        .push(assistant_tool_use(&[("c1", "read_file")], effect))
        .push(effect_complete(effect))
        .push(tool_result("c1", "once", effect))
        .build();
    let (before, after, error) = probe(&history, tool_result("c1", "twice", effect), JournalSeq(6));
    assert!(
        matches!(error, FoldError::DuplicateToolResult { .. }),
        "{error:?}"
    );
    assert_eq!(before, after);
}

/// The effect-pairing guard: a settlement without a preparation is refused.
#[test]
fn the_effect_pairing_guard_is_load_bearing() {
    let history = HistoryBuilder::new().push(started(grant(1_000))).build();
    let (before, after, error) = probe(
        &history,
        effect_complete(model_effect(agent(1), JournalSeq(1))),
        JournalSeq(1),
    );
    assert!(
        matches!(error, FoldError::UnknownEffect { .. }),
        "{error:?}"
    );
    assert_eq!(before, after);
    assert!(after.settled_effects.is_empty());
}

/// The budget guard: a spawn with no headroom is refused, not clamped, and reserves nothing.
#[test]
fn the_budget_guard_is_load_bearing() {
    let mut node = BudgetNode::root(only(Dimension::ActiveChildren, 2));
    let before = node;
    let error = node
        .spawn(
            only(Dimension::ActiveChildren, 3),
            StructuralLimits::default(),
        )
        .expect_err("three of two is refused");
    assert!(
        matches!(
            error,
            aex_brain_domain::budget::BudgetError::Exhausted { .. }
        ),
        "{error:?}"
    );
    assert_eq!(
        before, node,
        "a refused spawn must reserve nothing: a clamped grant would silently under-serve"
    );
    assert!(node.invariant_i1());
}

/// The consumption guard: over-consuming is refused and charges nothing.
#[test]
fn the_consumption_guard_is_load_bearing() {
    let mut node = BudgetNode::root(DimensionVector::uniform(5));
    node.consume(Dimension::CostMicroUsd, 5).expect("five fits");
    let before = node;
    node.consume(Dimension::CostMicroUsd, 1)
        .expect_err("the sixth does not");
    assert_eq!(before, node, "a refused charge must not partially apply");
}

/// The structural guard: depth is checked before the grant is reserved.
#[test]
fn the_depth_guard_runs_before_any_reservation() {
    let structural = StructuralLimits {
        max_depth: 0,
        max_fanout: 8,
    };
    let mut node = BudgetNode::root(DimensionVector::uniform(100));
    let before = node;
    let error = node
        .spawn(DimensionVector::uniform(1), structural)
        .expect_err("depth 1 exceeds a zero limit");
    assert!(
        matches!(
            error,
            aex_brain_domain::budget::BudgetError::DepthExceeded { .. }
        ),
        "{error:?}"
    );
    assert_eq!(
        before, node,
        "a depth rejection must not leave a reservation behind"
    );
}

/// The start guard: nothing folds before `agent_started`.
#[test]
fn the_start_guard_is_load_bearing() {
    let mut state = FoldState::empty();
    let entry = JournalEntry::seal(JournalSeq(0), Timestamp(0), user_text("orphan"))
        .expect("the record canonicalizes");
    let error = apply(&mut state, &entry).expect_err("the guard fires");
    assert!(matches!(error, FoldError::NotStarted { .. }), "{error:?}");
    assert_eq!(state, FoldState::empty());
}
