//! Temporary inspectable transaction plan until the regional application merge.

// TODO(cross-stream): replaced by aex_session_app::TransactionPlan at merge
/// Minimal transaction plan needed to prove atomic admission composition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionPlan<A> {
    /// `DynamoDB` client request token.
    pub client_request_token: String,
    /// Ordered transaction actions.
    pub actions: Vec<A>,
}
