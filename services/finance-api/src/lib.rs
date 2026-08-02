//! `finance-api` is the public central money surface: balances, automatic
//! top-up policy, statements and the two hosted provider pages.
//!
//! # Invariants
//!
//! - money never crosses a boundary as a floating-point value; micro-USD widens
//!   to whole cents only through an exact conversion that refuses a remainder;
//! - a provider effect is durable **before** the provider is contacted and its
//!   answer is durable **before** the caller sees it;
//! - an indeterminate provider or commit outcome is recorded as unknown and is
//!   never turned into a determinate refusal, because that is how an account
//!   gets charged twice;
//! - the mounted route set is `RouteGroup::Billing.routes()` and is asserted to
//!   be, so an authored route that is never mounted is a red suite.
//!
//! # Not this crate's job
//!
//! - credential verification and principal resolution (`aex-central-http`,
//!   behind [`edge::CentralEdge`]);
//! - the Stripe protocol (`stripe-command-edge`);
//! - rating, settlement and the journal itself.

pub mod aurora;
pub mod authority;
pub mod billing;
pub mod config;
pub mod download;
pub mod edge;
pub mod gateway;
pub mod health;
pub mod mount;
