//! `aex-control-aurora` owns the control `PostgreSQL` adapter: control-schema statements,
//! inbox/outbox tables, grants and concurrent membership/key transactions.
//!
//! # Invariants
//!
//! - inbox, outbox and state mutations commit in one transaction
//! - concurrent membership and key transitions serialize; the loser gets a typed conflict
//! - the adapter holds no finance grant
//!
//! # Not this crate's job
//!
//! - control policy (`aex-control-domain`)
//! - migrations and grants (`central-schema-admin`)
//! - queue delivery or email transport

pub mod store;
