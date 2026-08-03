//! `aex-brain-domain` owns the pure Brain fold, effect, child-activation, join and budget
//! model.
//!
//! # Invariants
//!
//! - effect identity is derived from the fold state, so a replay produces the same effect id
//! - a waiter is released exactly once, whatever the arrival order of its inputs
//! - a budget is checked before an effect is admitted, never after it has run
//! - the journal sequence is contiguous: a gap is a typed error and never folds, and the
//!   same sequence under a different hash quarantines the agent
//! - only a message carrying a [`wire_pending::CompleteProof`] enters model-visible
//!   history, and a [`wire_pending::PreviewFrame`] has no conversion into any journal
//!   variant
//!
//! # Not this crate's job
//!
//! - providers, tools, `MCP` or Hands transport
//! - `DynamoDB`, `S3` or `SQS` (`aex-brain-store-aws`)
//! - clock, randomness and identifier generation: all arrive as parameters
//!
//! There is deliberately no dependency on `tokio`, an AWS SDK, or a time-of-day source. A
//! [`ids::Timestamp`] is always a parameter, never a reading, which is what lets every
//! history in the test suite be replayed exactly.

pub mod budget;
pub mod canonical;
pub mod child;
pub mod commit;
pub mod context;
pub mod effect;
pub mod fold;
pub mod ids;
pub mod journal;
pub mod planner;
pub mod snapshot;
pub mod wire_pending;

pub use budget::{BudgetDelta, BudgetError, BudgetGrant, BudgetNode, Dimension, StructuralLimits};
pub use child::{CancelCause, ChildOutcome, ChildRecord, ChildState, QueuedReason};
pub use commit::{DecisionCommit, EnvelopeViolation, FenceGuardRef};
pub use effect::{
    DetachedOperationRef, DispatchEvidence, DispatchProof, DispatchStage, DurableEffect,
    EffectClass, EffectKind, EffectState, RecoveryDecision, SettledOutcome, recover,
};
pub use fold::{FoldError, FoldState, Phase, apply, fold};
pub use ids::{
    AgentId, AgentKey, AgentRevision, CancelEpoch, ContentHash, EffectId, Fence, JournalSeq,
    OwnerToken, SessionId, Timestamp,
};
pub use journal::{FinishReason, JournalDecodeError, JournalEntry, JournalRecord, ParkReason};
pub use planner::{OwedStep, PlanPolicy, plan};
pub use snapshot::{
    FOLD_SNAPSHOT_SCHEMA, FoldSnapshotArtifact, FoldSnapshotError, FoldSnapshotPointer,
    JournalPoint, VerifiedFoldSnapshot,
};
