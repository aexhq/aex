//! Request and generation-definition conformance for `regional-registry`.

mod support;

use aex_content_domain::identity::{RegistryKind, Revision};
use aex_registry_dynamodb::keys;
use aex_registry_dynamodb::store::{RegistryDynamoStore, RegistryStore};
use aex_session_dynamodb::paging::PageBudget;
use aex_workspace_domain::upload::UploadState;

use support::{
    DEFINITION, TABLE, captured_body, capturing_client, next_pointer, pointer, upload, upload_id,
    workspace,
};

fn definition() -> serde_json::Value {
    serde_json::from_str(DEFINITION).expect("the generation definition is JSON")
}

#[test]
fn the_item_type_vocabulary_equals_the_generation_definition() {
    let declared: Vec<String> = definition()["itemTypes"]
        .as_array()
        .expect("an array")
        .iter()
        .map(|entry| entry.as_str().expect("a string").to_owned())
        .collect();
    assert_eq!(declared, keys::ITEM_TYPES);
}

#[test]
fn this_table_carries_no_index_and_no_stream() {
    let definition = definition();
    assert!(
        definition["globalSecondaryIndexes"]
            .as_array()
            .expect("an array")
            .is_empty(),
        "list-by-kind is a native Query, so the previous workspace-created-index \
         has no successor (D-22)"
    );
    assert_eq!(definition["stream"]["enabled"].as_bool(), Some(false));
}

#[test]
fn only_a_receipt_is_ever_reclaimed_on_a_timer() {
    let ttl = &definition()["timeToLive"];
    assert_eq!(ttl["attribute"].as_str(), Some("expiresAtEpochSeconds"));
    let applies: Vec<String> = ttl["appliesTo"]
        .as_array()
        .expect("an array")
        .iter()
        .map(|entry| entry.as_str().expect("a string").to_owned())
        .collect();
    assert_eq!(
        applies,
        ["idempotency_receipt"],
        "an upload row must outlive its own expiry so the sweeper can abort the \
         multipart upload before the row goes"
    );
}

#[tokio::test]
async fn a_first_write_of_a_name_is_conditional_on_its_absence() {
    let (client, receiver) = capturing_client();
    let store = RegistryDynamoStore::new(client, TABLE);
    let _ignored = store.put_pointer(&pointer(), None).await;

    let body = captured_body(receiver);
    assert_eq!(
        body["ConditionExpression"].as_str(),
        Some("attribute_not_exists(pk)")
    );
    assert_eq!(
        body["ReturnValuesOnConditionCheckFailure"].as_str(),
        Some("ALL_OLD")
    );
    assert_eq!(body["Item"]["kind"]["S"].as_str(), Some("tool"));
    assert_eq!(body["Item"]["pk"]["S"].as_str().map(str::to_owned), {
        Some(keys::kind_partition(workspace(), RegistryKind::Tool))
    });
}

#[tokio::test]
async fn a_replacement_is_conditional_on_the_revision_the_caller_observed() {
    let (client, receiver) = capturing_client();
    let store = RegistryDynamoStore::new(client, TABLE);
    let _ignored = store
        .put_pointer(&next_pointer(), Some(Revision::FIRST))
        .await;

    let body = captured_body(receiver);
    assert_eq!(
        body["ConditionExpression"].as_str(),
        Some("attribute_exists(pk) AND revision = :fromRevision")
    );
    assert_eq!(
        body["ExpressionAttributeValues"][":fromRevision"]["N"].as_str(),
        Some("1")
    );
}

#[tokio::test]
async fn a_listing_walks_one_partition_with_native_pagination_and_no_filter() {
    let (client, receiver) = capturing_client();
    let store = RegistryDynamoStore::new(client, TABLE);
    let _ignored = store
        .list_pointers(
            workspace(),
            RegistryKind::Skill,
            PageBudget::new(25).expect("a page"),
            None,
        )
        .await;

    let body = captured_body(receiver);
    assert!(body["IndexName"].is_null(), "there is no index to name");
    assert!(body["FilterExpression"].is_null());
    assert_eq!(
        body["KeyConditionExpression"].as_str(),
        Some("#pk = :pk AND begins_with(#sk, :prefix)")
    );
    assert_eq!(
        body["ExpressionAttributeValues"][":prefix"]["S"].as_str(),
        Some(keys::name_prefix())
    );
    assert_eq!(body["Limit"].as_u64(), Some(25));
}

#[tokio::test]
async fn a_completion_begins_under_a_manifest_identity() {
    let (client, receiver) = capturing_client();
    let store = RegistryDynamoStore::new(client, TABLE);
    let _ignored = store.begin_completion(&upload(), &"a".repeat(64)).await;

    let body = captured_body(receiver);
    let condition = body["ConditionExpression"].as_str().expect("conditional");
    assert!(condition.contains("attribute_not_exists(consumedByName)"));
    assert!(condition.contains("completionIntentHash = :hash"));
    assert!(
        condition.contains("providerUploadId = :handle"),
        "a completion is fenced on the handle it was decided against: {condition}"
    );
    // The submitted manifest lands with the intent, so a retried completion is
    // deterministic rather than dependent on the client resending identical input.
    assert_eq!(
        body["UpdateExpression"].as_str(),
        Some("SET #state = :completing, completionIntentHash = :hash, completionManifest = :manifest")
    );
}

#[tokio::test]
async fn an_upload_transition_names_the_state_it_moves_from() {
    let (client, receiver) = capturing_client();
    let store = RegistryDynamoStore::new(client, TABLE);
    let _ignored = store
        .transition_upload(upload_id(), UploadState::PartsGranted, UploadState::Aborted)
        .await;

    let body = captured_body(receiver);
    assert_eq!(
        body["ExpressionAttributeValues"][":from"]["S"].as_str(),
        Some("parts_granted")
    );
    assert_eq!(
        body["ExpressionAttributeValues"][":to"]["S"].as_str(),
        Some("aborted")
    );
}

#[tokio::test]
async fn every_authority_point_read_is_strongly_consistent() {
    for read in ["pointer", "upload"] {
        let (client, receiver) = capturing_client();
        let store = RegistryDynamoStore::new(client, TABLE);
        if read == "pointer" {
            let _ignored = store
                .load_pointer(workspace(), RegistryKind::Tool, "search")
                .await;
        } else {
            let _ignored = store.load_upload(workspace(), upload().id).await;
        }
        let body = captured_body(receiver);
        assert_eq!(
            body["ConsistentRead"].as_bool(),
            Some(true),
            "`{read}` answered from a replica"
        );
    }
}
