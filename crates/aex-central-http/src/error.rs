//! Total edge failure mapping into the closed wire error vocabulary.
//!
//! Two boundaries produce a failure, and they are deliberately different:
//!
//! * **Precedence stages 1–9** run before any handler and are mapped here. Every
//!   route participates in those stages by definition, so the codes below are
//!   not filtered against `RouteDescriptor::errors`.
//! * **Stages 10–13** are the handler's own answers, and `aex_wire::dispatch`
//!   already refuses a code the route does not declare.
//!
//! `details` never carries a database message, a SQL string, a constraint value
//! or an id the caller cannot already see.

use aex_wire::error::{ApiError, ErrorCode, WireError};
use aex_wire::ids::OperationId;
use aex_wire::types::RequestId;

use crate::authorizer::ContextError;

/// Every failure this composition crate produces before a handler runs.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EdgeError {
    /// The method or path resolved to no central route.
    #[error("no central route matches")]
    NoRoute,
    /// The authorizer context was absent, malformed or not current.
    #[error(transparent)]
    Context(#[from] ContextError),
    /// A presented credential was resolved in-process and not admitted.
    ///
    /// One arm for every refusal — unknown, mismatched, revoked, lapsed,
    /// inactive, wrongly pinned, or a MAC that does not verify — so a caller
    /// cannot learn which check failed. Deliberately distinct from
    /// [`Self::AuthenticationUnavailable`], which says the answer is *unknown*
    /// rather than negative and is the only one worth retrying.
    #[error("the credential was refused")]
    CredentialRefused,
    /// The authorization authority could not be reached.
    ///
    /// Deliberately distinct from every other arm: it is the only
    /// authentication failure a caller should retry.
    #[error("the authorization authority is unavailable")]
    AuthenticationUnavailable,
    /// The route does not admit this principal, or the credential names another
    /// organization or workspace.
    #[error("the caller is not authorized for that resource")]
    Forbidden,
    /// The credential lacks the route's scope, or the actor's role is too weak.
    #[error("the credential lacks the required scope")]
    InsufficientScope,
    /// The account is paused and the route is not exempt.
    #[error("the account is paused")]
    AccountPaused,
    /// The account state could not be established.
    ///
    /// An unreadable current state **rejects** new admission. There is no stale
    /// fallback, because the only two alternatives are to serve a paused account
    /// or to refuse a paying one, and both are worse than a retryable `503`.
    #[error("the account state is unavailable")]
    AccountStateUnavailable,
    /// The named resource does not exist, or the caller may not see that it does.
    #[error("no such resource")]
    NotFound,
    /// The resource existed and is gone.
    #[error("the resource was deleted")]
    Gone,
    /// The request body exceeded the composition's bound.
    #[error("the body is {found} bytes, over the {bound} byte bound")]
    PayloadTooLarge {
        /// How many bytes arrived.
        found: usize,
        /// The bound in force.
        bound: usize,
    },
    /// A continuation cursor did not verify, was bound to another query, or
    /// lapsed.
    #[error("the cursor was refused")]
    InvalidCursor,
    /// A header the route declares was absent, malformed, or supplied where the
    /// route declares none.
    #[error("{0}")]
    InvalidRequest(String),
    /// The composition itself is inconsistent. Never caller input.
    #[error("the composition is inconsistent: {0}")]
    Internal(&'static str),
}

impl EdgeError {
    /// The closed wire code this failure renders as.
    ///
    /// Six codes the central plane would name — `workspace_provision_pending`,
    /// `idempotency_in_flight`, `commit_outcome_unknown`, `last_owner_required`,
    /// `resource_conflict` and `invalid_scope` — are not in the generated
    /// registry, so the application layer folds them onto declared codes rather
    /// than inventing spellings here.
    /// TODO(cross-stream): contracts owns adding those six.
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::NoRoute | Self::NotFound => ErrorCode::NotFound,
            Self::Context(_) | Self::CredentialRefused => ErrorCode::Unauthenticated,
            Self::AuthenticationUnavailable => ErrorCode::AuthenticationUnavailable,
            Self::Forbidden => ErrorCode::Forbidden,
            Self::InsufficientScope => ErrorCode::InsufficientScope,
            Self::AccountPaused => ErrorCode::AccountPaused,
            Self::AccountStateUnavailable => ErrorCode::AccountStateUnavailable,
            Self::Gone => ErrorCode::Gone,
            Self::PayloadTooLarge { .. } => ErrorCode::PayloadTooLarge,
            Self::InvalidCursor => ErrorCode::InvalidCursor,
            Self::InvalidRequest(_) => ErrorCode::InvalidRequest,
            Self::Internal(_) => ErrorCode::InternalError,
        }
    }

    /// The failure as a wire error, with a message that leaks nothing.
    ///
    /// An authentication failure renders the code's own default message rather
    /// than the parse reason: the caller does not author the authorizer context,
    /// so naming the field it got wrong tells an attacker about the edge and the
    /// caller nothing useful.
    #[must_use]
    pub fn into_wire(self) -> WireError {
        let code = self.code();
        match self {
            Self::InvalidRequest(reason) => WireError::new(code).with_message(reason),
            Self::PayloadTooLarge { found, bound } => WireError::new(code).with_message(format!(
                "the body is {found} bytes, over the {bound} byte bound"
            )),
            _ => WireError::new(code),
        }
    }

    /// The rendered status, envelope and retry hint.
    #[must_use]
    pub fn render(
        self,
        request_id: &RequestId,
        operation_id: Option<OperationId>,
    ) -> (u16, ApiError, Option<core::time::Duration>) {
        self.into_wire()
            .into_response_parts(request_id, operation_id)
    }
}

/// Maps an authorization denial onto the edge vocabulary.
///
/// The order matches the wire contract's precedence: a principal-kind or binding
/// refusal is a `403`, a role or scope shortfall is `403 insufficient_scope`, and
/// an absent credential is `401`.
#[must_use]
pub const fn from_denial(denial: aex_control_domain::Denial) -> EdgeError {
    use aex_control_domain::Denial;
    match denial {
        Denial::NotAuthenticated => EdgeError::Context(ContextError::NotCurrent),
        Denial::InsufficientRole { .. } | Denial::InsufficientScope { .. } => {
            EdgeError::InsufficientScope
        }
        Denial::NotAMember
        | Denial::WrongPrincipalKind { .. }
        | Denial::WrongOrganization
        | Denial::WrongWorkspace
        | Denial::WrongResourceClass => EdgeError::Forbidden,
    }
}

/// Maps an admission failure onto the edge vocabulary.
#[must_use]
pub const fn from_admission(admission: aex_control_domain::Admission) -> EdgeError {
    use aex_control_domain::Admission;
    match admission {
        Admission::Denied(denial) => from_denial(denial),
        Admission::Paused => EdgeError::AccountPaused,
        Admission::StateUnavailable => EdgeError::AccountStateUnavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::{EdgeError, from_admission, from_denial};
    use aex_control_domain::{Admission, Denial, OrgRole, Scope};
    use aex_wire::error::ErrorCode;
    use aex_wire::types::RequestId;

    fn request_id() -> RequestId {
        RequestId::parse("req-1").expect("a valid request id")
    }

    #[test]
    fn every_edge_failure_renders_its_pinned_status() {
        let table = [
            (EdgeError::NoRoute, 404),
            (EdgeError::CredentialRefused, 401),
            (EdgeError::AuthenticationUnavailable, 503),
            (EdgeError::Forbidden, 403),
            (EdgeError::InsufficientScope, 403),
            (EdgeError::AccountPaused, 402),
            (EdgeError::AccountStateUnavailable, 503),
            (EdgeError::NotFound, 404),
            (EdgeError::Gone, 410),
            (EdgeError::PayloadTooLarge { found: 1, bound: 0 }, 413),
            (EdgeError::InvalidCursor, 400),
            (EdgeError::InvalidRequest("bad".to_owned()), 400),
            (EdgeError::Internal("unmounted"), 500),
        ];
        for (error, status) in table {
            let rendered = error.clone().render(&request_id(), None);
            assert_eq!(rendered.0, status, "{error:?}");
            assert_eq!(rendered.1.error.request_id, "req-1");
        }
    }

    #[test]
    fn an_authentication_failure_never_repeats_the_parse_reason() {
        let error = EdgeError::Context(crate::authorizer::ContextError::Undeclared(
            "aex.impersonate".to_owned(),
        ));
        let (status, body, _) = error.render(&request_id(), None);
        assert_eq!(status, 401);
        assert!(
            !body.error.message.contains("impersonate"),
            "{}",
            body.error.message
        );
    }

    #[test]
    fn a_role_shortfall_and_a_scope_shortfall_are_the_same_public_answer() {
        assert_eq!(
            from_denial(Denial::InsufficientRole {
                required: OrgRole::Admin
            })
            .code(),
            ErrorCode::InsufficientScope
        );
        assert_eq!(
            from_denial(Denial::InsufficientScope {
                required: Scope::ApiKeysWrite
            })
            .code(),
            ErrorCode::InsufficientScope
        );
    }

    #[test]
    fn a_credential_bound_to_another_tenant_is_forbidden_not_not_found() {
        for denial in [Denial::WrongOrganization, Denial::WrongWorkspace] {
            assert_eq!(from_denial(denial).code(), ErrorCode::Forbidden);
        }
    }

    #[test]
    fn an_unreadable_account_state_is_retryable_and_a_paused_one_is_not() {
        let unavailable = from_admission(Admission::StateUnavailable);
        let paused = from_admission(Admission::Paused);
        assert!(unavailable.code().retryable());
        assert!(!paused.code().retryable());
    }
}
