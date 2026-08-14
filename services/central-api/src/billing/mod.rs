//! Essential prepaid billing handlers retained by the session-MVP control API.
//!
//! Balance, cards, top-up, immutable transactions, and bounded rated usage
//! share the control listener. Provider protocol remains isolated in the
//! IAM-only arm of `stripe-webhook-edge`.

pub mod aurora;
pub mod authority;
pub mod billing;
pub mod gateway;

/// Purpose-specific database role used by the billing authority.
pub const REQUIRED_ROLE: &str = "aex_finance_api";
