//! `aex-work-dynamodb` owns the `regional-work` table adapter: the due index, claim leases,
//! fences, attempt counters and the bounded reconciliation cursor.
//!
//! # Invariants
//!
//! - a claim is won by exactly one worker; the losers observe a typed conflict
//! - the fence recorded with a claim is written into every effect that claim produces
//! - the due index is queried with a bounded page, never a full scan
//!
//! # Not this crate's job
//!
//! - operation semantics (`aex-operation-domain`)
//! - the work a claim performs
//! - session or content tables

pub mod claim;
pub mod expressions;
pub mod store;
