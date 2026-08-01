//! `aex-runtime-control` owns the pure Hands lifecycle and true-idle model, including the
//! exact 179999/180000 millisecond idle boundary and generation fencing.
//!
//! # Invariants
//!
//! - true idle requires a proven 180000 ms of inactivity; 179999 ms is not idle
//! - a generation is monotonic and a stale generation is always rejected
//! - lifecycle transitions are total functions of the recorded state
//!
//! # Not this crate's job
//!
//! - the compute provider (`aex-hands-control-aws`)
//! - activity storage (`aex-runtime-activity-dynamodb`)
//! - queues, schedules or wall-clock reads

pub mod generation;
pub mod idle;
pub mod lifecycle;
