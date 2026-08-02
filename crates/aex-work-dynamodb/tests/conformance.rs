//! Request conformance for `regional-work`.
//!
//! The expressions are asserted on the serialized body, captured from a real
//! client, because that is what `DynamoDB` evaluates.

mod support;

use aex_session_dynamodb::paging::PageBudget;
use aex_work_dynamodb::WorkClaim;
use aex_work_dynamodb::codec::ReconciliationCursor;
use aex_work_dynamodb::keys;
use aex_work_dynamodb::store::{WorkAuthority, WorkStore};

use support::{TABLE, captured_body, capturing_client, later, now, record, workspace};

fn hold() -> WorkClaim {
    WorkClaim {
        work_id: record().work_id,
        fence: 3,
        owner: "worker-1".to_owned(),
        attempt: 1,
        lease_expires_at: later(30_000),
    }
}

#[tokio::test]
async fn the_serialized_claim_carries_the_declared_condition_and_all_old_return_values() {
    let (client, receiver) = capturing_client();
    let store = WorkStore::new(client, TABLE);
    let _ignored = store
        .claim_work(&record().work_id, "worker-1", now(), later(30_000))
        .await;

    let body = captured_body(receiver);
    assert_eq!(body["TableName"].as_str(), Some(TABLE));
    let condition = body["ConditionExpression"]
        .as_str()
        .expect("the claim is conditional");
    assert!(condition.contains("#state IN (:pending, :claimed)"));
    assert!(condition.contains("attempt < maxAttempts"));
    assert!(condition.contains("(#state = :pending OR leaseExpiresAt < :now)"));
    assert_eq!(
        body["ReturnValuesOnConditionCheckFailure"].as_str(),
        Some("ALL_OLD")
    );
    assert_eq!(body["ReturnValues"].as_str(), Some("ALL_NEW"));
}

#[tokio::test]
async fn the_serialized_retirement_removes_both_due_index_attributes_and_sets_the_ttl() {
    let (client, receiver) = capturing_client();
    let store = WorkStore::new(client, TABLE);
    let _ignored = store.complete_work(&hold(), now()).await;

    let body = captured_body(receiver);
    let update = body["UpdateExpression"]
        .as_str()
        .expect("an update expression");
    assert!(update.contains("REMOVE dueShardPk, dueShardSk"));
    assert!(update.contains("expiresAtEpochSeconds = :ttl"));
    let condition = body["ConditionExpression"].as_str().expect("conditional");
    assert!(condition.contains("fence = :fence"));
    assert!(condition.contains("claimOwner = :owner"));
}

#[tokio::test]
async fn a_due_scan_queries_a_shard_of_the_index_and_never_a_literal_single_partition() {
    let (client, receiver) = capturing_client();
    let store = WorkStore::new(client, TABLE);
    let _ignored = store
        .scan_due(7, now(), PageBudget::new(25).expect("a page"))
        .await;

    let body = captured_body(receiver);
    assert_eq!(body["IndexName"].as_str(), Some(keys::DUE_INDEX));
    assert_eq!(
        body["ExpressionAttributeValues"][":pk"]["S"].as_str(),
        Some("DUE#0007"),
        "the due partition is always sharded"
    );
    assert_eq!(
        body["KeyConditionExpression"].as_str(),
        Some("#pk = :pk AND #sk <= :now")
    );
    let upper = body["ExpressionAttributeValues"][":now"]["S"]
        .as_str()
        .expect("the due upper bound");
    assert_eq!(upper, format!("{}#\u{10ffff}", now().to_wire()));
}

#[tokio::test]
async fn a_resumed_due_scan_starts_strictly_after_all_four_index_and_base_keys() {
    let (client, receiver) = capturing_client();
    let store = WorkStore::new(client, TABLE);
    let position = ReconciliationCursor {
        shard: 7,
        scanned_through_effective_due_at: now(),
        last_work_id: Some(record().work_id.clone()),
        revision: 3,
        updated_at: now(),
    };
    let _ignored = store
        .scan_due_after(
            7,
            later(60_000),
            PageBudget::new(25).expect("a page"),
            Some(&position),
        )
        .await;

    let body = captured_body(receiver);
    let start = &body["ExclusiveStartKey"];
    assert_eq!(
        start["pk"]["S"].as_str(),
        Some(format!("WORK#{}", record().work_id).as_str())
    );
    assert_eq!(start["sk"]["S"].as_str(), Some("STATE"));
    assert_eq!(start["dueShardPk"]["S"].as_str(), Some("DUE#0007"));
    assert_eq!(
        start["dueShardSk"]["S"].as_str(),
        Some(format!("{}#{}", now().to_wire(), record().work_id).as_str())
    );
}

#[tokio::test]
async fn a_reconciliation_cursor_read_is_strong_and_targets_its_exact_shard() {
    let (client, receiver) = capturing_client();
    let store = WorkStore::new(client, TABLE);
    let _ignored = store.load_cursor(7).await;

    let body = captured_body(receiver);
    assert_eq!(body["ConsistentRead"].as_bool(), Some(true));
    assert_eq!(body["Key"]["pk"]["S"].as_str(), Some("WCURSOR#0007"));
    assert_eq!(body["Key"]["sk"]["S"].as_str(), Some("STATE"));
}

#[tokio::test]
async fn a_point_read_of_a_work_record_is_strongly_consistent() {
    let (client, receiver) = capturing_client();
    let store = WorkStore::new(client, TABLE);
    let _ignored = store.load(workspace(), &record().work_id).await;

    let body = captured_body(receiver);
    assert_eq!(
        body["ConsistentRead"].as_bool(),
        Some(true),
        "an authority that answers from a replica cannot fence anything"
    );
}

#[tokio::test]
async fn a_cursor_advance_conditions_on_the_previous_revision() {
    let (client, receiver) = capturing_client();
    let store = WorkStore::new(client, TABLE);
    let _ignored = store
        .advance_cursor(&ReconciliationCursor {
            shard: 3,
            scanned_through_effective_due_at: now(),
            last_work_id: None,
            revision: 8,
            updated_at: now(),
        })
        .await;

    let body = captured_body(receiver);
    assert_eq!(
        body["ConditionExpression"].as_str(),
        Some("attribute_not_exists(pk) OR revision = :previous")
    );
    assert_eq!(
        body["ExpressionAttributeValues"][":previous"]["N"].as_str(),
        Some("7")
    );
}
