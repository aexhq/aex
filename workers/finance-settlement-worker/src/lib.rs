//! `finance-settlement-worker` drains the FIFO rating backlog into the money
//! authority, one account at a time.
//!
//! # Invariants
//!
//! - `MessageGroupId` is exactly the organization id, so SQS serialises one
//!   account and parallelises across accounts (OD-19, F-10);
//! - one account group is one serializable transaction; a partial-batch
//!   response names only the messages whose group did not commit;
//! - a duplicate delivery converges on the inbox key and posts nothing twice;
//! - the same fact identity under a different intent is quarantined, never
//!   overwritten;
//! - only a serialization failure is retried inside an invocation; an unknown
//!   commit outcome goes back to the queue and is resolved against durable
//!   state on redelivery.
//!
//! # Not this crate's job
//!
//! - the exact rating arithmetic (`aex-usage-rating`);
//! - producing usage facts (the regional usage workers);
//! - dispatching settlement receipts (`usage-receipt-dispatcher`).

pub mod backlog;
pub mod config;
pub mod handler;
pub mod settle;
