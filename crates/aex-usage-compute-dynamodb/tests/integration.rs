//! The queue halves of this authority against `moto`'s SQS.
//!
//! What this proves, and could not be proved without a service: that an
//! [`OutboxMessage`] built from an admitted fact is *accepted by a real FIFO
//! queue* — the group id, the dedupe id and both message attributes are inside
//! the grammar SQS enforces, which no in-memory double checks; that the body
//! that comes back off the wire is byte-for-byte the one the producer
//! serialised, which is the defect the single `RatingRequest` declaration
//! exists to prevent; that a queue the service does not hold is a typed refusal
//! carrying the service's own answer rather than a swallowed success; and that
//! a settlement receipt sent the way `usage-receipt-dispatcher` sends it
//! survives a real queue and decodes into this category's
//! [`SettlementReceipt`].
//!
//! What it cannot prove, stated rather than assumed:
//!
//! - **Which** `PortError` a missing queue maps to. The adapter discriminates
//!   on the error *code*, and the code's spelling is a wire-protocol property:
//!   SQS's JSON protocol names the shape `QueueDoesNotExist` while the legacy
//!   query protocol answers `AWS.SimpleQueueService.NonExistentQueue`. An
//!   emulator is not authority over which one a real endpoint serves, so this
//!   suite asserts the port and the service's own reason and leaves the
//!   `NotFound`-versus-`Unavailable` split to the live companion.
//! - The FIFO deduplication window, visibility-timeout and redrive timing, and
//!   DLQ behaviour at scale. `release/policy/test-images.toml` records all four
//!   as `cannot_prove` for this engine, and `aws.sqs.fifo` therefore stays
//!   `requires_live`. A retry collapsing onto one settlement is a *money*
//!   guarantee and is not evidenced here.
//! - The `DynamoDB` half of this adapter. The `DynamoDB` Local suite — concurrent
//!   producers on one workspace, the closure-vector write, four discriminated
//!   fact shapes round-tripping — is still unwritten and still owed by the
//!   `observations-usage` stream; the expression and codec halves are covered at
//!   the unit layer. This file replaces that deferral note, not the suite it
//!   described.
//!
//! [`SettlementReceipt`]: aex_usage_app::ports::SettlementReceipt

use aex_test_harness::MotoContainer;
use aex_usage_app::outbox::OutboxMessage;
use aex_usage_app::ports::{PortError, RatingQueue as _};
use aex_usage_compute_dynamodb::CATEGORY;
use aex_usage_compute_dynamodb::queue::{
    BUSINESS_KEY_ATTRIBUTE, PRICING_VERSION_ATTRIBUTE, SettlementQueue,
};
use aex_usage_compute_dynamodb::stream::{ReceiptEnvelope, receipt_records};
use aex_usage_domain::fact::{
    Attribution, FactDraft, FactKind, ResourceGeneration, ResourceKind, SCHEMA_VERSION, UsageFact,
};
use aex_usage_domain::frontier::AcceptedSequence;
use aex_usage_domain::identity::{AuthorityId, AuthorityKey, AuthorityKind, SegmentOrdinal};
use aex_usage_domain::measurement::{
    Evidence, FactBasis, Measurement, ReceiptKind, ReservationClass, ServiceTime, SourceReceipt,
};
use aex_usage_domain::meter::Meter;
use aex_usage_domain::wire_pending::{
    OrganizationId, PricingVersion, RegionId, ServiceId, Timestamp, WorkspaceId,
};
use aex_wire::ids::PrefixedId as _;
use aws_sdk_sqs::Client;
use aws_sdk_sqs::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_sqs::types::{Message, QueueAttributeName};

/// The central settlement queue, named the way the plane names it.
const RATING_QUEUE: &str = "aex-integration-usage-rating.fifo";
/// A queue name nothing ever creates.
const ABSENT_QUEUE: &str = "aex-integration-usage-rating-absent.fifo";
/// This category's settlement receipt queue.
const RECEIPT_QUEUE: &str = "aex-integration-usage-receipt-compute.fifo";
/// The region every fixture is measured in.
const REGION: &str = "eu-west-1";

/// How many receives a delivered message has to appear within.
///
/// A bound rather than a sleep: a queue that never delivers fails as a queue
/// that never delivered, and the assertion is never made against a queue that
/// simply had not answered yet.
const RECEIVE_ATTEMPTS: usize = 20;
/// How long one receive waits before answering empty.
const RECEIVE_WAIT_SECONDS: i32 = 1;

fn client(engine: &MotoContainer) -> Client {
    let config = aws_sdk_sqs::Config::builder()
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

/// Creates one FIFO queue and returns the URL the adapter is bound to.
async fn fifo_queue(client: &Client, name: &str) -> String {
    client
        .create_queue()
        .queue_name(name)
        .attributes(QueueAttributeName::FifoQueue, "true")
        .send()
        .await
        .expect("the FIFO queue is created")
        .queue_url
        .expect("a created queue carries its URL")
}

/// Receives one message, polling until the queue yields it.
async fn receive_one(client: &Client, queue_url: &str) -> Message {
    for _ in 0..RECEIVE_ATTEMPTS {
        let answer = client
            .receive_message()
            .queue_url(queue_url)
            .max_number_of_messages(1)
            .message_attribute_names("All")
            .wait_time_seconds(RECEIVE_WAIT_SECONDS)
            .send()
            .await
            .expect("the queue answers a receive");
        if let Some(message) = answer.messages.unwrap_or_default().into_iter().next() {
            return message;
        }
    }
    panic!("the queue delivered nothing across {RECEIVE_ATTEMPTS} receives");
}

/// One string message attribute of a delivered message.
fn attribute<'a>(message: &'a Message, name: &str) -> &'a str {
    let value = message
        .message_attributes
        .as_ref()
        .and_then(|attributes| attributes.get(name))
        .and_then(aws_sdk_sqs::types::MessageAttributeValue::string_value);
    let Some(value) = value else {
        panic!("the delivered message carries no `{name}` attribute");
    };
    value
}

/// A canonical instant from whole milliseconds.
fn at(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(millis).expect("representable")
}

/// The fixture organization, minted from the wire owner rather than hand-typed.
///
/// The outbox boundary parses this string into the central contract's
/// identifier type, so a hand-written spelling would keep passing after the
/// format moved — and the FIFO group id is this string.
fn organization() -> OrganizationId {
    let wire = aex_wire::ids::OrganizationId::from_uuid7(aex_wire::ids::Uuid7::compose(1, [7; 10]));
    OrganizationId::parse(wire.encode().as_str()).expect("organization")
}

/// The fixture workspace, in the same canonical spelling.
fn workspace() -> WorkspaceId {
    let wire = aex_wire::ids::WorkspaceId::from_uuid7(aex_wire::ids::Uuid7::compose(2, [9; 10]));
    WorkspaceId::parse(wire.encode().as_str()).expect("workspace")
}

fn region() -> RegionId {
    RegionId::parse(REGION).expect("region")
}

/// A measurement with the evidence shape this authority carries.
fn measurement() -> Measurement {
    Measurement::new(
        Meter::MemoryByteMs,
        FactBasis::Reserved,
        ServiceTime::Interval {
            start: at(0),
            end: at(250),
        },
        SourceReceipt {
            kind: ReceiptKind::ReservationToken,
            id: Box::from("res-1"),
            digest: None,
        },
        Evidence::Reservation {
            class: ReservationClass::Context,
            bytes: 4_096,
            held_ms: 250,
        },
    )
    .expect("a fixture measurement is valid for its own meter")
}

/// An admitted fact of this authority at `sequence`.
fn fact(sequence: u64) -> UsageFact {
    FactDraft {
        schema_version: SCHEMA_VERSION,
        organization: organization(),
        workspace: workspace(),
        region: region(),
        attribution: Attribution::default(),
        service: ServiceId::parse("regional-stream").expect("service"),
        resource: ResourceGeneration {
            kind: ResourceKind::MuxTask,
            generation: Box::from("task-1"),
        },
        authority: AuthorityKey {
            region: region(),
            category: CATEGORY,
            kind: AuthorityKind::MemoryReservation,
            authority_id: AuthorityId::parse(&format!("a-{sequence}")).expect("authority"),
            segment_ordinal: SegmentOrdinal::FIRST,
        },
        pricing_version: PricingVersion::parse("synthetic-zero-v1").expect("rate book"),
        reservation: None,
        kind: FactKind::Measured(measurement()),
    }
    .admit(
        AcceptedSequence::new(sequence).expect("a sequence is one based"),
        at(600_000),
    )
    .expect("a fixture draft is admissible")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_published_fact_crosses_a_real_fifo_queue_with_its_body_and_both_attributes() {
    let engine = MotoContainer::start().await.expect("moto starts");
    let client = client(&engine);
    let url = fifo_queue(&client, RATING_QUEUE).await;

    let message = OutboxMessage::for_fact(&fact(1)).expect("an admitted fact converts");
    // The send itself is an assertion: a FIFO queue refuses a message with no
    // group id, and refuses a group or dedupe id outside its own grammar. Both
    // identities are derived from the fact, so this is the first place anything
    // checks that a derived identity is one SQS will actually take.
    SettlementQueue::new(client.clone(), url.as_str())
        .publish(&message)
        .await
        .expect("the queue accepts the message the outbox built");

    let delivered = receive_one(&client, &url).await;
    let body: serde_json::Value =
        serde_json::from_str(delivered.body().expect("a delivered message has a body"))
            .expect("the body that crossed the wire is JSON");
    assert_eq!(
        body,
        serde_json::to_value(&message.body).expect("the rating request serialises"),
        "the settlement worker decodes through the producer's own declaration; a \
         body the wire altered is a message central cannot rate"
    );
    assert_eq!(
        attribute(&delivered, BUSINESS_KEY_ATTRIBUTE),
        message.business_key,
        "central posts the journal transaction under this key"
    );
    assert_eq!(
        attribute(&delivered, PRICING_VERSION_ATTRIBUTE),
        message.pricing_version,
        "the rate book is pinned at admission and must arrive pinned"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_publish_to_a_queue_the_service_does_not_hold_carries_the_services_own_refusal() {
    let engine = MotoContainer::start().await.expect("moto starts");
    let client = client(&engine);
    // Derived from a created queue's URL so the host and account segments are
    // the engine's own and only the queue name is wrong.
    let present = fifo_queue(&client, RATING_QUEUE).await;
    let absent = present.replace(RATING_QUEUE, ABSENT_QUEUE);
    assert_ne!(
        absent, present,
        "the absent URL must differ from the created"
    );

    let message = OutboxMessage::for_fact(&fact(2)).expect("an admitted fact converts");
    let error = SettlementQueue::new(client, absent.as_str())
        .publish(&message)
        .await
        .expect_err("a queue the service does not hold");

    let (what, reason) = match &error {
        PortError::NotFound { what, id } => (*what, id.as_str()),
        PortError::Unavailable { what, reason } => (*what, reason.as_str()),
        other => panic!(
            "a misconfigured queue binding must never be reported as {other:?}: a \
             conflict or a corrupt body would send the caller down the wrong path"
        ),
    };
    assert_eq!(
        what, "usage rating queue",
        "the failure names the port that failed, not the SDK"
    );
    assert!(
        reason.contains("does not exist"),
        "the adapter reported `{reason}`, which is not the service's own answer; \
         the transport branch answers `central is unreachable`, so this means the \
         service error never reached the classification at all"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_settlement_receipt_delivered_by_a_real_queue_decodes_into_this_authoritys_receipt() {
    let engine = MotoContainer::start().await.expect("moto starts");
    let client = client(&engine);
    let url = fifo_queue(&client, RECEIPT_QUEUE).await;

    let settled = fact(3);
    let envelope = ReceiptEnvelope {
        receipt_id: "rcpt-1".to_owned(),
        region: REGION.to_owned(),
        category: CATEGORY.id().to_owned(),
        fact_id: settled.fact_id.to_string(),
        // Negative: a reversal has to survive the wire as a reversal.
        rated_microusd: -25,
        transaction_id: "txn-1".to_owned(),
        pricing_version: "synthetic-zero-v1".to_owned(),
        settled_at: at(700_000).to_canonical(),
        workspace_id: Some(workspace().to_string()),
        accepted_sequence: Some(settled.accepted_sequence.get()),
    };
    // Exactly what `usage-receipt-dispatcher` sends: the region is the FIFO
    // group and central's receipt identity is the dedupe id.
    client
        .send_message()
        .queue_url(&url)
        .message_body(serde_json::to_string(&envelope).expect("the envelope serialises"))
        .message_group_id(REGION)
        .message_deduplication_id(&envelope.receipt_id)
        .send()
        .await
        .expect("central's receipt lands on the queue");

    let delivered = receive_one(&client, &url).await;
    let identifier = delivered
        .message_id()
        .expect("a delivered message has an id")
        .to_owned();
    let event = serde_json::json!({
        "Records": [{
            "messageId": identifier,
            "eventSource": "aws:sqs",
            "body": delivered.body().expect("a delivered message has a body"),
        }]
    });

    let records = receipt_records(&event).expect("the delivered batch decodes");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].identifier, identifier);
    assert_eq!(
        records[0].receipt.category, CATEGORY,
        "a receipt this worker settles must belong to the authority it owns"
    );
    assert_eq!(records[0].receipt.fact_id, settled.fact_id);
    assert_eq!(
        records[0].receipt.rated_microusd, -25,
        "a reversal that arrives as a charge is a double settlement"
    );
    assert_eq!(
        records[0].receipt.accepted_sequence,
        AcceptedSequence::new(settled.accepted_sequence.get()).ok()
    );
}
