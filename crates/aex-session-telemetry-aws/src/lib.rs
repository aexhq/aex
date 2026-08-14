//! Customer-safe, immutable session telemetry segments.
//!
//! This crate deliberately has no raw OTLP-input API. It accepts only bounded,
//! normalized public [`TelemetryFrame`] values produced by the closed Brain and
//! Tool Mux adapters, then creates the official OpenTelemetry protobuf message
//! itself. Producer adapters own privacy classification and redact reasoning,
//! partial tool arguments, credentials and private tracing before this storage
//! boundary.

use std::io::Read as _;
use std::time::Duration;

use aex_wire::ids::{SessionId, SpanId, TraceId};
use aex_wire::models::{TelemetryFrame, TelemetryKind};
use aex_wire::types::{DecimalU128, Timestamp};
use aws_sdk_s3::error::ProvideErrorMetadata as _;
use aws_sdk_s3::presigning::PresigningConfig;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{ChecksumAlgorithm, ServerSideEncryption};
use base64::Engine as _;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue, any_value};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs, SeverityNumber};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use prost::Message as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// MIME type of every immutable segment.
pub const OTLP_PROTOBUF_MEDIA_TYPE: &str = "application/x-protobuf";
/// MIME type of the closed Aex header followed by zstd-compressed OTLP protobuf.
pub const TELEMETRY_SEGMENT_MEDIA_TYPE: &str = "application/vnd.aex.telemetry-segment+protobuf";
/// MIME type of bounded zstd-compressed customer download packages.
pub const TELEMETRY_EXPORT_MEDIA_TYPE: &str = "application/x-ndjson+zstd";
/// Maximum compressed segment size accepted from S3.
pub const MAX_SEGMENT_BYTES: usize = 4 * 1024 * 1024;
/// Maximum decompressed OTLP batch size.
pub const MAX_DECOMPRESSED_BYTES: usize = 8 * 1024 * 1024;
/// Maximum records in one immutable segment.
pub const MAX_SEGMENT_RECORDS: usize = 128;
/// Maximum objects returned by one public list request or deletion pass.
pub const MAX_PAGE_ITEMS: i32 = 100;
/// Lifetime of a customer download grant.
pub const DOWNLOAD_GRANT_TTL: Duration = Duration::from_mins(5);

const INSTRUMENTATION_SCOPE: &str = "aex.session";
const SERVICE_NAME: &str = "aex";
const SEGMENT_MAGIC: &[u8; 8] = b"AEXTEL01";
const HEADER_VERSION: u16 = 1;

/// A deterministic official OTLP protobuf segment ready for object storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedSegment {
    /// Stable public segment id: zero-padded sequence plus content hash.
    pub id: String,
    /// SHA-256 of the exact header plus compressed protobuf bytes.
    pub sha256: String,
    /// Closed header plus zstd-compressed official OTLP protobuf bytes.
    pub bytes: Vec<u8>,
}

/// One immutable object visible to the finite session API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentDescriptor {
    /// Stable segment id (never a raw object key).
    pub id: String,
    /// First ordered producer-local sequence.
    pub sequence: u64,
    /// Last ordered producer-local sequence in this segment.
    pub last_sequence: u64,
    /// SHA-256 parsed from the content-addressed key.
    pub sha256: String,
    /// Object size in bytes.
    pub size_bytes: u64,
    /// S3 last-modified instant in epoch milliseconds, when supplied by S3.
    pub created_at_ms: Option<i64>,
}

/// One bounded S3 listing page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentPage {
    /// Strictly validated objects under the session prefix.
    pub segments: Vec<SegmentDescriptor>,
    /// Opaque S3 continuation state. Public callers receive it only inside the
    /// API's authenticated cursor envelope.
    pub next: Option<String>,
}

/// Immutable export package metadata and private object key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportDescriptor {
    /// Exact object digest.
    pub sha256: String,
    /// Compressed package size.
    pub size_bytes: u64,
    object_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SignalKind {
    Logs,
    Spans,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SegmentHeader {
    schema_version: u16,
    session_id: String,
    signal: SignalKind,
    first_sequence: u64,
    last_sequence: u64,
    record_count: u32,
    uncompressed_bytes: u64,
    content_sha256: String,
}

/// Why encoding or immutable S3 storage failed.
#[derive(Debug, thiserror::Error)]
pub enum SessionTelemetryError {
    /// A timestamp cannot be represented by OTLP's unsigned nanoseconds.
    #[error("the session event timestamp is outside the OTLP range")]
    TimestampRange,
    /// The closed schema unexpectedly exceeded its hard byte ceiling.
    #[error("the OTLP segment exceeds the hard size ceiling")]
    SegmentTooLarge,
    /// A batch is empty, mixes signal families, or exceeds record limits.
    #[error("the telemetry batch is outside the closed segment contract")]
    InvalidBatch,
    /// A retained segment failed header, hash, compression, or OTLP validation.
    #[error("the retained telemetry segment is corrupt")]
    CorruptSegment,
    /// A segment id does not match the content-addressed grammar.
    #[error("the session telemetry segment id is malformed")]
    InvalidSegmentId,
    /// A requested page size is outside the bounded contract.
    #[error("the session telemetry page size is outside 1..={MAX_PAGE_ITEMS}")]
    InvalidPageSize,
    /// An object returned beneath the owned prefix violates the key grammar.
    #[error("the session telemetry prefix contains a malformed object key")]
    InvalidStoredKey,
    /// The requested immutable object does not exist.
    #[error("the session telemetry segment does not exist")]
    NotFound,
    /// S3 refused an operation.
    #[error("the session telemetry object store refused `{operation}`")]
    Store {
        /// Stable operation class; the SDK error text is intentionally not
        /// propagated through public or diagnostic surfaces.
        operation: &'static str,
    },
}

/// Encodes a bounded same-signal batch as a closed header and zstd-compressed
/// official OTLP Logs or Traces protobuf body.
///
/// # Errors
///
/// Rejects empty, mixed-signal, oversized, invalid timestamp, or invalid
/// correlation input before object storage.
pub fn encode_frames(
    session: SessionId,
    frames: &[TelemetryFrame],
) -> Result<EncodedSegment, SessionTelemetryError> {
    if frames.is_empty() || frames.len() > MAX_SEGMENT_RECORDS {
        return Err(SessionTelemetryError::InvalidBatch);
    }
    let signal = if frames[0].kind == TelemetryKind::Span {
        SignalKind::Spans
    } else {
        SignalKind::Logs
    };
    if frames
        .iter()
        .any(|frame| (frame.kind == TelemetryKind::Span) != (signal == SignalKind::Spans))
    {
        return Err(SessionTelemetryError::InvalidBatch);
    }
    let first_sequence = frame_sequence(&frames[0])?;
    let mut last_sequence = first_sequence;
    for frame in frames {
        let sequence = frame_sequence(frame)?;
        if sequence < last_sequence {
            return Err(SessionTelemetryError::InvalidBatch);
        }
        last_sequence = sequence;
    }
    let protobuf = match signal {
        SignalKind::Logs => encode_logs(frames)?.encode_to_vec(),
        SignalKind::Spans => encode_spans(frames)?.encode_to_vec(),
    };
    if protobuf.len() > MAX_DECOMPRESSED_BYTES {
        return Err(SessionTelemetryError::SegmentTooLarge);
    }
    let content_sha256 = hex::encode(Sha256::digest(&protobuf));
    let header = SegmentHeader {
        schema_version: HEADER_VERSION,
        session_id: session.to_string(),
        signal,
        first_sequence,
        last_sequence,
        record_count: u32::try_from(frames.len())
            .map_err(|_| SessionTelemetryError::InvalidBatch)?,
        uncompressed_bytes: u64::try_from(protobuf.len())
            .map_err(|_| SessionTelemetryError::SegmentTooLarge)?,
        content_sha256,
    };
    let header = serde_json::to_vec(&header).map_err(|_| SessionTelemetryError::InvalidBatch)?;
    let compressed = zstd::stream::encode_all(protobuf.as_slice(), 3)
        .map_err(|_| SessionTelemetryError::CorruptSegment)?;
    let header_len =
        u32::try_from(header.len()).map_err(|_| SessionTelemetryError::SegmentTooLarge)?;
    let mut bytes = Vec::with_capacity(SEGMENT_MAGIC.len() + 4 + header.len() + compressed.len());
    bytes.extend_from_slice(SEGMENT_MAGIC);
    bytes.extend_from_slice(&header_len.to_be_bytes());
    bytes.extend_from_slice(&header);
    bytes.extend_from_slice(&compressed);
    if bytes.len() > MAX_SEGMENT_BYTES {
        return Err(SessionTelemetryError::SegmentTooLarge);
    }
    let sha256 = hex::encode(Sha256::digest(&bytes));
    Ok(EncodedSegment {
        id: format!("{first_sequence:020}-{last_sequence:020}-{sha256}"),
        sha256,
        bytes,
    })
}

/// Validates and decodes an encoded segment without object storage.
///
/// This is the deterministic compatibility seam used by official OTLP fixture
/// tests and by readers after the S3 content hash has been verified.
///
/// # Errors
///
/// Returns a corruption error for any invalid header, compression, hash, or
/// Logs/Traces protobuf body.
pub fn decode_encoded(
    session: SessionId,
    segment: &EncodedSegment,
) -> Result<Vec<TelemetryFrame>, SessionTelemetryError> {
    if hex::encode(Sha256::digest(&segment.bytes)) != segment.sha256 {
        return Err(SessionTelemetryError::CorruptSegment);
    }
    decode_frames(session, &segment.bytes)
}

fn frame_sequence(frame: &TelemetryFrame) -> Result<u64, SessionTelemetryError> {
    u64::try_from(frame.sequence.get()).map_err(|_| SessionTelemetryError::InvalidBatch)
}

fn frame_nanos(frame: &TelemetryFrame) -> Result<u64, SessionTelemetryError> {
    u64::try_from(frame.occurred_at.unix_millis())
        .map_err(|_| SessionTelemetryError::TimestampRange)?
        .checked_mul(1_000_000)
        .ok_or(SessionTelemetryError::TimestampRange)
}

fn frame_attributes(frame: &TelemetryFrame) -> Result<Vec<KeyValue>, SessionTelemetryError> {
    let body = frame
        .body
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|_| SessionTelemetryError::InvalidBatch)?;
    let mut attributes = vec![
        string_attribute("aex.telemetry.kind", frame.kind.as_str()),
        string_attribute("aex.telemetry.sequence", &frame.sequence.get().to_string()),
        string_attribute(
            "aex.telemetry.truncated",
            if frame.truncated { "true" } else { "false" },
        ),
    ];
    if let Some(body) = body {
        attributes.push(string_attribute("aex.telemetry.body", &body));
    }
    if let Some(preview) = frame.preview.as_deref() {
        attributes.push(string_attribute("aex.telemetry.preview", preview));
    }
    Ok(attributes)
}

fn common_resource() -> Resource {
    Resource {
        attributes: vec![string_attribute("service.name", SERVICE_NAME)],
        dropped_attributes_count: 0,
        entity_refs: Vec::new(),
    }
}

fn common_scope() -> InstrumentationScope {
    InstrumentationScope {
        name: INSTRUMENTATION_SCOPE.to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        attributes: Vec::new(),
        dropped_attributes_count: 0,
    }
}

fn encode_logs(
    frames: &[TelemetryFrame],
) -> Result<ExportLogsServiceRequest, SessionTelemetryError> {
    let mut log_records = Vec::with_capacity(frames.len());
    for frame in frames {
        let nanos = frame_nanos(frame)?;
        log_records.push(LogRecord {
            time_unix_nano: nanos,
            observed_time_unix_nano: nanos,
            severity_number: SeverityNumber::Info as i32,
            severity_text: "INFO".to_owned(),
            body: Some(string_value("aex.telemetry.frame")),
            attributes: frame_attributes(frame)?,
            dropped_attributes_count: 0,
            flags: 0,
            trace_id: frame
                .trace_id
                .map_or_else(Vec::new, |id| id.as_bytes().to_vec()),
            span_id: frame
                .span_id
                .map_or_else(Vec::new, |id| id.as_bytes().to_vec()),
            event_name: String::new(),
        });
    }
    Ok(ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(common_resource()),
            scope_logs: vec![ScopeLogs {
                scope: Some(common_scope()),
                log_records,
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    })
}

fn encode_spans(
    frames: &[TelemetryFrame],
) -> Result<ExportTraceServiceRequest, SessionTelemetryError> {
    let mut spans = Vec::with_capacity(frames.len());
    for frame in frames {
        let nanos = frame_nanos(frame)?;
        let trace_id = frame.trace_id.ok_or(SessionTelemetryError::InvalidBatch)?;
        let span_id = frame.span_id.ok_or(SessionTelemetryError::InvalidBatch)?;
        spans.push(Span {
            trace_id: trace_id.as_bytes().to_vec(),
            span_id: span_id.as_bytes().to_vec(),
            trace_state: String::new(),
            parent_span_id: Vec::new(),
            flags: 1,
            name: "aex.telemetry.span".to_owned(),
            kind: opentelemetry_proto::tonic::trace::v1::span::SpanKind::Internal as i32,
            start_time_unix_nano: nanos,
            end_time_unix_nano: nanos,
            attributes: frame_attributes(frame)?,
            dropped_attributes_count: 0,
            events: Vec::new(),
            dropped_events_count: 0,
            links: Vec::new(),
            dropped_links_count: 0,
            status: None,
        });
    }
    Ok(ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(common_resource()),
            scope_spans: vec![ScopeSpans {
                scope: Some(common_scope()),
                spans,
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    })
}

fn parse_header_and_body(
    session: SessionId,
    bytes: &[u8],
) -> Result<(SegmentHeader, Vec<u8>), SessionTelemetryError> {
    if bytes.len() < SEGMENT_MAGIC.len() + 4 || &bytes[..SEGMENT_MAGIC.len()] != SEGMENT_MAGIC {
        return Err(SessionTelemetryError::CorruptSegment);
    }
    let length_offset = SEGMENT_MAGIC.len();
    let header_len = u32::from_be_bytes(
        bytes[length_offset..length_offset + 4]
            .try_into()
            .map_err(|_| SessionTelemetryError::CorruptSegment)?,
    );
    let header_len =
        usize::try_from(header_len).map_err(|_| SessionTelemetryError::CorruptSegment)?;
    let header_start = length_offset + 4;
    let header_end = header_start
        .checked_add(header_len)
        .filter(|end| *end < bytes.len())
        .ok_or(SessionTelemetryError::CorruptSegment)?;
    let header: SegmentHeader = serde_json::from_slice(&bytes[header_start..header_end])
        .map_err(|_| SessionTelemetryError::CorruptSegment)?;
    if header.schema_version != HEADER_VERSION || header.session_id != session.to_string() {
        return Err(SessionTelemetryError::CorruptSegment);
    }
    let decoder = zstd::stream::read::Decoder::new(&bytes[header_end..])
        .map_err(|_| SessionTelemetryError::CorruptSegment)?;
    let mut body = Vec::new();
    decoder
        .take(u64::try_from(MAX_DECOMPRESSED_BYTES + 1).expect("bounded"))
        .read_to_end(&mut body)
        .map_err(|_| SessionTelemetryError::CorruptSegment)?;
    if body.len() > MAX_DECOMPRESSED_BYTES
        || body.len()
            != usize::try_from(header.uncompressed_bytes)
                .map_err(|_| SessionTelemetryError::CorruptSegment)?
        || hex::encode(Sha256::digest(&body)) != header.content_sha256
    {
        return Err(SessionTelemetryError::CorruptSegment);
    }
    Ok((header, body))
}

fn attribute<'a>(attributes: &'a [KeyValue], key: &str) -> Option<&'a str> {
    attributes.iter().find_map(|attribute| {
        (attribute.key == key)
            .then_some(attribute.value.as_ref())
            .flatten()
            .and_then(|value| match value.value.as_ref() {
                Some(any_value::Value::StringValue(value)) => Some(value.as_str()),
                _ => None,
            })
    })
}

fn decode_kind(value: &str) -> Result<TelemetryKind, SessionTelemetryError> {
    match value {
        "assistant" => Ok(TelemetryKind::Assistant),
        "tool" => Ok(TelemetryKind::Tool),
        "runtime" => Ok(TelemetryKind::Runtime),
        "log" => Ok(TelemetryKind::Log),
        "span" => Ok(TelemetryKind::Span),
        "usage" => Ok(TelemetryKind::Usage),
        "gap" => Ok(TelemetryKind::Gap),
        "heartbeat" => Ok(TelemetryKind::Heartbeat),
        _ => Err(SessionTelemetryError::CorruptSegment),
    }
}

fn decode_frame(
    attributes: &[KeyValue],
    nanos: u64,
    trace_id: &[u8],
    span_id: &[u8],
) -> Result<TelemetryFrame, SessionTelemetryError> {
    let occurred_at = Timestamp::from_unix_millis(
        i64::try_from(nanos / 1_000_000).map_err(|_| SessionTelemetryError::CorruptSegment)?,
    )
    .map_err(|_| SessionTelemetryError::CorruptSegment)?;
    let sequence = attribute(attributes, "aex.telemetry.sequence")
        .ok_or(SessionTelemetryError::CorruptSegment)?
        .parse::<u128>()
        .map_err(|_| SessionTelemetryError::CorruptSegment)?;
    let kind = decode_kind(
        attribute(attributes, "aex.telemetry.kind").ok_or(SessionTelemetryError::CorruptSegment)?,
    )?;
    let truncated = match attribute(attributes, "aex.telemetry.truncated") {
        Some("true") => true,
        Some("false") => false,
        _ => return Err(SessionTelemetryError::CorruptSegment),
    };
    let body = attribute(attributes, "aex.telemetry.body")
        .map(|value| {
            serde_json::from_str(value)
                .map_err(|_| SessionTelemetryError::CorruptSegment)
                .and_then(|value| {
                    aex_wire::CanonicalJson::from_value(&value)
                        .map_err(|_| SessionTelemetryError::CorruptSegment)
                })
        })
        .transpose()?;
    let trace_id = decode_trace_id(trace_id)?;
    let span_id = decode_span_id(span_id)?;
    Ok(TelemetryFrame {
        sequence: DecimalU128::new(sequence),
        kind,
        occurred_at,
        trace_id,
        span_id,
        body,
        preview: attribute(attributes, "aex.telemetry.preview").map(str::to_owned),
        truncated,
    })
}

fn decode_trace_id(bytes: &[u8]) -> Result<Option<TraceId>, SessionTelemetryError> {
    if bytes.is_empty() {
        return Ok(None);
    }
    TraceId::from_bytes(
        bytes
            .try_into()
            .map_err(|_| SessionTelemetryError::CorruptSegment)?,
    )
    .map(Some)
    .map_err(|_| SessionTelemetryError::CorruptSegment)
}

fn decode_span_id(bytes: &[u8]) -> Result<Option<SpanId>, SessionTelemetryError> {
    if bytes.is_empty() {
        return Ok(None);
    }
    SpanId::from_bytes(
        bytes
            .try_into()
            .map_err(|_| SessionTelemetryError::CorruptSegment)?,
    )
    .map(Some)
    .map_err(|_| SessionTelemetryError::CorruptSegment)
}

fn decode_frames(
    session: SessionId,
    bytes: &[u8],
) -> Result<Vec<TelemetryFrame>, SessionTelemetryError> {
    let (header, protobuf) = parse_header_and_body(session, bytes)?;
    let mut frames = Vec::with_capacity(usize::try_from(header.record_count).unwrap_or_default());
    match header.signal {
        SignalKind::Logs => {
            let request = ExportLogsServiceRequest::decode(protobuf.as_slice())
                .map_err(|_| SessionTelemetryError::CorruptSegment)?;
            for resource in request.resource_logs {
                for scope in resource.scope_logs {
                    for record in scope.log_records {
                        frames.push(decode_frame(
                            &record.attributes,
                            record.time_unix_nano,
                            &record.trace_id,
                            &record.span_id,
                        )?);
                    }
                }
            }
        }
        SignalKind::Spans => {
            let request = ExportTraceServiceRequest::decode(protobuf.as_slice())
                .map_err(|_| SessionTelemetryError::CorruptSegment)?;
            for resource in request.resource_spans {
                for scope in resource.scope_spans {
                    for span in scope.spans {
                        frames.push(decode_frame(
                            &span.attributes,
                            span.start_time_unix_nano,
                            &span.trace_id,
                            &span.span_id,
                        )?);
                    }
                }
            }
        }
    }
    if frames.len()
        != usize::try_from(header.record_count)
            .map_err(|_| SessionTelemetryError::CorruptSegment)?
        || frames.first().map(frame_sequence).transpose()? != Some(header.first_sequence)
        || frames.last().map(frame_sequence).transpose()? != Some(header.last_sequence)
    {
        return Err(SessionTelemetryError::CorruptSegment);
    }
    Ok(frames)
}

fn string_attribute(key: &str, value: &str) -> KeyValue {
    KeyValue {
        key: key.to_owned(),
        value: Some(string_value(value)),
        key_strindex: 0,
    }
}

fn string_value(value: &str) -> AnyValue {
    AnyValue {
        value: Some(any_value::Value::StringValue(value.to_owned())),
    }
}

fn prefix(session: SessionId) -> String {
    format!("sessions/{session}/segments/")
}

fn session_prefix(session: SessionId) -> String {
    format!("sessions/{session}/")
}

fn object_key(session: SessionId, id: &str) -> Result<String, SessionTelemetryError> {
    parse_segment_id(id)?;
    Ok(format!("{}{id}.otlp.pb.zst", prefix(session)))
}

fn parse_segment_id(id: &str) -> Result<(u64, u64, &str), SessionTelemetryError> {
    if id.len() != 106 {
        return Err(SessionTelemetryError::InvalidSegmentId);
    }
    let mut parts = id.split('-');
    let first = parts
        .next()
        .ok_or(SessionTelemetryError::InvalidSegmentId)?;
    let last = parts
        .next()
        .ok_or(SessionTelemetryError::InvalidSegmentId)?;
    let sha256 = parts
        .next()
        .ok_or(SessionTelemetryError::InvalidSegmentId)?;
    if parts.next().is_some()
        || first.len() != 20
        || last.len() != 20
        || !first.bytes().all(|byte| byte.is_ascii_digit())
        || !last.bytes().all(|byte| byte.is_ascii_digit())
        || sha256.len() != 64
        || !sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(SessionTelemetryError::InvalidSegmentId);
    }
    let first_sequence = first
        .parse()
        .map_err(|_| SessionTelemetryError::InvalidSegmentId)?;
    let last_sequence = last
        .parse()
        .map_err(|_| SessionTelemetryError::InvalidSegmentId)?;
    if last_sequence < first_sequence {
        return Err(SessionTelemetryError::InvalidSegmentId);
    }
    Ok((first_sequence, last_sequence, sha256))
}

/// Write-only capability composed into Brain.
#[derive(Debug, Clone)]
pub struct SessionTelemetryWriter {
    client: aws_sdk_s3::Client,
    bucket: String,
    kms_key_id: String,
}

impl SessionTelemetryWriter {
    /// Binds the existing session-telemetry bucket and its exact KMS key.
    #[must_use]
    pub fn new(
        client: aws_sdk_s3::Client,
        bucket: impl Into<String>,
        kms_key_id: impl Into<String>,
    ) -> Self {
        Self {
            client,
            bucket: bucket.into(),
            kms_key_id: kms_key_id.into(),
        }
    }

    /// Conditionally writes one bounded deterministic OTLP batch. Identical
    /// retries converge on the same content-addressed object.
    ///
    /// # Errors
    ///
    /// Returns a closed contract, encoding, compression, or store error.
    pub async fn write_frames(
        &self,
        session: SessionId,
        frames: &[TelemetryFrame],
    ) -> Result<EncodedSegment, SessionTelemetryError> {
        let segment = encode_frames(session, frames)?;
        let key = object_key(session, &segment.id)?;
        let checksum =
            base64::engine::general_purpose::STANDARD.encode(Sha256::digest(&segment.bytes));
        let result = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .if_none_match("*")
            .content_type(TELEMETRY_SEGMENT_MEDIA_TYPE)
            .checksum_algorithm(ChecksumAlgorithm::Sha256)
            .checksum_sha256(checksum)
            .server_side_encryption(ServerSideEncryption::AwsKms)
            .ssekms_key_id(&self.kms_key_id)
            .body(ByteStream::from(segment.bytes.clone()))
            .send()
            .await;
        match result {
            Ok(_) => Ok(segment),
            Err(error)
                if error
                    .as_service_error()
                    .is_some_and(|service| service.code() == Some("PreconditionFailed")) =>
            {
                Ok(segment)
            }
            Err(_) => Err(SessionTelemetryError::Store {
                operation: "put_segment",
            }),
        }
    }
}

/// Read/presign-only capability composed into the authenticated session API.
#[derive(Debug, Clone)]
pub struct SessionTelemetryReader {
    client: aws_sdk_s3::Client,
    bucket: String,
    kms_key_id: Option<String>,
}

impl SessionTelemetryReader {
    /// Binds the existing session-telemetry bucket.
    #[must_use]
    pub fn new(client: aws_sdk_s3::Client, bucket: impl Into<String>) -> Self {
        Self {
            client,
            bucket: bucket.into(),
            kms_key_id: None,
        }
    }

    /// Binds read, replay, export-write, and presign authority for session-api.
    #[must_use]
    pub fn with_exports(
        client: aws_sdk_s3::Client,
        bucket: impl Into<String>,
        kms_key_id: impl Into<String>,
    ) -> Self {
        Self {
            client,
            bucket: bucket.into(),
            kms_key_id: Some(kms_key_id.into()),
        }
    }

    /// Whether a non-empty bucket binding was supplied.
    #[must_use]
    pub fn is_bound(&self) -> bool {
        !self.bucket.is_empty()
    }

    /// Lists one bounded page beneath the exact session prefix.
    ///
    /// # Errors
    ///
    /// Rejects invalid limits, malformed owned keys, and S3 failures.
    pub async fn list(
        &self,
        session: SessionId,
        continuation: Option<String>,
        limit: i32,
    ) -> Result<SegmentPage, SessionTelemetryError> {
        if !(1..=MAX_PAGE_ITEMS).contains(&limit) {
            return Err(SessionTelemetryError::InvalidPageSize);
        }
        let owned_prefix = prefix(session);
        let output = self
            .client
            .list_objects_v2()
            .bucket(&self.bucket)
            .prefix(&owned_prefix)
            .set_continuation_token(continuation)
            .max_keys(limit)
            .send()
            .await
            .map_err(|_| SessionTelemetryError::Store {
                operation: "list_segments",
            })?;
        let mut segments = Vec::with_capacity(output.contents().len());
        for object in output.contents() {
            let key = object
                .key()
                .ok_or(SessionTelemetryError::InvalidStoredKey)?;
            let id = key
                .strip_prefix(&owned_prefix)
                .and_then(|tail| tail.strip_suffix(".otlp.pb.zst"))
                .ok_or(SessionTelemetryError::InvalidStoredKey)?;
            let (sequence, last_sequence, sha256) =
                parse_segment_id(id).map_err(|_| SessionTelemetryError::InvalidStoredKey)?;
            let size_bytes = u64::try_from(object.size().unwrap_or_default())
                .map_err(|_| SessionTelemetryError::InvalidStoredKey)?;
            if usize::try_from(size_bytes).map_or(true, |size| size > MAX_SEGMENT_BYTES) {
                return Err(SessionTelemetryError::InvalidStoredKey);
            }
            segments.push(SegmentDescriptor {
                id: id.to_owned(),
                sequence,
                last_sequence,
                sha256: sha256.to_owned(),
                size_bytes,
                created_at_ms: object
                    .last_modified()
                    .and_then(|value| value.to_millis().ok()),
            });
        }
        Ok(SegmentPage {
            segments,
            next: output.next_continuation_token().map(str::to_owned),
        })
    }

    /// Reads immutable metadata for one validated segment.
    ///
    /// # Errors
    ///
    /// Returns [`SessionTelemetryError::InvalidSegmentId`] before S3 for an
    /// invalid id, and a classified store error for a missing/refused object.
    pub async fn describe(
        &self,
        session: SessionId,
        segment_id: &str,
    ) -> Result<SegmentDescriptor, SessionTelemetryError> {
        let (sequence, last_sequence, sha256) = parse_segment_id(segment_id)?;
        let output =
            self.client
                .head_object()
                .bucket(&self.bucket)
                .key(object_key(session, segment_id)?)
                .send()
                .await
                .map_err(|error| {
                    if error.as_service_error().is_some_and(|service| {
                        matches!(service.code(), Some("NotFound" | "NoSuchKey"))
                    }) {
                        SessionTelemetryError::NotFound
                    } else {
                        SessionTelemetryError::Store {
                            operation: "describe_segment",
                        }
                    }
                })?;
        let size_bytes = u64::try_from(output.content_length().unwrap_or_default())
            .map_err(|_| SessionTelemetryError::InvalidStoredKey)?;
        if usize::try_from(size_bytes).map_or(true, |size| size > MAX_SEGMENT_BYTES) {
            return Err(SessionTelemetryError::InvalidStoredKey);
        }
        Ok(SegmentDescriptor {
            id: segment_id.to_owned(),
            sequence,
            last_sequence,
            sha256: sha256.to_owned(),
            size_bytes,
            created_at_ms: output
                .last_modified()
                .and_then(|value| value.to_millis().ok()),
        })
    }

    /// Mints a five-minute GET grant for one validated segment id.
    ///
    /// # Errors
    ///
    /// Rejects malformed ids and signing failures. The caller must first list
    /// the segment, which proves it exists and belongs to the session.
    pub async fn presign(
        &self,
        session: SessionId,
        segment_id: &str,
    ) -> Result<String, SessionTelemetryError> {
        let key = object_key(session, segment_id)?;
        let config = PresigningConfig::expires_in(DOWNLOAD_GRANT_TTL).map_err(|_| {
            SessionTelemetryError::Store {
                operation: "configure_download_grant",
            }
        })?;
        self.client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .response_content_type(TELEMETRY_SEGMENT_MEDIA_TYPE)
            .presigned(config)
            .await
            .map(|request| request.uri().to_string())
            .map_err(|_| SessionTelemetryError::Store {
                operation: "presign_segment",
            })
    }

    /// Reads, hashes, decompresses, validates, and decodes one immutable segment.
    ///
    /// # Errors
    ///
    /// Rejects malformed ids, missing/oversized objects, hash mismatches,
    /// malformed headers, invalid zstd, and non-OTLP payloads.
    pub async fn read_frames(
        &self,
        session: SessionId,
        segment_id: &str,
    ) -> Result<Vec<TelemetryFrame>, SessionTelemetryError> {
        let (_, _, expected_sha256) = parse_segment_id(segment_id)?;
        let output =
            self.client
                .get_object()
                .bucket(&self.bucket)
                .key(object_key(session, segment_id)?)
                .send()
                .await
                .map_err(|error| {
                    if error.as_service_error().is_some_and(|service| {
                        matches!(service.code(), Some("NotFound" | "NoSuchKey"))
                    }) {
                        SessionTelemetryError::NotFound
                    } else {
                        SessionTelemetryError::Store {
                            operation: "read_segment",
                        }
                    }
                })?;
        let bytes = output
            .body
            .collect()
            .await
            .map_err(|_| SessionTelemetryError::Store {
                operation: "read_segment_body",
            })?
            .into_bytes();
        if bytes.len() > MAX_SEGMENT_BYTES || hex::encode(Sha256::digest(&bytes)) != expected_sha256
        {
            return Err(SessionTelemetryError::CorruptSegment);
        }
        decode_frames(session, &bytes)
    }

    /// Builds one deterministic zstd-compressed NDJSON export for a bounded
    /// half-open sequence range and stores it immutably.
    ///
    /// # Errors
    ///
    /// Returns `NotFound` when the selected range has no retained frame and
    /// fails closed when export KMS authority was not composed.
    pub async fn export_range(
        &self,
        session: SessionId,
        from: u128,
        to: Option<u128>,
    ) -> Result<ExportDescriptor, SessionTelemetryError> {
        let kms_key_id = self
            .kms_key_id
            .as_deref()
            .ok_or(SessionTelemetryError::Store {
                operation: "export_authority_unbound",
            })?;
        let mut continuation = None;
        let mut frames = Vec::new();
        loop {
            let page = self.list(session, continuation, MAX_PAGE_ITEMS).await?;
            for descriptor in page.segments {
                if u128::from(descriptor.last_sequence) < from
                    || to.is_some_and(|ceiling| u128::from(descriptor.sequence) >= ceiling)
                {
                    continue;
                }
                frames.extend(
                    self.read_frames(session, &descriptor.id)
                        .await?
                        .into_iter()
                        .filter(|frame| {
                            let sequence = frame.sequence.get();
                            sequence >= from && to.is_none_or(|ceiling| sequence < ceiling)
                        }),
                );
            }
            continuation = page.next;
            if continuation.is_none() {
                break;
            }
        }
        frames.sort_by_key(|frame| frame.sequence.get());
        frames.dedup_by_key(|frame| frame.sequence.get());
        if frames.is_empty() {
            return Err(SessionTelemetryError::NotFound);
        }
        let mut ndjson = Vec::new();
        for frame in &frames {
            serde_json::to_writer(&mut ndjson, frame)
                .map_err(|_| SessionTelemetryError::CorruptSegment)?;
            ndjson.push(b'\n');
        }
        if ndjson.len() > MAX_DECOMPRESSED_BYTES {
            return Err(SessionTelemetryError::SegmentTooLarge);
        }
        let compressed = zstd::stream::encode_all(ndjson.as_slice(), 3)
            .map_err(|_| SessionTelemetryError::CorruptSegment)?;
        if compressed.len() > MAX_SEGMENT_BYTES {
            return Err(SessionTelemetryError::SegmentTooLarge);
        }
        let sha256 = hex::encode(Sha256::digest(&compressed));
        let (Some(first), Some(last)) = (frames.first(), frames.last()) else {
            return Err(SessionTelemetryError::NotFound);
        };
        let first = first.sequence.get();
        let last = last.sequence.get();
        let key = format!("sessions/{session}/exports/{first:020}-{last:020}-{sha256}.ndjson.zst");
        let checksum =
            base64::engine::general_purpose::STANDARD.encode(Sha256::digest(&compressed));
        let result = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(&key)
            .if_none_match("*")
            .content_type(TELEMETRY_EXPORT_MEDIA_TYPE)
            .checksum_algorithm(ChecksumAlgorithm::Sha256)
            .checksum_sha256(checksum)
            .server_side_encryption(ServerSideEncryption::AwsKms)
            .ssekms_key_id(kms_key_id)
            .body(ByteStream::from(compressed.clone()))
            .send()
            .await;
        match result {
            Ok(_) => {}
            Err(error)
                if error
                    .as_service_error()
                    .is_some_and(|service| service.code() == Some("PreconditionFailed")) => {}
            Err(_) => {
                return Err(SessionTelemetryError::Store {
                    operation: "put_export",
                });
            }
        }
        Ok(ExportDescriptor {
            sha256,
            size_bytes: u64::try_from(compressed.len())
                .map_err(|_| SessionTelemetryError::SegmentTooLarge)?,
            object_key: key,
        })
    }

    /// Mints a five-minute grant for an export created by [`Self::export_range`].
    ///
    /// # Errors
    ///
    /// Returns a classified signing refusal.
    pub async fn presign_export(
        &self,
        export: &ExportDescriptor,
    ) -> Result<String, SessionTelemetryError> {
        let config = PresigningConfig::expires_in(DOWNLOAD_GRANT_TTL).map_err(|_| {
            SessionTelemetryError::Store {
                operation: "configure_export_grant",
            }
        })?;
        self.client
            .get_object()
            .bucket(&self.bucket)
            .key(&export.object_key)
            .response_content_type(TELEMETRY_EXPORT_MEDIA_TYPE)
            .presigned(config)
            .await
            .map(|request| request.uri().to_string())
            .map_err(|_| SessionTelemetryError::Store {
                operation: "presign_export",
            })
    }
}

/// Delete-only capability composed into session deletion.
#[derive(Debug, Clone)]
pub struct SessionTelemetryDeleter {
    client: aws_sdk_s3::Client,
    bucket: String,
}

impl SessionTelemetryDeleter {
    /// Binds the existing session-telemetry bucket.
    #[must_use]
    pub fn new(client: aws_sdk_s3::Client, bucket: impl Into<String>) -> Self {
        Self {
            client,
            bucket: bucket.into(),
        }
    }

    /// Deletes at most `limit` objects and reports whether work was performed.
    /// A later deletion turn repeats this until it returns `false`.
    ///
    /// # Errors
    ///
    /// Rejects invalid limits, keys outside the exact session-owned prefix, and
    /// any S3 refusal. Deletion intentionally does not apply the reader's
    /// segment/export grammar: an obsolete or partially written owned object
    /// must not strand irreversible session cleanup.
    pub async fn delete_page(
        &self,
        session: SessionId,
        limit: i32,
    ) -> Result<bool, SessionTelemetryError> {
        if !(1..=MAX_PAGE_ITEMS).contains(&limit) {
            return Err(SessionTelemetryError::InvalidPageSize);
        }
        let owned_prefix = session_prefix(session);
        let output = self
            .client
            .list_objects_v2()
            .bucket(&self.bucket)
            .prefix(&owned_prefix)
            .max_keys(limit)
            .send()
            .await
            .map_err(|_| SessionTelemetryError::Store {
                operation: "list_for_delete",
            })?;
        if output.contents().is_empty() {
            return Ok(false);
        }
        for object in output.contents() {
            let key = object
                .key()
                .ok_or(SessionTelemetryError::InvalidStoredKey)?;
            deletion_tail(&owned_prefix, key)?;
            self.client
                .delete_object()
                .bucket(&self.bucket)
                .key(key)
                .send()
                .await
                .map_err(|_| SessionTelemetryError::Store {
                    operation: "delete_session_object",
                })?;
        }
        Ok(true)
    }
}

fn deletion_tail<'a>(owned_prefix: &str, key: &'a str) -> Result<&'a str, SessionTelemetryError> {
    key.strip_prefix(owned_prefix)
        .ok_or(SessionTelemetryError::InvalidStoredKey)
}

#[cfg(test)]
mod deletion_tests {
    use super::deletion_tail;

    #[test]
    fn exact_session_cleanup_accepts_unknown_historical_children() {
        let prefix = "sessions/ses_01kyw2qa4ne00r40r40m30e209/";
        assert_eq!(
            deletion_tail(prefix, &format!("{prefix}tool-results/legacy.out"))
                .expect("the exact session prefix owns every child"),
            "tool-results/legacy.out"
        );
        assert!(
            deletion_tail(
                prefix,
                "sessions/ses_01kyw2qa4ne00r40r40m30e20a/segments/foreign.otlp.pb.zst"
            )
            .is_err()
        );
    }
}
