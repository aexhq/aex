//! `central-schema-admin` is the one identity in the platform with DDL,
//! ownership, role-management and migration-history write privilege.
//!
//! # Invariants
//!
//! - one task per admitted migration: the outer advisory lock makes a second
//!   task exit `11` instead of interleaving;
//! - checksum drift on an applied version fails closed; there is deliberately
//!   no checksum-exception mechanism, because an approved-transition map
//!   rewrites history and defeats the checksum;
//! - the expected bundle head and the expected applied head are both asserted
//!   **before** any mutation, so an out-of-order release applies nothing;
//! - grants are declarative and diff-applied; a grant written by hand inside a
//!   migration is a CI failure;
//! - the DDL credential is held in a `Zeroizing<String>` and never enters argv,
//!   output or a log line.
//!
//! # Not this crate's job
//!
//! - application data access: every request and worker Lambda reaches Aurora
//!   through the Data API and stays out of the VPC;
//! - deciding *what* a migration does: the SQL is the authority.

pub mod connect;
pub mod grants;
pub mod migration;
pub mod runner;
