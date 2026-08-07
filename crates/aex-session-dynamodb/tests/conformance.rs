//! Request and plan conformance for `session-authority`.
//!
//! The plan-compilation cases assert that every accepted command compiles to
//! exactly the declared participants, in the declared order, with the declared
//! conditions. The request cases assert the same thing one level lower, on the
//! bytes `DynamoDB` will actually evaluate, captured from a real client over a
//! capturing transport.

mod support;

use std::collections::HashMap;

use aex_operation_domain::operation::{OperationKind, OperationStatus};
use aex_session_dynamodb::attr::{n, s};
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::paging::{PageBudget, PagePosition};
use aex_session_dynamodb::plan::{Participant, TransactionPlan, key};
use aex_session_dynamodb::store::{
    OperationApiStore, OperationAuthority, OperationFilter, OperationStore,
};
use aex_session_dynamodb::transactions::{
    ADMISSION_ORDER, AdmissionForeign, DECISION_ORDER, Foreign, ForeignAction,
    OperationCancelRequest, OperationStepCancel, TERMINAL_ORDER, TerminalForeign,
    compile_admission, compile_decision, compile_fanout_page, compile_lifecycle, compile_terminal,
    operation_cancel_requested, operation_cancelled,
};
use aex_session_dynamodb::wire_pending::LifecycleTransition;
use aex_wire::ids::{OperationId, PrefixedId as _, Uuid7};
use aex_wire::types::Timestamp;
use serde_json::Value;

use support::{
    admission, captured_body, capturing_client, decision, fanout, lifecycle, operation,
    scripted_client, tables, terminal, workspace,
};

#[tokio::test]
async fn an_operation_authority_read_is_strongly_consistent_and_targets_the_exact_key() {
    let (client, receiver) = capturing_client();
    let store = OperationStore::new(client, &tables().session_authority);
    let _ignored = OperationAuthority::load(&store, workspace(), operation()).await;

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

#[tokio::test]
async fn an_operation_listing_uses_the_sparse_workspace_index_without_a_filter_expression() {
    let (client, receiver) = capturing_client();
    let store = OperationStore::new(client, &tables().session_authority);
    let index_partition =
        aex_session_dynamodb::keys::workspace_index::operation_partition(workspace());
    let after = PagePosition {
        pk: format!("OP#{}", operation()),
        sk: "STATE".to_owned(),
        index_pk: Some(index_partition.clone()),
        index_sk: Some(format!("2026-08-01T12:34:56.789Z#{}", operation())),
    };
    let _ignored = store
        .page(
            workspace(),
            &OperationFilter {
                session: None,
                kind: Some(OperationKind::SessionPersist),
                status: Some(OperationStatus::Running),
            },
            PageBudget::new(25).expect("a page"),
            Some(&after),
        )
        .await;

    let body = captured_body(receiver);
    assert_eq!(
        body["IndexName"].as_str(),
        Some(aex_session_dynamodb::keys::workspace_index::NAME)
    );
    assert_eq!(body["ConsistentRead"].as_bool(), Some(false));
    assert_eq!(body["ScanIndexForward"].as_bool(), Some(true));
    assert_eq!(body["Limit"].as_u64(), Some(25));
    assert!(body["FilterExpression"].is_null());
    assert_eq!(
        body["ExpressionAttributeValues"][":workspace"]["S"].as_str(),
        Some(index_partition.as_str())
    );
    for attribute in ["pk", "sk", "wsIndexPk", "wsIndexSk"] {
        assert!(
            body["ExclusiveStartKey"][attribute]["S"].is_string(),
            "the continuation must carry `{attribute}`"
        );
    }
}

fn operation_row(operation: OperationId, created_at: &str) -> Value {
    let workspace = workspace().to_string();
    serde_json::json!({
        "pk": {"S": format!("OP#{operation}")},
        "sk": {"S": "STATE"},
        "itemType": {"S": "operation"},
        "operationId": {"S": operation.to_string()},
        "workspaceId": {"S": workspace.clone()},
        "kind": {"S": "session_persist"},
        "status": {"S": "queued"},
        "intentHash": {"S": "03".repeat(32)},
        "scopeKind": {"S": "workspace"},
        "scopeId": {"S": workspace.clone()},
        "version": {"N": "7"},
        "claimsSessionDeletion": {"BOOL": false},
        "cancelRequested": {"BOOL": false},
        "resultPresent": {"BOOL": false},
        "createdAt": {"S": created_at},
        "updatedAt": {"S": created_at},
        "wsIndexPk": {"S": format!("WS#{workspace}#OP")},
        "wsIndexSk": {"S": format!("{created_at}#{operation}")}
    })
}

fn projected_operation_row(operation: OperationId, created_at: &str) -> Value {
    let mut row = operation_row(operation, created_at);
    let object = row.as_object_mut().expect("an item object");
    for attribute in [
        "itemType",
        "intentHash",
        "scopeKind",
        "scopeId",
        "version",
        "claimsSessionDeletion",
        "cancelRequested",
        "resultPresent",
    ] {
        object.remove(attribute);
    }
    row
}

#[tokio::test]
async fn a_provider_short_slice_spends_the_remaining_physical_budget_before_ending() {
    let first = OperationId::from_uuid7(Uuid7::compose(1_754_051_696_790, [8; 10]));
    let second = OperationId::from_uuid7(Uuid7::compose(1_754_051_696_791, [9; 10]));
    let first_at = "2026-08-01T12:34:56.790Z";
    let second_at = "2026-08-01T12:34:56.791Z";
    let first_projected = projected_operation_row(first, first_at);
    let first_last = serde_json::json!({
        "pk": {"S": format!("OP#{first}")},
        "sk": {"S": "STATE"},
        "wsIndexPk": {"S": format!("WS#{}#OP", workspace())},
        "wsIndexSk": {"S": format!("{first_at}#{first}")}
    });
    let responses = vec![
        serde_json::json!({"Items": [first_projected], "LastEvaluatedKey": first_last}).to_string(),
        serde_json::json!({"Item": operation_row(first, first_at)}).to_string(),
        serde_json::json!({"Items": [projected_operation_row(second, second_at)]}).to_string(),
        serde_json::json!({"Item": operation_row(second, second_at)}).to_string(),
    ];
    let (client, replay) = scripted_client(responses);
    let store = OperationStore::new(client, &tables().session_authority);
    let page = store
        .page(
            workspace(),
            &OperationFilter::default(),
            PageBudget::new(2).expect("a two-row read budget"),
            None,
        )
        .await
        .expect("both provider slices hydrate");
    assert_eq!(
        page.items
            .iter()
            .map(|stored| stored.record.id)
            .collect::<Vec<_>>(),
        [first, second]
    );
    assert_eq!(page.next, None);

    let requests = replay
        .actual_requests()
        .map(|request| {
            serde_json::from_slice::<Value>(
                request
                    .body()
                    .bytes()
                    .expect("the DynamoDB request body is in memory"),
            )
            .expect("the request body is JSON")
        })
        .collect::<Vec<_>>();
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[0]["Limit"].as_u64(), Some(2));
    assert_eq!(requests[2]["Limit"].as_u64(), Some(1));
    assert!(requests[0]["FilterExpression"].is_null());
    assert!(requests[2]["FilterExpression"].is_null());
    assert_eq!(requests[1]["ConsistentRead"].as_bool(), Some(true));
    assert_eq!(requests[3]["ConsistentRead"].as_bool(), Some(true));
    for attribute in ["pk", "sk", "wsIndexPk", "wsIndexSk"] {
        assert_eq!(
            requests[2]["ExclusiveStartKey"][attribute], first_last[attribute],
            "the second slice resumes from the complete first LEK"
        );
    }
}

#[test]
fn a_public_cancel_request_fences_the_exact_observed_operation() {
    let request = OperationCancelRequest {
        workspace: workspace(),
        operation: operation(),
        kind: OperationKind::SessionPersist,
        status: OperationStatus::Running,
        version: 4,
        now: Timestamp::from_unix_millis(1_754_138_096_000).expect("fixture instant"),
    };
    let update = operation_cancel_requested(&tables().session_authority, &request)
        .expect("the public cancellation update builds")
        .build()
        .expect("the update is complete");
    let condition = update.condition_expression().expect("a condition");
    for fence in [
        "workspaceId = :workspaceId",
        "operationId = :operationId",
        "kind = :kind",
        "version = :version",
        "#status = :status",
        "cancelRequested = :false",
        "attribute_not_exists(committedAt)",
    ] {
        assert!(
            condition.contains(fence),
            "missing `{fence}` from `{condition}`"
        );
    }
    assert!(
        update
            .update_expression()
            .contains("cancelRequested = :true")
    );
    assert!(
        !update.update_expression().contains("terminalAt"),
        "a running cancellation is a request for the next fenced step"
    );

    let queued = operation_cancel_requested(
        &tables().session_authority,
        &OperationCancelRequest {
            status: OperationStatus::Queued,
            ..request
        },
    )
    .expect("a queued cancel builds")
    .build()
    .expect("the update is complete");
    assert!(queued.update_expression().contains("#status = :cancelled"));
    assert!(queued.update_expression().contains("terminalAt = :now"));
}

#[test]
fn a_telemetry_export_cancel_is_refused_by_the_session_authority_owner() {
    let request = OperationCancelRequest {
        workspace: workspace(),
        operation: operation(),
        kind: OperationKind::TelemetryExport,
        status: OperationStatus::Queued,
        version: 4,
        now: Timestamp::from_unix_millis(1_754_138_096_000).expect("fixture instant"),
    };

    assert!(operation_cancel_requested(&tables().session_authority, &request).is_err());
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
fn a_decision_moves_the_authoritative_tail_sequence_and_hash_together() {
    let request = decision();
    let plan = compile_decision(&tables(), &request, None).expect("compiles");
    let control = plan.actions()[1].update().expect("the control update");
    assert!(control.update_expression().contains("journalTailHash"));
    assert!(control.update_expression().contains("hasJournal"));
    let values = control
        .expression_attribute_values()
        .expect("control values");
    assert_eq!(
        values[":nextTailHash"].as_s().expect("hash string"),
        &request.entry.entry_id
    );
    assert_eq!(
        values[":hasJournal"].as_bool().expect("journal marker"),
        &true
    );
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
