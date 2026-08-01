//! `aex-observation-domain` owns the pure customer-observation authority model:
//! canonicalization, revision and order, series claims, receipts, gaps, frontiers and
//! deletion.
//!
//! # Invariants
//!
//! - a batch is hash-bound: its recorded digest covers exactly the admitted bytes
//! - a frontier advances only over a contiguous verified range
//! - a gap stays explicit until it is filled or explicitly retired; it is never skipped
//!
//! # Not this crate's job
//!
//! - `AWS`, `HTTP` or any `OTel` `SDK`
//! - the public query language (`aex-observation-query`)
//! - internal operational telemetry (`aex-platform-telemetry`)

pub mod batch;
pub mod frontier;
pub mod gap;
pub mod series;
