//! The incremental, bounded server-sent-event decoder (plan 08 §3.4).
//!
//! All six providers stream `text/event-stream`. The decoder is fed arbitrary
//! byte chunks — a TCP segment can split a frame, a field, or a single UTF-8
//! scalar — and yields whole events or a typed error. It never allocates past
//! `max_frame_bytes`, so an unterminated frame from a hostile peer is a
//! rejection rather than a heap exhaustion.
//!
//! Deliberately absent: any interpretation of `data: [DONE]`. That sentinel is
//! present on `deepseek`, `zai` and `moonshotai` and absent on `openai`,
//! `anthropic` and `google`, so it is a **dialect** fact, surfaced as an
//! ordinary event and interpreted by the adapter.

/// The line terminator the SSE grammar joins and strips.
const NEWLINE: u8 = 0x0a;

/// One decoded event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent<'a> {
    /// The `event:` field, where the dialect sends one.
    pub name: Option<&'a str>,
    /// The joined `data:` payload. Multiple `data:` lines join with `\n`.
    pub data: &'a [u8],
    /// The `id:` field, where the dialect sends one.
    pub id: Option<&'a str>,
}

impl SseEvent<'_> {
    /// The payload as text.
    ///
    /// # Errors
    ///
    /// Returns [`SseError::InvalidUtf8`] when the payload is not valid UTF-8.
    /// The decoder already validated the frame, so this is a total-function
    /// guard rather than a reachable path.
    pub fn data_str(&self) -> Result<&str, SseError> {
        core::str::from_utf8(self.data).map_err(|error| SseError::InvalidUtf8 {
            offset: error.valid_up_to(),
        })
    }

    /// Whether the payload is the `[DONE]` sentinel three of the six dialects
    /// send.
    #[must_use]
    pub fn is_done_sentinel(&self) -> bool {
        self.data == b"[DONE]"
    }
}

/// Why decoding failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SseError {
    /// A single frame exceeded the configured bound.
    #[error("a frame of at least {seen} bytes exceeds the {limit}-byte bound")]
    FrameTooLarge {
        /// The bound.
        limit: u32,
        /// How much had accumulated when the bound was crossed.
        seen: u32,
    },
    /// The bytes are not valid UTF-8.
    #[error("invalid UTF-8 at offset {offset}")]
    InvalidUtf8 {
        /// Where the first invalid sequence starts.
        offset: usize,
    },
    /// The stream ended in the middle of a frame.
    #[error("the stream ended mid-frame")]
    UnterminatedFrame,
    /// A single field line exceeded the configured bound.
    #[error("a field line of at least {seen} bytes exceeds the {limit}-byte bound")]
    FieldTooLong {
        /// The bound.
        limit: u32,
        /// How long the line got.
        seen: u32,
    },
}

/// A bounded, incremental SSE decoder.
#[derive(Debug)]
pub struct SseDecoder {
    /// Bytes of the frame under construction, not yet split into lines.
    buffer: Vec<u8>,
    /// The joined `data:` payload of the frame under construction.
    data: Vec<u8>,
    /// The `event:` field of the frame under construction.
    name: Option<String>,
    /// The `id:` field of the frame under construction.
    id: Option<String>,
    /// Whether any field at all has been seen since the last dispatch.
    started: bool,
    /// The completed events waiting to be taken.
    ready: std::collections::VecDeque<OwnedEvent>,
    /// The per-frame byte bound.
    max_frame_bytes: u32,
    /// How many bytes the current frame has accumulated.
    frame_bytes: u32,
    /// Whether a failure has already been reported, so the decoder stays
    /// failed rather than silently resuming on the next chunk.
    poisoned: Option<SseError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OwnedEvent {
    name: Option<String>,
    data: Vec<u8>,
    id: Option<String>,
}

/// The bound on one `field: value` line, independent of the frame bound.
///
/// A frame is many lines; a single line longer than this is already a protocol
/// violation for every dialect in this set.
pub const MAX_FIELD_BYTES: u32 = 256 * 1024;

impl SseDecoder {
    /// Builds a decoder with a per-frame byte bound.
    #[must_use]
    pub fn new(max_frame_bytes: u32) -> Self {
        Self {
            buffer: Vec::new(),
            data: Vec::new(),
            name: None,
            id: None,
            started: false,
            ready: std::collections::VecDeque::new(),
            max_frame_bytes,
            frame_bytes: 0,
            poisoned: None,
        }
    }

    /// Feeds a chunk of arbitrary bytes.
    ///
    /// # Errors
    ///
    /// Returns [`SseError::FrameTooLarge`] or [`SseError::FieldTooLong`] as
    /// soon as a bound is crossed — before the buffer grows past it — and
    /// [`SseError::InvalidUtf8`] for a field name or a completed payload that
    /// is not valid UTF-8.
    pub fn push(&mut self, chunk: &[u8]) -> Result<(), SseError> {
        if let Some(error) = self.poisoned {
            return Err(error);
        }
        match self.push_inner(chunk) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.poisoned = Some(error);
                Err(error)
            }
        }
    }

    fn push_inner(&mut self, chunk: &[u8]) -> Result<(), SseError> {
        for &byte in chunk {
            // The bound is checked *before* the push, so the buffer never
            // exceeds it even by one byte.
            if self.frame_bytes >= self.max_frame_bytes {
                return Err(SseError::FrameTooLarge {
                    limit: self.max_frame_bytes,
                    seen: self.frame_bytes.saturating_add(1),
                });
            }
            if byte == b'\n' {
                let line = core::mem::take(&mut self.buffer);
                let line = strip_cr(&line);
                self.consume_line(line)?;
                continue;
            }
            if u32::try_from(self.buffer.len()).unwrap_or(u32::MAX) >= MAX_FIELD_BYTES {
                return Err(SseError::FieldTooLong {
                    limit: MAX_FIELD_BYTES,
                    seen: MAX_FIELD_BYTES.saturating_add(1),
                });
            }
            self.buffer.push(byte);
            self.frame_bytes = self.frame_bytes.saturating_add(1);
        }
        Ok(())
    }

    fn consume_line(&mut self, line: &[u8]) -> Result<(), SseError> {
        self.frame_bytes = self.frame_bytes.saturating_add(1);

        if line.is_empty() {
            // A blank line dispatches the frame — unless nothing has been
            // accumulated, in which case it is inter-frame padding.
            if self.started {
                self.dispatch();
            }
            return Ok(());
        }
        if line[0] == b':' {
            // A comment. `DeepSeek` sends `: keep-alive`, which must not
            // dispatch a frame and must not count as progress.
            return Ok(());
        }

        let (field, value) = split_field(line);
        let field = core::str::from_utf8(field).map_err(|error| SseError::InvalidUtf8 {
            offset: error.valid_up_to(),
        })?;
        match field {
            "data" => {
                // The SSE grammar appends the value plus a newline per line and
                // strips one trailing newline at dispatch. Joining with a
                // separator instead would silently drop an empty `data` line,
                // which is a real frame shape rather than padding.
                self.data.extend_from_slice(value);
                self.data.push(NEWLINE);
                self.started = true;
            }
            "event" => {
                let text = core::str::from_utf8(value).map_err(|error| SseError::InvalidUtf8 {
                    offset: error.valid_up_to(),
                })?;
                self.name = Some(text.to_owned());
                self.started = true;
            }
            "id" => {
                let text = core::str::from_utf8(value).map_err(|error| SseError::InvalidUtf8 {
                    offset: error.valid_up_to(),
                })?;
                self.id = Some(text.to_owned());
                self.started = true;
            }
            // `retry` and any unknown field are ignored per the SSE spec. They
            // still count as frame content for the byte bound, which the
            // caller has already been charged for.
            _ => {}
        }
        Ok(())
    }

    fn dispatch(&mut self) {
        let mut data = core::mem::take(&mut self.data);
        if data.last() == Some(&NEWLINE) {
            data.pop();
        }
        self.ready.push_back(OwnedEvent {
            name: self.name.take(),
            data,
            id: self.id.take(),
        });
        self.started = false;
        self.frame_bytes = 0;
    }

    /// Takes every complete event as owned values.
    ///
    /// # Errors
    ///
    /// As [`SseDecoder::next_event`].
    pub fn drain(&mut self) -> Result<Vec<DecodedEvent>, SseError> {
        let mut out = Vec::with_capacity(self.ready.len());
        while let Some(event) = self.ready.pop_front() {
            if let Err(invalid) = core::str::from_utf8(&event.data) {
                let error = SseError::InvalidUtf8 {
                    offset: invalid.valid_up_to(),
                };
                self.poisoned = Some(error);
                return Err(error);
            }
            out.push(DecodedEvent {
                name: event.name,
                data: event.data,
                id: event.id,
            });
        }
        Ok(out)
    }

    /// Ends the stream.
    ///
    /// # Errors
    ///
    /// Returns [`SseError::UnterminatedFrame`] when bytes have accumulated
    /// since the last blank line. A stream that stops mid-frame is a failure,
    /// never a silently shorter result.
    pub fn finish(self) -> Result<(), SseError> {
        if let Some(error) = self.poisoned {
            return Err(error);
        }
        if self.started || !self.buffer.is_empty() {
            return Err(SseError::UnterminatedFrame);
        }
        Ok(())
    }

    /// Whether any complete event is waiting.
    #[must_use]
    pub fn has_event(&self) -> bool {
        !self.ready.is_empty()
    }
}

/// An owned decoded event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedEvent {
    /// The `event:` field.
    pub name: Option<String>,
    /// The joined payload.
    pub data: Vec<u8>,
    /// The `id:` field.
    pub id: Option<String>,
}

impl DecodedEvent {
    /// Borrows the event.
    #[must_use]
    pub fn as_ref(&self) -> SseEvent<'_> {
        SseEvent {
            name: self.name.as_deref(),
            data: &self.data,
            id: self.id.as_deref(),
        }
    }

    /// The payload as text.
    ///
    /// # Errors
    ///
    /// As [`SseEvent::data_str`].
    pub fn data_str(&self) -> Result<&str, SseError> {
        core::str::from_utf8(&self.data).map_err(|error| SseError::InvalidUtf8 {
            offset: error.valid_up_to(),
        })
    }

    /// Whether the payload is the `[DONE]` sentinel.
    #[must_use]
    pub fn is_done_sentinel(&self) -> bool {
        self.data == b"[DONE]"
    }
}

fn strip_cr(line: &[u8]) -> &[u8] {
    match line.split_last() {
        Some((b'\r', rest)) => rest,
        _ => line,
    }
}

/// Splits `field: value`, dropping exactly one optional leading space from the
/// value, per the SSE grammar.
fn split_field(line: &[u8]) -> (&[u8], &[u8]) {
    match line.iter().position(|byte| *byte == b':') {
        None => (line, b""),
        Some(index) => {
            let value = &line[index + 1..];
            let value = match value.split_first() {
                Some((b' ', rest)) => rest,
                _ => value,
            };
            (&line[..index], value)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DecodedEvent, MAX_FIELD_BYTES, SseDecoder, SseError};

    fn decode_all(chunks: &[&[u8]], limit: u32) -> Result<Vec<DecodedEvent>, SseError> {
        let mut decoder = SseDecoder::new(limit);
        let mut out = Vec::new();
        for chunk in chunks {
            decoder.push(chunk)?;
            out.extend(decoder.drain()?);
        }
        decoder.finish()?;
        Ok(out)
    }

    #[test]
    fn a_whole_buffer_and_a_byte_at_a_time_feed_produce_identical_events() {
        let script = b"event: message_start\ndata: {\"type\":\"message_start\"}\n\n\
                       event: content_block_delta\ndata: {\"i\":0}\n\n";
        let whole = decode_all(&[script], 1024).expect("whole");
        let single: Vec<&[u8]> = script.iter().map(core::slice::from_ref).collect();
        let byte_at_a_time = decode_all(&single, 1024).expect("incremental");
        assert_eq!(whole, byte_at_a_time);
        assert_eq!(whole.len(), 2);
        assert_eq!(whole[0].name.as_deref(), Some("message_start"));
    }

    #[test]
    fn every_split_point_of_a_three_frame_script_decodes_identically() {
        let script: &[u8] = b"data: one\n\ndata: two\n\ndata: three\n\n";
        let reference = decode_all(&[script], 1024).expect("reference");
        for split in 0..=script.len() {
            let (left, right) = script.split_at(split);
            let split_feed = decode_all(&[left, right], 1024)
                .unwrap_or_else(|error| panic!("split at {split} failed: {error}"));
            assert_eq!(split_feed, reference, "split at {split}");
        }
    }

    #[test]
    fn a_multi_byte_scalar_split_across_chunks_decodes_once_whole() {
        // U+1F600, four bytes, split down the middle.
        let payload = "data: \u{1F600}\n\n".as_bytes();
        let (left, right) = payload.split_at(8);
        let events = decode_all(&[left, right], 1024).expect("split scalar");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data_str().expect("utf8"), "\u{1F600}");
    }

    #[test]
    fn a_truly_invalid_sequence_is_reported_with_its_offset() {
        let mut decoder = SseDecoder::new(1024);
        decoder.push(b"data: ab\xff\xfe\n\n").expect("push");
        let error = decoder.drain().expect_err("invalid UTF-8 must be reported");
        assert_eq!(error, SseError::InvalidUtf8 { offset: 2 });
    }

    #[test]
    fn a_poisoned_decoder_stays_failed() {
        let mut decoder = SseDecoder::new(1024);
        decoder.push(b"data: ab\xff\n\n").expect("push");
        decoder.drain().expect_err("first failure");
        assert!(decoder.push(b"data: fine\n\n").is_err());
        assert!(decoder.finish().is_err());
    }

    #[test]
    fn comment_lines_and_blank_padding_are_skipped() {
        let events = decode_all(
            &[b": keep-alive\n\n\n\ndata: real\n\n: keep-alive\n\n"],
            1024,
        )
        .expect("noise");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, b"real");
    }

    #[test]
    fn crlf_and_lf_both_terminate() {
        let crlf = decode_all(&[b"event: a\r\ndata: x\r\n\r\n"], 1024).expect("crlf");
        let lf = decode_all(&[b"event: a\ndata: x\n\n"], 1024).expect("lf");
        assert_eq!(crlf, lf);
    }

    #[test]
    fn multiple_data_lines_join_with_a_newline() {
        let events = decode_all(&[b"data: one\ndata: two\n\n"], 1024).expect("join");
        assert_eq!(events[0].data, b"one\ntwo");
    }

    #[test]
    fn an_id_field_is_carried() {
        let events = decode_all(&[b"id: 42\ndata: x\n\n"], 1024).expect("id");
        assert_eq!(events[0].id.as_deref(), Some("42"));
    }

    #[test]
    fn a_field_with_no_colon_is_a_field_with_an_empty_value() {
        let events = decode_all(&[b"data\ndata: x\n\n"], 1024).expect("bare field");
        assert_eq!(events[0].data, b"\nx");
    }

    #[test]
    fn exactly_one_leading_space_is_dropped_from_a_value() {
        let events = decode_all(&[b"data:  two spaces\n\n"], 1024).expect("spaces");
        assert_eq!(events[0].data, b" two spaces");
    }

    #[test]
    fn a_frame_one_byte_over_the_bound_fails_before_the_buffer_grows() {
        let payload = format!("data: {}\n\n", "x".repeat(64));
        let error = decode_all(&[payload.as_bytes()], 32).expect_err("over the bound");
        assert!(matches!(error, SseError::FrameTooLarge { limit: 32, .. }));
    }

    #[test]
    fn the_bound_resets_between_frames() {
        // Two frames each just inside the bound must both decode: the bound is
        // per frame, not per stream.
        let frame = format!("data: {}\n\n", "x".repeat(20));
        let script = format!("{frame}{frame}");
        let events = decode_all(&[script.as_bytes()], 32).expect("two bounded frames");
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn an_over_long_single_line_fails_even_under_a_huge_frame_bound() {
        let mut decoder = SseDecoder::new(u32::MAX);
        let line = vec![b'x'; MAX_FIELD_BYTES as usize + 16];
        let error = decoder
            .push(&line)
            .expect_err("an unterminated line is bounded");
        assert!(matches!(error, SseError::FieldTooLong { .. }));
    }

    #[test]
    fn a_stream_that_ends_mid_frame_fails() {
        let mut decoder = SseDecoder::new(1024);
        decoder.push(b"data: half a frame\n").expect("push");
        assert_eq!(decoder.finish(), Err(SseError::UnterminatedFrame));
    }

    #[test]
    fn a_stream_that_ends_after_a_terminal_frame_succeeds() {
        let mut decoder = SseDecoder::new(1024);
        decoder.push(b"data: whole\n\n").expect("push");
        assert_eq!(decoder.drain().expect("drain").len(), 1);
        decoder.finish().expect("a clean end");
    }

    #[test]
    fn a_slowloris_feed_never_completes_a_frame() {
        // One byte at a time with no terminator: the decoder must accumulate
        // nothing dispatchable, so the caller's idle timer is the only thing
        // that can end the stream.
        let mut decoder = SseDecoder::new(1024);
        for _ in 0..64 {
            decoder.push(b"x").expect("push");
            assert!(!decoder.has_event());
        }
        assert_eq!(decoder.finish(), Err(SseError::UnterminatedFrame));
    }

    #[test]
    fn the_done_sentinel_is_an_ordinary_event() {
        // Three of the six dialects send it and three do not, so the decoder
        // surfaces it rather than interpreting it.
        let events = decode_all(&[b"data: [DONE]\n\n"], 1024).expect("sentinel");
        assert_eq!(events.len(), 1);
        assert!(events[0].is_done_sentinel());
    }

    #[test]
    fn a_two_hundred_frame_script_decodes_in_order() {
        let mut script = Vec::new();
        for index in 0..200u32 {
            script.extend_from_slice(format!("data: {index}\n\n").as_bytes());
        }
        let events = decode_all(&[&script], 1024).expect("long stream");
        assert_eq!(events.len(), 200);
        for (index, event) in events.iter().enumerate() {
            assert_eq!(event.data_str().expect("utf8"), index.to_string());
        }
    }

    #[test]
    fn a_drained_event_borrows_as_an_sse_event() {
        let mut decoder = SseDecoder::new(1024);
        decoder
            .push(
                b"event: a
data: x

",
            )
            .expect("push");
        let events = decoder.drain().expect("drain");
        let borrowed = events[0].as_ref();
        assert_eq!(borrowed.name, Some("a"));
        assert_eq!(borrowed.data, b"x");
        assert!(!decoder.has_event());
    }
}
