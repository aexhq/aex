//! Slice S-2.1 — `DecisionCommit::validate()` rejects an over-large transaction before AWS
//! does.
//!
//! A transaction rejected by `DynamoDB` for size gives no usable diagnosis and costs a round
//! trip, so the envelope rule is enforced here and the caller pages instead.

use aex_brain_domain::budget::{BudgetDelta, Dimension, DimensionVector};
use aex_brain_domain::commit::{
    ChildWrite, ControlUpdate, DecisionCommit, EnvelopeViolation, FenceGuardRef, MAX_ITEM_BYTES,
    MAX_TRANSACTION_ACTIONS, MAX_TRANSACTION_BYTES, SPAWN_PAGE_CHILDREN, event_seq, fanout_pages,
};
use aex_brain_domain::ids::{
    AgentId, AgentKey, AgentRevision, CancelEpoch, FanoutIntentId, Fence, JoinId, JournalSeq,
    OwnerToken, SessionId,
};
use aex_brain_domain::journal::{FinishReason, JournalRecord, TypedFailure};
use uuid::Uuid;

fn guard(tail: Option<JournalSeq>) -> FenceGuardRef {
    FenceGuardRef {
        key: AgentKey::new(SessionId(Uuid::from_u128(1)), AgentId(Uuid::from_u128(2))),
        owner: OwnerToken(Uuid::from_u128(3)),
        fence: Fence(1),
        revision: AgentRevision(4),
        tail,
        cancel_epoch: CancelEpoch::ZERO,
    }
}

fn decision(appends: Vec<JournalRecord>, children: Vec<ChildWrite>) -> DecisionCommit {
    let next_tail = JournalSeq(appends.len().max(1) as u64 - 1);
    DecisionCommit {
        guard: guard(None),
        appends,
        control: ControlUpdate {
            next_revision: AgentRevision(5),
            next_tail,
            phase: "awaiting_model".to_owned(),
            finish: None,
        },
        effects: Vec::new(),
        budget: Vec::new(),
        session_budget: Vec::new(),
        children,
        joins: Vec::new(),
        wakes: Vec::new(),
        events: Vec::new(),
        run: None,
        session: None,
        idempotency: None,
    }
}

fn spawn(ordinal: u32) -> ChildWrite {
    ChildWrite::Spawn {
        child: AgentId(Uuid::from_u128(u128::from(ordinal) + 100)),
        ordinal,
        grant: DimensionVector::uniform(1),
        join: JoinId(Uuid::from_u128(7)),
        queued_reason: None,
    }
}

fn finished() -> JournalRecord {
    JournalRecord::AgentFinished {
        reason: FinishReason::Completed,
        failure: None,
    }
}

#[test]
fn the_action_ceiling_is_the_dynamodb_one() {
    assert_eq!(MAX_TRANSACTION_ACTIONS, 100);
    assert_eq!(MAX_TRANSACTION_BYTES, 4 * 1_024 * 1_024);
    assert_eq!(MAX_ITEM_BYTES, 256 * 1_024);
}

#[test]
fn a_minimal_decision_costs_its_appends_plus_one_control_update() {
    let cost = decision(vec![finished()], Vec::new())
        .validate()
        .expect("one append plus a control update fits");
    assert_eq!(cost.actions, 2);
}

/// A real spawn page: the parent control update, the fanout intent, `n` children at three
/// items each, and the session budget update.
fn spawn_page(children: u32) -> DecisionCommit {
    let mut writes = vec![ChildWrite::FanoutIntent {
        intent: FanoutIntentId(Uuid::from_u128(11)),
        total: children,
        grant_template: DimensionVector::uniform(1),
    }];
    writes.extend((0..children).map(spawn));
    let mut page = decision(Vec::new(), writes);
    page.session_budget = vec![BudgetDelta::new(
        Dimension::ActiveChildren,
        u64::from(children),
    )];
    page
}

#[test]
fn the_page_size_is_derived_from_the_action_envelope_not_guessed() {
    // A child costs a control put, an index put and a queued-index put; the page also
    // carries the parent control update, the fanout intent and the session budget update.
    // That is `3n + 3 <= 100`, so 32. This asserts the derivation, not the constant.
    let full = spawn_page(SPAWN_PAGE_CHILDREN)
        .validate()
        .expect("the derived page size fits one transaction");
    assert_eq!(full.actions, 3 * SPAWN_PAGE_CHILDREN as usize + 3);
    assert!(full.actions <= MAX_TRANSACTION_ACTIONS, "{full:?}");

    let widest = (0..64_u32)
        .take_while(|children| spawn_page(*children).validate().is_ok())
        .last()
        .expect("at least one page size fits");
    assert_eq!(
        widest, SPAWN_PAGE_CHILDREN,
        "the constant must equal the largest page that actually fits"
    );
}

#[test]
fn a_decision_over_the_action_ceiling_is_rejected_before_aws_sees_it() {
    let error = spawn_page(SPAWN_PAGE_CHILDREN + 1)
        .validate()
        .expect_err("one child over the derived page does not fit");
    assert!(
        matches!(error, EnvelopeViolation::TooManyActions { actions } if actions > MAX_TRANSACTION_ACTIONS),
        "{error:?}"
    );
}

#[test]
fn an_over_large_item_is_rejected_and_named() {
    let huge = JournalRecord::AgentFinished {
        reason: FinishReason::Failed,
        failure: Some(TypedFailure {
            code: "oversized".to_owned(),
            message: "m".repeat(MAX_ITEM_BYTES + 1_000),
            detail: None,
        }),
    };
    let error = decision(vec![huge], Vec::new())
        .validate()
        .expect_err("an over-large item is rejected");
    let EnvelopeViolation::ItemTooLarge { bytes, which } = error else {
        panic!("the rejection must name the offending item, got {error:?}");
    };
    assert!(bytes > MAX_ITEM_BYTES);
    assert!(
        which.starts_with("journal record agent_finished"),
        "the caller has to know which item to page or place: {which}"
    );
}

#[test]
fn appends_must_be_contiguous_from_the_guards_tail() {
    let mut decision = decision(vec![finished(), finished()], Vec::new());
    decision.guard.tail = Some(JournalSeq(9));
    decision.control.next_tail = JournalSeq(9);
    let error = decision
        .validate()
        .expect_err("a control tail behind the appends is rejected");
    assert!(
        matches!(error, EnvelopeViolation::NonContiguousAppends { expected } if expected == JournalSeq(10)),
        "{error:?}"
    );
}

#[test]
fn a_fanout_pages_into_whole_transactions() {
    assert_eq!(fanout_pages(0), 0);
    assert_eq!(fanout_pages(1), 1);
    assert_eq!(fanout_pages(SPAWN_PAGE_CHILDREN), 1);
    assert_eq!(fanout_pages(SPAWN_PAGE_CHILDREN + 1), 2);
    // The per-decision admission bound of 128 is realized as exactly four pages.
    assert_eq!(fanout_pages(128), 4);
}

#[test]
fn preview_sequences_order_totally_without_a_shared_counter() {
    assert_eq!(event_seq(JournalSeq(0), 0), 0);
    assert_eq!(event_seq(JournalSeq(0), 1_023), 1_023);
    assert_eq!(event_seq(JournalSeq(1), 0), 1_024);
    // The whole point: a record's last slot still precedes the next record's first.
    for seq in 0_u64..8 {
        assert!(event_seq(JournalSeq(seq), 1_023) < event_seq(JournalSeq(seq + 1), 0));
    }
}
