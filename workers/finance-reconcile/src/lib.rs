//! `finance-reconcile` runs the conservation, unresolved-effect and statement
//! sweeps.
//!
//! # Invariants
//!
//! - the sweeps **report and fence**; they never auto-repair money, because
//!   auto-repair hides the defect that caused the drift (F-30);
//! - an effect is escalated to `manual_review` only after its exact-key replay
//!   window has elapsed; inside the window recovery replays the same
//!   idempotency key and never mints a second one;
//! - the journal wins over the projection on divergence, and this worker holds
//!   no grant that could rewrite either.
//!
//! # Not this crate's job
//!
//! - settling usage (`finance-settlement-worker`);
//! - the Stripe protocol (`stripe-command-edge`).

pub mod config;
pub mod handler;
pub mod sweep;
