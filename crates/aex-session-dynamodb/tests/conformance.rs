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
use aex_session_dynamodb::attr::s;
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::paging::{PageBudget, PagePosition};
use aex_session_dynamodb::plan::{Participant, TransactionPlan};
use aex_session_dynamodb::store::{
    OperationApiStore, OperationAuthority, OperationFilter, OperationStore, SessionQueries,
    SessionReads, SessionScoped,
};
use aex_session_dynamodb::transactions::{
    DECISION_ORDER, Foreign, ForeignAction, OperationCancelRequest, OperationStepCancel,
    compile_decision, compile_fanout_page, operation_cancel_requested, operation_cancelled,
};
use aex_wire::ids::{OperationId, PrefixedId as _, Uuid7};
use aex_wire::types::Timestamp;
use serde_json::Value;

use support::{
    captured_body, capturing_client, decision, fanout, operation, run_id, scripted_client, session,
    tables, workspace,
};

#[tokio::test]
async fn a_canonical_session_point_read_is_strong_and_targets_only_the_head() {
    let (client, receiver) = capturing_client();
    let reads = SessionReads::new(client, &tables().session_authority);
    let _ignored = reads.load_session(workspace(), session()).await;

    let body = captured_body(receiver);
    assert_eq!(body["ConsistentRead"], true);
    assert_eq!(body["Key"]["pk"]["S"], format!("SESSION#{}", session()));
    assert_eq!(body["Key"]["sk"]["S"], "HEAD");
}

#[tokio::test]
async fn a_scoped_run_point_read_is_one_atomic_two_item_read() {
    let (client, receiver) = capturing_client();
    let reads = SessionReads::new(client, &tables().session_authority);
    let _ignored = reads.load_run(workspace(), session(), run_id()).await;

    let body = captured_body(receiver);
    let reads = body["TransactItems"]
        .as_array()
        .expect("a transactional point read");
    assert_eq!(reads.len(), 2);
    assert_eq!(
        reads[0]["Get"]["Key"]["pk"]["S"],
        format!("SESSION#{}", session())
    );
    assert_eq!(reads[0]["Get"]["Key"]["sk"]["S"], "HEAD");
    assert_eq!(
        reads[1]["Get"]["Key"]["pk"]["S"],
        format!("SESSION#{}", session())
    );
    assert_eq!(
        reads[1]["Get"]["Key"]["sk"]["S"],
        format!("RUN#{}", run_id())
    );
}

fn dynamo_json_item(item: &aex_session_dynamodb::attr::Item) -> Value {
    Value::Object(
        item.iter()
            .map(|(name, value)| {
                let value = match value {
                    aws_sdk_dynamodb::types::AttributeValue::S(value) => {
                        serde_json::json!({"S": value})
                    }
                    aws_sdk_dynamodb::types::AttributeValue::N(value) => {
                        serde_json::json!({"N": value})
                    }
                    aws_sdk_dynamodb::types::AttributeValue::Bool(value) => {
                        serde_json::json!({"BOOL": value})
                    }
                    other => panic!("canonical session fixtures use no {other:?} attribute"),
                };
                (name.clone(), value)
            })
            .collect(),
    )
}

fn scoped_run_response(
    session: &aex_session_domain::Session,
    run: &aex_session_domain::Run,
) -> String {
    let head = aex_session_dynamodb::authority_codec::encode_session(session)
        .expect("canonical session row");
    let run = aex_session_dynamodb::authority_codec::encode_domain_run(
        run,
        session.workspace,
        session.organization,
    )
    .expect("canonical run row");
    serde_json::json!({
        "Responses": [
            {"Item": dynamo_json_item(&head)},
            {"Item": dynamo_json_item(&run)}
        ]
    })
    .to_string()
}

fn session_head_response(session: &aex_session_domain::Session) -> String {
    let head = aex_session_dynamodb::authority_codec::encode_session(session)
        .expect("canonical session row");
    serde_json::json!({"Item": dynamo_json_item(&head)}).to_string()
}

fn request_bodies(replay: &aws_smithy_http_client::test_util::StaticReplayClient) -> Vec<Value> {
    replay
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
        .collect()
}

#[tokio::test]
async fn a_session_page_uses_two_parent_reads_only_and_reuses_a_resumed_epoch() {
    let session = aex_session_domain::testing::session_fixture();
    let empty_query = serde_json::json!({"Items": []}).to_string();

    let (client, first_replay) = scripted_client(vec![
        session_head_response(&session),
        empty_query.clone(),
        session_head_response(&session),
    ]);
    let reads = SessionReads::new(client, &tables().session_authority);
    let first = reads
        .page_runs(
            session.workspace,
            session.id,
            None,
            PageBudget::new(1).expect("a page"),
            None,
        )
        .await
        .expect("a fenced first page");
    assert!(matches!(
        first,
        SessionScoped::Active(page) if page.deletion_epoch == session.deletion.epoch
    ));
    let first_requests = request_bodies(&first_replay);
    assert_eq!(first_requests.len(), 3, "parent, query, parent");
    assert_eq!(first_requests[0]["ConsistentRead"], true);
    assert_eq!(first_requests[1]["ConsistentRead"], true);
    assert_eq!(first_requests[2]["ConsistentRead"], true);

    let (client, resumed_replay) =
        scripted_client(vec![empty_query, session_head_response(&session)]);
    let reads = SessionReads::new(client, &tables().session_authority);
    let resumed = reads
        .page_runs(
            session.workspace,
            session.id,
            Some(session.deletion.epoch),
            PageBudget::new(1).expect("a page"),
            None,
        )
        .await
        .expect("a fenced resumed page");
    assert!(matches!(resumed, SessionScoped::Active(_)));
    let resumed_requests = request_bodies(&resumed_replay);
    assert_eq!(
        resumed_requests.len(),
        2,
        "the edge's prevalidated parent is the resumed before-read"
    );
    assert_eq!(resumed_requests[0]["ConsistentRead"], true);
    assert_eq!(resumed_requests[1]["ConsistentRead"], true);
}

#[tokio::test]
async fn a_scoped_point_read_hides_foreign_tenants_and_refuses_deleted_parents() {
    let (session, run, _agent, _message) = aex_session_domain::testing::running_session();

    let (client, _replay) = scripted_client(vec![scoped_run_response(&session, &run)]);
    let reads = SessionReads::new(client, &tables().session_authority);
    assert!(matches!(
        reads.load_run(session.workspace, session.id, run.id).await,
        Ok(SessionScoped::Active(Some(found))) if found == run
    ));

    let (client, _replay) = scripted_client(vec![scoped_run_response(&session, &run)]);
    let reads = SessionReads::new(client, &tables().session_authority);
    let another_workspace = aex_session_domain::testing::id(99);
    assert_eq!(
        reads
            .load_run(another_workspace, session.id, run.id)
            .await
            .expect("a hidden foreign row"),
        SessionScoped::Missing
    );

    let mut deleted = session.clone();
    deleted.deletion.state = aex_session_domain::DeletionState::Trashed;
    deleted.status = aex_session_domain::SessionStatus::Trashed;
    let (client, _replay) = scripted_client(vec![scoped_run_response(&deleted, &run)]);
    let reads = SessionReads::new(client, &tables().session_authority);
    assert_eq!(
        reads
            .load_run(deleted.workspace, deleted.id, run.id)
            .await
            .expect("a typed deletion fence"),
        SessionScoped::Deleted
    );
}

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
    object.retain(|attribute, _| {
        ["pk", "sk", "wsIndexPk", "wsIndexSk"].contains(&attribute.as_str())
    });
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
