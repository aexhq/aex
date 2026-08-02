//! `usage-receipt-dispatcher` replays immutable settlement receipts until the
//! regional frontiers converge.
//!
//! # Invariants
//!
//! - a receipt is immutable, so replay is free and central truth never waits on
//!   a regional acknowledgement;
//! - this deployable can move `dispatch_state` and nothing else: its grants
//!   prove it can neither create nor alter a settlement;
//! - a regional outage grows the outbox and changes no money.
//!
//! # Not this crate's job
//!
//! - settling usage (`finance-settlement-worker`);
//! - advancing the regional frontier (the regional category workers).

pub mod config;
pub mod handler;
pub mod outbox;
