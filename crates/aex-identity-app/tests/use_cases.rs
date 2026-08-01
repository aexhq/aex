//! Identity ceremonies against a scripted store.
//!
//! The load-bearing assertion is the unknown-outcome mapping: a lost commit
//! becomes a retryable error naming the *preassigned* identity, and the
//! application never retries under a fresh one. Retrying under a fresh identity
//! is how one lost response becomes two credentials.

use std::sync::Mutex;

use async_trait::async_trait;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use aex_control_domain::Revision;
use aex_identity_app::ports::{
    Clock, ConsumeDeviceAuthorizationCommand, ConsumeEmailChallengeCommand,
    CreateDashboardSessionCommand, CreateDeviceAuthorizationCommand,
    DecideDeviceAuthorizationCommand, DeviceConsumeOutcome, IdFactory, IdentityStore,
    IssueEmailChallengeCommand, PepperKeystore, PepperPurpose, ReconcileIdentity, RequestContext,
    RequestId, ResolveDashboardSessionQuery, ResolveExternalIdentity, ResolvedUser,
    RevokeAccountTokenCommand, RevokeDashboardSessionCommand, SetUserStatusCommand, StoreError,
    TxOutcome, UnknownCommit,
};
use aex_identity_app::use_cases::{
    ConsumeEmailLink, IdentityDeps, IdentityError, IssueEmailLink, OpenDashboardSession,
    ResolveActor, ceremony,
};
use aex_identity_domain::{
    CredentialKind, DASHBOARD_SESSION_TTL, DashboardSession, DeviceAuthorization, EmailChallenge,
    NormalizedEmail, Pepper, PepperVersion, PresentedDigest, SecretRng, User, UserStatus, mint,
    verifier,
};

const USER: u128 = 0x0192_3f2a_1c00_7000_8000_0000_0000_0001;
const SESSION: u128 = 0x0192_3f2a_1c00_7000_8000_0000_0000_0002;
const MINTED: u128 = 0x0192_3f2a_1c00_7000_8000_0000_0000_0003;

fn at() -> OffsetDateTime {
    OffsetDateTime::UNIX_EPOCH
}

fn context() -> RequestContext {
    RequestContext {
        request_id: RequestId::new("req-1"),
        now: at(),
    }
}

fn user(status: UserStatus) -> User {
    User {
        id: Uuid::from_u128(USER),
        email: NormalizedEmail::parse("a@b.test").expect("valid"),
        email_verified_at: Some(at()),
        name: None,
        image_url: None,
        status,
        revision: Revision::INITIAL,
        created_at: at(),
        updated_at: at(),
    }
}

fn session(revoked: bool, expires_at: OffsetDateTime) -> DashboardSession {
    DashboardSession {
        id: Uuid::from_u128(SESSION),
        user_id: Uuid::from_u128(USER),
        pepper_version: PepperVersion::new(1),
        issued_at: at(),
        expires_at,
        revoked_at: revoked.then(at),
    }
}

/// A generator whose bytes are all the same, so a token is reproducible.
struct Fixed(u8);

impl SecretRng for Fixed {
    fn fill(&self, out: &mut [u8]) {
        out.fill(self.0);
    }
}

/// A clock frozen at the fixture instant.
struct Frozen;

impl Clock for Frozen {
    fn now(&self) -> OffsetDateTime {
        at()
    }
}

/// An identifier factory that hands out one preassigned id, so the test can
/// assert that a lost commit reports *that* id.
struct Preassigned(Uuid);

impl IdFactory for Preassigned {
    fn next(&self) -> Uuid {
        self.0
    }
}

/// A keystore that either answers or refuses.
struct Keystore(bool);

#[async_trait]
impl PepperKeystore for Keystore {
    async fn active(&self, _purpose: PepperPurpose) -> Result<(PepperVersion, Pepper), StoreError> {
        if self.0 {
            Ok((PepperVersion::new(1), Pepper::new([9_u8; 32])))
        } else {
            Err(StoreError::Unavailable)
        }
    }

    async fn by_version(
        &self,
        _purpose: PepperPurpose,
        _version: PepperVersion,
    ) -> Result<Pepper, StoreError> {
        if self.0 {
            Ok(Pepper::new([9_u8; 32]))
        } else {
            Err(StoreError::Unavailable)
        }
    }
}

/// What a programmed resolution answers with. A dedicated enum rather than
/// `Option<Option<_>>`, so "nothing was programmed" and "the store found
/// nothing" cannot be confused at a glance.
#[derive(Debug, Clone)]
enum Resolution {
    Found(Box<(DashboardSession, User)>),
    Absent,
}

/// What the scripted store was told to answer.
#[derive(Debug, Default)]
struct Script {
    challenge: Option<Result<TxOutcome<EmailChallenge>, StoreError>>,
    consumed: Option<Result<TxOutcome<ResolvedUser>, StoreError>>,
    session: Option<Result<TxOutcome<DashboardSession>, StoreError>>,
    resolved: Option<Resolution>,
    calls: usize,
}

#[derive(Debug, Default)]
struct Store(Mutex<Script>);

impl Store {
    fn calls(&self) -> usize {
        self.0.lock().expect("script").calls
    }
}

const UNDRIVEN: &str = "this suite drives the email, session and actor paths only";

#[async_trait]
impl IdentityStore for Store {
    async fn resolve_or_create_by_external_identity(
        &self,
        _command: &ResolveExternalIdentity,
    ) -> Result<TxOutcome<ResolvedUser>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn issue_email_challenge(
        &self,
        _command: &IssueEmailChallengeCommand,
    ) -> Result<TxOutcome<EmailChallenge>, StoreError> {
        let mut script = self.0.lock().expect("script");
        script.calls += 1;
        script
            .challenge
            .take()
            .expect("a challenge answer was programmed")
    }

    async fn consume_email_challenge(
        &self,
        _command: &ConsumeEmailChallengeCommand,
    ) -> Result<TxOutcome<ResolvedUser>, StoreError> {
        let mut script = self.0.lock().expect("script");
        script.calls += 1;
        script
            .consumed
            .take()
            .expect("a consumption answer was programmed")
    }

    async fn create_dashboard_session(
        &self,
        _command: &CreateDashboardSessionCommand,
    ) -> Result<TxOutcome<DashboardSession>, StoreError> {
        let mut script = self.0.lock().expect("script");
        script.calls += 1;
        script
            .session
            .take()
            .expect("a session answer was programmed")
    }

    async fn resolve_dashboard_session(
        &self,
        _query: &ResolveDashboardSessionQuery,
    ) -> Result<Option<(DashboardSession, User)>, StoreError> {
        let mut script = self.0.lock().expect("script");
        script.calls += 1;
        match script
            .resolved
            .take()
            .expect("a resolution answer was programmed")
        {
            Resolution::Found(pair) => Ok(Some(*pair)),
            Resolution::Absent => Ok(None),
        }
    }

    async fn revoke_dashboard_session(
        &self,
        _command: &RevokeDashboardSessionCommand,
    ) -> Result<TxOutcome<()>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn set_user_status(
        &self,
        _command: &SetUserStatusCommand,
    ) -> Result<TxOutcome<User>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn unlink_external_identity(
        &self,
        _command: &aex_identity_app::ports::UnlinkExternalIdentityCommand,
    ) -> Result<TxOutcome<()>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn create_device_authorization(
        &self,
        _command: &CreateDeviceAuthorizationCommand,
    ) -> Result<TxOutcome<DeviceAuthorization>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn approve_device_authorization(
        &self,
        _command: &DecideDeviceAuthorizationCommand,
    ) -> Result<TxOutcome<DeviceAuthorization>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn deny_device_authorization(
        &self,
        _command: &DecideDeviceAuthorizationCommand,
    ) -> Result<TxOutcome<DeviceAuthorization>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn poll_device_authorization(
        &self,
        _device_id: Uuid,
        _digest: &PresentedDigest,
        _now: OffsetDateTime,
    ) -> Result<Option<DeviceAuthorization>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn consume_device_authorization(
        &self,
        _command: &ConsumeDeviceAuthorizationCommand,
    ) -> Result<TxOutcome<DeviceConsumeOutcome>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn revoke_account_token(
        &self,
        _command: &RevokeAccountTokenCommand,
    ) -> Result<TxOutcome<()>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }
}

fn deps<'a>(
    store: &'a Store,
    keystore: &'a Keystore,
    ids: &'a Preassigned,
    rng: &'a Fixed,
    clock: &'a Frozen,
) -> IdentityDeps<'a> {
    IdentityDeps {
        store,
        keystore,
        clock,
        ids,
        rng,
    }
}

fn challenge_row() -> EmailChallenge {
    EmailChallenge {
        id: Uuid::from_u128(MINTED),
        email: NormalizedEmail::parse("a@b.test").expect("valid"),
        pepper_version: PepperVersion::new(1),
        issued_at: at(),
        expires_at: at() + Duration::minutes(15),
        consumed_at: None,
    }
}

#[tokio::test]
async fn issuing_an_email_link_returns_the_plaintext_exactly_once() {
    let store = Store::default();
    store.0.lock().expect("script").challenge = Some(Ok(TxOutcome::Committed(challenge_row())));
    let (keystore, ids, rng, clock) = (
        Keystore(true),
        Preassigned(Uuid::from_u128(MINTED)),
        Fixed(3),
        Frozen,
    );
    let minted = IssueEmailLink::run(
        &deps(&store, &keystore, &ids, &rng, &clock),
        &context(),
        NormalizedEmail::parse("a@b.test").expect("valid"),
    )
    .await
    .expect("the link is issued");

    assert!(minted.secret.expose().starts_with("aex_ec_"));
    assert_eq!(minted.record.id, Uuid::from_u128(MINTED));
    // The stored verifier is over the digest of the exact minted token.
    let expected = verifier(
        &Pepper::new([9_u8; 32]),
        &PresentedDigest::of(minted.secret.expose()),
    );
    let (_, digest) = mint(
        CredentialKind::EmailChallenge,
        None,
        Uuid::from_u128(MINTED),
        &Fixed(3),
    );
    assert_eq!(expected, verifier(&Pepper::new([9_u8; 32]), &digest));
}

#[tokio::test]
async fn a_lost_commit_names_the_preassigned_identity_and_is_retryable() {
    let store = Store::default();
    store.0.lock().expect("script").challenge = Some(Ok(TxOutcome::Unknown(UnknownCommit {
        identity: ReconcileIdentity {
            ceremony: ceremony::ISSUE_EMAIL_LINK,
            id: Uuid::from_u128(MINTED),
        },
    })));
    let (keystore, ids, rng, clock) = (
        Keystore(true),
        Preassigned(Uuid::from_u128(MINTED)),
        Fixed(3),
        Frozen,
    );
    let error = IssueEmailLink::run(
        &deps(&store, &keystore, &ids, &rng, &clock),
        &context(),
        NormalizedEmail::parse("a@b.test").expect("valid"),
    )
    .await
    .expect_err("a lost commit is never a success");
    assert_eq!(
        error,
        IdentityError::CommitOutcomeUnknown {
            ceremony: ceremony::ISSUE_EMAIL_LINK,
            id: Uuid::from_u128(MINTED)
        }
    );
    assert!(error.retryable());
    assert_eq!(store.calls(), 1, "the application never retried by itself");
}

#[tokio::test]
async fn an_unloadable_pepper_is_retryable_and_never_reaches_the_store() {
    let store = Store::default();
    let (keystore, ids, rng, clock) = (
        Keystore(false),
        Preassigned(Uuid::from_u128(MINTED)),
        Fixed(3),
        Frozen,
    );
    let error = IssueEmailLink::run(
        &deps(&store, &keystore, &ids, &rng, &clock),
        &context(),
        NormalizedEmail::parse("a@b.test").expect("valid"),
    )
    .await
    .expect_err("a credential cannot be minted without a pepper");
    assert_eq!(error, IdentityError::PepperUnavailable);
    assert!(error.retryable());
    assert_eq!(store.calls(), 0);
}

#[tokio::test]
async fn an_unknown_email_link_is_unauthenticated_rather_than_not_found() {
    // "No such link" and "wrong secret" must be the same answer, or the failure
    // itself tells an attacker which challenge ids exist.
    let store = Store::default();
    store.0.lock().expect("script").consumed = Some(Err(StoreError::NotFound));
    let (keystore, ids, rng, clock) = (
        Keystore(true),
        Preassigned(Uuid::from_u128(USER)),
        Fixed(3),
        Frozen,
    );
    assert_eq!(
        ConsumeEmailLink::run(
            &deps(&store, &keystore, &ids, &rng, &clock),
            &context(),
            Uuid::from_u128(MINTED),
            PresentedDigest::of("aex_ec_whatever"),
        )
        .await,
        Err(IdentityError::Unauthenticated)
    );
}

#[tokio::test]
async fn a_disabled_person_cannot_open_a_session_and_the_store_is_never_touched() {
    let store = Store::default();
    let (keystore, ids, rng, clock) = (
        Keystore(true),
        Preassigned(Uuid::from_u128(SESSION)),
        Fixed(3),
        Frozen,
    );
    let error = OpenDashboardSession::run(
        &deps(&store, &keystore, &ids, &rng, &clock),
        &context(),
        &user(UserStatus::Disabled),
    )
    .await
    .expect_err("a disabled person may not sign in");
    assert_eq!(error, IdentityError::UserDisabled);
    assert_eq!(store.calls(), 0);
}

#[tokio::test]
async fn opening_a_session_caps_expiry_at_thirty_days() {
    let store = Store::default();
    store.0.lock().expect("script").session = Some(Ok(TxOutcome::Committed(session(
        false,
        at() + DASHBOARD_SESSION_TTL,
    ))));
    let (keystore, ids, rng, clock) = (
        Keystore(true),
        Preassigned(Uuid::from_u128(SESSION)),
        Fixed(3),
        Frozen,
    );
    let minted = OpenDashboardSession::run(
        &deps(&store, &keystore, &ids, &rng, &clock),
        &context(),
        &user(UserStatus::Active),
    )
    .await
    .expect("an active person may sign in");
    assert!(minted.secret.expose().starts_with("aex_ds_"));
    assert_eq!(minted.record.expires_at, at() + DASHBOARD_SESSION_TTL);
    assert_eq!(minted.record.expiry_cap(), at() + DASHBOARD_SESSION_TTL);
}

#[tokio::test]
async fn resolving_an_actor_distinguishes_every_unusable_state() {
    let table: [(Resolution, IdentityError); 4] = [
        (Resolution::Absent, IdentityError::Unauthenticated),
        (
            Resolution::Found(Box::new((
                session(true, at() + DASHBOARD_SESSION_TTL),
                user(UserStatus::Active),
            ))),
            IdentityError::Revoked,
        ),
        (
            Resolution::Found(Box::new((session(false, at()), user(UserStatus::Active)))),
            IdentityError::Expired,
        ),
        (
            Resolution::Found(Box::new((
                session(false, at() + DASHBOARD_SESSION_TTL),
                user(UserStatus::Disabled),
            ))),
            IdentityError::UserDisabled,
        ),
    ];

    for (answer, expected) in table {
        let store = Store::default();
        store.0.lock().expect("script").resolved = Some(answer);
        let (keystore, ids, rng, clock) = (
            Keystore(true),
            Preassigned(Uuid::from_u128(SESSION)),
            Fixed(3),
            Frozen,
        );
        assert_eq!(
            ResolveActor::run(
                &deps(&store, &keystore, &ids, &rng, &clock),
                &context(),
                Uuid::from_u128(SESSION),
                PresentedDigest::of("aex_ds_whatever"),
            )
            .await,
            Err(expected.clone()),
            "{expected:?}"
        );
    }
}

#[tokio::test]
async fn resolving_a_live_actor_returns_the_session_and_the_person() {
    let store = Store::default();
    store.0.lock().expect("script").resolved = Some(Resolution::Found(Box::new((
        session(false, at() + DASHBOARD_SESSION_TTL),
        user(UserStatus::Active),
    ))));
    let (keystore, ids, rng, clock) = (
        Keystore(true),
        Preassigned(Uuid::from_u128(SESSION)),
        Fixed(3),
        Frozen,
    );
    let (resolved_session, resolved_user) = ResolveActor::run(
        &deps(&store, &keystore, &ids, &rng, &clock),
        &context(),
        Uuid::from_u128(SESSION),
        PresentedDigest::of("aex_ds_whatever"),
    )
    .await
    .expect("a live session resolves");
    assert_eq!(resolved_session.id, Uuid::from_u128(SESSION));
    assert_eq!(resolved_user.id, Uuid::from_u128(USER));
}

#[test]
fn every_store_failure_maps_to_a_typed_identity_error() {
    let table = [
        (
            StoreError::Conflict {
                constraint: "user_email_uk".to_owned(),
            },
            IdentityError::Conflict {
                constraint: "user_email_uk".to_owned(),
            },
        ),
        (StoreError::NotFound, IdentityError::NotFound),
        (StoreError::Unavailable, IdentityError::Unavailable),
    ];
    for (store_error, expected) in table {
        assert_eq!(IdentityError::from(store_error.clone()), expected);
    }
    assert!(IdentityError::from(StoreError::Unavailable).retryable());
    assert!(
        !IdentityError::from(StoreError::PermissionDenied).retryable(),
        "a privilege failure is never retried"
    );
}
