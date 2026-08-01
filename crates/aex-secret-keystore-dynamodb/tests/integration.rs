//! Engine-backed cases for `regional-secret-keystore`, against `DynamoDB` Local.
//!
//! The records are seeded with a raw client, exactly as the provider's own
//! key-store operations would write them, because this crate has no write path
//! to seed them with. That is the point of the target: the reader has to decode
//! what somebody else wrote.

mod support;

use aex_secret_keystore_dynamodb::branch_key;
use aex_secret_keystore_dynamodb::store::{BranchKeyStoreReader, KeyStoreReader};
use aex_session_dynamodb::paging::PageBudget;
use aex_test_harness::DynamoDbLocalContainer;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_dynamodb::types::{
    AttributeDefinition, BillingMode, KeySchemaElement, KeyType, ScalarAttributeType,
};

use support::{TABLE, active_record, binding, branch_key_id};

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

async fn engine() -> (DynamoDbLocalContainer, Client, KeyStoreReader) {
    let engine = DynamoDbLocalContainer::start()
        .await
        .expect("DynamoDB Local starts");
    let client = client(&engine);
    client
        .create_table()
        .table_name(TABLE)
        .set_attribute_definitions(Some(vec![
            AttributeDefinition::builder()
                .attribute_name(branch_key::BRANCH_KEY_ID)
                .attribute_type(ScalarAttributeType::S)
                .build()
                .expect("a complete attribute"),
            AttributeDefinition::builder()
                .attribute_name(branch_key::TYPE)
                .attribute_type(ScalarAttributeType::S)
                .build()
                .expect("a complete attribute"),
        ]))
        .set_key_schema(Some(vec![
            KeySchemaElement::builder()
                .attribute_name(branch_key::BRANCH_KEY_ID)
                .key_type(KeyType::Hash)
                .build()
                .expect("a partition key"),
            KeySchemaElement::builder()
                .attribute_name(branch_key::TYPE)
                .key_type(KeyType::Range)
                .build()
                .expect("a sort key"),
        ]))
        .billing_mode(BillingMode::PayPerRequest)
        .send()
        .await
        .expect("the table is created in the provider's own shape");
    let reader = KeyStoreReader::new(client.clone(), binding());
    (engine, client, reader)
}

#[tokio::test]
async fn the_reader_decodes_a_record_written_the_way_the_provider_writes_it() {
    let (_engine, client, reader) = engine().await;
    client
        .put_item()
        .table_name(TABLE)
        .set_item(Some(active_record(1)))
        .send()
        .await
        .expect("the record is seeded");

    let active = reader
        .describe_active(&branch_key_id(1))
        .await
        .expect("the read succeeds")
        .expect("the record exists");
    assert_eq!(active.branch_key_id, branch_key_id(1));
    assert_eq!(active.hierarchy_version, 1);
    assert!(active.version.is_some());
}

#[tokio::test]
async fn a_workspace_with_no_branch_key_reads_as_absent_rather_than_as_an_error() {
    let (_engine, _client, reader) = engine().await;
    assert!(
        reader
            .describe_active(&branch_key_id(7))
            .await
            .expect("the read succeeds")
            .is_none()
    );
}

#[tokio::test]
async fn listing_returns_one_identity_per_workspace_and_ignores_versioned_records() {
    let (_engine, client, reader) = engine().await;
    for byte in 1..=3u8 {
        client
            .put_item()
            .table_name(TABLE)
            .set_item(Some(active_record(byte)))
            .send()
            .await
            .expect("the record is seeded");

        // A versioned record under the same branch key must not produce a
        // second identity in the listing.
        let mut versioned = active_record(byte);
        versioned.insert(
            branch_key::TYPE.to_owned(),
            aex_session_dynamodb::attr::s("branch:version:0192f0ac"),
        );
        client
            .put_item()
            .table_name(TABLE)
            .set_item(Some(versioned))
            .send()
            .await
            .expect("the versioned record is seeded");
    }

    let listed = reader
        .list_branch_key_ids(PageBudget::new(100).expect("a page"))
        .await
        .expect("the listing succeeds");
    assert_eq!(listed.len(), 3, "{listed:?}");
    assert_eq!(
        listed,
        vec![branch_key_id(1), branch_key_id(2), branch_key_id(3)]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
    );
}
