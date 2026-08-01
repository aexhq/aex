//! Whole-frame NDJSON writes and the durable sent-cursor invariant.

use aex_wire::error::ApiError;
use aex_wire::types::Timestamp;
use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;

/// Maximum records in one frame.
pub const MAX_FRAME_RECORDS: usize = 200;
/// Maximum encoded bytes in one frame.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// Why a stream rotates after `200 OK`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RotateReason {
    /// Fixed connection rotation.
    Rotation,
    /// Host is draining.
    Draining,
    /// Writer failed the stall deadline.
    SlowReader,
    /// Credential assertion expired.
    AssertionExpired,
    /// Credential epoch advanced.
    Revoked,
    /// Account became paused.
    AccountPaused,
    /// Session was tombstoned.
    SessionDeleted,
    /// Capacity must be released.
    Capacity,
}

/// The closed NDJSON frame vocabulary.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Frame {
    /// A bounded record page.
    Records {
        /// Stored records in authority order.
        records: Vec<Value>,
        /// Resume token valid after the whole frame arrives.
        cursor: String,
    },
    /// A durable telemetry gap.
    Gap {
        /// Stable gap identifier.
        gap_id: String,
        /// Resume token.
        cursor: String,
    },
    /// Heartbeat carrying current resume state.
    Cursor {
        /// Resume token.
        cursor: String,
        /// Authority time.
        at: Timestamp,
    },
    /// The only terminal frame after headers were flushed.
    Rotate {
        /// Resume token.
        cursor: String,
        /// Terminal reason.
        reason: RotateReason,
        /// Whether reconnect is meaningful.
        retryable: bool,
        /// Typed terminal error when retry is not meaningful.
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<ApiError>,
    },
}

impl Frame {
    fn cursor(&self) -> &str {
        match self {
            Self::Records { cursor, .. }
            | Self::Gap { cursor, .. }
            | Self::Cursor { cursor, .. }
            | Self::Rotate { cursor, .. } => cursor,
        }
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

/// Cursor from the last fully written and flushed frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SentCursor(Option<String>);

impl SentCursor {
    /// Borrow the token, when a frame has completed.
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
        let adopted = frame.cursor().to_owned();
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
        self.sent = SentCursor(Some(adopted));
        Ok(())
    }

    /// Last completely flushed frame cursor.
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

/// Splits records without dropping any item and refuses a single oversize record.
///
/// # Errors
///
/// Returns [`FrameSplitError`] for cursor-count, serialization or size violations.
pub fn split_records(
    records: Vec<Value>,
    cursors: &[String],
) -> Result<Vec<Frame>, FrameSplitError> {
    if records.len() != cursors.len() {
        return Err(FrameSplitError::CursorCountMismatch);
    }
    let mut frames = Vec::new();
    let mut current = Vec::new();
    for (record, cursor) in records.into_iter().zip(cursors) {
        let mut candidate = current.clone();
        candidate.push(record.clone());
        let frame = Frame::Records {
            records: candidate,
            cursor: cursor.clone(),
        };
        let size = serde_json::to_vec(&frame)
            .map_err(|_| FrameSplitError::Serialization)?
            .len()
            + 1;
        if current.len() == MAX_FRAME_RECORDS || size > MAX_FRAME_BYTES {
            if current.is_empty() {
                return Err(FrameSplitError::RecordTooLarge);
            }
            let previous_cursor = cursors
                [frames.iter().map(frame_record_count).sum::<usize>() + current.len() - 1]
                .clone();
            frames.push(Frame::Records {
                records: std::mem::take(&mut current),
                cursor: previous_cursor,
            });
        }
        current.push(record);
    }
    if !current.is_empty() {
        let cursor = cursors
            .last()
            .cloned()
            .ok_or(FrameSplitError::CursorCountMismatch)?;
        frames.push(Frame::Records {
            records: current,
            cursor,
        });
    }
    Ok(frames)
}

fn frame_record_count(frame: &Frame) -> usize {
    match frame {
        Frame::Records { records, .. } => records.len(),
        _ => 0,
    }
}

/// Why records could not be split into valid frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FrameSplitError {
    /// Each record must have the cursor after that record.
    #[error("record and cursor counts differ")]
    CursorCountMismatch,
    /// One stored record exceeds the frame limit by itself.
    #[error("one record exceeds the stream frame byte limit")]
    RecordTooLarge,
    /// JSON serialization failed.
    #[error("record frame is not JSON serializable")]
    Serialization,
}
