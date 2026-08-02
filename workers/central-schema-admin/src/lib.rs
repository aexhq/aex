//! Central schema administration: the migration bundle and the privilege
//! allowlist that go with it.
//!
//! The binary in `src/main.rs` is a one-shot CLI over these modules and adds no
//! schema knowledge of its own. They are a library because the
//! privilege model has to be *proved*, and the only place that can happen is a
//! real `PostgreSQL` server: `crates/aex-control-aurora/tests/migrations.rs`
//! applies [`migration`]'s bundle and then [`grants`]'s rendered statements, and
//! probes the denial matrix through the same code path production runs. A grant
//! table nobody connects as is a document, not a control — and a second renderer
//! written for the test would prove only that the test agrees with itself.
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

/// Fixed outer advisory lock, ASCII `AEX_MIGR`.
///
/// Re-exported from the connection boundary so callers that only need to prove
/// lock identity do not have to import credential and TLS composition details.
pub use connect::ADVISORY_LOCK_KEY;
