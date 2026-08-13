//! Customer-safe, immutable session telemetry segments.
//!
//! This crate deliberately has no generic telemetry-record or OTLP-input API.
//! Its encoder accepts one closed AEX event vocabulary and creates the official
//! OpenTelemetry protobuf message itself. Prompts, completions, tool payloads,
//! internal identifiers and process tracing therefore cannot cross this type
//! boundary.

use std::time::Duration;

use aex_wire::ids::SessionId;
use aex_wire::types::Timestamp;
use aws_sdk_s3::error::ProvideErrorMetadata as _;
use aws_sdk_s3::presigning::PresigningConfig;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{ChecksumAlgorithm, ServerSideEncryption};
use base64::Engine as _;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue, any_value};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs, SeverityNumber};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message as _;
use sha2::{Digest as _, Sha256};

/// MIME type of every immutable segment.
pub const OTLP_PROTOBUF_MEDIA_TYPE: &str = "application/x-protobuf";
/// Maximum encoded size. The closed schema currently encodes below 1 KiB; this
/// ceiling prevents a future schema edit from silently turning segments into
/// arbitrary payload storage.
pub const MAX_SEGMENT_BYTES: usize = 16 * 1024;
/// Maximum objects returned by one public list request or deletion pass.
pub const MAX_PAGE_ITEMS: i32 = 100;
/// Lifetime of a customer download grant.
pub const DOWNLOAD_GRANT_TTL: Duration = Duration::from_mins(5);

const EVENT_NAME: &str = "aex.session.message.completed";
const INSTRUMENTATION_SCOPE: &str = "aex.session";
const SERVICE_NAME: &str = "aex";

/// The complete outcome vocabulary admitted into customer telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionOutcome {
    /// The message completed successfully.
    Succeeded,
    /// The message failed.
    Failed,
    /// The message exceeded its time budget.
    TimedOut,
    /// The customer cancelled the message.
    Cancelled,
    /// Platform/session state interrupted the message.
    Interrupted,
}

impl SessionOutcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }
}

/// One closed, customer-safe event accepted by the encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionEvent {
    sequence: u64,
    outcome: SessionOutcome,
    occurred_at: Timestamp,
}

impl SessionEvent {
    /// Creates the one event shape exported by the initial schema.
    #[must_use]
    pub const fn message_completed(
        sequence: u64,
        outcome: SessionOutcome,
        occurred_at: Timestamp,
    ) -> Self {
        Self {
            sequence,
            outcome,
            occurred_at,
        }
    }
}

/// A deterministic official OTLP protobuf segment ready for object storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedSegment {
    /// Stable public segment id: zero-padded sequence plus content hash.
    pub id: String,
    /// SHA-256 of the exact protobuf bytes.
    pub sha256: String,
    /// Encoded `ExportLogsServiceRequest` bytes.
    pub bytes: Vec<u8>,
}

/// One immutable object visible to the finite session API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentDescriptor {
    /// Stable segment id (never a raw object key).
    pub id: String,
    /// Ordered session-event sequence.
    pub sequence: u64,
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

/// Why encoding or immutable S3 storage failed.
#[derive(Debug, thiserror::Error)]
pub enum SessionTelemetryError {
    /// A timestamp cannot be represented by OTLP's unsigned nanoseconds.
    #[error("the session event timestamp is outside the OTLP range")]
    TimestampRange,
    /// The closed schema unexpectedly exceeded its hard byte ceiling.
    #[error("the OTLP segment exceeds the hard size ceiling")]
    SegmentTooLarge,
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

/// Encodes one event into the official OpenTelemetry logs request message.
///
/// # Errors
///
/// Returns [`SessionTelemetryError::TimestampRange`] for negative/overflowing
/// timestamps and [`SessionTelemetryError::SegmentTooLarge`] if the closed
/// schema grows past [`MAX_SEGMENT_BYTES`].
pub fn encode(event: SessionEvent) -> Result<EncodedSegment, SessionTelemetryError> {
    let millis = u64::try_from(event.occurred_at.unix_millis())
        .map_err(|_| SessionTelemetryError::TimestampRange)?;
    let nanos = millis
        .checked_mul(1_000_000)
        .ok_or(SessionTelemetryError::TimestampRange)?;
    let request = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![string_attribute("service.name", SERVICE_NAME)],
                dropped_attributes_count: 0,
                entity_refs: Vec::new(),
            }),
            scope_logs: vec![ScopeLogs {
                scope: Some(InstrumentationScope {
                    name: INSTRUMENTATION_SCOPE.to_owned(),
                    version: env!("CARGO_PKG_VERSION").to_owned(),
                    attributes: Vec::new(),
                    dropped_attributes_count: 0,
                }),
                log_records: vec![LogRecord {
                    time_unix_nano: nanos,
                    observed_time_unix_nano: nanos,
                    severity_number: SeverityNumber::Info as i32,
                    severity_text: "INFO".to_owned(),
                    body: Some(string_value(EVENT_NAME)),
                    attributes: vec![
                        string_attribute("aex.session.event", EVENT_NAME),
                        string_attribute("aex.session.outcome", event.outcome.as_str()),
                        string_attribute("aex.session.sequence", &event.sequence.to_string()),
                    ],
                    dropped_attributes_count: 0,
                    flags: 0,
                    trace_id: Vec::new(),
                    span_id: Vec::new(),
                    event_name: String::new(),
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };
    let bytes = request.encode_to_vec();
    if bytes.len() > MAX_SEGMENT_BYTES {
        return Err(SessionTelemetryError::SegmentTooLarge);
    }
    let sha256 = hex::encode(Sha256::digest(&bytes));
    Ok(EncodedSegment {
        id: format!("{:020}-{sha256}", event.sequence),
        sha256,
        bytes,
    })
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

fn object_key(session: SessionId, id: &str) -> Result<String, SessionTelemetryError> {
    parse_segment_id(id)?;
    Ok(format!("{}{id}.otlp", prefix(session)))
}

fn parse_segment_id(id: &str) -> Result<(u64, &str), SessionTelemetryError> {
    if id.len() != 85 {
        return Err(SessionTelemetryError::InvalidSegmentId);
    }
    let (sequence, sha256) = id
        .split_once('-')
        .ok_or(SessionTelemetryError::InvalidSegmentId)?;
    if sequence.len() != 20
        || !sequence.bytes().all(|byte| byte.is_ascii_digit())
        || sha256.len() != 64
        || !sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(SessionTelemetryError::InvalidSegmentId);
    }
    let sequence = sequence
        .parse()
        .map_err(|_| SessionTelemetryError::InvalidSegmentId)?;
    Ok((sequence, sha256))
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

    /// Conditionally writes the deterministic segment. A duplicate is success.
    ///
    /// # Errors
    ///
    /// Returns an encoding error or a classified object-store refusal.
    pub async fn write(
        &self,
        session: SessionId,
        event: SessionEvent,
    ) -> Result<EncodedSegment, SessionTelemetryError> {
        let segment = encode(event)?;
        let key = object_key(session, &segment.id)?;
        let checksum =
            base64::engine::general_purpose::STANDARD.encode(Sha256::digest(&segment.bytes));
        let result = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .if_none_match("*")
            .content_type(OTLP_PROTOBUF_MEDIA_TYPE)
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
}

impl SessionTelemetryReader {
    /// Binds the existing session-telemetry bucket.
    #[must_use]
    pub fn new(client: aws_sdk_s3::Client, bucket: impl Into<String>) -> Self {
        Self {
            client,
            bucket: bucket.into(),
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
                .and_then(|tail| tail.strip_suffix(".otlp"))
                .ok_or(SessionTelemetryError::InvalidStoredKey)?;
            let (sequence, sha256) =
                parse_segment_id(id).map_err(|_| SessionTelemetryError::InvalidStoredKey)?;
            let size_bytes = u64::try_from(object.size().unwrap_or_default())
                .map_err(|_| SessionTelemetryError::InvalidStoredKey)?;
            if usize::try_from(size_bytes).map_or(true, |size| size > MAX_SEGMENT_BYTES) {
                return Err(SessionTelemetryError::InvalidStoredKey);
            }
            segments.push(SegmentDescriptor {
                id: id.to_owned(),
                sequence,
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
        let (sequence, sha256) = parse_segment_id(segment_id)?;
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
            .response_content_type(OTLP_PROTOBUF_MEDIA_TYPE)
            .presigned(config)
            .await
            .map(|request| request.uri().to_string())
            .map_err(|_| SessionTelemetryError::Store {
                operation: "presign_segment",
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
    /// Rejects invalid limits, malformed owned keys, and any S3 refusal.
    pub async fn delete_page(
        &self,
        session: SessionId,
        limit: i32,
    ) -> Result<bool, SessionTelemetryError> {
        if !(1..=MAX_PAGE_ITEMS).contains(&limit) {
            return Err(SessionTelemetryError::InvalidPageSize);
        }
        let owned_prefix = prefix(session);
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
            let id = key
                .strip_prefix(&owned_prefix)
                .and_then(|tail| tail.strip_suffix(".otlp"))
                .ok_or(SessionTelemetryError::InvalidStoredKey)?;
            parse_segment_id(id).map_err(|_| SessionTelemetryError::InvalidStoredKey)?;
            self.client
                .delete_object()
                .bucket(&self.bucket)
                .key(key)
                .send()
                .await
                .map_err(|_| SessionTelemetryError::Store {
                    operation: "delete_segment",
                })?;
        }
        Ok(true)
    }
}
