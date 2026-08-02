//! `aex-runtime-control-aws` owns the runtime-activity, queue and provider composition used
//! by the runtime control worker: partial-batch handling, claims and receipts.
//!
//! # Invariants
//!
//! - a partial batch failure reports exactly the failed identifiers, never the whole batch
//! - a receipt is written before the claim is released
//! - an `IAM` denial is a typed denial, not a retry loop
//!
//! # Not this crate's job
//!
//! - the lifecycle rules (`aex-runtime-control`)
//! - provider control calls (`aex-hands-control-aws`)
//! - Brain semantics

pub mod composition;
pub mod queue;
pub mod usage_ingress;
pub mod worker;

pub use composition::{HoldReason, SUSPEND_LOCK_MS, SuspendDecision, evaluate_suspend, recount};
pub use queue::{
    BatchItem, BatchItemFailure, BatchResult, ItemOutcome, MAX_RECEIVE_COUNT, PartialBatchFailure,
    Quarantined, fold_batch,
};
pub use worker::{
    AWAIT_BUDGET_MS, CommandOutcome, Pace, QueueRecord, RuntimeCommand, RuntimeControl,
    RuntimePorts, RuntimeSettings, SchedulePass, Settled, intent_id,
};
