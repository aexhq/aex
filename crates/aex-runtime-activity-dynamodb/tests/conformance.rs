//! Request and generation-definition conformance for `runtime-activity`.

mod support;

use aex_hands_protocol::rpc::Fence;
use aex_runtime_activity_dynamodb::keys;
use aex_runtime_activity_dynamodb::store::RuntimeActivityDynamoStore;
use aex_runtime_control::generation::{GenerationState, Revision};
use aex_session_dynamodb::paging::PageBudget;

use support::{
    DEFINITION, TABLE, captured_body, capturing_client, generation, head, intent, now, probe,
    receipt, session, workspace,
};

fn definition() -> serde_json::Value {
    serde_json::from_str(DEFINITION).expect("the generation definition is JSON")
}

fn strings(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .expect("an array")
        .iter()
        .map(|entry| entry.as_str().expect("a string").to_owned())
        .collect()
}

#[test]
fn the_item_type_vocabulary_equals_the_generation_definition() {
    assert_eq!(strings(&definition()["itemTypes"]), keys::ITEM_TYPES);
}

#[test]
fn the_due_index_projection_equals_the_generation_definition() {
    let index = &definition()["globalSecondaryIndexes"][0];
    assert_eq!(index["name"].as_str(), Some(keys::DUE_INDEX));
    assert_eq!(index["partition"].as_str(), Some(keys::DUE_PK));
    assert_eq!(index["sort"].as_str(), Some(keys::DUE_SK));
    assert_eq!(index["sparse"].as_bool(), Some(true));
    assert_eq!(
        strings(&index["projection"]["attributes"]),
        keys::DUE_PROJECTION
    );
}

#[test]
fn only_a_probe_is_ever_reclaimed_on_a_timer() {
    let ttl = &definition()["timeToLive"];
    assert_eq!(
        strings(&ttl["appliesTo"]),
        ["idle_probe"],
        "a receipt is lifecycle evidence a usage fact references; reclaiming one \
         on a timer would remove the basis of a bill"
    );
}

#[test]
fn the_change_feed_carries_an_image_because_the_table_holds_no_customer_content() {
    let stream = &definition()["stream"];
    assert_eq!(stream["enabled"].as_bool(), Some(true));
    assert_eq!(stream["viewType"].as_str(), Some("NEW_IMAGE"));
}

#[tokio::test]
async fn a_transition_is_conditional_on_the_exact_fence_and_revision() {
    let (client, receiver) = capturing_client();
    let store = RuntimeActivityDynamoStore::new(client, TABLE);
    let _ignored = store
        .transition(
            &head(GenerationState::Running),
            GenerationState::Launching,
            Fence(2),
            Revision::new(4),
            now(),
        )
        .await;

    let body = captured_body(receiver);
    let condition = body["ConditionExpression"].as_str().expect("conditional");
    assert!(condition.contains("fence = :fromFence"));
    assert!(condition.contains("revision = :fromRevision"));
    assert!(condition.contains("#state = :fromState"));
    assert_eq!(
        body["ReturnValuesOnConditionCheckFailure"].as_str(),
        Some("ALL_OLD")
    );
}

#[tokio::test]
async fn a_terminal_transition_removes_the_due_index_attributes() {
    let (client, receiver) = capturing_client();
    let store = RuntimeActivityDynamoStore::new(client, TABLE);
    let _ignored = store
        .transition(
            &head(GenerationState::Terminated),
            GenerationState::Terminating,
            Fence(3),
            Revision::new(5),
            now(),
        )
        .await;

    let body = captured_body(receiver);
    assert!(
        body["UpdateExpression"]
            .as_str()
            .expect("an update")
            .contains("REMOVE rtDuePk, rtDueSk")
    );
}

#[tokio::test]
async fn a_receipt_is_written_immutably() {
    let (client, receiver) = capturing_client();
    let store = RuntimeActivityDynamoStore::new(client, TABLE);
    let _ignored = store.settle_intent(&receipt()).await;

    let body = captured_body(receiver);
    assert_eq!(
        body["ConditionExpression"].as_str(),
        Some("attribute_not_exists(pk)"),
        "a second settlement must lose rather than overwrite lifecycle evidence"
    );
    assert!(
        body["Item"]["expiresAtEpochSeconds"].is_null(),
        "a receipt carries no TTL"
    );
}

#[tokio::test]
async fn a_probe_carries_the_only_ttl_on_this_table() {
    let (client, receiver) = capturing_client();
    let store = RuntimeActivityDynamoStore::new(client, TABLE);
    let _ignored = store.record_probe(&probe()).await;

    let body = captured_body(receiver);
    let ttl = body["Item"]["expiresAtEpochSeconds"]["N"]
        .as_str()
        .expect("a TTL")
        .parse::<i64>()
        .expect("an integer");
    assert_eq!(
        ttl,
        now().unix_millis().div_euclid(1_000) + keys::PROBE_TTL_SECONDS
    );
}

#[tokio::test]
async fn a_due_scan_reads_one_shard_of_the_index_and_never_a_literal_partition() {
    let (client, receiver) = capturing_client();
    let store = RuntimeActivityDynamoStore::new(client, TABLE);
    let _ignored = store
        .scan_due(7, now(), PageBudget::new(25).expect("a page"))
        .await;

    let body = captured_body(receiver);
    assert_eq!(body["IndexName"].as_str(), Some(keys::DUE_INDEX));
    assert_eq!(
        body["ExpressionAttributeValues"][":pk"]["S"].as_str(),
        Some("RTDUE#07")
    );
    assert_eq!(
        body["KeyConditionExpression"].as_str(),
        Some("#pk = :pk AND #sk <= :now")
    );
    let upper = body["ExpressionAttributeValues"][":now"]["S"]
        .as_str()
        .expect("the due upper bound");
    assert_eq!(
        upper,
        format!("{}#\u{10ffff}", now().to_wire()),
        "a bare trailing separator sorts before every `#{{generationId}}` and \
         would miss generations due at exactly `now`"
    );
}

/// The reaper path and the runtime-control port must agree on the millisecond
/// boundary: the sort key is `{ts}#{generationId}`, so an upper bound that
/// ends at the separator excludes every generation due at the scan instant.
#[tokio::test]
async fn the_port_due_scan_includes_the_whole_scan_millisecond() {
    use aex_runtime_control::store::{
        PageBudget as PortPageBudget, RuntimeActivityStore, RuntimeShard,
    };

    let (client, receiver) = capturing_client();
    let store = RuntimeActivityDynamoStore::new(client, TABLE);
    // Fully qualified: the adapter also exposes an inherent reaper `scan_due`,
    // and this case is about the port the runtime-control loop calls.
    let _ignored = RuntimeActivityStore::scan_due(
        &store,
        RuntimeShard(7),
        now(),
        PortPageBudget {
            max_items: 25,
            max_reads: 100,
        },
    )
    .await;

    let body = captured_body(receiver);
    assert_eq!(body["IndexName"].as_str(), Some(keys::DUE_INDEX));
    assert_eq!(
        body["ExpressionAttributeValues"][":now"]["S"].as_str(),
        Some(format!("{}#\u{10ffff}", now().to_wire()).as_str())
    );
}

#[tokio::test]
async fn every_authority_point_read_is_strongly_consistent() {
    for read in ["current", "generation"] {
        let (client, receiver) = capturing_client();
        let store = RuntimeActivityDynamoStore::new(client, TABLE);
        if read == "current" {
            let _ignored = store.load_current(session()).await;
        } else {
            let _ignored = store
                .load_generation(workspace(), session(), generation(4))
                .await;
        }
        let body = captured_body(receiver);
        assert_eq!(
            body["ConsistentRead"].as_bool(),
            Some(true),
            "`{read}` answered from a replica"
        );
    }
    let _ = intent();
}
