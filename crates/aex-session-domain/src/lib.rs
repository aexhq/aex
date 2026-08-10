//! `aex-session-domain` owns the pure session, run, agent and journal model: fold order,
//! lineage, the terminal barrier and idempotency.
//!
//! # Invariants
//!
//! - the journal fold is deterministic and total: `fold(prior, a ++ b)` equals
//!   `fold(fold(prior, a), b)` for every split, and a rejected batch leaves
//!   `prior` byte-identical;
//! - exactly one writer settles a run, and every loser produces no write at all;
//! - a terminal run, a sealed message and a purged session all reject every
//!   further command instead of reopening;
//! - lineage is append-only: a clone never mutates its source.
//!
//! # Not this crate's job
//!
//! - `DynamoDB`, `S3` or any storage expression (`aex-session-dynamodb`);
//! - Brain activation, payload interpretation or provider calls
//!   (`aex-brain-domain`);
//! - clock and identifier generation: both arrive as parameters (D-01).

pub mod agent;
pub mod approval;
pub mod budget;
pub mod deletion;
pub mod idempotency;
pub mod ids;
pub mod journal;
pub mod lineage;
pub mod message;
pub mod pause;
pub mod run;
pub mod session;
pub mod terminal;
pub mod testing;

pub use agent::{
    AgentClaim, AgentCommit, AgentControl, AgentError, AgentKind, AgentStatus, AgentTerminal,
    CancelCause, CancelSessionWorkCommit, JoinEdge, MaterializedState, OpenEffectSet,
    PublicAgentStatus, QueueReason, cancel_session_work, complete_agent, create_root, spawn,
    start_agent,
};
pub use approval::{
    Approval, ApprovalBinding, ApprovalCancelCause, ApprovalCommit, ApprovalDecision,
    ApprovalOutcome, ApprovalRejection, ApprovalStatus, BindingField, CancelScope, binding_drift,
    cancel_pending, cause_in_scope, expire_pending, request_approval, respond,
};
pub use budget::{BudgetGrant, EffectiveLimits, LimitUnresolved};
pub use deletion::{
    DeletionEpoch, DeletionGuard, DeletionRejection, DeletionState, PurgeCommit, PurgeEvidence,
    RestoreCommit, SessionTombstone, TrashCommit, purge, purge_complete, restore, trash,
};
pub use idempotency::{
    IdempotencyIdentity, IdempotencyReceipt, ReceiptKey, ReceiptKeyError, ReceiptOutcome,
    ReplayDecision, ResourceId, ResourceKind, ResponseBody, replay,
};
pub use ids::{
    AccountRevision, AgentFence, AgentRevision, AuthorizationEpoch, CancellationEpoch, EffectId,
    EntryIdentity, JournalSeq, PersistRevision, ReservationId, SessionRevision, UsageClosureId,
};
pub use journal::{
    AuthorityFact, INLINE_BODY_MAX_BYTES, JournalBody, JournalEntry, JournalError, JournalPage,
    fold_control, validate_append,
};
pub use lineage::{
    CloneFiles, CloneOutcome, CloneRequest, Lineage, Origin, PurgeCascade, detach, plan_clone,
};
pub use message::{
    Message, MessageDelta, MessageError, MessagePart, MessageRole, MessageState, append_part, seal,
};
pub use pause::{
    AccountProjection, AccountState, CommandClass, ExemptCommand, PauseReason, PauseRejection,
    pause_gate, project_account,
};
pub use run::{
    DomainError, InterruptReason, QueueRun, Run, RunCommit, RunOutcome, RunStatus,
    SessionDomainRunError, queue, start,
};
pub use session::{
    MutationGuard, ResolvedConfigAuthority, ResolvedConfigDigest, ResolvedConfigError, Session,
    SessionError, SessionMetadata, SessionMetadataError, SessionStatus, WorkAdmission,
    acquire_mutation_guard, public_root_hash, release_mutation_guard,
};
pub use terminal::{
    OutboxEvent, TerminalAttempt, TerminalCommit, TerminalRejection, claim_terminal, sealed_ids,
};
