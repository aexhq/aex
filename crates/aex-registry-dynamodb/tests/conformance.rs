//! Request and generation-definition conformance for `regional-registry`.

mod support;

use aex_content_domain::identity::{RegistryKind, Revision};
use aex_registry_dynamodb::keys;
use aex_registry_dynamodb::store::{RegistryDynamoStore, RegistryStore, SetCommit};
use aex_session_dynamodb::paging::PageBudget;
use aex_workspace_domain::upload::UploadState;

use support::{
    DEFINITION, TABLE, captured_body, capturing_client, created_commit, name, pointer, receipt_for,
    receipt_for_attempt, replaced_commit, upload, upload_id, workspace,
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

/// A create is one transaction: the pointer, the entry claim and the receipt.
///
/// The receipt participates in the same transaction as the pointer, which is
/// what makes a replay resolvable from the condition failure alone rather than
/// from a second read (D-8).
#[tokio::test]
async fn a_first_write_of_a_name_is_one_transaction_conditional_on_its_absence() {
    let (client, receiver) = capturing_client();
    let store = RegistryDynamoStore::new(client, TABLE);
    let commit = created_commit();
    let receipt = receipt_for(&commit);
    let _ignored = store
        .commit_set(
            workspace(),
            SetCommit {
                commit: &commit,
                from_revision: None,
                entries_cap: 1_000,
                creates: true,
                receipt: &receipt,
            },
        )
        .await;

    let body = captured_body(receiver);
    let actions = body["TransactItems"]
        .as_array()
        .expect("a transaction carries actions");
    assert_eq!(actions.len(), 3, "pointer, entry claim and receipt");
    let pointer_put = &actions[0]["Put"];
    assert_eq!(
        pointer_put["ConditionExpression"].as_str(),
        Some("attribute_not_exists(pk)")
    );
    assert_eq!(
        pointer_put["ReturnValuesOnConditionCheckFailure"].as_str(),
        Some("ALL_OLD")
    );
    assert_eq!(pointer_put["Item"]["kind"]["S"].as_str(), Some("tool"));
    assert!(
        pointer_put["Item"]["valueDoc"]["S"].is_string(),
        "the value lives on the pointer row"
    );
    assert!(
        pointer_put["Item"]["valueKind"].is_null() && pointer_put["Item"]["valueId"].is_null(),
        "a durable pointer names a digest and nothing else (D-15)"
    );
    assert_eq!(
        pointer_put["Item"]["pk"]["S"].as_str().map(str::to_owned),
        { Some(keys::kind_partition(workspace(), RegistryKind::Tool)) }
    );
    let claim = &actions[1]["Update"];
    assert_eq!(
        claim["ConditionExpression"].as_str(),
        Some("attribute_not_exists(#count) OR #count < :cap"),
        "the cap is the fence, not a preceding count"
    );
    assert_eq!(
        actions[2]["Put"]["ConditionExpression"].as_str(),
        Some("attribute_not_exists(pk)"),
        "the receipt is what a replay loses against"
    );
}

#[tokio::test]
async fn distinct_set_intents_have_distinct_transport_identities() {
    let (first_client, first_receiver) = capturing_client();
    let first_store = RegistryDynamoStore::new(first_client, TABLE);
    let first = created_commit();
    let first_receipt = receipt_for_attempt(
        &first,
        aex_workspace_domain::registry::SetOutcome::Created,
        1,
    );
    let _ignored = first_store
        .commit_set(
            workspace(),
            SetCommit {
                commit: &first,
                from_revision: None,
                entries_cap: 1_000,
                creates: true,
                receipt: &first_receipt,
            },
        )
        .await;
    let first_body = captured_body(first_receiver);

    let (second_client, second_receiver) = capturing_client();
    let second_store = RegistryDynamoStore::new(second_client, TABLE);
    let mut second = created_commit();
    second.pointer.row.name = name("another-name");
    let second_receipt = receipt_for_attempt(
        &second,
        aex_workspace_domain::registry::SetOutcome::Created,
        2,
    );
    let _ignored = second_store
        .commit_set(
            workspace(),
            SetCommit {
                commit: &second,
                from_revision: None,
                entries_cap: 1_000,
                creates: true,
                receipt: &second_receipt,
            },
        )
        .await;
    let second_body = captured_body(second_receiver);

    assert_ne!(
        first_body["ClientRequestToken"], second_body["ClientRequestToken"],
        "DynamoDB treats one token with different transaction parameters as an error"
    );
}

/// A replace claims nothing: it occupies a name the workspace already holds.
#[tokio::test]
async fn a_replacement_claims_no_further_entry() {
    let (client, receiver) = capturing_client();
    let store = RegistryDynamoStore::new(client, TABLE);
    let commit = replaced_commit();
    let receipt = receipt_for(&commit);
    let _ignored = store
        .commit_set(
            workspace(),
            SetCommit {
                commit: &commit,
                from_revision: Some(Revision::FIRST),
                entries_cap: 1_000,
                creates: false,
                receipt: &receipt,
            },
        )
        .await;

    let body = captured_body(receiver);
    let actions = body["TransactItems"]
        .as_array()
        .expect("a transaction carries actions");
    assert_eq!(actions.len(), 2, "pointer and receipt only");
}

/// A delete honours `If-Match` as a condition and reads nothing first (D-9).
#[tokio::test]
async fn a_delete_conditions_on_the_stored_tag_and_releases_the_entry() {
    let (client, receiver) = capturing_client();
    let store = RegistryDynamoStore::new(client, TABLE);
    let tag = pointer().row.etag;
    let _ignored = store
        .commit_delete(workspace(), RegistryKind::Tool, "search", Some(&tag))
        .await;

    let body = captured_body(receiver);
    let actions = body["TransactItems"]
        .as_array()
        .expect("a transaction carries actions");
    assert_eq!(actions.len(), 2, "the pointer delete and the entry release");
    assert_eq!(
        actions[0]["Delete"]["ConditionExpression"].as_str(),
        Some("attribute_exists(pk) AND etag = :ifMatch")
    );
    assert_eq!(
        actions[0]["Delete"]["ReturnValuesOnConditionCheckFailure"].as_str(),
        Some("ALL_OLD"),
        "the observed row is what tells an absent name from a stale tag"
    );
}

#[tokio::test]
async fn a_replacement_is_conditional_on_the_revision_the_caller_observed() {
    let (client, receiver) = capturing_client();
    let store = RegistryDynamoStore::new(client, TABLE);
    let commit = replaced_commit();
    let receipt = receipt_for(&commit);
    let _ignored = store
        .commit_set(
            workspace(),
            SetCommit {
                commit: &commit,
                from_revision: Some(Revision::FIRST),
                entries_cap: 1_000,
                creates: false,
                receipt: &receipt,
            },
        )
        .await;

    let body = captured_body(receiver);
    let put = &body["TransactItems"][0]["Put"];
    assert_eq!(
        put["ConditionExpression"].as_str(),
        Some("attribute_exists(pk) AND revision = :fromRevision")
    );
    assert_eq!(
        put["ExpressionAttributeValues"][":fromRevision"]["N"].as_str(),
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
    let projection = body["ProjectionExpression"]
        .as_str()
        .expect("a listing projects the collection columns");
    assert!(
        !projection.contains("valueDoc"),
        "a thousand-row page must not carry a thousand value documents"
    );
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
        Some(
            "SET #state = :completing, completionIntentHash = :hash, completionManifest = :manifest"
        )
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

/// Each point read takes exactly the consistency its own fence needs.
///
/// A registry pointer read is **eventually consistent** (D-12): every mutation is
/// fenced by a conditional write, so a stale pre-read cannot produce a wrong
/// write — it produces a lost condition, which is a `412`. Read-your-writes is
/// the only casualty and no correct client needs it, because a `PUT` already
/// answers with the complete resource and its `ETag`. Halving the read cost of
/// the highest-volume registry route is the trade the priority order asks for.
///
/// An upload read stays **strong**: its state machine reads a state and then
/// conditions on it, so that read is a fence rather than a projection.
#[tokio::test]
async fn each_point_read_takes_exactly_the_consistency_its_fence_needs() {
    let (client, receiver) = capturing_client();
    let store = RegistryDynamoStore::new(client, TABLE);
    let _ignored = store
        .load_pointer(workspace(), RegistryKind::Tool, "search")
        .await;
    assert_ne!(
        captured_body(receiver)["ConsistentRead"].as_bool(),
        Some(true),
        "a pointer read must not pay for a fence the conditional write provides"
    );

    let (client, receiver) = capturing_client();
    let store = RegistryDynamoStore::new(client, TABLE);
    let _ignored = store.load_upload(workspace(), upload().id).await;
    assert_eq!(
        captured_body(receiver)["ConsistentRead"].as_bool(),
        Some(true),
        "an upload state read is a fence and was answered from a replica"
    );
}
