//! One use case per internal route.
//!
//! Each takes a [`RequestContext`], validates with the domain, calls exactly one
//! store method, and maps the outcome. The shape is deliberately boring: the
//! interesting decisions are in the domain and the adapter, and a use case that
//! made a second decision would be a second place for the two to disagree.

use time::OffsetDateTime;
use uuid::Uuid;

use aex_identity_domain::{
    CredentialKind, DASHBOARD_SESSION_TTL, DashboardSession, EMAIL_CHALLENGE_TTL, EmailChallenge,
    NormalizedEmail, PresentedDigest, Provider, ProviderAccountId, SecretRng, User, mint, verifier,
};

use crate::ports::{
    Clock, ConsumeEmailChallengeCommand, CreateDashboardSessionCommand, IdFactory, IdentityStore,
    Minted, PepperKeystore, PepperPurpose, ReconcileIdentity, RequestContext,
    ResolveDashboardSessionQuery, ResolveExternalIdentity, ResolvedUser,
    RevokeDashboardSessionCommand, SetUserStatusCommand, StoreError, TxOutcome,
    UnlinkExternalIdentityCommand,
};

/// Why an identity ceremony did not complete.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdentityError {
    /// The presented credential did not verify, or names nothing.
    #[error("the credential did not verify")]
    Unauthenticated,
    /// The credential has been revoked.
    #[error("the credential was revoked")]
    Revoked,
    /// The credential lapsed.
    #[error("the credential expired")]
    Expired,
    /// The person may not authenticate.
    #[error("the person is disabled")]
    UserDisabled,
    /// A uniqueness or state guard refused the write.
    #[error("conflict on `{constraint}`")]
    Conflict {
        /// The exact database constraint name.
        constraint: String,
    },
    /// Nothing matched.
    #[error("not found")]
    NotFound,
    /// The same identity was reused with a different intent.
    #[error("the identity was reused with a different intent")]
    IntentConflict,
    /// The store could not be reached; nothing was applied.
    #[error("the identity store is unavailable")]
    Unavailable,
    /// The commit outcome is unknown. **Retry the same identity.**
    #[error("the commit outcome is unknown; retry ceremony `{ceremony}` for `{id}`")]
    CommitOutcomeUnknown {
        /// Which ceremony to reconcile.
        ceremony: &'static str,
        /// The identity it preassigned.
        id: Uuid,
    },
    /// The credential's pepper version is unknown, so it is unverifiable rather
    /// than invalid.
    #[error("the credential's pepper version is not loadable")]
    PepperUnavailable,
    /// Anything unrecoverable.
    #[error("fatal: {0}")]
    Fatal(String),
}

impl IdentityError {
    /// Whether the caller may retry the same identity.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Unavailable | Self::CommitOutcomeUnknown { .. } | Self::PepperUnavailable
        )
    }
}

impl From<StoreError> for IdentityError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Conflict { constraint } => Self::Conflict { constraint },
            StoreError::NotFound => Self::NotFound,
            StoreError::Unavailable => Self::Unavailable,
            StoreError::Unknown => Self::Fatal("the store outcome is unknown".to_owned()),
            StoreError::Decode(reason) | StoreError::Fatal(reason) => Self::Fatal(reason),
            StoreError::PermissionDenied => Self::Fatal("permission denied".to_owned()),
        }
    }
}

/// Maps one atomic outcome onto the application's error vocabulary.
///
/// `Unknown` becomes a retryable error naming the preassigned identity, never a
/// silent retry: retrying under a fresh identity is exactly how one lost commit
/// response becomes two credentials.
fn settle<T>(outcome: TxOutcome<T>) -> Result<(T, bool), IdentityError> {
    match outcome {
        TxOutcome::Committed(value) => Ok((value, true)),
        TxOutcome::Replayed(value) => Ok((value, false)),
        TxOutcome::IntentConflict => Err(IdentityError::IntentConflict),
        TxOutcome::Unknown(unknown) => Err(IdentityError::CommitOutcomeUnknown {
            ceremony: unknown.identity.ceremony,
            id: unknown.identity.id,
        }),
    }
}

/// The named ceremonies, for reconciliation after a lost commit.
pub mod ceremony {
    /// `POST /internal/v1/identity/users/resolutions`.
    pub const RESOLVE_OAUTH_SIGN_IN: &str = "identity.resolve_oauth_sign_in";
    /// `POST /internal/v1/identity/email-challenges`.
    pub const ISSUE_EMAIL_LINK: &str = "identity.issue_email_link";
    /// `POST /internal/v1/identity/email-challenges/consumptions`.
    pub const CONSUME_EMAIL_LINK: &str = "identity.consume_email_link";
    /// `POST /internal/v1/identity/sessions`.
    pub const OPEN_DASHBOARD_SESSION: &str = "identity.open_dashboard_session";
}

/// The dependencies every identity use case shares.
pub struct IdentityDeps<'a> {
    /// The authority.
    pub store: &'a dyn IdentityStore,
    /// The peppers.
    pub keystore: &'a dyn PepperKeystore,
    /// The clock.
    pub clock: &'a dyn Clock,
    /// The identifier factory.
    pub ids: &'a dyn IdFactory,
    /// The random source.
    pub rng: &'a dyn SecretRng,
}

/// What an OAuth callback asserted about a person.
#[derive(Debug, Clone)]
pub struct OauthProfile {
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
}

/// Link-or-create a person from an OAuth callback.
pub struct ResolveOauthSignIn;

impl ResolveOauthSignIn {
    /// Runs the ceremony.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError`] naming the failure; a lost commit is
    /// [`IdentityError::CommitOutcomeUnknown`] carrying the preassigned user id.
    pub async fn run(
        deps: &IdentityDeps<'_>,
        context: &RequestContext,
        profile: OauthProfile,
    ) -> Result<ResolvedUser, IdentityError> {
        let command = ResolveExternalIdentity {
            preassigned_user_id: deps.ids.next(),
            preassigned_link_id: deps.ids.next(),
            provider: profile.provider,
            provider_account_id: profile.provider_account_id,
            email: profile.email,
            email_verified: profile.email_verified,
            name: profile.name,
            image_url: profile.image_url,
            now: context.now,
        };
        let outcome = deps
            .store
            .resolve_or_create_by_external_identity(&command)
            .await?;
        settle(outcome).map(|(resolved, _)| resolved)
    }
}

/// Mint and record a single-use email sign-in link.
pub struct IssueEmailLink;

impl IssueEmailLink {
    /// Runs the ceremony, returning the plaintext exactly once.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError`] naming the failure.
    pub async fn run(
        deps: &IdentityDeps<'_>,
        context: &RequestContext,
        email: NormalizedEmail,
    ) -> Result<Minted<EmailChallenge>, IdentityError> {
        let id = deps.ids.next();
        let (version, pepper) = deps
            .keystore
            .active(PepperPurpose::Identity)
            .await
            .map_err(|_| IdentityError::PepperUnavailable)?;
        let (secret, digest) = mint(CredentialKind::EmailChallenge, None, id, deps.rng);
        let command = crate::ports::IssueEmailChallengeCommand {
            preassigned_id: id,
            email,
            verifier: verifier(&pepper, &digest),
            pepper_version: version,
            issued_at: context.now,
            expires_at: context.now + EMAIL_CHALLENGE_TTL,
        };
        let outcome = deps.store.issue_email_challenge(&command).await?;
        let (record, _) = settle(outcome)?;
        Ok(Minted { record, secret })
    }
}

/// Redeem an email sign-in link.
pub struct ConsumeEmailLink;

impl ConsumeEmailLink {
    /// Runs the ceremony.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::Unauthenticated`] when the link does not verify,
    /// which is deliberately the same answer as "no such link".
    pub async fn run(
        deps: &IdentityDeps<'_>,
        context: &RequestContext,
        challenge_id: Uuid,
        digest: PresentedDigest,
    ) -> Result<ResolvedUser, IdentityError> {
        let command = ConsumeEmailChallengeCommand {
            challenge_id,
            digest,
            preassigned_user_id: deps.ids.next(),
            now: context.now,
        };
        match deps.store.consume_email_challenge(&command).await {
            Ok(outcome) => settle(outcome).map(|(resolved, _)| resolved),
            Err(StoreError::NotFound) => Err(IdentityError::Unauthenticated),
            Err(error) => Err(error.into()),
        }
    }
}

/// Open a browser session.
pub struct OpenDashboardSession;

impl OpenDashboardSession {
    /// Runs the ceremony, returning the plaintext exactly once.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::UserDisabled`] for a person who may not
    /// authenticate, plus every store failure.
    pub async fn run(
        deps: &IdentityDeps<'_>,
        context: &RequestContext,
        user: &User,
    ) -> Result<Minted<DashboardSession>, IdentityError> {
        if !user.may_authenticate() {
            return Err(IdentityError::UserDisabled);
        }
        let id = deps.ids.next();
        let (version, pepper) = deps
            .keystore
            .active(PepperPurpose::Identity)
            .await
            .map_err(|_| IdentityError::PepperUnavailable)?;
        let (secret, digest) = mint(CredentialKind::DashboardSession, None, id, deps.rng);
        let command = CreateDashboardSessionCommand {
            preassigned_id: id,
            user_id: user.id,
            verifier: verifier(&pepper, &digest),
            pepper_version: version,
            issued_at: context.now,
            expires_at: context.now + DASHBOARD_SESSION_TTL,
        };
        let outcome = deps.store.create_dashboard_session(&command).await?;
        let (record, _) = settle(outcome)?;
        Ok(Minted { record, secret })
    }
}

/// Resolve a presented browser session into a person.
pub struct ResolveActor;

impl ResolveActor {
    /// Runs the read. Performs no write at all, which the adapter proves by
    /// running it as a role with no write privilege.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::Unauthenticated`] for an unknown or
    /// non-verifying session, [`IdentityError::Revoked`] and
    /// [`IdentityError::Expired`] for a resolved but unusable one, and
    /// [`IdentityError::UserDisabled`] when the person may not authenticate.
    pub async fn run(
        deps: &IdentityDeps<'_>,
        context: &RequestContext,
        session_id: Uuid,
        digest: PresentedDigest,
    ) -> Result<(DashboardSession, User), IdentityError> {
        let query = ResolveDashboardSessionQuery {
            session_id,
            digest,
            now: context.now,
        };
        let Some((session, user)) = deps.store.resolve_dashboard_session(&query).await? else {
            return Err(IdentityError::Unauthenticated);
        };
        match session.state_at(context.now) {
            aex_identity_domain::SessionState::Revoked => return Err(IdentityError::Revoked),
            aex_identity_domain::SessionState::Expired => return Err(IdentityError::Expired),
            aex_identity_domain::SessionState::Active => {}
        }
        if !user.may_authenticate() {
            return Err(IdentityError::UserDisabled);
        }
        Ok((session, user))
    }
}

/// Close a browser session.
pub struct CloseDashboardSession;

impl CloseDashboardSession {
    /// Runs the ceremony.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError`] naming the failure.
    pub async fn run(
        deps: &IdentityDeps<'_>,
        context: &RequestContext,
        session_id: Uuid,
    ) -> Result<(), IdentityError> {
        let command = RevokeDashboardSessionCommand {
            session_id,
            now: context.now,
        };
        let outcome = deps.store.revoke_dashboard_session(&command).await?;
        settle(outcome).map(|((), _)| ())
    }
}

/// Enable or disable a person.
pub struct DisableUser;

impl DisableUser {
    /// Runs the ceremony.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError`] naming the failure.
    pub async fn run(
        deps: &IdentityDeps<'_>,
        context: &RequestContext,
        user_id: Uuid,
        enabled: bool,
    ) -> Result<User, IdentityError> {
        let command = SetUserStatusCommand {
            user_id,
            enabled,
            now: context.now,
        };
        let outcome = deps.store.set_user_status(&command).await?;
        settle(outcome).map(|(user, _)| user)
    }
}

/// Remove one provider link.
pub struct UnlinkProvider;

impl UnlinkProvider {
    /// Runs the ceremony.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError`] naming the failure; the store refuses the
    /// unlink when it would leave the person with no way to sign in.
    pub async fn run(
        deps: &IdentityDeps<'_>,
        context: &RequestContext,
        user_id: Uuid,
        provider: Provider,
    ) -> Result<(), IdentityError> {
        let command = UnlinkExternalIdentityCommand {
            user_id,
            provider,
            now: context.now,
        };
        let outcome = deps.store.unlink_external_identity(&command).await?;
        settle(outcome).map(|((), _)| ())
    }
}

/// Builds the reconciliation identity for a ceremony.
#[must_use]
pub fn reconcile_identity(ceremony: &'static str, id: Uuid) -> ReconcileIdentity {
    ReconcileIdentity { ceremony, id }
}

/// The instant a request is evaluated against, truncated to whole milliseconds.
#[must_use]
pub fn truncate_millis(instant: OffsetDateTime) -> OffsetDateTime {
    let millis = instant.unix_timestamp_nanos().div_euclid(1_000_000);
    OffsetDateTime::from_unix_timestamp_nanos(millis * 1_000_000).unwrap_or(instant)
}
