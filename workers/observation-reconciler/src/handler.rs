//! The Lambda entry surface: one handler over three invocation shapes.
//!
//! This deployable is driven by an `EventBridge` schedule **and** by `SQS`, and
//! it also answers the two cross-stream health paths. It has no listener, so the
//! paths are matched against
//! [`aex_observation_store_dynamodb::health::HEALTHZ`] and
//! [`aex_observation_store_dynamodb::health::READYZ`] rather than mounted; the
//! constants are the single source and no path is ever spelled out here.
//!
//! A queue-driven invocation answers with an **SQS partial-batch failure body**
//! instead of throwing: throwing would re-drive every record in the batch,
//! including the ones whose duty is already durably resolved.

use aex_observation_store_dynamodb::health::{HEALTHZ, Probe, READYZ};
use aex_wire::types::Timestamp;

use crate::duty::{BatchOutcome, DutyEngine, DutyError, ItemId, ItemKey, QueuedRecord};
use crate::health::{health_body, readiness_body};

/// The three shapes one invocation can carry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Invocation {
    /// A liveness probe.
    Health,
    /// A readiness probe.
    Readiness,
    /// Queue records, each answered separately.
    Records(Vec<QueuedRecord>),
    /// The scheduled due-scan.
    Scheduled,
}

/// Classifies one raw Lambda payload.
///
/// An event that is neither a probe nor a queue delivery is the schedule: an
/// empty payload and an `EventBridge` scheduled event both run one bounded scan.
#[must_use]
pub fn classify(event: &serde_json::Value) -> Invocation {
    if let Some(path) = probe_path(event) {
        if path == HEALTHZ {
            return Invocation::Health;
        }
        if path == READYZ {
            return Invocation::Readiness;
        }
    }
    match event.get("Records").and_then(serde_json::Value::as_array) {
        Some(records) if !records.is_empty() => {
            Invocation::Records(records.iter().map(queued_record).collect())
        }
        _ => Invocation::Scheduled,
    }
}

/// The path a probe invocation names, under either payload spelling.
fn probe_path(event: &serde_json::Value) -> Option<&str> {
    event
        .get("rawPath")
        .or_else(|| event.get("path"))
        .and_then(serde_json::Value::as_str)
}

/// Reads one queue record, keeping an unreadable body as a named failure.
fn queued_record(record: &serde_json::Value) -> QueuedRecord {
    let message_id = record
        .get("messageId")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    QueuedRecord {
        message_id: ItemId::new(message_id),
        target: record
            .get("body")
            .and_then(serde_json::Value::as_str)
            .and_then(|body| serde_json::from_str::<serde_json::Value>(body).ok())
            .as_ref()
            .and_then(item_key),
    }
}

/// The item a record body names.
fn item_key(body: &serde_json::Value) -> Option<ItemKey> {
    Some(ItemKey {
        pk: body.get("pk")?.as_str()?.to_owned(),
        sk: body.get("sk")?.as_str()?.to_owned(),
    })
}

/// Renders the SQS partial-batch failure body.
///
/// Exactly the failed identifiers appear; a succeeded item is never listed,
/// because listing it would re-drive a duty that is already durably resolved.
#[must_use]
pub fn partial_batch_body(outcome: &BatchOutcome) -> serde_json::Value {
    serde_json::json!({
        "batchItemFailures": outcome
            .failed
            .iter()
            .map(|(id, _)| serde_json::json!({ "itemIdentifier": id.as_str() }))
            .collect::<Vec<serde_json::Value>>(),
    })
}

/// Everything one invocation needs, resolved once at start-up.
#[derive(Clone, Debug)]
pub struct Handler {
    engine: DutyEngine,
    release_digest: String,
    passed: Vec<Probe>,
}

impl Handler {
    /// Binds the handler to a composed engine and the probes that actually
    /// passed.
    #[must_use]
    pub fn new(engine: DutyEngine, release_digest: String, passed: Vec<Probe>) -> Self {
        Self {
            engine,
            release_digest,
            passed,
        }
    }

    /// Answers one invocation.
    ///
    /// # Errors
    ///
    /// Returns [`DutyError`] only when the **scheduled** due-scan itself fails,
    /// which is a whole-invocation failure the schedule re-drives. A queue
    /// delivery never throws: it answers with the partial-batch body.
    pub async fn respond(
        &self,
        event: &serde_json::Value,
        now: Timestamp,
    ) -> Result<serde_json::Value, DutyError> {
        match classify(event) {
            Invocation::Health => Ok(rendered(
                crate::health::READY_STATUS,
                &health_body(&self.release_digest),
            )),
            Invocation::Readiness => {
                let (status, body) = readiness_body(&self.release_digest, &self.passed);
                Ok(rendered(status, &body))
            }
            Invocation::Records(records) => {
                let outcome = self.engine.run_records(&records, now).await;
                Ok(partial_batch_body(&outcome))
            }
            Invocation::Scheduled => {
                let outcome = self.engine.run_scheduled(now).await?;
                Ok(partial_batch_body(&outcome))
            }
        }
    }
}

/// One probe answer in the Lambda proxy shape.
fn rendered(status: u16, body: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "statusCode": status,
        "headers": { "content-type": "application/json", "cache-control": "no-store" },
        "body": body.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use aex_observation_store_dynamodb::health::{HEALTHZ, READYZ};

    use super::{Invocation, classify, partial_batch_body, rendered};
    use crate::duty::{BatchOutcome, ItemId};
    use crate::health::health_body;

    #[test]
    fn an_unknown_probe_path_is_the_schedule_rather_than_a_probe() {
        assert_eq!(
            classify(&serde_json::json!({ "rawPath": "/anything-else" })),
            Invocation::Scheduled
        );
        assert_eq!(
            classify(&serde_json::json!({ "rawPath": HEALTHZ })),
            Invocation::Health
        );
        assert_eq!(
            classify(&serde_json::json!({ "path": READYZ })),
            Invocation::Readiness
        );
    }

    #[test]
    fn an_empty_record_list_is_the_schedule_rather_than_an_empty_batch() {
        assert_eq!(
            classify(&serde_json::json!({ "Records": [] })),
            Invocation::Scheduled
        );
    }

    #[test]
    fn a_record_without_a_message_id_still_gets_a_verdict() {
        let event = serde_json::json!({ "Records": [{ "body": "{}" }] });
        match classify(&event) {
            Invocation::Records(records) => {
                assert_eq!(records.len(), 1);
                assert_eq!(records[0].message_id.as_str(), "");
                assert!(records[0].target.is_none());
            }
            other => panic!("expected queue records, got {other:?}"),
        }
    }

    #[test]
    fn a_clean_batch_still_carries_the_failure_array() {
        let mut outcome = BatchOutcome::default();
        outcome.succeed(ItemId::new("only"));
        let body = partial_batch_body(&outcome);
        assert!(
            body["batchItemFailures"]
                .as_array()
                .expect("always present")
                .is_empty()
        );
    }

    #[test]
    fn a_probe_answer_carries_its_status_and_a_no_store_header() {
        let answer = rendered(200, &health_body("d"));
        assert_eq!(answer["statusCode"].as_i64(), Some(200));
        assert_eq!(
            answer["headers"]["cache-control"].as_str(),
            Some("no-store")
        );
        assert!(
            answer["body"]
                .as_str()
                .expect("a rendered body")
                .contains("healthy")
        );
    }
}
