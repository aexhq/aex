//! Bounded producer ingress for the session telemetry gateway.
//!
//! Provider and tool callbacks share this queue, but neither can await it. The
//! async exporter owns the receiver and is deliberately outside the activation
//! and tool-result critical paths.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aex_brain_app::ports::{AssistantPreview, PreviewEvent, PreviewPort};
use aex_model_catalog::canonical::PreviewFrame;
use aex_tool_mux::{
    TelemetryEnvelope, TelemetryKind as ToolTelemetryKind, TelemetryPort, TelemetryPressure,
};
use aex_wire::models::{TelemetryFrame, TelemetryKind};
use aex_wire::types::{DecimalU128, Timestamp};
use aws_sdk_dynamodb::types::AttributeValue;
use tokio::sync::mpsc;

const MAX_BATCH_INPUTS: usize = 64;
const BATCH_WINDOW: Duration = Duration::from_millis(50);

/// One closed item accepted by the process-local producer queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayInput {
    /// Non-authoritative provider output for the assistant message stream.
    Assistant(AssistantPreview),
    /// Customer-safe Tool Mux event for live and retained telemetry.
    Tool(TelemetryEnvelope),
}

/// Shared nonblocking producer side of the session gateway connection.
#[derive(Debug, Clone)]
pub struct GatewayIngress {
    sender: mpsc::Sender<GatewayInput>,
}

impl GatewayIngress {
    /// Creates one bounded queue and returns the exporter-owned receiver.
    ///
    /// # Panics
    ///
    /// Panics when `capacity` is zero; a zero-capacity queue cannot implement
    /// an immediate `try_send` producer contract.
    #[must_use]
    pub fn channel(capacity: usize) -> (Self, mpsc::Receiver<GatewayInput>) {
        assert!(capacity > 0, "gateway ingress capacity must be non-zero");
        let (sender, receiver) = mpsc::channel(capacity);
        (Self { sender }, receiver)
    }
}

impl PreviewPort for GatewayIngress {
    fn try_offer(&self, preview: AssistantPreview) -> bool {
        self.sender
            .try_send(GatewayInput::Assistant(preview))
            .is_ok()
    }
}

impl TelemetryPort for GatewayIngress {
    fn try_emit(&self, envelope: TelemetryEnvelope) -> Result<(), TelemetryPressure> {
        self.sender
            .try_send(GatewayInput::Tool(envelope))
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => TelemetryPressure::Full,
                mpsc::error::TrySendError::Closed(_) => TelemetryPressure::Closed,
            })
    }
}

#[derive(Debug)]
struct FrameDraft {
    kind: TelemetryKind,
    occurred_at: Timestamp,
    body: aex_wire::CanonicalJson,
    preview: Option<String>,
    truncated: bool,
}

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

    fn draft(&self, occurred_at: Timestamp) -> Result<FrameDraft, ()> {
        let body = aex_wire::CanonicalJson::from_value(&serde_json::json!({
            "event": "aex.session.telemetry.exporter_gap",
            "droppedFrames": self.dropped_frames,
            "firstMissingSequence": self.first_missing_sequence.map(|value| value.to_string()),
            "lastMissingSequence": self.last_missing_sequence.map(|value| value.to_string()),
            "producer": "telemetry_gateway",
        }))
        .map_err(|_| ())?;
        Ok(FrameDraft {
            kind: TelemetryKind::Gap,
            occurred_at,
            body,
            preview: None,
            truncated: false,
        })
    }
}

/// Drains the bounded producer queue outside activation/tool latency, allocates
/// one contiguous session sequence range per batch, and writes one immutable
/// compressed OTLP segment per session. Store or sequence pressure drops only
/// observations; it never reaches provider/tool control flow.
pub async fn export(
    mut receiver: mpsc::Receiver<GatewayInput>,
    sequences: aws_sdk_dynamodb::Client,
    session_table: String,
    writer: aex_session_telemetry_aws::SessionTelemetryWriter,
) {
    let mut losses = BTreeMap::<aex_wire::ids::SessionId, ExportLoss>::new();
    while let Some(first) = receiver.recv().await {
        let mut inputs = Vec::with_capacity(MAX_BATCH_INPUTS);
        inputs.push(first);
        let deadline = tokio::time::sleep(BATCH_WINDOW);
        tokio::pin!(deadline);
        while inputs.len() < MAX_BATCH_INPUTS {
            tokio::select! {
                biased;
                input = receiver.recv() => match input {
                    Some(input) => inputs.push(input),
                    None => break,
                },
                () = &mut deadline => break,
            }
        }

        let mut grouped = BTreeMap::<aex_wire::ids::SessionId, Vec<FrameDraft>>::new();
        for input in inputs {
            let Ok((session, drafts)) = drafts(input) else {
                tracing::warn!(
                    target: "aex::diagnostics",
                    event = "session_telemetry_input_rejected",
                    deployable = crate::DEPLOYABLE,
                );
                continue;
            };
            grouped.entry(session).or_default().extend(drafts);
        }

        for (session, mut drafts) in grouped {
            if drafts.is_empty() {
                continue;
            }
            let original_count = drafts.len();
            let pending = losses.get(&session).cloned();
            if let Some(loss) = &pending {
                let Ok(gap) = loss.draft(drafts[0].occurred_at) else {
                    losses
                        .entry(session)
                        .or_default()
                        .record_unallocated(original_count);
                    continue;
                };
                drafts.insert(0, gap);
            }
            let Ok(first_sequence) =
                allocate_sequence_range(&sequences, &session_table, session, drafts.len()).await
            else {
                tracing::warn!(
                    target: "aex::diagnostics",
                    event = "session_telemetry_sequence_allocation_failed",
                    deployable = crate::DEPLOYABLE,
                    session = %session,
                    dropped = drafts.len(),
                );
                losses
                    .entry(session)
                    .or_default()
                    .record_unallocated(original_count);
                continue;
            };
            let frames = drafts
                .into_iter()
                .enumerate()
                .map(|(offset, draft)| TelemetryFrame {
                    sequence: DecimalU128::new(
                        first_sequence.saturating_add(u128::try_from(offset).unwrap_or(u128::MAX)),
                    ),
                    kind: draft.kind,
                    occurred_at: draft.occurred_at,
                    trace_id: None,
                    span_id: None,
                    body: Some(draft.body),
                    preview: draft.preview,
                    truncated: draft.truncated,
                })
                .collect::<Vec<_>>();
            if writer.write_frames(session, &frames).await.is_err() {
                tracing::warn!(
                    target: "aex::diagnostics",
                    event = "session_telemetry_segment_write_failed",
                    deployable = crate::DEPLOYABLE,
                    session = %session,
                    dropped = frames.len(),
                );
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

fn drafts(input: GatewayInput) -> Result<(aex_wire::ids::SessionId, Vec<FrameDraft>), ()> {
    let occurred_at = now().ok_or(())?;
    match input {
        GatewayInput::Assistant(preview) => {
            let session = aex_brain_store_dynamodb::translate::session(preview.scope.session)
                .map_err(|_| ())?;
            let scope = serde_json::json!({
                "agentId": preview.scope.agent.0.to_string(),
                "attempt": preview.scope.attempt,
                "effectId": hex::encode(preview.scope.effect.0),
                "messageId": preview.scope.message.map(|message| message.to_string()),
                "producerSequence": preview.producer_sequence,
            });
            let (kind, body, text) = match preview.event {
                PreviewEvent::Gap { dropped_frames } => (
                    TelemetryKind::Gap,
                    serde_json::json!({
                        "scope": scope,
                        "event": "aex.session.assistant.gap",
                        "droppedFrames": dropped_frames,
                    }),
                    None,
                ),
                PreviewEvent::Frame(frame) => {
                    let (metadata, text) = preview_frame(frame);
                    (
                        TelemetryKind::Assistant,
                        serde_json::json!({
                            "scope": scope,
                            "event": "aex.session.assistant.preview",
                            "frame": metadata,
                        }),
                        text,
                    )
                }
                PreviewEvent::Committed { journal_sequence } => (
                    TelemetryKind::Assistant,
                    serde_json::json!({
                        "scope": scope,
                        "event": "aex.session.assistant.committed",
                        "journalSequence": journal_sequence,
                    }),
                    None,
                ),
            };
            let body = aex_wire::CanonicalJson::from_value(&body).map_err(|_| ())?;
            Ok((
                session,
                vec![FrameDraft {
                    kind,
                    occurred_at,
                    body,
                    preview: text,
                    truncated: false,
                }],
            ))
        }
        GatewayInput::Tool(envelope) => {
            let session = envelope.event.session;
            let mut frames = Vec::with_capacity(2);
            if let Some(gap) = envelope.gap_before {
                frames.push(FrameDraft {
                    kind: TelemetryKind::Gap,
                    occurred_at,
                    body: aex_wire::CanonicalJson::from_value(&serde_json::json!({
                        "event": "aex.session.telemetry.producer_gap",
                        "first": gap.first,
                        "last": gap.last,
                        "producer": "tool_mux",
                    }))
                    .map_err(|_| ())?,
                    preview: None,
                    truncated: false,
                });
            }
            let kind = match envelope.event.kind {
                ToolTelemetryKind::SandboxRequested | ToolTelemetryKind::SandboxProgress { .. } => {
                    TelemetryKind::Runtime
                }
                _ => TelemetryKind::Tool,
            };
            let truncated = matches!(
                envelope.event.kind,
                ToolTelemetryKind::ToolPreview {
                    truncated: true,
                    ..
                }
            );
            frames.push(FrameDraft {
                kind,
                occurred_at,
                body: aex_wire::CanonicalJson::from_value(
                    &serde_json::to_value(&envelope).map_err(|_| ())?,
                )
                .map_err(|_| ())?,
                preview: None,
                truncated,
            });
            Ok((session, frames))
        }
    }
}

fn preview_frame(frame: PreviewFrame) -> (serde_json::Value, Option<String>) {
    match frame {
        PreviewFrame::BlockStart { index, kind } => (
            serde_json::json!({ "frame": "block_start", "index": index, "kind": kind }),
            None,
        ),
        PreviewFrame::TextDelta { index, text } => (
            serde_json::json!({ "frame": "text_delta", "index": index }),
            Some(text.as_str().to_owned()),
        ),
        PreviewFrame::ReasoningDelta { index, .. } => (
            serde_json::json!({ "frame": "reasoning_delta", "index": index }),
            None,
        ),
        PreviewFrame::ToolCallStart { index, id, name } => (
            serde_json::json!({
                "frame": "tool_call_start",
                "index": index,
                "callId": id,
                "name": name,
            }),
            None,
        ),
        PreviewFrame::ToolArgumentsDelta { index, .. } => (
            serde_json::json!({ "frame": "tool_arguments_delta", "index": index }),
            None,
        ),
        PreviewFrame::BlockStop { index } => (
            serde_json::json!({ "frame": "block_stop", "index": index }),
            None,
        ),
        PreviewFrame::InterimUsage(_) => (serde_json::json!({ "frame": "interim_usage" }), None),
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
    use aex_brain_app::ports::{AssistantPreview, PreviewEvent, PreviewPort as _, PreviewScope};
    use aex_brain_domain::ids::{AgentId, EffectId, SessionId as BrainSessionId};
    use aex_model_catalog::BoundedString;
    use aex_tool_mux::{TelemetryEvent, TelemetryKind, TelemetryPort as _};
    use aex_wire::ids::{PrefixedId as _, SessionId, Uuid7};
    use uuid::Uuid;

    use super::*;

    fn preview() -> AssistantPreview {
        AssistantPreview {
            scope: PreviewScope {
                session: BrainSessionId(Uuid::from_u128(1)),
                agent: AgentId(Uuid::from_u128(2)),
                effect: EffectId(Uuid::from_u128(3).into_bytes()),
                attempt: 1,
                message: None,
            },
            producer_sequence: 1,
            event: PreviewEvent::Gap { dropped_frames: 2 },
        }
    }

    fn tool() -> TelemetryEnvelope {
        TelemetryEnvelope {
            producer_ordinal: 1,
            gap_before: None,
            event: TelemetryEvent {
                session: SessionId::from_uuid7(Uuid7::compose(1, [4; 10])),
                hand: None,
                generation: None,
                call: None,
                kind: TelemetryKind::RemoteMcpStarted,
            },
        }
    }

    #[tokio::test]
    async fn both_producers_share_one_bounded_async_drain() {
        let (ingress, mut receiver) = GatewayIngress::channel(2);
        assert!(ingress.try_offer(preview()));
        assert_eq!(ingress.try_emit(tool()), Ok(()));
        assert!(matches!(
            receiver.recv().await,
            Some(GatewayInput::Assistant(_))
        ));
        assert!(matches!(receiver.recv().await, Some(GatewayInput::Tool(_))));
    }

    #[test]
    fn pressure_is_immediate_and_classified_for_tools() {
        let (ingress, _receiver) = GatewayIngress::channel(1);
        assert_eq!(ingress.try_emit(tool()), Ok(()));
        assert_eq!(ingress.try_emit(tool()), Err(TelemetryPressure::Full));
        assert!(!ingress.try_offer(preview()));

        let (closed, receiver) = GatewayIngress::channel(1);
        drop(receiver);
        assert_eq!(closed.try_emit(tool()), Err(TelemetryPressure::Closed));
        assert!(!closed.try_offer(preview()));
    }

    #[test]
    fn exporter_failures_coalesce_into_one_later_gap() {
        let mut loss = ExportLoss::default();
        loss.record_unallocated(3);
        loss.record_allocated(2, 11, 2);
        loss.record_allocated(4, 20, 5);

        let body = loss
            .draft(Timestamp::from_unix_millis(1).expect("timestamp"))
            .expect("bounded gap")
            .body
            .to_value();
        assert_eq!(body["event"], "aex.session.telemetry.exporter_gap");
        assert_eq!(body["droppedFrames"], 9);
        assert_eq!(body["firstMissingSequence"], "11");
        assert_eq!(body["lastMissingSequence"], "24");
        assert_eq!(body["producer"], "telemetry_gateway");
    }

    #[test]
    fn reasoning_and_partial_tool_arguments_never_enter_retained_frames() {
        for frame in [
            PreviewFrame::ReasoningDelta {
                index: 0,
                text: BoundedString::truncating("PRIVATE-REASONING-CANARY"),
            },
            PreviewFrame::ToolArgumentsDelta {
                index: 1,
                fragment: BoundedString::truncating("TOOL-ARGUMENT-CANARY"),
            },
        ] {
            let (metadata, preview) = preview_frame(frame);
            let encoded = serde_json::to_string(&metadata).expect("metadata encodes");
            assert!(preview.is_none());
            assert!(!encoded.contains("CANARY"));
        }
    }
}
