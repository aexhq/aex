//! The direct-invoke request and response shapes.
//!
//! `finance-ingest` is not an HTTP surface: the webhook edge invokes it with
//! `InvocationType=RequestResponse` and renders its answer. The probe arms are
//! this deployable's equivalent of `/internal/healthz` and `/internal/readyz`,
//! which a Lambda that serves no HTTP cannot expose as routes.

use std::sync::Arc;

use aex_payment_contracts::ProviderEventEnvelope;
use serde::{Deserialize, Serialize};

use crate::inbox::{AppliedState, IngestError, ProviderEventInbox};

/// What the caller may ask for.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "request", rename_all = "snake_case", deny_unknown_fields)]
pub enum IngestRequest {
    /// Settle one verified provider event.
    ProviderEvent {
        /// The normalized event, exactly as the edge produced it.
        event: Box<ProviderEventEnvelope>,
    },
    /// Liveness: the process is running and can answer.
    Healthz,
    /// Readiness: this deployable has proved its own database grants.
    Readyz,
}

/// What the caller receives.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub enum IngestResponse {
    /// The event is durable.
    Accepted {
        /// How the event was settled.
        applied: String,
        /// Whether this call is the one that made it durable.
        first_delivery: bool,
        /// The journal transaction it posted, when it posted one.
        #[serde(skip_serializing_if = "Option::is_none")]
        transaction_id: Option<String>,
    },
    /// The process is running.
    Healthy,
    /// The process has proved its own grants.
    Ready {
        /// Whether traffic may be served.
        ready: bool,
        /// Why readiness is not held, when it is not.
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

/// The durable spelling of a settled state.
#[must_use]
pub const fn applied_label(applied: AppliedState) -> &'static str {
    applied.as_str()
}

/// Handles one invocation.
///
/// # Errors
///
/// Returns the ingest failure unchanged. The edge renders any error as a
/// non-`2xx`, which is what makes Stripe redeliver rather than the event being
/// silently dropped.
pub async fn handle<I: ProviderEventInbox>(
    inbox: &Arc<I>,
    request: IngestRequest,
) -> Result<IngestResponse, IngestError> {
    match request {
        IngestRequest::Healthz => Ok(IngestResponse::Healthy),
        IngestRequest::Readyz => match inbox.probe_role().await {
            Ok(()) => Ok(IngestResponse::Ready {
                ready: true,
                reason: None,
            }),
            Err(error) => Ok(IngestResponse::Ready {
                ready: false,
                reason: Some(error.to_string()),
            }),
        },
        IngestRequest::ProviderEvent { event } => {
            let outcome = inbox.ingest(&event).await?;
            Ok(IngestResponse::Accepted {
                applied: applied_label(outcome.applied).to_owned(),
                first_delivery: outcome.first_delivery,
                transaction_id: outcome.transaction_id.map(|id| id.to_string()),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use aex_payment_contracts::{
        PinnedApiVersion, ProviderEventEnvelope, ProviderEventFacts, ProviderEventId,
        ProviderObjectRef,
    };
    use aex_wire::PrefixedId as _;
    use aex_wire::ids::{ContentHash, OrganizationId};
    use aex_wire::types::{Cents, Timestamp};

    use super::{IngestRequest, IngestResponse, handle};
    use crate::inbox::{AppliedState, IngestError, IngestOutcome, ProviderEventInbox};

    /// An inbox whose answer the case chooses.
    #[derive(Debug)]
    struct Scripted {
        answer: Result<IngestOutcome, IngestError>,
        probe: Result<(), IngestError>,
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl ProviderEventInbox for Scripted {
        async fn probe_role(&self) -> Result<(), IngestError> {
            self.probe.clone()
        }

        async fn ingest(
            &self,
            _event: &ProviderEventEnvelope,
        ) -> Result<IngestOutcome, IngestError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.answer.clone()
        }
    }

    fn event() -> Box<ProviderEventEnvelope> {
        let organization =
            OrganizationId::parse("org_01kyw2qa4pew48j2gb1g6gw3rg").expect("an organization");
        let facts = ProviderEventFacts::PaymentIntentSucceeded {
            organization,
            credit: Cents::new(2_000),
            charged: Cents::new(2_000),
            intent: ProviderObjectRef("pi_abcdefghij".to_owned()),
        };
        Box::new(ProviderEventEnvelope {
            schema_version: aex_internal_contracts::SchemaVersion::V1,
            provider_event_id: ProviderEventId("evt_abcdefghij".to_owned()),
            object: ProviderObjectRef("pi_abcdefghij".to_owned()),
            kind: facts.kind(),
            occurred_at: Timestamp::from_unix_millis(1_800_000_000_000).expect("an instant"),
            facts,
            raw_digest: ContentHash::from_bytes([9u8; 32]),
            provider_api_version: PinnedApiVersion("2026-06-24.dahlia".to_owned()),
            effect: None,
            received_at: Timestamp::from_unix_millis(1_800_000_000_001).expect("an instant"),
        })
    }

    fn scripted(answer: Result<IngestOutcome, IngestError>) -> Arc<Scripted> {
        Arc::new(Scripted {
            answer,
            probe: Ok(()),
            calls: AtomicUsize::new(0),
        })
    }

    #[tokio::test]
    async fn success_is_answered_only_after_a_durable_handoff() {
        let inbox = scripted(Ok(IngestOutcome {
            applied: AppliedState::Applied,
            first_delivery: true,
            transaction_id: Some(uuid::Uuid::now_v7()),
        }));
        let answer = handle(&inbox, IngestRequest::ProviderEvent { event: event() })
            .await
            .expect("a durable handoff answers");
        match answer {
            IngestResponse::Accepted {
                applied,
                first_delivery,
                transaction_id,
            } => {
                assert_eq!(applied, "applied");
                assert!(first_delivery);
                assert!(
                    transaction_id.is_some(),
                    "a money event posts a transaction"
                );
            }
            other => panic!("expected an acceptance, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_unknown_commit_outcome_is_an_error_rather_than_a_2xx() {
        let inbox = scripted(Err(IngestError::OutcomeUnknown("lost response".to_owned())));
        let error = handle(&inbox, IngestRequest::ProviderEvent { event: event() })
            .await
            .expect_err("an unknown outcome never answers success");
        assert!(matches!(error, IngestError::OutcomeUnknown(_)));
    }

    #[tokio::test]
    async fn an_unavailable_authority_is_an_error_so_the_provider_redelivers() {
        let inbox = scripted(Err(IngestError::Unavailable("no route".to_owned())));
        assert!(
            handle(&inbox, IngestRequest::ProviderEvent { event: event() })
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_duplicate_delivery_answers_without_a_second_transaction() {
        let inbox = scripted(Ok(IngestOutcome {
            applied: AppliedState::Applied,
            first_delivery: false,
            transaction_id: None,
        }));
        let answer = handle(&inbox, IngestRequest::ProviderEvent { event: event() })
            .await
            .expect("a duplicate is a success");
        assert!(matches!(
            answer,
            IngestResponse::Accepted {
                first_delivery: false,
                transaction_id: None,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn the_probe_arms_answer_different_questions() {
        let healthy = Arc::new(Scripted {
            answer: Ok(IngestOutcome {
                applied: AppliedState::Applied,
                first_delivery: true,
                transaction_id: None,
            }),
            probe: Err(IngestError::Unavailable("no grant".to_owned())),
            calls: AtomicUsize::new(0),
        });
        assert_eq!(
            handle(&healthy, IngestRequest::Healthz)
                .await
                .expect("liveness answers"),
            IngestResponse::Healthy
        );
        match handle(&healthy, IngestRequest::Readyz)
            .await
            .expect("readiness answers")
        {
            IngestResponse::Ready { ready, reason } => {
                assert!(!ready);
                assert!(reason.is_some_and(|reason| reason.contains("no grant")));
            }
            other => panic!("expected a readiness answer, got {other:?}"),
        }
    }

    #[test]
    fn the_request_union_admits_no_arm_this_deployable_does_not_serve() {
        for raw in [
            r#"{"request":"delete_journal"}"#,
            r#"{"request":"provider_event"}"#,
            r#"{}"#,
        ] {
            assert!(
                serde_json::from_str::<IngestRequest>(raw).is_err(),
                "{raw} must not decode"
            );
        }
    }
}
