//! Request and generation-definition conformance for `regional-content`.
//!
//! Expressions are asserted on the serialized body captured from a real client,
//! because that is what `DynamoDB` evaluates; vocabularies are asserted against
//! the checked-in generation definition, because that is what Terraform creates.

mod support;

use aex_content_dynamodb::codec::{self, GrantExpiryCursor, GrantExpiryPosition};
use aex_content_dynamodb::keys;
use aex_content_dynamodb::store::{ContentMetadataStore, ContentStore};
use aex_content_dynamodb::wire_pending::GcSweepPlan;
use aex_session_dynamodb::paging::PageBudget;

use support::{
    Answer, DEFINITION, TABLE, captured_body, capturing_client, digest, gc_epoch, grant, later,
    now, organization, scripted_client, workspace,
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
fn the_grant_expiry_index_is_sharded_and_projects_only_cleanup_evidence() {
    let index = &definition()["globalSecondaryIndexes"][1];
    assert_eq!(index["name"].as_str(), Some(keys::EXPIRY_INDEX));
    assert_eq!(index["partition"].as_str(), Some(keys::EXPIRY_PK));
    assert_eq!(index["sort"].as_str(), Some(keys::EXPIRY_SK));
    assert_eq!(index["projection"]["type"].as_str(), Some("INCLUDE"));
    assert_eq!(
        strings(&index["projection"]["attributes"]),
        keys::EXPIRY_PROJECTION
    );
    assert_ne!(
        keys::expiry_partition(0),
        keys::expiry_partition(1),
        "expiry must never use one hot partition"
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
    let grant = &actions[0]["Put"]["Item"];
    assert!(grant[keys::EXPIRY_PK]["S"].as_str().is_some());
    assert!(grant[keys::EXPIRY_SK]["S"].as_str().is_some());
    assert!(
        pin[keys::EXPIRY_PK].is_null(),
        "only the grant lookup is due-indexed; its transaction removes the known pin"
    );
}

#[tokio::test]
async fn an_expiry_page_is_a_bounded_due_index_query_without_a_scan_or_filter() {
    let (client, receiver) = capturing_client();
    let store = ContentStore::new(client, TABLE);
    let _ignored = store
        .scan_expired_grants(7, now(), PageBudget::new(17).expect("a page"), None)
        .await;

    let body = captured_body(receiver);
    assert_eq!(body["IndexName"].as_str(), Some(keys::EXPIRY_INDEX));
    assert_eq!(body["Limit"].as_i64(), Some(17));
    assert_eq!(
        body["ExpressionAttributeValues"][":pk"]["S"].as_str(),
        Some("EXPIRY#0007")
    );
    let upper = body["ExpressionAttributeValues"][":now"]["S"]
        .as_str()
        .expect("a due upper bound");
    assert!(upper.starts_with(&now().to_wire()));
    assert!(
        upper.ends_with("#\u{fffd}"),
        "the upper bound must include every token due in the exact millisecond"
    );
    assert!(body["FilterExpression"].is_null());
    assert!(body["ExclusiveStartKey"].is_null());
}

#[tokio::test]
async fn an_expiry_page_resumes_after_the_exact_complete_provider_key() {
    let grant = grant();
    let shard = keys::expiry_shard(&grant.token_sha256);
    let grant_key = keys::grant(&grant.token_sha256).expect("a grant key");
    let position = GrantExpiryPosition {
        expiry_partition: keys::expiry_partition(shard),
        expiry_sort: keys::expiry_sort(grant.expires_at, &grant.token_sha256)
            .expect("an expiry key"),
        grant_pk: grant_key.pk,
        grant_sk: grant_key.sk,
    };
    let (client, receiver) = capturing_client();
    let store = ContentStore::new(client, TABLE);
    let _ignored = store
        .scan_expired_grants(
            shard,
            later(600_000),
            PageBudget::new(17).expect("a page"),
            Some(&position),
        )
        .await;

    let body = captured_body(receiver);
    let start = &body["ExclusiveStartKey"];
    assert_eq!(start[keys::EXPIRY_PK]["S"], position.expiry_partition);
    assert_eq!(start[keys::EXPIRY_SK]["S"], position.expiry_sort);
    assert_eq!(start["pk"]["S"], position.grant_pk);
    assert_eq!(start["sk"]["S"], position.grant_sk);
    assert_eq!(start.as_object().expect("the full provider key").len(), 4);
}

#[tokio::test]
async fn an_expiry_cursor_advance_is_revision_fenced_and_never_enters_the_expiry_index() {
    let grant = grant();
    let shard = keys::expiry_shard(&grant.token_sha256);
    let grant_key = keys::grant(&grant.token_sha256).expect("a grant key");
    let position = GrantExpiryPosition {
        expiry_partition: keys::expiry_partition(shard),
        expiry_sort: keys::expiry_sort(grant.expires_at, &grant.token_sha256)
            .expect("an expiry key"),
        grant_pk: grant_key.pk,
        grant_sk: grant_key.sk,
    };
    let current = GrantExpiryCursor {
        shard,
        position: None,
        revision: 4,
        updated_at: now(),
    };
    let (client, receiver) = capturing_client();
    let store = ContentStore::new(client, TABLE);
    let _ignored = store
        .advance_grant_expiry_cursor(Some(&current), shard, Some(&position), later(1))
        .await;

    let body = captured_body(receiver);
    assert_eq!(
        body["ConditionExpression"].as_str(),
        Some("itemType = :cursor AND shard = :shard AND revision = :previous")
    );
    assert_eq!(body["ExpressionAttributeValues"][":previous"]["N"], "4");
    assert_eq!(body["ReturnValuesOnConditionCheckFailure"], "ALL_OLD");
    let item = &body["Item"];
    assert!(item[keys::EXPIRY_PK].is_null());
    assert!(item[keys::EXPIRY_SK].is_null());
    assert_eq!(item["revision"]["N"], "5");
    assert_eq!(
        item["position"]["M"].as_object().expect("full LEK").len(),
        4
    );
}

#[tokio::test]
async fn expiry_removes_the_grant_and_pin_atomically_only_after_expiry() {
    let (client, receiver) = capturing_client();
    let store = ContentStore::new(client, TABLE);
    let expiry = aex_content_dynamodb::store::GrantExpiry::from(&grant());
    let _ignored = store.expire_grant(&expiry, later(300_000)).await;

    let body = captured_body(receiver);
    let actions = body["TransactItems"].as_array().expect("two deletes");
    assert_eq!(actions.len(), 2);
    for action in actions {
        let delete = &action["Delete"];
        let condition = delete["ConditionExpression"].as_str().expect("conditional");
        assert!(
            condition.contains("attribute_not_exists(pk)"),
            "{condition}"
        );
        assert!(condition.contains("expiresAt <= :now"), "{condition}");
        assert_eq!(
            delete["ReturnValuesOnConditionCheckFailure"].as_str(),
            Some("ALL_OLD")
        );
    }
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
async fn every_fence_point_read_is_strongly_consistent() {
    for read in ["grant", "epoch", "expiry_cursor"] {
        let (client, receiver) = capturing_client();
        let store = ContentStore::new(client, TABLE);
        match read {
            "grant" => {
                let _ignored = store.redeem_grant(&"b".repeat(64), now()).await;
            }
            "epoch" => {
                let _ignored = store.load_gc_epoch(workspace()).await;
            }
            _ => {
                let _ignored = store.load_grant_expiry_cursor(7).await;
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

#[tokio::test]
async fn immutable_content_addressed_reads_accept_replica_lag() {
    // A descriptor, an inline body or a tree page is content addressed: a
    // present row already holds the only bytes its digest can name, so the
    // only fact replica lag can change is presence moments after a write.
    for read in ["descriptor", "body", "tree_page"] {
        let (client, receiver) = capturing_client();
        let store = ContentStore::new(client, TABLE);
        match read {
            "descriptor" => {
                let _ignored = store.load_descriptor(workspace(), &digest(1)).await;
            }
            "body" => {
                let _ignored = store.read_inline_body(workspace(), &digest(1)).await;
            }
            _ => {
                let _ignored = store
                    .read_tree_page(
                        workspace(),
                        aex_content_dynamodb::Blake3Digest::from_bytes([7_u8; 32]),
                    )
                    .await;
            }
        }
        let body = captured_body(receiver);
        assert_eq!(
            body["ConsistentRead"].as_bool(),
            Some(false),
            "`{read}` is immutable and must take the cheap read path"
        );
    }
}

#[tokio::test]
async fn a_tree_page_wave_treats_a_lost_condition_as_replay_and_a_hard_failure_as_refusal() {
    let page = |byte: u8| codec::TreePage {
        workspace: workspace(),
        page: aex_content_dynamodb::Blake3Digest::of(&[byte]),
        level: 0,
        entry_count: 1,
        body: vec![7u8; 16],
        created_at: now(),
    };
    let conditional = serde_json::json!({
        "__type": "com.amazonaws.dynamodb.v20120810#ConditionalCheckFailedException",
        "message": "the page already exists"
    })
    .to_string();
    // The three puts are driven concurrently; whichever of them the scripted
    // transport answers with the lost condition, the wave is an idempotent
    // success, because a content-addressed page that already exists holds the
    // identical bytes.
    let (client, replay) = scripted_client(vec![
        Answer {
            status: 200,
            body: "{}".to_owned(),
        },
        Answer {
            status: 400,
            body: conditional,
        },
        Answer {
            status: 200,
            body: "{}".to_owned(),
        },
    ]);
    let store = ContentStore::new(client, TABLE);
    store
        .put_tree_pages(&[page(1), page(2), page(3)])
        .await
        .expect("a lost condition is a replayed page, never a failure");
    assert_eq!(replay.actual_requests().count(), 3);

    let denied = serde_json::json!({
        "__type": "com.amazonaws.dynamodb.v20120810#AccessDeniedException",
        "message": "no"
    })
    .to_string();
    let (client, _replay) = scripted_client(vec![
        Answer {
            status: 200,
            body: "{}".to_owned(),
        },
        Answer {
            status: 400,
            body: denied,
        },
        Answer {
            status: 200,
            body: "{}".to_owned(),
        },
    ]);
    let store = ContentStore::new(client, TABLE);
    store
        .put_tree_pages(&[page(1), page(2), page(3)])
        .await
        .expect_err("a hard provider failure refuses the whole wave");
}

#[test]
fn the_epoch_state_machine_only_ever_moves_through_the_declared_states() {
    let epoch = gc_epoch();
    assert!(keys::GC_STATES.contains(&epoch.state.as_str()));
    assert_eq!(keys::GC_STATES, ["idle", "marking", "sweeping"]);
}
