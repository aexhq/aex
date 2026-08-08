//! Category-scoped SQS producers for canonical usage drafts.

use std::future::Future;
use std::pin::Pin;

use aex_runtime_control::usage::{SinkError, UsageCategory, UsageFactSink};
use aex_usage_domain::fact::FactDraft;
use aex_usage_domain::ingress::FactDraftEnvelope;
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
/// `MissingParameter` is here for a specific trap: neither ingress queue is
/// FIFO today and this producer sends no `MessageGroupId`. If either queue is
/// ever recreated as `.fifo`, every send fails with that code forever.
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

/// One category's queue producer.
///
/// The linked category and queue URL cannot change after construction. A caller
/// must also supply the category on every emission, giving three category fences:
/// sink binding, envelope, and draft authority key.
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

    fn body(&self, category: UsageCategory, draft: FactDraft) -> Result<String, SinkError> {
        if category != self.category {
            return Err(SinkError::Refused {
                category,
                reason: format!(
                    "the call addressed {category}, but this sink is bound to {}",
                    self.category
                ),
            });
        }
        let envelope =
            FactDraftEnvelope::new(category, draft).map_err(|error| SinkError::Refused {
                category,
                reason: error.to_string(),
            })?;
        let body = serde_json::to_string(&envelope).map_err(|error| SinkError::Refused {
            category,
            reason: format!("draft is not serializable: {error}"),
        })?;
        if body.len() > SQS_BODY_MAX_BYTES {
            return Err(SinkError::Refused {
                category,
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
            let body = self.body(category, draft)?;
            self.client
                .send_message()
                .queue_url(&self.queue_url)
                .message_body(body)
                .send()
                .await
                .map_err(|error| classify(category, &error))?;
            Ok(())
        })
    }
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

    fn draft(category: Category) -> FactDraft {
        let at = Timestamp::from_unix_millis(0).expect("time");
        FactDraft {
            schema_version: SCHEMA_VERSION,
            organization: OrganizationId::parse("org-1").expect("org"),
            workspace: WorkspaceId::parse("ws-1").expect("workspace"),
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
    fn encoded_body_is_the_strict_canonical_envelope() {
        let config = aws_sdk_sqs::Config::builder()
            .behavior_version_latest()
            .build();
        let sink = SqsFactDraftSink::new(
            Client::from_conf(config),
            "https://sqs.eu-west-1.amazonaws.com/1/compute",
            Category::Compute,
        );
        let mut compute = draft(Category::Compute);
        compute.authority.kind = AuthorityKind::HandsGeneration;
        let body = sink.body(Category::Compute, compute).expect("body");
        let value: serde_json::Value = serde_json::from_str(&body).expect("json");
        assert_eq!(value["type"], "usage_fact_draft.v1");
        assert_eq!(value["category"], "compute");
    }

    #[test]
    fn sibling_category_is_refused_before_any_network_call() {
        let config = aws_sdk_sqs::Config::builder()
            .behavior_version_latest()
            .build();
        let sink = SqsFactDraftSink::new(Client::from_conf(config), "unused", Category::Compute);
        assert!(matches!(
            sink.body(Category::Storage, draft(Category::Storage)),
            Err(SinkError::Refused { .. })
        ));
    }
}
