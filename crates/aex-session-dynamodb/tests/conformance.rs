//! Request and plan conformance for `session-authority`.
//!
//! The plan-compilation cases assert that every accepted command compiles to
//! exactly the declared participants, in the declared order, with the declared
//! conditions. The request cases assert the same thing one level lower, on the
//! bytes `DynamoDB` will actually evaluate, captured from a real client over a
//! capturing transport.

mod support;

use std::collections::HashMap;

use aex_session_dynamodb::attr::{n, s};
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::plan::{Participant, TransactionPlan, key};
use aex_session_dynamodb::store::{OperationAuthority, OperationStore};
use aex_session_dynamodb::transactions::{
    ADMISSION_ORDER, AdmissionForeign, DECISION_ORDER, Foreign, ForeignAction, OperationStepCancel,
    TERMINAL_ORDER, TerminalForeign, compile_admission, compile_decision, compile_fanout_page,
    compile_lifecycle, compile_terminal, operation_cancelled,
};
use aex_session_dynamodb::wire_pending::LifecycleTransition;
use aex_wire::types::Timestamp;
use serde_json::Value;

use support::{
    admission, captured_body, capturing_client, decision, fanout, lifecycle, operation, tables,
    terminal, workspace,
};

#[tokio::test]
async fn an_operation_authority_read_is_strongly_consistent_and_targets_the_exact_key() {
    let (client, receiver) = capturing_client();
    let store = OperationStore::new(client, &tables().session_authority);
    let _ignored = store.load(workspace(), operation()).await;

    let body = captured_body(receiver);
    let expected_operation = format!("OP#{}", operation());
    assert_eq!(
        body["TableName"].as_str(),
        Some(tables().session_authority.as_str())
    );
    assert_eq!(body["ConsistentRead"].as_bool(), Some(true));
    assert_eq!(
        body["Key"]["pk"]["S"].as_str(),
        Some(expected_operation.as_str())
    );
    assert_eq!(body["Key"]["sk"]["S"].as_str(), Some("STATE"));
}

#[test]
fn a_cancelled_operation_step_fences_the_exact_running_uncommitted_version() {
    let request = OperationStepCancel {
        workspace: workspace(),
        operation: operation(),
        version: 4,
        now: Timestamp::from_unix_millis(1_754_138_096_000).expect("fixture instant"),
    };
    let update = operation_cancelled(&tables().session_authority, &request)
        .expect("the cancellation update builds")
        .build()
        .expect("the update is complete");

    assert_eq!(
        update.condition_expression(),
        Some(
            "attribute_exists(pk) AND workspaceId = :workspaceId AND operationId = :operationId \
             AND version = :version AND #status = :running AND cancelRequested = :true \
             AND attribute_not_exists(committedAt)"
        )
    );
    assert_eq!(
        update.update_expression(),
        "SET #status = :cancelled, version = :nextVersion, updatedAt = :now, terminalAt = :now"
    );
    assert_eq!(
        update
            .expression_attribute_values()
            .and_then(|values| values.get(":nextVersion"))
            .and_then(|value| value.as_n().ok())
            .map(String::as_str),
        Some("5")
    );
}

#[tokio::test]
async fn the_operation_step_committer_refuses_any_participant_shape_but_operation_then_work() {
    let (client, _receiver) = capturing_client();
    let store = OperationStore::new(client, &tables().session_authority);
    let request = OperationStepCancel {
        workspace: workspace(),
        operation: operation(),
        version: 4,
        now: Timestamp::from_unix_millis(1_754_138_096_000).expect("fixture instant"),
    };
    let mut plan = TransactionPlan::new("wrong-shape");
    plan.update(
        Participant::SESSION_OPERATION,
        operation_cancelled(&tables().session_authority, &request).expect("update builds"),
    )
    .expect("one conditional participant");

    assert!(matches!(
        store.commit_cancelled_step(&plan).await,
        Err(StoreError::Invalid { .. })
    ));
}

fn foreign_put(participant: Participant, table: &str) -> Foreign {
    Foreign::new(
        participant,
        ForeignAction::Put(Box::new(
            aws_sdk_dynamodb::types::Put::builder()
                .table_name(table)
                .set_item(Some(HashMap::from([
                    ("pk".to_owned(), s("WORK#w")),
                    ("sk".to_owned(), s("STATE")),
                    ("itemType".to_owned(), s("work")),
                ])))
                .condition_expression("attribute_not_exists(pk)"),
        )),
    )
}

fn foreign_guard(participant: Participant, table: &str) -> Foreign {
    Foreign::new(
        participant,
        ForeignAction::ConditionCheck(Box::new(
            aws_sdk_dynamodb::types::ConditionCheck::builder()
                .table_name(table)
                .set_key(Some(key("WS#ws", "PLACEMENT")))
                .condition_expression(
                    "#status = :active AND keyEpoch = :keyEpoch AND accountEpoch = :accountEpoch \
                     AND revocationEpoch = :revocationEpoch",
                )
                .expression_attribute_names("#status", "status")
                .expression_attribute_values(":active", s("active"))
                .expression_attribute_values(":keyEpoch", n(1))
                .expression_attribute_values(":accountEpoch", n(2))
                .expression_attribute_values(":revocationEpoch", n(3)),
        )),
    )
}

fn full_admission_foreign() -> AdmissionForeign {
    let tables = tables();
    AdmissionForeign {
        placement: Some(foreign_guard(
            Participant::AUTHZ_PLACEMENT,
            &tables.regional_authz_projection,
        )),
        content: vec![
            Foreign::new(
                Participant::CONTENT_COMMIT,
                ForeignAction::Update(Box::new(
                    aws_sdk_dynamodb::types::Update::builder()
                        .table_name(&tables.regional_content)
                        .set_key(Some(key("CONTENT#ws#digest", "DESC")))
                        .condition_expression(
                            "#state IN (:staged, :committed) AND digestSha256 = :digest",
                        )
                        .update_expression("SET #state = :committed, verifiedAt = :now")
                        .expression_attribute_names("#state", "state")
                        .expression_attribute_values(":staged", s("staged"))
                        .expression_attribute_values(":committed", s("committed"))
                        .expression_attribute_values(":digest", s("sha256:aa"))
                        .expression_attribute_values(":now", s("2026-08-01T12:34:56.789Z")),
                )),
            ),
            foreign_put(Participant::CONTENT_MESSAGE_PIN, &tables.regional_content),
        ],
        work: vec![
            foreign_put(Participant::WORK_ROOT_WAKE, &tables.regional_work),
            foreign_put(Participant::WORK_DEDUPE, &tables.regional_work),
        ],
    }
}

#[test]
fn the_admission_transaction_compiles_to_exactly_the_declared_participants_in_order() {
    let plan = compile_admission(&tables(), &admission(), full_admission_foreign())
        .expect("the admission compiles");
    assert_eq!(plan.participants(), ADMISSION_ORDER);
    assert_eq!(plan.len(), 12, "ten to twelve actions, inside the envelope");
}

#[test]
fn an_inline_message_admission_omits_the_two_content_participants() {
    let mut foreign = full_admission_foreign();
    foreign.content.clear();
    let plan = compile_admission(&tables(), &admission(), foreign).expect("compiles");
    let expected: Vec<Participant> = ADMISSION_ORDER
        .iter()
        .copied()
        .filter(|participant| {
            *participant != Participant::CONTENT_COMMIT
                && *participant != Participant::CONTENT_MESSAGE_PIN
        })
        .collect();
    assert_eq!(plan.participants(), expected.as_slice());
}

#[test]
fn every_admission_action_carries_a_condition_expression() {
    let plan =
        compile_admission(&tables(), &admission(), full_admission_foreign()).expect("compiles");
    for (participant, action) in plan.participants().iter().zip(plan.actions()) {
        let condition = action
            .put()
            .and_then(aws_sdk_dynamodb::types::Put::condition_expression)
            .or_else(|| {
                action
                    .update()
                    .and_then(aws_sdk_dynamodb::types::Update::condition_expression)
            })
            .or_else(|| {
                action
                    .delete()
                    .and_then(aws_sdk_dynamodb::types::Delete::condition_expression)
            })
            .or_else(|| {
                action
                    .condition_check()
                    .map(aws_sdk_dynamodb::types::ConditionCheck::condition_expression)
            });
        assert!(
            condition.is_some_and(|expression| !expression.is_empty()),
            "`{participant}` is unconditional"
        );
    }
}

#[test]
fn the_admission_head_condition_names_every_fence_the_plan_declares() {
    let plan =
        compile_admission(&tables(), &admission(), full_admission_foreign()).expect("compiles");
    let index = plan
        .participants()
        .iter()
        .position(|participant| *participant == Participant::SESSION_HEAD)
        .expect("the head is a participant");
    let condition = plan.actions()[index]
        .update()
        .and_then(aws_sdk_dynamodb::types::Update::condition_expression)
        .expect("the head update is conditional");
    for fence in [
        "attribute_exists(pk)",
        "revision = :revision",
        "#status = :idle",
        "lifecycle = :active",
        "deletionEpoch = :deletionEpoch",
        "cancelEpoch = :cancelEpoch",
        "contentAdmissionEpoch = :contentEpoch",
        "resolvedConfigDigest = :configDigest",
        "attribute_not_exists(activeRunId)",
    ] {
        assert!(
            condition.contains(fence),
            "the head condition drops `{fence}`"
        );
    }
}

#[test]
fn the_terminal_barrier_compiles_to_exactly_the_declared_participants_in_order() {
    let tables = tables();
    let foreign = TerminalForeign {
        usage_closure: Some(foreign_put(
            Participant::WORK_USAGE_CLOSURE,
            &tables.regional_work,
        )),
        wake_done: Some(Foreign::new(
            Participant::WORK_WAKE_DONE,
            ForeignAction::Update(Box::new(
                aws_sdk_dynamodb::types::Update::builder()
                    .table_name(&tables.regional_work)
                    .set_key(Some(key("WORK#w", "STATE")))
                    .condition_expression(
                        "fence = :fence AND claimOwner = :owner AND #state = :claimed",
                    )
                    .update_expression(
                        "SET #state = :done, expiresAtEpochSeconds = :ttl \
                         REMOVE dueShardPk, dueShardSk",
                    )
                    .expression_attribute_names("#state", "state")
                    .expression_attribute_values(":fence", n(2))
                    .expression_attribute_values(":owner", s("worker-1"))
                    .expression_attribute_values(":claimed", s("claimed"))
                    .expression_attribute_values(":done", s("done"))
                    .expression_attribute_values(":ttl", n(1_754_138_096)),
            )),
        )),
    };
    let plan = compile_terminal(&tables, &terminal(), foreign).expect("compiles");
    assert_eq!(plan.participants(), TERMINAL_ORDER);
}

#[test]
fn the_terminal_run_condition_admits_only_a_non_terminal_run() {
    let plan =
        compile_terminal(&tables(), &terminal(), TerminalForeign::default()).expect("compiles");
    let condition = plan.actions()[0]
        .update()
        .and_then(aws_sdk_dynamodb::types::Update::condition_expression)
        .expect("conditional");
    assert!(condition.contains("#status IN (:queued, :running)"));
}

#[test]
fn the_decision_transaction_compiles_to_exactly_the_declared_participants_in_order() {
    let tables = tables();
    let plan = compile_decision(
        &tables,
        &decision(),
        Some(foreign_put(
            Participant::WORK_NEXT_WAKE,
            &tables.regional_work,
        )),
    )
    .expect("compiles");
    assert_eq!(plan.participants(), DECISION_ORDER);
}

#[test]
fn a_decision_guards_the_head_without_writing_it() {
    let plan = compile_decision(&tables(), &decision(), None).expect("compiles");
    let head = &plan.actions()[0];
    assert!(
        head.condition_check().is_some(),
        "the head guard is read-only"
    );
    assert!(head.update().is_none());
    assert!(head.put().is_none());
    assert!(head.delete().is_none());
}

#[test]
fn a_fanout_page_conditions_on_the_parent_budget_and_never_on_a_shared_counter() {
    let plan = compile_fanout_page(&tables(), &fanout(4), Vec::new()).expect("compiles");
    let condition = plan.actions()[0]
        .update()
        .and_then(aws_sdk_dynamodb::types::Update::condition_expression)
        .expect("conditional");
    assert!(condition.contains("childBudgetRemaining >= :childCount"));
    assert!(condition.contains("revision = :revision"));
}

#[test]
fn a_fanout_page_too_large_for_one_transaction_is_refused_rather_than_truncated() {
    let error = compile_fanout_page(&tables(), &fanout(40), Vec::new())
        .expect_err("40 children exceed the page ceiling");
    assert!(matches!(error, StoreError::Invalid { .. }), "{error}");
}

#[test]
fn purge_completion_removes_every_content_bearing_attribute_and_both_index_attributes() {
    let plan = compile_lifecycle(
        &tables(),
        &lifecycle(LifecycleTransition::PurgeComplete),
        Vec::new(),
    )
    .expect("compiles");
    let update = plan.actions()[0]
        .update()
        .map(aws_sdk_dynamodb::types::Update::update_expression)
        .expect("an update expression");
    for removed in [
        "wsIndexPk",
        "wsIndexSk",
        "resolvedConfig",
        "initialRootDigest",
        "persistedRootDigest",
        "continuity",
        "activeRunId",
    ] {
        assert!(update.contains(removed), "purge keeps `{removed}`");
    }
    assert!(update.contains("lifecycle = :purged"));
}

#[test]
fn trash_advances_all_three_epochs_and_moves_the_index_partition() {
    let plan = compile_lifecycle(
        &tables(),
        &lifecycle(LifecycleTransition::Trash),
        Vec::new(),
    )
    .expect("compiles");
    let update = plan.actions()[0]
        .update()
        .map(aws_sdk_dynamodb::types::Update::update_expression)
        .expect("an update expression");
    assert!(update.contains("deletionEpoch = deletionEpoch + :one"));
    assert!(update.contains("cancelEpoch = cancelEpoch + :one"));
    assert!(update.contains("contentAdmissionEpoch = contentAdmissionEpoch + :one"));
    assert!(update.contains("wsIndexPk = :trashedIndexPk"));
}

#[test]
fn a_lifecycle_transition_with_no_owning_operation_is_refused() {
    let mut plan = lifecycle(LifecycleTransition::Trash);
    plan.operation = None;
    assert!(matches!(
        compile_lifecycle(&tables(), &plan, Vec::new()),
        Err(StoreError::Invalid { .. })
    ));
}

#[tokio::test]
async fn the_serialized_admission_carries_the_transport_token_and_all_old_return_values() {
    let (client, receiver) = capturing_client();
    let plan =
        compile_admission(&tables(), &admission(), full_admission_foreign()).expect("compiles");
    let _ignored = plan
        .compile(&client)
        .expect("compiles to a request")
        .send()
        .await;

    let body = captured_body(receiver);
    assert_eq!(
        body["ClientRequestToken"].as_str(),
        Some(admission().replay.intent.to_string().as_str()),
        "the transport deduplication identity is the canonical intent"
    );
    let items = body["TransactItems"]
        .as_array()
        .expect("a transaction carries items");
    assert_eq!(items.len(), 12);
    for item in items {
        let (_, action) = item
            .as_object()
            .expect("one action per entry")
            .iter()
            .next()
            .expect("exactly one action kind");
        assert_eq!(
            action["ReturnValuesOnConditionCheckFailure"].as_str(),
            Some("ALL_OLD"),
            "a failed condition must return the row it saw"
        );
    }
}

#[tokio::test]
async fn the_serialized_head_update_is_exactly_the_declared_expression() {
    let (client, receiver) = capturing_client();
    let plan =
        compile_admission(&tables(), &admission(), full_admission_foreign()).expect("compiles");
    let _ignored = plan.compile(&client).expect("compiles").send().await;

    let body = captured_body(receiver);
    let update = body["TransactItems"][3]["Update"].clone();
    assert_eq!(
        update["TableName"].as_str(),
        Some("dev-eu-west-1-session-authority")
    );
    assert_eq!(
        update["Key"]["pk"]["S"].as_str(),
        Some(format!("SESSION#{}", support::session()).as_str())
    );
    assert_eq!(update["Key"]["sk"]["S"].as_str(), Some("HEAD"));
    assert_eq!(
        update["ExpressionAttributeValues"][":revision"]["N"].as_str(),
        Some("12")
    );
    assert_eq!(
        update["ExpressionAttributeValues"][":nextRevision"]["N"].as_str(),
        Some("13")
    );
    assert_eq!(
        update["ExpressionAttributeNames"]["#status"].as_str(),
        Some("status")
    );
}

#[tokio::test]
async fn the_message_body_is_serialized_onto_exactly_one_row() {
    let (client, receiver) = capturing_client();
    let plan =
        compile_admission(&tables(), &admission(), full_admission_foreign()).expect("compiles");
    let _ignored = plan.compile(&client).expect("compiles").send().await;

    let body = captured_body(receiver);
    assert_eq!(
        count_attribute(&body, "contentInline"),
        1,
        "a prompt body must never be duplicated across rows"
    );
}

fn count_attribute(value: &Value, attribute: &str) -> usize {
    match value {
        Value::Object(members) => members
            .iter()
            .map(|(name, member)| {
                usize::from(name == attribute) + count_attribute(member, attribute)
            })
            .sum(),
        Value::Array(items) => items
            .iter()
            .map(|item| count_attribute(item, attribute))
            .sum(),
        _ => 0,
    }
}
