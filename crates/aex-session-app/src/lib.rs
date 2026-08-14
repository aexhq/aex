//! `aex-session-app` owns transactional session admission, current lifecycle
//! planning, live-file reads and the authority ports.
//!
//! # Invariants
//!
//! - a use case returns exactly one [`plan::SessionTransaction`] and never
//!   commits: [`ports::AppContext`] holds no committer, so it cannot;
//! - every port call happens **before** the plan is built, so a plan is a pure
//!   function of what was read;
//! - an admission either writes every participant of its declared transaction or
//!   none;
//! - replayable admissions strongly read their receipt before mutable planning
//!   dependencies, so an exact retry remains state-independent;
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
    PreparedSessionCreate, RequestedSessionLaunch, ResolvedInitialFile, RootStartedEvidence,
    initial_root_record, prepare_session_create, publish_requested_session,
    replay_session_create_receipt,
};
pub use error::AppError;
pub use lifecycle::{
    LifecycleAdmissionOutcome, LifecycleCommand, LifecycleWorkClaim, admit_lifecycle_operation,
    settle_lifecycle_loss, settle_lifecycle_operation, settle_sandbox_preparation,
    settle_session_delete,
};
pub use outcome::{Attempted, Observed, ProviderAnswer, Resolution, resolve};
pub use plan::{
    AgentWake, Condition, ConditionId, Hint, ItemKey, MAX_ACTIONS, MAX_BYTES, MessageAdmittedEvent,
    PlanError, PlanShape, Planned, SessionTransaction, TableFamily, TransactionIntent, Write,
};
pub use ports::{
    AccountStateReader, AgentCancelPage, AgentCancelTarget, AgentPage, AppContext,
    AuthorityCommitter, Clock, CommitError, CommitOutcome, CredentialState, DeploymentFacts,
    IdFactory, LimitsBundle, LimitsReader, LiveEntry, LiveEntryKind, LiveListQuery, LiveListing,
    LiveWorkspaceReader, MessageAdmissionSnapshot, ModelQualifier, PageBudget, PortError,
    ProviderCredentialBinding, ProviderCredentialReader, QualificationRefusal, QualifiedModel,
    RegistryReader, RootAdmissionState, SessionReader, VersionedOperation,
};
pub use projection::{
    canonical_session_bytes, public_session, public_session_list_item, public_status,
};
pub use use_cases::{
    CommitTerminal, LiveRead, MESSAGE_TEXT_MAX_BYTES, MessageAdmissionOutcome, SendMessage,
    StartRun, admit_message, commit_terminal, list_live_files, message_receipt_scope,
    replay_message_receipt, start_run, stat_live_file,
};
