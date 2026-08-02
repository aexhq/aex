//! The NDJSON frame stream and its writer.
//!
//! `aex-wire` deliberately leaves `ObservationsApi::FrameStream` unconstrained
//! (`Send + 'static`): it has no async dependency and must not gain one. This
//! module supplies the concrete stream type and the frame writer, so the wire
//! contract stays framework-free and the transport lives in the composition.
//!
//! Two invariants the writer enforces structurally:
//!
//! - **A frame is written whole or not at all.** The resume cursor is adopted
//!   only after the whole frame and its newline have been handed to the
//!   transport, so a reader that saw a cursor saw everything before it.
//! - **`rotate` is the only terminal frame after `200 OK`.** Once headers are
//!   flushed there is no status left to change, so a failure is a `rotate`
//!   carrying its reason, its retryability and a typed error.

use aex_wire::cursor::Cursor;
use aex_wire::models::{ApiErrorBody, ObservationFrame, ObservationFrameRotate, RotateReason};
use bytes::Bytes;
use futures::stream::BoxStream;
use std::time::Duration;
use tokio::sync::mpsc;

/// How many frames may be buffered before the producer is back-pressured.
pub const DEFAULT_CHANNEL_FRAMES: usize = 8;

/// Why a frame could not be produced.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// A frame was not serializable, which is a defect in this process.
    #[error("a frame could not be serialized: {reason}")]
    Serialize {
        /// What failed.
        reason: String,
    },
}

/// The concrete frame stream this deployable produces.
pub type FrameStream = BoxStream<'static, Result<Bytes, FrameError>>;

/// The producer half of one stream.
#[derive(Debug)]
pub struct FrameSender {
    sender: mpsc::Sender<Result<Bytes, FrameError>>,
    sent: Option<Cursor>,
    write_stall: Duration,
}

impl FrameSender {
    /// Serializes one complete newline-terminated frame and hands it over.
    ///
    /// Returns `false` once the reader has gone away, which is the producer's
    /// signal to stop rather than an error to report.
    pub async fn send(&mut self, frame: &ObservationFrame) -> bool {
        let cursor = frame_cursor(frame);
        let encoded = match serde_json::to_vec(frame) {
            Ok(mut bytes) => {
                bytes.push(b'\n');
                Ok(Bytes::from(bytes))
            }
            Err(error) => Err(FrameError::Serialize {
                reason: error.to_string(),
            }),
        };
        let ok = tokio::time::timeout(self.write_stall, self.sender.send(encoded))
            .await
            .is_ok_and(|result| result.is_ok());
        if ok {
            // The cursor is adopted only after the whole frame was accepted.
            if let Some(cursor) = cursor {
                self.sent = Some(cursor);
            }
        }
        ok
    }

    /// The cursor of the last fully written frame.
    #[must_use]
    pub fn sent(&self) -> Option<&Cursor> {
        self.sent.as_ref()
    }
}

/// Opens one frame stream and its producer.
#[must_use]
pub fn channel(capacity: usize, write_stall: Duration) -> (FrameSender, FrameStream) {
    let (sender, receiver) = mpsc::channel(capacity.max(1));
    let stream = futures::StreamExt::boxed(tokio_stream_of(receiver));
    (
        FrameSender {
            sender,
            sent: None,
            write_stall,
        },
        stream,
    )
}

/// Adapts the bounded channel into a stream without a second dependency.
fn tokio_stream_of(
    mut receiver: mpsc::Receiver<Result<Bytes, FrameError>>,
) -> impl futures::Stream<Item = Result<Bytes, FrameError>> + Send + 'static {
    futures::stream::poll_fn(move |context| receiver.poll_recv(context))
}

/// The terminal frame every stream ends with.
#[must_use]
pub fn rotate(cursor: Option<Cursor>, reason: RotateReason, retryable: bool) -> ObservationFrame {
    ObservationFrame::Rotate(ObservationFrameRotate {
        cursor,
        reason,
        retryable,
        error: None,
    })
}

/// A typed terminal failure after response headers were sent.
#[must_use]
pub fn failed(cursor: Option<Cursor>, error: ApiErrorBody) -> ObservationFrame {
    let retryable = error.retryable;
    ObservationFrame::Rotate(ObservationFrameRotate {
        cursor,
        reason: RotateReason::Failed,
        retryable,
        error: Some(error),
    })
}

/// The resume token one frame carries.
fn frame_cursor(frame: &ObservationFrame) -> Option<Cursor> {
    match frame {
        ObservationFrame::Cursor(frame) => Some(frame.cursor.clone()),
        ObservationFrame::Rotate(frame) => frame.cursor.clone(),
        ObservationFrame::Records(_) | ObservationFrame::Gap(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use aex_wire::cursor::Cursor;
    use aex_wire::models::{
        ObservationCoverage, ObservationFrame, ObservationFrameCursor, RotateReason,
    };
    use aex_wire::types::DecimalU128;
    use futures::StreamExt as _;

    use super::{channel, rotate};

    fn test_channel() -> (super::FrameSender, super::FrameStream) {
        channel(8, std::time::Duration::from_secs(1))
    }

    #[tokio::test]
    async fn a_frame_is_newline_terminated_and_whole() {
        let (mut sender, mut stream) = test_channel();
        let cursor = Cursor::parse("cur_one").expect("cursor");
        let frame = ObservationFrame::Cursor(ObservationFrameCursor {
            cursor,
            coverage: coverage(),
        });
        assert!(sender.send(&frame).await);
        let bytes = stream
            .next()
            .await
            .expect("one frame")
            .expect("a serializable frame");
        assert!(bytes.ends_with(b"\n"), "every frame ends with a newline");
        let text = String::from_utf8(bytes.to_vec()).expect("utf-8");
        assert_eq!(text.matches('\n').count(), 1, "exactly one frame per write");
        assert!(text.contains("cur_one"));
    }

    #[tokio::test]
    async fn the_sent_cursor_advances_only_after_the_frame_is_accepted() {
        let (mut sender, mut stream) = test_channel();
        assert_eq!(sender.sent(), None, "nothing is claimed before a write");
        let cursor = Cursor::parse("cur_two").expect("cursor");
        let frame = rotate(Some(cursor.clone()), RotateReason::BudgetExhausted, true);
        assert!(sender.send(&frame).await);
        assert_eq!(sender.sent(), Some(&cursor));
        let _ = stream.next().await;
    }

    #[tokio::test]
    async fn a_dropped_reader_stops_the_producer_rather_than_failing_it() {
        let (mut sender, stream) = test_channel();
        drop(stream);
        let frame = rotate(None, RotateReason::ServerRotating, false);
        assert!(
            !sender.send(&frame).await,
            "a gone reader is a stop signal, not an error"
        );
    }

    #[test]
    fn rotate_is_the_only_terminal_frame_and_carries_its_reason() {
        let frame = rotate(None, RotateReason::ClientIdle, false);
        match frame {
            ObservationFrame::Rotate(frame) => {
                assert_eq!(frame.reason, RotateReason::ClientIdle);
                assert!(!frame.retryable);
            }
            other => panic!("expected a rotate frame, got {other:?}"),
        }
    }

    fn coverage() -> ObservationCoverage {
        ObservationCoverage {
            accepted: DecimalU128::new(1),
            caught_up: true,
            complete: true,
            earliest_replay: DecimalU128::new(0),
            indexed: DecimalU128::new(1),
            missing_intervals: Vec::new(),
            snapshot: DecimalU128::new(1),
            unbounded_gaps: Vec::new(),
        }
    }
}
