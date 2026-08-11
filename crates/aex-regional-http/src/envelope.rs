//! Static provider-envelope and content-type admission.

use aex_wire::routes::BodyClass;
use http::HeaderValue;

/// Provider envelope hard bound, checked before authentication.
pub const ENVELOPE_BYTES: usize = 10 * 1024 * 1024;

/// Why stage-one admission failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EnvelopeError {
    /// Encoded provider body exceeded ten mebibytes.
    #[error("provider envelope has {measured} bytes; maximum is {maximum}")]
    TooLarge {
        /// Observed encoded bytes.
        measured: usize,
        /// Fixed bound.
        maximum: usize,
    },
    /// Content type did not match generated route metadata.
    #[error("content type is not accepted by this route")]
    UnsupportedContentType,
}

/// Applies the static envelope bound.
///
/// # Errors
///
/// Returns [`EnvelopeError::TooLarge`] above [`ENVELOPE_BYTES`].
pub const fn check_size(measured: usize) -> Result<(), EnvelopeError> {
    if measured > ENVELOPE_BYTES {
        return Err(EnvelopeError::TooLarge {
            measured,
            maximum: ENVELOPE_BYTES,
        });
    }
    Ok(())
}

/// Applies the generated body-class content-type rule.
///
/// # Errors
///
/// Returns [`EnvelopeError::UnsupportedContentType`] on any mismatch.
pub fn check_content_type(
    class: BodyClass,
    value: Option<&HeaderValue>,
) -> Result<(), EnvelopeError> {
    match class {
        BodyClass::None => {
            if value.is_some() {
                return Err(EnvelopeError::UnsupportedContentType);
            }
        }
        BodyClass::AexJson => {
            let content_type = value
                .and_then(|value| value.to_str().ok())
                .ok_or(EnvelopeError::UnsupportedContentType)?;
            let mut parts = content_type.split(';').map(str::trim);
            if parts.next() != Some("application/json")
                || parts.any(|parameter| !parameter.eq_ignore_ascii_case("charset=utf-8"))
            {
                return Err(EnvelopeError::UnsupportedContentType);
            }
        }
        BodyClass::Otlp => {
            let content_type = value
                .and_then(|value| value.to_str().ok())
                .ok_or(EnvelopeError::UnsupportedContentType)?;
            if !matches!(content_type, "application/json" | "application/x-protobuf") {
                return Err(EnvelopeError::UnsupportedContentType);
            }
        }
        BodyClass::Binary => {
            if value.and_then(|value| value.to_str().ok()) != Some("application/octet-stream") {
                return Err(EnvelopeError::UnsupportedContentType);
            }
        }
    }
    Ok(())
}
