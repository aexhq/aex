//! The producer contract this worker consumes, asserted against the peer that
//! publishes it.

use aex_finance_app::use_cases::FifoRatingMessage;
use aex_finance_domain::IntentHash;
use aex_internal_contracts::usage::{
    Attribution, AuthorityKind, FactAuthority, FactBasis, FactId, FactIdempotency, Meter,
    ServiceTime, SourceReceipt, UsageFact,
};
use aex_internal_contracts::{PricingVersion, SchemaVersion};
use aex_wire::PrefixedId as _;
use aex_wire::ids::{OrganizationId, WorkspaceId};
use aex_wire::types::{DecimalU128, Region, Timestamp};
use finance_settlement_worker::backlog::{
    MESSAGE_DEDUPLICATION_ATTRIBUTE, MESSAGE_GROUP_ATTRIBUTE,
};
use finance_settlement_worker::settle::{category, interval};

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
fn the_fifo_group_the_producer_mints_is_exactly_the_organization() {
    let fact = fact(Meter::ComputeMillicpuMs);
    let organization = fact.organization;
    let message = FifoRatingMessage::new(fact, IntentHash::new([4u8; 32]));
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
    let message = FifoRatingMessage::new(fact, IntentHash::new([4u8; 32]));
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
        let message = FifoRatingMessage::new(fact, IntentHash::new([4u8; 32]));
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
