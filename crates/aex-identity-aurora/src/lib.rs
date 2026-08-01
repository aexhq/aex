//! `aex-identity-aurora` is the schema-owned SQL for the identity plane.
//!
//! # Invariants asserted by this crate's own suite
//!
//! - every statement is a `const &str` in [`sql`]; no format-string SQL and no
//!   identifier interpolation
//! - a single-use credential is consumed by a **conditional** `UPDATE` whose
//!   predicate repeats the domain guard, so exactly one of N concurrent
//!   consumers wins without the application having to arbitrate
//! - resolving a session performs no write at all, which the read-only role
//!   proves by holding no write privilege
//!
//! # Not this crate's job
//!
//! - domain policy (`aex-identity-domain`)
//! - retry policy: `aex-rds-data` classifies, the application decides

pub mod error;
pub mod sql;

pub use error::map_store_error;
