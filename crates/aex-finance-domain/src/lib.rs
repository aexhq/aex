//! `aex-finance-domain` owns the pure balanced money authority: journal postings,
//! reservations, settlements, reversals and disputes.
//!
//! # Invariants
//!
//! - every transition is balanced: postings sum to zero across the affected accounts
//! - money is integer micro-USD; no floating point value is representable
//! - a reversal is a new balanced posting, never an edit of a recorded one
//!
//! # Not this crate's job
//!
//! - AWS delivery, Stripe transport or SQL (each finance deployable owns its own)
//! - rate cards and rating arithmetic (`aex-usage-rating`)
//! - reading the clock: settlement time arrives as a parameter

pub mod account;
pub mod billing_account;
pub mod effect;
pub mod journal;
pub mod money;
pub mod reservation;
pub mod settlement;
pub mod transitions;

pub use account::{AccountKind, AccountRef, AccountSide, Currency};
pub use journal::{
    BalancedTransaction, BusinessKey, ConservationError, IntentHash, Posting, TransactionId,
    TransactionKind,
};
pub use money::{Cents, MICROUSD_PER_CENT, Microusd, MicrousdDelta, MoneyError};

#[cfg(test)]
mod tests;
