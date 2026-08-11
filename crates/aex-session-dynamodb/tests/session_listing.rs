//! Exact workspace-session listing authority.
//!
//! The workspace GSI is only an ordered locator. Every selected head is read
//! strongly before lifecycle/status filtering, and the continuation remains
//! the complete four-part `DynamoDB` position owned by the regional edge.

mod support;

use aex_session_domain::{DeletionState, Session, SessionStatus, WorkAdmission};
use aex_session_dynamodb::paging::{PageBudget, PagePosition};
use aex_session_dynamodb::store::{
    SessionListFilter, SessionListStatus, SessionQueries, SessionReads,
};
use aex_wire::ids::{OperationId, PrefixedId as _};
use aex_wire::types::Timestamp;
use serde_json::Value;

use support::{captured_body, capturing_client, scripted_client, tables};

fn fixture(status: SessionStatus) -> Session {
    let mut session = aex_session_domain::testing::session_fixture();
    session.status = status;
    match status {
        SessionStatus::Idle | SessionStatus::Running | SessionStatus::AwaitingApproval => {}
        SessionStatus::Trashed => {
            session.deletion.state = DeletionState::Trashed;
            session.deletion.trashed_at = Some(session.updated_at);
            session.work_admission = WorkAdmission::Trashing;
        }
        SessionStatus::Purging => {
            session.deletion.state = DeletionState::Purging;
            session.deletion.trashed_at = Some(session.updated_at);
            session.deletion.purge_operation = Some(OperationId::from_uuid7(
                aex_wire::ids::Uuid7::compose(2, [9; 10]),
            ));
            session.work_admission = WorkAdmission::Purging;
        }
    }
    session
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

fn responses(session: &Session, continuation: bool) -> Vec<String> {
    let head = aex_session_dynamodb::authority_codec::encode_session(session)
        .expect("the canonical head encodes");
    let mut projected = dynamo_json_item(&head);
    projected
        .as_object_mut()
        .expect("an object")
        .retain(|attribute, _| {
            ["pk", "sk", "wsIndexPk", "wsIndexSk"].contains(&attribute.as_str())
        });
    let last = continuation.then(|| projected.clone());
    vec![
        serde_json::json!({"Items": [projected], "LastEvaluatedKey": last}).to_string(),
        serde_json::json!({"Item": dynamo_json_item(&head)}).to_string(),
    ]
}

#[tokio::test]
async fn one_lifecycle_neutral_partition_is_snapshot_bounded_and_cursor_ready() {
    let session = fixture(SessionStatus::Trashed);
    let partition =
        aex_session_dynamodb::keys::workspace_index::session_partition(session.workspace);
    let after = PagePosition {
        pk: format!("SESSION#{}", session.id),
        sk: "HEAD".to_owned(),
        index_pk: Some(partition.clone()),
        index_sk: Some(aex_session_dynamodb::keys::workspace_index::session_sort(
            session.created_at,
            session.id,
        )),
    };
    let (client, receiver) = capturing_client();
    let reads = SessionReads::new(client, &tables().session_authority);
    let _ignored = reads
        .page_sessions(
            session.workspace,
            &SessionListFilter::default(),
            session.updated_at,
            PageBudget::new(25).expect("a page"),
            Some(&after),
        )
        .await;

    let body = captured_body(receiver);
    assert_eq!(body["IndexName"], "gsi_workspace_index");
    assert_eq!(body["ConsistentRead"], false);
    assert_eq!(body["ScanIndexForward"], true);
    assert_eq!(body["Limit"], 25);
    assert!(body["FilterExpression"].is_null());
    assert_eq!(
        body["ExpressionAttributeValues"][":workspace"]["S"],
        partition
    );
    assert_eq!(
        body["ExpressionAttributeValues"][":snapshot"]["S"],
        aex_session_dynamodb::keys::workspace_index::session_snapshot_sort(session.updated_at)
    );
    for attribute in ["pk", "sk", "wsIndexPk", "wsIndexSk"] {
        assert_eq!(
            body["ExclusiveStartKey"][attribute],
            serde_json::json!({"S": match attribute {
                "pk" => after.pk.as_str(),
                "sk" => after.sk.as_str(),
                "wsIndexPk" => after.index_pk.as_deref().expect("index pk"),
                "wsIndexSk" => after.index_sk.as_deref().expect("index sk"),
                _ => unreachable!(),
            }})
        );
    }
}

#[tokio::test]
async fn deleting_matches_both_trashed_and_purging_but_not_live() {
    for (status, expected) in [
        (SessionStatus::Idle, false),
        (SessionStatus::Trashed, true),
        (SessionStatus::Purging, true),
    ] {
        let session = fixture(status);
        let (client, _replay) = scripted_client(responses(&session, false));
        let reads = SessionReads::new(client, &tables().session_authority);
        let page = reads
            .page_sessions(
                session.workspace,
                &SessionListFilter {
                    status: Some(SessionListStatus::Deleting),
                },
                Timestamp::from_unix_millis(session.created_at.unix_millis() + 1)
                    .expect("a snapshot"),
                PageBudget::new(1).expect("one physical row"),
                None,
            )
            .await
            .expect("the index locator hydrates");
        assert_eq!(page.items.len(), usize::from(expected), "{status:?}");
    }
}

#[tokio::test]
async fn the_continuation_is_the_complete_provider_position() {
    let session = fixture(SessionStatus::Purging);
    let (client, _replay) = scripted_client(responses(&session, true));
    let reads = SessionReads::new(client, &tables().session_authority);
    let page = reads
        .page_sessions(
            session.workspace,
            &SessionListFilter::default(),
            Timestamp::from_unix_millis(session.created_at.unix_millis() + 1).expect("a snapshot"),
            PageBudget::new(1).expect("one physical row"),
            None,
        )
        .await
        .expect("the row hydrates");
    assert_eq!(page.items, vec![session.clone()]);
    assert_eq!(
        page.next,
        Some(PagePosition {
            pk: format!("SESSION#{}", session.id),
            sk: "HEAD".to_owned(),
            index_pk: Some(
                aex_session_dynamodb::keys::workspace_index::session_partition(session.workspace),
            ),
            index_sk: Some(aex_session_dynamodb::keys::workspace_index::session_sort(
                session.created_at,
                session.id,
            )),
        })
    );
}
