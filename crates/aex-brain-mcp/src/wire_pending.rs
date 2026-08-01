//! Temporary peer vocabulary until Brain domain contracts land.

/// Proof of whether a request could have crossed the socket boundary.
// TODO(cross-stream): replaced by aex_brain_domain::DispatchProof at merge
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchProof {
    /// Failure was observed before any socket write.
    NotSent,
    /// Dispatch may have occurred, but no response byte was observed.
    PossiblySent,
    /// At least one response byte was observed.
    ResponseStarted,
}
