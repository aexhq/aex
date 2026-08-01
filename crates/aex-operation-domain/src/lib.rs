//! `aex-operation-domain` owns the pure durable-operation, lease and fence model shared by
//! every regional worker.
//!
//! # Invariants
//!
//! - a stale fence is always rejected, whatever the lease timing
//! - claim, renew, steal, complete and cancel are total transitions over the recorded state
//! - a due scan is idempotent: rescanning the same due set produces no extra effect
//!
//! # Not this crate's job
//!
//! - queues, tables or schedulers (`aex-work-dynamodb`)
//! - the work a specific operation performs
//! - wall-clock reads: the current instant is a parameter

pub mod fence;
pub mod lease;
pub mod operation;
