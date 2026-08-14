use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use aex_runtime_control::HandId;
use aex_wire::ids::{GenerationId, SessionId};
use serde::{Deserialize, Serialize};

/// Runtime progress emitted by the trusted preparation port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreparationProgress {
    /// Provider allocation started.
    Provisioning,
    /// Guest boot started.
    Booting,
    /// Frozen workspace is being materialized.
    MaterializingWorkspace,
    /// Generation reached readiness.
    Ready,
    /// No waiter existed; suspend started.
    Suspending,
    /// Generation is suspended.
    Suspended,
    /// A waiter caused resume.
    Resuming,
    /// Preparation failed before the sandbox could become ready.
    Failed,
}

/// Live and retained telemetry vocabulary owned by Tool Mux.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "event",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum TelemetryKind {
    /// Sandbox preparation was durably requested.
    SandboxRequested,
    /// One setup phase became visible.
    SandboxProgress {
        /// Visible setup phase.
        progress: PreparationProgress,
    },
    /// A call waits for exact-generation readiness.
    ToolWaiting,
    /// Exact generation is ready and execution starts.
    ToolStarted,
    /// Remote MCP starts through the builtin client in the exact Hand.
    RemoteMcpStarted,
    /// Bounded preview is available live.
    ToolPreview {
        /// Preview bytes transmitted.
        bytes: u64,
        /// Whether complete bytes live elsewhere.
        truncated: bool,
    },
    /// Full bytes were placed at a call-scoped sandbox path.
    ToolResultPlaced {
        /// Stable path inside the exact sandbox generation.
        path: String,
        /// Complete local byte count.
        bytes: u64,
    },
    /// Tool reached a model-visible terminal outcome.
    ToolCompleted {
        /// Whether the tool reported an error result.
        is_error: bool,
    },
}

/// Contiguous producer ordinals lost while the bounded gateway ingress was
/// under pressure. The next accepted envelope carries the coalesced range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TelemetryGap {
    /// First dropped producer ordinal, inclusive.
    pub first: u64,
    /// Last dropped producer ordinal, inclusive.
    pub last: u64,
}

/// Correlated event sent through the standard live-and-retained telemetry path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TelemetryEvent {
    /// Owning session.
    pub session: SessionId,
    /// Hand when event belongs to a sandbox.
    pub hand: Option<HandId>,
    /// Exact generation when known.
    pub generation: Option<GenerationId>,
    /// Stable provider/model tool call id when applicable.
    pub call: Option<String>,
    /// Event payload.
    pub kind: TelemetryKind,
}

/// One bounded ingress envelope. The session gateway assigns the public
/// per-session sequence, fans out live, and batches immutable S3 segments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TelemetryEnvelope {
    /// Process-local monotonic ordinal used only to detect producer loss.
    pub producer_ordinal: u64,
    /// Coalesced pressure loss immediately preceding this accepted event.
    pub gap_before: Option<TelemetryGap>,
    /// Bounded customer-safe event.
    pub event: TelemetryEvent,
}

/// Why a nonblocking producer could not enqueue an envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelemetryPressure {
    /// The bounded ingress queue has no capacity right now.
    Full,
    /// The gateway ingress is unavailable.
    Closed,
}

/// Bounded, nonblocking ingress owned by the session telemetry gateway.
pub trait TelemetryPort: Send + Sync + 'static {
    /// Attempts one enqueue without awaiting network, disk, or queue capacity.
    fn try_emit(&self, envelope: TelemetryEnvelope) -> Result<(), TelemetryPressure>;
}

/// Fail-open producer that turns bounded-ingress pressure into coalesced gap
/// evidence on the next accepted envelope.
#[derive(Clone)]
pub struct TelemetryProducer {
    sink: Arc<dyn TelemetryPort>,
    sessions: Arc<Mutex<BTreeMap<SessionId, ProducerState>>>,
}

#[derive(Debug, Clone, Copy)]
struct ProducerState {
    next_ordinal: u64,
    pending_gap: Option<TelemetryGap>,
}

impl core::fmt::Debug for TelemetryProducer {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("TelemetryProducer")
            .field(
                "tracked_sessions",
                &self
                    .sessions
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .len(),
            )
            .finish_non_exhaustive()
    }
}

impl TelemetryProducer {
    /// Binds one process-local producer to the gateway ingress.
    #[must_use]
    pub fn new(sink: Arc<dyn TelemetryPort>) -> Self {
        Self {
            sink,
            sessions: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// Offers one event and returns immediately. Pressure never reaches tool
    /// control flow; dropped ordinals are coalesced for later evidence.
    pub fn emit(&self, event: TelemetryEvent) {
        let session = event.session;
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let state = sessions.entry(session).or_insert(ProducerState {
            next_ordinal: 1,
            pending_gap: None,
        });
        let ordinal = state.next_ordinal;
        state.next_ordinal = state.next_ordinal.saturating_add(1);
        let envelope = TelemetryEnvelope {
            producer_ordinal: ordinal,
            gap_before: state.pending_gap,
            event,
        };
        if self.sink.try_emit(envelope).is_ok() {
            state.pending_gap = None;
        } else {
            match state.pending_gap.as_mut() {
                Some(gap) => gap.last = ordinal,
                None => {
                    state.pending_gap = Some(TelemetryGap {
                        first: ordinal,
                        last: ordinal,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use aex_wire::ids::{PrefixedId as _, SessionId, Uuid7};

    use super::*;

    #[derive(Default)]
    struct Sink {
        accept: Mutex<bool>,
        envelopes: Mutex<Vec<TelemetryEnvelope>>,
    }

    impl TelemetryPort for Sink {
        fn try_emit(&self, envelope: TelemetryEnvelope) -> Result<(), TelemetryPressure> {
            if !*self.accept.lock().expect("accept") {
                return Err(TelemetryPressure::Full);
            }
            self.envelopes.lock().expect("envelopes").push(envelope);
            Ok(())
        }
    }

    fn event(call: &str) -> TelemetryEvent {
        TelemetryEvent {
            session: SessionId::from_uuid7(Uuid7::compose(1, [1; 10])),
            hand: None,
            generation: None,
            call: Some(call.to_owned()),
            kind: TelemetryKind::RemoteMcpStarted,
        }
    }

    #[test]
    fn pressure_is_fail_open_and_the_next_envelope_coalesces_the_gap() {
        let sink = Arc::new(Sink::default());
        let producer = TelemetryProducer::new(sink.clone());
        producer.emit(event("lost-1"));
        producer.emit(event("lost-2"));
        *sink.accept.lock().expect("accept") = true;
        producer.emit(event("accepted"));

        let envelopes = sink.envelopes.lock().expect("envelopes");
        assert_eq!(envelopes.len(), 1);
        assert_eq!(envelopes[0].producer_ordinal, 3);
        assert_eq!(
            envelopes[0].gap_before,
            Some(TelemetryGap { first: 1, last: 2 })
        );
    }
}
