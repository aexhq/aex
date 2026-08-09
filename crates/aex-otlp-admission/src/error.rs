//! The typed OTLP admission failures.
//!
//! Every variant names the exact bound it hit and, where a record caused it, the
//! record pointer that did. A batch never partially succeeds: one invalid record
//! rejects the whole batch, so a pointer is diagnostic and never a hint that the
//! rest was kept.

use aex_wire::error::ErrorCode;

/// Which OTLP signal a request carried.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum OtlpSignal {
    /// `v1/logs`.
    Logs,
    /// `v1/traces`.
    Traces,
    /// `v1/metrics`.
    Metrics,
}

impl OtlpSignal {
    /// Every signal, in declared order.
    pub const ALL: &'static [OtlpSignal] =
        &[OtlpSignal::Logs, OtlpSignal::Traces, OtlpSignal::Metrics];

    /// The route path segment.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Logs => "logs",
            Self::Traces => "traces",
            Self::Metrics => "metrics",
        }
    }
}

/// A pointer into the decoded request, for diagnostics only.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RecordPointer {
    /// Index of the resource block.
    pub resource: usize,
    /// Index of the instrumentation scope inside the resource.
    pub scope: usize,
    /// Index of the record inside the scope.
    pub record: usize,
}

impl RecordPointer {
    /// A pointer to one record.
    #[must_use]
    pub const fn new(resource: usize, scope: usize, record: usize) -> Self {
        Self {
            resource,
            scope,
            record,
        }
    }

    /// The JSON-pointer-shaped rendering used in diagnostics.
    #[must_use]
    pub fn to_path(self) -> String {
        format!("/{}/{}/{}", self.resource, self.scope, self.record)
    }
}

/// Why an OTLP request was refused.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum OtlpError {
    /// The encoded body was larger than the effective ceiling.
    #[error("encoded body of {observed} bytes exceeds the {limit}-byte ceiling")]
    EncodedTooLarge {
        /// The measured encoded size.
        observed: usize,
        /// The effective ceiling.
        limit: usize,
    },
    /// The decoded payload was larger than the effective ceiling.
    #[error("decoded payload exceeds the {limit}-byte ceiling")]
    DecodedTooLarge {
        /// The effective ceiling.
        limit: usize,
    },
    /// Decompression expanded faster than the ratio guard allows.
    #[error("decompression ratio {observed} exceeds the {limit}:1 guard")]
    RatioExceeded {
        /// The measured ratio at the moment the guard fired.
        observed: u64,
        /// The effective guard.
        limit: u64,
    },
    /// The `Content-Encoding` is not one this endpoint accepts.
    #[error("unsupported content coding `{coding}`")]
    UnsupportedCoding {
        /// The offered coding.
        coding: Box<str>,
    },
    /// The `Content-Type` is not one this endpoint accepts.
    #[error("unsupported content type `{content_type}`")]
    UnsupportedContentType {
        /// The offered content type.
        content_type: Box<str>,
    },
    /// The body is not decodable as the declared encoding.
    #[error("malformed {encoding} payload: {reason}")]
    Malformed {
        /// Which encoding failed.
        encoding: &'static str,
        /// What was wrong.
        reason: Box<str>,
    },
    /// A field the pinned protocol does not define was present.
    ///
    /// An unknown field is either version drift or an attack; both deserve a
    /// 4xx rather than a silent drop.
    #[error("unknown field `{field}` at {at}")]
    UnknownField {
        /// The offending member name.
        field: Box<str>,
        /// Where it appeared.
        at: Box<str>,
    },
    /// The request carried more records than the effective ceiling.
    #[error("{observed} records exceeds the {limit}-record ceiling")]
    TooManyRecords {
        /// The measured record count.
        observed: usize,
        /// The effective ceiling.
        limit: usize,
    },
    /// One record exceeded a per-record bound.
    #[error("record {at} violates {bound}: {observed} exceeds {limit}")]
    RecordBound {
        /// Which record.
        at: Box<str>,
        /// The bound's name.
        bound: &'static str,
        /// The measured value.
        observed: usize,
        /// The effective ceiling.
        limit: usize,
    },
    /// A record carried a reserved internal attribute.
    #[error("record {at} carries the reserved attribute `{key}`")]
    ReservedAttribute {
        /// Which record.
        at: Box<str>,
        /// The reserved key.
        key: Box<str>,
    },
    /// The authenticated scope hierarchy is inconsistent.
    #[error("scope hierarchy violation: {reason}")]
    ScopeHierarchy {
        /// What was inconsistent.
        reason: &'static str,
    },
    /// No decode-memory permit was available inside the reservation wait.
    #[error("no decode memory available: {requested} bytes requested")]
    MemoryUnavailable {
        /// How many bytes the request needed.
        requested: usize,
    },
}

impl OtlpError {
    /// The public error code this failure maps to.
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::EncodedTooLarge { .. }
            | Self::DecodedTooLarge { .. }
            | Self::RatioExceeded { .. } => ErrorCode::TelemetryPayloadTooLarge,
            Self::UnsupportedCoding { .. } | Self::UnsupportedContentType { .. } => {
                ErrorCode::UnsupportedMediaType
            }
            Self::MemoryUnavailable { .. } => ErrorCode::ObservabilityUnavailable,
            Self::Malformed { .. }
            | Self::UnknownField { .. }
            | Self::TooManyRecords { .. }
            | Self::RecordBound { .. }
            | Self::ReservedAttribute { .. }
            | Self::ScopeHierarchy { .. } => ErrorCode::InvalidTelemetry,
        }
    }

    /// Whether a client may retry the identical request.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        matches!(self, Self::MemoryUnavailable { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::{OtlpError, OtlpSignal, RecordPointer};
    use aex_wire::error::ErrorCode;

    #[test]
    fn a_size_failure_is_a_413_and_a_memory_failure_is_a_retryable_503() {
        let too_large = OtlpError::EncodedTooLarge {
            observed: 1,
            limit: 0,
        };
        assert_eq!(too_large.code(), ErrorCode::TelemetryPayloadTooLarge);
        assert_eq!(too_large.code().http_status(), 413);
        assert!(!too_large.retryable());

        let unavailable = OtlpError::MemoryUnavailable { requested: 8 };
        assert_eq!(unavailable.code().http_status(), 503);
        assert!(unavailable.retryable());
    }

    #[test]
    fn an_unsupported_coding_is_the_registered_415() {
        let error = OtlpError::UnsupportedCoding {
            coding: "br".into(),
        };
        assert_eq!(error.code().http_status(), 415);
        assert_eq!(error.code().as_str(), "unsupported_media_type");
    }

    #[test]
    fn a_record_pointer_renders_its_path() {
        assert_eq!(RecordPointer::new(1, 2, 3).to_path(), "/1/2/3");
        assert_eq!(OtlpSignal::ALL.len(), 3);
        assert_eq!(OtlpSignal::Metrics.as_str(), "metrics");
    }
}
