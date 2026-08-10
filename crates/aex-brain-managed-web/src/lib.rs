//! `aex-brain-managed-web` owns the Brain-managed network tool adapter: bounded fetch and
//! search with `URL`, redirect, `DNS`, size, media-type and timeout guards.
//!
//! # Invariants
//!
//! - every resolved address is checked against the `SSRF` deny policy, including after each
//!   redirect
//! - response size and time are bounded before the body is read
//! - results are canonicalized so the same page yields the same tool result bytes
//!
//! # Not this crate's job
//!
//! - tool policy or approval (`aex-brain-tool-catalog`)
//! - customer egress from the Hands guest (`aex-hands-tools`)
//! - credential admission and rebind policy (`aex-secret-domain`)

/// The Brain's tenant-BYOK credential source. Behind `brain-adapter` because it
/// links the session and secret-custody stores, which the platform-paid executor
/// must not.
#[cfg(feature = "brain-adapter")]
pub mod credential;
pub mod egress;
/// The Brain's `ToolExecutor` implementation. Behind `brain-adapter` for the
/// same reason as [`credential`].
#[cfg(feature = "brain-adapter")]
pub mod executor;
pub mod fetch;
pub mod search;
pub mod serializer;

#[cfg(all(test, feature = "brain-adapter"))]
mod tests;
