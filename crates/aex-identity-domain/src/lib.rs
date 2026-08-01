//! `aex-identity-domain` owns the pure identity state machine: browser challenges, identity
//! links, sessions and revocation.
//!
//! # Invariants
//!
//! - a challenge is single-use; consuming it twice is a typed rejection
//! - session and link transitions are total functions of the prior state and the command
//! - revocation is monotonic: an epoch never moves backwards
//!
//! # Not this crate's job
//!
//! - storage, SQL or transactions (`aex-identity-aurora`)
//! - HTTP, cookies or `OAuth` provider transport (`central-identity-api`)
//! - reading the clock or the environment: time arrives as a parameter

pub mod challenge;
pub mod link;
pub mod session;
