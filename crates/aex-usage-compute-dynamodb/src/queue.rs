//! The `RatingQueue` implementation over the central settlement FIFO queue.
//!
//! Nothing here mints an identity. The group id, the dedupe id and the business
//! key all come from [`OutboxMessage`], which derives them from the
//! deterministic fact identity — so a producer retry, an SQS redelivery and a
//! sweep republish are byte-identical messages that central collapses onto one
//! inbox row with no extra state anywhere.
//!
//! A central outage is [`PortError::Unavailable`], never a swallowed success.
//! `U-18` requires an outage to accumulate backlog rather than stall admission,
//! and that only works if the caller can tell the difference between "delivered"
//! and "deferred".

use aex_usage_app::outbox::OutboxMessage;
use aex_usage_app::ports::{PortError, RatingQueue};
use async_trait::async_trait;
use aws_sdk_sqs::Client;
use aws_sdk_sqs::types::MessageAttributeValue;

/// The name this queue reports in a port error.
const WHAT: &str = "usage rating queue";

/// The message attribute carrying the journal identity central posts under.
pub const BUSINESS_KEY_ATTRIBUTE: &str = "businessKey";
/// The message attribute carrying the rate book pinned at admission.
pub const PRICING_VERSION_ATTRIBUTE: &str = "pricingVersion";

/// The central settlement FIFO queue.
#[derive(Debug, Clone)]
pub struct SettlementQueue {
    client: Client,
    queue_url: String,
}

impl SettlementQueue {
    /// Binds the producer to a client and a queue URL.
    ///
    /// The URL's last path segment is the queue name, which is what the
    /// deployment's charging gate checks: a shadow deployment must point at
    /// `…-shadow.fifo` and an active one must not.
    #[must_use]
    pub fn new(client: Client, queue_url: impl Into<String>) -> Self {
        Self {
            client,
            queue_url: queue_url.into(),
        }
    }

    /// The queue this producer publishes to.
    #[must_use]
    pub fn queue_url(&self) -> &str {
        &self.queue_url
    }
}

#[async_trait]
impl RatingQueue for SettlementQueue {
    async fn publish(&self, message: &OutboxMessage) -> Result<(), PortError> {
        let body = serde_json::to_string(&message.body).map_err(|error| PortError::Corrupt {
            what: "rating request",
            reason: error.to_string(),
        })?;
        let attribute = |value: &str| {
            MessageAttributeValue::builder()
                .data_type("String")
                .string_value(value)
                .build()
                .map_err(|error| PortError::Corrupt {
                    what: "rating request",
                    reason: error.to_string(),
                })
        };
        self.client
            .send_message()
            .queue_url(&self.queue_url)
            .message_body(body)
            // Settlement is ordered per paying account: regional fan-out does not
            // need global ordering, an account's ledger does.
            .message_group_id(message.message_group_id.as_str())
            // Derived entirely from the deterministic fact identity, so nothing
            // is minted at send time and a retry is the same message.
            .message_deduplication_id(&message.message_deduplication_id)
            .message_attributes(BUSINESS_KEY_ATTRIBUTE, attribute(&message.business_key)?)
            .message_attributes(
                PRICING_VERSION_ATTRIBUTE,
                attribute(message.pricing_version.as_str())?,
            )
            .send()
            .await
            .map_err(|error| sqs_error(&error))?;
        Ok(())
    }
}

/// Maps one SQS failure onto the port vocabulary.
///
/// A send that did not reach the queue is retryable; the sweep owns delivery
/// from there either way, so nothing here ever reports a success it did not get.
fn sqs_error<E, R>(error: &aws_sdk_sqs::error::SdkError<E, R>) -> PortError
where
    E: aws_sdk_sqs::error::ProvideErrorMetadata,
{
    match error {
        aws_sdk_sqs::error::SdkError::ServiceError(inner) => {
            let code = inner.err().code().unwrap_or("Unknown").to_owned();
            let reason = inner
                .err()
                .message()
                .map_or_else(|| code.clone(), |message| format!("{code}: {message}"));
            match code.as_str() {
                // A malformed message never becomes well-formed by waiting.
                "InvalidParameterValue" | "InvalidMessageContents" | "UnsupportedOperation" => {
                    PortError::Corrupt { what: WHAT, reason }
                }
                "QueueDoesNotExist" => PortError::NotFound {
                    what: WHAT,
                    id: reason,
                },
                _ => PortError::Unavailable { what: WHAT, reason },
            }
        }
        _ => PortError::Unavailable {
            what: WHAT,
            reason: "central is unreachable".to_owned(),
        },
    }
}
