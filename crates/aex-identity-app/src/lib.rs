//! `aex-identity-app` orchestrates the identity ceremonies over coarse ports.
//!
//! # Invariants
//!
//! - one port method equals one atomic unit of work, so the adapter owns the
//!   transaction and no unit-of-work handle crosses the port boundary
//! - a [`TxOutcome::Unknown`] becomes a typed retryable error carrying the
//!   preassigned identity; it is never retried under a *new* identity, because
//!   that is how one lost response becomes two credentials
//! - every use case validates with the domain, calls exactly one store method,
//!   and maps the result; there is no second decision point
//!
//! # Not this crate's job
//!
//! - SQL, transactions or retry policy (`aex-identity-aurora`)
//! - HTTP, status codes or headers (`aex-central-http`)
//! - reading a clock, an RNG or the environment: [`ports::Clock`],
//!   [`ports::IdFactory`] and `SecretRng` are injected

pub mod ports;
pub mod use_cases;

pub use ports::{
    Clock, IdFactory, IdentityStore, PepperKeystore, PepperPurpose, ReconcileIdentity,
    RequestContext, RequestId, StoreError, TxOutcome, UnknownCommit,
};
pub use use_cases::{
    CloseDashboardSession, ConsumeEmailLink, DisableUser, IdentityError, IssueEmailLink,
    OauthProfile, OpenDashboardSession, ResolveActor, ResolveOauthSignIn, UnlinkProvider,
};
