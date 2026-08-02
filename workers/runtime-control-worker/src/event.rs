//! The two entry points, and the envelopes they arrive in.
//!
//! One handler serves both: a queue delivery of lifecycle commands, and a
//! scheduled sweep of one shard of the due index. They are told apart by shape
//! rather than by an environment variable, so a single deployable cannot be
//! mis-wired into answering the wrong one.

use aex_runtime_control::store::RuntimeShard;
use aex_runtime_control_aws::queue::Quarantined;
use aex_runtime_control_aws::worker::{CommandOutcome, QueueRecord, RuntimeControl, SchedulePass};
use aex_wire::types::Timestamp;
use aws_lambda_events::sqs::SqsEvent;
use axum::body::Body;
use axum::http::Request;
use serde::{Deserialize, Serialize};
use tower::ServiceExt as _;

use crate::health::{self, Bindings};

/// The SQS attribute carrying the delivery count.
const RECEIVE_COUNT_ATTRIBUTE: &str = "ApproximateReceiveCount";

/// The largest probe body the internal surface may return.
const MAX_PROBE_BODY_BYTES: usize = 64 * 1024;

/// Why an invocation envelope was refused.
///
/// Every arm fails the whole invocation. A malformed envelope is not a poison
/// *message* — there is no message identity to quarantine — so the only honest
/// response is to fail loudly and let the delivery redrive.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EventError {
    /// The payload matched neither entry point.
    #[error(
        "the invocation payload is not an SQS batch, a `{{\"runtimeSweep\": n}}` schedule or an          `{{\"internal\": path}}` probe"
    )]
    Unrecognized,
    /// The payload matched an entry point but did not decode.
    #[error("the invocation payload did not decode: {reason}")]
    Undecodable {
        /// Why.
        reason: String,
    },
    /// A record in the batch was missing something the response needs.
    #[error("SQS record {index} carries no `{field}`; a partial-batch response needs it")]
    IncompleteRecord {
        /// Which record.
        index: usize,
        /// What was absent.
        field: &'static str,
    },
}

/// What arrived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerEvent {
    /// A batch of lifecycle commands.
    Queue(Box<SqsEvent>),
    /// A scheduled sweep of one shard of the due index.
    Sweep {
        /// Which shard.
        shard: RuntimeShard,
    },
    /// A synthetic health probe.
    ///
    /// A Lambda has no listening socket, so the workspace-wide `/internal/healthz`
    /// and `/internal/readyz` surface is reached by invoking the function with the
    /// path. The router is the same one an ALB would target if this deployable ever
    /// ran as a task, so the two cannot answer differently.
    Internal {
        /// Which internal path.
        path: String,
    },
}

/// The synthetic health-probe payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct InternalPayload {
    /// The internal path to probe.
    internal: String,
}

/// The scheduled sweep payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SweepPayload {
    /// Which shard of the due index this invocation sweeps.
    runtime_sweep: u16,
}

impl WorkerEvent {
    /// Decides which entry point a payload is for.
    ///
    /// # Errors
    ///
    /// See [`EventError`].
    pub fn decode(payload: &serde_json::Value) -> Result<Self, EventError> {
        if payload.get("Records").is_some() {
            let event: SqsEvent = serde_json::from_value(payload.clone()).map_err(|error| {
                EventError::Undecodable {
                    reason: error.to_string(),
                }
            })?;
            return Ok(Self::Queue(Box::new(event)));
        }
        if payload.get("internal").is_some() {
            let probe: InternalPayload =
                serde_json::from_value(payload.clone()).map_err(|error| {
                    EventError::Undecodable {
                        reason: error.to_string(),
                    }
                })?;
            return Ok(Self::Internal {
                path: probe.internal,
            });
        }
        if payload.get("runtimeSweep").is_some() {
            let sweep: SweepPayload = serde_json::from_value(payload.clone()).map_err(|error| {
                EventError::Undecodable {
                    reason: error.to_string(),
                }
            })?;
            return Ok(Self::Sweep {
                shard: RuntimeShard(sweep.runtime_sweep),
            });
        }
        Err(EventError::Unrecognized)
    }
}

/// One SQS batch, reduced to what the engine needs.
///
/// # Errors
///
/// Returns [`EventError::IncompleteRecord`] when a record carries no message id,
/// no body, or no delivery count. None of the three can be defaulted: without an
/// id the failure cannot be reported, without a body there is nothing to run, and
/// without a count the poison budget cannot be evaluated.
pub fn queue_records(event: &SqsEvent) -> Result<Vec<QueueRecord>, EventError> {
    event
        .records
        .iter()
        .enumerate()
        .map(|(index, message)| {
            let message_id = message
                .message_id
                .clone()
                .ok_or(EventError::IncompleteRecord {
                    index,
                    field: "messageId",
                })?;
            let body = message.body.clone().ok_or(EventError::IncompleteRecord {
                index,
                field: "body",
            })?;
            let receive_count = message
                .attributes
                .get(RECEIVE_COUNT_ATTRIBUTE)
                .and_then(|raw| raw.parse::<u32>().ok())
                .ok_or(EventError::IncompleteRecord {
                    index,
                    field: RECEIVE_COUNT_ATTRIBUTE,
                })?;
            Ok(QueueRecord {
                message_id,
                receive_count,
                body,
            })
        })
        .collect()
}

/// What one sweep invocation did, as the schedule's response body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SweepSummary {
    /// Which shard was swept.
    pub shard: u16,
    /// How many due generations the scan returned.
    pub scanned: u32,
    /// How many evaluations needed nothing further.
    pub settled: u32,
    /// How many will be looked at again.
    pub retry: u32,
    /// How many quarantined.
    pub poison: u32,
    /// The cursor for the next page, absent when the shard is exhausted.
    pub cursor: Option<String>,
    /// Why the scan itself failed, when it did.
    pub scan_failure: Option<String>,
}

impl SweepSummary {
    /// Folds one pass into its summary.
    #[must_use]
    pub fn of(shard: RuntimeShard, pass: &SchedulePass) -> Self {
        let count = |wanted: fn(&CommandOutcome) -> bool| {
            u32::try_from(
                pass.outcomes
                    .iter()
                    .filter(|(_, outcome)| wanted(outcome))
                    .count(),
            )
            .unwrap_or(u32::MAX)
        };
        Self {
            shard: shard.0,
            scanned: pass.scanned,
            settled: count(|outcome| matches!(outcome, CommandOutcome::Settled(_))),
            retry: count(|outcome| matches!(outcome, CommandOutcome::Retry { .. })),
            poison: count(|outcome| matches!(outcome, CommandOutcome::Poison { .. })),
            cursor: pass.cursor.clone(),
            scan_failure: pass.scan_failure.clone(),
        }
    }
}

/// Turns scheduled poison outcomes into the same operator diagnostics emitted
/// for poisoned queue records.
fn schedule_quarantines(pass: &SchedulePass) -> Vec<Quarantined> {
    pass.outcomes
        .iter()
        .filter_map(|(generation, outcome)| match outcome {
            CommandOutcome::Poison { reason } => Some(Quarantined {
                message_id: format!("runtime-sweep:{generation}"),
                reason: reason.clone(),
                receive_count: 1,
            }),
            _ => None,
        })
        .collect()
}

/// What one invocation produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handled {
    /// The response body the service reads.
    pub response: serde_json::Value,
    /// Items that were quarantined. Each needs an operator record and an alarm,
    /// and none of them is ever redriven hot.
    pub quarantined: Vec<Quarantined>,
}

/// Serves the internal health surface.
///
/// A Lambda has no listening socket, so the probe is an invocation. It is answered
/// by the same router an ALB would target, so the two cannot answer differently.
///
/// # Errors
///
/// See [`EventError`].
pub async fn probe(bindings: &Bindings, path: &str) -> Result<Handled, EventError> {
    let request =
        Request::get(path)
            .body(Body::empty())
            .map_err(|error| EventError::Undecodable {
                reason: error.to_string(),
            })?;
    let response = health::router(bindings.clone())
        .oneshot(request)
        .await
        .map_err(|error| EventError::Undecodable {
            reason: error.to_string(),
        })?;
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), MAX_PROBE_BODY_BYTES)
        .await
        .map_err(|error| EventError::Undecodable {
            reason: error.to_string(),
        })?;
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    Ok(Handled {
        response: serde_json::json!({ "status": status, "body": body }),
        quarantined: Vec::new(),
    })
}

/// Serves one invocation.
///
/// # Errors
///
/// See [`EventError`]. A response that cannot be serialized is impossible for
/// these two shapes and is reported as [`EventError::Undecodable`] rather than
/// unwrapped.
pub async fn handle(
    control: &RuntimeControl,
    bindings: &Bindings,
    event: &WorkerEvent,
    now: Timestamp,
) -> Result<Handled, EventError> {
    match event {
        WorkerEvent::Internal { path } => probe(bindings, path).await,
        WorkerEvent::Queue(batch) => {
            let records = queue_records(batch)?;
            let result = control.handle_queue(&records, now).await;
            Ok(Handled {
                response: serde_json::to_value(&result.response).map_err(|error| {
                    EventError::Undecodable {
                        reason: error.to_string(),
                    }
                })?,
                quarantined: result.quarantined,
            })
        }
        WorkerEvent::Sweep { shard } => {
            let pass = control.handle_schedule(*shard, now).await;
            let summary = SweepSummary::of(*shard, &pass);
            let quarantined = schedule_quarantines(&pass);
            Ok(Handled {
                response: serde_json::to_value(&summary).map_err(|error| {
                    EventError::Undecodable {
                        reason: error.to_string(),
                    }
                })?,
                quarantined,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EventError, SweepSummary, WorkerEvent, probe, queue_records, schedule_quarantines,
    };
    use crate::health::{Bindings, Dependency};
    use aex_runtime_control::store::RuntimeShard;
    use aex_runtime_control_aws::worker::{CommandOutcome, SchedulePass, Settled};
    use aex_wire::ids::{GenerationId, PrefixedId as _, Uuid7};
    use serde_json::json;

    fn sqs(records: &serde_json::Value) -> serde_json::Value {
        json!({ "Records": records })
    }

    fn record(id: &str, count: &str) -> serde_json::Value {
        json!({
            "messageId": id,
            "receiptHandle": "rh",
            "body": "{}",
            "attributes": { "ApproximateReceiveCount": count },
            "messageAttributes": {},
            "eventSource": "aws:sqs"
        })
    }

    #[test]
    fn the_entry_points_are_told_apart_by_shape() {
        let queue =
            WorkerEvent::decode(&sqs(&json!([record("a", "1")]))).expect("an SQS batch decodes");
        assert!(matches!(queue, WorkerEvent::Queue(_)));

        let sweep = WorkerEvent::decode(&json!({ "runtimeSweep": 3 })).expect("a sweep decodes");
        assert_eq!(
            sweep,
            WorkerEvent::Sweep {
                shard: RuntimeShard(3)
            }
        );
    }

    #[test]
    fn a_payload_matching_neither_entry_point_fails_the_invocation() {
        assert_eq!(
            WorkerEvent::decode(&json!({ "detail-type": "Scheduled Event" })),
            Err(EventError::Unrecognized),
            "guessing which entry point an unknown payload meant is how a sweep runs as a batch"
        );
    }

    #[test]
    fn a_sweep_payload_with_an_unknown_field_is_refused() {
        assert!(matches!(
            WorkerEvent::decode(&json!({ "runtimeSweep": 3, "force": true })),
            Err(EventError::Undecodable { .. })
        ));
    }

    #[test]
    fn every_field_a_partial_batch_response_needs_is_required() {
        for (field, payload) in [
            (
                "messageId",
                json!({"body": "{}", "attributes": {"ApproximateReceiveCount": "1"}}),
            ),
            (
                "body",
                json!({"messageId": "a", "attributes": {"ApproximateReceiveCount": "1"}}),
            ),
            (
                "ApproximateReceiveCount",
                json!({"messageId": "a", "body": "{}", "attributes": {}}),
            ),
        ] {
            let WorkerEvent::Queue(batch) =
                WorkerEvent::decode(&sqs(&json!([payload]))).expect("the envelope itself decodes")
            else {
                panic!("an SQS batch");
            };
            assert_eq!(
                queue_records(&batch),
                Err(EventError::IncompleteRecord { index: 0, field }),
                "none of the three can be defaulted"
            );
        }
    }

    #[test]
    fn a_complete_batch_reduces_to_exactly_what_the_engine_needs() {
        let WorkerEvent::Queue(batch) =
            WorkerEvent::decode(&sqs(&json!([record("a", "1"), record("b", "5")])))
                .expect("the batch decodes")
        else {
            panic!("an SQS batch");
        };
        let records = queue_records(&batch).expect("both records are complete");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].message_id, "a");
        assert_eq!(records[0].receive_count, 1);
        assert_eq!(records[1].receive_count, 5);
    }

    #[tokio::test]
    async fn the_internal_probe_answers_through_the_same_router_an_alb_would_target() {
        let unbound = probe(&Bindings::default(), "/internal/readyz")
            .await
            .expect("the probe answers");
        assert_eq!(unbound.response["status"], 503);
        assert_eq!(unbound.response["body"]["status"], "not_ready");
        assert_eq!(
            unbound.response["body"]["missing"]
                .as_array()
                .expect("a named list")
                .len(),
            5
        );

        let live = probe(&Bindings::default(), "/internal/healthz")
            .await
            .expect("the probe answers");
        assert_eq!(
            live.response["status"], 200,
            "liveness answers while readiness does not"
        );

        let bound = Dependency::ALL
            .into_iter()
            .fold(Bindings::default(), Bindings::with);
        let ready = probe(&bound, "/internal/readyz")
            .await
            .expect("the probe answers");
        assert_eq!(ready.response["status"], 200);
        assert_eq!(
            ready.response["body"]["usageCategories"],
            json!(["compute", "storage"])
        );
    }

    #[test]
    fn a_sweep_summary_counts_each_class_and_carries_the_cursor() {
        let generation = GenerationId::from_uuid7(Uuid7::compose(1, [1; 10]));
        let pass = SchedulePass {
            scanned: 3,
            outcomes: vec![
                (
                    generation,
                    CommandOutcome::Settled(Settled::NotMaterialized),
                ),
                (
                    generation,
                    CommandOutcome::Retry {
                        reason: "throttled".to_owned(),
                    },
                ),
                (
                    generation,
                    CommandOutcome::Poison {
                        reason: "invented generation".to_owned(),
                    },
                ),
            ],
            cursor: Some("next".to_owned()),
            scan_failure: None,
        };
        let summary = SweepSummary::of(RuntimeShard(2), &pass);
        assert_eq!(summary.shard, 2);
        assert_eq!(summary.scanned, 3);
        assert_eq!(summary.settled, 1);
        assert_eq!(summary.retry, 1);
        assert_eq!(summary.poison, 1);
        assert_eq!(summary.cursor.as_deref(), Some("next"));
        assert_eq!(summary.scan_failure, None);
    }

    #[test]
    fn scheduled_poison_reaches_the_operator_diagnostic_channel() {
        let generation = GenerationId::from_uuid7(Uuid7::compose(1, [7; 10]));
        let pass = SchedulePass {
            scanned: 1,
            outcomes: vec![(
                generation,
                CommandOutcome::Poison {
                    reason: "reconciliation exhausted".to_owned(),
                },
            )],
            cursor: None,
            scan_failure: None,
        };
        let diagnostics = schedule_quarantines(&pass);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].message_id,
            format!("runtime-sweep:{generation}")
        );
        assert_eq!(diagnostics[0].reason, "reconciliation exhausted");
    }
}
