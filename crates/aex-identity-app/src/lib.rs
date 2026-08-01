//! `aex-identity-app` owns identity use cases and the ports they need: command handling,
//! idempotency, authorization and error mapping.
//!
//! # Invariants
//!
//! - a command either commits once or fails closed; there is no partial identity mutation
//! - an unknown downstream outcome is recorded as unknown and reconciled, never inferred from
//!   a timeout
//! - every port is an interface owned here, not a concrete client
//!
//! # Not this crate's job
//!
//! - concrete vendor clients, connection pools or global singletons
//! - the identity state machine itself (`aex-identity-domain`)
//! - transport framing or route definitions

pub mod ports;
pub mod use_cases;
