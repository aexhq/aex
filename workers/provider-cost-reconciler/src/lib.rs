//! `provider-cost-reconciler` records the private COGS fact family and reports
//! a margin finding.
//!
//! # Invariants
//!
//! - `finance.provider_cost_fact` has **no path to a customer posting** (F-26):
//!   this deployable holds no grant that could charge anybody, and its
//!   readiness probe refuses to start if it ever does;
//! - a duplicate export row normalises on `(source, source_row_id)`;
//! - margin is exact integer basis points and is undefined — not `-100 %` —
//!   against zero revenue;
//! - missing COGS degrades margin visibility and alerts; it never changes a
//!   customer balance.
//!
//! # Not this crate's job
//!
//! - pricing or rating (`aex-usage-rating`);
//! - any customer-visible amount at all.

pub mod config;
pub mod cost;
pub mod export;
pub mod handler;
