//! `aex-payment-contracts` owns the generated payment command and event boundary shared by
//! the finance Rust deployables and the two `TypeScript` Stripe edge Lambdas.
//!
//! # Invariants
//!
//! - every command carries an admitted effect identity and idempotency key
//! - redaction is part of the type: no payment instrument detail is representable in a
//!   loggable field
//! - the `TypeScript` and Rust encodings agree byte-for-byte on the shared corpus
//!
//! # Not this crate's job
//!
//! - talking to Stripe: the pinned provider protocol lives in `stripe-command-edge`
//! - money arithmetic, balances or the finance state machine (`aex-finance-domain`)
//! - webhook signature verification (`stripe-webhook-edge`)

pub mod command;
pub mod event;
