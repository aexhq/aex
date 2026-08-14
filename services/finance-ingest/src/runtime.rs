//! Total dispatch for the three event sources owned by `billing-worker`.

use aws_lambda_events::sqs::SqsEvent;
use serde_json::Value;

use crate::handler::IngestRequest;
use crate::reconcile::handler::ReconcileRequest;

/// One admitted invocation mode.
#[derive(Debug)]
pub enum Invocation {
    /// Verified provider event or liveness probe from the Stripe edge.
    Ingest(IngestRequest),
    /// A FIFO usage-rating batch.
    Settlement(SqsEvent),
    /// A scheduled conservation/effect sweep.
    Reconcile(ReconcileRequest),
    /// Readiness across every internal money role.
    Readyz,
}

/// Why an invocation was not one of the worker's exact event sources.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvocationError {
    /// An event source envelope was recognizable but malformed.
    #[error("invalid {kind} invocation: {detail}")]
    Invalid {
        /// Event source that was recognized.
        kind: &'static str,
        /// Decode failure.
        detail: String,
    },
    /// The request discriminant is outside the billing contract.
    #[error("unsupported billing-worker invocation")]
    Unsupported,
}

/// Classifies raw Lambda JSON without letting one event source impersonate
/// another. SQS is recognized by its `Records` envelope; direct requests use
/// their explicit `request` tag; an ordinary EventBridge envelope means sweep.
pub fn classify(value: Value) -> Result<Invocation, InvocationError> {
    if value.get("Records").is_some() {
        return serde_json::from_value(value)
            .map(Invocation::Settlement)
            .map_err(|error| InvocationError::Invalid {
                kind: "SQS settlement",
                detail: error.to_string(),
            });
    }

    match value.get("request").and_then(Value::as_str) {
        Some("provider_event" | "healthz") => serde_json::from_value(value)
            .map(Invocation::Ingest)
            .map_err(|error| InvocationError::Invalid {
                kind: "provider ingest",
                detail: error.to_string(),
            }),
        Some("readyz") => Ok(Invocation::Readyz),
        Some("sweep") => Ok(Invocation::Reconcile(ReconcileRequest::Sweep)),
        Some(_) => Err(InvocationError::Unsupported),
        None if value.get("source").is_some() || value.get("detail-type").is_some() => {
            Ok(Invocation::Reconcile(ReconcileRequest::Sweep))
        }
        None => Err(InvocationError::Unsupported),
    }
}

#[cfg(test)]
mod tests {
    use aws_lambda_events::sqs::SqsEvent;
    use serde_json::json;

    use super::{Invocation, InvocationError, classify};

    #[test]
    fn the_three_event_sources_are_disjoint() {
        assert!(matches!(
            classify(json!({"request": "healthz"})),
            Ok(Invocation::Ingest(_))
        ));
        assert!(matches!(
            classify(serde_json::to_value(SqsEvent::default()).expect("SQS serializes")),
            Ok(Invocation::Settlement(_))
        ));
        assert!(matches!(
            classify(json!({"source": "aws.events", "detail-type": "Scheduled Event"})),
            Ok(Invocation::Reconcile(_))
        ));
    }

    #[test]
    fn readiness_covers_the_whole_worker() {
        assert!(matches!(
            classify(json!({"request": "readyz"})),
            Ok(Invocation::Readyz)
        ));
    }

    #[test]
    fn unknown_direct_requests_fail_closed() {
        assert_eq!(
            classify(json!({"request": "repair_balance"})).expect_err("not admitted"),
            InvocationError::Unsupported
        );
    }
}
