//! `aex-brain-store-aws` owns the Brain `DynamoDB`, `S3` and `SQS` adapter: journal and
//! effect records, activation leases and fences, and wake publication.
//!
//! # Invariants
//!
//! - a lease is held by exactly one owner; a stale fence cannot write
//! - journal and effect records survive task loss and are replayable in order
//! - a wake message is published only after its durable cause is committed
//!
//! # Not this crate's job
//!
//! - Brain semantics (`aex-brain-domain`, `aex-brain-application`)
//! - provider or tool transport
//! - session authority rows (`aex-session-dynamodb`)

pub mod control;
pub mod effect;
pub mod expressions;
pub mod journal;
pub mod keys;
pub mod lease;
pub mod plan;
pub mod translate;
pub mod wake;

pub use expressions::{Action, ActionKind, Condition, Table, WakeItem, WorkExpressions};
pub use journal::BrainStore;
pub use keys::{BRAIN_AGENT_PARTITION_PREFIX, BRAIN_PREFIX, BrainKeyError};
pub use plan::{BrainTables, PlanError};
pub use translate::TranslateError;
pub use wake::{DueScan, SqsWakeQueue};
