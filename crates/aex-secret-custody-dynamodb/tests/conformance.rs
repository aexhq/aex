//! Request and generation-definition conformance for `regional-secret-custody`.

mod support;

use aex_secret_custody_dynamodb::expressions::{self, AUTHORIZE_ORDER, SET_ORDER};
use aex_secret_custody_dynamodb::keys;
use aex_secret_custody_dynamodb::store::{CustodyStore, SecretCustodyStore};
use aex_secret_domain::revocation::RevocationEpoch;
use aex_secret_domain::secret::SecretRevision;
use aex_session_dynamodb::paging::PageBudget;
use aex_session_dynamodb::plan::Participant;

use support::{
    DEFINITION, TABLE, authorization, captured_body, capturing_client, credential_id, custody_head,
    generation, metadata, now, provider_credential, replaying_client, secret_name, session,
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
fn this_table_has_no_change_feed_no_index_and_no_timer() {
    let definition = definition();
    assert_eq!(
        definition["stream"]["enabled"].as_bool(),
        Some(false),
        "no consumer role should ever be able to read secret ciphertext off a feed"
    );
    assert!(
        definition["globalSecondaryIndexes"]
            .as_array()
            .expect("an array")
            .is_empty()
    );
    assert_eq!(
        definition["timeToLive"]["enabled"].as_bool(),
        Some(false),
        "deletion is explicit; a timer here would be a silent erasure path"
    );
}

#[test]
fn the_collector_holds_a_read_and_nothing_else() {
    let definition = definition();
    let grants = definition["iam"].as_array().expect("an array");
    let collector = grants
        .iter()
        .find(|grant| grant["role"].as_str() == Some("regional-otlp"))
        .expect("the collector is granted something");
    let actions: Vec<&str> = collector["actions"]
        .as_array()
        .expect("an array")
        .iter()
        .map(|action| action.as_str().expect("a string"))
        .collect();
    assert_eq!(
        actions,
        ["dynamodb:GetItem"],
        "the redaction manifest is reachable by one point read, and nothing else is"
    );
}

#[tokio::test]
async fn a_set_writes_the_generation_before_the_metadata_that_names_it() {
    let (client, receiver) = capturing_client();
    let store = CustodyStore::new(client, TABLE);
    let plan = expressions::set(TABLE, &generation(), &metadata(), None).expect("compiles");
    assert_eq!(plan.participants(), SET_ORDER);
    let _ignored = store.commit(&plan).await;

    let body = captured_body(receiver);
    let actions = body["TransactItems"].as_array().expect("three actions");
    assert_eq!(actions.len(), 3);
    assert_eq!(
        actions[0]["Put"]["Item"]["itemType"]["S"].as_str(),
        Some("secret_source_generation"),
        "metadata must never point at a generation that does not exist yet"
    );
    assert_eq!(
        actions[1]["Update"]["ConditionExpression"].as_str(),
        Some("attribute_not_exists(pk)")
    );
    assert_eq!(
        actions[1]["Update"]["ExpressionAttributeValues"][":itemType"]["S"].as_str(),
        Some("workspace_secret"),
        "an Update that creates metadata must write its discriminator"
    );
    assert!(
        actions[1]["Update"]["UpdateExpression"]
            .as_str()
            .expect("an update expression")
            .contains("revision = if_not_exists(revision, :zero) + :one"),
        "the authority, not a caller-supplied target value, advances the revision"
    );
}

#[tokio::test]
async fn a_replacement_set_conditions_on_the_revision_the_caller_read() {
    let (client, receiver) = capturing_client();
    let store = CustodyStore::new(client, TABLE);
    let mut replacement = metadata();
    replacement.revision = SecretRevision(2);
    replacement.generation = aex_secret_domain::secret::SourceGeneration(2);
    let mut next = generation();
    next.generation = replacement.generation;
    let plan = expressions::set(TABLE, &next, &replacement, Some(SecretRevision::FIRST))
        .expect("compiles");
    let _ignored = store.commit(&plan).await;

    let body = captured_body(receiver);
    assert_eq!(
        body["TransactItems"][1]["Update"]["ConditionExpression"].as_str(),
        Some("#state = :ready AND revision = :expectedRevision")
    );
}

#[tokio::test]
async fn an_authorization_checks_the_record_and_the_custody_before_it_writes_anything() {
    let (client, receiver) = capturing_client();
    let store = CustodyStore::new(client, TABLE);
    let plan = expressions::authorize_managed_call(TABLE, &authorization(), SecretRevision::FIRST)
        .expect("compiles");
    assert_eq!(plan.participants(), AUTHORIZE_ORDER);
    let _ignored = store.commit(&plan).await;

    let body = captured_body(receiver);
    let actions = body["TransactItems"].as_array().expect("three actions");
    assert_eq!(
        actions[0]["ConditionCheck"]["ConditionExpression"].as_str(),
        Some("#state = :ready AND revokedThroughRevision < :boundSourceRevision")
    );
    assert_eq!(
        actions[1]["ConditionCheck"]["ConditionExpression"].as_str(),
        Some("#state = :active AND custodyRevision = :expectedRevision")
    );
    assert_eq!(
        actions[2]["Put"]["ConditionExpression"].as_str(),
        Some("attribute_not_exists(pk)")
    );
}

#[tokio::test]
async fn a_revoke_is_one_conditional_update_and_never_a_fan_out() {
    let (client, receiver) = capturing_client();
    let store = CustodyStore::new(client, TABLE);
    let builder = expressions::revoke(
        TABLE,
        workspace(),
        secret_name().as_str(),
        RevocationEpoch::INITIAL,
        now(),
    )
    .expect("builds");
    let _ignored = store
        .commit_update(builder, Participant::SECRET_METADATA)
        .await;

    let body = captured_body(receiver);
    assert!(body["TransactItems"].is_null(), "a revoke is one item");
    assert!(
        body["UpdateExpression"]
            .as_str()
            .expect("an update")
            .contains("revokedThroughRevision = revision")
    );
}

#[tokio::test]
async fn every_authority_point_read_is_strongly_consistent() {
    for read in ["secret", "generation", "custody", "manifest"] {
        let (client, receiver) = capturing_client();
        let store = CustodyStore::new(client, TABLE);
        match read {
            "secret" => {
                let _ignored = store.load_secret(workspace(), &secret_name()).await;
            }
            "generation" => {
                let _ignored = store
                    .load_generation(workspace(), &secret_name(), generation().generation)
                    .await;
            }
            "custody" => {
                let _ignored = store.load_custody(workspace(), session()).await;
            }
            _ => {
                let _ignored = store.load_manifest(workspace(), session()).await;
            }
        }
        let body = captured_body(receiver);
        assert_eq!(
            body["ConsistentRead"].as_bool(),
            Some(true),
            "`{read}` answered from a replica"
        );
    }
    let _ = custody_head();
}

#[tokio::test]
async fn a_list_reads_the_metadata_partition_and_never_the_generation_partition() {
    let (client, receiver) = capturing_client();
    let store = CustodyStore::new(client, TABLE);
    let _ignored = store
        .list_secrets(workspace(), PageBudget::new(25).expect("a page"))
        .await;

    let body = captured_body(receiver);
    assert_eq!(
        body["ExpressionAttributeValues"][":pk"]["S"].as_str(),
        Some(keys::secret_partition(workspace()).as_str())
    );
    assert_eq!(
        body["ExpressionAttributeValues"][":prefix"]["S"].as_str(),
        Some(keys::secret_prefix())
    );
}

#[tokio::test]
async fn a_delete_tombstones_the_fence_row_rather_than_removing_it() {
    let (client, receiver) = capturing_client();
    let store = CustodyStore::new(client, TABLE);
    let builder = expressions::delete(
        TABLE,
        workspace(),
        secret_name().as_str(),
        SecretRevision::FIRST,
        now(),
    )
    .expect("builds");
    let _ignored = store
        .commit_update(builder, Participant::SECRET_METADATA)
        .await;

    let body = captured_body(receiver);
    assert!(
        body["TransactItems"].is_null(),
        "a delete is one conditional update on one item"
    );
    let condition = body["ConditionExpression"]
        .as_str()
        .expect("a conditional delete");
    assert!(
        condition.contains("attribute_exists(pk)"),
        "a delete of an absent record must not create a tombstone: {condition}"
    );
    assert!(
        condition.contains("revision = :expectedRevision"),
        "a delete must lose to a concurrent editor: {condition}"
    );
    assert!(
        condition.contains("#state <> :deleted"),
        "a second delete must be refused rather than advance the revision again: {condition}"
    );
    let update = body["UpdateExpression"].as_str().expect("an update");
    assert!(update.contains("#state = :deleted"), "{update}");
    assert!(
        update.contains("activeSourceGeneration = :none"),
        "a tombstone must point at no generation: {update}"
    );
    assert_eq!(
        body["ExpressionAttributeValues"][":nextRevision"]["N"].as_str(),
        Some("2"),
        "the tombstone advances the revision a concurrent editor fences on"
    );
}

#[test]
fn a_delete_of_an_unadvanceable_revision_is_refused_rather_than_wrapped() {
    assert!(
        expressions::delete(
            TABLE,
            workspace(),
            secret_name().as_str(),
            SecretRevision(u64::MAX),
            now(),
        )
        .is_err()
    );
}

#[tokio::test]
async fn a_credential_revocation_is_one_conditional_update_from_ready_only() {
    let (client, receiver) = capturing_client();
    let store = CustodyStore::new(client, TABLE);
    let builder = expressions::revoke_provider_credential(TABLE, &provider_credential(), now())
        .expect("builds");
    let _ignored = store
        .commit_update(builder, Participant::CUSTODY_PROVIDER_CREDENTIAL)
        .await;

    let body = captured_body(receiver);
    assert!(body["TransactItems"].is_null(), "a revocation is one item");
    let condition = body["ConditionExpression"]
        .as_str()
        .expect("a conditional revocation");
    assert!(condition.contains("#state = :ready"), "{condition}");
    assert!(
        condition.contains("revision = :expectedRevision"),
        "{condition}"
    );
    let update = body["UpdateExpression"].as_str().expect("an update");
    assert!(update.contains("#state = :revoked"), "{update}");
    assert!(update.contains("revokedAt = :now"), "{update}");
}

#[test]
fn a_revocation_cannot_be_compiled_from_an_already_revoked_binding() {
    let mut revoked = provider_credential();
    revoked.state = aex_secret_custody_dynamodb::CredentialState::Revoked;
    assert!(
        expressions::revoke_provider_credential(TABLE, &revoked, now()).is_err(),
        "a terminal binding is answered from the stored row, never revoked twice"
    );
}

#[tokio::test]
async fn a_credential_point_read_costs_one_read_per_provider_and_never_scans() {
    // The sort key is `CRED#{provider}#{credential}`, so an identity alone names
    // a suffix. `ProviderId` is closed at eight, so the read is a bounded fan of
    // strongly consistent point reads — never a Scan, and never an unbounded
    // Query over a directory that has no ceiling of its own.
    let providers = aex_wire::models::ProviderId::ALL.len();
    let (client, replay) = replaying_client(providers);
    let store = CustodyStore::new(client, TABLE);
    let found = store
        .load_provider_credential(workspace(), credential_id())
        .await
        .expect("the empty directory answers");
    assert_eq!(found, None);

    let requests: Vec<serde_json::Value> = replay
        .actual_requests()
        .map(|request| {
            serde_json::from_slice(
                request
                    .body()
                    .bytes()
                    .expect("the DynamoDB request body is always in memory"),
            )
            .expect("the DynamoDB request body is JSON")
        })
        .collect();
    assert_eq!(
        requests.len(),
        providers,
        "the read is bounded by the closed provider set"
    );
    let mut seen: Vec<String> = Vec::new();
    for body in &requests {
        assert_eq!(body["ConsistentRead"].as_bool(), Some(true));
        assert!(body["FilterExpression"].is_null(), "never a scan");
        let sort = body["Key"]["sk"]["S"].as_str().expect("a sort key");
        assert!(sort.starts_with("CRED#"), "{sort}");
        assert!(
            sort.ends_with(&credential_id().to_string()),
            "every read names the requested identity: {sort}"
        );
        assert!(!seen.contains(&sort.to_owned()), "no key is read twice");
        seen.push(sort.to_owned());
    }
}

#[tokio::test]
async fn a_page_resumes_from_the_continuation_it_was_given() {
    let (client, receiver) = capturing_client();
    let store = CustodyStore::new(client, TABLE);
    let position = aex_session_dynamodb::paging::PagePosition {
        pk: keys::secret_partition(workspace()),
        sk: "NAME#openai-key".to_owned(),
        index_pk: None,
        index_sk: None,
    };
    let _ignored = store
        .page_secrets(
            workspace(),
            PageBudget::new(25).expect("a page"),
            Some(&position),
        )
        .await;

    let body = captured_body(receiver);
    assert_eq!(
        body["ExclusiveStartKey"]["sk"]["S"].as_str(),
        Some("NAME#openai-key"),
        "a continuation that is not sent would replay the first page for ever"
    );
    assert_eq!(body["Limit"].as_u64(), Some(25));
}

/// `aex_secret_domain::set` produces `revoked_at: None`, so a set that left a
/// stale instant behind would publish a revocation date for a record that is
/// admissible again.
#[tokio::test]
async fn a_set_clears_the_revocation_instant_it_supersedes() {
    let (client, receiver) = capturing_client();
    let store = CustodyStore::new(client, TABLE);
    let plan = expressions::set(TABLE, &generation(), &metadata(), None).expect("compiles");
    let _ignored = store.commit(&plan).await;

    let body = captured_body(receiver);
    let update = body["TransactItems"][1]["Update"]["UpdateExpression"]
        .as_str()
        .expect("an update expression");
    assert!(
        update.contains("REMOVE revokedAt"),
        "the expression must match the domain transition it commits: {update}"
    );
}
