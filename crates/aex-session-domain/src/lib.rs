//! `aex-session-domain` owns the pure session, run, agent and journal model: fold order,
//! lineage, the terminal barrier and idempotency.
//!
//! # Invariants
//!
//! - the journal fold is deterministic: replaying the same command history yields the same
//!   state
//! - a terminal run rejects every further command instead of reopening
//! - lineage is append-only; a clone never mutates its parent
//!
//! # Not this crate's job
//!
//! - `DynamoDB`, `S3` or any storage expression (`aex-session-dynamodb`)
//! - Brain activation or provider calls (`aex-brain-domain`)
//! - clock and identifier generation: both arrive as parameters

pub mod journal;
pub mod lineage;
pub mod run;
pub mod session;
