//! `aex-brain-test-support` owns deterministic Brain, provider, tool, `MCP` and Hands fixtures plus the cleanup ledger for everything an activation creates.
//!
//! # Invariants
//!
//! - every fixture is deterministic: the clock is injected, identifiers come
//!   from a seeded factory, and nothing reads the wall clock or a random source
//! - every created resource is recorded in a [`teardown::FixtureLedger`], which
//!   panics at drop if anything was left behind
//! - every synthetic name lives under one per-run prefix, so two concurrent runs
//!   cannot collide and residue is always attributable
//! - no fixture contains a real credential, a real customer identifier or a
//!   plaintext secret
//!
//! # Not this crate's job
//!
//! - production behaviour: this crate is `publish = false` and must be absent
//!   from every production artifact link graph, which
//!   `tools/aex-workspace-check` proves
//! - live-environment orchestration: that belongs to the `tests/live/aex-live-*`
//!   companion packages
//! - assertions about product correctness; it supplies inputs and teardown, not
//!   expectations

pub mod clock;
pub mod fixtures;
pub mod histories;
pub mod ids;
pub mod journal_gen;
pub mod prefix;
pub mod teardown;

pub use clock::TestClock;
pub use histories::{Golden, Rejection};
pub use ids::IdFactory;
pub use journal_gen::{HistoryBuilder, Hostile};
pub use prefix::{PrefixError, RunPrefix};
pub use teardown::{BrainResource, FixtureEntry, FixtureLedger};
