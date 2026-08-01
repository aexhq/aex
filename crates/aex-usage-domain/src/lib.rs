//! `aex-usage-domain` owns the pure immutable usage fact and frontier model: facts,
//! corrections, evidence, identities, closure vectors and frontier order.
//!
//! # Invariants
//!
//! - a fact is immutable; an amendment is a correction fact that names its target
//! - a frontier advances only over a contiguous accepted sequence
//! - the four launch meters are the only representable meters
//!
//! # Not this crate's job
//!
//! - prices, rate cards or money (`aex-usage-rating`, `aex-finance-domain`)
//! - `AWS` or `SQL`
//! - customer `HTTP`

pub mod closure;
pub mod fact;
pub mod frontier;
