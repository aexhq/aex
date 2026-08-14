//! The producer contract this worker consumes, asserted against the peer that
//! publishes it — including one round trip through the regional producer's
//! exact bytes.

use aex_finance_app::use_cases::FifoRatingMessage;
use aex_internal_contracts::usage::{
    Attribution, AuthorityKind, FactAuthority, FactBasis, FactId, FactIdempotency, Meter,
    ModelTokenClass, ModelUsageFact, ModelUsageFactId, ModelUsageRatingRequest, RatingMessage,
    ServiceTime, SourceReceipt, UsageFact,
};
use aex_internal_contracts::{PricingVersion, SchemaVersion};
use aex_wire::PrefixedId as _;
use aex_wire::idempotency::IntentDigest;
use aex_wire::ids::{OrganizationId, SessionId, WorkspaceId};
use aex_wire::provider::ProviderId;
use aex_wire::types::{DecimalU128, Region, Timestamp};
use finance_ingest::settlement::backlog::{
    MESSAGE_DEDUPLICATION_ATTRIBUTE, MESSAGE_GROUP_ATTRIBUTE,
};
use finance_ingest::settlement::settle::{category, interval};

fn identity(seed: u8) -> aex_wire::Uuid7 {
    aex_wire::Uuid7::from_bytes([
        0x01, 0x93, 0x3f, 0x2a, 0x1c, 0x00, 0x70, 0x00, 0x80, 0x00, 0, 0, 0, 0, 0, seed,
    ])
    .expect("a UUIDv7")
}

fn fact(meter: Meter) -> UsageFact {
    let region = Region::from_name("eu-west-1").expect("a region");
    let authority = FactAuthority {
        kind: AuthorityKind::Compute,
        authority_id: "run_01kyw2qa4pew48j2gb1g6gw3rg".into(),
        segment_ordinal: DecimalU128::new(0),
    };
    UsageFact {
        schema_version: SchemaVersion::V1,
        fact_id: FactId::derive(region, meter, &authority),
        meter,
        organization: OrganizationId::from_uuid7(identity(1)),
        workspace: WorkspaceId::from_uuid7(identity(9)),
        region,
        attribution: Attribution {
            session: None,
            run: None,
            operation: None,
        },
        authority,
        basis: FactBasis::Consumed,
        quantity: DecimalU128::new(1_000),
        service_time: ServiceTime::Interval {
            start: Timestamp::from_unix_millis(1_800_000_000_000).expect("an instant"),
            end: Timestamp::from_unix_millis(1_800_000_060_000).expect("an instant"),
        },
        source_receipt: SourceReceipt {
            source: "usage-compute-worker".into(),
            receipt_id: "rcp_1".into(),
            observed_at: Timestamp::from_unix_millis(1_800_000_000_000).expect("an instant"),
        },
        pricing_version: PricingVersion("synthetic-zero-v1".to_owned()),
        idempotency: FactIdempotency {
            deduplication_id: "eu-west-1:compute:usage_1".into(),
            business_key: "usage:eu-west-1:compute:usage_1".into(),
        },
    }
}

#[test]
fn regional_model_observation_decodes_and_groups_on_the_billing_worker_wire() {
    let region = Region::from_name("eu-west-1").expect("region");
    let organization = OrganizationId::from_uuid7(identity(1));
    let session = SessionId::from_uuid7(identity(2));
    let fact_id =
        ModelUsageFactId::derive(region, session, "0707070707070707", ModelTokenClass::Input);
    let body = RatingMessage::Model(ModelUsageRatingRequest {
        model_usage: ModelUsageFact {
            schema_version: SchemaVersion::V1,
            fact_id,
            organization,
            workspace: WorkspaceId::from_uuid7(identity(9)),
            region,
            session,
            assistant_effect: "0707070707070707".into(),
            provider: ProviderId::Openai,
            model: "gpt-5".into(),
            token_class: ModelTokenClass::Input,
            quantity: DecimalU128::new(123),
            observed_at: Timestamp::from_unix_millis(1_800_000_000_000).expect("time"),
            idempotency: FactIdempotency {
                deduplication_id: format!("eu-west-1:model:{fact_id}").into(),
                business_key: format!("model:eu-west-1:{fact_id}").into(),
            },
        },
        intent_hash: IntentDigest::from_bytes([8; 32]),
    });
    let body = serde_json::to_string(&body).expect("model body");
    let event: aws_lambda_events::sqs::SqsEvent = serde_json::from_str(&format!(
        r#"{{"Records":[{{"messageId":"model-1","body":{body},
           "attributes":{{"{MESSAGE_GROUP_ATTRIBUTE}":"{group}"}},
           "messageAttributes":{{}},"md5OfBody":"","eventSource":"aws:sqs",
           "eventSourceARN":"arn:aws:sqs:eu-west-1:000000000000:aex-dev-usage-rating.fifo",
           "awsRegion":"eu-west-1"}}]}}"#,
        body = serde_json::to_string(&body).expect("nested body"),
        group = organization.encode().as_str(),
    ))
    .expect("SQS event");
    let (delivered, failed) = finance_ingest::settlement::handler::decode(event);
    assert!(failed.is_empty());
    let groups = finance_ingest::settlement::backlog::partition(delivered).expect("group");
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].organization, organization);
    let RatingMessage::Model(decoded) = &groups[0].messages[0].request else {
        panic!("the zero-dollar arm must remain distinct")
    };
    assert_eq!(decoded.model_usage.quantity.get(), 123);
}

#[test]
fn the_fifo_group_the_producer_mints_is_exactly_the_organization() {
    let fact = fact(Meter::ComputeMillicpuMs);
    let organization = fact.organization;
    let message = FifoRatingMessage::new(fact, IntentDigest::from_bytes([4u8; 32]));
    assert_eq!(
        message.message_group_id,
        organization.encode().as_str(),
        "OD-19 and F-10: the group is the account, so one account is serialised"
    );
}

#[test]
fn the_producer_deduplication_hint_carries_region_category_and_fact() {
    let fact = fact(Meter::ComputeMillicpuMs);
    let fact_id = fact.fact_id.to_string();
    let message = FifoRatingMessage::new(fact, IntentDigest::from_bytes([4u8; 32]));
    let parts: Vec<&str> = message.message_deduplication_id.split(':').collect();
    assert_eq!(parts.len(), 3, "the hint is <region>:<category>:<factId>");
    assert_eq!(parts[0], "eu-west-1");
    assert_eq!(parts[1], "compute");
    assert_eq!(parts[2], fact_id);
}

#[test]
fn the_worker_and_the_producer_agree_on_the_rating_category_of_every_meter() {
    for meter in Meter::ALL {
        let fact = fact(meter);
        let message = FifoRatingMessage::new(fact, IntentDigest::from_bytes([4u8; 32]));
        let declared = message
            .message_deduplication_id
            .split(':')
            .nth(1)
            .expect("the hint carries a category");
        assert_eq!(
            declared,
            category(meter),
            "{meter:?} folds onto two different categories in producer and consumer"
        );
    }
}

#[test]
fn an_interval_fact_projects_a_half_open_service_window() {
    let (start, end) = interval(&fact(Meter::ComputeMillicpuMs));
    assert!(
        end > start,
        "the DDL requires interval_end >= interval_start"
    );
    assert_eq!(end - start, 60_000);
}

#[test]
fn the_two_fifo_attributes_are_named_exactly_as_sqs_spells_them() {
    assert_eq!(MESSAGE_GROUP_ATTRIBUTE, "MessageGroupId");
    assert_eq!(MESSAGE_DEDUPLICATION_ATTRIBUTE, "MessageDeduplicationId");
}

/// One regional fact, admitted by the regional domain exactly as a producer
/// would admit it.
fn regional_fact() -> aex_usage_domain::fact::UsageFact {
    use aex_usage_domain::fact::{
        Attribution as RegionalAttribution, FactDraft, FactKind, ResourceGeneration, ResourceKind,
        SCHEMA_VERSION,
    };
    use aex_usage_domain::frontier::AcceptedSequence;
    use aex_usage_domain::identity::{
        AuthorityId, AuthorityKey, AuthorityKind as RegionalAuthorityKind, SegmentOrdinal,
    };
    use aex_usage_domain::measurement::{
        BoundaryId, Evidence, FactBasis as RegionalBasis, Measurement, ReceiptKind,
        ServiceTime as RegionalServiceTime, SourceReceipt as RegionalReceipt,
    };
    use aex_usage_domain::meter::{Category, Meter as RegionalMeter};
    use aex_usage_domain::wire_pending as regional;

    let region = regional::RegionId::parse("eu-west-1").expect("a region");
    let organization = OrganizationId::from_uuid7(identity(1));
    let workspace = WorkspaceId::from_uuid7(identity(9));
    let session = aex_wire::ids::SessionId::from_uuid7(identity(3));
    let draft = FactDraft {
        schema_version: SCHEMA_VERSION,
        organization: regional::OrganizationId::parse(organization.encode().as_str())
            .expect("an org"),
        workspace: regional::WorkspaceId::parse(workspace.encode().as_str()).expect("a workspace"),
        region: region.clone(),
        attribution: RegionalAttribution {
            session: Some(
                regional::SessionId::parse(session.encode().as_str()).expect("a session"),
            ),
            agent: None,
            run: None,
            operation: None,
        },
        service: regional::ServiceId::parse("usage-transfer-worker").expect("a service"),
        resource: ResourceGeneration {
            kind: ResourceKind::StreamConnection,
            generation: Box::from("conn-7"),
        },
        authority: AuthorityKey {
            region,
            category: Category::Transfer,
            kind: RegionalAuthorityKind::EgressCrossing,
            authority_id: AuthorityId::parse("cross-7").expect("an id"),
            segment_ordinal: SegmentOrdinal::FIRST,
        },
        pricing_version: regional::PricingVersion::parse("synthetic-zero-v1").expect("a version"),
        reservation: None,
        kind: FactKind::Measured(
            Measurement::new(
                RegionalMeter::DataTransferEgressByte,
                RegionalBasis::Consumed,
                RegionalServiceTime::Instant {
                    at: regional::Timestamp::from_unix_millis(1_800_000_000_000)
                        .expect("an instant"),
                },
                RegionalReceipt {
                    kind: ReceiptKind::DeliveryLog,
                    id: Box::from("d-7"),
                    digest: None,
                },
                Evidence::DeliveryReceipt {
                    boundary: BoundaryId::CONTENT_DOWNLOAD,
                    receipt_id: Box::from("d-7"),
                    bytes: 4_096,
                },
            )
            .expect("a valid measurement"),
        ),
    };
    draft
        .admit(
            AcceptedSequence::new(1).expect("one-based"),
            regional::Timestamp::from_unix_millis(1_800_000_000_500).expect("an instant"),
        )
        .expect("admissible")
}

#[test]
fn the_producers_exact_bytes_decode_through_this_workers_exact_path() {
    // The defect this pins: the outbox once serialized the regional domain
    // fact while this worker decoded the contracts fact, so the first real
    // publish was 100% undecodable and whole batches died into the DLQ. The
    // producer's serialization and the worker's per-record decode must round
    // trip, cross-crate, forever.
    let fact = regional_fact();
    let message = aex_usage_app::outbox::OutboxMessage::for_fact(&fact)
        .expect("a measured fact converts to the contract");

    // Exactly the bytes `SettlementQueue::publish` puts on the queue.
    let body = serde_json::to_string(&message.body).expect("the producer body encodes");

    // Exactly the shape Lambda delivers, through the worker's own decoder.
    let event: aws_lambda_events::sqs::SqsEvent = serde_json::from_str(&format!(
        r#"{{"Records":[{{"messageId":"m1","body":{body},
           "attributes":{{"{MESSAGE_GROUP_ATTRIBUTE}":"{group}"}},
           "messageAttributes":{{}},"md5OfBody":"","eventSource":"aws:sqs",
           "eventSourceARN":"arn:aws:sqs:eu-west-1:000000000000:aex-dev-usage-rating.fifo",
           "awsRegion":"eu-west-1"}}]}}"#,
        body = serde_json::to_string(&body).expect("the body nests"),
        group = message.message_group_id,
    ))
    .expect("the delivered batch decodes");
    let (delivered, failed) = finance_ingest::settlement::handler::decode(event);
    assert_eq!(failed, Vec::<String>::new(), "nothing may fail to decode");
    assert_eq!(delivered.len(), 1);

    // The decoded request is the producer's request, field for field.
    let decoded = &delivered[0].request;
    let aex_internal_contracts::usage::RatingMessage::Priced(decoded) = decoded else {
        panic!("the priced producer must remain on the priced wire arm")
    };
    assert_eq!(decoded, &message.body);
    assert_eq!(decoded.fact.meter, Meter::DataTransferEgressByte);
    assert_eq!(decoded.fact.quantity.get(), 4_096);
    assert_eq!(
        decoded.fact.organization.encode().as_str(),
        message.message_group_id
    );
    assert_eq!(
        decoded.intent_hash.as_bytes(),
        fact.idempotency.intent_hash.as_bytes(),
        "the admission intent digest survives the wire"
    );

    // The grouping the producer minted is the one the partition accepts.
    let groups = finance_ingest::settlement::backlog::partition(delivered)
        .expect("the producer's group is the account");
    assert_eq!(groups.len(), 1);

    // And both sides derive one deduplication identity from the fact.
    let expected = FifoRatingMessage::new(message.body.fact.clone(), message.body.intent_hash);
    assert_eq!(
        expected.message_deduplication_id, message.message_deduplication_id,
        "producer and consumer spell the FIFO dedupe identity identically"
    );
    assert_eq!(expected.message_group_id, message.message_group_id);
}
