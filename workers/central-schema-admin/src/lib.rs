//! Central schema administration: the migration bundle and the privilege
//! allowlist that go with it.
//!
//! The binary in `src/main.rs` is a one-shot CLI over exactly these two modules
//! and adds no schema knowledge of its own. They are a library because the
//! privilege model has to be *proved*, and the only place that can happen is a
//! real `PostgreSQL` server: `crates/aex-control-aurora/tests/migrations.rs`
//! applies [`migration`]'s bundle and then [`grants`]'s rendered statements, and
//! probes the denial matrix through the same code path production runs. A grant
//! table nobody connects as is a document, not a control — and a second renderer
//! written for the test would prove only that the test agrees with itself.

pub mod grants;
pub mod migration;

/// Fixed outer advisory lock, ASCII `AEX_MIGR`.
///
/// Every mutating command takes it, so two schema-admin tasks racing on the same
/// database serialise rather than interleave.
pub const ADVISORY_LOCK_KEY: i64 = 0x4145_585F_4D49_4752;
