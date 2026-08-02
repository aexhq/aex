//! Whole-frame NDJSON writes and the durable sent-cursor invariant.

use aex_wire::models::{Observation, ObservationFrameRecords};
use async_trait::async_trait;

pub use aex_wire::models::{ObservationFrame as Frame, RotateReason};

/// Maximum records in one frame.
pub const MAX_FRAME_RECORDS: usize = 200;
/// Maximum encoded bytes in one frame.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

fn resumable_cursor(frame: &Frame) -> Option<&str> {
    match frame {
        Frame::Cursor(frame) => Some(frame.cursor.as_str()),
        Frame::Rotate(frame) => frame.cursor.as_ref().map(aex_wire::cursor::Cursor::as_str),
        Frame::Records(_) | Frame::Gap(_) => None,
    }
}

/// Minimal transport port used by Lambda/hyper adapters and deterministic tests.
#[async_trait]
pub trait FrameSink: Send {
    /// Transport error.
    type Error: Send;

    /// Writes every byte or returns failure.
    async fn write_all(&mut self, bytes: &[u8]) -> Result<(), Self::Error>;
    /// Flushes previously written bytes to the peer.
    async fn flush(&mut self) -> Result<(), Self::Error>;
}

/// Cursor from the last fully written and flushed cursor-bearing frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SentCursor(Option<String>);

impl SentCursor {
    /// Borrow the token, when a cursor-bearing frame has completed.
    #[must_use]
    pub fn as_deref(&self) -> Option<&str> {
        self.0.as_deref()
    }
}

/// Whole-frame writer. It adopts a cursor only after write and flush succeed.
pub struct FrameWriter<S> {
    sink: S,
    sent: SentCursor,
}

impl<S> FrameWriter<S>
where
    S: FrameSink,
{
    /// Starts with no sent cursor.
    #[must_use]
    pub const fn new(sink: S) -> Self {
        Self {
            sink,
            sent: SentCursor(None),
        }
    }

    /// Serializes one complete newline-terminated frame and flushes it.
    ///
    /// # Errors
    ///
    /// Returns [`StreamWriteError`] without advancing the sent cursor.
    pub async fn send(&mut self, frame: Frame) -> Result<(), StreamWriteError<S::Error>> {
        let adopted = resumable_cursor(&frame).map(str::to_owned);
        let mut bytes = serde_json::to_vec(&frame).map_err(StreamWriteError::Serialize)?;
        bytes.push(b'\n');
        self.sink
            .write_all(&bytes)
            .await
            .map_err(StreamWriteError::Transport)?;
        self.sink
            .flush()
            .await
            .map_err(StreamWriteError::Transport)?;
        if adopted.is_some() {
            self.sent = SentCursor(adopted);
        }
        Ok(())
    }

    /// Last completely flushed cursor-bearing frame cursor.
    #[must_use]
    pub fn sent(&self) -> SentCursor {
        self.sent.clone()
    }

    /// Inspect the transport.
    #[must_use]
    pub const fn sink(&self) -> &S {
        &self.sink
    }

    /// Mutate a transport control such as a deterministic fault script.
    #[must_use]
    pub const fn sink_mut(&mut self) -> &mut S {
        &mut self.sink
    }
}

/// A whole-frame serialization or transport failure.
#[derive(Debug)]
pub enum StreamWriteError<E> {
    /// Frame was not JSON serializable.
    Serialize(serde_json::Error),
    /// Write or flush failed.
    Transport(E),
}

/// Splits observations without dropping any item and refuses a single oversize record.
///
/// Cursors are emitted in their own generated wire frame after a records frame has
/// been delivered; embedding one in every records frame would create a second wire
/// contract and falsely advance resumability before the cursor frame is flushed.
///
/// # Errors
///
/// Returns [`FrameSplitError`] for serialization or size violations.
pub fn split_records(records: Vec<Observation>) -> Result<Vec<Frame>, FrameSplitError> {
    let mut frames = Vec::new();
    let mut current = Vec::new();

    for record in records {
        if current.len() == MAX_FRAME_RECORDS {
            frames.push(records_frame(std::mem::take(&mut current)));
        }

        current.push(record);
        if encoded_records_size(&current)? > MAX_FRAME_BYTES {
            let Some(record) = current.pop() else {
                return Err(FrameSplitError::Serialization);
            };
            if current.is_empty() {
                return Err(FrameSplitError::RecordTooLarge);
            }
            frames.push(records_frame(std::mem::take(&mut current)));
            current.push(record);
            if encoded_records_size(&current)? > MAX_FRAME_BYTES {
                return Err(FrameSplitError::RecordTooLarge);
            }
        }
    }

    if !current.is_empty() {
        frames.push(records_frame(current));
    }
    Ok(frames)
}

fn records_frame(items: Vec<Observation>) -> Frame {
    Frame::Records(ObservationFrameRecords { items })
}

fn encoded_records_size(items: &[Observation]) -> Result<usize, FrameSplitError> {
    serde_json::to_vec(&records_frame(items.to_vec()))
        .map(|encoded| encoded.len() + 1)
        .map_err(|_| FrameSplitError::Serialization)
}

/// Why records could not be split into valid frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FrameSplitError {
    /// One stored record exceeds the frame limit by itself.
    #[error("one record exceeds the stream frame byte limit")]
    RecordTooLarge,
    /// JSON serialization failed.
    #[error("record frame is not JSON serializable")]
    Serialization,
}
