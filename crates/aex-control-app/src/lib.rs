//! `aex-control-app` owns central control use cases: command idempotency, the durable
//! outbox, unknown-effect resolution and authorization matrices.
//!
//! # Invariants
//!
//! - the durable commit and the external effect are separate steps joined by an outbox row
//! - the same command identity replays to the same result without a second effect
//! - authorization is decided from current state; there is no stale-permission fallback
//!
//! # Not this crate's job
//!
//! - concrete AWS, email or `HTTP` clients
//! - control invariants themselves (`aex-control-domain`)
//! - regional data-plane authority

pub mod ports;
pub mod use_cases;
