//! Error codes the observation stream needs that the generated wire does not
//! carry yet.
//!
//! Each row is a `TODO(cross-stream)` to the contracts stream. Keeping them in
//! one typed place means the mapping is exhaustive today and becomes a single
//! delete when the generated vocabulary catches up, rather than a scatter of
//! approximate substitutions across handlers.

/// A public error code the generated `aex_wire::ErrorCode` does not yet define.
// TODO(cross-stream): replaced by aex_wire::error::ErrorCode variants at merge.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PendingErrorCode {
    /// `415 unsupported_media_type` — the `Content-Type` or `Content-Encoding`
    /// is not one the endpoint accepts.
    UnsupportedMediaType,
    /// `409 telemetry_query_budget_exhausted` — a single page could make no
    /// progress at all. Never returned for a page that yielded rows.
    TelemetryQueryBudgetExhausted,
    /// `503 export_capacity` — a memory or worker reservation could not be met.
    ExportCapacity,
}

impl PendingErrorCode {
    /// Every code, in declared order.
    pub const ALL: &'static [PendingErrorCode] = &[
        PendingErrorCode::UnsupportedMediaType,
        PendingErrorCode::TelemetryQueryBudgetExhausted,
        PendingErrorCode::ExportCapacity,
    ];

    /// The wire spelling the contracts stream must register.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedMediaType => "unsupported_media_type",
            Self::TelemetryQueryBudgetExhausted => "telemetry_query_budget_exhausted",
            Self::ExportCapacity => "export_capacity",
        }
    }

    /// The HTTP status the contracts stream must register.
    #[must_use]
    pub const fn status(self) -> u16 {
        match self {
            Self::UnsupportedMediaType => 415,
            Self::TelemetryQueryBudgetExhausted => 409,
            Self::ExportCapacity => 503,
        }
    }

    /// Whether a client may retry the identical request.
    #[must_use]
    pub const fn retryable(self) -> bool {
        matches!(self, Self::ExportCapacity)
    }
}

#[cfg(test)]
mod tests {
    use super::PendingErrorCode;

    #[test]
    fn no_pending_code_collides_with_a_registered_one() {
        for pending in PendingErrorCode::ALL {
            assert!(
                aex_wire::error::ErrorCode::ALL
                    .iter()
                    .all(|code| code.as_str() != pending.as_str()),
                "`{}` is already registered; delete the pending row",
                pending.as_str()
            );
            assert!(pending.status() >= 400);
        }
        assert!(PendingErrorCode::ExportCapacity.retryable());
        assert!(!PendingErrorCode::UnsupportedMediaType.retryable());
    }
}
