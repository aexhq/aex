//! Static envelope and dynamic authenticated body limits.

/// The provider envelope and effective JSON body bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyLimits {
    envelope: usize,
    json: usize,
}

impl BodyLimits {
    /// Requires two non-zero limits with the dynamic limit inside the envelope.
    ///
    /// # Errors
    ///
    /// Returns [`LimitError::InvalidConfiguration`] for zero or reversed bounds.
    pub const fn new(envelope: usize, json: usize) -> Result<Self, LimitError> {
        if envelope == 0 || json == 0 || json > envelope {
            return Err(LimitError::InvalidConfiguration);
        }
        Ok(Self { envelope, json })
    }

    /// Stage one, before authentication.
    ///
    /// # Errors
    ///
    /// Returns [`LimitError::EnvelopeTooLarge`] above the configured bound.
    pub const fn check_envelope(self, measured: usize) -> Result<(), LimitError> {
        if measured > self.envelope {
            return Err(LimitError::EnvelopeTooLarge {
                measured,
                maximum: self.envelope,
            });
        }
        Ok(())
    }

    /// Stage two, after workspace limit resolution.
    ///
    /// # Errors
    ///
    /// Returns [`LimitError::JsonBodyTooLarge`] above the effective bound.
    pub const fn check_json(self, measured: usize) -> Result<(), LimitError> {
        if measured > self.json {
            return Err(LimitError::JsonBodyTooLarge {
                measured,
                maximum: self.json,
            });
        }
        Ok(())
    }
}

/// Exact refusal with measured and maximum bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LimitError {
    /// Limits were zero or reversed.
    #[error("invalid body-limit configuration")]
    InvalidConfiguration,
    /// Static provider envelope exceeded.
    #[error("encoded envelope is {measured} bytes; maximum is {maximum}")]
    EnvelopeTooLarge {
        /// Observed bytes.
        measured: usize,
        /// Enforced bound.
        maximum: usize,
    },
    /// Effective workspace JSON bound exceeded.
    #[error("JSON body is {measured} bytes; maximum is {maximum}")]
    JsonBodyTooLarge {
        /// Observed bytes.
        measured: usize,
        /// Enforced bound.
        maximum: usize,
    },
}
