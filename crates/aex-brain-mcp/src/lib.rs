//! `aex-brain-mcp` owns the `MCP` protocol and recovery adapter: pinned protocol
//! conformance, task identity, cancellation and reconnect.
//!
//! # Invariants
//!
//! - a reconnect resumes by task identity; it never silently re-runs a completed task
//! - an unsupported protocol revision is refused at handshake, not worked around
//! - cancellation is propagated and acknowledged, not assumed
//!
//! # Not this crate's job
//!
//! - tool policy (`aex-brain-tool-catalog`)
//! - provider model calls (`aex-brain-provider-gateway`)
//! - server hosting: this crate is the client side

pub mod client;
pub mod recovery;
pub mod wire_pending;

#[cfg(test)]
mod tests;
