//! `billing-worker` owns provider ingest, usage settlement, and money reconciliation for the session MVP.
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
//! The single Lambda accepts direct verified provider events, SQS FIFO rating
//! batches, and scheduled reconciliation sweeps. The three modes keep separate
//! database roles so consolidating compute does not collapse money authority.

pub mod config;
pub mod handler;
pub mod inbox;
pub mod journal;
pub mod reconcile;
pub mod runtime;
pub mod settlement;
