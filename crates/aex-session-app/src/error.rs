//! The application error.
//!
//! Every arm carries the domain rejection it came from, so a deployable maps one
//! typed value onto one stable public code instead of re-deciding what a
//! rejection meant.

use aex_operation_domain::TransitionError;
use aex_secret_domain::CustodyRejection;
use aex_session_domain::{
    ApprovalRejection, DeletionRejection, PauseRejection, SessionDomainRunError, SessionError,
    TerminalRejection,
};
use aex_wire::canonical::CanonicalError;
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
    Run(#[from] SessionDomainRunError),
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
    /// Credential custody refused an admission or rebind.
    #[error(transparent)]
    Custody(#[from] CustodyRejection),
    /// A typed public operation result could not be canonicalized.
    #[error(transparent)]
    Canonical(#[from] CanonicalError),
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
            Self::Port(_)
            | Self::Plan(_)
            | Self::Commit(_)
            | Self::Transition(_)
            | Self::Canonical(_) => ErrorCode::InternalError,
            Self::Paused(rejection) => rejection.code,
            Self::Session(SessionError::NotIdle { .. })
            | Self::Custody(CustodyRejection::NotTrueIdle(_)) => ErrorCode::SessionNotIdle,
            Self::Session(SessionError::Deleted(_))
            | Self::Deletion(DeletionRejection::Purged(_)) => ErrorCode::SessionDeleted,
            Self::Approval(ApprovalRejection::NotFound) => ErrorCode::ApprovalNotFound,
            Self::Approval(ApprovalRejection::AlreadyResolved(_)) => {
                ErrorCode::ApprovalAlreadyResolved
            }
            Self::Approval(ApprovalRejection::BindingChanged { .. }) => {
                ErrorCode::ApprovalBindingChanged
            }
            Self::Deletion(DeletionRejection::PurgeInProgress(_)) => ErrorCode::DeletionInProgress,
            Self::Session(_)
            | Self::Run(_)
            | Self::Terminal(_)
            | Self::Approval(_)
            | Self::Deletion(_)
            | Self::Custody(_) => ErrorCode::PreconditionFailed,
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
    use aex_session_domain::testing::approval_binding;
    use aex_session_domain::{
        ApprovalCancelCause, ApprovalCommit, ApprovalRejection, ApprovalStatus, PauseReason,
        PauseRejection,
    };
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

    #[test]
    fn approval_binding_drift_uses_the_dedicated_landed_code() {
        let binding = approval_binding();
        let approval = aex_session_domain::Approval {
            id: aex_session_domain::testing::id(1),
            binding,
            status: ApprovalStatus::Cancelled,
            requested_at: aex_session_domain::testing::moment(0),
            expires_at: aex_session_domain::testing::moment(10),
            resolved_at: Some(aex_session_domain::testing::moment(1)),
            decision: None,
            cancel_cause: Some(ApprovalCancelCause::BindingDrift),
        };
        let error = AppError::Approval(ApprovalRejection::BindingChanged {
            fields: vec![aex_session_domain::BindingField::ArgumentDigest],
            commit: Box::new(ApprovalCommit {
                approval,
                dispatch: false,
                denial_result: None,
            }),
        });
        assert_eq!(error.code(), ErrorCode::ApprovalBindingChanged);
    }
}
