//! The application error.
//!
//! Every arm carries the domain rejection it came from, so a deployable maps one
//! typed value onto one stable public code instead of re-deciding what a
//! rejection meant.

use aex_operation_domain::TransitionError;
use aex_session_domain::{
    ApprovalRejection, DeletionRejection, PauseRejection, RunError, SessionError, TerminalRejection,
};
use aex_wire::error::ErrorCode;

use crate::plan::PlanError;
use crate::ports::{CommitError, PortError};

/// Why a use case did not produce a plan.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AppError {
    /// A port read failed.
    #[error(transparent)]
    Port(#[from] PortError),
    /// The account is paused.
    #[error(transparent)]
    Paused(#[from] PauseRejection),
    /// The session head refused the command.
    #[error(transparent)]
    Session(#[from] SessionError),
    /// The run refused the command.
    #[error(transparent)]
    Run(#[from] RunError),
    /// The terminal barrier refused the attempt.
    #[error(transparent)]
    Terminal(#[from] TerminalRejection),
    /// The approval refused the decision.
    #[error(transparent)]
    Approval(#[from] ApprovalRejection),
    /// The deletion guard refused the command.
    #[error(transparent)]
    Deletion(#[from] DeletionRejection),
    /// An operation transition was illegal.
    #[error(transparent)]
    Transition(#[from] TransitionError),
    /// The same identity was reused for a different intent.
    #[error("idempotency conflict")]
    Conflict(ErrorCode),
    /// The plan the use case built is not submittable. Always an internal fault.
    #[error(transparent)]
    Plan(#[from] PlanError),
    /// A commit failed. Present so a caller can carry one error type end to end.
    #[error(transparent)]
    Commit(#[from] CommitError),
}

impl AppError {
    /// The stable public code.
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::Port(PortError::NotFound { .. }) => ErrorCode::NotFound,
            Self::Port(_) | Self::Plan(_) | Self::Commit(_) | Self::Transition(_) => {
                ErrorCode::InternalError
            }
            Self::Paused(rejection) => rejection.code,
            Self::Session(SessionError::NotIdle { .. }) => ErrorCode::SessionNotIdle,
            Self::Session(SessionError::Deleted(_))
            | Self::Deletion(DeletionRejection::Purged(_)) => ErrorCode::SessionDeleted,
            Self::Approval(ApprovalRejection::NotFound) => ErrorCode::ApprovalNotFound,
            Self::Approval(ApprovalRejection::AlreadyResolved(_)) => {
                ErrorCode::ApprovalAlreadyResolved
            }
            Self::Deletion(DeletionRejection::PurgeInProgress(_)) => ErrorCode::DeletionInProgress,
            Self::Session(_)
            | Self::Run(_)
            | Self::Terminal(_)
            | Self::Approval(_)
            | Self::Deletion(_) => ErrorCode::PreconditionFailed,
            Self::Conflict(code) => *code,
        }
    }

    /// Whether the caller may retry the same request unchanged.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Port(PortError::Throttled { .. } | PortError::Unavailable { .. })
                | Self::Commit(CommitError::Throttled | CommitError::Unavailable)
        )
    }
}

#[cfg(test)]
mod tests {
    use aex_session_domain::{PauseReason, PauseRejection};
    use aex_wire::error::ErrorCode;

    use super::AppError;
    use crate::ports::{CommitError, PortError};

    #[test]
    fn every_arm_maps_onto_a_stable_code() {
        assert_eq!(
            AppError::Port(PortError::NotFound { kind: "session" }).code(),
            ErrorCode::NotFound
        );
        assert_eq!(
            AppError::Paused(PauseRejection {
                reason: PauseReason::TopUpRequired,
                code: ErrorCode::AccountPaused,
            })
            .code(),
            ErrorCode::AccountPaused
        );
        assert_eq!(
            AppError::Commit(CommitError::Unavailable).code(),
            ErrorCode::InternalError
        );
    }

    #[test]
    fn only_transient_failures_are_retryable() {
        assert!(AppError::Commit(CommitError::Throttled).retryable());
        assert!(AppError::Port(PortError::Unavailable { kind: "session" }).retryable());
        assert!(!AppError::Port(PortError::NotFound { kind: "session" }).retryable());
        assert!(!AppError::Conflict(ErrorCode::IdempotencyConflict).retryable());
    }
}
