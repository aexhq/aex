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

use aex_regional_http::stream::{Frame, RotateReason};
use bytes::Bytes;
use futures::stream::BoxStream;
use tokio::sync::mpsc;

/// How many frames may be buffered before the producer is back-pressured.
pub const CHANNEL_FRAMES: usize = 8;

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
    sent: Option<String>,
}

impl FrameSender {
    /// Serializes one complete newline-terminated frame and hands it over.
    ///
    /// Returns `false` once the reader has gone away, which is the producer's
    /// signal to stop rather than an error to report.
    pub async fn send(&mut self, frame: &Frame) -> bool {
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
        let ok = self.sender.send(encoded).await.is_ok();
        if ok {
            // The cursor is adopted only after the whole frame was accepted.
            self.sent = Some(cursor);
        }
        ok
    }

    /// The cursor of the last fully written frame.
    #[must_use]
    pub fn sent(&self) -> Option<&str> {
        self.sent.as_deref()
    }
}

/// Opens one frame stream and its producer.
#[must_use]
pub fn channel() -> (FrameSender, FrameStream) {
    let (sender, receiver) = mpsc::channel(CHANNEL_FRAMES);
    let stream = futures::StreamExt::boxed(tokio_stream_of(receiver));
    (FrameSender { sender, sent: None }, stream)
}

/// Adapts the bounded channel into a stream without a second dependency.
fn tokio_stream_of(
    mut receiver: mpsc::Receiver<Result<Bytes, FrameError>>,
) -> impl futures::Stream<Item = Result<Bytes, FrameError>> + Send + 'static {
    futures::stream::poll_fn(move |context| receiver.poll_recv(context))
}

/// The terminal frame every stream ends with.
#[must_use]
pub fn rotate(cursor: String, reason: RotateReason, retryable: bool) -> Frame {
    Frame::Rotate {
        cursor,
        reason,
        retryable,
        error: None,
    }
}

/// The resume token one frame carries.
fn frame_cursor(frame: &Frame) -> String {
    match frame {
        Frame::Records { cursor, .. }
        | Frame::Gap { cursor, .. }
        | Frame::Cursor { cursor, .. }
        | Frame::Rotate { cursor, .. } => cursor.clone(),
    }
}

#[cfg(test)]
mod tests {
    use aex_regional_http::stream::{Frame, RotateReason};
    use futures::StreamExt as _;

    use super::{channel, rotate};

    #[tokio::test]
    async fn a_frame_is_newline_terminated_and_whole() {
        let (mut sender, mut stream) = channel();
        let frame = Frame::Cursor {
            cursor: "cur_one".to_owned(),
            at: aex_wire::types::Timestamp::from_unix_millis(1).expect("bounded"),
        };
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
        let (mut sender, mut stream) = channel();
        assert_eq!(sender.sent(), None, "nothing is claimed before a write");
        let frame = rotate("cur_two".to_owned(), RotateReason::Rotation, true);
        assert!(sender.send(&frame).await);
        assert_eq!(sender.sent(), Some("cur_two"));
        let _ = stream.next().await;
    }

    #[tokio::test]
    async fn a_dropped_reader_stops_the_producer_rather_than_failing_it() {
        let (mut sender, stream) = channel();
        drop(stream);
        let frame = rotate("cur_three".to_owned(), RotateReason::Draining, false);
        assert!(
            !sender.send(&frame).await,
            "a gone reader is a stop signal, not an error"
        );
    }

    #[test]
    fn rotate_is_the_only_terminal_frame_and_carries_its_reason() {
        let frame = rotate("cur_four".to_owned(), RotateReason::SlowReader, false);
        match frame {
            Frame::Rotate {
                reason, retryable, ..
            } => {
                assert_eq!(reason, RotateReason::SlowReader);
                assert!(!retryable);
            }
            other => panic!("expected a rotate frame, got {other:?}"),
        }
    }
}
