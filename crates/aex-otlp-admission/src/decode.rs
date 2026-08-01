//! Bounded OTLP decoding.
//!
//! Every bound is checked before the allocation it guards, not after it:
//! encoded size before the body is read, the decompression ratio while the
//! stream is still being consumed, the record count before normalization. A
//! decompression bomb is therefore refused while it is still small.

use std::io::Read as _;

use prost::Message as _;

use crate::error::{OtlpError, OtlpSignal};
use crate::limits::OtlpLimits;
use crate::memory::MemoryLease;
use crate::proto::{collector_logs, collector_metrics, collector_trace};

/// How the body is serialized.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum OtlpEncoding {
    /// `application/x-protobuf`.
    Protobuf,
    /// `application/json`.
    Json,
}

impl OtlpEncoding {
    /// Resolves a `Content-Type` header value.
    ///
    /// # Errors
    ///
    /// Returns [`OtlpError::UnsupportedContentType`] for anything else; the
    /// endpoint accepts exactly two media types.
    pub fn from_content_type(value: &str) -> Result<Self, OtlpError> {
        let media = value
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        match media.as_str() {
            "application/x-protobuf" | "application/protobuf" => Ok(Self::Protobuf),
            "application/json" => Ok(Self::Json),
            _ => Err(OtlpError::UnsupportedContentType {
                content_type: value.into(),
            }),
        }
    }

    /// The name used in a malformed-payload diagnostic.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Protobuf => "protobuf",
            Self::Json => "json",
        }
    }
}

/// How the body is compressed.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ContentCoding {
    /// No compression.
    #[default]
    Identity,
    /// `gzip`.
    Gzip,
}

impl ContentCoding {
    /// Resolves a `Content-Encoding` header value.
    ///
    /// # Errors
    ///
    /// Returns [`OtlpError::UnsupportedCoding`] for anything other than
    /// `identity` and `gzip`.
    pub fn from_content_encoding(value: Option<&str>) -> Result<Self, OtlpError> {
        match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            None | Some("" | "identity") => Ok(Self::Identity),
            Some("gzip") => Ok(Self::Gzip),
            Some(other) => Err(OtlpError::UnsupportedCoding {
                coding: other.into(),
            }),
        }
    }

    /// The worst-case expansion this coding is reserved against.
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "the ratio guard is a small configured integer"
    )]
    pub const fn expansion_estimate(self, max_ratio: u64) -> usize {
        match self {
            Self::Identity => 1,
            Self::Gzip => max_ratio as usize,
        }
    }
}

/// One decode request.
#[derive(Debug)]
pub struct DecodeRequest<'a> {
    /// Which signal the route serves.
    pub signal: OtlpSignal,
    /// How the body is serialized.
    pub encoding: OtlpEncoding,
    /// How the body is compressed.
    pub coding: ContentCoding,
    /// The exact encoded bytes.
    pub body: &'a [u8],
    /// The effective limits.
    pub limits: &'a OtlpLimits,
    /// Proof that the worst-case decoded footprint is reserved.
    pub lease: &'a MemoryLease,
}

/// A decoded, still-untrusted OTLP request.
#[derive(Clone, Debug, PartialEq)]
pub enum DecodedBatch {
    /// `v1/logs`.
    Logs(Box<collector_logs::ExportLogsServiceRequest>),
    /// `v1/traces`.
    Traces(Box<collector_trace::ExportTraceServiceRequest>),
    /// `v1/metrics`.
    Metrics(Box<collector_metrics::ExportMetricsServiceRequest>),
}

impl DecodedBatch {
    /// Which signal this batch carries.
    #[must_use]
    pub const fn signal(&self) -> OtlpSignal {
        match self {
            Self::Logs(_) => OtlpSignal::Logs,
            Self::Traces(_) => OtlpSignal::Traces,
            Self::Metrics(_) => OtlpSignal::Metrics,
        }
    }

    /// How many records or data points the batch carries.
    #[must_use]
    pub fn record_count(&self) -> usize {
        match self {
            Self::Logs(request) => request
                .resource_logs
                .iter()
                .flat_map(|resource| resource.scope_logs.iter())
                .map(|scope| scope.log_records.len())
                .sum(),
            Self::Traces(request) => request
                .resource_spans
                .iter()
                .flat_map(|resource| resource.scope_spans.iter())
                .map(|scope| scope.spans.len())
                .sum(),
            Self::Metrics(request) => request
                .resource_metrics
                .iter()
                .flat_map(|resource| resource.scope_metrics.iter())
                .flat_map(|scope| scope.metrics.iter())
                .map(crate::normalize::metric_point_count)
                .sum(),
        }
    }
}

/// Rejects an encoded body before anything large is allocated.
///
/// # Errors
///
/// Returns [`OtlpError::EncodedTooLarge`] above the effective ceiling.
pub const fn check_encoded_size(len: usize, limits: &OtlpLimits) -> Result<(), OtlpError> {
    if len > limits.encoded_max {
        return Err(OtlpError::EncodedTooLarge {
            observed: len,
            limit: limits.encoded_max,
        });
    }
    Ok(())
}

/// Decodes one OTLP request under its bounds.
///
/// # Errors
///
/// Returns the exact typed bound failure: [`OtlpError::EncodedTooLarge`],
/// [`OtlpError::DecodedTooLarge`], [`OtlpError::RatioExceeded`],
/// [`OtlpError::TooManyRecords`], [`OtlpError::Malformed`] or
/// [`OtlpError::UnknownField`].
pub fn decode(request: &DecodeRequest<'_>) -> Result<DecodedBatch, OtlpError> {
    check_encoded_size(request.body.len(), request.limits)?;
    if !request.lease.covers(1) {
        return Err(OtlpError::MemoryUnavailable {
            requested: request.limits.decoded_max,
        });
    }

    let plain = match request.coding {
        ContentCoding::Identity => {
            if request.body.len() > request.limits.decoded_max {
                return Err(OtlpError::DecodedTooLarge {
                    limit: request.limits.decoded_max,
                });
            }
            std::borrow::Cow::Borrowed(request.body)
        }
        ContentCoding::Gzip => std::borrow::Cow::Owned(inflate(request.body, request.limits)?),
    };

    let batch = match request.encoding {
        OtlpEncoding::Protobuf => decode_protobuf(request.signal, &plain)?,
        OtlpEncoding::Json => crate::json::decode_json(request.signal, &plain)?,
    };

    let records = batch.record_count();
    if records > request.limits.max_records {
        return Err(OtlpError::TooManyRecords {
            observed: records,
            limit: request.limits.max_records,
        });
    }
    Ok(batch)
}

fn decode_protobuf(signal: OtlpSignal, bytes: &[u8]) -> Result<DecodedBatch, OtlpError> {
    let malformed = |error: prost::DecodeError| OtlpError::Malformed {
        encoding: "protobuf",
        reason: error.to_string().into_boxed_str(),
    };
    Ok(match signal {
        OtlpSignal::Logs => DecodedBatch::Logs(Box::new(
            collector_logs::ExportLogsServiceRequest::decode(bytes).map_err(malformed)?,
        )),
        OtlpSignal::Traces => DecodedBatch::Traces(Box::new(
            collector_trace::ExportTraceServiceRequest::decode(bytes).map_err(malformed)?,
        )),
        OtlpSignal::Metrics => DecodedBatch::Metrics(Box::new(
            collector_metrics::ExportMetricsServiceRequest::decode(bytes).map_err(malformed)?,
        )),
    })
}

/// Inflates a gzip body under both the decoded ceiling and the ratio guard.
///
/// The guard is evaluated **per chunk while inflating**, so a small body that
/// would expand past the ceiling is abandoned long before the allocation
/// completes.
fn inflate(body: &[u8], limits: &OtlpLimits) -> Result<Vec<u8>, OtlpError> {
    const CHUNK: usize = 64 * 1024;

    let consumed = body.len().max(1) as u64;
    let mut decoder = flate2::read::GzDecoder::new(body);
    let mut out: Vec<u8> = Vec::with_capacity(CHUNK.min(limits.decoded_max));
    let mut buffer = vec![0u8; CHUNK];
    loop {
        let read = decoder
            .read(&mut buffer)
            .map_err(|error| OtlpError::Malformed {
                encoding: "gzip",
                reason: error.to_string().into_boxed_str(),
            })?;
        if read == 0 {
            break;
        }
        if out.len() + read > limits.decoded_max {
            return Err(OtlpError::DecodedTooLarge {
                limit: limits.decoded_max,
            });
        }
        out.extend_from_slice(&buffer[..read]);
        let ratio = out.len() as u64 / consumed;
        if ratio > limits.max_ratio {
            return Err(OtlpError::RatioExceeded {
                observed: ratio,
                limit: limits.max_ratio,
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{ContentCoding, DecodeRequest, DecodedBatch, OtlpEncoding, decode};
    use crate::error::{OtlpError, OtlpSignal};
    use crate::limits::OtlpLimits;
    use crate::memory::MemoryBudget;
    use prost::Message as _;

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        use std::io::Write as _;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(bytes).expect("in-memory write");
        encoder.finish().expect("in-memory finish")
    }

    #[test]
    fn a_content_type_resolves_to_exactly_two_encodings() {
        assert_eq!(
            OtlpEncoding::from_content_type("application/x-protobuf").expect("protobuf"),
            OtlpEncoding::Protobuf
        );
        assert_eq!(
            OtlpEncoding::from_content_type("application/json; charset=utf-8").expect("json"),
            OtlpEncoding::Json
        );
        assert!(matches!(
            OtlpEncoding::from_content_type("text/plain"),
            Err(OtlpError::UnsupportedContentType { .. })
        ));
        assert_eq!(OtlpEncoding::Json.as_str(), "json");
    }

    #[test]
    fn only_identity_and_gzip_are_accepted_codings() {
        assert_eq!(
            ContentCoding::from_content_encoding(None).expect("identity"),
            ContentCoding::Identity
        );
        assert_eq!(
            ContentCoding::from_content_encoding(Some("gzip")).expect("gzip"),
            ContentCoding::Gzip
        );
        assert!(matches!(
            ContentCoding::from_content_encoding(Some("br")),
            Err(OtlpError::UnsupportedCoding { .. })
        ));
        assert_eq!(ContentCoding::Identity.expansion_estimate(200), 1);
        assert_eq!(ContentCoding::Gzip.expansion_estimate(200), 200);
    }

    #[test]
    fn an_encoded_body_above_the_ceiling_is_refused_before_decoding() {
        let limits = OtlpLimits {
            encoded_max: 8,
            ..OtlpLimits::REGISTERED
        };
        let budget = MemoryBudget::new(1_024);
        let lease = budget.try_reserve(64).expect("reserves");
        let error = decode(&DecodeRequest {
            signal: OtlpSignal::Logs,
            encoding: OtlpEncoding::Protobuf,
            coding: ContentCoding::Identity,
            body: &[0u8; 9],
            limits: &limits,
            lease: &lease,
        })
        .expect_err("refused");
        assert_eq!(
            error,
            OtlpError::EncodedTooLarge {
                observed: 9,
                limit: 8
            }
        );
    }

    #[test]
    fn a_gzip_bomb_is_refused_while_it_is_still_small() {
        let bomb = gzip(&vec![0u8; 4 * 1024 * 1024]);
        assert!(
            bomb.len() < 64 * 1024,
            "the fixture must actually be a bomb"
        );
        let limits = OtlpLimits {
            decoded_max: 256 * 1024,
            ..OtlpLimits::REGISTERED
        };
        let budget = MemoryBudget::new(4 * 1024 * 1024);
        let lease = budget.try_reserve(limits.decoded_max).expect("reserves");
        let error = decode(&DecodeRequest {
            signal: OtlpSignal::Logs,
            encoding: OtlpEncoding::Protobuf,
            coding: ContentCoding::Gzip,
            body: &bomb,
            limits: &limits,
            lease: &lease,
        })
        .expect_err("refused");
        assert!(
            matches!(
                error,
                OtlpError::DecodedTooLarge { .. } | OtlpError::RatioExceeded { .. }
            ),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn the_ratio_guard_fires_on_a_high_ratio_stream() {
        let bomb = gzip(&vec![b'a'; 1024 * 1024]);
        let limits = OtlpLimits {
            decoded_max: 16 * 1024 * 1024,
            max_ratio: 2,
            ..OtlpLimits::REGISTERED
        };
        let budget = MemoryBudget::new(16 * 1024 * 1024);
        let lease = budget.try_reserve(1_024).expect("reserves");
        let error = decode(&DecodeRequest {
            signal: OtlpSignal::Logs,
            encoding: OtlpEncoding::Protobuf,
            coding: ContentCoding::Gzip,
            body: &bomb,
            limits: &limits,
            lease: &lease,
        })
        .expect_err("refused");
        assert!(matches!(error, OtlpError::RatioExceeded { limit: 2, .. }));
    }

    #[test]
    fn an_empty_protobuf_request_decodes_to_an_empty_batch() {
        let request = crate::proto::collector_logs::ExportLogsServiceRequest::default();
        let bytes = request.encode_to_vec();
        let limits = OtlpLimits::REGISTERED;
        let budget = MemoryBudget::new(1_024);
        let lease = budget.try_reserve(64).expect("reserves");
        let batch = decode(&DecodeRequest {
            signal: OtlpSignal::Logs,
            encoding: OtlpEncoding::Protobuf,
            coding: ContentCoding::Identity,
            body: &bytes,
            limits: &limits,
            lease: &lease,
        })
        .expect("decodes");
        assert_eq!(batch.signal(), OtlpSignal::Logs);
        assert_eq!(batch.record_count(), 0);
        assert!(matches!(batch, DecodedBatch::Logs(_)));
    }
}
