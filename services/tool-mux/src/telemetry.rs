//! Bounded, nonblocking ingress adapter for the session telemetry gateway.

use aex_tool_mux::TelemetryKind as ToolTelemetryKind;
use aex_tool_mux::{TelemetryEnvelope, TelemetryPort, TelemetryPressure};
use aex_wire::models::{TelemetryFrame, TelemetryKind};
use aex_wire::types::{DecimalU128, Timestamp};
use aws_sdk_dynamodb::types::AttributeValue;
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

const MAX_BATCH_INPUTS: usize = 64;
const BATCH_WINDOW: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ExportLoss {
    dropped_frames: u64,
    first_missing_sequence: Option<u128>,
    last_missing_sequence: Option<u128>,
}

impl ExportLoss {
    fn record_unallocated(&mut self, dropped: usize) {
        self.dropped_frames = self
            .dropped_frames
            .saturating_add(u64::try_from(dropped).unwrap_or(u64::MAX));
    }

    fn record_allocated(&mut self, dropped: usize, first: u128, allocated: usize) {
        self.record_unallocated(dropped);
        let last = first.saturating_add(
            u128::try_from(allocated)
                .unwrap_or(u128::MAX)
                .saturating_sub(1),
        );
        self.first_missing_sequence = Some(
            self.first_missing_sequence
                .map_or(first, |current| current.min(first)),
        );
        self.last_missing_sequence = Some(
            self.last_missing_sequence
                .map_or(last, |current| current.max(last)),
        );
    }

    fn body(&self) -> Result<aex_wire::CanonicalJson, ()> {
        aex_wire::CanonicalJson::from_value(&serde_json::json!({
            "event": "aex.session.telemetry.exporter_gap",
            "droppedFrames": self.dropped_frames,
            "firstMissingSequence": self.first_missing_sequence.map(|value| value.to_string()),
            "lastMissingSequence": self.last_missing_sequence.map(|value| value.to_string()),
            "producer": "tool_mux_gateway",
        }))
        .map_err(|_| ())
    }
}

/// Process-local producer side of the session gateway ingress.
#[derive(Debug, Clone)]
pub struct BoundedTelemetryIngress {
    sender: mpsc::Sender<TelemetryEnvelope>,
}

impl BoundedTelemetryIngress {
    /// Creates a bounded ingress and returns its gateway-owned receiver.
    ///
    /// # Panics
    ///
    /// Panics when `capacity` is zero; a zero-capacity queue cannot provide a
    /// nonblocking producer contract.
    #[must_use]
    pub fn new(capacity: usize) -> (Self, mpsc::Receiver<TelemetryEnvelope>) {
        let (sender, receiver) = mpsc::channel(capacity);
        (Self { sender }, receiver)
    }
}

impl TelemetryPort for BoundedTelemetryIngress {
    fn try_emit(&self, envelope: TelemetryEnvelope) -> Result<(), TelemetryPressure> {
        self.sender.try_send(envelope).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => TelemetryPressure::Full,
            mpsc::error::TrySendError::Closed(_) => TelemetryPressure::Closed,
        })
    }
}

/// Drains the fail-open ingress, allocates one contiguous range per session,
/// and writes immutable compressed OTLP segments outside tool latency.
pub async fn export(
    mut receiver: mpsc::Receiver<TelemetryEnvelope>,
    sequences: aws_sdk_dynamodb::Client,
    session_table: String,
    writer: aex_session_telemetry_aws::SessionTelemetryWriter,
) {
    let mut losses = BTreeMap::<aex_wire::ids::SessionId, ExportLoss>::new();
    while let Some(first) = receiver.recv().await {
        let mut inputs = vec![first];
        let deadline = tokio::time::sleep(BATCH_WINDOW);
        tokio::pin!(deadline);
        while inputs.len() < MAX_BATCH_INPUTS {
            tokio::select! {
                input = receiver.recv() => match input {
                    Some(input) => inputs.push(input),
                    None => break,
                },
                () = &mut deadline => break,
            }
        }
        let mut grouped = BTreeMap::new();
        for envelope in inputs {
            grouped
                .entry(envelope.event.session)
                .or_insert_with(Vec::new)
                .push(envelope);
        }
        for (session, envelopes) in grouped {
            let original_count = envelopes
                .iter()
                .map(|envelope| 1_usize + usize::from(envelope.gap_before.is_some()))
                .sum::<usize>();
            let pending = losses.get(&session).cloned();
            let draft_count = original_count.saturating_add(usize::from(pending.is_some()));
            let Some(occurred_at) = now() else {
                losses
                    .entry(session)
                    .or_default()
                    .record_unallocated(original_count);
                continue;
            };
            let Ok(first_sequence) =
                allocate_sequence_range(&sequences, &session_table, session, draft_count).await
            else {
                tracing::warn!(target: "aex::diagnostics", event = "tool_mux_telemetry_sequence_failed", session = %session, dropped = draft_count);
                losses
                    .entry(session)
                    .or_default()
                    .record_unallocated(original_count);
                continue;
            };
            let mut frames = Vec::with_capacity(draft_count);
            if let Some(loss) = &pending {
                let Ok(body) = loss.body() else {
                    losses.entry(session).or_default().record_allocated(
                        original_count,
                        first_sequence,
                        draft_count,
                    );
                    continue;
                };
                frames.push((TelemetryKind::Gap, Ok(body), false));
            }
            for envelope in envelopes {
                if let Some(gap) = envelope.gap_before {
                    frames.push((
                        TelemetryKind::Gap,
                        aex_wire::CanonicalJson::from_value(&serde_json::json!({
                            "event": "aex.session.telemetry.producer_gap",
                            "producer": "tool_mux",
                            "first": gap.first,
                            "last": gap.last,
                        })),
                        false,
                    ));
                }
                let kind = match envelope.event.kind {
                    ToolTelemetryKind::SandboxRequested
                    | ToolTelemetryKind::SandboxProgress { .. } => TelemetryKind::Runtime,
                    _ => TelemetryKind::Tool,
                };
                let truncated = matches!(
                    envelope.event.kind,
                    ToolTelemetryKind::ToolPreview {
                        truncated: true,
                        ..
                    }
                );
                frames.push((
                    kind,
                    aex_wire::CanonicalJson::from_value(
                        &serde_json::to_value(&envelope).unwrap_or(serde_json::Value::Null),
                    ),
                    truncated,
                ));
            }
            let frames = frames
                .into_iter()
                .enumerate()
                .filter_map(|(offset, (kind, body, truncated))| {
                    Some(TelemetryFrame {
                        sequence: DecimalU128::new(
                            first_sequence.saturating_add(u128::try_from(offset).ok()?),
                        ),
                        kind,
                        occurred_at,
                        trace_id: None,
                        span_id: None,
                        body: body.ok(),
                        preview: None,
                        truncated,
                    })
                })
                .collect::<Vec<_>>();
            if writer.write_frames(session, &frames).await.is_err() {
                tracing::warn!(target: "aex::diagnostics", event = "tool_mux_telemetry_segment_failed", session = %session, dropped = frames.len());
                losses.entry(session).or_default().record_allocated(
                    original_count,
                    first_sequence,
                    frames.len(),
                );
            } else if pending.is_some() {
                losses.remove(&session);
            }
        }
    }
}

async fn allocate_sequence_range(
    client: &aws_sdk_dynamodb::Client,
    table: &str,
    session: aex_wire::ids::SessionId,
    count: usize,
) -> Result<u128, ()> {
    let count = u128::try_from(count).map_err(|_| ())?;
    if count == 0 {
        return Err(());
    }
    let key = aex_session_dynamodb::keys::head(session);
    let response = client
        .update_item()
        .table_name(table)
        .key(aex_session_dynamodb::attr::PK, AttributeValue::S(key.pk))
        .key(aex_session_dynamodb::attr::SK, AttributeValue::S(key.sk))
        .update_expression("SET #sequence = if_not_exists(#sequence, :zero) + :count")
        .condition_expression("attribute_exists(#pk)")
        .expression_attribute_names("#sequence", "sessionTelemetrySequence")
        .expression_attribute_names("#pk", aex_session_dynamodb::attr::PK)
        .expression_attribute_values(":zero", AttributeValue::N("0".to_owned()))
        .expression_attribute_values(":count", AttributeValue::N(count.to_string()))
        .return_values(aws_sdk_dynamodb::types::ReturnValue::UpdatedNew)
        .send()
        .await
        .map_err(|_| ())?;
    let last = response
        .attributes()
        .and_then(|attributes| attributes.get("sessionTelemetrySequence"))
        .and_then(|value| value.as_n().ok())
        .and_then(|value| value.parse::<u128>().ok())
        .ok_or(())?;
    last.checked_sub(count.saturating_sub(1)).ok_or(())
}

fn now() -> Option<Timestamp> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis();
    Timestamp::from_unix_millis(i64::try_from(millis).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use aex_tool_mux::{TelemetryEvent, TelemetryKind, TelemetryPort as _, TelemetryProducer};
    use aex_wire::ids::{PrefixedId as _, SessionId, Uuid7};

    use super::*;

    fn event(call: &str) -> TelemetryEvent {
        TelemetryEvent {
            session: SessionId::from_uuid7(Uuid7::compose(1, [2; 10])),
            hand: None,
            generation: None,
            call: Some(call.to_owned()),
            kind: TelemetryKind::RemoteMcpStarted,
        }
    }

    #[tokio::test]
    async fn full_queue_never_blocks_and_the_next_delivery_carries_the_gap() {
        let (ingress, mut receiver) = BoundedTelemetryIngress::new(1);
        let producer = TelemetryProducer::new(std::sync::Arc::new(ingress));
        producer.emit(event("accepted-1"));
        producer.emit(event("lost-2"));

        let first = receiver.recv().await.expect("first envelope");
        assert_eq!(first.producer_ordinal, 1);
        producer.emit(event("accepted-3"));
        let third = receiver.recv().await.expect("third envelope");
        assert_eq!(third.producer_ordinal, 3);
        assert_eq!(third.gap_before.expect("gap").first, 2);
        assert_eq!(third.gap_before.expect("gap").last, 2);
    }

    #[test]
    fn closed_gateway_is_reported_without_waiting() {
        let (ingress, receiver) = BoundedTelemetryIngress::new(1);
        drop(receiver);
        let envelope = aex_tool_mux::TelemetryEnvelope {
            producer_ordinal: 1,
            gap_before: None,
            event: event("closed"),
        };
        assert_eq!(ingress.try_emit(envelope), Err(TelemetryPressure::Closed));
    }

    #[test]
    fn exporter_failures_are_coalesced_for_the_next_segment() {
        let mut loss = ExportLoss::default();
        loss.record_unallocated(2);
        loss.record_allocated(3, 10, 4);
        loss.record_allocated(1, 20, 1);
        let body = loss.body().expect("bounded gap").to_value();
        assert_eq!(body["event"], "aex.session.telemetry.exporter_gap");
        assert_eq!(body["droppedFrames"], 6);
        assert_eq!(body["firstMissingSequence"], "10");
        assert_eq!(body["lastMissingSequence"], "20");
        assert_eq!(body["producer"], "tool_mux_gateway");
    }
}
