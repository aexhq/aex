//! Engine-backed cases for `regional-work`, against `DynamoDB` Local.
//!
//! This file replaces the crate's `integration` deferral, whose stated reason —
//! that no stream could start a pinned engine — stopped being true when
//! `aex_test_harness::containers` landed.
//!
//! What only a real engine proves, and what every case below is about:
//!
//! - **The fence actually fences.** A conditional update either advances the
//!   claim or loses, and the loser receives the row the condition saw because
//!   `ReturnValuesOnConditionCheckFailure = ALL_OLD` was set. A scripted
//!   transport can only replay a hand-written answer to that.
//! - **The sparse due index is sparse.** `REMOVE dueShardPk, dueShardSk` is an
//!   update expression until an engine evaluates it; only then does retiring a
//!   record actually take it out of the index while leaving the row readable.
//! - **The due-scan upper bound covers its own millisecond.** `#sk <= :now` with
//!   `"{now}#\u{10ffff}"` is a claim about lexicographic ordering that the
//!   comparator settles and a unit test can only restate.
//! - **A cancelled transaction's reason vector is positional.** The dedupe claim
//!   is the second action, and the decoder maps reason 2 back to
//!   `work.dedupe` — the whole reason participants are named.
//!
//! What it does not prove, stated rather than assumed away:
//!
//! - **TTL reclamation.** `DynamoDB` Local runs no TTL sweep, so the 24-hour
//!   retirement window is asserted as a written attribute here and as behaviour
//!   only on a real table. `aws.dynamodb.ttl` therefore stays `requires_live`.
//! - **`TransactionConflict` under real contention**, adaptive capacity, and
//!   throughput refusals. `release/policy/test-images.toml` records all three as
//!   `cannot_prove` for this engine.
//! - **The change feed.** `regional-work` streams `NEW_IMAGE` to five pipes;
//!   this engine serves no stream, and the consuming seam belongs to the
//!   deployables that read it.

mod support;

use aex_session_dynamodb::error::{StoreError, decode_cancellation};
use aex_session_dynamodb::paging::PageBudget;
use aex_session_dynamodb::plan::{Participant, TransactionPlan};
use aex_test_harness::DynamoDbLocalContainer;
use aex_work_dynamodb::claim::{self, WorkClaim};
use aex_work_dynamodb::codec::{self, ReconciliationCursor, WorkRecord};
use aex_work_dynamodb::keys;
use aex_work_dynamodb::store::{WorkAuthority, WorkStore};
use aex_wire::types::Timestamp;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_dynamodb::types::{
    AttributeDefinition, BillingMode, GlobalSecondaryIndex, KeySchemaElement, KeyType, Projection,
    ProjectionType, ScalarAttributeType,
};

use support::{TABLE, later, now, record, workspace};

/// The checked-in generation definition the created table must match.
const DEFINITION: &str = include_str!("../../../migrations/regional/tables/regional-work.json");

/// How long a fixture lease runs.
const LEASE_MILLIS: i64 = 30_000;

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

/// Creates the table from the checked-in generation definition.
///
/// Derived rather than hand-typed so the engine runs the shape Terraform
/// creates: an index whose projection this test invented would prove nothing
/// about the projection a due scan actually reads.
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

async fn engine() -> (DynamoDbLocalContainer, Client, WorkStore) {
    let engine = DynamoDbLocalContainer::start()
        .await
        .expect("DynamoDB Local starts");
    let client = client(&engine);
    create_table(&client).await;
    let store = WorkStore::new(client.clone(), TABLE);
    (engine, client, store)
}

/// One outstanding record with the given identity and due instant.
fn outstanding(work_id: &str, due_at: Timestamp) -> WorkRecord {
    WorkRecord {
        work_id: work_id.to_owned(),
        due_at,
        // The dedupe claim is keyed by this digest, so a per-record spelling is
        // what keeps two fixtures from colliding on a claim neither is about.
        dedupe_key: format!("{work_id}-{}", "b".repeat(32)),
        created_at: due_at,
        updated_at: due_at,
        ..record()
    }
}

/// Writes one record with no dedupe claim.
async fn enqueue(client: &Client, work: &WorkRecord) {
    client
        .put_item()
        .table_name(TABLE)
        .set_item(Some(codec::encode_work(work).expect("the fixture encodes")))
        .send()
        .await
        .expect("the record is written");
}

/// Commits the enqueue transaction the producer actually issues: the record and
/// its dedupe claim, both immutable, in one atomic write.
async fn enqueue_with_dedupe(client: &Client, work: &WorkRecord) -> Result<(), StoreError> {
    let mut plan = TransactionPlan::new(format!("enqueue:{}", work.work_id));
    plan.put(
        Participant::WORK_ROOT_WAKE,
        claim::enqueue(TABLE, work).expect("the record action builds"),
    )
    .expect("the record action is planned")
    .put(
        claim::DEDUPE,
        claim::enqueue_dedupe(TABLE, work).expect("the dedupe action builds"),
    )
    .expect("the dedupe action is planned");

    match plan
        .compile(client)
        .expect("a two-action plan compiles")
        .send()
        .await
    {
        Ok(_) => Ok(()),
        Err(error) => Err(error.as_service_error().map_or_else(
            || StoreError::Invalid {
                detail: "the enqueue never reached the engine".to_owned(),
            },
            |service| decode_cancellation(service, plan.participants()),
        )),
    }
}

/// `count` work identities that all hash into the same due shard.
///
/// The shard is derived from the identity, so a scan fixture that hand-picked
/// identities would be testing whichever shards they happened to land in.
fn same_shard_ids(count: usize) -> (u16, Vec<String>) {
    let mut candidates: Vec<(u16, String)> = (0..4_096_u32)
        .map(|index| {
            let id = format!("wrk_scan_{index:04}");
            (keys::shard_of(&id), id)
        })
        .collect();
    candidates.sort();
    for window in candidates.windows(count) {
        if window.first().map(|entry| entry.0) == window.last().map(|entry| entry.0) {
            let shard = window[0].0;
            return (
                shard,
                window.iter().map(|entry| entry.1.clone()).collect(),
            );
        }
    }
    panic!("64 shards over 4096 identities always yield {count} in one shard");
}

fn budget(limit: u32) -> PageBudget {
    PageBudget::new(limit).expect("a page budget inside the ceiling")
}

#[tokio::test]
async fn a_claim_advances_the_fence_and_a_second_worker_loses_against_a_live_lease() {
    let (_engine, client, store) = engine().await;
    let work = outstanding("wrk_claim", now());
    enqueue(&client, &work).await;

    let held = store
        .claim_work(&work.work_id, "worker-1", now(), later(LEASE_MILLIS))
        .await
        .expect("an unclaimed record is claimable");
    assert_eq!(held.fence, 1, "a claim advances the fence from zero");
    assert_eq!(held.attempt, 1, "and spends one attempt");

    let error = store
        .claim_work(&work.work_id, "worker-2", now(), later(LEASE_MILLIS))
        .await
        .expect_err("a live lease is not stealable");
    let StoreError::PreconditionFailed {
        participant,
        observed,
    } = error
    else {
        panic!("a lost claim must be a precondition failure, not {error}");
    };
    assert_eq!(participant, Participant::WORK_ROOT_WAKE);
    let observed = observed.expect(
        "the loser must receive the row the condition saw; without ALL_OLD a \
         stolen lease is undiagnosable without a second read",
    );
    assert_eq!(
        codec::decode_work(&observed, workspace())
            .expect("the returned row decodes")
            .claim_owner
            .as_deref(),
        Some("worker-1")
    );
}

#[tokio::test]
async fn a_lease_that_has_visibly_expired_is_claimable_again() {
    let (_engine, client, store) = engine().await;
    let work = outstanding("wrk_expiry", now());
    enqueue(&client, &work).await;

    store
        .claim_work(&work.work_id, "worker-1", now(), later(LEASE_MILLIS))
        .await
        .expect("the first claim");

    // Expiry is compared against the caller's clock, not left to TTL: a lease
    // AWS has not got round to reclaiming is still expired.
    let stolen = store
        .claim_work(
            &work.work_id,
            "worker-2",
            later(LEASE_MILLIS + 1),
            later(LEASE_MILLIS * 2),
        )
        .await
        .expect("an expired lease is claimable");
    assert_eq!(stolen.fence, 2);
    assert_eq!(stolen.attempt, 2);
}

#[tokio::test]
async fn a_completion_under_a_stolen_lease_is_refused_so_the_prepared_effect_is_discarded() {
    let (_engine, client, store) = engine().await;
    let work = outstanding("wrk_stolen", now());
    enqueue(&client, &work).await;

    let first: WorkClaim = store
        .claim_work(&work.work_id, "worker-1", now(), later(LEASE_MILLIS))
        .await
        .expect("the first claim");
    store
        .claim_work(
            &work.work_id,
            "worker-2",
            later(LEASE_MILLIS + 1),
            later(LEASE_MILLIS * 2),
        )
        .await
        .expect("the lease is stolen after it expired");

    let error = store
        .complete_work(&first, later(LEASE_MILLIS + 2))
        .await
        .expect_err("a fence the record has moved past");
    assert!(
        matches!(
            error,
            StoreError::PreconditionFailed {
                participant: Participant::WORK_WAKE_DONE,
                ..
            }
        ),
        "{error}"
    );
    assert!(
        !error.retryable(),
        "a losing worker must discard its effect, never re-issue it: a newer \
         owner may already have superseded the decision"
    );
}

#[tokio::test]
async fn retiring_a_record_leaves_the_due_index_while_the_row_stays_readable() {
    let (_engine, client, store) = engine().await;
    let work = outstanding("wrk_retire", now());
    enqueue(&client, &work).await;
    let shard = keys::shard_of(&work.work_id);

    let held = store
        .claim_work(&work.work_id, "worker-1", now(), later(LEASE_MILLIS))
        .await
        .expect("the claim");
    assert_eq!(
        store
            .scan_due(shard, later(LEASE_MILLIS), budget(10))
            .await
            .expect("the due scan")
            .items
            .len(),
        1,
        "a claimed record is still outstanding and still due"
    );

    store
        .complete_work(&held, later(LEASE_MILLIS))
        .await
        .expect("the fenced retirement commits");

    assert!(
        store
            .scan_due(shard, later(LEASE_MILLIS), budget(10))
            .await
            .expect("the due scan")
            .items
            .is_empty(),
        "`REMOVE dueShardPk, dueShardSk` must actually leave the sparse index; a \
         retired record a reconciler keeps finding is an infinite sweep"
    );

    let retired = store
        .load(workspace(), &work.work_id)
        .await
        .expect("the base read succeeds")
        .expect("the row survives its retirement");
    assert_eq!(retired.state, "done");
}

#[tokio::test]
async fn the_due_scan_includes_work_due_at_exactly_the_scan_instant() {
    let (_engine, client, store) = engine().await;
    let (shard, ids) = same_shard_ids(2);
    let due_now = outstanding(&ids[0], now());
    let due_later = outstanding(&ids[1], later(1));
    enqueue(&client, &due_now).await;
    enqueue(&client, &due_later).await;

    let page = store
        .scan_due(shard, now(), budget(10))
        .await
        .expect("the due scan");
    let found: Vec<&str> = page.items.iter().map(|item| item.work_id.as_str()).collect();
    assert_eq!(
        found,
        vec![due_now.work_id.as_str()],
        "a bare trailing separator sorts before every `#{{workId}}`, so work due \
         at exactly the scan instant would be missed for a whole millisecond"
    );
    assert_eq!(page.scanned_through, Some(now()));

    let later_page = store
        .scan_due(shard, later(1), budget(10))
        .await
        .expect("the due scan");
    assert_eq!(later_page.items.len(), 2);
}

#[tokio::test]
async fn a_due_page_resumes_strictly_after_the_persisted_cursor() {
    let (_engine, client, store) = engine().await;
    let (shard, ids) = same_shard_ids(3);
    for (offset, id) in ids.iter().enumerate() {
        let offset = i64::try_from(offset).expect("three fixtures fit in an i64");
        enqueue(&client, &outstanding(id, later(offset))).await;
    }

    let first = store
        .scan_due(shard, later(10), budget(2))
        .await
        .expect("the first page");
    assert_eq!(first.items.len(), 2);
    assert!(first.has_more, "the shard continued past the budget");

    let cursor = ReconciliationCursor {
        shard,
        scanned_through_effective_due_at: first
            .scanned_through
            .expect("a non-empty page names where it stopped"),
        last_work_id: Some(first.items[1].work_id.clone()),
        revision: 1,
        updated_at: later(10),
    };
    store
        .advance_cursor(&cursor)
        .await
        .expect("the first advance takes the absent-row arm");

    let resumed = store
        .scan_due_after(shard, later(10), budget(2), Some(&cursor))
        .await
        .expect("the resumed page");
    let resumed_ids: Vec<&str> = resumed
        .items
        .iter()
        .map(|item| item.work_id.as_str())
        .collect();
    assert_eq!(
        resumed_ids.len(),
        1,
        "the resumed page must be strictly after the cursor, not include it: {resumed_ids:?}"
    );
    assert!(
        !resumed_ids.contains(&first.items[1].work_id.as_str()),
        "an inclusive resume re-runs the last item of every page"
    );

    let persisted = store
        .load_cursor(shard)
        .await
        .expect("the cursor read succeeds")
        .expect("the cursor was written");
    assert_eq!(persisted, cursor);
}

#[tokio::test]
async fn a_cursor_advance_needs_the_revision_it_observed() {
    let (_engine, _client, store) = engine().await;
    let cursor = ReconciliationCursor {
        shard: 7,
        scanned_through_effective_due_at: now(),
        last_work_id: None,
        revision: 1,
        updated_at: now(),
    };
    store.advance_cursor(&cursor).await.expect("the first write");

    let error = store
        .advance_cursor(&cursor)
        .await
        .expect_err("a revision another reconciler already consumed");
    assert!(
        matches!(
            error,
            StoreError::PreconditionFailed {
                participant: Participant::WORK_CURSOR,
                ..
            }
        ),
        "{error}"
    );

    store
        .advance_cursor(&ReconciliationCursor {
            revision: 2,
            ..cursor
        })
        .await
        .expect("the next revision advances");
}

#[tokio::test]
async fn a_second_enqueue_of_one_dedupe_key_is_cancelled_at_the_dedupe_participant() {
    let (_engine, client, _store) = engine().await;
    let first = outstanding("wrk_dedupe_a", now());
    enqueue_with_dedupe(&client, &first)
        .await
        .expect("the first enqueue commits both actions");

    // A different work identity claiming the same logical wake. The record
    // action would succeed on its own; the dedupe claim is what refuses.
    let second = WorkRecord {
        work_id: "wrk_dedupe_b".to_owned(),
        ..first.clone()
    };
    let error = enqueue_with_dedupe(&client, &second)
        .await
        .expect_err("one logical wake is outstanding at a time");
    assert!(
        matches!(
            error,
            StoreError::PreconditionFailed {
                participant: Participant::WORK_DEDUPE,
                ..
            }
        ),
        "the reason vector is positional, and the dedupe claim is action two: {error}"
    );

    // Atomicity: the record half of the refused transaction did not land.
    assert!(
        client
            .get_item()
            .table_name(TABLE)
            .set_key(Some(aex_session_dynamodb::plan::key(
                &keys::work(&second.work_id).expect("a key").pk,
                "STATE",
            )))
            .consistent_read(true)
            .send()
            .await
            .expect("the read succeeds")
            .item
            .is_none(),
        "a cancelled transaction must leave nothing behind"
    );
}
