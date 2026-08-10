//! Engine-backed cases for `session-authority`, against `DynamoDB` Local.
//!
//! This file replaces the crate's `integration` deferral, whose stated reason —
//! that no stream could start a pinned engine — stopped being true when
//! `aex_test_harness::containers` landed.
//!
//! This crate owns the machinery every regional adapter shares, so the cases are
//! about that machinery rather than about any one command:
//!
//! - **The cancellation decoding is positional and the position is real.** A
//!   plan whose *second* action loses must come back naming the second
//!   participant and carrying the row that condition saw. Every other adapter in
//!   the workspace depends on this being true, and until now every proof of it
//!   was a hand-written cancellation reason vector replayed by a scripted
//!   transport — which proves the decoder against its own author, not against
//!   `DynamoDB`.
//! - **A cancelled transaction leaves nothing behind.** Atomicity across four
//!   item families in one table is the property the whole plan compiler exists
//!   to deliver.
//! - **TTL is never the fence.** `DynamoDB` Local runs no TTL sweep, which makes
//!   it the ideal engine for this claim: the receipt row is provably still
//!   present and the reader must still refuse it once `expiresAt` has passed.
//!   On a real table AWS reclaims within 48 hours, so a reader that trusted TTL
//!   would replay an expired receipt for two days.
//! - **The workspace index is sparse.** Only a session head carries the index
//!   attributes, so a list query cannot surface a journal entry, a control row
//!   or an idempotency receipt however the query is written. `KEYS_ONLY` plus
//!   sparseness is an engine behaviour, not a request shape.
//! - **The zero-padded journal sort key orders numerically.** `J#…10` must sort
//!   after `J#…9` and before `J#…100`; a comparator settles that and a unit test
//!   can only restate it.
//!
//! What it does not prove, stated rather than assumed away:
//!
//! - **Strong consistency and `TransactionConflict` under real contention.**
//!   Both are recorded as `cannot_prove` for this engine in
//!   `release/policy/test-images.toml`, so `aws.dynamodb.transact_write` and
//!   `aws.dynamodb.throughput` stay `requires_live`.
//! - **TTL reclamation itself, and the change feed.** No sweep and no stream
//!   here; `aws.dynamodb.ttl` and `aws.dynamodb.streams` stay `requires_live`.
//! - **`aws.iam.denial`.** This engine authorises nothing.

mod support;

use std::sync::OnceLock;

use aex_session_dynamodb::attr::Item;
use aex_session_dynamodb::codec;
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::keys;
use aex_session_dynamodb::paging::PageBudget;
use aex_session_dynamodb::plan::{IMMUTABLE, Participant, TransactionPlan, key};
use aex_session_dynamodb::replay::{
    IdempotencyScope, Receipt, ReceiptBody, ReceiptStore, encode_receipt_row, key_digest,
};
use aex_session_dynamodb::store::{AgentJournalStore, SessionStore};
use aex_session_dynamodb::wire_pending::{Body, JournalEntry};
use aex_test_harness::DynamoDbLocalContainer;
use aex_wire::idempotency::{IdempotencyKey, IntentDigest};
use aex_wire::types::Timestamp;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_dynamodb::types::{
    AttributeDefinition, BillingMode, GlobalSecondaryIndex, KeySchemaElement, KeyType, Projection,
    ProjectionType, ScalarAttributeType,
};

use support::{control, later, now, root_agent, session, tables, workspace};

/// The checked-in generation definition the created table must match.
const DEFINITION: &str = include_str!("../../../migrations/regional/tables/session-authority.json");

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
/// `session-authority` carries one `KEYS_ONLY` index and two `INCLUDE` ones, so
/// the projection type is read from the document rather than assumed: a
/// `KEYS_ONLY` index recreated here as `INCLUDE` would make the sparse-index
/// case pass for the wrong reason.
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
            let declared = index["projection"]["type"].as_str().expect("a projection");
            let mut projection =
                Projection::builder().projection_type(ProjectionType::from(declared));
            if declared == "INCLUDE" {
                projection = projection.set_non_key_attributes(Some(
                    index["projection"]["attributes"]
                        .as_array()
                        .expect("an attribute list")
                        .iter()
                        .map(|value| value.as_str().expect("a string").to_owned())
                        .collect(),
                ));
            }
            GlobalSecondaryIndex::builder()
                .index_name(index["name"].as_str().expect("a name"))
                .set_key_schema(Some(schema(
                    index["partition"].as_str().expect("a partition"),
                    index["sort"].as_str().expect("a sort"),
                )))
                .projection(projection.build())
                .build()
                .expect("a complete index")
        })
        .collect();

    client
        .create_table()
        .table_name(definition_table())
        .set_attribute_definitions(Some(attributes))
        .set_key_schema(Some(schema("pk", "sk")))
        .set_global_secondary_indexes(Some(indexes))
        .billing_mode(BillingMode::PayPerRequest)
        .send()
        .await
        .expect("the table is created from the generation definition");
}

/// The physical `session-authority` name every case binds to.
fn definition_table() -> String {
    tables().session_authority
}

async fn engine() -> (DynamoDbLocalContainer, Client, SessionStore) {
    let engine = DynamoDbLocalContainer::start()
        .await
        .expect("DynamoDB Local starts");
    let client = client(&engine);
    create_table(&client).await;
    let store = SessionStore::new(client.clone(), tables());
    (engine, client, store)
}

/// One immutable journal entry at `seq`.
fn entry(seq: u64) -> JournalEntry {
    JournalEntry {
        seq,
        entry_id: format!("{seq:0>64}"),
        kind: "tool_call".to_owned(),
        body: Body::Inline(b"{}".to_vec()),
        body_bytes: 2,
        occurred_at: now(),
    }
}

/// The idempotency scope every receipt case uses.
///
/// The subject is interned rather than rebuilt per call because the scope
/// borrows it: a scope built from a temporary would not outlive the statement
/// that made it.
fn scope() -> IdempotencyScope<'static> {
    static SUBJECT: OnceLock<String> = OnceLock::new();
    let subject = SUBJECT.get_or_init(|| session().to_string());
    IdempotencyScope::new("session.message", Some(subject.as_str())).expect("a registered scope")
}

fn replay_key() -> IdempotencyKey {
    IdempotencyKey::parse("caller-chosen-key").expect("a well-formed replay key")
}

/// A receipt that stops being readable at `expires_at`.
fn receipt(expires_at: Timestamp) -> Receipt {
    Receipt {
        scope: scope().render(),
        key_sha256: key_digest(&replay_key()),
        intent: IntentDigest::from_bytes([0xab; 32]),
        response_kind: "run".to_owned(),
        response: ReceiptBody::Inline(b"{\"runId\":\"run_x\"}".to_vec()),
        committed_at: now(),
        expires_at,
    }
}

async fn put(client: &Client, item: Item) {
    client
        .put_item()
        .table_name(definition_table())
        .set_item(Some(item))
        .send()
        .await
        .expect("the fixture row is written");
}

async fn exists(client: &Client, pk: &str, sk: &str) -> bool {
    client
        .get_item()
        .table_name(definition_table())
        .set_key(Some(key(pk, sk)))
        .consistent_read(true)
        .send()
        .await
        .expect("the read succeeds")
        .item
        .is_some()
}

#[tokio::test]
async fn a_cancelled_transaction_names_the_participant_that_lost_and_returns_the_row_it_saw() {
    let (_engine, client, store) = engine().await;
    // A previous request already committed this replay key.
    put(
        &client,
        encode_receipt_row(workspace(), &receipt(later(86_400_000))).expect("the receipt encodes"),
    )
    .await;

    let journal = entry(1);
    let mut plan = TransactionPlan::new("integration:replayed-message");
    plan.put(
        Participant::AGENT_JOURNAL,
        aws_sdk_dynamodb::types::Put::builder()
            .table_name(definition_table())
            .set_item(Some(codec::encode_journal(
                session(),
                root_agent(),
                &journal,
            )))
            .condition_expression(IMMUTABLE),
    )
    .expect("the journal action is planned")
    .put(
        Participant::SESSION_IDEMPOTENCY,
        aws_sdk_dynamodb::types::Put::builder()
            .table_name(definition_table())
            .set_item(Some(
                encode_receipt_row(workspace(), &receipt(later(86_400_000)))
                    .expect("the receipt encodes"),
            ))
            .condition_expression(IMMUTABLE),
    )
    .expect("the receipt action is planned")
    .put(
        Participant::AGENT_CONTROL,
        aws_sdk_dynamodb::types::Put::builder()
            .table_name(definition_table())
            .set_item(Some(codec::encode_control(&control())))
            .condition_expression(IMMUTABLE),
    )
    .expect("the control action is planned");

    let error = store
        .commit(&plan)
        .await
        .expect_err("the replay key is already spent");
    let StoreError::PreconditionFailed {
        participant,
        observed,
    } = error
    else {
        panic!("a lost condition inside a transaction must be typed, not {error}");
    };
    assert_eq!(
        participant,
        Participant::SESSION_IDEMPOTENCY,
        "reason two belongs to action two; an off-by-one here mis-attributes every \
         cancellation in every regional adapter"
    );
    assert!(
        observed.is_some(),
        "every action carries ALL_OLD, so the loser receives the row without a second read"
    );

    let journal_key = keys::journal(session(), root_agent(), journal.seq);
    let control_key = keys::agent_control(session(), root_agent());
    assert!(
        !exists(&client, &journal_key.pk, &journal_key.sk).await,
        "a cancelled transaction committed its first action"
    );
    assert!(
        !exists(&client, &control_key.pk, &control_key.sk).await,
        "a cancelled transaction committed its third action"
    );
}

#[tokio::test]
async fn an_expired_receipt_is_refused_while_its_row_is_demonstrably_still_present() {
    let (_engine, client, store) = engine().await;
    let expires_at = later(60_000);
    put(
        &client,
        encode_receipt_row(workspace(), &receipt(expires_at)).expect("the receipt encodes"),
    )
    .await;

    let live = store
        .read_receipt(workspace(), &scope(), &replay_key(), now())
        .await
        .expect("the read succeeds")
        .expect("a live receipt replays");
    assert_eq!(
        live.response,
        ReceiptBody::Inline(b"{\"runId\":\"run_x\"}".to_vec())
    );

    assert!(
        store
            .read_receipt(workspace(), &scope(), &replay_key(), expires_at)
            .await
            .expect("the read succeeds")
            .is_none(),
        "expiry is the reader's own check; a receipt readable at its expiry \
         instant is a receipt that replays for as long as TTL takes to notice"
    );

    let receipt_key = keys::receipt(workspace(), &scope().render(), &key_digest(&replay_key()))
        .expect("the receipt key builds");
    assert!(
        exists(&client, &receipt_key.pk, &receipt_key.sk).await,
        "this engine runs no TTL sweep, which is exactly why it can prove the \
         refusal came from the reader and not from a deleted row"
    );
}

#[tokio::test]
async fn the_workspace_index_is_sparse_so_no_other_item_family_can_reach_a_list() {
    let (_engine, client, store) = engine().await;
    let head = aex_session_domain::testing::session_fixture();
    let encoded = aex_session_dynamodb::authority_codec::encode_session(&head)
        .expect("the canonical head encodes");
    let partition = keys::workspace_index::session_partition(head.workspace, "active");
    assert_eq!(
        encoded
            .get(keys::workspace_index::PK)
            .and_then(|value| value.as_s().ok())
            .map(String::as_str),
        Some(partition.as_str()),
        "the fixture head must be the live one; lifecycle lives inside the index \
         partition key, so a trashed fixture would be listed somewhere else"
    );
    put(&client, encoded).await;
    // Three other families in the same table, none of which may ever be listed.
    put(&client, codec::encode_control(&control())).await;
    put(
        &client,
        codec::encode_journal(session(), root_agent(), &entry(1)),
    )
    .await;
    put(
        &client,
        encode_receipt_row(workspace(), &receipt(later(86_400_000))).expect("the receipt encodes"),
    )
    .await;

    let listed = client
        .query()
        .table_name(store.table())
        .index_name(keys::workspace_index::NAME)
        .key_condition_expression("#pk = :pk")
        .expression_attribute_names("#pk", keys::workspace_index::PK)
        .expression_attribute_values(":pk", aex_session_dynamodb::attr::s(partition))
        .send()
        .await
        .expect("the index query succeeds")
        .items
        .expect("the index answers");

    assert_eq!(
        listed.len(),
        1,
        "only a session head carries the index attributes; anything else in this \
         partition means a prompt, a journal entry or a receipt can be listed"
    );
    let projected = &listed[0];
    assert_eq!(
        projected.len(),
        4,
        "a KEYS_ONLY index projects the base key and the index key and nothing \
         else; a wider projection would make an eventually consistent read into \
         session truth: {:?}",
        projected.keys().collect::<Vec<_>>()
    );
    assert!(
        !projected.contains_key("authorityDocument"),
        "the list index must not carry the authority document"
    );
}

#[tokio::test]
async fn the_journal_range_read_orders_by_sequence_rather_than_by_digits() {
    let (_engine, client, store) = engine().await;
    // 2, 10 and 100 are the three that expose an unpadded sort key: as bare
    // decimal text they sort 10, 100, 2.
    for seq in [2_u64, 10, 100] {
        put(
            &client,
            codec::encode_journal(session(), root_agent(), &entry(seq)),
        )
        .await;
    }

    let all = store
        .read_journal(
            session(),
            root_agent(),
            2,
            PageBudget::new(10).expect("a page budget"),
        )
        .await
        .expect("the journal read succeeds");
    assert_eq!(
        all.iter().map(|item| item.seq).collect::<Vec<_>>(),
        vec![2, 10, 100],
        "a fold that replays its journal out of order produces a different agent"
    );

    let bounded = store
        .read_journal(
            session(),
            root_agent(),
            2,
            PageBudget::new(2).expect("a page budget"),
        )
        .await
        .expect("the bounded journal read succeeds");
    assert_eq!(
        bounded.iter().map(|item| item.seq).collect::<Vec<_>>(),
        vec![2, 10],
        "the page budget must bound the read at the engine, not after it"
    );

    let resumed = store
        .read_journal(
            session(),
            root_agent(),
            11,
            PageBudget::new(10).expect("a page budget"),
        )
        .await
        .expect("the resumed journal read succeeds");
    assert_eq!(
        resumed.iter().map(|item| item.seq).collect::<Vec<_>>(),
        vec![100],
        "`sk >= :from` over the padded key is what makes `from_seq` a sequence \
         rather than a string prefix"
    );
}
