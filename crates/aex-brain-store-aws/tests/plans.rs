//! Slice S-2.2 — the compiled shape of one decision transaction, asserted without an AWS
//! call.
//!
//! A transaction is only diagnosable if it names its participants, and it is only correct
//! if every action carries its precondition. Both are properties of the compiled plan, so
//! both are asserted here rather than observed indirectly and expensively on a live plane.

use aex_brain_domain::budget::{BudgetDelta, Dimension, DimensionVector};
use aex_brain_domain::commit::{
    ChildWrite, ControlUpdate, DecisionCommit, EffectWrite, FenceGuardRef, JoinWrite,
    SPAWN_PAGE_CHILDREN, WakeCreate,
};
use aex_brain_domain::effect::{EffectClass, EffectKind};
use aex_brain_domain::ids::{
    AgentId, AgentKey, AgentRevision, CancelEpoch, ContentHash, EffectId, Fence, JoinId,
    JournalSeq, OwnerToken, SessionId, Timestamp, WakeId,
};
use aex_brain_domain::journal::{FinishReason, JournalRecord};
use aex_brain_domain::wire_pending::JoinMode;
use aex_brain_store_aws::plan::{self, DecisionContext, participant};
use aex_session_dynamodb::plan::{Participant, RegionalTables};
use aex_wire::ids::{PrefixedId, Uuid7};

fn v7(millis: u64, seed: u8) -> uuid::Uuid {
    uuid::Uuid::from_bytes(*Uuid7::compose(millis, [seed; 10]).as_bytes())
}

fn key() -> AgentKey {
    AgentKey::new(
        SessionId(v7(1_767_225_600_000, 1)),
        AgentId(v7(1_767_225_600_001, 2)),
    )
}

fn context() -> DecisionContext {
    DecisionContext {
        tables: RegionalTables::composed("dev", "eu-west-1"),
        workspace: aex_wire::ids::WorkspaceId::from_uuid7(Uuid7::compose(
            1_767_225_600_002,
            [3; 10],
        )),
        organization: aex_wire::ids::OrganizationId::from_uuid7(Uuid7::compose(
            1_767_225_600_003,
            [4; 10],
        )),
        deletion_epoch: 0,
        lease_expires_at: Timestamp::from_millis(1_767_225_615_000),
        now: Timestamp::from_millis(1_767_225_600_000),
    }
}

fn guard() -> FenceGuardRef {
    FenceGuardRef {
        key: key(),
        owner: OwnerToken(v7(1_767_225_600_004, 5)),
        fence: Fence(4),
        revision: AgentRevision(5),
        tail: Some(JournalSeq(9)),
        cancel_epoch: CancelEpoch(6),
    }
}

fn base(appends: Vec<JournalRecord>) -> DecisionCommit {
    let next_tail = JournalSeq(9 + appends.len() as u64);
    DecisionCommit {
        guard: guard(),
        appends,
        control: ControlUpdate {
            next_revision: AgentRevision(6),
            next_tail,
            phase: "awaiting_model".to_owned(),
            finish: None,
        },
        effects: Vec::new(),
        budget: Vec::new(),
        session_budget: Vec::new(),
        children: Vec::new(),
        joins: Vec::new(),
        wakes: Vec::new(),
        events: Vec::new(),
        run: None,
        session: None,
        idempotency: None,
    }
}

fn finished() -> JournalRecord {
    JournalRecord::AgentFinished {
        reason: FinishReason::Completed,
        failure: None,
    }
}

fn spawn(ordinal: u32) -> ChildWrite {
    ChildWrite::Spawn {
        child: aex_brain_domain::ids::child_agent_id(key().agent, ordinal),
        ordinal,
        grant: DimensionVector::uniform(1),
        join: JoinId(v7(1_767_225_600_005, 6)),
        queued_reason: None,
    }
}

fn wake() -> WakeCreate {
    WakeCreate {
        id: WakeId(v7(1_767_225_600_006, 7)),
        key: key(),
        dedup_key: "agent:next".to_owned(),
        reason: aex_brain_domain::journal::ParkReason::AwaitingUserMessage,
        due: Some(Timestamp::from_millis(1_767_225_601_000)),
        priority: 1,
        tenant: "ws".to_owned(),
        shard: aex_brain_domain::ids::WorkShard(0),
    }
}

fn condition_of(action: &aws_sdk_dynamodb::types::TransactWriteItem) -> Option<String> {
    action
        .condition_check()
        .map(|it| it.condition_expression().to_owned())
        .or_else(|| {
            action
                .put()
                .and_then(|it| it.condition_expression().map(ToOwned::to_owned))
        })
        .or_else(|| {
            action
                .update()
                .and_then(|it| it.condition_expression().map(ToOwned::to_owned))
        })
        .or_else(|| {
            action
                .delete()
                .and_then(|it| it.condition_expression().map(ToOwned::to_owned))
        })
}

/// The one-append form must produce the order the shared compiler publishes, restricted to
/// the participants it actually carries. Brain compiles the vector form itself; if the two
/// ever disagreed on order, a peer reading a cancellation from one would misname the
/// participant of the other.
#[test]
fn the_single_append_form_matches_the_shared_decision_order() {
    let mut commit = base(vec![finished()]);
    commit.wakes.push(wake());
    let compiled = plan::compile(&context(), &commit).expect("a minimal decision compiles");
    let observed: Vec<Participant> = compiled.participants().to_vec();
    let expected: Vec<Participant> = plan::decision_order()
        .iter()
        .copied()
        .filter(|it| *it != Participant::AGENT_EFFECT && *it != Participant::SESSION_PREVIEW_EVENT)
        .chain(core::iter::once(aex_work_dynamodb::claim::DEDUPE))
        .collect();
    assert_eq!(observed, expected, "{observed:?}");
}

/// The head is read, never written. A decision that took the head's revision would
/// serialize every agent in the session behind every other one.
#[test]
fn a_decision_guards_the_session_head_without_writing_it() {
    let compiled = plan::compile(&context(), &base(vec![finished()])).expect("compiles");
    assert_eq!(compiled.participants()[0], Participant::SESSION_HEAD_GUARD);
    let first = &compiled.actions()[0];
    assert!(
        first.condition_check().is_some(),
        "the head guard must read"
    );
    assert!(first.update().is_none());
    assert!(first.put().is_none());
}

/// Every action carries a condition. The shared compiler refuses an unconditional
/// authority write outright, so this asserts the plan actually reaches that compiler.
#[test]
fn every_compiled_action_is_conditional() {
    let mut commit = base(vec![finished()]);
    commit.effects.push(EffectWrite::Prepare {
        id: EffectId([1; 16]),
        kind: EffectKind::ModelCall,
        class: EffectClass::NonReplayable,
        request_hash: ContentHash::of(b"request"),
        deadline: Timestamp::from_millis(1_767_225_660_000),
        attempt: 1,
    });
    commit
        .budget
        .push(BudgetDelta::new(Dimension::ProviderCalls, 1));
    commit
        .session_budget
        .push(BudgetDelta::new(Dimension::ActiveChildren, 1));
    commit.children.push(spawn(0));
    commit.joins.push(JoinWrite::Open {
        join: JoinId(v7(1_767_225_600_005, 6)),
        mode: JoinMode::All,
        members: vec![aex_brain_domain::ids::child_agent_id(key().agent, 0)],
        shards: 1,
    });
    commit.joins.push(JoinWrite::IncrementShard {
        join: JoinId(v7(1_767_225_600_005, 6)),
        shard: 0,
    });
    commit.wakes.push(wake());

    let compiled = plan::compile(&context(), &commit).expect("compiles");
    for (index, action) in compiled.actions().iter().enumerate() {
        assert!(
            condition_of(action).is_some_and(|it| !it.trim().is_empty()),
            "action {index} ({}) is unconditional",
            compiled.participants()[index]
        );
    }
}

/// The control update carries the whole precondition set. Each rejects a different way of
/// being stale, so a missing one is a hole rather than an optimization.
#[test]
fn the_control_update_carries_the_whole_precondition_set() {
    let compiled = plan::compile(&context(), &base(vec![finished()])).expect("compiles");
    let index = compiled
        .participants()
        .iter()
        .position(|it| *it == Participant::AGENT_CONTROL)
        .expect("a decision always writes the control item");
    let expression = compiled.actions()[index]
        .update()
        .expect("the control item is updated")
        .condition_expression()
        .expect("conditional")
        .to_owned();
    for attribute in ["revision", "fence", "claimOwner", "journalTail"] {
        assert!(expression.contains(attribute), "{expression}");
    }
}

/// A journal put is immutable. That single condition is what makes a redelivered decision
/// idempotent for ever, rather than only inside the provider's dedup window.
#[test]
fn a_journal_put_is_immutable_so_a_redelivery_loses_rather_than_duplicates() {
    let compiled = plan::compile(&context(), &base(vec![finished()])).expect("compiles");
    let index = compiled
        .participants()
        .iter()
        .position(|it| *it == Participant::AGENT_JOURNAL)
        .expect("the append is present");
    assert_eq!(
        compiled.actions()[index]
            .put()
            .expect("a journal entry is put")
            .condition_expression(),
        Some(aex_session_dynamodb::plan::IMMUTABLE)
    );
}

/// The transport dedup identity is derived, not minted, so a redelivered wake replanning
/// the same step reuses it.
#[test]
fn the_client_request_token_is_a_function_of_the_agent_and_the_tail() {
    let commit = base(vec![finished()]);
    let first = plan::compile(&context(), &commit).expect("compiles");
    let second = plan::compile(&context(), &commit).expect("compiles");
    assert_eq!(first.client_request_token(), second.client_request_token());
    assert_eq!(
        first.client_request_token(),
        plan::client_request_token(&commit)
    );
}

/// Exactly the derived page size fits, and one more does not. Three numbers that must move
/// together: the page size, the per-child action cost and the provider's action ceiling.
#[test]
fn a_full_spawn_page_compiles_and_one_more_child_does_not() {
    let mut commit = base(Vec::new());
    commit.children = (0..SPAWN_PAGE_CHILDREN).map(spawn).collect();
    let compiled = plan::compile(&context(), &commit).expect("a full page fits one transaction");
    assert!(
        compiled.len() <= aex_session_dynamodb::plan::MAX_ACTIONS,
        "{} actions",
        compiled.len()
    );

    let mut over = base(Vec::new());
    over.children = (0..=SPAWN_PAGE_CHILDREN).map(spawn).collect();
    assert!(
        plan::compile(&context(), &over).is_err(),
        "the envelope must be enforced before the round trip, not by DynamoDB"
    );
}

/// A wake rides through the work adapter's own builders, so Brain never forks that row
/// shape, and it always carries its dedupe claim.
#[test]
fn a_wake_is_written_through_the_work_adapters_builders_with_its_dedupe_claim() {
    let mut commit = base(vec![finished()]);
    commit.wakes.push(wake());
    let compiled = plan::compile(&context(), &commit).expect("compiles");
    let participants = compiled.participants();
    let wake_index = participants
        .iter()
        .position(|it| *it == Participant::WORK_NEXT_WAKE)
        .expect("the wake is present");
    assert_eq!(
        participants[wake_index + 1],
        aex_work_dynamodb::claim::DEDUPE
    );
    let table = compiled.actions()[wake_index]
        .put()
        .expect("a wake is put")
        .table_name()
        .to_owned();
    assert_eq!(
        table,
        RegionalTables::composed("dev", "eu-west-1").regional_work
    );
}

/// A budget movement is an `ADD` on a top-level attribute, so a consumption never rewrites
/// a document and two consumers never lose each other's increment. The agent's own node
/// rides inside the control update: two actions on one item are illegal in a transaction.
#[test]
fn an_agent_budget_movement_rides_inside_the_one_control_update() {
    let mut commit = base(vec![finished()]);
    commit
        .budget
        .push(BudgetDelta::new(Dimension::CostMicroUsd, 12));
    let compiled = plan::compile(&context(), &commit).expect("compiles");
    assert_eq!(
        compiled
            .participants()
            .iter()
            .filter(|it| **it == Participant::AGENT_CONTROL)
            .count(),
        1,
        "the control row is touched exactly once"
    );
    assert!(
        !compiled.participants().contains(&participant::AGENT_BUDGET),
        "the agent's node has no row of its own"
    );
    let index = compiled
        .participants()
        .iter()
        .position(|it| *it == Participant::AGENT_CONTROL)
        .expect("present");
    let expression = compiled.actions()[index]
        .update()
        .expect("updated")
        .update_expression()
        .to_owned();
    assert!(expression.contains("ADD"), "{expression}");
    assert!(
        expression.contains(plan::used_attribute(Dimension::CostMicroUsd)),
        "{expression}"
    );
}

/// The session budget is fenced by the cancellation epoch: a cancelled session must not be
/// able to reserve one more child.
#[test]
fn the_session_budget_is_fenced_by_the_cancellation_epoch() {
    let mut commit = base(vec![finished()]);
    commit
        .session_budget
        .push(BudgetDelta::new(Dimension::ActiveChildren, 1));
    let compiled = plan::compile(&context(), &commit).expect("compiles");
    let index = compiled
        .participants()
        .iter()
        .position(|it| *it == participant::SESSION_BUDGET)
        .expect("present");
    let expression = compiled.actions()[index]
        .update()
        .expect("updated")
        .condition_expression()
        .expect("conditional")
        .to_owned();
    assert!(expression.contains("cancelEpoch"), "{expression}");
}

/// An agent whose identity has no wire form is refused rather than written to a partition
/// nothing else will ever address.
#[test]
fn an_agent_with_no_wire_identity_never_compiles() {
    let mut commit = base(vec![finished()]);
    commit.guard.key = AgentKey::new(
        SessionId(uuid::Uuid::from_u128(1)),
        AgentId(uuid::Uuid::from_u128(2)),
    );
    assert!(plan::compile(&context(), &commit).is_err());
}
