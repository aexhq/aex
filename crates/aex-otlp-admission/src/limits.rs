//! The effective OTLP admission limits.
//!
//! Every value is loaded from the workspace's effective limits rather than
//! hard-coded at a call site. [`OtlpLimits::REGISTERED`] is the registry's own
//! row set, and the encoded ceiling is **4 MiB**: the replaced implementation's
//! `MAX_OTLP_BYTES = 6 MiB` contradicted the registry and is not ported.

use aex_observation_domain::limits;

/// The bounds one OTLP request is decoded under.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OtlpLimits {
    /// Maximum encoded request body, in bytes.
    pub encoded_max: usize,
    /// Maximum decoded payload, in bytes.
    pub decoded_max: usize,
    /// Maximum records or data points in one request.
    pub max_records: usize,
    /// Maximum canonical size of one normalized observation, in bytes.
    pub normalized_max: usize,
    /// Maximum attributes on one resource, scope or record.
    pub max_attributes: usize,
    /// Maximum attribute key length, in bytes.
    pub max_attribute_key_bytes: usize,
    /// Maximum attribute value length, in bytes.
    pub max_attribute_value_bytes: usize,
    /// Maximum elements in an attribute array value.
    pub max_array_elements: usize,
    /// Maximum decompression ratio before the stream is abandoned.
    pub max_ratio: u64,
    /// Bytes of canonical text the redactor may scan for one batch.
    pub redact_budget_bytes: u64,
}

impl OtlpLimits {
    /// The registered defaults.
    pub const REGISTERED: Self = Self {
        encoded_max: limits::OTLP_ENCODED_MAX,
        decoded_max: limits::OTLP_DECODED_MAX,
        max_records: limits::OTLP_MAX_RECORDS,
        normalized_max: limits::OBSERVATION_NORMALIZED_MAX,
        max_attributes: limits::OTLP_MAX_ATTRIBUTES,
        max_attribute_key_bytes: limits::OTLP_MAX_ATTRIBUTE_KEY_BYTES,
        max_attribute_value_bytes: limits::OTLP_MAX_ATTRIBUTE_VALUE_BYTES,
        max_array_elements: limits::OTLP_MAX_ARRAY_ELEMENTS,
        max_ratio: limits::OTLP_MAX_RATIO,
        redact_budget_bytes: 64 * 1024 * 1024,
    };
}

impl Default for OtlpLimits {
    fn default() -> Self {
        Self::REGISTERED
    }
}

#[cfg(test)]
mod tests {
    use super::OtlpLimits;

    #[test]
    fn the_registered_encoded_ceiling_is_four_mebibytes_not_six() {
        assert_eq!(OtlpLimits::REGISTERED.encoded_max, 4 * 1024 * 1024);
        assert_ne!(OtlpLimits::REGISTERED.encoded_max, 6 * 1024 * 1024);
        assert_eq!(OtlpLimits::default(), OtlpLimits::REGISTERED);
    }
}
