//! Scoped, bounded assistant-preview delivery.

use crate::kernel::sync::atomic::{AtomicU64, Ordering};
use aex_brain_domain::ids::{AgentId, EffectId, SessionId};
use aex_model_catalog::canonical::PreviewFrame;
use aex_wire::ids::MessageId;

use super::PreviewSink;

/// Immutable authority attached to every provisional provider frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewScope {
    /// Session whose live stream receives the frame.
    pub session: SessionId,
    /// Independent root or child agent whose model attempt produced it.
    pub agent: AgentId,
    /// Exact non-replayable model effect.
    pub effect: EffectId,
    /// Effect attempt, starting at one.
    pub attempt: u16,
    /// Deterministic public assistant message for a root run. Child previews
    /// remain telemetry-only and carry `None`.
    pub message: Option<MessageId>,
}

/// One non-authoritative observation offered to the live/retained preview path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistantPreview {
    /// Session/agent/effect authority.
    pub scope: PreviewScope,
    /// Monotone within one `(effect, attempt)` producer epoch.
    pub producer_sequence: u64,
    /// Provider frame or a coalesced loss marker.
    pub event: PreviewEvent,
}

/// The closed preview delivery vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewEvent {
    /// One provider delta. It can never enter model history.
    Frame(PreviewFrame),
    /// At least this many earlier frames were dropped under bounded pressure.
    Gap {
        /// Coalesced number of dropped frames.
        dropped_frames: u64,
    },
    /// The complete root assistant message committed with its journal fact.
    Committed {
        /// Journal sequence of the committed assistant record.
        journal_sequence: u64,
    },
}

/// Nonblocking preview output selected by the composition root.
pub trait PreviewPort: Send + Sync + 'static {
    /// Attempts one immediate enqueue. `false` means bounded pressure or a
    /// closed consumer; neither may delay or fail provider consumption.
    fn try_offer(&self, preview: AssistantPreview) -> bool;
}

/// Request-scoped adapter passed to one provider stream.
///
/// Overflow is carried forward as one coalesced gap. The gap must be accepted
/// before a later frame, so consumers can detect loss without observing a
/// misleading contiguous sequence.
pub struct ScopedPreviewSink<'a> {
    port: &'a dyn PreviewPort,
    scope: PreviewScope,
    dropped: AtomicU64,
    next_sequence: AtomicU64,
}

impl<'a> ScopedPreviewSink<'a> {
    /// Binds one effect attempt to the process-wide bounded port.
    #[must_use]
    pub const fn new(port: &'a dyn PreviewPort, scope: PreviewScope) -> Self {
        Self {
            port,
            scope,
            dropped: AtomicU64::new(0),
            next_sequence: AtomicU64::new(1),
        }
    }

    fn sequence(&self) -> u64 {
        self.next_sequence.fetch_add(1, Ordering::AcqRel)
    }

    /// Offers the reconciliation marker only after the authoritative message
    /// commit. Like deltas, it never delays or fails the activation.
    pub fn committed(&self, journal_sequence: u64) -> bool {
        if self.scope.message.is_none() {
            return true;
        }
        self.port.try_offer(AssistantPreview {
            scope: self.scope,
            producer_sequence: self.sequence(),
            event: PreviewEvent::Committed { journal_sequence },
        })
    }
}

impl PreviewSink for ScopedPreviewSink<'_> {
    fn offer(&self, frame: PreviewFrame) -> bool {
        let dropped = self.dropped.swap(0, Ordering::AcqRel);
        if dropped > 0
            && !self.port.try_offer(AssistantPreview {
                scope: self.scope,
                producer_sequence: self.sequence(),
                event: PreviewEvent::Gap {
                    dropped_frames: dropped,
                },
            })
        {
            self.dropped
                .fetch_add(dropped.saturating_add(1), Ordering::Release);
            return false;
        }
        if self.port.try_offer(AssistantPreview {
            scope: self.scope,
            producer_sequence: self.sequence(),
            event: PreviewEvent::Frame(frame),
        }) {
            true
        } else {
            self.dropped.fetch_add(1, Ordering::Release);
            false
        }
    }
}

/// Explicit test/local refusal port.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullPreviewPort;

impl PreviewPort for NullPreviewPort {
    fn try_offer(&self, _preview: AssistantPreview) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use aex_model_catalog::BoundedString;
    use aex_wire::ids::PrefixedId as _;
    use uuid::Uuid;

    use super::*;

    #[derive(Default)]
    struct RecordingPort {
        accept: AtomicU64,
        received: Mutex<Vec<AssistantPreview>>,
    }

    impl PreviewPort for RecordingPort {
        fn try_offer(&self, preview: AssistantPreview) -> bool {
            if self.accept.load(Ordering::Acquire) == 0 {
                return false;
            }
            self.received.lock().expect("preview lock").push(preview);
            true
        }
    }

    fn scope() -> PreviewScope {
        PreviewScope {
            session: SessionId(Uuid::from_u128(1)),
            agent: AgentId(Uuid::from_u128(2)),
            effect: EffectId(Uuid::from_u128(3).into_bytes()),
            attempt: 1,
            message: Some(MessageId::from_uuid7(aex_wire::ids::Uuid7::compose(
                4, [4; 10],
            ))),
        }
    }

    fn delta(text: &str) -> PreviewFrame {
        PreviewFrame::TextDelta {
            index: 0,
            text: BoundedString::truncating(text),
        }
    }

    #[test]
    fn overflow_is_nonblocking_and_the_next_delivery_starts_with_one_gap() {
        let port = RecordingPort::default();
        let sink = ScopedPreviewSink::new(&port, scope());
        assert!(!sink.offer(delta("lost-1")));
        assert!(!sink.offer(delta("lost-2")));

        port.accept.store(1, Ordering::Release);
        assert!(sink.offer(delta("visible")));
        let received = port.received.lock().expect("preview lock");
        assert_eq!(received.len(), 2);
        assert_eq!(
            received[0],
            AssistantPreview {
                scope: scope(),
                producer_sequence: 3,
                event: PreviewEvent::Gap { dropped_frames: 2 },
            }
        );
        assert_eq!(received[1].producer_sequence, 4);
        assert!(matches!(received[1].event, PreviewEvent::Frame(_)));
        drop(received);

        assert!(sink.committed(9));
        let received = port.received.lock().expect("preview lock");
        assert!(matches!(
            received.last().expect("commit event").event,
            PreviewEvent::Committed {
                journal_sequence: 9
            }
        ));
    }
}
