//! Bounded outbound URL, redirect, DNS, size, media-type and timeout guards reused by MCP.
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

pub mod egress;
pub mod fetch;
pub mod search;
pub mod serializer;
