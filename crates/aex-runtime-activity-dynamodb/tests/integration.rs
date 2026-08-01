//! Engine-backed cases for `runtime-activity`, against `DynamoDB` Local.

mod support;

use aex_hands_protocol::rpc::Fence;
use aex_runtime_activity_dynamodb::keys;
use aex_runtime_activity_dynamodb::store::{RuntimeActivityDynamoStore, RuntimeActivityStore};
use aex_runtime_control::generation::{GenerationState, Revision};
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::paging::PageBudget;
use aex_session_dynamodb::plan::Participant;
use aex_test_harness::DynamoDbLocalContainer;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_dynamodb::types::{
    AttributeDefinition, BillingMode, GlobalSecondaryIndex, KeySchemaElement, KeyType, Projection,
    ProjectionType, ScalarAttributeType,
};

use support::{DEFINITION, TABLE, generation, head, later, now, receipt, session, workspace};

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

async fn engine() -> (DynamoDbLocalContainer, RuntimeActivityDynamoStore) {
    let engine = DynamoDbLocalContainer::start()
        .await
        .expect("DynamoDB Local starts");
    let client = client(&engine);
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
    let index = &definition["globalSecondaryIndexes"][0];
    client
        .create_table()
        .table_name(TABLE)
        .set_attribute_definitions(Some(attributes))
        .set_key_schema(Some(schema("pk", "sk")))
        .global_secondary_indexes(
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
                .expect("a complete index"),
        )
        .billing_mode(BillingMode::PayPerRequest)
        .send()
        .await
        .expect("the table is created from the generation definition");
    let store = RuntimeActivityDynamoStore::new(client, TABLE);
    (engine, store)
}

#[tokio::test]
async fn a_generation_moves_only_under_the_fence_and_revision_it_was_read_at() {
    let (_engine, store) = engine().await;
    let launching = head(GenerationState::Launching);
    store
        .create_generation(&launching)
        .await
        .expect("the head is written");

    let mut running = launching.clone();
    running.state = GenerationState::Running;
    running.fence = Fence(launching.fence.0 + 1);
    running.revision = launching.revision.next();

    let stale = store
        .transition(
            &running,
            GenerationState::Launching,
            Fence(99),
            launching.revision,
            now(),
        )
        .await
        .expect_err("a fence nobody observed");
    assert!(
        matches!(
            stale,
            StoreError::PreconditionFailed {
                participant: Participant::RUNTIME_GENERATION,
                ..
            }
        ),
        "{stale}"
    );

    store
        .transition(
            &running,
            GenerationState::Launching,
            launching.fence,
            launching.revision,
            now(),
        )
        .await
        .expect("the transition commits");

    let loaded = store
        .load_generation(workspace(), session(), generation(4))
        .await
        .expect("the read succeeds")
        .expect("the head exists");
    assert_eq!(loaded.state, GenerationState::Running);
    assert_eq!(loaded.fence, running.fence);
}

#[tokio::test]
async fn a_terminal_generation_disappears_from_the_reapers_scan() {
    let (_engine, store) = engine().await;
    let running = head(GenerationState::Running);
    store
        .create_generation(&running)
        .await
        .expect("the head is written");

    let shard = keys::shard_of(generation(4));
    let due = store
        .scan_due(shard, later(200_000), PageBudget::new(25).expect("a page"))
        .await
        .expect("the scan succeeds");
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].generation, generation(4));
    assert_eq!(due[0].state, GenerationState::Running);

    let mut terminated = running.clone();
    terminated.state = GenerationState::Terminated;
    terminated.revision = running.revision.next();
    store
        .transition(
            &terminated,
            GenerationState::Running,
            running.fence,
            running.revision,
            now(),
        )
        .await
        .expect("the terminal transition commits");

    let after = store
        .scan_due(shard, later(200_000), PageBudget::new(25).expect("a page"))
        .await
        .expect("the scan succeeds");
    assert!(
        after.is_empty(),
        "the reaper must never see a generation that is already gone: {after:?}"
    );
}

#[tokio::test]
async fn a_receipt_settles_once_and_a_second_settlement_loses() {
    let (_engine, store) = engine().await;
    store
        .record_intent(&support::intent())
        .await
        .expect("the intent is recorded");
    store
        .settle_intent(&receipt())
        .await
        .expect("the receipt is written");

    let mut second = receipt();
    second.outcome = "failed".to_owned();
    let error = store
        .settle_intent(&second)
        .await
        .expect_err("a second outcome");
    assert!(
        matches!(
            error,
            StoreError::PreconditionFailed {
                participant: Participant::RUNTIME_RECEIPT,
                ..
            }
        ),
        "lifecycle evidence a usage fact references must never be overwritten: {error}"
    );
}

#[tokio::test]
async fn the_current_pointer_moves_only_under_the_revision_the_caller_read() {
    let (_engine, store) = engine().await;
    store
        .point_current(session(), generation(4), Fence(3), None, now())
        .await
        .expect("the pointer is written");

    let pointer = store
        .load_current(session())
        .await
        .expect("the read succeeds")
        .expect("the pointer exists");
    assert_eq!(pointer.generation, generation(4));
    assert_eq!(pointer.fence, Fence(3));

    let stale = store
        .point_current(
            session(),
            generation(5),
            Fence(4),
            Some(Revision::new(99)),
            now(),
        )
        .await
        .expect_err("a revision nobody read");
    assert!(matches!(stale, StoreError::PreconditionFailed { .. }));

    store
        .point_current(
            session(),
            generation(5),
            Fence(4),
            Some(pointer.revision),
            now(),
        )
        .await
        .expect("the pointer moves");
    assert_eq!(
        store
            .load_current(session())
            .await
            .expect("the read succeeds")
            .expect("the pointer exists")
            .generation,
        generation(5)
    );
}
