//! Engine-backed cases for `regional-registry`, against `DynamoDB` Local.

mod support;

use aex_content_domain::identity::{RegistryKind, Revision};
use aex_registry_dynamodb::store::{RegistryDynamoStore, RegistryStore, SetCommit, SetCommitted};
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::paging::PageBudget;
use aex_session_dynamodb::plan::Participant;
use aex_test_harness::DynamoDbLocalContainer;
use aex_workspace_domain::registry::{RegistryCommit, RegistryPointer, SetOutcome, etag_of};
use aex_workspace_domain::upload::UploadState;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_dynamodb::types::{
    AttributeDefinition, BillingMode, KeySchemaElement, KeyType, ScalarAttributeType,
};

use support::{
    TABLE, name, next_pointer, pointer, receipt_for_attempt, upload, upload_id, workspace,
};

async fn commit_pointer(
    store: &RegistryDynamoStore,
    pointer: RegistryPointer,
    from_revision: Option<Revision>,
    creates: bool,
    identity: u8,
) -> Result<SetCommitted, StoreError> {
    let workspace = pointer.row.workspace;
    let outcome = if creates {
        SetOutcome::Created
    } else {
        SetOutcome::Replaced
    };
    let commit = RegistryCommit {
        pointer,
        consumed_upload: None,
        wrote: true,
    };
    let receipt = receipt_for_attempt(&commit, outcome, identity);
    store
        .commit_set(
            workspace,
            SetCommit {
                commit: &commit,
                from_revision,
                entries_cap: 25,
                creates,
                receipt: &receipt,
            },
        )
        .await
}

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

async fn engine() -> (DynamoDbLocalContainer, RegistryDynamoStore) {
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
    let store = RegistryDynamoStore::new(client, TABLE);
    (engine, store)
}

#[tokio::test]
async fn a_create_without_an_observed_revision_cannot_overwrite_a_name() {
    let (_engine, store) = engine().await;
    commit_pointer(&store, pointer(), None, true, 1)
        .await
        .expect("creates");

    let error = commit_pointer(&store, pointer(), None, true, 2)
        .await
        .expect_err("the name is taken");
    assert!(
        matches!(
            error,
            StoreError::PreconditionFailed {
                participant: Participant::REGISTRY_POINTER,
                ..
            }
        ),
        "{error}"
    );

    let loaded = store
        .load_pointer(workspace(), RegistryKind::Tool, "search")
        .await
        .expect("the read succeeds")
        .expect("the pointer exists");
    assert_eq!(loaded, pointer());
    assert_eq!(
        loaded.row.etag,
        etag_of(loaded.row.kind, loaded.row.revision, &loaded.row.sha256),
        "the tag a client receives is recomputed from the row it describes"
    );
}

#[tokio::test]
async fn a_replacement_needs_the_revision_the_caller_observed() {
    let (_engine, store) = engine().await;
    commit_pointer(&store, pointer(), None, true, 3)
        .await
        .expect("creates");

    let stale = commit_pointer(&store, next_pointer(), Some(Revision(7)), false, 4)
        .await
        .expect_err("a revision nobody observed");
    assert!(
        matches!(stale, StoreError::PreconditionFailed { .. }),
        "{stale}"
    );

    commit_pointer(&store, next_pointer(), Some(Revision::FIRST), false, 5)
        .await
        .expect("replaces under the observed revision");

    let loaded = store
        .load_pointer(workspace(), RegistryKind::Tool, "search")
        .await
        .expect("the read succeeds")
        .expect("the pointer exists");
    assert_eq!(loaded.row.revision, Revision::FIRST.next());
    assert_ne!(loaded.row.etag, pointer().row.etag);
}

#[tokio::test]
async fn a_listing_returns_one_kind_in_name_order_and_never_another_kind() {
    let (_engine, store) = engine().await;
    for (identity, text) in [(10, "zeta"), (11, "alpha"), (12, "middle")] {
        let mut entry = pointer();
        entry.row.name = name(text);
        commit_pointer(&store, entry, None, true, identity)
            .await
            .expect("creates");
    }
    let mut other = pointer();
    other.row.kind = RegistryKind::Skill;
    other.row.etag = etag_of(RegistryKind::Skill, other.row.revision, &other.row.sha256);
    commit_pointer(&store, other, None, true, 13)
        .await
        .expect("creates");

    let page = store
        .list_pointers(
            workspace(),
            RegistryKind::Tool,
            PageBudget::new(25).expect("a page"),
            None,
        )
        .await
        .expect("the listing succeeds");
    let names: Vec<&str> = page.rows.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(names, ["alpha", "middle", "zeta"]);
    assert!(page.next.is_none());
}

#[tokio::test]
async fn a_listing_pages_through_a_signed_position_without_repeating_a_name() {
    let (_engine, store) = engine().await;
    for (identity, text) in [(20, "a"), (21, "b"), (22, "c"), (23, "d")] {
        let mut entry = pointer();
        entry.row.name = name(text);
        commit_pointer(&store, entry, None, true, identity)
            .await
            .expect("creates");
    }

    let first = store
        .list_pointers(
            workspace(),
            RegistryKind::Tool,
            PageBudget::new(2).expect("a page"),
            None,
        )
        .await
        .expect("the first page");
    assert_eq!(first.rows.len(), 2);
    let position = first.next.expect("a continuation");

    let second = store
        .list_pointers(
            workspace(),
            RegistryKind::Tool,
            PageBudget::new(2).expect("a page"),
            Some(&position),
        )
        .await
        .expect("the second page");
    let all: Vec<&str> = first
        .rows
        .iter()
        .chain(second.rows.iter())
        .map(|row| row.name.as_str())
        .collect();
    assert_eq!(all, ["a", "b", "c", "d"]);
}

#[tokio::test]
async fn an_upload_state_machine_admits_exactly_one_path() {
    let (_engine, store) = engine().await;
    store.create_upload(&upload()).await.expect("stages");

    let duplicate = store
        .create_upload(&upload())
        .await
        .expect_err("the identity is taken");
    assert!(matches!(duplicate, StoreError::PreconditionFailed { .. }));

    let wrong_source = store
        .transition_upload(upload_id(), UploadState::Ready, UploadState::Consumed)
        .await
        .expect_err("the upload is not ready");
    assert!(matches!(
        wrong_source,
        StoreError::PreconditionFailed {
            participant: Participant::REGISTRY_UPLOAD,
            ..
        }
    ));

    store
        .begin_completion(&upload(), &"a".repeat(64))
        .await
        .expect("the completion begins");

    // The same manifest re-enters `completing`; a different one cannot.
    store
        .begin_completion(&upload(), &"a".repeat(64))
        .await
        .expect("an identical retry is safe");
    let conflicting = store
        .begin_completion(&upload(), &"b".repeat(64))
        .await
        .expect_err("a different manifest");
    assert!(matches!(conflicting, StoreError::PreconditionFailed { .. }));

    store
        .finish_completion(upload_id(), &"a".repeat(64))
        .await
        .expect("the completion finishes");

    let loaded = store
        .load_upload(workspace(), upload_id())
        .await
        .expect("the read succeeds")
        .expect("the upload exists");
    assert_eq!(loaded.state, UploadState::Ready);
    assert_eq!(loaded.parts, upload().parts);
}
