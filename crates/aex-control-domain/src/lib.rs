//! `aex-control-domain` owns the pure control-plane state: organizations,
//! memberships, invitations, workspaces, API key metadata, durable operations,
//! revocation epochs, audit and outbox records, and the one authorization
//! decision every central route runs through.
//!
//! # Invariants
//!
//! - one scope vocabulary and one role enum serve workspace keys, account tokens
//!   and route requirements alike; there is no second spelling of either
//! - every transition is a total function of the prior state and the command and
//!   returns a typed error rather than a mutated value
//! - an [`epoch::Epoch`] has no decrement constructor
//! - a workspace's region and organization are immutable once created
//! - a terminal operation never re-enters a non-terminal status, and a stale
//!   fence is rejected rather than merged
//!
//! # Not this crate's job
//!
//! - storage, SQL or transactions (`aex-control-aurora`)
//! - HTTP, headers or status codes (`aex-central-http`)
//! - reading a clock, an RNG or the environment: all three arrive as parameters

pub mod account;
pub mod api_key;
pub mod audit;
pub mod authz;
pub mod codec;
pub mod cursor;
pub mod epoch;
pub mod intent;
pub mod invitation;
pub mod membership;
pub mod operation;
pub mod organization;
pub mod outbox;
pub mod scope;
pub mod slug;
pub mod workspace;

pub use account::{
    AccountPauseCause, AccountProfile, AccountProjectionError, account_operational_state,
};
pub use api_key::{ApiKey, ApiKeyTransition};
pub use audit::{ActorKind, AuditEvent, AuditOutcome, ResourceKind};
pub use authz::{
    AccountState, Action, ActorCredential, Admission, Denial, Granted, NotCentral, OrgMembership,
    OrgRole, Principal, PrincipalKindTag, PrincipalKinds, Requirement, Resource, ResourceClass,
    admit, decide, requirement,
};
pub use codec::{base64url, unbase64url};
pub use cursor::{CursorClaims, CursorError, CursorSecret, decode_cursor, encode_cursor};
pub use epoch::{Epoch, EpochSubjectKind};
pub use intent::{
    IdempotencyIdentity, IdempotencyKeyKind, IntentError, IntentHash, ScopeKind,
    canonical_intent_hash,
};
pub use invitation::{
    Invitation, InvitationStatus, InvitationTransition, MAX_ACCEPTABLE_INVITATIONS,
};
pub use membership::{Membership, MembershipStatus, MembershipTransition};
pub use operation::{
    Fence, Lease, LeaseOwner, Operation, OperationKind, OperationStatus, OperationTransition,
    OperationVisibility,
};
pub use organization::{Organization, OrganizationStatus};
pub use outbox::{OutboxMessage, Topic};
pub use scope::{Scope, ScopeError, ScopeSet};
pub use slug::{Slug, SlugError};
pub use workspace::{Workspace, WorkspaceStatus, WorkspaceTransition};

/// Why a revision could not be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a revision starts at 1; 0 can only mean `never written`")]
pub struct RevisionError;

/// The optimistic-concurrency revision every mutable row carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Revision(u64);

impl Revision {
    /// The revision a freshly created row carries.
    pub const INITIAL: Self = Self(1);

    /// Wraps a stored revision.
    ///
    /// # Errors
    ///
    /// Returns [`RevisionError`] when the value is zero.
    pub const fn new(value: u64) -> Result<Self, RevisionError> {
        if value == 0 {
            Err(RevisionError)
        } else {
            Ok(Self(value))
        }
    }

    /// The stored value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// The next revision.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}
