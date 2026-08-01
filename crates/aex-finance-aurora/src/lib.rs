//! `aex-finance-aurora` owns the finance `PostgreSQL` adapter: balanced-posting
//! constraints, atomic projection/inbox/outbox writes and lock/serialization behaviour.
//!
//! # Invariants
//!
//! - the balance projection and its journal rows are written in the same transaction
//! - a crash between statements leaves either the whole transition or none of it
//! - database constraints enforce balance independently of application code
//!
//! # Not this crate's job
//!
//! - finance policy (`aex-finance-domain`)
//! - migrations and grants (`central-schema-admin`)
//! - observation or usage query implementation

pub mod row;
pub mod store;
pub mod tx;

#[cfg(test)]
mod tests;
