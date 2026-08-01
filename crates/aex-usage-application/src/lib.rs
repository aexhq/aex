//! `aex-usage-application` owns the usage accept, project, publish,
//! receive-settlement-receipt and rebuild use cases.
//!
//! # Invariants
//!
//! - duplicate and out-of-order facts converge on the same projection
//! - a central outage accumulates outbox backlog without corrupting the local frontier
//! - a rebuild reproduces the projection exactly from the immutable facts
//!
//! # Not this crate's job
//!
//! - finance balance mutation (`aex-finance-app`)
//! - concrete table or queue clients
//! - rating arithmetic (`aex-usage-rating`)

pub mod outbox;
pub mod ports;
#[cfg(feature = "probe")]
pub mod probe;
pub mod projection;
pub mod shadow;
pub mod use_cases;
pub mod worker;

#[cfg(test)]
pub(crate) mod testing;
