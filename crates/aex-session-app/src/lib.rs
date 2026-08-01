//! `aex-session-app` owns session admission and continuation use cases: transactional
//! admission, clone/trash/restore/purge/rebind coordination and the authority ports.
//!
//! # Invariants
//!
//! - durable authority commits before any queue hint is published, so a lost hint is
//!   recoverable
//! - an admission either writes every participant of its declared transaction or none
//! - long commands become durable operations rather than held-open requests
//!
//! # Not this crate's job
//!
//! - concrete AWS clients or table names
//! - the session fold itself (`aex-session-domain`)
//! - `HTTP` routing or authentication

pub mod ports;
pub mod use_cases;
