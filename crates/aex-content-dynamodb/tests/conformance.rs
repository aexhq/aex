//! Request and generation-definition conformance for `regional-content`.
//!
//! Expressions are asserted on the serialized body captured from a real client,
//! because that is what `DynamoDB` evaluates; vocabularies are asserted against
//! the checked-in generation definition, because that is what Terraform creates.

mod support;

use aex_content_dynamodb::codec;
use aex_content_dynamodb::keys;
use aex_content_dynamodb::store::{ContentMetadataStore, ContentStore};
use aex_content_dynamodb::wire_pending::GcSweepPlan;
use aex_session_dynamodb::paging::PageBudget;

use support::{
    DEFINITION, TABLE, captured_body, capturing_client, digest, gc_epoch, grant, now, organization,
    workspace,
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
fn the_scan_index_projection_equals_the_generation_definition() {
    let index = &definition()["globalSecondaryIndexes"][0];
    assert_eq!(index["name"].as_str(), Some(keys::GC_INDEX));
    assert_eq!(index["partition"].as_str(), Some(keys::GC_PK));
    assert_eq!(index["sort"].as_str(), Some(keys::GC_SK));
    assert_eq!(index["projection"]["type"].as_str(), Some("INCLUDE"));
    assert_eq!(
        strings(&index["projection"]["attributes"]),
        keys::GC_PROJECTION,
        "the projection is exhaustive on purpose: an attribute added to it must \
         be a deliberate edit in both places"
    );
}

#[test]
fn the_only_ttl_attribute_is_the_declared_one_and_it_covers_only_disposable_rows() {
    let ttl = &definition()["timeToLive"];
    assert_eq!(ttl["enabled"].as_bool(), Some(true));
    assert_eq!(ttl["attribute"].as_str(), Some("expiresAtEpochSeconds"));
    assert_eq!(
        strings(&ttl["appliesTo"]),
        ["content_pin", "download_grant"],
        "a descriptor or a body must never be reclaimed on a timer"
    );
}

#[test]
fn the_change_feed_never_carries_an_image_of_a_ciphertext_row() {
    let stream = &definition()["stream"];
    assert_eq!(stream["enabled"].as_bool(), Some(true));
    assert_eq!(
        stream["viewType"].as_str(),
        Some("KEYS_ONLY"),
        "this table holds ciphertext bodies; NEW_IMAGE would duplicate every one \
         into stream storage and into the pipe role's blast radius"
    );
}

#[tokio::test]
async fn the_serialized_sweep_names_every_fence_it_depends_on() {
    let (client, receiver) = capturing_client();
    let store = ContentStore::new(client, TABLE);
    let _ignored = store
        .sweep_candidate(&GcSweepPlan {
            workspace: workspace(),
            organization: organization(),
            digest: digest(0xab),
            epoch: 4,
            marked_epoch: 3,
        })
        .await;

    let body = captured_body(receiver);
    let actions = body["TransactItems"].as_array().expect("three actions");
    assert_eq!(
        actions.len(),
        3,
        "DynamoDB refuses two operations on one item, so the descriptor is checked          by the delete that already carries the same condition"
    );
    assert_eq!(
        actions[0]["ConditionCheck"]["ConditionExpression"].as_str(),
        Some("epoch = :epoch AND #state = :sweeping")
    );
    assert_eq!(
        actions[1]["Delete"]["ConditionExpression"].as_str(),
        Some("gcEpoch = :markedEpoch")
    );
    assert_eq!(
        actions[2]["Delete"]["ConditionExpression"].as_str(),
        Some("epoch = :epoch")
    );
    for action in actions {
        let inner = action
            .as_object()
            .expect("one action")
            .values()
            .next()
            .expect("one member");
        assert_eq!(
            inner["ReturnValuesOnConditionCheckFailure"].as_str(),
            Some("ALL_OLD"),
            "a failed sweep condition must return the row that survived"
        );
    }
    let token = body["ClientRequestToken"].as_str().expect("a token");
    assert!(token.len() <= 36, "`{token}` is {} bytes", token.len());
}

#[tokio::test]
async fn the_serialized_grant_mint_writes_the_grant_and_its_pin_in_one_transaction() {
    let (client, receiver) = capturing_client();
    let store = ContentStore::new(client, TABLE);
    let _ignored = store.mint_grant(&grant(), now()).await;

    let body = captured_body(receiver);
    let actions = body["TransactItems"].as_array().expect("two actions");
    assert_eq!(actions.len(), 2);
    for action in actions {
        assert_eq!(
            action["Put"]["ConditionExpression"].as_str(),
            Some("attribute_not_exists(pk)")
        );
    }
    let pin = &actions[1]["Put"]["Item"];
    assert_eq!(pin["itemType"]["S"].as_str(), Some(codec::CONTENT_PIN));
    assert_eq!(pin["pinKind"]["S"].as_str(), Some(codec::GRANT_PIN_KIND));
    assert!(
        pin["expiresAt"]["S"].as_str().is_some(),
        "the sweeper reads `expiresAt`, never the TTL attribute"
    );
}

#[tokio::test]
async fn a_mark_scan_reads_one_bucket_of_the_index_and_never_the_base_table() {
    let (client, receiver) = capturing_client();
    let store = ContentStore::new(client, TABLE);
    let _ignored = store
        .scan_gc_bucket(workspace(), 255, PageBudget::new(25).expect("a page"))
        .await;

    let body = captured_body(receiver);
    assert_eq!(body["IndexName"].as_str(), Some(keys::GC_INDEX));
    assert!(
        body["ExpressionAttributeValues"][":pk"]["S"]
            .as_str()
            .expect("a partition")
            .ends_with("#255")
    );
    assert!(
        body["FilterExpression"].is_null(),
        "a bounded bucket scan needs no filter"
    );
}

#[tokio::test]
async fn the_reachability_read_is_strongly_consistent_and_covers_pins_and_grants_together() {
    let (client, receiver) = capturing_client();
    let store = ContentStore::new(client, TABLE);
    let _ignored = store.reachability(workspace(), &digest(0xab), now()).await;

    let body = captured_body(receiver);
    assert_eq!(
        body["ConsistentRead"].as_bool(),
        Some(true),
        "a delete decision taken from a replica is a delete decision taken blind"
    );
    let values = &body["ExpressionAttributeValues"];
    assert_eq!(values[":low"]["S"].as_str(), Some(keys::grant_pin_prefix()));
    assert!(
        values[":high"]["S"]
            .as_str()
            .expect("an upper bound")
            .starts_with(keys::pin_prefix())
    );
}

#[tokio::test]
async fn every_authority_point_read_is_strongly_consistent() {
    for read in ["descriptor", "body", "grant", "epoch"] {
        let (client, receiver) = capturing_client();
        let store = ContentStore::new(client, TABLE);
        match read {
            "descriptor" => {
                let _ignored = store.load_descriptor(workspace(), &digest(1)).await;
            }
            "body" => {
                let _ignored = store.read_inline_body(workspace(), &digest(1)).await;
            }
            "grant" => {
                let _ignored = store.redeem_grant(&"b".repeat(64), now()).await;
            }
            _ => {
                let _ignored = store.load_gc_epoch(workspace()).await;
            }
        }
        let body = captured_body(receiver);
        assert_eq!(
            body["ConsistentRead"].as_bool(),
            Some(true),
            "`{read}` answered from a replica"
        );
    }
}

#[test]
fn the_epoch_state_machine_only_ever_moves_through_the_declared_states() {
    let epoch = gc_epoch();
    assert!(keys::GC_STATES.contains(&epoch.state.as_str()));
    assert_eq!(keys::GC_STATES, ["idle", "marking", "sweeping"]);
}
