//! `aex-hands-control-aws` owns the trusted Hands provider control adapter: launch, probe,
//! suspend, resume, terminate and snapshot with explicit ambiguous-outcome handling.
//!
//! # Invariants
//!
//! - every control call carries the exact generation it intends to act on
//! - an ambiguous provider outcome is recorded for reconciliation, never retried blindly
//! - a terminate is idempotent: repeating it on a gone runtime is success
//!
//! # Not this crate's job
//!
//! - the lifecycle and true-idle rules (`aex-runtime-control`)
//! - activity table rows (`aex-runtime-activity-dynamodb`)
//! - anything inside the guest

pub mod lifecycle;
pub mod provider;
