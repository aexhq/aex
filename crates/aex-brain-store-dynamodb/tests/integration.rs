//! Engine-backed cases for the wake queue, against `moto`'s SQS and its due
//! backstop against `DynamoDB` Local.
//!
//! The scripted suite in `adapters.rs` asserts the bytes this adapter *would*
//! send. These cases assert what comes back. A wake is published onto a real
//! queue and read out of it, so the receipt handle, the
//! `ApproximateReceiveCount` system attribute and the decode are the service's
//! own values rather than a hand-built [`aws_sdk_sqs::types::Message`]: an
//! acknowledgement really removes one message and not another, a redelivery
//! really carries a higher count, a body that is not a projection still carries
//! a usable receipt, and the queue hint and the durable due index agree about
//! the row they both describe. The due index is created from the plane's own
//! migration document, `INCLUDE` projection included, so the scan is held to
//! the twelve attributes a plane will actually return it.
//!
//! What these cases cannot prove is the larger half. `moto` is an emulator:
//! real visibility and redrive timing, dead-letter behaviour, the FIFO
//! deduplication window and IAM evaluation are all live concerns, and
//! `release/policy/test-images.toml` records them as such. The receive count
//! here is exact bookkeeping inside one process, while SQS keeps an
//! *approximate*, per-host count, so a poison threshold that looks safe here is
//! not thereby safe in a plane.
//!
//! One gap is sharp enough to name. `moto` returns `ApproximateReceiveCount`
//! whether or not a receive asked for it (getmoto/moto#3607), so these cases
//! prove the adapter *reads* the count the service kept and cannot prove it
//! *requested* it. A real queue returns no system attribute unless the receive
//! names one, and `decode_message` then falls back to a count of one - silently,
//! for every delivery, which retires the poison policy rather than failing. That
//! request field is asserted nowhere: no suite in this crate exercises
//! `receive` against a captured request.
//!
//! Neither engine has streams or pipes, so nothing
//! here proves the projection itself: in a plane the body is written by the
//! `agent-wake` `EventBridge` pipe from a `NEW_IMAGE` stream record, and these
//! cases write the body that pipe is configured to write. An input-template
//! drift stays invisible here. `DynamoDB` Local also updates a secondary index
//! within the write that changed it, so the due backstop's behaviour under real
//! index lag is untested.

use core::time::Duration;

use aex_brain_app::ports::{
    MalformedWakeReason, WakeBatch, WakeDelivery, WakeOrigin, WakeQueue, WakeState,
};
use aex_brain_domain::ids::{AgentKey, Timestamp, WorkShard};
use aex_brain_store_dynamodb::wake::MAX_BATCH;
use aex_brain_store_dynamodb::{DueScan, SqsWakeQueue};
use aex_test_harness::{DynamoDbLocalContainer, MotoContainer};
use aex_wire::ids::{PrefixedId, Uuid7};
use aex_work_dynamodb::codec::{DeliveryEvidence, Payload, WorkRecord, encode_work};
use aws_sdk_dynamodb::types::{
    AttributeDefinition, BillingMode, GlobalSecondaryIndex, KeySchemaElement, KeyType, Projection,
    ProjectionType, ScalarAttributeType,
};
use aws_sdk_sqs::types::QueueAttributeName;

/// The `regional-work` table this suite creates.
const TABLE: &str = "aex-integration-regional-work";

/// The wake queue this suite creates.
const QUEUE: &str = "aex-integration-brain-wake";

/// The instant every fixture row is due at: 2026-01-01T00:00:00Z.
const DUE_MILLIS: i64 = 1_767_225_600_000;

/// The priority band every fixture row carries.
///
/// Band two takes a thirty-second lead off the due time, so a row whose index
/// position is computed from the priority sorts somewhere the raw due time does
/// not. A band-zero fixture would let a lost lead pass unnoticed.
const PRIORITY: u8 = 2;

/// One long poll.
///
/// Short on purpose: every wait in this file sits inside a bounded loop that
/// stops on evidence, so the poll length is a granularity rather than a
/// deadline.
const POLL: Duration = Duration::from_secs(1);

/// How many bounded receives a case gives the queue before it fails.
const POLLS: usize = 8;

/// The visibility a case gives a delivery when it needs the queue to hand the
/// message back on its own.
const SHORT_VISIBILITY: Duration = Duration::from_secs(3);

/// The visibility a case gives a delivery when only an explicit release may
/// bring it back inside the case's window.
const LONG_VISIBILITY: Duration = Duration::from_secs(30);

/// The deployed `regional-work` table, as the plane declares it.
const REGIONAL_WORK: &str = include_str!("../../../migrations/regional/tables/regional-work.json");

fn sqs_client(engine: &MotoContainer) -> aws_sdk_sqs::Client {
    let config = aws_sdk_sqs::Config::builder()
        .behavior_version_latest()
        .region(aws_sdk_sqs::config::Region::new(engine.region()))
        .endpoint_url(engine.endpoint_url())
        .credentials_provider(aws_sdk_sqs::config::Credentials::new(
            engine.access_key_id(),
            engine.secret_access_key(),
            None,
            None,
            "aex-integration",
        ))
        .build();
    aws_sdk_sqs::Client::from_conf(config)
}

fn dynamo_client(engine: &DynamoDbLocalContainer) -> aws_sdk_dynamodb::Client {
    let config = aws_sdk_dynamodb::Config::builder()
        .behavior_version_latest()
        .region(aws_sdk_dynamodb::config::Region::new(engine.region()))
        .endpoint_url(engine.endpoint_url())
        .credentials_provider(aws_sdk_dynamodb::config::Credentials::new(
            engine.access_key_id(),
            engine.secret_access_key(),
            None,
            None,
            "aex-integration",
        ))
        .build();
    aws_sdk_dynamodb::Client::from_conf(config)
}

/// A string field of the migration document.
fn text<'a>(value: &'a serde_json::Value, field: &str) -> &'a str {
    value[field]
        .as_str()
        .unwrap_or_else(|| panic!("`{field}` is a string in the migration document"))
}

/// The two-element key schema `partition` and `sort` name.
fn key_schema(partition: &str, sort: &str) -> Vec<KeySchemaElement> {
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
}

/// Creates `regional-work` exactly as the plane's migration document declares
/// it, index projection included.
///
/// Restating the shape here would let the due scan pass against attributes the
/// deployed `gsi_due` never returns: that index projects twelve named
/// attributes rather than the row, so a scan that read a thirteenth would work
/// against a locally-widened index and fail in a plane. The index name and its
/// two key attributes are asserted against the adapter's own constants, because
/// a drift between the document and the code is otherwise a query that quietly
/// finds nothing.
async fn create_regional_work(client: &aws_sdk_dynamodb::Client) {
    let document: serde_json::Value =
        serde_json::from_str(REGIONAL_WORK).expect("the migration document is JSON");
    let index = &document["globalSecondaryIndexes"][0];
    assert_eq!(text(index, "name"), aex_work_dynamodb::keys::DUE_INDEX);
    assert_eq!(text(index, "partition"), aex_work_dynamodb::keys::DUE_PK);
    assert_eq!(text(index, "sort"), aex_work_dynamodb::keys::DUE_SK);

    let attributes: Vec<AttributeDefinition> = document["attributes"]
        .as_array()
        .expect("the document declares its key attributes")
        .iter()
        .map(|attribute| {
            AttributeDefinition::builder()
                .attribute_name(text(attribute, "name"))
                .attribute_type(ScalarAttributeType::from(text(attribute, "type")))
                .build()
                .expect("a complete attribute definition")
        })
        .collect();
    let projected: Vec<String> = index["projection"]["attributes"]
        .as_array()
        .expect("an INCLUDE projection names the attributes it carries")
        .iter()
        .map(|name| {
            name.as_str()
                .expect("a projected attribute is a string")
                .to_owned()
        })
        .collect();
    let due = GlobalSecondaryIndex::builder()
        .index_name(text(index, "name"))
        .set_key_schema(Some(key_schema(
            text(index, "partition"),
            text(index, "sort"),
        )))
        .projection(
            Projection::builder()
                .projection_type(ProjectionType::from(text(&index["projection"], "type")))
                .set_non_key_attributes(Some(projected))
                .build(),
        )
        .build()
        .expect("a complete due index");

    client
        .create_table()
        .table_name(TABLE)
        .set_attribute_definitions(Some(attributes))
        .set_key_schema(Some(key_schema(
            text(&document["keySchema"], "partition"),
            text(&document["keySchema"], "sort"),
        )))
        .global_secondary_indexes(due)
        .billing_mode(BillingMode::PayPerRequest)
        .send()
        .await
        .expect("the regional-work table is created");
}

fn session() -> aex_wire::ids::SessionId {
    aex_wire::ids::SessionId::from_uuid7(Uuid7::compose(1_767_225_600_000, [1; 10]))
}

fn agent() -> aex_wire::ids::AgentId {
    aex_wire::ids::AgentId::from_uuid7(Uuid7::compose(1_767_225_600_001, [2; 10]))
}

fn workspace() -> aex_wire::ids::WorkspaceId {
    aex_wire::ids::WorkspaceId::from_uuid7(Uuid7::compose(1_767_225_600_002, [3; 10]))
}

fn organization() -> aex_wire::ids::OrganizationId {
    aex_wire::ids::OrganizationId::from_uuid7(Uuid7::compose(1_767_225_600_003, [4; 10]))
}

/// The agent every fixture names, in the Brain's own vocabulary.
fn agent_key() -> AgentKey {
    AgentKey::new(
        aex_brain_store_dynamodb::translate::session_from_wire(session()),
        aex_brain_store_dynamodb::translate::agent_from_wire(agent()),
    )
}

fn due_at() -> aex_wire::types::Timestamp {
    aex_wire::types::Timestamp::from_unix_millis(DUE_MILLIS)
        .expect("the fixture instant is inside the wire range")
}

/// One `regional-work` agent wake, in the state given.
fn wake_row(work_id: &str, state: &str) -> WorkRecord {
    WorkRecord {
        work_id: work_id.to_owned(),
        workspace: workspace(),
        organization: organization(),
        session: Some(session()),
        agent: Some(agent()),
        kind: "agent.wake".to_owned(),
        priority: PRIORITY,
        due_at: due_at(),
        state: state.to_owned(),
        attempt: 0,
        max_attempts: 5,
        fence: 1,
        claim_owner: None,
        lease_expires_at: None,
        dedupe_key: work_id.to_owned(),
        payload: Payload::new()
            .set("sessionId", session().to_string())
            .set("agentId", agent().to_string()),
        delivery: DeliveryEvidence {
            publish_count: 1,
            last_published_at: Some(due_at()),
            last_receipt_id: None,
        },
        created_at: due_at(),
        updated_at: due_at(),
    }
}

/// The body the `agent-wake` pipe projects from one row.
///
/// Derived from the row rather than written beside it, so the two halves of the
/// verification read - the hint and the authoritative row - cannot drift apart
/// inside a case and hide a real mismatch.
fn projection(record: &WorkRecord) -> String {
    serde_json::json!({
        "workId": record.work_id,
        "workspaceId": record.workspace.to_string(),
        "dueAt": record.due_at.to_wire(),
        "priority": record.priority,
        "payload": {
            "sessionId": record.session.expect("an agent wake names its session").to_string(),
            "agentId": record.agent.expect("an agent wake names its agent").to_string(),
        }
    })
    .to_string()
}

/// The receipt and receive count one queue delivery carries.
fn queue_origin(delivery: &WakeDelivery) -> (String, u32) {
    match &delivery.origin {
        WakeOrigin::Queue {
            receipt,
            receive_count,
        } => (receipt.clone(), *receive_count),
        WakeOrigin::DueScan => panic!("a queue receive produced a due-scan origin"),
    }
}

/// Both engines, the table, the queue and the adapter bound to them.
struct Engines {
    _moto: MotoContainer,
    _dynamo: DynamoDbLocalContainer,
    dynamo: aws_sdk_dynamodb::Client,
    sqs: aws_sdk_sqs::Client,
    queue_url: String,
    queue: SqsWakeQueue,
}

impl Engines {
    /// Starts both engines, creates the table and the queue, and binds the
    /// adapter to them.
    ///
    /// The two starts are concurrent because neither depends on the other; the
    /// containers die with the returned value.
    async fn start(visibility: Duration) -> Self {
        let (moto, dynamo) = tokio::join!(MotoContainer::start(), DynamoDbLocalContainer::start());
        let moto = moto.expect("moto starts");
        let dynamo_engine = dynamo.expect("DynamoDB Local starts");
        let sqs = sqs_client(&moto);
        let dynamo = dynamo_client(&dynamo_engine);
        create_regional_work(&dynamo).await;

        let queue_url = sqs
            .create_queue()
            .queue_name(QUEUE)
            .attributes(
                QueueAttributeName::VisibilityTimeout,
                visibility.as_secs().to_string(),
            )
            .send()
            .await
            .expect("the wake queue is created")
            .queue_url
            .expect("a created queue has a URL");

        let queue = SqsWakeQueue::new(
            sqs.clone(),
            queue_url.clone(),
            DueScan::new(dynamo.clone(), TABLE.to_owned()),
        );
        Self {
            _moto: moto,
            _dynamo: dynamo_engine,
            dynamo,
            sqs,
            queue_url,
            queue,
        }
    }

    /// Publishes one body the way the pipe would.
    async fn publish(&self, body: String) {
        self.sqs
            .send_message()
            .queue_url(&self.queue_url)
            .message_body(body)
            .send()
            .await
            .expect("the projection is published");
    }

    /// Writes one `regional-work` row.
    async fn write(&self, record: &WorkRecord) {
        self.dynamo
            .put_item()
            .table_name(TABLE)
            .set_item(Some(encode_work(record).expect("the fixture row encodes")))
            .send()
            .await
            .expect("the row is written");
    }
}

/// Receives until the wanted counts have been seen, or fails.
///
/// A bounded loop rather than one receive: SQS answers from whichever hosts it
/// polled, so an empty or partial batch is an ordinary answer and never
/// evidence that nothing was published. A record seen twice inside the window
/// is one record redelivered, so it replaces its earlier sighting - keeping the
/// fresher receipt - instead of doubling the count.
async fn drain(queue: &SqsWakeQueue, deliveries: usize, malformed: usize) -> WakeBatch {
    let mut collected = WakeBatch::default();
    for _ in 0..POLLS {
        let batch = queue
            .receive(MAX_BATCH, POLL)
            .await
            .expect("the receive succeeds");
        for delivery in batch.deliveries {
            match collected
                .deliveries
                .iter_mut()
                .find(|seen| seen.wake.work_id == delivery.wake.work_id)
            {
                Some(seen) => *seen = delivery,
                None => collected.deliveries.push(delivery),
            }
        }
        for record in batch.malformed {
            match collected
                .malformed
                .iter_mut()
                .find(|seen| seen.fingerprint == record.fingerprint)
            {
                Some(seen) => *seen = record,
                None => collected.malformed.push(record),
            }
        }
        if collected.deliveries.len() >= deliveries && collected.malformed.len() >= malformed {
            return collected;
        }
    }
    panic!(
        "the queue delivered {} of {deliveries} valid and {} of {malformed} malformed records within {POLLS} bounded receives",
        collected.deliveries.len(),
        collected.malformed.len()
    )
}

/// Watches the queue until `control` is delivered again, and returns every
/// identity seen on the way there.
///
/// The control is the whole shape of the assertion. Absence is not evidence
/// unless something else is present: a queue that answers nothing at all would
/// otherwise "prove" that an acknowledgement removed a message.
async fn watch_until(queue: &SqsWakeQueue, control: &str) -> Vec<String> {
    let mut seen = Vec::new();
    for _ in 0..POLLS {
        let batch = queue
            .receive(MAX_BATCH, POLL)
            .await
            .expect("the receive succeeds");
        for delivery in batch.deliveries {
            seen.push(delivery.wake.work_id);
        }
        for record in batch.malformed {
            seen.push(format!("malformed:{}", record.fingerprint));
        }
        if seen.iter().any(|identity| identity == control) {
            return seen;
        }
    }
    panic!(
        "the control record never came back within {POLLS} bounded receives, so the absence of anything else proves nothing: {seen:?}"
    )
}

#[tokio::test]
async fn a_projected_wake_survives_a_real_queue_and_an_acknowledged_one_never_comes_back() {
    let engines = Engines::start(SHORT_VISIBILITY).await;
    // The readiness probe is a signed round trip in a plane, so it is one here.
    engines
        .queue
        .probe()
        .await
        .expect("the queue answers the readiness probe");

    let acknowledged = wake_row("wrk_acknowledged", "pending");
    let control = wake_row("wrk_control", "pending");
    engines.publish(projection(&acknowledged)).await;
    engines.publish(projection(&control)).await;

    let batch = drain(&engines.queue, 2, 0).await;
    assert!(
        batch.malformed.is_empty(),
        "a well-formed projection is not a malformed record"
    );
    let delivery = batch
        .deliveries
        .iter()
        .find(|delivery| delivery.wake.work_id == acknowledged.work_id)
        .expect("the published wake is delivered")
        .clone();

    let (receipt, count) = queue_origin(&delivery);
    assert!(
        !receipt.is_empty(),
        "the receipt is the service's own, not the adapter's"
    );
    assert_eq!(count, 1, "a first delivery has been received once");
    assert_eq!(delivery.wake.key, agent_key());
    assert_eq!(delivery.wake.priority, acknowledged.priority);
    assert_eq!(delivery.wake.tenant, acknowledged.workspace.to_string());
    assert_eq!(delivery.wake.due, Some(Timestamp::from_millis(DUE_MILLIS)));

    engines
        .queue
        .ack(delivery)
        .await
        .expect("the delivery is acknowledged");

    let seen = watch_until(&engines.queue, &control.work_id).await;
    assert!(
        !seen.contains(&acknowledged.work_id),
        "an acknowledged delivery came back while its sibling was still arriving: {seen:?}"
    );
}

/// The poison policy is a function of the receive count, so the count has to be
/// the service's rather than a default the adapter fell back to.
#[tokio::test]
async fn a_released_delivery_returns_carrying_the_receive_count_the_service_kept() {
    let engines = Engines::start(LONG_VISIBILITY).await;
    let row = wake_row("wrk_released", "pending");
    engines.publish(projection(&row)).await;

    let delivery = drain(&engines.queue, 1, 0)
        .await
        .deliveries
        .pop()
        .expect("one delivery");
    let (first_receipt, first_count) = queue_origin(&delivery);
    assert_eq!(first_count, 1);
    let wake = delivery.wake.clone();

    engines
        .queue
        .release(delivery, Duration::ZERO)
        .await
        .expect("the delivery is released");

    // Only the release can bring this back inside the window: the queue would
    // otherwise hold it invisible for thirty seconds.
    let redelivered = drain(&engines.queue, 1, 0)
        .await
        .deliveries
        .pop()
        .expect("one redelivery");
    let (second_receipt, second_count) = queue_origin(&redelivered);
    assert_eq!(
        second_count, 2,
        "a second delivery of one message is the service's count, not the adapter's fallback"
    );
    assert_ne!(
        second_receipt, first_receipt,
        "each delivery of one message carries its own receipt"
    );
    assert_eq!(
        redelivered.wake, wake,
        "two deliveries of one row are one wake, and collapse before admission"
    );
}

#[tokio::test]
async fn a_record_that_is_not_a_projection_is_isolated_and_poisoned_by_its_own_receipt() {
    let engines = Engines::start(SHORT_VISIBILITY).await;
    let control = wake_row("wrk_control", "pending");
    engines.publish("{not a wake projection}".to_owned()).await;
    engines.publish(projection(&control)).await;

    let batch = drain(&engines.queue, 1, 1).await;
    assert_eq!(
        batch.deliveries.len(),
        1,
        "a malformed sibling does not discard a valid delivery"
    );
    assert_eq!(batch.deliveries[0].wake.work_id, control.work_id);

    let record = batch
        .malformed
        .into_iter()
        .next()
        .expect("one malformed record");
    assert_eq!(record.reason, MalformedWakeReason::InvalidProjection);
    assert_eq!(record.receive_count, 1);
    assert_eq!(record.fingerprint.len(), 16);
    assert!(
        record
            .receipt
            .as_deref()
            .is_some_and(|receipt| !receipt.is_empty()),
        "a body the adapter cannot read still arrives with a receipt it can act on"
    );

    engines
        .queue
        .ack_malformed(record)
        .await
        .expect("the malformed record is poisoned");

    let seen = watch_until(&engines.queue, &control.work_id).await;
    assert!(
        !seen
            .iter()
            .any(|identity| identity.starts_with("malformed:")),
        "a poisoned record came back while its valid sibling was still arriving: {seen:?}"
    );
}

/// The queue is a hint and the durable row is the authority, so the two have to
/// agree about the same work - and a retired row has to leave both.
#[tokio::test]
async fn the_due_backstop_and_the_delivery_hint_agree_about_the_row_they_both_read() {
    let engines = Engines::start(LONG_VISIBILITY).await;
    let row = wake_row("wrk_backstop", "pending");
    engines.write(&row).await;
    engines.publish(projection(&row)).await;

    let delivery = drain(&engines.queue, 1, 0)
        .await
        .deliveries
        .pop()
        .expect("one delivery");
    assert_eq!(
        engines
            .queue
            .state(&delivery.wake)
            .await
            .expect("the authoritative row is read"),
        WakeState::Pending
    );

    let shard = WorkShard(aex_work_dynamodb::keys::shard_of(&row.work_id));
    let now = Timestamp::from_millis(DUE_MILLIS + 1);
    let page = engines
        .queue
        .due_scan(shard, now, 10, None)
        .await
        .expect("the due shard is scanned");
    assert_eq!(page.malformed, 0);
    assert!(page.isolations.is_empty());
    assert!(page.next.is_none(), "one row does not need a continuation");
    assert_eq!(
        page.wakes,
        vec![delivery.wake.clone()],
        "the index recovery and the queue hint are one wake, read through a projection that carries neither the payload nor the row body"
    );

    // The row reaches a terminal state; the sparse index attributes go with it.
    engines.write(&wake_row(&row.work_id, "done")).await;
    assert_eq!(
        engines
            .queue
            .state(&delivery.wake)
            .await
            .expect("the authoritative row is read"),
        WakeState::Retired,
        "a hint for retired work must never admit an activation"
    );
    assert!(
        engines
            .queue
            .due_scan(shard, now, 10, None)
            .await
            .expect("the due shard is scanned")
            .wakes
            .is_empty(),
        "a retired row must fall out of the due index rather than be re-woken by it"
    );
}
