//! Slice S-2.2 — the compiled shape of one decision transaction, asserted without an AWS
//! call.
//!
//! A transaction is only diagnosable if it names its participants, and it is only correct
//! if every action carries its precondition. Both are properties of the compiled plan, so
//! both are asserted here rather than observed indirectly and expensively on a live plane.

use aex_brain_app::ports::{DecisionContext, RunBoundaryAuthority, SessionAuthority};
use aex_brain_domain::budget::{BudgetDelta, Dimension, DimensionVector};
use aex_brain_domain::commit::{
    ChildWrite, ControlUpdate, DecisionCommit, EffectWrite, FenceGuardRef, JoinWrite,
    PublicSessionEvent, RunTransition, SPAWN_PAGE_CHILDREN, SessionHeadTransition, WakeCreate,
    WakeRetirement, event_seq,
};
use aex_brain_domain::effect::{EffectClass, EffectKind};
use aex_brain_domain::ids::{
    AgentId, AgentKey, AgentRevision, CancelEpoch, ContentHash, EffectId, Fence, JoinId,
    JournalSeq, OwnerToken, SessionId, Timestamp, WakeId,
};
use aex_brain_domain::journal::{FinishReason, JournalRecord};
use aex_brain_domain::wire_pending::JoinMode;
use aex_brain_store_dynamodb::plan::{self, BrainTables, participant};
use aex_session_dynamodb::plan::Participant;
use aex_wire::ids::{GenerationId, PrefixedId, Uuid7};

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
        authority: SessionAuthority {
            workspace: aex_wire::ids::WorkspaceId::from_uuid7(Uuid7::compose(
                1_767_225_600_002,
                [3; 10],
            )),
            organization: aex_wire::ids::OrganizationId::from_uuid7(Uuid7::compose(
                1_767_225_600_003,
                [4; 10],
            )),
            deletion_epoch: 0,
            active: None,
        },
        lease_expires_at: Timestamp::from_millis(1_767_225_615_000),
        now: Timestamp::from_millis(1_767_225_600_000),
    }
}

fn tables() -> BrainTables {
    BrainTables {
        session_authority: "dev-eu-west-1-session-authority".to_owned(),
        regional_work: "dev-eu-west-1-regional-work".to_owned(),
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
        retired_wake: None,
        events: Vec::new(),
        messages: Vec::new(),
        session_events: Vec::new(),
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

#[test]
fn source_retirement_is_one_fenced_transaction_and_removes_the_due_projection() {
    let mut commit = base(Vec::new());
    commit.control.next_revision = commit.guard.revision;
    commit.retired_wake = Some(WakeRetirement {
        work_id: "wrk_01".to_owned(),
    });

    let compiled = plan::compile(&tables(), &context(), &commit).expect("retirement compiles");
    assert_eq!(
        compiled.participants(),
        &[
            Participant::SESSION_HEAD_GUARD,
            Participant::AGENT_CONTROL,
            Participant::WORK_WAKE_DONE,
        ]
    );
    assert!(
        compiled.actions()[1].condition_check().is_some(),
        "retirement checks the agent fence without manufacturing a journal decision"
    );
    let work = compiled.actions()[2].update().expect("a work update");
    let condition = work.condition_expression().expect("conditional");
    for required in [
        "itemType = :work",
        "kind = :kind",
        "workId = :workId",
        "workspaceId = :workspaceId",
        "sessionId = :sessionId",
        "agentId = :agentId",
        "fence = :unclaimed",
        "#state = :pending",
    ] {
        assert!(
            condition.contains(required),
            "missing `{required}`: {condition}"
        );
    }
    let update = work.update_expression();
    assert!(update.contains("#state = :done"), "{update}");
    assert!(update.contains("REMOVE dueShardPk, dueShardSk"), "{update}");
}

#[test]
fn a_retirement_never_reuses_the_journal_decisions_transport_token() {
    let journal = base(Vec::new());
    let mut retirement = journal.clone();
    retirement.control.next_revision = retirement.guard.revision;
    retirement.retired_wake = Some(WakeRetirement {
        work_id: "wrk_01".to_owned(),
    });

    let journal_token = plan::client_request_token(&journal);
    let retirement_token = plan::client_request_token(&retirement);
    assert_ne!(journal_token, retirement_token);
    assert!(journal_token.len() <= 36, "{journal_token}");
    assert!(retirement_token.len() <= 36, "{retirement_token}");
}

/// The one-append form must produce the order the shared compiler publishes, restricted to
/// the participants it actually carries. Brain compiles the vector form itself; if the two
/// ever disagreed on order, a peer reading a cancellation from one would misname the
/// participant of the other.
#[test]
fn the_single_append_form_matches_the_shared_decision_order() {
    let mut commit = base(vec![finished()]);
    commit.wakes.push(wake());
    let compiled =
        plan::compile(&tables(), &context(), &commit).expect("a minimal decision compiles");
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
    let compiled = plan::compile(&tables(), &context(), &base(vec![finished()])).expect("compiles");
    assert_eq!(compiled.participants()[0], Participant::SESSION_HEAD_GUARD);
    let first = &compiled.actions()[0];
    assert!(
        first.condition_check().is_some(),
        "the head guard must read"
    );
    assert!(first.update().is_none());
    assert!(first.put().is_none());
}

#[test]
fn a_root_run_boundary_is_one_six_action_session_centric_commit() {
    let (session, run, agent, _) = aex_session_domain::testing::running_session();
    let mut context = context();
    context.authority.workspace = session.workspace;
    context.authority.organization = session.organization;
    context.authority.deletion_epoch = session.deletion.epoch.0;
    context.authority.active = Some(Box::new(RunBoundaryAuthority {
        session: session.clone(),
        run: run.clone(),
    }));
    let mut commit = base(vec![JournalRecord::RunFinished {
        run: run.id,
        reason: FinishReason::Completed,
        failure: None,
        output_messages: Vec::new(),
        ambiguous_effect: None,
    }]);
    commit.guard.key = AgentKey::new(
        SessionId(uuid::Uuid::from_bytes(*session.id.uuid7().as_bytes())),
        AgentId(uuid::Uuid::from_bytes(*agent.id.uuid7().as_bytes())),
    );
    commit.guard.cancel_epoch = CancelEpoch(session.cancellation.0);
    commit.guard.fence = Fence(agent.fence().0);
    commit.run = Some(RunTransition {
        run: run.id,
        finish: FinishReason::Completed,
        failure: None,
        output_messages: Vec::new(),
        cancellation: None,
        ambiguous_effect: None,
    });
    commit.session = Some(SessionHeadTransition {
        status: "idle".to_owned(),
        revision: session.revision.0,
    });
    commit.session_events.push(PublicSessionEvent {
        event_seq: event_seq(JournalSeq(10), 0),
        message: run.message,
        outcome: "succeeded".to_owned(),
        at: context.now,
    });
    commit.control.phase = "awaiting_input".to_owned();

    let compiled = plan::compile(&tables(), &context, &commit).expect("boundary compiles");
    assert_eq!(compiled.len(), 6);
    assert_eq!(
        compiled.participants(),
        &[
            Participant::AGENT_CONTROL,
            Participant::AGENT_JOURNAL,
            Participant::SESSION_COMPLETED_EVENT,
            Participant::SESSION_RUN,
            Participant::SESSION_HEAD,
            Participant::SESSION_TERMINAL_EVENT,
        ]
    );
    assert!(
        !compiled
            .participants()
            .contains(&Participant::SESSION_HEAD_GUARD),
        "the guarded head write replaces a conflicting read action"
    );
    let event = compiled.actions()[2]
        .put()
        .expect("public event put")
        .item();
    assert!(event.get("runId").is_none());
    assert!(event.get("agentId").is_none());
}

/// Every action carries a condition. The shared compiler refuses an unconditional
/// authority write outright, so this asserts the plan actually reaches that compiler.
#[test]
fn every_compiled_action_is_conditional() {
    let mut commit = base(vec![finished()]);
    commit.effects.push(EffectWrite::Prepare {
        id: EffectId([1; 16]),
        kind: EffectKind::ModelCall,
        generation: None,
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

    let compiled = plan::compile(&tables(), &context(), &commit).expect("compiles");
    for (index, action) in compiled.actions().iter().enumerate() {
        assert!(
            condition_of(action).is_some_and(|it| !it.trim().is_empty()),
            "action {index} ({}) is unconditional",
            compiled.participants()[index]
        );
    }
}

#[test]
fn a_hands_prepare_persists_the_exact_canonical_generation() {
    let generation = GenerationId::from_uuid7(Uuid7::compose(1_767_225_600_009, [9; 10]));
    let mut commit = base(vec![finished()]);
    commit.effects.push(EffectWrite::Prepare {
        id: EffectId([3; 16]),
        kind: EffectKind::HandsOperation,
        generation: Some(generation),
        class: EffectClass::DurableDetached,
        request_hash: ContentHash::of(b"hands request"),
        deadline: Timestamp::from_millis(1_767_225_660_000),
        attempt: 1,
    });
    let compiled = plan::compile(&tables(), &context(), &commit).expect("compiles");
    let index = compiled
        .participants()
        .iter()
        .position(|participant| *participant == Participant::AGENT_EFFECT)
        .expect("effect participant");
    let item = compiled.actions()[index]
        .put()
        .expect("a prepare is a put")
        .item();
    assert_eq!(
        item["generationId"].as_s().expect("generation string"),
        &generation.to_string()
    );
    assert_eq!(item["kind"].as_s().expect("kind string"), "HandsOperation");
    assert_eq!(item["state"].as_s().expect("state string"), "prepared");
}

/// The control update carries the whole precondition set. Each rejects a different way of
/// being stale, so a missing one is a hole rather than an optimization.
#[test]
fn the_control_update_carries_the_whole_precondition_set() {
    let record = finished();
    let expected_tail_hash = record.content_hash().expect("the record canonicalizes");
    let compiled = plan::compile(&tables(), &context(), &base(vec![record])).expect("compiles");
    let index = compiled
        .participants()
        .iter()
        .position(|it| *it == Participant::AGENT_CONTROL)
        .expect("a decision always writes the control item");
    let update = compiled.actions()[index]
        .update()
        .expect("the control item is updated");
    let expression = update
        .condition_expression()
        .expect("conditional")
        .to_owned();
    for attribute in ["revision", "fence", "claimOwner", "journalTail"] {
        assert!(expression.contains(attribute), "{expression}");
    }
    assert!(
        update.update_expression().contains("journalTailHash"),
        "the authoritative sequence and hash move together"
    );
    assert_eq!(
        update
            .expression_attribute_values()
            .expect("control values")[":nextTailHash"]
            .as_s()
            .expect("a hash string"),
        &expected_tail_hash.to_hex()
    );
}

/// A journal put is immutable. That single condition is what makes a redelivered decision
/// idempotent for ever, rather than only inside the provider's dedup window.
#[test]
fn a_journal_put_is_immutable_so_a_redelivery_loses_rather_than_duplicates() {
    let compiled = plan::compile(&tables(), &context(), &base(vec![finished()])).expect("compiles");
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
fn the_client_request_token_is_a_function_of_the_commit_identity() {
    let commit = base(vec![finished()]);
    let first = plan::compile(&tables(), &context(), &commit).expect("compiles");
    let second = plan::compile(&tables(), &context(), &commit).expect("compiles");
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
    let compiled =
        plan::compile(&tables(), &context(), &commit).expect("a full page fits one transaction");
    assert!(
        compiled.len() <= aex_session_dynamodb::plan::MAX_ACTIONS,
        "{} actions",
        compiled.len()
    );

    let mut over = base(Vec::new());
    over.children = (0..=SPAWN_PAGE_CHILDREN).map(spawn).collect();
    assert!(
        plan::compile(&tables(), &context(), &over).is_err(),
        "the envelope must be enforced before the round trip, not by DynamoDB"
    );
}

/// A wake rides through the work adapter's own builders, so Brain never forks that row
/// shape, and it always carries its dedupe claim.
#[test]
fn a_wake_is_written_through_the_work_adapters_builders_with_its_dedupe_claim() {
    let mut commit = base(vec![finished()]);
    commit.wakes.push(wake());
    let compiled = plan::compile(&tables(), &context(), &commit).expect("compiles");
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
    assert_eq!(table, tables().regional_work);
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
    let compiled = plan::compile(&tables(), &context(), &commit).expect("compiles");
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
    let compiled = plan::compile(&tables(), &context(), &commit).expect("compiles");
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
    assert!(plan::compile(&tables(), &context(), &commit).is_err());
}

/// Tenant ownership and deletion fencing are session facts. Compiling two sessions through
/// one process must stamp each wake with its own authority and fence on its own epoch.
#[test]
fn decision_authority_is_per_session_not_fixed_when_the_store_is_built() {
    let mut commit = base(vec![finished()]);
    commit.wakes.push(wake());

    let first = context();
    let mut second = context();
    second.authority.workspace =
        aex_wire::ids::WorkspaceId::from_uuid7(Uuid7::compose(1_767_225_600_012, [13; 10]));
    second.authority.organization =
        aex_wire::ids::OrganizationId::from_uuid7(Uuid7::compose(1_767_225_600_013, [14; 10]));
    second.authority.deletion_epoch = 9;

    let first_plan = plan::compile(&tables(), &first, &commit).expect("first session compiles");
    let second_plan = plan::compile(&tables(), &second, &commit).expect("second session compiles");
    let wake_index = first_plan
        .participants()
        .iter()
        .position(|participant| *participant == Participant::WORK_NEXT_WAKE)
        .expect("the wake is present");
    let first_wake = first_plan.actions()[wake_index]
        .put()
        .expect("wake put")
        .item();
    let second_wake = second_plan.actions()[wake_index]
        .put()
        .expect("wake put")
        .item();
    assert_eq!(
        first_wake["workspaceId"].as_s().expect("workspace string"),
        &first.authority.workspace.to_string()
    );
    assert_eq!(
        second_wake["workspaceId"].as_s().expect("workspace string"),
        &second.authority.workspace.to_string()
    );
    assert_eq!(
        second_wake["organizationId"]
            .as_s()
            .expect("organization string"),
        &second.authority.organization.to_string()
    );

    let first_guard = first_plan.actions()[0]
        .condition_check()
        .expect("session guard");
    let second_guard = second_plan.actions()[0]
        .condition_check()
        .expect("session guard");
    let expression = second_guard.condition_expression();
    assert!(
        expression.contains("workspaceId = :workspaceId"),
        "{expression}"
    );
    assert!(
        expression.contains("organizationId = :organizationId"),
        "{expression}"
    );
    assert_eq!(
        second_guard
            .expression_attribute_values()
            .expect("guard values")[":workspaceId"]
            .as_s()
            .expect("workspace string"),
        &second.authority.workspace.to_string(),
        "a context for another tenant must lose the session-head condition"
    );
    assert_eq!(
        first_guard
            .expression_attribute_values()
            .expect("guard values")[":deletionEpoch"]
            .as_n()
            .expect("epoch number"),
        "0"
    );
    assert_eq!(
        second_guard
            .expression_attribute_values()
            .expect("guard values")[":deletionEpoch"]
            .as_n()
            .expect("epoch number"),
        "9",
        "a stale process-wide epoch must never guard another session's decision"
    );
}
