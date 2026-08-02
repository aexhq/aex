//! `aex-control-app` orchestrates the control-plane commands over coarse ports.
//!
//! # Invariants
//!
//! - authorization is decided **before** idempotency replay, so a denied
//!   principal never sees a stored response
//! - a cross-region effect is prepared, fenced and idempotent; an unknown
//!   outcome leaves the workspace hidden and retryable and never returns a
//!   different workspace or a false `201`
//! - [`ports::EffectError::Unknown`] is never converted into failure; it is the
//!   only correct answer to a lost response and it is a distinct arm from
//!   `Unavailable`
//! - an outbox row is committed in the same transaction as the aggregate it
//!   describes
//!
//! # Not this crate's job
//!
//! - SQL, transactions or retry policy (`aex-control-aurora`)
//! - HTTP, status codes or headers (`aex-central-http`)

pub mod ports;
pub mod use_cases;

pub use ports::{
    AuthorizationReader, ControlStore, ControlViewStore, EffectError, MailerPort, Page,
    RegionalControlPort, RequestContext, StoreError, TxOutcome, UnknownCommit,
};
pub use use_cases::{
    AcceptInvitations, CancelOperation, ControlError, CreateApiKey, CreateInvitation,
    CreateOrganization, CreateWorkspace, CreatedApiKey, DeleteWorkspace, RevokeApiKey,
};
