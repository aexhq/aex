//! `aex-session-app` owns session admission and continuation use cases: transactional
//! admission, trash/restore/purge coordination and the authority ports.
//!
//! # Invariants
//!
//! - a use case returns exactly one [`plan::SessionTransaction`] and never
//!   commits: [`ports::AppContext`] holds no committer, so it cannot;
//! - every port call happens **before** the plan is built, so a plan is a pure
//!   function of what was read;
//! - an admission either writes every participant of its declared transaction or
//!   none;
//! - authorization and the pause gate run before idempotent replay, so a paused
//!   caller is never re-shown a grant;
//! - long commands become durable operations rather than held-open requests.
//!
//! # Not this crate's job
//!
//! - concrete AWS clients or table names;
//! - the session fold itself (`aex-session-domain`);
//! - `HTTP` routing or authentication.

pub mod error;
pub mod outcome;
pub mod plan;
pub mod ports;
pub mod testing;
pub mod use_cases;

pub use error::AppError;
pub use outcome::{Attempted, Observed, ProviderAnswer, Resolution, resolve};
pub use plan::{
    Condition, ConditionId, Hint, ItemKey, MAX_ACTIONS, MAX_BYTES, PlanError, PlanShape, Planned,
    SessionTransaction, TableFamily, TransactionIntent, Write,
};
pub use ports::{
    AccountStateReader, AgentCancelPage, AgentCancelTarget, AgentPage, AppContext,
    AuthorityCommitter, Clock, CommitError, CommitOutcome, ContentReader, ContinuityReader,
    IdFactory, LimitsReader, LiveWorkspaceReader, PageBudget, PortError, RegistryReader,
    ReservationAuthority, ReservationGrant, ReservationRequest, SecretCustodyReader, SessionReader,
    SessionSnapshot, VersionedOperation, WorkspaceContinuity,
};
pub use use_cases::{
    CommitTerminal, Purge, RECOVERY_WINDOW, Rebind, Resume, STOP_BATCH_AGENTS, SendMessage,
    SessionCommand, StartRun, admit_message, commit_terminal, continue_operation, continue_stop,
    purge_session, rebind_credentials, restore_session, start_run, stop_session, trash_session,
};
