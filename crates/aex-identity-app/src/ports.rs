//! The coarse ports the identity application depends on.
//!
//! Coarse means one method equals one atomic unit of work. The alternative — a
//! `Transaction` handle crossing the boundary — would put the transaction shape
//! in the application and leave the adapter unable to make a ceremony atomic
//! without the caller's cooperation. Multi-commit orchestration is therefore
//! explicit here, where its crash windows are reasoned about.

use async_trait::async_trait;
use time::OffsetDateTime;
use uuid::Uuid;

use aex_identity_domain::{
    DashboardSession, EmailChallenge, ExternalIdentity, MintedSecret, NormalizedEmail, Pepper,
    PepperVersion, PresentedDigest, Provider, ProviderAccountId, User, Verifier,
};

/// The request this work belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequestId(String);

impl RequestId {
    /// Wraps a request identifier.
    #[must_use]
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    /// The identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Everything a use case knows about its caller.
#[derive(Debug, Clone)]
pub struct RequestContext {
    /// Which request.
    pub request_id: RequestId,
    /// The instant the whole request is evaluated against, read once at the
    /// edge so two checks inside one ceremony cannot straddle a boundary.
    pub now: OffsetDateTime,
}

/// What the store reports when it could not act.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// A uniqueness or state guard refused the write.
    #[error("conflict on `{constraint}`")]
    Conflict {
        /// The exact database constraint name, so mapping is deterministic.
        constraint: String,
    },
    /// The row does not exist.
    #[error("not found")]
    NotFound,
    /// The store could not be reached. Nothing was applied.
    #[error("the store is unavailable")]
    Unavailable,
    /// The store could not be reached and the outcome is not known.
    #[error("the store outcome is unknown")]
    Unknown,
    /// A row did not decode.
    #[error("decode failure: {0}")]
    Decode(String),
    /// The connected role lacks the privilege.
    #[error("permission denied")]
    PermissionDenied,
    /// Anything unrecoverable.
    #[error("fatal: {0}")]
    Fatal(String),
}

impl StoreError {
    /// Whether the caller may retry the same identity.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        matches!(self, Self::Unavailable | Self::Unknown)
    }
}

/// What a ceremony must be reconciled by after a lost commit.
///
/// The identity is preassigned *before* the write, so a reconciler can ask "did
/// this exact ceremony land?" rather than "did something like it land?".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileIdentity {
    /// Which ceremony.
    pub ceremony: &'static str,
    /// The row id the ceremony preassigned.
    pub id: Uuid,
}

/// A commit whose outcome the store could not establish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownCommit {
    /// What to reconcile by.
    pub identity: ReconcileIdentity,
}

/// The outcome of one atomic unit of work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxOutcome<T> {
    /// It was applied by this call.
    Committed(T),
    /// It had already been applied; this is the recorded result.
    Replayed(T),
    /// The same identity was reused with a different intent.
    IntentConflict,
    /// The outcome is not known.
    Unknown(UnknownCommit),
}

impl<T> TxOutcome<T> {
    /// The value, when the work is known to have landed.
    #[must_use]
    pub fn value(self) -> Option<T> {
        match self {
            Self::Committed(value) | Self::Replayed(value) => Some(value),
            Self::IntentConflict | Self::Unknown(_) => None,
        }
    }

    /// Whether this call was the one that applied the work.
    #[must_use]
    pub const fn is_first(&self) -> bool {
        matches!(self, Self::Committed(_))
    }
}

/// Which pepper a caller wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PepperPurpose {
    /// Identity credentials: browser sessions and email challenges.
    Identity,
    /// Workspace API keys minted by the central control authority.
    ApiKey,
    /// Keyset cursors.
    Cursor,
}

impl PepperPurpose {
    /// The database spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::ApiKey => "api_key",
            Self::Cursor => "cursor",
        }
    }
}

/// The clock, injected so no domain or application code reads one.
pub trait Clock: Send + Sync {
    /// The current instant, truncated to whole milliseconds.
    fn now(&self) -> OffsetDateTime;
}

/// The identifier factory, injected for the same reason.
pub trait IdFactory: Send + Sync {
    /// A fresh time-ordered identifier.
    fn next(&self) -> Uuid;
}

/// Where the peppers live.
#[async_trait]
pub trait PepperKeystore: Send + Sync {
    /// The one pepper new credentials are minted under.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the keystore cannot be reached or no pepper
    /// is active for that purpose.
    async fn active(&self, purpose: PepperPurpose) -> Result<(PepperVersion, Pepper), StoreError>;

    /// The pepper a stored verifier was computed under.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the version is unknown, which is a hard
    /// failure: the credential is *unverifiable*, not invalid, and answering
    /// "invalid" would be a lie the caller would act on.
    async fn by_version(
        &self,
        purpose: PepperPurpose,
        version: PepperVersion,
    ) -> Result<Pepper, StoreError>;
}

/// Link-or-create a person from an OAuth callback, in one ceremony.
#[derive(Debug, Clone)]
pub struct ResolveExternalIdentity {
    /// The user id to use when the person is new.
    pub preassigned_user_id: Uuid,
    /// The link id to use when the link is new.
    pub preassigned_link_id: Uuid,
    /// Which provider.
    pub provider: Provider,
    /// The provider's identifier.
    pub provider_account_id: ProviderAccountId,
    /// The address the provider asserts.
    pub email: NormalizedEmail,
    /// Whether the provider asserts the address is verified.
    pub email_verified: bool,
    /// A display name, when the provider supplied one.
    pub name: Option<String>,
    /// An avatar URL, when the provider supplied one.
    pub image_url: Option<String>,
    /// When the ceremony ran.
    pub now: OffsetDateTime,
}

/// The person a ceremony resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedUser {
    /// The person.
    pub user: User,
    /// Whether this call created them.
    pub created: bool,
    /// Their provider links, after the ceremony.
    pub links: Vec<ExternalIdentity>,
}

/// Issue a single-use email sign-in link.
#[derive(Debug, Clone)]
pub struct IssueEmailChallengeCommand {
    /// The challenge id, which is also the credential lookup key.
    pub preassigned_id: Uuid,
    /// Which address.
    pub email: NormalizedEmail,
    /// The keyed verifier.
    pub verifier: Verifier,
    /// Which pepper it was computed under.
    pub pepper_version: PepperVersion,
    /// When it was issued.
    pub issued_at: OffsetDateTime,
    /// When it lapses.
    pub expires_at: OffsetDateTime,
}

/// Redeem an email sign-in link.
#[derive(Debug, Clone)]
pub struct ConsumeEmailChallengeCommand {
    /// Which challenge.
    pub challenge_id: Uuid,
    /// What the caller presented.
    pub digest: PresentedDigest,
    /// The user id to use when the person is new.
    pub preassigned_user_id: Uuid,
    /// When the ceremony ran.
    pub now: OffsetDateTime,
}

/// Open a browser session.
#[derive(Debug, Clone)]
pub struct CreateDashboardSessionCommand {
    /// The session id, which is also the credential lookup key.
    pub preassigned_id: Uuid,
    /// Whose session.
    pub user_id: Uuid,
    /// The keyed verifier.
    pub verifier: Verifier,
    /// Which pepper it was computed under.
    pub pepper_version: PepperVersion,
    /// When it was minted.
    pub issued_at: OffsetDateTime,
    /// When it lapses.
    pub expires_at: OffsetDateTime,
}

/// Resolve a presented browser session.
#[derive(Debug, Clone)]
pub struct ResolveDashboardSessionQuery {
    /// Which session.
    pub session_id: Uuid,
    /// What the caller presented.
    pub digest: PresentedDigest,
    /// The instant liveness is evaluated at.
    pub now: OffsetDateTime,
}

/// Set a person's status.
#[derive(Debug, Clone)]
pub struct SetUserStatusCommand {
    /// Which person.
    pub user_id: Uuid,
    /// Whether they may authenticate.
    pub enabled: bool,
    /// When the change happened.
    pub now: OffsetDateTime,
}

/// Remove one provider link.
#[derive(Debug, Clone)]
pub struct UnlinkExternalIdentityCommand {
    /// Whose link.
    pub user_id: Uuid,
    /// Which provider.
    pub provider: Provider,
    /// When the change happened.
    pub now: OffsetDateTime,
}

/// Close a browser session.
#[derive(Debug, Clone)]
pub struct RevokeDashboardSessionCommand {
    /// Which session.
    pub session_id: Uuid,
    /// When it was closed.
    pub now: OffsetDateTime,
}

/// The identity authority.
///
/// Every method is one atomic unit of work. Nothing here returns a handle the
/// caller has to remember to finish.
#[async_trait]
pub trait IdentityStore: Send + Sync {
    /// Links or creates a person from an OAuth callback, atomically.
    ///
    /// This fuses what an adapter-style identity library splits into
    /// `get_user_by_account`, `create_user` and `link_account`. Those three
    /// calls have a real interleaving that creates two people for one provider
    /// account; one ceremony has none.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport or privilege failure.
    async fn resolve_or_create_by_external_identity(
        &self,
        command: &ResolveExternalIdentity,
    ) -> Result<TxOutcome<ResolvedUser>, StoreError>;

    /// Records a single-use email sign-in link.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport or privilege failure.
    async fn issue_email_challenge(
        &self,
        command: &IssueEmailChallengeCommand,
    ) -> Result<TxOutcome<EmailChallenge>, StoreError>;

    /// Redeems a sign-in link and resolves the person, atomically.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] when no such challenge exists, and
    /// [`StoreError`] for a transport or privilege failure.
    async fn consume_email_challenge(
        &self,
        command: &ConsumeEmailChallengeCommand,
    ) -> Result<TxOutcome<ResolvedUser>, StoreError>;

    /// Opens a browser session for an active person.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport or privilege failure.
    async fn create_dashboard_session(
        &self,
        command: &CreateDashboardSessionCommand,
    ) -> Result<TxOutcome<DashboardSession>, StoreError>;

    /// Resolves a presented browser session. Performs no write.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport or privilege failure.
    async fn resolve_dashboard_session(
        &self,
        query: &ResolveDashboardSessionQuery,
    ) -> Result<Option<(DashboardSession, User)>, StoreError>;

    /// Closes a browser session.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport or privilege failure.
    async fn revoke_dashboard_session(
        &self,
        command: &RevokeDashboardSessionCommand,
    ) -> Result<TxOutcome<()>, StoreError>;

    /// Enables or disables a person, advancing the `user` epoch in the same
    /// transaction when the status actually changes.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport or privilege failure.
    async fn set_user_status(
        &self,
        command: &SetUserStatusCommand,
    ) -> Result<TxOutcome<User>, StoreError>;

    /// Removes one provider link.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport or privilege failure.
    async fn unlink_external_identity(
        &self,
        command: &UnlinkExternalIdentityCommand,
    ) -> Result<TxOutcome<()>, StoreError>;
}

/// What a use case returns alongside a freshly minted credential.
#[derive(Debug, Clone)]
pub struct Minted<T> {
    /// The recorded row.
    pub record: T,
    /// The plaintext, returned exactly once.
    pub secret: MintedSecret,
}
