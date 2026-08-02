//! Generated properties over the account partition.
//!
//! The FIFO design rests on two claims: an account's messages never split
//! across groups, and a committed account never appears in a partial-batch
//! response. Both are asserted over arbitrary batches rather than examples.

use aex_finance_app::use_cases::RatingRequest;
use aex_finance_domain::IntentHash;
use aex_internal_contracts::usage::{
    Attribution, AuthorityKind, FactAuthority, FactBasis, FactId, FactIdempotency, Meter,
    ServiceTime, SourceReceipt, UsageFact,
};
use aex_internal_contracts::{PricingVersion, SchemaVersion};
use aex_wire::PrefixedId as _;
use aex_wire::ids::{OrganizationId, WorkspaceId};
use aex_wire::types::{DecimalU128, Region, Timestamp};
use finance_settlement_worker::backlog::{Delivered, chunks, partition, uncommitted};
use proptest::prelude::*;

fn identity(seed: u8) -> aex_wire::Uuid7 {
    aex_wire::Uuid7::from_bytes([
        0x01, 0x93, 0x3f, 0x2a, 0x1c, 0x00, 0x70, 0x00, 0x80, 0x00, 0, 0, 0, 0, 0, seed,
    ])
    .expect("a UUIDv7")
}

fn organization(seed: u8) -> OrganizationId {
    OrganizationId::from_uuid7(identity(seed))
}

fn delivered(index: usize, seed: u8) -> Delivered {
    let organization = organization(seed);
    let region = Region::from_name("eu-west-1").expect("a region");
    let authority = FactAuthority {
        kind: AuthorityKind::Compute,
        authority_id: "run_01kyw2qa4pew48j2gb1g6gw3rg".into(),
        segment_ordinal: DecimalU128::new(index as u128),
    };
    let fact = UsageFact {
        schema_version: SchemaVersion::V1,
        fact_id: FactId::derive(region, Meter::ComputeMillicpuMs, &authority),
        meter: Meter::ComputeMillicpuMs,
        organization,
        workspace: WorkspaceId::from_uuid7(identity(200)),
        region,
        attribution: Attribution {
            session: None,
            run: None,
            operation: None,
        },
        authority,
        basis: FactBasis::Consumed,
        quantity: DecimalU128::new(1),
        service_time: ServiceTime::Instant {
            at: Timestamp::from_unix_millis(1_800_000_000_000).expect("an instant"),
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
    };
    Delivered {
        message_id: format!("m{index}"),
        group: organization.encode().as_str().to_owned(),
        request: RatingRequest {
            fact,
            intent_hash: IntentHash::new([1u8; 32]),
        },
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Every delivered message appears in exactly one account group.
    #[test]
    fn the_partition_is_total_and_disjoint(seeds in prop::collection::vec(1u8..6, 0..40)) {
        let batch: Vec<Delivered> = seeds
            .iter()
            .enumerate()
            .map(|(index, seed)| delivered(index, *seed))
            .collect();
        let expected = batch.len();
        let groups = partition(batch).expect("every fixture declares its own account");
        let grouped: usize = groups.iter().map(|group| group.messages.len()).sum();
        prop_assert_eq!(grouped, expected);

        let mut seen = std::collections::BTreeSet::new();
        for group in &groups {
            prop_assert!(
                seen.insert(group.organization),
                "an account appears in exactly one group"
            );
            for message in &group.messages {
                prop_assert_eq!(message.request.fact.organization, group.organization);
            }
        }
    }

    /// A committed account is never named in a partial-batch response.
    #[test]
    fn a_committed_account_is_never_redelivered(
        seeds in prop::collection::vec(1u8..6, 1..30),
        failing in 1u8..6,
    ) {
        let batch: Vec<Delivered> = seeds
            .iter()
            .enumerate()
            .map(|(index, seed)| delivered(index, *seed))
            .collect();
        let groups = partition(batch).expect("a valid batch");
        let failed = vec![organization(failing)];
        let named = uncommitted(&groups, &failed);
        for group in &groups {
            for message in &group.messages {
                let should_fail = group.organization == organization(failing);
                prop_assert_eq!(
                    named.contains(&message.message_id),
                    should_fail,
                    "message {} is named for a group that committed",
                    message.message_id
                );
            }
        }
    }

    /// Chunking preserves both the order and the multiset of an account group.
    #[test]
    fn chunking_preserves_order_and_loses_nothing(
        count in 1usize..30,
        bound in 0u32..8,
    ) {
        let batch: Vec<Delivered> = (0..count).map(|index| delivered(index, 1)).collect();
        let groups = partition(batch).expect("a valid batch");
        let flattened: Vec<String> = chunks(&groups[0], bound)
            .into_iter()
            .flatten()
            .map(|message| message.message_id)
            .collect();
        let original: Vec<String> = groups[0]
            .messages
            .iter()
            .map(|message| message.message_id.clone())
            .collect();
        prop_assert_eq!(flattened, original);
    }
}
