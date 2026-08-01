//! `aex-rds-data` owns the focused Aurora Data `API` transport: request and parameter
//! mapping, paging, transaction lifecycle and error classification.
//!
//! # Invariants
//!
//! - a commit whose outcome is unknown is reported as ambiguous, never as success
//! - parameters are typed and bound; no statement is assembled by string concatenation
//! - retry classification is explicit per error family, with no blanket retry
//!
//! # Not this crate's job
//!
//! - schema knowledge: table and column names belong to the owning `*-aurora` adapter
//! - native `PostgreSQL` connections (`central-schema-admin` owns the `sqlx` path)
//! - business transactions or authority decisions

pub mod client;
pub mod params;
pub mod transaction;
