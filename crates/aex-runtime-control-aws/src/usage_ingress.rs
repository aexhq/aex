//! Category-fenced producers for the one central billing FIFO.

use std::future::Future;
use std::pin::Pin;

use aex_runtime_control::usage::{SinkError, UsageCategory, UsageFactSink};
use aex_usage_app::outbox::OutboxMessage;
use aex_usage_domain::fact::FactDraft;
use aex_usage_domain::frontier::AcceptedSequence;
use aex_usage_domain::wire_pending::Timestamp;
use aws_sdk_sqs::Client;
use aws_sdk_sqs::error::{ProvideErrorMetadata, SdkError};

/// Maximum SQS message body size.
const SQS_BODY_MAX_BYTES: usize = 256 * 1024;

/// The service-error codes no redelivery can clear.
///
/// Every one of these is a binding or grammar fault: the queue named by
/// `AEX_USAGE_*_QUEUE_URL` does not exist, the role may not write to it, or the
/// request is malformed for the queue's own type. Redriving them to
/// `MAX_RECEIVE_COUNT` and only then quarantining turns a deploy-time
/// misconfiguration into a slow leak that reports as a transient outage, which
/// is exactly the shape the dev plane already produced once. Both spellings of
/// the missing-queue refusal are listed because the JSON protocol answers
/// `QueueDoesNotExist` while the legacy query protocol answers
/// `AWS.SimpleQueueService.NonExistentQueue`, and a producer that only knows one
/// of them silently reclassifies the other as retryable.
///
/// `MissingParameter` catches a malformed FIFO binding where the queue rejects
/// the producer-derived organization group or fact deduplication identity.
const TERMINAL_SQS_CODES: [&str; 8] = [
    "QueueDoesNotExist",
    "AWS.SimpleQueueService.NonExistentQueue",
    "AccessDenied",
    "AccessDeniedException",
    "InvalidAddress",
    "InvalidParameterValue",
    "InvalidMessageContents",
    "MissingParameter",
];

/// Classifies one send failure as permanent or worth redriving.
///
/// The default is [`SinkError::Unavailable`], because an unrecognised failure is
/// more likely to be an outage than a grammar fault; the named codes are the
/// ones that are never worth another delivery.
fn classify<E, R>(category: UsageCategory, error: &SdkError<E, R>) -> SinkError
where
    E: ProvideErrorMetadata,
{
    let reason = error.to_string();
    match error {
        SdkError::ServiceError(service) => {
            let code = service.err().code().unwrap_or("Unknown");
            let reason = format!("{reason} (the service reported `{code}`)");
            if TERMINAL_SQS_CODES.contains(&code) {
                SinkError::Refused { category, reason }
            } else {
                SinkError::Unavailable { category, reason }
            }
        }
        _ => SinkError::Unavailable { category, reason },
    }
}

/// One category-fenced producer for the shared billing FIFO.
///
/// The linked category and queue URL cannot change after construction. A caller
/// must also supply the category on every emission. The queue is shared because
/// billing orders by organization, while the sink and draft retain the category
/// fence that prevents a producer from relabelling a meter.
#[derive(Debug, Clone)]
pub struct SqsFactDraftSink {
    client: Client,
    queue_url: String,
    category: UsageCategory,
}

impl SqsFactDraftSink {
    /// Binds one producer to exactly one authority ingress queue.
    pub fn new(client: Client, queue_url: impl Into<String>, category: UsageCategory) -> Self {
        Self {
            client,
            queue_url: queue_url.into(),
            category,
        }
    }

    /// Queue URL used by startup probes.
    #[must_use]
    pub fn queue_url(&self) -> &str {
        &self.queue_url
    }

    fn message(
        &self,
        category: UsageCategory,
        draft: FactDraft,
    ) -> Result<OutboxMessage, SinkError> {
        if category != self.category {
            return Err(SinkError::Refused {
                category,
                reason: format!(
                    "the call addressed {category}, but this sink is bound to {}",
                    self.category
                ),
            });
        }
        let accepted_position = draft
            .authority
            .segment_ordinal
            .get()
            .checked_add(1)
            .ok_or_else(|| SinkError::Refused {
                category,
                reason: "usage segment ordinal exhausted".to_owned(),
            })?;
        let accepted_sequence =
            AcceptedSequence::new(accepted_position).map_err(|error| SinkError::Refused {
                category,
                reason: error.to_string(),
            })?;
        let accepted_at =
            system_timestamp().map_err(|reason| SinkError::Refused { category, reason })?;
        let fact = draft
            .admit(accepted_sequence, accepted_at)
            .map_err(|error| SinkError::Refused {
                category,
                reason: error.to_string(),
            })?;
        OutboxMessage::for_fact(&fact).map_err(|error| SinkError::Refused {
            category,
            reason: error.to_string(),
        })
    }

    fn body(&self, message: &OutboxMessage) -> Result<String, SinkError> {
        let body = serde_json::to_string(&message.body).map_err(|error| SinkError::Refused {
            category: self.category,
            reason: format!("billing fact is not serializable: {error}"),
        })?;
        if body.len() > SQS_BODY_MAX_BYTES {
            return Err(SinkError::Refused {
                category: self.category,
                reason: format!(
                    "encoded draft is {} bytes, over the {SQS_BODY_MAX_BYTES}-byte SQS ceiling",
                    body.len()
                ),
            });
        }
        Ok(body)
    }
}

impl UsageFactSink for SqsFactDraftSink {
    fn emit<'a>(
        &'a self,
        category: UsageCategory,
        draft: FactDraft,
    ) -> Pin<Box<dyn Future<Output = Result<(), SinkError>> + Send + 'a>> {
        Box::pin(async move {
            let message = self.message(category, draft)?;
            let body = self.body(&message)?;
            self.client
                .send_message()
                .queue_url(&self.queue_url)
                .message_body(body)
                .message_group_id(message.message_group_id)
                .message_deduplication_id(message.message_deduplication_id)
                .send()
                .await
                .map_err(|error| classify(category, &error))?;
            Ok(())
        })
    }
}

fn system_timestamp() -> Result<Timestamp, String> {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| format!("system clock precedes the Unix epoch: {error}"))?
        .as_millis();
    let millis = i64::try_from(millis).map_err(|_| "system clock exceeds i64 milliseconds")?;
    Timestamp::from_unix_millis(millis).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aex_usage_domain::fact::{
        Attribution, FactKind, ResourceGeneration, ResourceKind, SCHEMA_VERSION,
    };
    use aex_usage_domain::identity::{AuthorityId, AuthorityKey, AuthorityKind, SegmentOrdinal};
    use aex_usage_domain::measurement::{
        BoundaryId, Evidence, FactBasis, Measurement, ReceiptKind, ServiceTime, SourceReceipt,
    };
    use aex_usage_domain::meter::{Category, Meter};
    use aex_usage_domain::wire_pending::{
        OrganizationId, PricingVersion, RegionId, ServiceId, Timestamp, WorkspaceId,
    };
    use aex_wire::ids::PrefixedId as _;

    fn draft(category: Category) -> FactDraft {
        let at = Timestamp::from_unix_millis(0).expect("time");
        let organization =
            aex_wire::ids::OrganizationId::from_uuid7(aex_wire::ids::Uuid7::compose(1, [1; 10]))
                .encode();
        let workspace =
            aex_wire::ids::WorkspaceId::from_uuid7(aex_wire::ids::Uuid7::compose(2, [2; 10]))
                .encode();
        FactDraft {
            schema_version: SCHEMA_VERSION,
            organization: OrganizationId::parse(organization.as_str()).expect("org"),
            workspace: WorkspaceId::parse(workspace.as_str()).expect("workspace"),
            region: RegionId::parse("eu-west-1").expect("region"),
            attribution: Attribution::default(),
            service: ServiceId::parse("runtime-control-worker").expect("service"),
            resource: ResourceGeneration {
                kind: ResourceKind::HandsGeneration,
                generation: "gen-1".into(),
            },
            authority: AuthorityKey {
                region: RegionId::parse("eu-west-1").expect("region"),
                category,
                kind: AuthorityKind::EgressCrossing,
                authority_id: AuthorityId::parse("cross-1").expect("authority"),
                segment_ordinal: SegmentOrdinal::FIRST,
            },
            pricing_version: PricingVersion::parse("2026-08-01").expect("pricing"),
            reservation: None,
            kind: FactKind::Measured(
                Measurement::new(
                    Meter::DataTransferEgressByte,
                    FactBasis::Consumed,
                    ServiceTime::Instant { at },
                    SourceReceipt {
                        kind: ReceiptKind::DeliveryLog,
                        id: "r-1".into(),
                        digest: None,
                    },
                    Evidence::DeliveryReceipt {
                        boundary: BoundaryId::HANDS_EGRESS,
                        receipt_id: "r-1".into(),
                        bytes: 1,
                    },
                )
                .expect("measurement"),
            ),
        }
    }

    #[test]
    fn encoded_body_is_the_central_rating_contract() {
        let config = aws_sdk_sqs::Config::builder()
            .behavior_version_latest()
            .build();
        let sink = SqsFactDraftSink::new(
            Client::from_conf(config),
            "https://sqs.eu-west-1.amazonaws.com/1/compute",
            Category::Transfer,
        );
        let transfer = draft(Category::Transfer);
        let message = sink.message(Category::Transfer, transfer).expect("message");
        let body = sink.body(&message).expect("body");
        let decoded: aex_internal_contracts::usage::RatingRequest =
            serde_json::from_str(&body).expect("rating request");
        assert_eq!(decoded.fact.meter.category(), "transfer");
        assert_eq!(
            message.message_group_id,
            decoded.fact.organization.encode().as_str()
        );
    }

    #[test]
    fn sibling_category_is_refused_before_any_network_call() {
        let config = aws_sdk_sqs::Config::builder()
            .behavior_version_latest()
            .build();
        let sink = SqsFactDraftSink::new(Client::from_conf(config), "unused", Category::Compute);
        assert!(matches!(
            sink.message(Category::Storage, draft(Category::Transfer)),
            Err(SinkError::Refused { .. })
        ));
    }
}
