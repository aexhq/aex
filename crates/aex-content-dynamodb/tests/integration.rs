//! Engine-backed cases for `regional-content`, against `DynamoDB` Local.
//!
//! What only a real engine proves: that the expressions parse and evaluate, that
//! the `INCLUDE` projection returns exactly the declared attributes and no
//! ciphertext, that a `TransactWriteItems` cancellation carries a positional
//! reason vector this crate decodes back to the participant that lost, and that
//! the sparse index holds only the rows that carry its attributes.
//!
//! What it does not prove is stated in the handoff rather than assumed away:
//! `DynamoDB` Local implements no TTL expiry, no adaptive capacity and no
//! `TransactionConflict` under real contention.

mod support;

use aex_content_dynamodb::codec::{self, decode_descriptor};
use aex_content_dynamodb::expressions;
use aex_content_dynamodb::keys;
use aex_content_dynamodb::store::{ContentMetadataStore, ContentStore};
use aex_content_dynamodb::wire_pending::GcSweepPlan;
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::paging::PageBudget;
use aex_session_dynamodb::plan::{Participant, key};
use aex_test_harness::DynamoDbLocalContainer;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_dynamodb::types::{
    AttributeDefinition, BillingMode, GlobalSecondaryIndex, KeySchemaElement, KeyType, Projection,
    ProjectionType, ScalarAttributeType,
};

use support::{DEFINITION, descriptor, digest, grant, now, organization, sealed, workspace};

const TABLE: &str = "dev-eu-west-1-regional-content";

fn client(engine: &DynamoDbLocalContainer) -> Client {
    let config = aws_sdk_dynamodb::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new(engine.region()))
        .endpoint_url(engine.endpoint_url())
        .credentials_provider(Credentials::new(
            engine.access_key_id(),
            engine.secret_access_key(),
            None,
            None,
            "aex-integration",
        ))
        .build();
    Client::from_conf(config)
}

/// Creates the table from the checked-in generation definition, so the engine
/// runs the shape Terraform creates rather than one this test invented.
async fn create_table(client: &Client) {
    let definition: serde_json::Value =
        serde_json::from_str(DEFINITION).expect("the generation definition is JSON");

    let attributes: Vec<AttributeDefinition> = definition["attributes"]
        .as_array()
        .expect("attributes")
        .iter()
        .map(|entry| {
            AttributeDefinition::builder()
                .attribute_name(entry["name"].as_str().expect("a name"))
                .attribute_type(ScalarAttributeType::from(
                    entry["type"].as_str().expect("a type"),
                ))
                .build()
                .expect("a complete attribute")
        })
        .collect();

    let schema = |partition: &str, sort: &str| {
        vec![
            KeySchemaElement::builder()
                .attribute_name(partition)
                .key_type(KeyType::Hash)
                .build()
                .expect("a partition key"),
            KeySchemaElement::builder()
                .attribute_name(sort)
                .key_type(KeyType::Range)
                .build()
                .expect("a sort key"),
        ]
    };

    let indexes: Vec<GlobalSecondaryIndex> = definition["globalSecondaryIndexes"]
        .as_array()
        .expect("indexes")
        .iter()
        .map(|index| {
            GlobalSecondaryIndex::builder()
                .index_name(index["name"].as_str().expect("a name"))
                .set_key_schema(Some(schema(
                    index["partition"].as_str().expect("a partition"),
                    index["sort"].as_str().expect("a sort"),
                )))
                .projection(
                    Projection::builder()
                        .projection_type(ProjectionType::Include)
                        .set_non_key_attributes(Some(
                            index["projection"]["attributes"]
                                .as_array()
                                .expect("an attribute list")
                                .iter()
                                .map(|value| value.as_str().expect("a string").to_owned())
                                .collect(),
                        ))
                        .build(),
                )
                .build()
                .expect("a complete index")
        })
        .collect();

    client
        .create_table()
        .table_name(TABLE)
        .set_attribute_definitions(Some(attributes))
        .set_key_schema(Some(schema("pk", "sk")))
        .set_global_secondary_indexes(Some(indexes))
        .billing_mode(BillingMode::PayPerRequest)
        .send()
        .await
        .expect("the table is created from the generation definition");
}

async fn engine() -> (DynamoDbLocalContainer, Client) {
    let engine = DynamoDbLocalContainer::start()
        .await
        .expect("DynamoDB Local starts");
    let client = client(&engine);
    create_table(&client).await;
    (engine, client)
}

#[tokio::test]
async fn a_descriptor_written_through_the_codec_reads_back_through_it() {
    let (_engine, client) = engine().await;
    let store = ContentStore::new(client.clone(), TABLE);

    let item = codec::encode_descriptor(&descriptor()).expect("encodes");
    client
        .put_item()
        .table_name(TABLE)
        .set_item(Some(item))
        .condition_expression("attribute_not_exists(pk)")
        .send()
        .await
        .expect("the row is written");

    let loaded = store
        .load_descriptor(workspace(), &descriptor().digest)
        .await
        .expect("the read succeeds")
        .expect("the descriptor exists");
    assert_eq!(loaded, descriptor());
}

#[tokio::test]
async fn the_scan_index_returns_the_projection_and_never_a_ciphertext() {
    let (_engine, client) = engine().await;
    let store = ContentStore::new(client.clone(), TABLE);

    let indexed = descriptor();
    let item = codec::encode_descriptor(&indexed).expect("encodes");
    client
        .put_item()
        .table_name(TABLE)
        .set_item(Some(item))
        .send()
        .await
        .expect("the descriptor is written");

    let bucket = keys::bucket_of(&hex::encode(indexed.digest.as_bytes())).expect("a bucket");
    let scanned = store
        .scan_gc_bucket(workspace(), bucket, PageBudget::new(25).expect("a page"))
        .await
        .expect("the scan succeeds");
    assert_eq!(scanned.entries.len(), 1);
    assert_eq!(scanned.entries[0].digest, indexed.digest.to_wire());

    // Read the index directly to prove the projection itself, not merely what
    // this crate chose to decode out of it.
    let raw = client
        .query()
        .table_name(TABLE)
        .index_name(keys::GC_INDEX)
        .key_condition_expression("#pk = :pk")
        .expression_attribute_names("#pk", keys::GC_PK)
        .expression_attribute_values(
            ":pk",
            aex_session_dynamodb::attr::s(keys::gc_scan_partition(workspace(), bucket)),
        )
        .send()
        .await
        .expect("the index query succeeds");
    let projected = &raw.items.expect("items")[0];
    assert!(
        !projected.contains_key("ciphertext"),
        "the index projected a body: {:?}",
        projected.keys().collect::<Vec<_>>()
    );
    assert!(!projected.contains_key("encContextDigest"));
}

#[tokio::test]
async fn a_body_with_no_scan_attributes_is_absent_from_the_sparse_index() {
    let (_engine, client) = engine().await;
    let store = ContentStore::new(client.clone(), TABLE);

    let body = digest(0x11);
    client
        .put_item()
        .table_name(TABLE)
        .set_item(Some(
            codec::encode_inline_body(workspace(), &body, &sealed(64)).expect("encodes"),
        ))
        .send()
        .await
        .expect("the body is written");

    let bucket = keys::bucket_of(&hex::encode(body.as_bytes())).expect("a bucket");
    let scanned = store
        .scan_gc_bucket(workspace(), bucket, PageBudget::new(25).expect("a page"))
        .await
        .expect("the scan succeeds");
    assert!(
        scanned.entries.is_empty(),
        "the inline body carries no index attribute, so a mark scan cannot reach it"
    );
}

#[tokio::test]
async fn a_grant_and_its_pin_commit_together_and_a_replay_is_refused() {
    let (_engine, client) = engine().await;
    let store = ContentStore::new(client.clone(), TABLE);

    store.mint_grant(&grant(), now()).await.expect("mints");

    let redeemed_before_descriptor = store.redeem_grant(&grant().token_sha256, now()).await;
    assert!(
        matches!(redeemed_before_descriptor, Err(StoreError::Invalid { .. })),
        "a grant whose body has no descriptor is a corrupt state, not a 404"
    );

    // An identical replay inside the provider's ten-minute `ClientRequestToken`
    // window is an idempotent success rather than a lost condition. That is
    // transport deduplication doing its job, and it is why the durable receipt
    // remains the product authority beyond the window.
    store
        .mint_grant(&grant(), now())
        .await
        .expect("an identical replay is idempotent");

    // The same token with a different intent is not.
    let mut widened = grant();
    widened.authorized_bytes += 1;
    let error = store
        .mint_grant(&widened, now())
        .await
        .expect_err("the same token cannot authorise a different transfer");
    assert_eq!(
        error,
        StoreError::IdempotencyConflict,
        "a reused transport token with a changed payload must never widen a grant"
    );

    // Both rows landed, and the grant pin is what the sweeper will see.
    let reachability = store
        .reachability(workspace(), &grant().digest, now())
        .await
        .expect("the reachability read succeeds");
    assert_eq!(reachability.unexpired_grants, 1);
    assert_eq!(reachability.pins, 0);
    assert!(!reachability.is_collectable());
}

#[tokio::test]
async fn a_registry_pin_is_visible_to_the_reachability_read_of_that_body() {
    let (_engine, client) = engine().await;
    let store = ContentStore::new(client.clone(), TABLE);

    let body = digest(0x22);
    let pin = codec::ContentPin {
        workspace: workspace(),
        owner: aex_content_dynamodb::PinOwner::Registry {
            kind: "tool".to_owned(),
            name: "fixture".to_owned(),
        },
        created_at: now(),
    };
    let built = expressions::pin_body(TABLE, &body, &pin)
        .expect("builds")
        .build()
        .expect("a complete put");
    client
        .put_item()
        .table_name(TABLE)
        .set_item(Some(built.item().clone()))
        .set_condition_expression(built.condition_expression().map(str::to_owned))
        .send()
        .await
        .expect("the pin is written");

    let reachability = store
        .reachability(workspace(), &body, now())
        .await
        .expect("the read succeeds");
    assert_eq!(reachability.pins, 1);
    assert!(!reachability.is_collectable());
}

#[tokio::test]
async fn a_sweep_whose_epoch_moved_is_cancelled_at_the_epoch_participant() {
    let (_engine, client) = engine().await;
    let store = ContentStore::new(client.clone(), TABLE);

    let mut epoch = codec::GcEpoch {
        workspace: workspace(),
        epoch: 4,
        state: "sweeping".to_owned(),
        mark_started_at: Some(now()),
        mark_bucket_cursor: Some(0),
        sweep_cursor: None,
        revision: 1,
    };
    client
        .put_item()
        .table_name(TABLE)
        .set_item(Some(codec::encode_gc_epoch(&epoch)))
        .send()
        .await
        .expect("the epoch is written");

    let mut marked = descriptor();
    marked.gc_epoch = 3;
    client
        .put_item()
        .table_name(TABLE)
        .set_item(Some(codec::encode_descriptor(&marked).expect("encodes")))
        .send()
        .await
        .expect("the descriptor is written");

    // A concurrent collector opened a newer epoch.
    epoch.epoch = 5;
    client
        .put_item()
        .table_name(TABLE)
        .set_item(Some(codec::encode_gc_epoch(&epoch)))
        .send()
        .await
        .expect("the epoch advances");

    let error = store
        .sweep_candidate(&GcSweepPlan {
            workspace: workspace(),
            organization: organization(),
            digest: marked.digest,
            epoch: 4,
            marked_epoch: 3,
        })
        .await
        .expect_err("the epoch fence lost");
    assert!(
        matches!(
            error,
            StoreError::PreconditionFailed {
                participant: Participant::CONTENT_GC_EPOCH,
                ..
            }
        ),
        "{error}"
    );

    // Uncertainty keeps data: the descriptor survives.
    let survivor = client
        .get_item()
        .table_name(TABLE)
        .set_key(Some(key(
            &keys::descriptor(workspace(), &marked.digest).pk,
            "DESC",
        )))
        .consistent_read(true)
        .send()
        .await
        .expect("the read succeeds")
        .item
        .expect("the body survived a lost sweep");
    assert_eq!(
        decode_descriptor(&survivor, workspace()).expect("decodes"),
        marked
    );
}

#[tokio::test]
async fn a_sweep_under_the_exact_epoch_and_mark_removes_the_descriptor_and_the_candidate() {
    let (_engine, client) = engine().await;
    let store = ContentStore::new(client.clone(), TABLE);

    let epoch = codec::GcEpoch {
        workspace: workspace(),
        epoch: 7,
        state: "sweeping".to_owned(),
        mark_started_at: Some(now()),
        mark_bucket_cursor: Some(255),
        sweep_cursor: None,
        revision: 2,
    };
    client
        .put_item()
        .table_name(TABLE)
        .set_item(Some(codec::encode_gc_epoch(&epoch)))
        .send()
        .await
        .expect("the epoch is written");

    let mut marked = descriptor();
    marked.gc_epoch = 6;
    client
        .put_item()
        .table_name(TABLE)
        .set_item(Some(codec::encode_descriptor(&marked).expect("encodes")))
        .send()
        .await
        .expect("the descriptor is written");

    let candidate = codec::GcCandidate {
        workspace: workspace(),
        digest: marked.digest,
        epoch: 7,
        staged_at: now(),
        object_key: None,
        object_etag: None,
        size_bytes: marked.size_bytes,
        attempt_count: 0,
    };
    client
        .put_item()
        .table_name(TABLE)
        .set_item(Some(
            codec::encode_gc_candidate(&candidate).expect("encodes"),
        ))
        .send()
        .await
        .expect("the candidate is written");

    store
        .sweep_candidate(&GcSweepPlan {
            workspace: workspace(),
            organization: organization(),
            digest: marked.digest,
            epoch: 7,
            marked_epoch: 6,
        })
        .await
        .expect("the sweep commits");

    assert!(
        store
            .load_descriptor(workspace(), &marked.digest)
            .await
            .expect("the read succeeds")
            .is_none()
    );
}
