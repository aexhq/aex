//! `aex-identity-aurora` owns the identity `PostgreSQL` adapter: schema-bound statements,
//! transactions and role denials over the Aurora Data API.
//!
//! # Invariants
//!
//! - every write runs inside one transaction that either commits or leaves no trace
//! - a constraint violation maps to a typed domain error, never a raw driver string
//! - the adapter uses only the identity schema; control and finance `DML` is denied by role
//!
//! # Not this crate's job
//!
//! - identity policy or state transitions (`aex-identity-domain`)
//! - schema migration or grants (`central-schema-admin`)
//! - `HTTP` handling or authorization decisions

pub mod store;
