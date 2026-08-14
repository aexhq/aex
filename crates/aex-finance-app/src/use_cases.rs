//! Pure settlement delivery contracts and partial-batch accounting.

use aex_internal_contracts::usage::{FactId, UsageFact};
use aex_wire::idempotency::IntentDigest;
use aex_wire::ids::{OrganizationId, PrefixedId as _};

// The wire body is declared once, in the contracts crate, and consumed here.
// Declaring a second `RatingRequest` in this crate is how the producer and the
// consumer came to serialize two different facts for one queue.
pub use aex_internal_contracts::usage::{RatingMessage, RatingRequest};

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
    pub fn new(fact: UsageFact, intent_hash: IntentDigest) -> Self {
        let category = fact.meter.category();
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
