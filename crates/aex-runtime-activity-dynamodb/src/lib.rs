//! `aex-runtime-activity-dynamodb` owns the `runtime-activity` table adapter: exact Hands
//! generations, lifecycle intents and receipts, true-idle heartbeats and snapshot activity.
//!
//! # Invariants
//!
//! - every lifecycle write is conditional on the exact generation; a stale generation is
//!   rejected
//! - a heartbeat can only move activity forward in time
//! - an intent and its receipt are matched by identity, never by ordering
//!
//! # Not this crate's job
//!
//! - the lifecycle and true-idle rules (`aex-runtime-control`)
//! - the compute provider (`aex-hands-control-aws`)
//! - queue delivery (`aex-runtime-control-aws`)

pub mod expressions;
pub mod store;
