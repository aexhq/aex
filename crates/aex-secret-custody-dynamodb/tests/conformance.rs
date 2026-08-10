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
    dynamo_json, generation, metadata, now, provider_credential, receipt, replaying_client,
    scripted_client, secret_name, session, workspace,
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
fn the_collector_holds_nothing_at_all_on_this_table() {
    // The OTLP collector used to hold `GetItem` here, for the one row of the
    // removed redaction manifest. The platform's own credentials no longer
    // enter a sandbox, so there is nothing in a session's telemetry for a
    // collector to recognise and nothing on this table for it to read.
    let definition = definition();
    let grants = definition["iam"].as_array().expect("an array");
    assert!(
        grants
            .iter()
            .all(|grant| grant["role"].as_str() != Some("regional-otlp")),
        "a telemetry role with a grant on the secret-custody table is a path that should not exist"
    );
}

#[tokio::test]
async fn a_set_writes_the_generation_before_the_metadata_that_names_it() {
    let (client, receiver) = capturing_client();
    let store = CustodyStore::new(client, TABLE);
    let plan =
        expressions::set(TABLE, &generation(), &metadata(), None, None, None).expect("compiles");
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
    let plan = expressions::set(
        TABLE,
        &next,
        &replacement,
        Some(SecretRevision::FIRST),
        None,
        None,
    )
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

/// D-7 leaves exactly one point read on this table eventual — the idempotency
/// receipt — and this list is what says so. Nothing a conditional write is
/// compiled from may join the eventual side: a stale read there answers
/// `precondition_failed` about a precondition nothing violated.
#[tokio::test]
async fn every_authority_point_read_is_strongly_consistent() {
    for read in ["secret", "generation", "custody"] {
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
            _ => {
                let _ignored = store.load_custody(workspace(), session()).await;
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

/// The receipt read is the one point read D-7 relaxes, and relaxing it is the
/// larger half of the win because it is on **both** write paths.
///
/// A stale miss is self-healing rather than wrong: the receipt `Put` is
/// conditional on `attribute_not_exists`, so a duplicate attempt loses on the
/// receipt participant and `commit_or_replay` resolves it by exactly one
/// re-read. The whole cost is one wasted transaction attempt on a fast retry.
#[tokio::test]
async fn the_idempotency_receipt_is_the_one_point_read_answered_from_a_replica() {
    let (client, receiver) = capturing_client();
    let store = CustodyStore::new(client, TABLE);
    let _ignored = store
        .load_receipt(workspace(), "secret:set", &"ab".repeat(32), now())
        .await;

    let body = captured_body(receiver);
    assert_eq!(
        body["ConsistentRead"].as_bool(),
        Some(false),
        "the receipt read pays for consistency it provably does not need"
    );
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

/// A `Query` stops at the service's 1 MB boundary whatever `Limit` says, so
/// one request is not the collection. The bare list has no continuation to hand
/// back and must walk every service page itself: a list that dropped the
/// continuation would read as "these are all your secrets" to a rotation sweep.
#[tokio::test]
async fn a_list_walks_every_service_page_before_answering() {
    let first = aex_secret_custody_dynamodb::codec::encode_secret(&metadata()).expect("encodes");
    let mut renamed = metadata();
    renamed.name =
        aex_wire::ids::ResourceName::parse("anthropic-key").expect("an ASCII resource name");
    let second = aex_secret_custody_dynamodb::codec::encode_secret(&renamed).expect("encodes");
    let boundary = serde_json::json!({
        "pk": dynamo_json(&first)["pk"],
        "sk": dynamo_json(&first)["sk"],
    });
    let (client, replay) = scripted_client(vec![
        serde_json::json!({"Items": [dynamo_json(&first)], "LastEvaluatedKey": boundary}),
        serde_json::json!({"Items": [dynamo_json(&second)]}),
    ]);
    let store = CustodyStore::new(client, TABLE);

    let listed = store
        .list_secrets(workspace(), PageBudget::new(25).expect("a page"))
        .await
        .expect("both service pages answer");
    assert_eq!(
        listed
            .iter()
            .map(|secret| secret.name.as_str().to_owned())
            .collect::<Vec<_>>(),
        ["openai-key", "anthropic-key"],
        "rows from every service page survive, in key order"
    );

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
    assert_eq!(requests.len(), 2);
    assert!(requests[0]["ExclusiveStartKey"].is_null());
    assert_eq!(
        requests[1]["ExclusiveStartKey"]["sk"],
        dynamo_json(&first)["sk"],
        "the resumed query starts from the service continuation"
    );
    assert_eq!(requests[0]["Limit"].as_u64(), Some(25));
    assert_eq!(
        requests[1]["Limit"].as_u64(),
        Some(24),
        "the resumed query spends only the remaining budget"
    );
    // The consistency clause that stood here is gone, deliberately, and it is
    // stated rather than silently dropped: D-7 takes every custody listing
    // eventual, and this walk is a listing. What this test exists to prove — the
    // continuation, the `Limit` arithmetic and the refusal below — is unchanged
    // and is asserted above. The flip itself is pinned by
    // `every_custody_listing_is_answered_from_a_replica`.
    for request in &requests {
        assert_eq!(
            request["ConsistentRead"].as_bool(),
            Some(false),
            "a listing is a projection, not an authority read"
        );
    }
}

/// D-7's listing flip, pinned in one place.
///
/// `query_prefix` sets no consistency of its own — it walks its service pages
/// through the same private `query_page` the paged listings use — so all four
/// listings flip together and none can drift from the other three.
#[tokio::test]
async fn every_custody_listing_is_answered_from_a_replica() {
    let budget = PageBudget::new(25).expect("a page");
    for listing in ["secrets", "credentials", "secret-page", "credential-page"] {
        let (client, receiver) = capturing_client();
        let store = CustodyStore::new(client, TABLE);
        match listing {
            "secrets" => {
                let _ignored = store.list_secrets(workspace(), budget).await;
            }
            "credentials" => {
                let _ignored = store.list_provider_credentials(workspace(), budget).await;
            }
            "secret-page" => {
                let _ignored = store.page_secrets(workspace(), budget, None).await;
            }
            _ => {
                let _ignored = store
                    .page_provider_credentials(workspace(), budget, None)
                    .await;
            }
        }
        let body = captured_body(receiver);
        assert_eq!(
            body["ConsistentRead"].as_bool(),
            Some(false),
            "`{listing}` still pays for consistency a projection does not need"
        );
    }
}

/// The bare list cannot resume, so a collection that outgrows its budget is a
/// typed refusal — the caller's fix is the paged listing, and a silently short
/// list would hide that a page of secrets was never considered.
#[tokio::test]
async fn a_collection_past_the_budget_is_refused_rather_than_truncated() {
    let first = aex_secret_custody_dynamodb::codec::encode_secret(&metadata()).expect("encodes");
    let mut renamed = metadata();
    renamed.name =
        aex_wire::ids::ResourceName::parse("anthropic-key").expect("an ASCII resource name");
    let second = aex_secret_custody_dynamodb::codec::encode_secret(&renamed).expect("encodes");
    let boundary = serde_json::json!({
        "pk": dynamo_json(&second)["pk"],
        "sk": dynamo_json(&second)["sk"],
    });
    let (client, _replay) = scripted_client(vec![serde_json::json!({
        "Items": [dynamo_json(&first), dynamo_json(&second)],
        "LastEvaluatedKey": boundary,
    })]);
    let store = CustodyStore::new(client, TABLE);

    let error = store
        .list_secrets(workspace(), PageBudget::new(2).expect("a page"))
        .await
        .expect_err("a full budget with rows still behind it cannot answer");
    assert!(
        matches!(
            error,
            aex_session_dynamodb::error::StoreError::Invalid { .. }
        ),
        "{error:?}"
    );
    assert!(format!("{error}").contains("paged listing"), "{error}");
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
    let plan =
        expressions::set(TABLE, &generation(), &metadata(), None, None, None).expect("compiles");
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

// --- D-6: the receipt is a participant of the write it protects ---------------------

/// The receipt `Put` travels in the **same** transaction as the domain rows.
///
/// A receipt written in a second transaction reintroduces exactly the ambiguous
/// window the receipt exists to close: a crash between the two leaves an effect
/// with no record that it happened, and the retry commits it twice.
#[tokio::test]
async fn a_first_seal_writes_its_receipt_inside_the_transaction_it_protects() {
    let (client, receiver) = capturing_client();
    let store = CustodyStore::new(client, TABLE);
    let plan = expressions::set(
        TABLE,
        &generation(),
        &metadata(),
        None,
        Some(&receipt()),
        None,
    )
    .expect("compiles");
    assert_eq!(
        plan.participants(),
        [
            Participant::SECRET_GENERATION,
            Participant::SECRET_METADATA,
            Participant::SECRET_LINEAGE,
            Participant::SECRET_IDEMPOTENCY,
        ]
    );
    let _ignored = store.commit(&plan).await;

    let body = captured_body(receiver);
    let actions = body["TransactItems"].as_array().expect("four actions");
    assert_eq!(actions.len(), 4);
    assert_eq!(
        actions[3]["Put"]["Item"]["itemType"]["S"].as_str(),
        Some("idempotency_receipt")
    );
    assert_eq!(
        actions[3]["Put"]["ConditionExpression"].as_str(),
        Some("attribute_not_exists(pk)"),
        "the conditional put is the replay election; a loser must write nothing"
    );
}

/// The receipt row is the shared shape, and no part of a secret may enter it.
#[tokio::test]
async fn no_ciphertext_or_wrapped_key_reaches_the_receipt_row() {
    let (client, receiver) = capturing_client();
    let store = CustodyStore::new(client, TABLE);
    let plan = expressions::set(
        TABLE,
        &generation(),
        &metadata(),
        None,
        Some(&receipt()),
        None,
    )
    .expect("compiles");
    let _ignored = store.commit(&plan).await;

    let body = captured_body(receiver);
    let stored = &body["TransactItems"][3]["Put"]["Item"];
    for forbidden in ["ciphertext", "wrappedKey", "nonce", "encContextDigest"] {
        assert!(
            stored[forbidden].is_null(),
            "`{forbidden}` reached a durable receipt"
        );
    }
    assert_eq!(
        stored["scope"]["S"].as_str(),
        Some(receipt().scope.as_str())
    );
}

// --- D-13: the declared `limit_exceeded` has something behind it --------------------

/// The counter guard rides the transaction, so a create past the bound commits
/// **nothing** — no generation, no metadata, no lineage and no receipt.
#[tokio::test]
async fn a_create_consumes_a_guarded_counter_in_the_same_transaction() {
    let (client, receiver) = capturing_client();
    let store = CustodyStore::new(client, TABLE);
    let plan = expressions::set(
        TABLE,
        &generation(),
        &metadata(),
        None,
        Some(&receipt()),
        Some(expressions::Quota::secrets(500)),
    )
    .expect("compiles");
    assert_eq!(plan.participants().last(), Some(&Participant::SECRET_COUNT));
    let _ignored = store.commit(&plan).await;

    let body = captured_body(receiver);
    let counter = &body["TransactItems"][4]["Update"];
    assert_eq!(
        counter["Key"]["sk"]["S"].as_str(),
        Some("COUNT"),
        "the counter sorts outside every listing prefix"
    );
    assert_eq!(
        counter["Key"]["pk"]["S"].as_str(),
        Some(keys::secret_partition(workspace()).as_str()),
        "the guard lives in the collection it bounds, so it is one participant \
         rather than a second round trip"
    );
    assert_eq!(
        counter["ConditionExpression"].as_str(),
        Some("attribute_not_exists(#count) OR #count < :max"),
        "the bound refuses the (max + 1)-th create, not the max-th"
    );
    assert!(
        counter["UpdateExpression"]
            .as_str()
            .expect("an update")
            .contains("ADD #count :one"),
        "the authority increments; a caller-supplied total would be a race"
    );
    assert_eq!(
        counter["ExpressionAttributeValues"][":max"]["N"].as_str(),
        Some("500")
    );
    assert_eq!(
        counter["ExpressionAttributeValues"][":itemType"]["S"].as_str(),
        Some("custody_counter"),
        "an Update that creates a row must write its own discriminator"
    );
}

/// A replace does not consume quota, and asking it to is refused rather than
/// silently ignored: the collection does not grow, so a rotation loop must not
/// be able to exhaust a workspace's bound.
#[test]
fn a_replace_cannot_be_compiled_with_a_quota_guard() {
    let mut replacement = metadata();
    replacement.revision = SecretRevision(2);
    replacement.generation = aex_secret_domain::secret::SourceGeneration(2);
    let mut next = generation();
    next.generation = replacement.generation;
    let error = expressions::set(
        TABLE,
        &next,
        &replacement,
        Some(SecretRevision::FIRST),
        Some(&receipt()),
        Some(expressions::Quota::secrets(500)),
    )
    .expect_err("a replace consumes no quota");
    assert!(format!("{error}").contains("does not grow"), "{error}");
}
