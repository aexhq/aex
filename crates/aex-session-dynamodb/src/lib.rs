//! `aex-session-dynamodb` owns the `session-authority` table adapter: transactions,
//! condition expressions, idempotency records and stream shapes.
//!
//! # Invariants
//!
//! - every mutation carries a condition expression; an unconditional write is a bug
//! - a transaction names every participant it touches, so a partial commit is impossible
//! - the adapter writes only `session-authority`; no sibling table is linked
//!
//! # Not this crate's job
//!
//! - the session fold or its invariants (`aex-session-domain`)
//! - runnable work and claims (`aex-work-dynamodb`)
//! - `HTTP`, authentication or queue publication

pub mod expressions;
pub mod store;
