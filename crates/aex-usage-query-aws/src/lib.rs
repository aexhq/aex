//! `aex-usage-query-aws` owns the read-only `usage-query-projection` adapter: bounded
//! aggregate and detail queries plus the cursor codec.
//!
//! # Invariants
//!
//! - the adapter exposes no write capability at all; the type system prevents one
//! - every query is bounded by page size and scanned range
//! - the copied coverage vector is returned with every answer
//!
//! # Not this crate's job
//!
//! - fact or outbox writes (`aex-usage-*-aws`)
//! - rating policy (`aex-usage-rating`)
//! - customer `HTTP` or authorization

pub mod cursor;
pub mod expressions;
