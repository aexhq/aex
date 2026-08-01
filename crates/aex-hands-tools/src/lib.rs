//! `aex-hands-tools` owns the guest-side command and tool implementation: command schemas,
//! working directory and environment handling, output capture, timeout and cancel.
//!
//! # Invariants
//!
//! - captured output is bounded and truncation is reported, never silently applied
//! - a timeout terminates the whole process tree and says so in the result
//! - resource observations are measurements, not estimates
//!
//! # Not this crate's job
//!
//! - the guest protocol framing (`aex-hands-protocol`, `aex-hands-agent`)
//! - `AEX` credentials or private routes: the guest has none
//! - Brain-managed network tools (`aex-brain-managed-web`)

pub mod command;
pub mod observation;
