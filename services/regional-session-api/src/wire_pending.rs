//! Temporary inspectable transaction plan until the regional application merge.

// TODO(cross-stream): `aex-session-app` publishes no `TransactionPlan`. Its plan type is
// `aex_session_app::plan::SessionTransaction`, which carries typed
// `plan::Write` actions and `plan::Condition` guards rather than an opaque `Vec<A>`, and
// it names its idempotency handle inside the plan rather than beside it.
/// Minimal transaction plan needed to prove atomic admission composition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionPlan<A> {
    /// `DynamoDB` client request token.
    pub client_request_token: String,
    /// Ordered transaction actions.
    pub actions: Vec<A>,
}
