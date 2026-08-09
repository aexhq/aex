//! Engine-backed cases for `regional-secret-custody`, against `DynamoDB` Local.

mod support;

use aex_secret_custody_dynamodb::expressions::{self, BoundName};
use aex_secret_custody_dynamodb::store::{CustodyStore, SecretCustodyStore};
use aex_secret_domain::custody::CustodyRevision;
use aex_secret_domain::revocation::RevocationEpoch;
use aex_secret_domain::secret::{SecretRevision, SourceGeneration};
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::paging::PageBudget;
use aex_session_dynamodb::plan::Participant;
use aex_test_harness::DynamoDbLocalContainer;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_dynamodb::types::{
    AttributeDefinition, BillingMode, KeySchemaElement, KeyType, ScalarAttributeType,
};

use support::{
    TABLE, authorization, custody_head, entry, generation, metadata, now, secret_name, session,
    workspace,
};

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

async fn engine() -> (DynamoDbLocalContainer, Client, CustodyStore) {
    let engine = DynamoDbLocalContainer::start()
        .await
        .expect("DynamoDB Local starts");
    let client = client(&engine);
    client
        .create_table()
        .table_name(TABLE)
        .set_attribute_definitions(Some(vec![
            AttributeDefinition::builder()
                .attribute_name("pk")
                .attribute_type(ScalarAttributeType::S)
                .build()
                .expect("a complete attribute"),
            AttributeDefinition::builder()
                .attribute_name("sk")
                .attribute_type(ScalarAttributeType::S)
                .build()
                .expect("a complete attribute"),
        ]))
        .set_key_schema(Some(vec![
            KeySchemaElement::builder()
                .attribute_name("pk")
                .key_type(KeyType::Hash)
                .build()
                .expect("a partition key"),
            KeySchemaElement::builder()
                .attribute_name("sk")
                .key_type(KeyType::Range)
                .build()
                .expect("a sort key"),
        ]))
        .billing_mode(BillingMode::PayPerRequest)
        .send()
        .await
        .expect("the table is created");
    let store = CustodyStore::new(client.clone(), TABLE);
    (engine, client, store)
}

async fn seed_secret(store: &CustodyStore) {
    let plan = expressions::set(TABLE, &generation(), &metadata(), None).expect("compiles");
    store.commit(&plan).await.expect("the set commits");
}

#[tokio::test]
async fn a_set_writes_metadata_and_a_generation_that_never_share_a_partition() {
    let (_engine, _client, store) = engine().await;
    seed_secret(&store).await;

    let listed = store
        .list_secrets(workspace(), PageBudget::new(25).expect("a page"))
        .await
        .expect("the list succeeds");
    assert_eq!(listed.len(), 1, "the list returns metadata only");
    assert_eq!(listed[0].name, secret_name());

    let sealed = store
        .load_generation(workspace(), &secret_name(), SourceGeneration::FIRST)
        .await
        .expect("the read succeeds")
        .expect("the generation exists");
    assert_eq!(sealed.ciphertext, generation().ciphertext);
}

#[tokio::test]
async fn an_identical_set_replay_inside_the_transport_window_is_successful() {
    let (_engine, _client, store) = engine().await;
    seed_secret(&store).await;

    store
        .commit(&expressions::set(TABLE, &generation(), &metadata(), None).expect("compiles"))
        .await
        .expect("an identical replay is idempotent");

    assert_eq!(
        store
            .load_secret(workspace(), &secret_name())
            .await
            .expect("the read succeeds"),
        Some(metadata()),
        "transport replay returns the first commit rather than applying a second mutation"
    );
}

#[tokio::test]
async fn a_distinct_set_at_the_revision_nobody_read_is_refused_atomically() {
    let (_engine, _client, store) = engine().await;
    seed_secret(&store).await;

    let mut replacement = metadata();
    replacement.revision = SecretRevision(8);
    replacement.generation = SourceGeneration(2);
    let mut next = generation();
    next.generation = SourceGeneration(2);
    let stale = store
        .commit(
            &expressions::set(TABLE, &next, &replacement, Some(SecretRevision(7)))
                .expect("compiles"),
        )
        .await
        .expect_err("a revision nobody observed");
    assert!(
        matches!(
            stale,
            StoreError::PreconditionFailed {
                participant: Participant::SECRET_METADATA,
                ..
            }
        ),
        "{stale}"
    );

    assert_eq!(
        store
            .load_secret(workspace(), &secret_name())
            .await
            .expect("the read succeeds"),
        Some(metadata()),
        "the stale write must not move the metadata revision"
    );
    assert!(
        store
            .load_generation(workspace(), &secret_name(), SourceGeneration(2))
            .await
            .expect("the generation read succeeds")
            .is_none(),
        "a cancelled transaction must not strand the stale writer's sealed generation"
    );
}

#[tokio::test]
async fn one_revoke_fences_every_generation_of_a_name_at_once() {
    let (_engine, _client, store) = engine().await;
    seed_secret(&store).await;

    store
        .commit_update(
            expressions::revoke(
                TABLE,
                workspace(),
                secret_name().as_str(),
                RevocationEpoch::INITIAL,
                now(),
            )
            .expect("builds"),
            Participant::SECRET_METADATA,
        )
        .await
        .expect("the revoke commits");

    let after = store
        .load_secret(workspace(), &secret_name())
        .await
        .expect("the read succeeds")
        .expect("the record exists");
    assert_eq!(after.revocation_epoch, RevocationEpoch(1));
    assert_eq!(
        after.revoked_through_revision, after.revision,
        "every generation at or below the current revision is fenced by one write"
    );

    // The use path now loses at the record, so nothing downstream may decrypt.
    let denied = store
        .commit(
            &expressions::authorize_managed_call(TABLE, &authorization(), SecretRevision::FIRST)
                .expect("compiles"),
        )
        .await
        .expect_err("the record is fenced");
    assert!(
        matches!(
            denied,
            StoreError::PreconditionFailed {
                participant: Participant::SECRET_METADATA,
                ..
            }
        ),
        "{denied}"
    );

    // A second revoke asking for a fresh fence must present the new epoch.
    let stale = store
        .commit_update(
            expressions::revoke(
                TABLE,
                workspace(),
                secret_name().as_str(),
                RevocationEpoch::INITIAL,
                now(),
            )
            .expect("builds"),
            Participant::SECRET_METADATA,
        )
        .await
        .expect_err("the epoch already moved");
    assert!(matches!(stale, StoreError::PreconditionFailed { .. }));
}

#[tokio::test]
async fn a_custody_admission_is_idle_only_and_binds_under_the_revision_it_read() {
    let (_engine, client, store) = engine().await;
    seed_secret(&store).await;

    let mut head = custody_head();
    head.revision = CustodyRevision::NONE;
    client
        .put_item()
        .table_name(TABLE)
        .set_item(Some(
            aex_secret_custody_dynamodb::codec::encode_custody_head(&head),
        ))
        .send()
        .await
        .expect("the custody head is written");

    let binding = aex_secret_custody_dynamodb::codec::encode_binding(
        session(),
        workspace(),
        CustodyRevision::FIRST,
        &entry(),
        [3; 32],
        now(),
    )
    .expect("encodes");

    let names = [BoundName {
        name: secret_name().as_str().to_owned(),
        bound_revision: SecretRevision::FIRST,
        source_generation: SourceGeneration::FIRST,
    }];

    let wrong_idle = expressions::admit_custody(
        TABLE,
        session(),
        workspace(),
        &names,
        vec![binding.clone()],
        CustodyRevision::NONE,
        CustodyRevision::FIRST,
        99,
        now(),
    )
    .expect("compiles");
    let error = store
        .commit(&wrong_idle)
        .await
        .expect_err("the session was not idle at that epoch");
    assert!(
        matches!(
            error,
            StoreError::PreconditionFailed {
                participant: Participant::CUSTODY_HEAD,
                ..
            }
        ),
        "{error}"
    );

    let admission = expressions::admit_custody(
        TABLE,
        session(),
        workspace(),
        &names,
        vec![binding],
        CustodyRevision::NONE,
        CustodyRevision::FIRST,
        head.idle_epoch,
        now(),
    )
    .expect("compiles");
    store
        .commit(&admission)
        .await
        .expect("the admission commits");

    let after = store
        .load_custody(workspace(), session())
        .await
        .expect("the read succeeds")
        .expect("the head exists");
    assert_eq!(after.revision, CustodyRevision::FIRST);
}
