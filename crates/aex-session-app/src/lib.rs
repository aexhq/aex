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

pub mod create;
pub mod error;
pub mod lifecycle;
pub mod outcome;
pub mod plan;
pub mod ports;
pub mod projection;
pub mod testing;
pub mod use_cases;

pub use create::{
    CREATE_SCOPE, CreateSession, INITIAL_FILES_HARD_MAX_BYTES, PrepareSessionCreateOutcome,
    PreparedSessionCreate, ReadySessionLaunch, ResolvedInitialFile, initial_root_agent,
    prepare_session_create, publish_ready_session,
};
pub use error::AppError;
pub use lifecycle::{LifecycleAdmissionOutcome, LifecycleCommand, admit_lifecycle_operation};
pub use outcome::{Attempted, Observed, ProviderAnswer, Resolution, resolve};
pub use plan::{
    Condition, ConditionId, Hint, ItemKey, MAX_ACTIONS, MAX_BYTES, PlanError, PlanShape, Planned,
    SessionTransaction, TableFamily, TransactionIntent, Write,
};
pub use ports::{
    AccountStateReader, AgentCancelPage, AgentCancelTarget, AgentPage, AppContext,
    AuthorityCommitter, Clock, CommitError, CommitOutcome, CredentialState, DeploymentFacts,
    IdFactory, LimitsBundle, LimitsReader, LiveEntry, LiveEntryKind, LiveListQuery, LiveListing,
    LiveWorkspaceReader, ModelQualifier, PageBudget, PortError, ProviderCredentialBinding,
    ProviderCredentialReader, QualificationRefusal, QualifiedModel, RegistryReader, SessionReader,
    SessionSnapshot, VersionedOperation,
};
pub use projection::{
    canonical_session_bytes, public_session, public_session_list_item, public_status,
};
pub use use_cases::{
    CommitTerminal, LiveRead, Purge, RECOVERY_WINDOW, Rebind, Resume, STOP_BATCH_AGENTS,
    SendMessage, SessionCommand, StartRun, admit_message, commit_terminal, continue_operation,
    continue_stop, list_live_files, purge_session, rebind_credentials, restore_session, start_run,
    stat_live_file, stop_session, trash_session,
};
