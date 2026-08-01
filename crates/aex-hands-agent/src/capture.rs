//! Output capture and its bounds.
//!
//! One capture path for every operation, with both bounds applied on the stream
//! and nothing buffering between them:
//!
//! - bytes are retained in `out.bin` until `bounds.max_output_bytes`; after that
//!   the file stops growing, `truncated` is set, the process keeps running and
//!   `produced_bytes` keeps counting;
//! - while delivery is `Attached`, chunks are mirrored into `Output` frames split
//!   at `min(max_frame_bytes, 1 MiB)` and **never inside a UTF-8 sequence**;
//! - the attached mirror stops permanently after
//!   [`ATTACH_STREAM_CAP_BYTES`] with a final truncated frame. The file capture is
//!   unaffected: a customer who detaches must still get the whole retained body.
//!
//! The terminal digest is over the **retained** bytes, so truncation is reported
//! honestly instead of surfacing later as a digest failure Brain cannot explain.

use aex_hands_protocol::operation::OperationBounds;
use aex_hands_protocol::rpc::OutputStream;

/// The largest attached mirror one operation may stream before it stops.
pub const ATTACH_STREAM_CAP_BYTES: u64 = 10_000_000;

/// The largest attached frame, before the per-generation frame bound is applied.
pub const ATTACH_FRAME_CEILING: usize = 1_048_576;

/// One chunk to mirror onto an attached connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorChunk {
    /// Which stream produced it.
    pub stream: OutputStream,
    /// The bytes, split on a UTF-8 boundary.
    pub bytes: Vec<u8>,
    /// Whether the mirror has dropped anything by this point.
    pub truncated: bool,
}

/// What one write produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureOutcome {
    /// The bytes to append to `out.bin`, possibly shorter than the input.
    pub retain: Vec<u8>,
    /// The frames to mirror, empty when the operation is detached or capped.
    pub mirror: Vec<MirrorChunk>,
}

/// The bounded capture state for one operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capture {
    /// The retention ceiling.
    max_output_bytes: u64,
    /// The attached frame ceiling.
    frame_bytes: usize,
    /// Whether an attached connection is mirroring.
    attached: bool,
    /// Bytes retained so far.
    retained_bytes: u64,
    /// Bytes the process produced, retained or not.
    produced_bytes: u64,
    /// Bytes mirrored so far.
    mirrored_bytes: u64,
    /// Whether anything was dropped from the retained capture.
    truncated: bool,
    /// Whether the attached mirror has stopped for good.
    mirror_capped: bool,
}

impl Capture {
    /// Opens a capture under `bounds`.
    #[must_use]
    pub fn new(bounds: &OperationBounds, attached: bool) -> Self {
        let frame_bytes = usize::try_from(bounds.max_frame_bytes)
            .unwrap_or(ATTACH_FRAME_CEILING)
            .clamp(1, ATTACH_FRAME_CEILING);
        Self {
            max_output_bytes: bounds.max_output_bytes,
            frame_bytes,
            attached,
            retained_bytes: 0,
            produced_bytes: 0,
            mirrored_bytes: 0,
            truncated: false,
            mirror_capped: false,
        }
    }

    /// Reopens a capture over an existing `out.bin` of `retained_bytes`, which is
    /// what replay does when it re-attaches to a still-running process group.
    #[must_use]
    pub fn resume(bounds: &OperationBounds, attached: bool, retained_bytes: u64) -> Self {
        let mut capture = Self::new(bounds, attached);
        capture.retained_bytes = retained_bytes;
        capture.produced_bytes = retained_bytes;
        capture.truncated = retained_bytes >= bounds.max_output_bytes;
        capture
    }

    /// Bytes retained in `out.bin`.
    #[must_use]
    pub const fn retained_bytes(&self) -> u64 {
        self.retained_bytes
    }

    /// Bytes the process produced, whether retained or dropped.
    #[must_use]
    pub const fn produced_bytes(&self) -> u64 {
        self.produced_bytes
    }

    /// Whether the retained capture dropped anything.
    #[must_use]
    pub const fn truncated(&self) -> bool {
        self.truncated
    }

    /// Whether the attached mirror has stopped for good.
    #[must_use]
    pub const fn mirror_capped(&self) -> bool {
        self.mirror_capped
    }

    /// Accepts one chunk from the process.
    #[must_use]
    pub fn write(&mut self, stream: OutputStream, bytes: &[u8]) -> CaptureOutcome {
        self.produced_bytes = self.produced_bytes.saturating_add(bytes.len() as u64);

        let room = self.max_output_bytes.saturating_sub(self.retained_bytes);
        let keep = usize::try_from(room).unwrap_or(usize::MAX).min(bytes.len());
        if keep < bytes.len() {
            self.truncated = true;
        }
        let retain = bytes[..keep].to_vec();
        self.retained_bytes = self.retained_bytes.saturating_add(keep as u64);

        let mirror = if self.attached && !self.mirror_capped {
            self.mirror(stream, bytes)
        } else {
            Vec::new()
        };
        CaptureOutcome { retain, mirror }
    }

    /// Splits `bytes` into attached frames, respecting the stream cap.
    fn mirror(&mut self, stream: OutputStream, bytes: &[u8]) -> Vec<MirrorChunk> {
        let room = ATTACH_STREAM_CAP_BYTES.saturating_sub(self.mirrored_bytes);
        let allowed = usize::try_from(room).unwrap_or(usize::MAX).min(bytes.len());
        let mut frames = Vec::new();
        let mut cursor = 0usize;
        while cursor < allowed {
            let end = utf8_boundary(bytes, cursor, (cursor + self.frame_bytes).min(allowed));
            if end == cursor {
                // The frame ceiling is smaller than the next scalar. Emitting a
                // split scalar would corrupt the stream, so the mirror stops and
                // says so; the retained capture is unaffected.
                self.mirror_capped = true;
                break;
            }
            frames.push(MirrorChunk {
                stream,
                bytes: bytes[cursor..end].to_vec(),
                truncated: self.truncated,
            });
            cursor = end;
        }
        self.mirrored_bytes = self.mirrored_bytes.saturating_add(cursor as u64);
        if allowed < bytes.len() || self.mirrored_bytes >= ATTACH_STREAM_CAP_BYTES {
            self.mirror_capped = true;
            frames.push(MirrorChunk {
                stream,
                bytes: Vec::new(),
                truncated: true,
            });
        }
        frames
    }
}

/// The largest index in `start..=end` that does not split a UTF-8 scalar.
///
/// Walks back at most three bytes, which is the longest possible continuation
/// prefix, and returns `start` when the whole window is one partial scalar.
fn utf8_boundary(bytes: &[u8], start: usize, end: usize) -> usize {
    if end >= bytes.len() {
        return bytes.len();
    }
    let mut candidate = end;
    while candidate > start && is_continuation(bytes[candidate]) {
        candidate -= 1;
    }
    candidate
}

/// Whether a byte is a UTF-8 continuation byte.
const fn is_continuation(byte: u8) -> bool {
    byte & 0b1100_0000 == 0b1000_0000
}

#[cfg(test)]
mod tests {
    use super::{ATTACH_STREAM_CAP_BYTES, Capture};
    use aex_hands_protocol::operation::OperationBounds;
    use aex_hands_protocol::rpc::OutputStream;

    fn bounds(max_output_bytes: u64, max_frame_bytes: u32) -> OperationBounds {
        OperationBounds {
            max_output_bytes,
            max_frame_bytes,
            max_wall_ms: 600_000,
            max_concurrent_operations: 32,
        }
    }

    #[test]
    fn retention_stops_exactly_at_the_bound_and_the_process_keeps_producing() {
        let mut capture = Capture::new(&bounds(10, 1_048_576), false);
        let first = capture.write(OutputStream::Stdout, b"0123456789");
        assert_eq!(first.retain, b"0123456789");
        assert!(
            !capture.truncated(),
            "exactly at the limit is not truncation"
        );
        assert_eq!(capture.retained_bytes(), 10);

        let second = capture.write(OutputStream::Stdout, b"X");
        assert!(second.retain.is_empty(), "one byte over retains nothing");
        assert!(capture.truncated());
        assert_eq!(capture.retained_bytes(), 10);
        assert_eq!(
            capture.produced_bytes(),
            11,
            "the process keeps producing and the counter keeps counting"
        );
    }

    #[test]
    fn a_partial_write_retains_the_prefix_and_marks_truncation() {
        let mut capture = Capture::new(&bounds(4, 1_048_576), false);
        let outcome = capture.write(OutputStream::Stdout, b"abcdefgh");
        assert_eq!(outcome.retain, b"abcd");
        assert!(capture.truncated());
        assert_eq!(capture.retained_bytes(), 4);
        assert_eq!(capture.produced_bytes(), 8);
    }

    #[test]
    fn a_resumed_capture_carries_the_retained_length_forward() {
        let mut capture = Capture::resume(&bounds(10, 1_048_576), false, 8);
        assert_eq!(capture.retained_bytes(), 8);
        let outcome = capture.write(OutputStream::Stdout, b"abcd");
        assert_eq!(outcome.retain, b"ab");
        assert!(capture.truncated());
    }

    #[test]
    fn a_detached_operation_mirrors_nothing() {
        let mut capture = Capture::new(&bounds(1_000, 1_048_576), false);
        let outcome = capture.write(OutputStream::Stdout, b"hello");
        assert!(outcome.mirror.is_empty());
    }

    #[test]
    fn an_attached_mirror_never_splits_a_multi_byte_scalar() {
        // Four-byte scalars with a frame ceiling of five bytes: a naive split at
        // five would cut the second scalar in half.
        let text = "\u{1f600}\u{1f600}\u{1f600}".as_bytes();
        let mut capture = Capture::new(&bounds(1_000, 5), true);
        let outcome = capture.write(OutputStream::Stdout, text);
        let mut rejoined = Vec::new();
        for chunk in &outcome.mirror {
            assert!(
                std::str::from_utf8(&chunk.bytes).is_ok(),
                "a mirrored frame is always valid UTF-8"
            );
            rejoined.extend_from_slice(&chunk.bytes);
        }
        assert_eq!(rejoined, text);
    }

    #[test]
    fn a_frame_ceiling_below_one_scalar_stops_the_mirror_rather_than_corrupting_it() {
        let text = "\u{1f600}".as_bytes();
        let mut capture = Capture::new(&bounds(1_000, 2), true);
        let outcome = capture.write(OutputStream::Stdout, text);
        assert!(capture.mirror_capped());
        for chunk in &outcome.mirror {
            assert!(std::str::from_utf8(&chunk.bytes).is_ok());
        }
        assert_eq!(
            outcome.retain, text,
            "the retained capture is unaffected by a mirror problem"
        );
    }

    #[test]
    fn the_mirror_stops_at_its_cap_while_the_file_capture_continues() {
        let bounds = bounds(u64::MAX, 1_048_576);
        let mut capture = Capture::new(&bounds, true);
        let block = vec![b'x'; 1_000_000];
        for _ in 0..10 {
            let outcome = capture.write(OutputStream::Stdout, &block);
            assert_eq!(outcome.retain.len(), block.len());
        }
        assert!(capture.mirror_capped());
        let after = capture.write(OutputStream::Stdout, &block);
        assert_eq!(
            after.retain.len(),
            block.len(),
            "the retained capture keeps growing after the mirror stops"
        );
        assert!(after.mirror.is_empty());
        assert_eq!(capture.produced_bytes(), 11_000_000);
        assert!(!capture.truncated(), "the file capture dropped nothing");
        assert_eq!(ATTACH_STREAM_CAP_BYTES, 10_000_000);
    }

    #[test]
    fn the_final_mirror_frame_declares_the_truncation() {
        let bounds = bounds(u64::MAX, 1_048_576);
        let mut capture = Capture::new(&bounds, true);
        let block = vec![b'x'; 10_000_001];
        let outcome = capture.write(OutputStream::Stdout, &block);
        let last = outcome.mirror.last().expect("a final frame exists");
        assert!(last.bytes.is_empty());
        assert!(last.truncated);
        assert!(capture.mirror_capped());
    }

    #[test]
    fn both_streams_share_one_retention_budget() {
        let mut capture = Capture::new(&bounds(6, 1_048_576), false);
        assert_eq!(capture.write(OutputStream::Stdout, b"abc").retain, b"abc");
        assert_eq!(capture.write(OutputStream::Stderr, b"def").retain, b"def");
        assert!(capture.write(OutputStream::Stdout, b"g").retain.is_empty());
        assert!(capture.truncated());
    }
}
