//! `SC-IDENTITY-ACCOUNT` — sign in, approve a device, redeem a token.
//!
//! The complete account identity and operational-state flow, driven through the
//! real ceremony rather than around it. Until 2026-08-10 no scenario could
//! obtain a credential at all except through CI's raw-SQL bypass; this body
//! exists so that is never true again, and it deliberately writes **no**
//! `identity.device_authorization`, `identity.dashboard_session` or
//! `identity.account_token` row of its own.
//!
//! # What is real here
//!
//! The committed schema and privileges, `aex-identity-app`'s use cases,
//! `AuroraIdentityStore` and its statements, the real credential mint, the real
//! `HMAC` verifier and the real constant-time verify. The store connects as
//! `aex_identity_api`.
//!
//! # What is not
//!
//! The engine is `PostgreSQL` in a container rather than Aurora, so the
//! `aws.rds_data.transaction` seam is untouched and stays claimed by the live
//! companion. The pepper is a fixed value rather than Secrets Manager, which is
//! the `aws.secretsmanager` seam and likewise not claimed here.

mod support;

use std::sync::{Arc, Mutex};

use aex_control_domain::{Scope, ScopeSet};
use aex_identity_app::ports::{
    Clock, IdFactory, IdentityStore, PepperKeystore, PepperPurpose, RequestContext, RequestId,
    StoreError,
};
use aex_identity_app::use_cases::{
    ApproveDevice, DenyDevice, IdentityDeps, IdentityError, OauthProfile, OpenDashboardSession,
    PollDevice, ResolveActor, ResolveOauthSignIn, StartDeviceAuthorization,
};
use aex_identity_aurora::AuroraIdentityStore;
use aex_identity_domain::{
    CredentialKind, DeviceState, Pepper, PepperVersion, PresentedDigest, SecretRng, User, parse,
};
use sqlx::Executor as _;
use time::OffsetDateTime;
use uuid::Uuid;

/// The pepper version the seeded `identity.credential_pepper` row carries.
const PEPPER_VERSION: u16 = 1;

/// One pepper, for one version, for the whole case.
///
/// Fixed rather than rotated on purpose. `ApproveDevice` looks a grant up by the
/// user-code hash computed under the **currently active** pepper, so a rotation
/// between mint and decision loses the grant — a real property, and one a
/// separate case should assert rather than one this body should trip over.
#[derive(Debug)]
struct FixedPepper;

#[async_trait::async_trait]
impl PepperKeystore for FixedPepper {
    async fn active(&self, _purpose: PepperPurpose) -> Result<(PepperVersion, Pepper), StoreError> {
        Ok((PepperVersion::new(PEPPER_VERSION), Pepper::new([0x5a; 32])))
    }

    async fn by_version(
        &self,
        _purpose: PepperPurpose,
        version: PepperVersion,
    ) -> Result<Pepper, StoreError> {
        if version.get() != PEPPER_VERSION {
            return Err(StoreError::NotFound);
        }
        Ok(Pepper::new([0x5a; 32]))
    }
}

/// A clock the case controls.
struct FixedClock(OffsetDateTime);

impl Clock for FixedClock {
    fn now(&self) -> OffsetDateTime {
        self.0
    }
}

/// Time-ordered ids, as production mints them.
struct Uuid7Ids;

impl IdFactory for Uuid7Ids {
    fn next(&self) -> Uuid {
        Uuid::now_v7()
    }
}

/// A deterministic secret source; see `control_workspace.rs` for why.
struct CountingRng(Mutex<u8>);

impl SecretRng for CountingRng {
    fn fill(&self, out: &mut [u8]) {
        let mut seed = self.0.lock().expect("the counter is not poisoned");
        for byte in out.iter_mut() {
            *seed = seed.wrapping_add(1);
            *byte = *seed;
        }
    }
}

/// The pepper row the credential foreign keys require.
///
/// The only row this body seeds. Every person, session, grant and token below is
/// written by the ceremony under test.
async fn seed(plane: &support::CentralPlane) {
    let mut connection = plane.superuser().await;
    connection
        .execute(
            "INSERT INTO identity.credential_pepper \
               (version, purpose, state, secret_ref, created_at) \
             VALUES (1, 'identity', 'active', 'scenario', now());",
        )
        .await
        .expect("the identity pepper is seeded");
}

/// Everything a case needs to drive the ceremony.
struct World {
    store: AuroraIdentityStore,
    peppers: Arc<FixedPepper>,
    clock: FixedClock,
    ids: Uuid7Ids,
    rng: CountingRng,
    now: OffsetDateTime,
}

impl World {
    async fn start(plane: &support::CentralPlane) -> Self {
        seed(plane).await;
        let now =
            OffsetDateTime::from_unix_timestamp(1_767_225_600).expect("a representable instant");
        let peppers = Arc::new(FixedPepper);
        Self {
            store: AuroraIdentityStore::new(
                plane.client_as("aex_identity_api").await,
                peppers.clone(),
            ),
            peppers,
            clock: FixedClock(now),
            ids: Uuid7Ids,
            rng: CountingRng(Mutex::new(0)),
            now,
        }
    }

    fn deps(&self) -> IdentityDeps<'_> {
        IdentityDeps {
            store: &self.store as &dyn IdentityStore,
            keystore: self.peppers.as_ref(),
            clock: &self.clock,
            ids: &self.ids,
            rng: &self.rng,
        }
    }

    fn context(&self, request: &str) -> RequestContext {
        RequestContext {
            request_id: RequestId::new(request.to_owned()),
            now: self.now,
        }
    }

    /// Signs a person in and opens the dashboard session they approve from.
    async fn sign_in(&self, account: &str) -> (User, Uuid, String) {
        let resolved = ResolveOauthSignIn::run(
            &self.deps(),
            &self.context("sign-in"),
            OauthProfile {
                provider: aex_identity_domain::Provider::Github,
                provider_account_id: aex_identity_domain::ProviderAccountId::parse(account)
                    .expect("a printable account id"),
                email: aex_identity_domain::NormalizedEmail::parse(&format!(
                    "{account}@example.test"
                ))
                .expect("a valid address"),
                email_verified: true,
                name: Some("Scenario Person".to_owned()),
                image_url: None,
            },
        )
        .await
        .expect("the sign-in ceremony commits");

        let session =
            OpenDashboardSession::run(&self.deps(), &self.context("open-session"), &resolved.user)
                .await
                .expect("the dashboard session ceremony commits");

        (
            resolved.user,
            session.record.id,
            session.secret.expose().to_owned(),
        )
    }
}

#[tokio::test]
async fn a_dashboard_session_is_minted_as_a_real_credential_and_resolves_its_own_actor() {
    let plane = support::CentralPlane::start().await;
    let world = World::start(&plane).await;
    let (user, session_id, secret) = world.sign_in("founder").await;

    assert!(
        secret.starts_with("aex_ds_"),
        "a dashboard session credential carries its own prefix"
    );
    let parsed = parse(CredentialKind::DashboardSession, &secret)
        .expect("the minted credential parses as a dashboard session");
    assert_eq!(
        parsed.id, session_id,
        "the credential names the session row it belongs to"
    );

    // The resolve path is what every authenticated dashboard request runs: it
    // reads the row, fetches the pepper by the row's own version, and compares
    // in constant time.
    let (session, actor) = ResolveActor::run(
        &world.deps(),
        &world.context("resolve"),
        session_id,
        parsed.digest,
    )
    .await
    .expect("the session the ceremony minted resolves to its person");
    assert_eq!(session.id, session_id);
    assert_eq!(actor.id, user.id);
    assert!(
        actor.may_authenticate(),
        "a freshly signed-in person may authenticate"
    );
}

#[tokio::test]
async fn a_device_grant_approved_by_its_person_redeems_exactly_one_account_token() {
    // The complete CLI sign-in. Its failure means there is no way to obtain an
    // account token that does not go through CI's raw-SQL bypass.
    let plane = support::CentralPlane::start().await;
    let world = World::start(&plane).await;
    let (user, session_id, _secret) = world.sign_in("founder").await;

    let started = StartDeviceAuthorization::run(
        &world.deps(),
        &world.context("device-start"),
        ScopeSet::of(&[Scope::SessionsRead]),
    )
    .await
    .expect("the device authorization ceremony commits");
    assert!(
        started.device_code.expose().starts_with("aex_dc_"),
        "a device code carries its own prefix"
    );

    let approved = ApproveDevice::run(
        &world.deps(),
        &world.context("device-approve"),
        &started.user_code,
        user.id,
        session_id,
    )
    .await
    .expect("the person approves their own device");
    assert_eq!(approved.id, started.grant.id);
    assert_eq!(approved.approved_by, Some(user.id));

    let digest = parse(CredentialKind::DeviceCode, started.device_code.expose())
        .expect("the device code parses")
        .digest;
    let redeemed = PollDevice::run(
        &world.deps(),
        &world.context("device-poll"),
        started.grant.id,
        digest,
    )
    .await
    .expect("an approved grant redeems an account token");

    assert!(
        redeemed.secret.expose().starts_with("aex_at_"),
        "redemption mints an account token, not another device code"
    );
    assert_eq!(redeemed.record.token.user_id, user.id);
    assert_eq!(
        redeemed.record.token.scopes,
        ScopeSet::of(&[Scope::SessionsRead]),
        "the token carries the scopes the grant requested, never more"
    );
    assert_eq!(
        redeemed.record.grant.account_token_id,
        Some(redeemed.record.token.id),
        "the consumed grant names the one token it minted"
    );
}

#[tokio::test]
async fn a_redeemed_grant_cannot_be_redeemed_a_second_time_into_a_second_token() {
    let plane = support::CentralPlane::start().await;
    let world = World::start(&plane).await;
    let (user, session_id, _secret) = world.sign_in("founder").await;

    let started = StartDeviceAuthorization::run(
        &world.deps(),
        &world.context("device-start"),
        ScopeSet::of(&[Scope::SessionsRead]),
    )
    .await
    .expect("the device authorization ceremony commits");
    ApproveDevice::run(
        &world.deps(),
        &world.context("device-approve"),
        &started.user_code,
        user.id,
        session_id,
    )
    .await
    .expect("the person approves their own device");

    let digest = parse(CredentialKind::DeviceCode, started.device_code.expose())
        .expect("the device code parses")
        .digest;
    PollDevice::run(
        &world.deps(),
        &world.context("poll-1"),
        started.grant.id,
        digest,
    )
    .await
    .expect("the first redemption mints a token");
    let second = PollDevice::run(
        &world.deps(),
        &world.context("poll-2"),
        started.grant.id,
        PresentedDigest::from_bytes(*digest.as_bytes()),
    )
    .await;

    assert!(
        matches!(second, Err(IdentityError::Unauthenticated)),
        "a consumed one-time grant must not return another credential: {second:?}"
    );
    let mut connection = plane.superuser().await;
    let tokens = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM identity.account_token")
        .fetch_one(&mut connection)
        .await
        .expect("the account-token table is readable");
    assert_eq!(
        tokens, 1,
        "a second redemption must not mint a second token"
    );
}

#[tokio::test]
async fn a_device_decision_needs_the_live_session_the_actor_claims_to_hold() {
    // Tenant isolation on the identity plane, which the priority order counts as
    // correctness rather than security: a refusal ends somebody else's sign-in.
    //
    // `APPROVE_DEVICE_AUTHORIZATION` made the actor's currency part of the
    // predicate from the start, with an `EXISTS` over a live session belonging
    // to an active person. Until 2026-08-10 `DENY_DEVICE_AUTHORIZATION` had no
    // such clause: `decide_device` bound `:actor_user_id` and
    // `:actor_session_id` for both paths and deny referenced neither, so a
    // caller holding no session at all could refuse a grant they had nothing
    // but the user code for.
    //
    // What the ceremony deliberately does **not** promise, and what this case
    // therefore does not assert: a pending grant has no owner to compare an
    // actor against, so any current actor holding the user code may refuse it,
    // exactly as any current actor may approve it and bind their own account to
    // the device. The user code is the capability — RFC 8628 — and its entropy
    // and its fifteen minutes are what protect it. The predicate's job is to
    // establish that there is a current actor at all, and that the session
    // presented belongs to the person presenting it.
    let plane = support::CentralPlane::start().await;
    let world = World::start(&plane).await;
    let (owner, owner_session, _owner_secret) = world.sign_in("founder").await;
    let (stranger, _stranger_session, _stranger_secret) = world.sign_in("stranger").await;
    assert_ne!(owner.id, stranger.id);

    let started = StartDeviceAuthorization::run(
        &world.deps(),
        &world.context("device-start"),
        ScopeSet::of(&[Scope::SessionsRead]),
    )
    .await
    .expect("the device authorization ceremony commits");

    // A live session, but somebody else's. `s.user_id = :actor_user_id` is the
    // conjunct that refuses it; without that pairing the `EXISTS` would admit
    // any session that happened to be current.
    let borrowed = DenyDevice::run(
        &world.deps(),
        &world.context("deny-borrowed-session"),
        &started.user_code,
        stranger.id,
        owner_session,
    )
    .await;
    assert!(
        matches!(
            borrowed,
            Err(IdentityError::NotFound | IdentityError::AccessDenied)
        ),
        "a session belonging to another person must not authorize a decision, \
         but the deny statement answered {borrowed:?}"
    );

    // And a caller holding no session whatsoever, which is what the statement
    // admitted before it carried the clause at all.
    let sessionless = DenyDevice::run(
        &world.deps(),
        &world.context("deny-without-a-session"),
        &started.user_code,
        stranger.id,
        Uuid::now_v7(),
    )
    .await;
    assert!(
        matches!(
            sessionless,
            Err(IdentityError::NotFound | IdentityError::AccessDenied)
        ),
        "a caller holding no browser session must not be able to refuse a grant, \
         but the deny statement answered {sessionless:?}"
    );

    // Neither refusal touched the grant, and refusal still works for an actor
    // who really does hold the session they claim — so this case cannot pass by
    // having broken the deny path for everybody.
    let refused = DenyDevice::run(
        &world.deps(),
        &world.context("deny-by-a-current-actor"),
        &started.user_code,
        owner.id,
        owner_session,
    )
    .await
    .expect("a current actor holding the user code may refuse the grant");
    assert_eq!(refused.state, DeviceState::Denied);
}

#[tokio::test]
async fn an_approved_grant_cannot_be_retracted_by_a_later_denial() {
    // `DENY_DEVICE_AUTHORIZATION` used to admit `status IN ('pending','approved')`.
    // Retracting an approved grant is not a capability this platform offers, and
    // it never was one: `dev_approved_ck` asserts that
    // `(status IN ('approved','consumed')) = (approved_by_user_id IS NOT NULL)`,
    // so writing `denied` over an approved row while leaving the approver in
    // place raises 23514. The wider status set could only ever have produced a
    // check violation, never a retraction. The remedy for a grant somebody
    // regrets is revoking the token it minted, which names the token and matches
    // its owner — something a decision keyed by user code cannot do.
    let plane = support::CentralPlane::start().await;
    let world = World::start(&plane).await;
    let (owner, owner_session, _owner_secret) = world.sign_in("founder").await;

    let started = StartDeviceAuthorization::run(
        &world.deps(),
        &world.context("device-start"),
        ScopeSet::of(&[Scope::SessionsRead]),
    )
    .await
    .expect("the device authorization ceremony commits");
    ApproveDevice::run(
        &world.deps(),
        &world.context("device-approve"),
        &started.user_code,
        owner.id,
        owner_session,
    )
    .await
    .expect("the person approves their own device");

    let retracted = DenyDevice::run(
        &world.deps(),
        &world.context("device-retract"),
        &started.user_code,
        owner.id,
        owner_session,
    )
    .await;
    assert!(
        matches!(
            retracted,
            Err(IdentityError::NotFound | IdentityError::AccessDenied)
        ),
        "an approved grant must not be retractable, but the deny statement answered {retracted:?}"
    );

    // The approval the denial could not retract still redeems, so the narrowed
    // status set refuses the retraction rather than breaking the grant.
    let digest = parse(CredentialKind::DeviceCode, started.device_code.expose())
        .expect("the device code parses")
        .digest;
    let redeemed = PollDevice::run(
        &world.deps(),
        &world.context("device-poll"),
        started.grant.id,
        digest,
    )
    .await
    .expect("the grant the denial could not retract still redeems");
    assert!(redeemed.secret.expose().starts_with("aex_at_"));
    assert_eq!(redeemed.record.token.user_id, owner.id);
}
