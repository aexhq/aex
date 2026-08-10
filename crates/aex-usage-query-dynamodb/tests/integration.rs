//! Engine-backed cases for `usage-query-projection`, against `DynamoDB` Local.
//!
//! This file replaces the crate's `integration` deferral. What it proves could
//! not be proved by a scripted transport, because every claim below is a
//! property of the engine's own key ordering and range evaluation rather than of
//! the request this crate builds:
//!
//! - a page stops at the row budget and hands back a continuation that resumes
//!   at exactly the next row, with no row read twice and none skipped;
//! - the bare-bucket upper bound really is exclusive, so two adjacent pages of
//!   one month never double-count a rollup — the reason `BETWEEN` is legal here
//!   at all is that `H#<bucket>` sorts strictly before `H#<bucket>#<hash>`, and
//!   only a real comparator settles that;
//! - `H#` and `D#` occupy disjoint sort-key space inside one partition, so an
//!   hourly total can never absorb a daily one;
//! - a read pinned to one generation cannot observe a rebuild writing another,
//!   which is what makes a cursor issued against a stale generation expire
//!   instead of silently mixing two;
//! - a row of another item type inside the range is a typed refusal rather than
//!   a silently skipped row, so an incomplete total is never served as a total;
//! - a table the composition does not hold is `Misconfigured`, never an empty
//!   answer — a customer must not be told they used nothing because a name is
//!   wrong.
//!
//! What it does not prove, stated rather than assumed away:
//!
//! - **Strong consistency.** Every read here asks for it and `DynamoDB` Local
//!   is a single process, so it cannot distinguish a strong read from an
//!   eventually consistent one. The claim in `store.rs`'s consistency note is
//!   evidenced by the request shape at the unit layer and by the live companion,
//!   not here.
//! - **`AccessDeniedException`.** The read-only IAM shape is the subject of
//!   `tests/write_incapability.rs` and of a deployed denial assertion; this
//!   engine authorises nothing.
//! - **TTL expiry of detail rows.** `DynamoDB` Local implements no TTL sweep.
//!   The reader's own `expiresAt` check is the fence and is covered at the unit
//!   layer.

use std::collections::HashMap;
use std::sync::OnceLock;

use aex_test_harness::DynamoDbLocalContainer;
use aex_usage_domain::frontier::{FrontierState, PoisonReason};
use aex_usage_domain::meter::PublicCategory;
use aex_usage_domain::projection::{Generation, ProjectionKey, ProjectionKeys};
use aex_usage_domain::quantity::Quantity;
use aex_usage_domain::wire_pending::WorkspaceId;
use aex_usage_query_dynamodb::store::{AGGREGATE, COVERAGE, GENERATION_POINTER, ITEM_TYPE};
use aex_usage_query_dynamodb::{
    AggregateRequest, Granularity, QueryError, UsageProjectionReads, UsageQueryStore,
};
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_dynamodb::types::{
    AttributeDefinition, AttributeValue, BillingMode, KeySchemaElement, KeyType,
    ScalarAttributeType,
};

/// The pinned physical table name.
const TABLE: &str = "dev-eu-west-1-usage-query-projection";
/// The month every fixture row lives in.
const MONTH: &str = "2026-08";

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

/// Creates the projection table.
///
/// `usage-query-projection` declares two attributes, one composite key and no
/// global secondary index, so the shape is stated inline rather than derived
/// from the definition document: there is nothing here a generated builder would
/// keep honest that this does not.
async fn create_table(client: &Client) {
    let attribute = |name: &str| {
        AttributeDefinition::builder()
            .attribute_name(name)
            .attribute_type(ScalarAttributeType::S)
            .build()
            .expect("a complete attribute")
    };
    let key = |name: &str, kind: KeyType| {
        KeySchemaElement::builder()
            .attribute_name(name)
            .key_type(kind)
            .build()
            .expect("a complete key element")
    };
    client
        .create_table()
        .table_name(TABLE)
        .set_attribute_definitions(Some(vec![attribute("pk"), attribute("sk")]))
        .set_key_schema(Some(vec![
            key("pk", KeyType::Hash),
            key("sk", KeyType::Range),
        ]))
        .billing_mode(BillingMode::PayPerRequest)
        .send()
        .await
        .expect("the table is created");
}

async fn engine() -> (DynamoDbLocalContainer, Client, UsageQueryStore) {
    let engine = DynamoDbLocalContainer::start()
        .await
        .expect("DynamoDB Local starts");
    let client = client(&engine);
    create_table(&client).await;
    let store = UsageQueryStore::new(client.clone(), TABLE);
    (engine, client, store)
}

/// The one fixture workspace.
///
/// Interned rather than rebuilt per call because [`AggregateRequest`] borrows
/// it, and a request that outlives a temporary is the only thing a caller could
/// get wrong here.
fn workspace() -> &'static WorkspaceId {
    static WORKSPACE: OnceLock<WorkspaceId> = OnceLock::new();
    WORKSPACE.get_or_init(|| WorkspaceId::parse("ws-1").expect("workspace"))
}

fn text(value: &str) -> AttributeValue {
    AttributeValue::S(value.to_owned())
}

fn number(value: u64) -> AttributeValue {
    AttributeValue::N(value.to_string())
}

/// Writes one row at the key the shared grammar produced.
async fn put(client: &Client, key: &ProjectionKey, attributes: Vec<(&str, AttributeValue)>) {
    let mut item: HashMap<String, AttributeValue> = attributes
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value))
        .collect();
    item.insert("pk".to_owned(), text(&key.pk));
    item.insert(
        "sk".to_owned(),
        text(key.sk.as_deref().expect("a fully qualified fixture key")),
    );
    client
        .put_item()
        .table_name(TABLE)
        .set_item(Some(item))
        .send()
        .await
        .expect("the fixture row is written");
}

/// One stored rollup, written the way the fold writes it.
///
/// The test writes rows with the raw client rather than through this crate on
/// purpose: the crate has no write surface at all, and `U-20` is the reason.
async fn put_aggregate(
    client: &Client,
    generation: Generation,
    granularity: Granularity,
    bucket: &str,
    dimension_hash: &str,
    quantity: u64,
) {
    let keys = ProjectionKeys;
    let key = match granularity {
        Granularity::Hourly => keys.hourly(
            generation,
            workspace(),
            PublicCategory::Compute,
            MONTH,
            bucket,
            dimension_hash,
        ),
        Granularity::Daily => keys.daily(
            generation,
            workspace(),
            PublicCategory::Compute,
            MONTH,
            bucket,
            dimension_hash,
        ),
    }
    .expect("the fixture key is inside the grammar");
    put(
        client,
        &key,
        vec![
            (ITEM_TYPE, text(AGGREGATE)),
            ("publicCategory", text(PublicCategory::Compute.id())),
            ("meter", text("compute.millicpu_ms.v1")),
            ("bucketStart", text(bucket)),
            ("bucketEnd", text(bucket)),
            ("quantity", number(quantity)),
            ("factCount", number(1)),
            ("highestSequence", number(quantity.max(1))),
            (
                "dimensions",
                AttributeValue::M(HashMap::from([(
                    "serviceId".to_owned(),
                    text("regional-stream"),
                )])),
            ),
        ],
    )
    .await;
}

/// One bounded hourly read of the fixture workspace's compute rollups.
///
/// The bucket bounds cover exactly the first day of the month, so the
/// exclusive-upper-bound case has a boundary row to be wrong about.
fn hourly_page(
    generation: Generation,
    limit: usize,
    after: Option<String>,
) -> AggregateRequest<'static> {
    AggregateRequest {
        generation,
        workspace: workspace(),
        category: PublicCategory::Compute,
        month: MONTH,
        granularity: Granularity::Hourly,
        from_bucket: "2026-08-01T00",
        until_bucket: "2026-08-02T00",
        limit,
        after,
    }
}

#[tokio::test]
async fn the_generation_pointer_is_absent_before_a_cutover_rather_than_defaulted() {
    let (_engine, client, store) = engine().await;

    assert_eq!(
        store
            .current_generation()
            .await
            .expect("an empty projection still answers"),
        None,
        "an absent pointer must not be read as the first generation: a read \
         against the wrong generation is confidently empty rather than visibly \
         absent"
    );

    put(
        &client,
        &ProjectionKeys.generation_pointer(),
        vec![
            (ITEM_TYPE, text(GENERATION_POINTER)),
            ("generation", number(3)),
        ],
    )
    .await;

    assert_eq!(
        store.current_generation().await.expect("the pointer reads"),
        Some(Generation::new(3).expect("inside the ceiling"))
    );
}

#[tokio::test]
async fn a_page_stops_at_the_row_budget_and_resumes_exactly_where_it_stopped() {
    let (_engine, client, store) = engine().await;
    for hour in 0..5_u32 {
        put_aggregate(
            &client,
            Generation::FIRST,
            Granularity::Hourly,
            &format!("2026-08-01T{hour:02}"),
            "d0",
            u64::from(hour) + 1,
        )
        .await;
    }

    let first = store
        .aggregates(&hourly_page(Generation::FIRST, 2, None))
        .await
        .expect("the first page reads");
    assert_eq!(first.rows.len(), 2);
    let cursor = first
        .next
        .clone()
        .expect("a partition that continued past the budget hands back a cursor");

    let second = store
        .aggregates(&hourly_page(Generation::FIRST, 2, Some(cursor)))
        .await
        .expect("the second page reads");
    assert_eq!(second.rows.len(), 2);

    let third = store
        .aggregates(&hourly_page(
            Generation::FIRST,
            2,
            Some(second.next.clone().expect("still more")),
        ))
        .await
        .expect("the third page reads");
    assert_eq!(third.rows.len(), 1);
    assert!(
        third.next.is_none(),
        "a page that did not fill its budget is the end of the month, and saying \
         otherwise makes a caller loop forever"
    );

    let mut seen: Vec<String> = Vec::new();
    for page in [&first, &second, &third] {
        for row in &page.rows {
            seen.push(row.bucket_start.clone());
        }
    }
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        seen.len(),
        unique.len(),
        "a keyset walk that returns a row twice bills it twice: {seen:?}"
    );
    assert_eq!(
        seen,
        vec![
            "2026-08-01T00",
            "2026-08-01T01",
            "2026-08-01T02",
            "2026-08-01T03",
            "2026-08-01T04",
        ],
        "the walk must be complete and in bucket order"
    );
}

#[tokio::test]
async fn the_upper_bucket_bound_is_exclusive_so_two_adjacent_pages_never_double_count() {
    let (_engine, client, store) = engine().await;
    // The boundary row: it belongs to the *next* day's page, not this one.
    put_aggregate(
        &client,
        Generation::FIRST,
        Granularity::Hourly,
        "2026-08-01T23",
        "d0",
        7,
    )
    .await;
    put_aggregate(
        &client,
        Generation::FIRST,
        Granularity::Hourly,
        "2026-08-02T00",
        "d0",
        11,
    )
    .await;

    let page = store
        .aggregates(&hourly_page(Generation::FIRST, 100, None))
        .await
        .expect("the page reads");
    let buckets: Vec<&str> = page
        .rows
        .iter()
        .map(|row| row.bucket_start.as_str())
        .collect();
    assert_eq!(
        buckets,
        vec!["2026-08-01T23"],
        "`H#2026-08-02T00` sorts after the bare `H#2026-08-02T00` upper bound \
         only because of the dimension segment; if it did not, every day \
         boundary would be counted in both pages"
    );
}

#[tokio::test]
async fn an_hourly_page_never_picks_up_a_daily_rollup_from_the_same_partition() {
    let (_engine, client, store) = engine().await;
    put_aggregate(
        &client,
        Generation::FIRST,
        Granularity::Hourly,
        "2026-08-01T00",
        "d0",
        5,
    )
    .await;
    put_aggregate(
        &client,
        Generation::FIRST,
        Granularity::Daily,
        "2026-08-01",
        "d0",
        500,
    )
    .await;

    let page = store
        .aggregates(&hourly_page(Generation::FIRST, 100, None))
        .await
        .expect("the page reads");
    assert_eq!(page.rows.len(), 1);
    assert_eq!(
        page.rows[0].quantity,
        Quantity::parse("5").expect("a quantity"),
        "the daily rollup of the same hours is in the same partition; folding it \
         into an hourly total doubles the invoice"
    );
}

#[tokio::test]
async fn a_read_pinned_to_one_generation_cannot_observe_a_rebuild_in_another() {
    let (_engine, client, store) = engine().await;
    let rebuilt = Generation::FIRST.next().expect("advances");
    put_aggregate(
        &client,
        Generation::FIRST,
        Granularity::Hourly,
        "2026-08-01T00",
        "d0",
        5,
    )
    .await;
    put_aggregate(
        &client,
        rebuilt,
        Granularity::Hourly,
        "2026-08-01T00",
        "d0",
        9,
    )
    .await;

    let live = store
        .aggregates(&hourly_page(Generation::FIRST, 100, None))
        .await
        .expect("the live page reads");
    assert_eq!(live.rows.len(), 1);
    assert_eq!(live.rows[0].quantity, Quantity::parse("5").expect("parses"));

    let next = store
        .aggregates(&hourly_page(rebuilt, 100, None))
        .await
        .expect("the rebuilt page reads");
    assert_eq!(next.rows.len(), 1);
    assert_eq!(next.rows[0].quantity, Quantity::parse("9").expect("parses"));
}

#[tokio::test]
async fn a_row_of_another_item_type_inside_the_range_is_refused_rather_than_skipped() {
    let (_engine, client, store) = engine().await;
    put_aggregate(
        &client,
        Generation::FIRST,
        Granularity::Hourly,
        "2026-08-01T00",
        "d0",
        5,
    )
    .await;
    // A row that a future writer could place inside the aggregate range. The
    // reader must refuse the page rather than serve a total missing a row.
    put(
        &client,
        &ProjectionKeys
            .hourly(
                Generation::FIRST,
                workspace(),
                PublicCategory::Compute,
                MONTH,
                "2026-08-01T01",
                "d0",
            )
            .expect("the fixture key is inside the grammar"),
        vec![
            (ITEM_TYPE, text("usage_detail")),
            ("publicCategory", text(PublicCategory::Compute.id())),
        ],
    )
    .await;

    let error = store
        .aggregates(&hourly_page(Generation::FIRST, 100, None))
        .await
        .expect_err("a foreign row inside the range");
    assert!(
        matches!(
            error,
            QueryError::ItemTypeMismatch {
                expected: AGGREGATE,
                ..
            }
        ),
        "{error}"
    );
}

#[tokio::test]
async fn a_quarantined_coverage_row_reports_the_stall_a_page_alone_cannot() {
    let (_engine, client, store) = engine().await;
    let key = ProjectionKeys
        .coverage(Generation::FIRST, workspace(), PublicCategory::Compute)
        .expect("the coverage key builds");
    put(
        &client,
        &key,
        vec![
            (ITEM_TYPE, text(COVERAGE)),
            ("publicCategory", text(PublicCategory::Compute.id())),
            ("acceptedSequence", number(42)),
            ("projectedSequence", number(17)),
            ("publishedSequence", number(17)),
            ("settledSequence", number(9)),
            ("state", text("quarantined")),
            ("quarantineAt", number(18)),
            ("quarantineReason", text(PoisonReason::Undecodable.id())),
        ],
    )
    .await;

    let coverage = store
        .coverage(Generation::FIRST, workspace(), PublicCategory::Compute)
        .await
        .expect("the coverage read succeeds")
        .expect("the row exists");
    assert!(
        coverage.is_stalled(),
        "an empty page plus an advancing frontier says `you used nothing`; the \
         same page plus a parked frontier says `we have not folded your facts`"
    );
    assert!(matches!(
        coverage.state,
        FrontierState::Quarantined {
            reason: PoisonReason::Undecodable,
            ..
        }
    ));
}

#[tokio::test]
async fn a_table_the_composition_does_not_hold_is_misconfigured_rather_than_empty() {
    let (engine, _client, _store) = engine().await;
    let absent = UsageQueryStore::new(client(&engine), "dev-eu-west-1-usage-query-projection-typo");

    let error = absent
        .aggregates(&hourly_page(Generation::FIRST, 10, None))
        .await
        .expect_err("a table the service does not hold");
    assert!(
        matches!(error, QueryError::Misconfigured { .. }),
        "a misspelled table must never answer `no usage`: {error}"
    );
}
