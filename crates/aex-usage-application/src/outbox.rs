//! The regional outbox message, shaped to the central settlement inbox contract.
//!
//! This is a conformance module, not a design one. Finance owns
//! `aex-<plane>-usage-rating.fifo`, its `PRIMARY KEY (region, category,
//! fact_id)` inbox, its `business_key` grammar and its `intent_hash` rule; this
//! crate reproduces them byte for byte and adds nothing of its own. Every
//! identity below appears in `03-central-finance-schema.md` §4.2 and is
//! reproduced here so a drift on either side is a failing test rather than a
//! duplicate settlement.
//!
//! The two identities that carry the money guarantee:
//!
//! - `MessageGroupId = organizationId`, so settlement is ordered per paying
//!   account (`OD-19`). Regional fan-out does not need global ordering; an
//!   account's ledger does.
//! - `MessageDeduplicationId = {region}:{category}:{factId}`, which is derived
//!   entirely from the deterministic fact identity. A producer retry, an SQS
//!   redelivery and a sweep republish therefore all collapse onto one inbox row
//!   with no extra state anywhere.

use aex_usage_domain::fact::UsageFact;
use aex_usage_domain::intent::IntentHash;
use aex_usage_domain::meter::Category;
use aex_usage_domain::wire_pending::{OrganizationId, PricingVersion, RegionId};
use serde::{Deserialize, Serialize};

/// The `business_key` prefix every usage settlement posts under.
pub const BUSINESS_KEY_PREFIX: &str = "usage";

/// Why an outbox message could not be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OutboxError {
    /// A component of a settlement identity was not canonical.
    ///
    /// The central grammar is lowercase and `:`-separated, so a component
    /// carrying either an upper-case letter or a colon would make two different
    /// facts able to render one `business_key`.
    #[error("`{component}` is not a canonical settlement identity component: {reason}")]
    NotCanonical {
        /// The offending component.
        component: String,
        /// Why it was refused.
        reason: &'static str,
    },
}

/// The body of one rating request.
///
/// Serialized field names are the central contract's, not this crate's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RatingRequest {
    /// The whole admitted fact. Central re-validates it rather than trusting it.
    pub fact: UsageFact,
    /// The admission-time intent hash, which decides replay from conflict.
    pub intent_hash: IntentHash,
}

/// One message as it is handed to the central FIFO queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxMessage {
    /// The FIFO ordering group: one paying account, one order.
    pub message_group_id: OrganizationId,
    /// The FIFO dedupe identity, derived only from the fact identity.
    pub message_deduplication_id: String,
    /// The journal identity central posts the settlement under.
    pub business_key: String,
    /// The rate book pinned at admission; never `current`.
    pub pricing_version: PricingVersion,
    /// The body.
    pub body: RatingRequest,
}

impl OutboxMessage {
    /// Builds the message for one admitted fact.
    ///
    /// # Errors
    ///
    /// Returns [`OutboxError::NotCanonical`] when a component of the identity is
    /// not lowercase or carries the `:` separator.
    pub fn for_fact(fact: &UsageFact) -> Result<Self, OutboxError> {
        let region = canonical(fact.region.as_str())?;
        let category = canonical(fact.category().id())?;
        let fact_id = canonical(fact.fact_id.as_str())?;
        let identity = format!("{region}:{category}:{fact_id}");
        Ok(Self {
            message_group_id: fact.organization.clone(),
            message_deduplication_id: identity.clone(),
            business_key: format!("{BUSINESS_KEY_PREFIX}:{identity}"),
            pricing_version: fact.pricing_version.clone(),
            body: RatingRequest {
                fact: fact.clone(),
                intent_hash: fact.idempotency.intent_hash,
            },
        })
    }

    /// The `(region, category, fact_id)` triple the central inbox keys on.
    #[must_use]
    pub fn inbox_key(&self) -> (RegionId, Category, String) {
        (
            self.body.fact.region.clone(),
            self.body.fact.category(),
            self.body.fact.fact_id.to_string(),
        )
    }
}

/// Refuses a component that is not canonical for the central grammar.
fn canonical(value: &str) -> Result<&str, OutboxError> {
    if value.is_empty() {
        return Err(OutboxError::NotCanonical {
            component: value.to_owned(),
            reason: "empty",
        });
    }
    if value.contains(':') {
        return Err(OutboxError::NotCanonical {
            component: value.to_owned(),
            reason: "carries the `:` separator",
        });
    }
    if value.chars().any(char::is_uppercase) {
        return Err(OutboxError::NotCanonical {
            component: value.to_owned(),
            reason: "is not lowercase",
        });
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::{BUSINESS_KEY_PREFIX, OutboxMessage};
    use crate::testing::{fact, observability_fact, void_fact};
    use aex_usage_domain::meter::Category;

    #[test]
    fn the_dedupe_identity_is_the_central_grammar_exactly() {
        let fact = fact(Category::Storage, 1);
        let message = OutboxMessage::for_fact(&fact).expect("builds");
        assert_eq!(
            message.message_deduplication_id,
            format!("eu-west-1:storage:{}", fact.fact_id),
            "finance keys its inbox on this triple; a different spelling is a \
             second settlement, not a second name"
        );
        assert_eq!(
            message.business_key,
            format!("{BUSINESS_KEY_PREFIX}:{}", message.message_deduplication_id)
        );
    }

    #[test]
    fn the_ordering_group_is_the_paying_account() {
        let message = OutboxMessage::for_fact(&fact(Category::Compute, 1)).expect("builds");
        assert_eq!(
            message.message_group_id.as_str(),
            "org-1",
            "settlement is ordered per organization, which is the money key"
        );
    }

    #[test]
    fn a_producer_retry_produces_a_byte_identical_message() {
        // The whole idempotency guarantee rests on this: nothing in the message
        // is minted at send time, so a retry, a redelivery and a sweep
        // republish are the same message.
        let fact = fact(Category::Transfer, 4);
        let first = OutboxMessage::for_fact(&fact).expect("builds");
        let second = OutboxMessage::for_fact(&fact).expect("builds");
        assert_eq!(first, second);
        assert_eq!(
            serde_json::to_string(&first.body).expect("serializes"),
            serde_json::to_string(&second.body).expect("serializes")
        );
    }

    #[test]
    fn two_categories_of_one_workspace_never_share_a_dedupe_identity() {
        let mut identities: Vec<String> = Category::ALL
            .into_iter()
            .map(|category| {
                OutboxMessage::for_fact(&fact(category, 1))
                    .expect("builds")
                    .message_deduplication_id
            })
            .collect();
        let count = identities.len();
        identities.sort();
        identities.dedup();
        assert_eq!(count, identities.len());
    }

    #[test]
    fn the_inbox_key_is_the_triple_finance_declares() {
        let fact = fact(Category::Storage, 7);
        let (region, category, fact_id) =
            OutboxMessage::for_fact(&fact).expect("builds").inbox_key();
        assert_eq!(region, fact.region);
        assert_eq!(category, Category::Storage);
        assert_eq!(fact_id, fact.fact_id.to_string());
    }

    #[test]
    fn a_correction_and_a_zero_dollar_fact_are_both_published() {
        // Central rates a void to a reversing entry and an observability fact to
        // nothing, but both are still money evidence and both must arrive.
        let target = fact(Category::Compute, 1);
        for fact in [
            void_fact(Category::Compute, 2, &target.fact_id),
            observability_fact(3),
        ] {
            let message = OutboxMessage::for_fact(&fact).expect("builds");
            assert_eq!(message.body.intent_hash, fact.idempotency.intent_hash);
            assert!(message.business_key.starts_with("usage:eu-west-1:compute:"));
        }
    }

    #[test]
    fn the_body_carries_the_admission_intent_hash_not_a_recomputed_one() {
        // A recomputed hash would agree with itself even if the stored fact had
        // drifted, which is exactly the conflict the central inbox exists to
        // catch.
        let fact = fact(Category::Storage, 2);
        let message = OutboxMessage::for_fact(&fact).expect("builds");
        assert_eq!(message.body.intent_hash, fact.idempotency.intent_hash);
        assert_eq!(message.pricing_version, fact.pricing_version);
    }
}
