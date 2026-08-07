//! The production exporter: one bounded JSON line per record, on a process log
//! stream the container log driver already ships.
//!
//! This is deliberately the whole transport for alpha. A JSON line on stdout
//! reaches `CloudWatch` through the ECS and Lambda log drivers with no
//! collector to deploy, no endpoint to configure, no retry queue to bound and
//! no network failure mode to reason about — so the first thing a long-lived
//! host gains is diagnostics that actually leave the process.
//!
//! Three properties are load-bearing and each has a test:
//!
//! 1. Redaction is not this file's job and never happens here. A record only
//!    reaches [`JsonLinesExporter`] through [`crate::facade::Handle::emit`],
//!    which has already removed every non-public attribute, so a forbidden
//!    value cannot be rendered even by a caller that supplies one.
//! 2. A record whose rendered line would exceed the ceiling is refused whole
//!    and replaced by a line naming the ceiling and the observed size. It is
//!    never truncated: a clipped JSON line is not JSON, and a silently
//!    shortened attribute is a diagnostic that lies.
//! 3. One batch is one locked, buffered write. A per-record write under load
//!    interleaves lines from other threads and multiplies syscalls on the
//!    flush path.

use std::io::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;

use crate::exporter::{ExportError, Exporter};
use crate::facade::TelemetryStats;
use crate::pump::StatsSink;
use crate::record::{AttributeValue, Record, RecordKind};

/// Where a [`JsonLinesExporter`] writes its rendered blocks.
///
/// A port rather than a hard-wired stream: an I/O failure has to be injectable,
/// and the stream a host's log driver reads is a composition decision.
pub trait LineSink: Send + Sync {
    /// Writes one already-rendered block of newline-terminated lines.
    ///
    /// # Errors
    ///
    /// Returns the underlying I/O error. The exporter turns that into a counted
    /// diagnostic loss; it never reaches product code.
    fn write_block(&self, block: &[u8]) -> std::io::Result<()>;
}

/// The process stream a host writes diagnostics to.
///
/// Named rather than a boolean, because which stream carries machine-readable
/// lines is a routing decision a reader of the composition root must see. The
/// hosts here put JSON on stdout and leave stderr to the human-readable
/// start-up and shutdown messages, so one stream stays parseable in full.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogStream {
    /// Standard output.
    Stdout,
    /// Standard error.
    Stderr,
}

impl LineSink for LogStream {
    fn write_block(&self, block: &[u8]) -> std::io::Result<()> {
        match self {
            Self::Stdout => {
                let mut stream = std::io::stdout().lock();
                stream.write_all(block)?;
                stream.flush()
            }
            Self::Stderr => {
                let mut stream = std::io::stderr().lock();
                stream.write_all(block)?;
                stream.flush()
            }
        }
    }
}

/// A configured line ceiling too small to render any record at all.
///
/// Refused at construction rather than at the first export, because a ceiling
/// that rejects every record is a silent telemetry outage that looks like a
/// quiet process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "a telemetry line ceiling of {requested} byte(s) is below the {minimum}-byte minimum and would refuse every record"
)]
pub struct LineCeilingTooSmall {
    /// The rejected ceiling.
    pub requested: usize,
    /// The smallest ceiling that can render a record.
    pub minimum: usize,
}

/// Capacity kept between batches so a steady-state process stops reallocating
/// without holding a rogue record's rendering for the life of the host.
const RETAINED_BUFFER_BYTES: usize = 64 * 1024;

/// Escapes for every code point below `0x20`, indexed by that code point.
///
/// A table rather than arithmetic: the escape for a control character is fixed
/// by RFC 8259, and a table cannot get the nibble order wrong.
const CONTROL_ESCAPES: [&[u8]; 32] = [
    b"\\u0000", b"\\u0001", b"\\u0002", b"\\u0003", b"\\u0004", b"\\u0005", b"\\u0006", b"\\u0007",
    b"\\b", b"\\t", b"\\n", b"\\u000b", b"\\f", b"\\r", b"\\u000e", b"\\u000f", b"\\u0010",
    b"\\u0011", b"\\u0012", b"\\u0013", b"\\u0014", b"\\u0015", b"\\u0016", b"\\u0017", b"\\u0018",
    b"\\u0019", b"\\u001a", b"\\u001b", b"\\u001c", b"\\u001d", b"\\u001e", b"\\u001f",
];

/// Writes redacted records as one bounded JSON line each.
pub struct JsonLinesExporter {
    sink: Arc<dyn LineSink>,
    line_ceiling_bytes: usize,
    buffer: Mutex<Vec<u8>>,
    written: AtomicU64,
    refused: AtomicU64,
}

impl JsonLinesExporter {
    /// The default ceiling on one rendered line, excluding its newline.
    ///
    /// Converts the container log driver's line split: Docker's `json-file`
    /// driver, which the ECS `awslogs` path is built on, breaks a line longer
    /// than 16 `KiB` into separate log events, and half a JSON object is not
    /// something a `CloudWatch` Logs Insights query can parse. 8 `KiB` sits
    /// well under that split and far above any record the registry can
    /// legitimately produce.
    pub const DEFAULT_LINE_CEILING_BYTES: usize = 8 * 1024;

    /// The smallest ceiling [`Self::with_line_ceiling`] accepts.
    ///
    /// A record carrying nothing but its registry-declared name still renders
    /// its envelope, so a ceiling below this refuses every record rather than
    /// bounding an unusual one.
    pub const MINIMUM_LINE_CEILING_BYTES: usize = 256;

    /// Writes to `sink` with [`Self::DEFAULT_LINE_CEILING_BYTES`].
    #[must_use]
    pub fn new(sink: Arc<dyn LineSink>) -> Self {
        Self {
            sink,
            line_ceiling_bytes: Self::DEFAULT_LINE_CEILING_BYTES,
            buffer: Mutex::new(Vec::new()),
            written: AtomicU64::new(0),
            refused: AtomicU64::new(0),
        }
    }

    /// Writes to `sink`, refusing any record whose line exceeds
    /// `line_ceiling_bytes`.
    ///
    /// # Errors
    ///
    /// Returns [`LineCeilingTooSmall`] when `line_ceiling_bytes` is below
    /// [`Self::MINIMUM_LINE_CEILING_BYTES`].
    pub fn with_line_ceiling(
        sink: Arc<dyn LineSink>,
        line_ceiling_bytes: usize,
    ) -> Result<Self, LineCeilingTooSmall> {
        if line_ceiling_bytes < Self::MINIMUM_LINE_CEILING_BYTES {
            return Err(LineCeilingTooSmall {
                requested: line_ceiling_bytes,
                minimum: Self::MINIMUM_LINE_CEILING_BYTES,
            });
        }
        Ok(Self {
            sink,
            line_ceiling_bytes,
            buffer: Mutex::new(Vec::new()),
            written: AtomicU64::new(0),
            refused: AtomicU64::new(0),
        })
    }

    /// Records rendered in full and written.
    #[must_use]
    pub fn written(&self) -> u64 {
        self.written.load(Ordering::Relaxed)
    }

    /// Records refused for exceeding the line ceiling.
    ///
    /// Each one left a refusal line in the log naming the ceiling it hit, so
    /// this counter and the log agree.
    #[must_use]
    pub fn refused(&self) -> u64 {
        self.refused.load(Ordering::Relaxed)
    }

    /// The ceiling one rendered line may not exceed.
    #[must_use]
    pub const fn line_ceiling_bytes(&self) -> usize {
        self.line_ceiling_bytes
    }
}

impl std::fmt::Debug for JsonLinesExporter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The sink and the render buffer are deliberately absent: a sink has no
        // useful rendering, and printing the buffer would put record contents in
        // whatever a caller formatted this into.
        formatter
            .debug_struct("JsonLinesExporter")
            .field("line_ceiling_bytes", &self.line_ceiling_bytes)
            .field("written", &self.written())
            .field("refused", &self.refused())
            .finish_non_exhaustive()
    }
}

impl Exporter for JsonLinesExporter {
    fn export(&self, batch: &[Record]) -> Result<(), ExportError> {
        let mut buffer = self.buffer.lock();
        buffer.clear();
        let mut written = 0_u64;
        let mut refused = 0_u64;
        for record in batch {
            let line_start = buffer.len();
            render_record(&mut buffer, record);
            let encoded_bytes = buffer.len() - line_start;
            if encoded_bytes > self.line_ceiling_bytes {
                buffer.truncate(line_start);
                render_refusal(
                    &mut buffer,
                    record.name,
                    encoded_bytes,
                    self.line_ceiling_bytes,
                );
                refused += 1;
            } else {
                written += 1;
            }
            buffer.push(b'\n');
        }
        let result = self.sink.write_block(&buffer);
        if buffer.capacity() > RETAINED_BUFFER_BYTES {
            buffer.shrink_to(RETAINED_BUFFER_BYTES);
        }
        drop(buffer);
        match result {
            Ok(()) => {
                self.written.fetch_add(written, Ordering::Relaxed);
                self.refused.fetch_add(refused, Ordering::Relaxed);
                Ok(())
            }
            // Nothing reached the log, so nothing is counted as written or
            // refused; the facade counts the whole batch as a drop instead.
            Err(error) => Err(ExportError::Refused {
                reason: error.to_string(),
            }),
        }
    }
}

impl StatsSink for JsonLinesExporter {
    fn publish(&self, stats: &TelemetryStats) {
        let mut buffer = self.buffer.lock();
        buffer.clear();
        render_stats(&mut buffer, stats);
        buffer.push(b'\n');
        // A failed stats write is counted nowhere on purpose: the counters it
        // would have reported are exactly the ones that could not be written,
        // so a counter for it would have the same fate.
        let _ = self.sink.write_block(&buffer);
        if buffer.capacity() > RETAINED_BUFFER_BYTES {
            buffer.shrink_to(RETAINED_BUFFER_BYTES);
        }
    }
}

/// The `kind` discriminator of a record line.
const fn kind_name(kind: RecordKind) -> &'static str {
    match kind {
        RecordKind::Span => "span",
        RecordKind::Event => "event",
        RecordKind::Metric => "metric",
    }
}

/// Renders one record with a fixed field set in a fixed order.
///
/// `value` is present as `null` on a non-metric rather than omitted, so every
/// record line has one shape and a query never has to test for a missing field.
/// There is no timestamp: the log driver stamps every line, and a second clock
/// here would be a nondeterministic field in a golden test.
fn render_record(out: &mut Vec<u8>, record: &Record) {
    out.extend_from_slice(br#"{"kind":"#);
    push_string(out, kind_name(record.kind));
    out.extend_from_slice(br#","name":"#);
    push_string(out, record.name);
    out.extend_from_slice(br#","value":"#);
    match record.value {
        Some(value) => push_number(out, value),
        None => out.extend_from_slice(b"null"),
    }
    out.extend_from_slice(br#","attributes":{"#);
    for (index, attribute) in record.attributes.iter().enumerate() {
        if index > 0 {
            out.push(b',');
        }
        push_string(out, attribute.key);
        out.push(b':');
        match &attribute.value {
            AttributeValue::Text(text) => push_string(out, text),
            AttributeValue::Integer(value) => push_number(out, *value),
            AttributeValue::Boolean(value) => {
                out.extend_from_slice(if *value { b"true" } else { b"false" });
            }
        }
    }
    out.extend_from_slice(b"}}");
}

/// Renders the line that replaces a refused record.
///
/// Bounded by construction — its only variable-length part is a
/// registry-declared name — so it is not itself subject to the ceiling it
/// reports. It names the ceiling and the observed size because the fix is
/// always a narrower attribute, and that has to be readable from the log alone.
fn render_refusal(out: &mut Vec<u8>, name: &str, encoded_bytes: usize, line_ceiling_bytes: usize) {
    out.extend_from_slice(br#"{"kind":"refused","name":"#);
    push_string(out, name);
    out.extend_from_slice(br#","encoded_bytes":"#);
    push_number(out, encoded_bytes);
    out.extend_from_slice(br#","line_ceiling_bytes":"#);
    push_number(out, line_ceiling_bytes);
    out.push(b'}');
}

/// Renders the pump's own queue state.
///
/// Its `kind` is outside the record kinds so one query can separate the
/// telemetry system's state from the diagnostics it carries.
fn render_stats(out: &mut Vec<u8>, stats: &TelemetryStats) {
    out.extend_from_slice(br#"{"kind":"telemetry_stats","pending":"#);
    push_number(out, stats.pending);
    out.extend_from_slice(br#","queue_capacity":"#);
    push_number(out, stats.queue_capacity);
    out.extend_from_slice(br#","emitted":"#);
    push_number(out, stats.emitted());
    out.extend_from_slice(br#","accepted":"#);
    push_number(out, stats.accepted);
    out.extend_from_slice(br#","exported":"#);
    push_number(out, stats.exported);
    out.extend_from_slice(br#","dropped":"#);
    push_number(out, stats.dropped);
    out.extend_from_slice(br#","redacted_attributes":"#);
    push_number(out, stats.redacted_attributes);
    out.extend_from_slice(br#","flush_deadline_exceeded":"#);
    push_number(out, stats.flush_deadline_exceeded);
    out.push(b'}');
}

/// Appends `value` as a quoted JSON string.
///
/// Byte-wise rather than character-wise: every byte that needs escaping is
/// ASCII, so a multi-byte sequence passes through untouched and the output
/// stays valid UTF-8 without a re-encode.
fn push_string(out: &mut Vec<u8>, value: &str) {
    out.push(b'"');
    for &byte in value.as_bytes() {
        match byte {
            b'"' => out.extend_from_slice(b"\\\""),
            b'\\' => out.extend_from_slice(b"\\\\"),
            0x00..=0x1f => out.extend_from_slice(CONTROL_ESCAPES[usize::from(byte)]),
            _ => out.push(byte),
        }
    }
    out.push(b'"');
}

/// Appends `value` as a JSON number.
fn push_number(out: &mut Vec<u8>, value: impl std::fmt::Display) {
    out.extend_from_slice(value.to_string().as_bytes());
}
