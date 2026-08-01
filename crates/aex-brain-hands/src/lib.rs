//! `aex-brain-hands` owns the Brain-side exact-generation Hands adapter: operation
//! dispatch, frame handling, fence checks, cancellation and result incorporation.
//!
//! # Invariants
//!
//! - every operation is addressed to an exact generation; a generation mismatch is refused
//! - guest loss is observable as guest loss, not as a failed operation result
//! - a cancelled operation is never incorporated into the fold
//!
//! # Not this crate's job
//!
//! - the wire protocol definition (`aex-hands-protocol`)
//! - guest-side execution (`aex-hands-agent`, `aex-hands-tools`)
//! - runtime lifecycle (`aex-runtime-control`, `aex-hands-control-aws`)

pub mod adapter;
pub mod operation;
