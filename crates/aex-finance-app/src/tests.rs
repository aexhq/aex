use aex_finance_domain::IntentHash;
use aex_internal_contracts::usage::{
    Attribution, AuthorityKind, FactAuthority, FactBasis, FactId, FactIdempotency, Meter,
    ServiceTime, SourceReceipt, UsageFact,
};
use aex_internal_contracts::{PricingVersion, SchemaVersion};
use aex_wire::ids::{OrganizationId, PrefixedId as _, Uuid7, WorkspaceId};
use aex_wire::types::{DecimalU128, Region, Timestamp};

use crate::use_cases::{FactFailure, FifoRatingMessage, partial_batch_failures};

#[test]
fn fifo_contract_partitions_by_organization_and_deduplicates_by_fact() {
    let fact = usage_fact(Meter::MemoryByteMs, 7, 1);
    let message = FifoRatingMessage::new(fact.clone(), IntentHash::new([9; 32]));
    assert_eq!(
        message.message_group_id,
        fact.organization.encode().as_str()
    );
    assert_eq!(
        message.message_deduplication_id,
        format!("eu-west-1:compute:{}", fact.fact_id)
    );
    assert_eq!(message.body.fact, fact);
}

#[test]
fn partial_batch_response_names_exactly_uncommitted_groups() {
    let facts = vec![
        (
            "item-0".to_owned(),
            usage_fact(Meter::ComputeMillicpuMs, 1, 1),
        ),
        ("item-1".to_owned(), usage_fact(Meter::MemoryByteMs, 1, 1)),
        ("item-2".to_owned(), usage_fact(Meter::StorageByteMin, 2, 2)),
        (
            "item-3".to_owned(),
            usage_fact(Meter::DataTransferEgressByte, 3, 3),
        ),
    ];
    let failures = partial_batch_failures(
        &facts,
        &[
            FactFailure::Organization(facts[0].1.organization),
            FactFailure::Fact(facts[3].1.fact_id),
        ],
    );
    assert_eq!(failures, vec!["item-0", "item-1", "item-3"]);
}

fn usage_fact(meter: Meter, org_seed: u8, ordinal: u128) -> UsageFact {
    let organization = OrganizationId::from_uuid7(Uuid7::compose(1, [org_seed; 10]));
    let workspace = WorkspaceId::from_uuid7(Uuid7::compose(2, [org_seed; 10]));
    let authority = FactAuthority {
        kind: match meter {
            Meter::StorageByteMin => AuthorityKind::Storage,
            Meter::ComputeMillicpuMs | Meter::MemoryByteMs => AuthorityKind::Compute,
            Meter::DataTransferEgressByte => AuthorityKind::Transfer,
        },
        authority_id: format!("fixture-{ordinal}").into_boxed_str(),
        segment_ordinal: DecimalU128::new(ordinal),
    };
    let fact_id = FactId::derive(Region::EuWest1, meter, &authority);
    UsageFact {
        schema_version: SchemaVersion::V1,
        fact_id,
        meter,
        organization,
        workspace,
        region: Region::EuWest1,
        attribution: Attribution {
            session: None,
            run: None,
            operation: None,
        },
        authority,
        basis: FactBasis::Consumed,
        quantity: DecimalU128::new(ordinal),
        service_time: ServiceTime::Instant {
            at: Timestamp::from_unix_millis(0).expect("epoch"),
        },
        source_receipt: SourceReceipt {
            source: "fixture".into(),
            receipt_id: format!("receipt-{ordinal}").into_boxed_str(),
            observed_at: Timestamp::from_unix_millis(0).expect("epoch"),
        },
        pricing_version: PricingVersion("synthetic-zero-v1".into()),
        idempotency: FactIdempotency {
            deduplication_id: "fixture".into(),
            business_key: "usage:fixture".into(),
        },
    }
}
