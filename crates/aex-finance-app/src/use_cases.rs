//! Pure settlement delivery contracts and partial-batch accounting.

use aex_finance_domain::IntentHash;
use aex_internal_contracts::usage::{FactId, Meter, UsageFact};
use aex_wire::ids::{OrganizationId, PrefixedId as _};
use serde::{Deserialize, Serialize};

/// Body delivered from a regional usage outbox to central rating.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RatingRequest {
    /// Immutable regional fact.
    pub fact: UsageFact,
    /// Canonical producer intent digest.
    pub intent_hash: IntentHash,
}

/// Transport-independent SQS FIFO producer contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FifoRatingMessage {
    /// Exactly the organization identity, serializing one account.
    pub message_group_id: String,
    /// `<region>:<category>:<factId>` hint.
    pub message_deduplication_id: String,
    /// Typed body.
    pub body: RatingRequest,
}

impl FifoRatingMessage {
    /// Derives every identity from the immutable fact; callers supply none.
    #[must_use]
    pub fn new(fact: UsageFact, intent_hash: IntentHash) -> Self {
        let category = category(fact.meter);
        let message_group_id = fact.organization.encode().as_str().to_owned();
        let message_deduplication_id =
            format!("{}:{category}:{}", fact.region.as_str(), fact.fact_id);
        Self {
            message_group_id,
            message_deduplication_id,
            body: RatingRequest { fact, intent_hash },
        }
    }
}

/// Durable fact or account group that did not commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactFailure {
    /// Every message for this organization failed its one transaction.
    Organization(OrganizationId),
    /// One poison or otherwise uncommitted fact.
    Fact(FactId),
}

/// Produces the exact SQS partial-batch item identifier list.
#[must_use]
pub fn partial_batch_failures(
    delivered: &[(String, UsageFact)],
    failures: &[FactFailure],
) -> Vec<String> {
    delivered
        .iter()
        .filter(|(_, fact)| {
            failures.iter().any(|failure| match failure {
                FactFailure::Organization(organization) => fact.organization == *organization,
                FactFailure::Fact(fact_id) => fact.fact_id == *fact_id,
            })
        })
        .map(|(item_id, _)| item_id.clone())
        .collect()
}

const fn category(meter: Meter) -> &'static str {
    match meter {
        Meter::ComputeMillicpuMs | Meter::MemoryByteMs => "compute",
        Meter::StorageByteMin => "storage",
        Meter::DataTransferEgressByte => "transfer",
    }
}
