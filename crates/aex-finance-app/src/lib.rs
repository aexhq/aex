//! `aex-finance-app` owns finance, provider-effect and settlement use cases: the
//! prepare/execute/finalize state machine and account serialization.
//!
//! # Invariants
//!
//! - a provider effect records `unknown` rather than issuing a second charge
//! - duplicate and out-of-order usage facts stop at the inbox before rating
//! - one account group is serialized so two settlements cannot interleave
//!
//! # Not this crate's job
//!
//! - the Stripe protocol or provider credentials
//! - the balanced-journal invariants themselves (`aex-finance-domain`)
//! - concrete database or queue clients

pub mod effects;
pub mod ports;
pub mod use_cases;
