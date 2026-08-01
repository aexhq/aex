//! `aex-hands-agent` owns the credential-free guest-root protocol agent: the command/result
//! journal, the process tree, reconnect, cancel and crash observation.
//!
//! # Invariants
//!
//! - the agent holds no `AEX` credential, role or private route; it is not a security
//!   authority
//! - a reconnect replays from the journal rather than losing or duplicating a command result
//! - a killed process tree is reported as observed, not inferred
//!
//! # Not this crate's job
//!
//! - being a security boundary: isolation is the runtime's job, not the agent's
//! - the protocol definition (`aex-hands-protocol`)
//! - runtime lifecycle decisions (`aex-runtime-control`)

pub mod journal;
pub mod session;
