//! `finance-ingest` is the Stripe normalized-event inbox and the authoritative
//! finance transition it implies.
//!
//! # Invariants
//!
//! - success is returned **only** after the inbox row and its money transition
//!   are both durable, so the webhook edge's `2xx` means the provider will not
//!   have to redeliver;
//! - a duplicate delivery converges on `provider_event_id` and posts nothing a
//!   second time;
//! - a lost commit response is reported as an unknown outcome, never as a
//!   failure the caller may treat as "nothing happened";
//! - money is integer micro-USD and every transition is balanced in Rust before
//!   a statement runs.
//!
//! # Not this crate's job
//!
//! - webhook signature verification or any Stripe protocol (`stripe-webhook-edge`);
//! - rating and settlement (`finance-settlement-worker`);
//! - resolving an unknown provider effect (`finance-reconcile`).

pub mod config;
pub mod handler;
pub mod inbox;
pub mod journal;
