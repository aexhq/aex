//! The FIFO backlog, partitioned by account.
//!
//! `MessageGroupId` is exactly the organization id (OD-19, F-10), so SQS
//! serialises one account and parallelises across accounts. This module turns
//! one delivered batch into per-account groups, settles each group in one
//! transaction, and names in the partial-batch response only the messages whose
//! group did not commit.

use std::collections::BTreeMap;

use aex_finance_app::use_cases::RatingRequest;
use aex_wire::PrefixedId as _;
use aex_wire::ids::OrganizationId;

/// The SQS message attribute carrying the FIFO group.
pub const MESSAGE_GROUP_ATTRIBUTE: &str = "MessageGroupId";
/// The SQS message attribute carrying the producer deduplication hint.
pub const MESSAGE_DEDUPLICATION_ATTRIBUTE: &str = "MessageDeduplicationId";

/// One delivered message, already decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivered {
    /// The SQS message identity, which is what a partial-batch failure names.
    pub message_id: String,
    /// The declared FIFO group.
    pub group: String,
    /// The typed body.
    pub request: RatingRequest,
}

/// One account's slice of a delivered batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountGroup {
    /// The account every message in the group belongs to.
    pub organization: OrganizationId,
    /// The messages, in delivery order.
    pub messages: Vec<Delivered>,
}

/// Why a delivered message could not be grouped.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GroupError {
    /// The message carried no SQS identity.
    #[error("a delivered message carries no message id")]
    MissingMessageId,
    /// The message carried no FIFO group.
    #[error("message `{0}` carries no {MESSAGE_GROUP_ATTRIBUTE}")]
    MissingGroup(String),
    /// The declared group is not the organization the fact belongs to.
    #[error("message `{message_id}` declares group `{group}` for organization `{organization}`")]
    GroupIsNotTheAccount {
        /// The message identity.
        message_id: String,
        /// What the producer declared.
        group: String,
        /// What the fact says.
        organization: String,
    },
}

/// Partitions one delivered batch into account groups, in first-seen order.
///
/// # Errors
///
/// Returns [`GroupError`] when a message has no identity, declares no group, or
/// declares a group that is not the account its fact belongs to. A message that
/// disagrees with its own group would break the ordering guarantee the whole
/// settlement design rests on, so it is refused rather than regrouped.
pub fn partition(delivered: Vec<Delivered>) -> Result<Vec<AccountGroup>, GroupError> {
    let mut order: Vec<OrganizationId> = Vec::new();
    let mut groups: BTreeMap<OrganizationId, Vec<Delivered>> = BTreeMap::new();
    for message in delivered {
        if message.message_id.is_empty() {
            return Err(GroupError::MissingMessageId);
        }
        if message.group.is_empty() {
            return Err(GroupError::MissingGroup(message.message_id));
        }
        let organization = message.request.fact.organization;
        if message.group != organization.encode().as_str() {
            return Err(GroupError::GroupIsNotTheAccount {
                message_id: message.message_id,
                group: message.group,
                organization: organization.encode().as_str().to_owned(),
            });
        }
        if !groups.contains_key(&organization) {
            order.push(organization);
        }
        groups.entry(organization).or_default().push(message);
    }
    Ok(order
        .into_iter()
        .map(|organization| AccountGroup {
            messages: groups.remove(&organization).unwrap_or_default(),
            organization,
        })
        .collect())
}

/// The message identities of every group that did not commit.
///
/// A committed group contributes nothing: naming a committed message would make
/// SQS redeliver work that is already durable.
#[must_use]
pub fn uncommitted(groups: &[AccountGroup], failed: &[OrganizationId]) -> Vec<String> {
    groups
        .iter()
        .filter(|group| failed.contains(&group.organization))
        .flat_map(|group| {
            group
                .messages
                .iter()
                .map(|message| message.message_id.clone())
        })
        .collect()
}

/// Splits one account group into transaction-sized chunks.
///
/// Ordering inside the account is preserved, because the chunks are contiguous
/// and settled in order.
#[must_use]
pub fn chunks(group: &AccountGroup, max: u32) -> Vec<Vec<Delivered>> {
    let max = usize::try_from(max).unwrap_or(usize::MAX).max(1);
    group
        .messages
        .chunks(max)
        .map(<[Delivered]>::to_vec)
        .collect()
}

#[cfg(test)]
mod tests {
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

    use super::{Delivered, GroupError, chunks, partition, uncommitted};

    fn identity(seed: u8) -> aex_wire::Uuid7 {
        aex_wire::Uuid7::from_bytes([
            0x01, 0x93, 0x3f, 0x2a, 0x1c, 0x00, 0x70, 0x00, 0x80, 0x00, 0, 0, 0, 0, 0, seed,
        ])
        .expect("a UUIDv7")
    }

    fn organization(seed: u8) -> OrganizationId {
        OrganizationId::from_uuid7(identity(seed))
    }

    fn fact(organization: OrganizationId, ordinal: u128) -> UsageFact {
        let region = Region::from_name("eu-west-1").expect("a region");
        let authority = FactAuthority {
            kind: AuthorityKind::Compute,
            authority_id: "run_01kyw2qa4pew48j2gb1g6gw3rg".into(),
            segment_ordinal: DecimalU128::new(ordinal),
        };
        let fact_id = FactId::derive(region, Meter::ComputeMillicpuMs, &authority);
        UsageFact {
            schema_version: SchemaVersion::V1,
            fact_id,
            meter: Meter::ComputeMillicpuMs,
            organization,
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
        }
    }

    fn delivered(
        message_id: &str,
        organization: OrganizationId,
        group: &str,
        ordinal: u128,
    ) -> Delivered {
        Delivered {
            message_id: message_id.to_owned(),
            group: group.to_owned(),
            request: RatingRequest {
                fact: fact(organization, ordinal),
                intent_hash: IntentHash::new([1u8; 32]),
            },
        }
    }

    #[test]
    fn a_batch_partitions_into_one_group_per_account_in_first_seen_order() {
        let first = organization(1);
        let second = organization(2);
        let batch = vec![
            delivered("m1", first, first.encode().as_str(), 0),
            delivered("m2", second, second.encode().as_str(), 1),
            delivered("m3", first, first.encode().as_str(), 2),
        ];
        let groups = partition(batch).expect("every message declares its own account");
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].organization, first);
        assert_eq!(groups[0].messages.len(), 2, "one account, one group");
        assert_eq!(groups[1].organization, second);
    }

    #[test]
    fn a_message_whose_group_is_not_its_account_is_refused() {
        let first = organization(1);
        let second = organization(2);
        let error = partition(vec![delivered("m1", first, second.encode().as_str(), 0)])
            .expect_err("the group is the account and nothing else");
        assert!(matches!(error, GroupError::GroupIsNotTheAccount { .. }));
    }

    #[test]
    fn a_message_with_no_group_is_refused_rather_than_settled_out_of_order() {
        let first = organization(1);
        let error = partition(vec![delivered("m1", first, "", 0)])
            .expect_err("a FIFO message always carries its group");
        assert_eq!(error, GroupError::MissingGroup("m1".to_owned()));
    }

    #[test]
    fn only_the_messages_of_an_uncommitted_account_are_named() {
        let first = organization(1);
        let second = organization(2);
        let groups = partition(vec![
            delivered("m1", first, first.encode().as_str(), 0),
            delivered("m2", first, first.encode().as_str(), 1),
            delivered("m3", second, second.encode().as_str(), 2),
        ])
        .expect("a valid batch");
        assert_eq!(uncommitted(&groups, &[second]), vec!["m3".to_owned()]);
        assert_eq!(
            uncommitted(&groups, &[first]),
            vec!["m1".to_owned(), "m2".to_owned()]
        );
        assert!(
            uncommitted(&groups, &[]).is_empty(),
            "a fully committed batch names nothing"
        );
    }

    #[test]
    fn an_account_group_chunks_contiguously_and_never_into_nothing() {
        let first = organization(1);
        let groups = partition(vec![
            delivered("m1", first, first.encode().as_str(), 0),
            delivered("m2", first, first.encode().as_str(), 1),
            delivered("m3", first, first.encode().as_str(), 2),
        ])
        .expect("a valid batch");
        let split = chunks(&groups[0], 2);
        assert_eq!(split.len(), 2);
        assert_eq!(split[0].len(), 2);
        assert_eq!(split[1].len(), 1);
        assert_eq!(
            chunks(&groups[0], 0).len(),
            3,
            "a zero bound is one message per chunk"
        );
    }
}
