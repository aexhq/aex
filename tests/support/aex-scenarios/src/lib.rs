//! The cross-service scenario bodies, written against the engine-backed
//! integration lane rather than against a plane that does not exist yet.
//!
//! `references/deferred-closure-design-2026-08-09/PLAN-2026-08-10.md` P7.1 asks
//! for four scenario bodies whose value is that they *execute*. The plane they
//! will eventually run against is Phase 6 work, so every body here composes the
//! same production crates a deployable composes and points them at a
//! digest-pinned engine started by [`aex_test_harness::containers`]. The body is
//! then promotable: replacing the engine handle with a plane endpoint moves it
//! from the `integration` layer to the `e2e` layer without rewriting a single
//! assertion.
//!
//! # What this crate is
//!
//! The substrate the four bodies share, and nothing else:
//!
//! - [`data_api`] — the Aurora Data `API` [`aex_rds_data::Transport`] spoken to
//!   a real `PostgreSQL`. This is what makes a central scenario executable at
//!   all: `aex-control-aurora`, `aex-identity-aurora` and `aex-finance-aurora`
//!   all issue their statements through that seam, so implementing it over a
//!   container runs **the production SQL**, the production parameter binding,
//!   the production row decoding and the production error classification. A
//!   fake repository would prove none of them.
//!
//! # What this crate is not
//!
//! - an emulator anybody may ship: it exists to run committed statements
//!   against the engine those statements were written for, and it refuses
//!   anything it cannot render faithfully rather than guessing;
//! - a replacement for live-seam evidence: `aws.rds_data.transaction` is a
//!   `requires_live` seam and stays claimed by `aex-live-central-identity-api`.
//!   What runs here is the schema, the statements and the classification. What
//!   does not is Aurora's own Data `API` endpoint.
//!
//! # Invariants
//!
//! - nothing falls back: a statement this transport cannot render is a loud
//!   failure carrying the statement's own text, never a silently altered query;
//! - floating point cannot cross the boundary in either direction, matching
//!   [`aex_rds_data::record`]'s refusal;
//! - a `PostgreSQL` `SQLSTATE` is forwarded verbatim, so the production error
//!   table classifies a real database answer rather than a harness invention.

pub mod data_api;

pub use data_api::literal::{RecordError, parse_array_literal, parse_row_literal};
pub use data_api::render::{Bound, RenderError, Rendered, Shape, render};
#[cfg(feature = "integration-engines")]
pub use data_api::transport::PostgresDataApi;
