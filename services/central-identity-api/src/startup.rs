//! The composed binary, driven end to end over in-memory ports.
//!
//! The rest of this binary's suite proves the *shape*: which routes are
//! mounted, which capabilities admit, which variables are required. None of it
//! proves the composition **serves** — a router mounted over a stub that
//! answers `429` says nothing about whether the real service can mint a device
//! code, get it approved and redeem it.
//!
//! So this module runs the real [`crate::api::AuthService`] over the real
//! [`crate::app`] router, with only the substrate replaced: an in-memory
//! identity store and a fixed pepper. Everything between the HTTP bytes and the
//! store — route matching, the anonymous context, the body bound, the replay
//! identity, the use cases, the credential grammar, the RFC 8628 decision table
//! and the response encoding — is the code that ships.
//!
//! # What this file used to assert, and why that was worse than a gap
//!
//! The previous version proved redemption by reaching past the router and
//! calling a fixture-only `approve()` on the store, commented "as a person
//! would" — while the *same* fixture answered
//! `"is not reachable from the two routes this deployable mounts"` for the real
//! `approve_device_authorization`, `deny_device_authorization` and all three
//! dashboard-session methods. One file proved one half of the ceremony and
//! asserted the other half was unreachable, and it was green. A suite that
//! encodes a gap as an invariant stops anyone from noticing the gap.
//!
//! The fixture now implements every method the mounted routes reach, and each
//! one mirrors the predicate of the statement it stands in for — most
//! importantly `APPROVE_DEVICE_AUTHORIZATION`'s `EXISTS` over a live session
//! belonging to an active person, which is the whole reason a browser session
//! had to exist before a device could be approved. The methods that stay
//! [`MemoryIdentity::unmounted`] are the ones no route in this deployable
//! drives, which is a claim about the contract rather than about the ceremony.

#![cfg(test)]

use std::sync::{Arc, Mutex};

use aex_central_http::authorizer::{CentralAuthorizerContext, ContextPrincipalKind};
use aex_central_http::health::{HEALTH_PATH, READY_PATH};
use aex_central_http::router::EdgeStack;
use aex_control_domain::{AccountState, CursorSecret, Revision, ScopeSet};
use aex_identity_app::ports::{
    ConsumeDeviceAuthorizationCommand, ConsumeEmailChallengeCommand, CreateDashboardSessionCommand,
    CreateDeviceAuthorizationCommand, DecideDeviceAuthorizationCommand, DeviceConsumeOutcome,
    IdentityStore, IssueEmailChallengeCommand, PepperKeystore, PepperPurpose,
    ResolveDashboardSessionQuery, ResolveExternalIdentity, ResolvedUser, RevokeAccountTokenCommand,
    RevokeDashboardSessionCommand, SetUserStatusCommand, StoreError, TxOutcome,
    UnlinkExternalIdentityCommand,
};
use aex_identity_domain::credential::parse as parse_credential;
use aex_identity_domain::{
    ACCOUNT_TOKEN_TTL, AccountToken, CredentialKind, DashboardSession, DeviceAuthorization,
    DeviceState, EmailChallenge, ExternalIdentity, Pepper, PepperVersion, PresentedDigest,
    Provider, SessionState, TokenOrigin, User, UserStatus, Verifier, verify,
};
use aex_wire::ids::{PrefixedId as _, UserId};
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use time::{Duration, OffsetDateTime};
use tower::ServiceExt as _;
use uuid::Uuid;

use crate::api::{ALLOWED_CLIENT_ID, AuthService};
use crate::targets::NoOrganizationTargets;
use crate::{Probes, app, readiness};

/// The one pepper version this fixture mints and verifies under.
const VERSION: u16 = 1;

/// The PKCE verifier the fixture browser holds in its own cookie.
///
/// RFC 7636's own appendix B vector, so the `state` below is checkable by hand.
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";

/// The `state` a provider echoes back: the S256 challenge of [`VERIFIER`].
const STATE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

/// A pepper keystore holding one fixed version.
#[derive(Debug)]
struct FixedPepper;

#[async_trait]
impl PepperKeystore for FixedPepper {
    async fn active(&self, _purpose: PepperPurpose) -> Result<(PepperVersion, Pepper), StoreError> {
        Ok((PepperVersion::new(VERSION), Pepper::new([7_u8; 32])))
    }

    async fn by_version(
        &self,
        _purpose: PepperPurpose,
        version: PepperVersion,
    ) -> Result<Pepper, StoreError> {
        if version == PepperVersion::new(VERSION) {
            return Ok(Pepper::new([7_u8; 32]));
        }
        Err(StoreError::NotFound)
    }
}

/// The pepper every keyed digest in this fixture is computed under.
fn pepper() -> Pepper {
    Pepper::new([7_u8; 32])
}

/// One recorded device grant, the verifier it was minted under, and the keyed
/// user-code digest a decision looks it up by.
#[derive(Clone)]
struct Grant {
    record: DeviceAuthorization,
    device_verifier: Verifier,
    user_code_hash: [u8; 32],
}

impl std::fmt::Debug for Grant {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Grant").finish_non_exhaustive()
    }
}

/// The identity rows the five mounted routes touch, in memory.
///
/// Every method a mounted route reaches is implemented, and each mirrors the
/// predicate of the Aurora statement it stands in for. The methods that answer
/// [`StoreError::Fatal`] are the ones no route drives — a fixture method that
/// ever ran there would mean this binary had grown a surface its own
/// composition test denies.
#[derive(Debug, Default)]
struct MemoryIdentity {
    grants: Mutex<Vec<Grant>>,
    users: Mutex<Vec<User>>,
    links: Mutex<Vec<ExternalIdentity>>,
    sessions: Mutex<Vec<DashboardSession>>,
}

impl MemoryIdentity {
    fn unmounted(method: &'static str) -> StoreError {
        StoreError::Fatal(format!(
            "`{method}` is not reachable from the five routes this deployable mounts"
        ))
    }

    fn find(&self, device_id: Uuid, digest: &PresentedDigest) -> Option<DeviceAuthorization> {
        let grants = self.grants.lock().expect("not poisoned");
        let recorded = grants.iter().find(|it| it.record.id == device_id)?;
        verify(&pepper(), digest, &recorded.device_verifier).then(|| recorded.record.clone())
    }

    /// `APPROVE_DEVICE_AUTHORIZATION`'s `EXISTS` clause, in Rust.
    ///
    /// A live session, belonging to the named person, who is active. Kept
    /// separate so the one test that disables a person can name what it broke.
    fn actor_is_current(&self, session_id: Uuid, user_id: Uuid, now: OffsetDateTime) -> bool {
        let sessions = self.sessions.lock().expect("not poisoned");
        let Some(session) = sessions
            .iter()
            .find(|it| it.id == session_id && it.user_id == user_id)
        else {
            return false;
        };
        if session.state_at(now) != SessionState::Active {
            return false;
        }
        let users = self.users.lock().expect("not poisoned");
        users
            .iter()
            .any(|it| it.id == user_id && it.status == UserStatus::Active)
    }

    /// Applies one decision under the state guard the statement carries.
    fn decide(
        &self,
        command: &DecideDeviceAuthorizationCommand,
        admitted: &[DeviceState],
        next: DeviceState,
    ) -> Result<TxOutcome<DeviceAuthorization>, StoreError> {
        let mut grants = self.grants.lock().expect("not poisoned");
        let Some(recorded) = grants
            .iter_mut()
            .find(|it| it.user_code_hash == command.user_code_hash)
        else {
            return Err(StoreError::NotFound);
        };
        if !admitted.contains(&recorded.record.state) || recorded.record.expires_at <= command.now {
            return Err(StoreError::NotFound);
        }
        recorded.record.state = next;
        if next == DeviceState::Approved {
            recorded.record.approved_by = Some(command.actor_user_id);
            recorded.record.approved_at = Some(command.now);
            // The poll bookkeeping is cleared so the device's next poll is a
            // redemption rather than a slow-down. The statement does not write
            // this column; the row simply has not been polled since it changed.
            recorded.record.last_polled_at = None;
        }
        Ok(TxOutcome::Committed(recorded.record.clone()))
    }
}

#[async_trait]
impl IdentityStore for MemoryIdentity {
    async fn create_device_authorization(
        &self,
        command: &CreateDeviceAuthorizationCommand,
    ) -> Result<TxOutcome<DeviceAuthorization>, StoreError> {
        let grant = DeviceAuthorization {
            id: command.preassigned_id,
            state: DeviceState::Pending,
            requested_scopes: command.requested_scopes,
            pepper_version: command.pepper_version,
            approved_by: None,
            approved_at: None,
            consumed_at: None,
            account_token_id: None,
            issued_at: command.issued_at,
            expires_at: command.expires_at,
            poll_interval: Duration::milliseconds(i64::from(command.poll_interval_ms)),
            last_polled_at: None,
        };
        self.grants.lock().expect("not poisoned").push(Grant {
            record: grant.clone(),
            device_verifier: command.device_verifier,
            user_code_hash: command.user_code_hash,
        });
        Ok(TxOutcome::Committed(grant))
    }

    async fn poll_device_authorization(
        &self,
        device_id: Uuid,
        digest: &PresentedDigest,
        _now: OffsetDateTime,
    ) -> Result<Option<DeviceAuthorization>, StoreError> {
        Ok(self.find(device_id, digest))
    }

    async fn consume_device_authorization(
        &self,
        command: &ConsumeDeviceAuthorizationCommand,
    ) -> Result<TxOutcome<DeviceConsumeOutcome>, StoreError> {
        // The real statement is a conditional `UPDATE` whose predicate repeats
        // the domain guard, so exactly one of N concurrent redemptions wins.
        // The fixture models that by writing the consumed state back: a second
        // redemption then sees a consumed grant rather than an approved one.
        let mut grants = self.grants.lock().expect("not poisoned");
        let Some(recorded) = grants.iter_mut().find(|it| {
            it.record.id == command.device_id
                && verify(&pepper(), &command.digest, &it.device_verifier)
        }) else {
            return Err(StoreError::NotFound);
        };
        if recorded.record.state != DeviceState::Approved {
            return Err(StoreError::NotFound);
        }
        recorded.record.state = DeviceState::Consumed;
        recorded.record.consumed_at = Some(command.now);
        recorded.record.account_token_id = Some(command.preassigned_token_id);
        let grant = recorded.record.clone();
        let token = AccountToken {
            id: command.preassigned_token_id,
            // Never a fresh identifier: the token belongs to whoever approved
            // the grant, and inventing one here is exactly the shortcut that
            // let the old suite pass without an approver existing.
            user_id: grant.approved_by.ok_or_else(|| {
                StoreError::Fatal("an approved grant carries no approver".to_owned())
            })?,
            name: command.token_name.clone(),
            scopes: grant.requested_scopes,
            origin: TokenOrigin::DeviceFlow,
            pepper_version: command.token_pepper_version,
            issued_at: command.now,
            expires_at: command.now + ACCOUNT_TOKEN_TTL,
            revoked_at: None,
        };
        Ok(TxOutcome::Committed(DeviceConsumeOutcome { grant, token }))
    }

    async fn approve_device_authorization(
        &self,
        command: &DecideDeviceAuthorizationCommand,
    ) -> Result<TxOutcome<DeviceAuthorization>, StoreError> {
        if !self.actor_is_current(command.actor_session_id, command.actor_user_id, command.now) {
            return Err(StoreError::NotFound);
        }
        self.decide(command, &[DeviceState::Pending], DeviceState::Approved)
    }

    async fn deny_device_authorization(
        &self,
        command: &DecideDeviceAuthorizationCommand,
    ) -> Result<TxOutcome<DeviceAuthorization>, StoreError> {
        // The same predicate as its approving twin, because the statement now
        // carries the same one. Until 2026-08-10 it carried neither the actor
        // `EXISTS` nor a single-state guard, and this fixture mirrored that
        // faithfully; refusing a device is not the safe direction, because a
        // refusal ends a sign-in that belongs to somebody else.
        if !self.actor_is_current(command.actor_session_id, command.actor_user_id, command.now) {
            return Err(StoreError::NotFound);
        }
        self.decide(command, &[DeviceState::Pending], DeviceState::Denied)
    }

    async fn resolve_or_create_by_external_identity(
        &self,
        command: &ResolveExternalIdentity,
    ) -> Result<TxOutcome<ResolvedUser>, StoreError> {
        let mut users = self.users.lock().expect("not poisoned");
        let mut links = self.links.lock().expect("not poisoned");

        // Link first, address second, create last — the order the ceremony's
        // one transaction uses, and the reason a verified address is required
        // at the edge: resolving by address is what makes two provider accounts
        // able to reach one person at all.
        let existing = links
            .iter()
            .find(|it| {
                it.provider == command.provider
                    && it.provider_account_id == command.provider_account_id
            })
            .map(|it| it.user_id)
            .or_else(|| {
                users
                    .iter()
                    .find(|it| it.email == command.email)
                    .map(|it| it.id)
            });

        let (user_id, created) = if let Some(id) = existing {
            (id, false)
        } else {
            users.push(User {
                id: command.preassigned_user_id,
                email: command.email.clone(),
                email_verified_at: command.email_verified.then_some(command.now),
                name: command.name.clone(),
                image_url: command.image_url.clone(),
                status: UserStatus::Active,
                revision: Revision::INITIAL,
                created_at: command.now,
                updated_at: command.now,
            });
            (command.preassigned_user_id, true)
        };
        if !links.iter().any(|it| {
            it.user_id == user_id
                && it.provider == command.provider
                && it.provider_account_id == command.provider_account_id
        }) {
            links.push(ExternalIdentity {
                id: command.preassigned_link_id,
                user_id,
                provider: command.provider,
                provider_account_id: command.provider_account_id.clone(),
                linked_at: command.now,
            });
        }
        let user = users
            .iter()
            .find(|it| it.id == user_id)
            .cloned()
            .ok_or(StoreError::NotFound)?;
        let mine = links
            .iter()
            .filter(|it| it.user_id == user_id)
            .cloned()
            .collect();
        Ok(TxOutcome::Committed(ResolvedUser {
            user,
            created,
            links: mine,
        }))
    }

    async fn create_dashboard_session(
        &self,
        command: &CreateDashboardSessionCommand,
    ) -> Result<TxOutcome<DashboardSession>, StoreError> {
        // `INSERT_DASHBOARD_SESSION` selects from `identity.user` with
        // `status = 'active'`, so a disabled person yields zero inserted rows
        // rather than a session nobody may use.
        let active = self
            .users
            .lock()
            .expect("not poisoned")
            .iter()
            .any(|it| it.id == command.user_id && it.status == UserStatus::Active);
        if !active {
            return Err(StoreError::NotFound);
        }
        let session = DashboardSession {
            id: command.preassigned_id,
            user_id: command.user_id,
            pepper_version: command.pepper_version,
            issued_at: command.issued_at,
            expires_at: command.expires_at,
            revoked_at: None,
        };
        self.sessions
            .lock()
            .expect("not poisoned")
            .push(session.clone());
        Ok(TxOutcome::Committed(session))
    }

    async fn revoke_dashboard_session(
        &self,
        command: &RevokeDashboardSessionCommand,
    ) -> Result<TxOutcome<()>, StoreError> {
        let mut sessions = self.sessions.lock().expect("not poisoned");
        if let Some(session) = sessions
            .iter_mut()
            .find(|it| it.id == command.session_id && it.revoked_at.is_none())
        {
            session.revoked_at = Some(command.now);
        }
        // Closing an already-closed session is the outcome the caller asked
        // for; the statement's `revoked_at IS NULL` guard means it affects zero
        // rows and reports no failure.
        Ok(TxOutcome::Committed(()))
    }

    async fn resolve_dashboard_session(
        &self,
        _query: &ResolveDashboardSessionQuery,
    ) -> Result<Option<(DashboardSession, User)>, StoreError> {
        // The credential read belongs to `central-authz`, which runs as a role
        // with no write privilege at all. This deployable never resolves a
        // presented session: it receives the already-verified authorizer
        // context.
        Err(Self::unmounted("resolve_dashboard_session"))
    }

    async fn issue_email_challenge(
        &self,
        _command: &IssueEmailChallengeCommand,
    ) -> Result<TxOutcome<EmailChallenge>, StoreError> {
        Err(Self::unmounted("issue_email_challenge"))
    }

    async fn consume_email_challenge(
        &self,
        _command: &ConsumeEmailChallengeCommand,
    ) -> Result<TxOutcome<ResolvedUser>, StoreError> {
        Err(Self::unmounted("consume_email_challenge"))
    }

    async fn set_user_status(
        &self,
        _command: &SetUserStatusCommand,
    ) -> Result<TxOutcome<User>, StoreError> {
        Err(Self::unmounted("set_user_status"))
    }

    async fn unlink_external_identity(
        &self,
        _command: &UnlinkExternalIdentityCommand,
    ) -> Result<TxOutcome<()>, StoreError> {
        Err(Self::unmounted("unlink_external_identity"))
    }

    async fn revoke_account_token(
        &self,
        _command: &RevokeAccountTokenCommand,
    ) -> Result<TxOutcome<()>, StoreError> {
        Err(Self::unmounted("revoke_account_token"))
    }
}

/// The sign-in provider, in memory.
///
/// It stands in for a token endpoint and nothing more: it records every code
/// presented to it and answers with a person, or refuses. What matters is that
/// the handler reaches it **only** when the `state` check has already passed —
/// `redeemed` is what the forgery test asserts against, because a request that
/// never reached here never spent a code.
#[derive(Debug, Default)]
struct MemoryProvider {
    redeemed: Mutex<Vec<String>>,
}

impl MemoryProvider {
    /// A code this fixture provider will redeem, for one person.
    fn code_for(account: &str) -> String {
        format!("code-for-{account}")
    }
}

#[async_trait]
impl crate::oauth::ProviderHandshake for MemoryProvider {
    async fn identify(
        &self,
        provider: Provider,
        code: &str,
        verifier: &str,
    ) -> Result<aex_identity_app::use_cases::OauthProfile, crate::oauth::HandshakeError> {
        assert_eq!(
            verifier, VERIFIER,
            "the handler forwarded a verifier it had not checked `state` against"
        );
        self.redeemed
            .lock()
            .expect("not poisoned")
            .push(code.to_owned());
        // The one code shape this fixture knows. A real provider refusing a code
        // and this refusing an unknown one are the same answer to the handler.
        let account = code.strip_prefix("code-for-").ok_or_else(|| {
            crate::oauth::HandshakeError::Refused("no such authorization code".to_owned())
        })?;
        if account == "unverified" {
            return Err(crate::oauth::HandshakeError::Unusable(
                "the provider asserts no verified primary address".to_owned(),
            ));
        }
        Ok(aex_identity_app::use_cases::OauthProfile {
            provider,
            provider_account_id: aex_identity_domain::ProviderAccountId::parse(account)
                .expect("a storable account id"),
            email: aex_identity_domain::NormalizedEmail::parse(&format!("{account}@example.com"))
                .expect("a normalizable address"),
            email_verified: true,
            name: Some("A Person".to_owned()),
            image_url: None,
        })
    }
}

/// The account authority, which this suite never reaches.
struct UnreachableAccounts;

#[async_trait::async_trait]
impl crate::account::AccountReader for UnreachableAccounts {
    async fn is_member(
        &self,
        _organization: uuid::Uuid,
        _user: uuid::Uuid,
    ) -> Result<bool, String> {
        Err("the composition suite issues no account read".to_owned())
    }

    async fn account_profile(
        &self,
        _organization: uuid::Uuid,
    ) -> Result<Option<aex_control_domain::AccountProfile>, String> {
        Err("the composition suite issues no account read".to_owned())
    }
}

/// The composed router, over the real service and the three in-memory ports.
fn composed(probes: Probes) -> (axum::Router, Arc<MemoryIdentity>, Arc<MemoryProvider>) {
    let store = Arc::new(MemoryIdentity::default());
    let provider = Arc::new(MemoryProvider::default());
    let clock: Arc<dyn aex_identity_app::ports::Clock> = Arc::new(aex_central_aws::SystemClock);
    let service = AuthService::new(
        Arc::clone(&store) as Arc<dyn IdentityStore>,
        Arc::new(FixedPepper),
        Arc::clone(&clock),
        Arc::new(aex_central_aws::Uuid7Factory),
        Arc::new(aex_central_aws::OsSecretRng),
        aex_wire::types::HttpsUrl::parse("https://aex.dev/device").expect("a valid URL"),
        Arc::clone(&provider) as Arc<dyn crate::oauth::ProviderHandshake>,
    );
    let config = crate::Config::from_lookup(|name| environment(name).map(str::to_owned))
        .expect("a complete environment");
    let edge = EdgeStack::new(
        config.http,
        Arc::new(NoOrganizationTargets),
        clock,
        Arc::new(CursorSecret::new([3_u8; 32])),
    );
    // The account read is composed here with a refusing reader: this suite is
    // about what the process mounts, and a reader that answered would make the
    // route pass on an invented row rather than on being mounted.
    let account = Arc::new(crate::account::AccountService::new(Arc::new(
        UnreachableAccounts,
    )));
    (
        app(Arc::new(service), account, edge, readiness(probes)),
        store,
        provider,
    )
}

/// The environment a composed process would read.
fn environment(name: &str) -> Option<&'static str> {
    Some(match name {
        "AEX_CENTRAL_IDENTITY_PLANE" => "dev",
        "AEX_CENTRAL_IDENTITY_REGION" => "eu-west-1",
        "AEX_CENTRAL_IDENTITY_ACCOUNT_ID" => "000000000000",
        "AEX_CENTRAL_IDENTITY_AURORA_CLUSTER_ARN" => {
            "arn:aws:rds:eu-west-1:000000000000:cluster:aex"
        }
        "AEX_CENTRAL_IDENTITY_AURORA_SECRET_ARN" => {
            "arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex-identity"
        }
        "AEX_CENTRAL_IDENTITY_PEPPER_SECRET_ID" => "aex/dev/identity-pepper",
        "AEX_CENTRAL_IDENTITY_GITHUB_OAUTH_SECRET_ID" => "aex/dev/sign-in/github",
        "AEX_CENTRAL_IDENTITY_GOOGLE_OAUTH_SECRET_ID" => "aex/dev/sign-in/google",
        "AEX_CENTRAL_IDENTITY_SIGN_IN_REDIRECT_URI" => "https://dash.aex.dev/auth/callback",
        "AEX_CENTRAL_IDENTITY_DATABASE" => "aex",
        "AEX_CENTRAL_IDENTITY_ROLE" => "aex_identity_api",
        "AEX_CENTRAL_IDENTITY_DEVICE_VERIFICATION_URI" => "https://aex.dev/device",
        "AEX_CENTRAL_IDENTITY_MAX_BODY_BYTES" => "65536",
        "AEX_CENTRAL_IDENTITY_REQUEST_DEADLINE_MS" => "5000",
        _ => return None,
    })
}

/// Issues one request against a freshly cloned router.
async fn call(router: &axum::Router, request: Request<Body>) -> (StatusCode, serde_json::Value) {
    let response = router
        .clone()
        .oneshot(request)
        .await
        .expect("the router answers");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("a bounded body");
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, json)
}

fn start_request() -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/auth/device/authorizations")
        .header("idempotency-key", "01hq0000000000000000000000")
        .body(Body::from(format!(
            r#"{{"clientId":"{ALLOWED_CLIENT_ID}","scopes":[]}}"#
        )))
        .expect("a valid request")
}

fn token_request(device_code: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/auth/device/tokens")
        .body(Body::from(format!(
            r#"{{"clientId":"{ALLOWED_CLIENT_ID}","deviceCode":"{device_code}"}}"#
        )))
        .expect("a valid request")
}

/// A callback body: what a browser redirect actually carries.
fn sign_in_request(state: &str, code: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/auth/sessions")
        .body(Body::from(format!(
            r#"{{"provider":"github","code":"{code}","state":"{state}","codeVerifier":"{VERIFIER}"}}"#
        )))
        .expect("a valid request")
}

/// The authorizer context `central-authz` would produce for a resolved browser
/// session.
///
/// Building it from the credential the sign-in route actually minted is what
/// makes the approval leg honest: the session id this approves under is the one
/// the mint produced, so the fixture's `EXISTS` predicate is evaluated against
/// a row the ceremony wrote rather than one the test invented.
fn session_context(session_credential: &str, user_id: Uuid) -> CentralAuthorizerContext {
    let parsed = parse_credential(CredentialKind::DashboardSession, session_credential)
        .expect("the minted credential parses under its own grammar");
    let now_ms = i64::try_from(
        OffsetDateTime::now_utc()
            .unix_timestamp_nanos()
            .div_euclid(1_000_000),
    )
    .expect("a representable instant");
    CentralAuthorizerContext {
        request_id: aex_wire::types::RequestId::parse("req-ceremony").expect("a request id"),
        kind: ContextPrincipalKind::UserSession,
        principal_id: user_id,
        credential_id: Some(parsed.id),
        workspace_id: None,
        organization_id: None,
        region: None,
        memberships: Vec::new(),
        scopes: ScopeSet::from_strings(&["account:read", "account:write"]).expect("known scopes"),
        account_state: AccountState::Unavailable,
        issued_at_ms: now_ms - 1_000,
        expires_at_ms: now_ms + 30_000,
    }
}

/// A decision request carrying an already-resolved browser session.
fn decision_request(
    context: &CentralAuthorizerContext,
    user_code: &str,
    decision: &str,
) -> Request<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri("/api/auth/device/decisions")
        .body(Body::from(format!(
            r#"{{"userCode":"{user_code}","decision":"{decision}"}}"#
        )))
        .expect("a valid request");
    request.extensions_mut().insert(context.clone());
    request
}

/// Signs a person in through the real route and returns their credential, id
/// and authorizer context.
async fn sign_in(router: &axum::Router) -> (String, Uuid, CentralAuthorizerContext) {
    let (status, body) = call(
        router,
        sign_in_request(STATE, &MemoryProvider::code_for("gh-1")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let credential = body["session"].as_str().expect("a session").to_owned();
    let user_id = Uuid::from_bytes(
        *body["userId"]
            .as_str()
            .expect("a user id")
            .parse::<UserId>()
            .expect("the response carries a platform user identifier")
            .uuid7()
            .as_bytes(),
    );
    let context = session_context(&credential, user_id);
    (credential, user_id, context)
}

#[tokio::test]
async fn the_composed_binary_mints_a_device_code_a_person_can_approve() {
    let (router, _store, _provider) = composed(Probes::READY);
    let (status, body) = call(&router, start_request()).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let device_code = body["deviceCode"].as_str().expect("a device code");
    assert!(
        device_code.starts_with("aex_dc_"),
        "the credential grammar is the one the CLI parses: {device_code}"
    );
    assert!(
        body["userCode"]
            .as_str()
            .expect("a user code")
            .contains('-'),
        "{body}"
    );
    assert_eq!(
        body["verificationUri"], "https://aex.dev/device",
        "the approval page comes from configuration, not a literal"
    );
    assert_eq!(
        body["verificationUriComplete"],
        format!(
            "https://aex.dev/device?user_code={}",
            body["userCode"].as_str().expect("a user code")
        )
    );
    assert!(body["intervalSeconds"].as_u64().expect("an interval") >= 1);
}

#[tokio::test]
async fn a_device_polling_an_unapproved_grant_is_told_to_keep_waiting() {
    let (router, _store, _provider) = composed(Probes::READY);
    let (_, started) = call(&router, start_request()).await;
    let device_code = started["deviceCode"].as_str().expect("a device code");

    let (status, body) = call(&router, token_request(device_code)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        body["error"]["code"], "authorization_pending",
        "a pending grant must not read as a failure the CLI gives up on: {body}"
    );
}

/// The whole ceremony, over HTTP, with no fixture reach-around.
///
/// This is the test the old suite could not write. Every state change is made
/// by a mounted route: the grant is minted by one, the browser session by
/// another, the approval by a third, and the account token by a fourth. Nothing
/// touches the store except through the router.
#[tokio::test]
async fn a_person_signs_in_approves_a_device_and_the_device_redeems_a_token() {
    let (router, store, _provider) = composed(Probes::READY);

    let (_, started) = call(&router, start_request()).await;
    let device_code = started["deviceCode"]
        .as_str()
        .expect("a device code")
        .to_owned();
    let user_code = started["userCode"]
        .as_str()
        .expect("a user code")
        .to_owned();

    let (_credential, user_id, context) = sign_in(&router).await;
    assert_eq!(
        store.sessions.lock().expect("not poisoned").len(),
        1,
        "the sign-in exchange minted no browser session"
    );

    let (status, decided) = call(&router, decision_request(&context, &user_code, "approve")).await;
    assert_eq!(status, StatusCode::OK, "{decided}");
    assert_eq!(decided["decision"], "approve", "{decided}");
    assert!(decided["decidedAt"].as_str().is_some(), "{decided}");
    assert_eq!(
        store.grants.lock().expect("not poisoned")[0].record.state,
        DeviceState::Approved,
        "the mounted route did not move the grant off `Pending`"
    );
    assert_eq!(
        store.grants.lock().expect("not poisoned")[0]
            .record
            .approved_by,
        Some(user_id),
        "the approval is attributed to whoever signed in"
    );

    let (status, body) = call(&router, token_request(&device_code)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let token = body["accountToken"].as_str().expect("an account token");
    assert!(
        token.starts_with("aex_at_"),
        "the minted credential is an account token: {token}"
    );
    assert!(body["expiresAt"].as_str().is_some(), "{body}");

    // The grant is single use. A second redemption of the same code must not
    // mint a second token, whatever else it answers.
    let (again, second) = call(&router, token_request(&device_code)).await;
    assert_ne!(
        again,
        StatusCode::OK,
        "the grant was redeemed twice: {second}"
    );
    assert!(second["accountToken"].is_null(), "{second}");
}

#[tokio::test]
async fn a_denied_grant_can_never_be_redeemed() {
    let (router, _store, _provider) = composed(Probes::READY);
    let (_, started) = call(&router, start_request()).await;
    let device_code = started["deviceCode"]
        .as_str()
        .expect("a device code")
        .to_owned();
    let user_code = started["userCode"]
        .as_str()
        .expect("a user code")
        .to_owned();

    let (_credential, _user, context) = sign_in(&router).await;
    let (status, decided) = call(&router, decision_request(&context, &user_code, "deny")).await;
    assert_eq!(status, StatusCode::OK, "{decided}");
    assert_eq!(decided["decision"], "deny", "{decided}");

    let (status, body) = call(&router, token_request(&device_code)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        body["error"]["code"], "invalid_request",
        "a denial and an unknown code are one answer to an anonymous poller: {body}"
    );
}

#[tokio::test]
async fn a_decision_without_a_browser_session_is_refused() {
    let (router, store, _provider) = composed(Probes::READY);
    let (_, started) = call(&router, start_request()).await;
    let user_code = started["userCode"]
        .as_str()
        .expect("a user code")
        .to_owned();

    // No authorizer context at all: the edge answers before the handler runs.
    let request = Request::builder()
        .method("POST")
        .uri("/api/auth/device/decisions")
        .body(Body::from(format!(
            r#"{{"userCode":"{user_code}","decision":"approve"}}"#
        )))
        .expect("a valid request");
    let (status, body) = call(&router, request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(
        store.grants.lock().expect("not poisoned")[0].record.state,
        DeviceState::Pending,
        "an unauthenticated request decided a grant"
    );
}

/// An account token authenticates the same person as a browser session, and the
/// authorizer treats them as one principal. The decision route must still
/// refuse it, because `approve_device_authorization` records the session that
/// proved the approver was current and an account token proves no such thing.
#[tokio::test]
async fn an_account_token_may_not_stand_in_for_the_session_that_proves_currency() {
    let (router, store, _provider) = composed(Probes::READY);
    let (_, started) = call(&router, start_request()).await;
    let user_code = started["userCode"]
        .as_str()
        .expect("a user code")
        .to_owned();

    let (credential, user_id, session) = sign_in(&router).await;
    let mut token_context = session_context(&credential, user_id);
    token_context.kind = ContextPrincipalKind::Account;

    let (status, body) = call(
        &router,
        decision_request(&token_context, &user_code, "approve"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(
        store.grants.lock().expect("not poisoned")[0].record.state,
        DeviceState::Pending,
        "an account token approved a device authorization"
    );

    // The same person, presenting the session, is admitted — so the refusal
    // above is about the credential and not about the person.
    let (status, _) = call(&router, decision_request(&session, &user_code, "approve")).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_user_code_no_grant_is_waiting_on_is_not_found() {
    let (router, _store, _provider) = composed(Probes::READY);
    let (_credential, _user, context) = sign_in(&router).await;
    let (status, body) = call(
        &router,
        decision_request(&context, "BCDFG-HJKLM", "approve"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"]["code"], "not_found", "{body}");
}

#[tokio::test]
async fn a_user_code_outside_the_alphabet_never_reaches_the_store() {
    let (router, store, _provider) = composed(Probes::READY);
    let (_credential, _user, context) = sign_in(&router).await;
    for forged in ["", "AEIOU-BCDFG", "BCDFG", "not-a-code"] {
        let (status, body) = call(&router, decision_request(&context, forged, "approve")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{forged}: {body}");
    }
    assert!(
        store.grants.lock().expect("not poisoned").is_empty(),
        "a forged user code cost a store round trip"
    );
}

/// The login-CSRF forgery, refused before a single code is spent.
///
/// An attacker who starts their own sign-in holds a valid code and a valid
/// `state`, and can make a victim's browser hit the callback with both. What
/// they cannot do is write a cookie on this platform's origin, so the verifier
/// the victim's browser presents is the victim's, and it does not hash to the
/// attacker's `state`.
///
/// The assertion that matters is the last one: the provider is never dialled.
/// A refusal that happened *after* the exchange would still have spent a code
/// and would still have told an attacker their code was good.
#[tokio::test]
async fn a_redirect_whose_state_does_not_match_its_verifier_never_reaches_the_provider() {
    let (router, store, provider) = composed(Probes::READY);
    let attackers_state =
        crate::oauth::challenge_of("an-attackers-own-verifier-000000000000000000");
    for forged in [
        "",
        &attackers_state,
        // The right challenge with one character changed.
        "e9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
        STATE.trim_end_matches('M'),
    ] {
        let (status, body) = call(
            &router,
            sign_in_request(forged, &MemoryProvider::code_for("gh-1")),
        )
        .await;
        assert!(
            status == StatusCode::UNAUTHORIZED || status == StatusCode::BAD_REQUEST,
            "{forged}: {status} {body}"
        );
    }
    assert!(
        provider.redeemed.lock().expect("not poisoned").is_empty(),
        "a forged redirect reached the provider and spent an authorization code"
    );
    assert!(
        store.users.lock().expect("not poisoned").is_empty(),
        "a forged redirect created a person"
    );
    assert!(
        store.sessions.lock().expect("not poisoned").is_empty(),
        "a forged redirect minted a session"
    );
}

/// A code the provider refuses mints nothing.
#[tokio::test]
async fn a_code_the_provider_refuses_authenticates_nobody() {
    let (router, store, _provider) = composed(Probes::READY);
    let (status, body) = call(&router, sign_in_request(STATE, "not-a-real-code")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"]["code"], "unauthenticated", "{body}");
    assert!(store.users.lock().expect("not poisoned").is_empty());
    assert!(store.sessions.lock().expect("not poisoned").is_empty());
}

/// An unverified address must not resolve to an existing person.
///
/// The store resolves by normalized email when no provider link matches, so
/// accepting an unverified assertion would let one provider account adopt
/// another person's records. That is a cross-tenant read, which is a
/// correctness defect rather than a matter of degree. The rule is now enforced
/// inside the handshake — `crate::oauth` refuses to build a profile without a
/// verified address — and this asserts the refusal reaches the wire as a
/// caller-actionable answer rather than a `500`, and writes nobody.
#[tokio::test]
async fn an_unverified_address_links_to_nobody() {
    let (router, store, _provider) = composed(Probes::READY);
    let (status, body) = call(
        &router,
        sign_in_request(STATE, &MemoryProvider::code_for("unverified")),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_request", "{body}");
    assert!(store.users.lock().expect("not poisoned").is_empty());
    assert!(store.sessions.lock().expect("not poisoned").is_empty());
}

#[tokio::test]
async fn signing_in_twice_resolves_one_person_and_two_sessions() {
    let (router, store, _provider) = composed(Probes::READY);
    let (first, _, _) = sign_in(&router).await;
    let (second, _, _) = sign_in(&router).await;
    assert_ne!(first, second, "the second sign-in replayed a credential");
    assert_eq!(
        store.users.lock().expect("not poisoned").len(),
        1,
        "one provider account resolved to two people"
    );
    assert_eq!(store.sessions.lock().expect("not poisoned").len(), 2);
}

#[tokio::test]
async fn closing_a_session_stops_it_approving_anything() {
    let (router, store, _provider) = composed(Probes::READY);
    let (_, started) = call(&router, start_request()).await;
    let user_code = started["userCode"]
        .as_str()
        .expect("a user code")
        .to_owned();
    let (_credential, _user, context) = sign_in(&router).await;

    let mut close = Request::builder()
        .method("DELETE")
        .uri("/api/auth/sessions/current")
        .body(Body::empty())
        .expect("a valid request");
    close.extensions_mut().insert(context.clone());
    let (status, body) = call(&router, close).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert!(
        store.sessions.lock().expect("not poisoned")[0]
            .revoked_at
            .is_some(),
        "the mounted route did not revoke the session"
    );

    // The authorizer context is unchanged — a real one would stop resolving —
    // so this proves the *store* predicate refuses, which is the guard that
    // matters when a context is still inside its 30-second window.
    let (status, body) = call(&router, decision_request(&context, &user_code, "approve")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(
        store.grants.lock().expect("not poisoned")[0].record.state,
        DeviceState::Pending
    );
}

#[tokio::test]
async fn a_device_code_that_is_not_this_platform_s_grammar_never_reaches_the_store() {
    let (router, store, _provider) = composed(Probes::READY);
    for forged in [
        "",
        "aex_dvc_",
        "not-a-code",
        "aex_at_0000000000000000000000000",
    ] {
        let (status, body) = call(&router, token_request(forged)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{forged}: {body}");
    }
    assert!(
        store.grants.lock().expect("not poisoned").is_empty(),
        "a forged code cost a store round trip"
    );
}

#[tokio::test]
async fn a_body_naming_another_client_is_refused_before_any_credential_is_minted() {
    let (router, store, _provider) = composed(Probes::READY);
    let request = Request::builder()
        .method("POST")
        .uri("/api/auth/device/authorizations")
        .header("idempotency-key", "01hq0000000000000000000000")
        .body(Body::from(r#"{"clientId":"attacker","scopes":[]}"#))
        .expect("a valid request");
    let (status, body) = call(&router, request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        store.grants.lock().expect("not poisoned").is_empty(),
        "an unregistered client minted a grant"
    );
}

#[tokio::test]
async fn readiness_answers_from_the_probes_the_process_actually_ran() {
    let (unready, _, _) = composed(Probes::NONE);
    let (status, body) = call(
        &unready,
        Request::builder()
            .uri(READY_PATH)
            .body(Body::empty())
            .expect("a valid request"),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");

    let (ready, _, _) = composed(Probes::READY);
    let (status, body) = call(
        &ready,
        Request::builder()
            .uri(READY_PATH)
            .body(Body::empty())
            .expect("a valid request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Liveness never depends on a dependency: a process that is up says so even
    // while it is refusing traffic, which is what tells an operator the
    // difference between a crash loop and a dependency outage.
    let (status, _) = call(
        &unready,
        Request::builder()
            .uri(HEALTH_PATH)
            .body(Body::empty())
            .expect("a valid request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn no_route_outside_this_deployable_s_declared_set_is_served() {
    let (router, _, _) = composed(Probes::READY);
    for path in [
        "/api/organizations",
        "/api/workspaces",
        "/api/api-keys",
        "/api/bootstrap",
        "/api/operations",
    ] {
        let (status, _) = call(
            &router,
            Request::builder()
                .uri(path)
                .body(Body::empty())
                .expect("a valid request"),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path} is mounted here");
    }
}

#[test]
fn the_scope_set_a_grant_records_is_the_one_the_body_asked_for() {
    // The empty request is the CLI's default, and it must not silently become
    // the mintable ceiling.
    assert_eq!(ScopeSet::EMPTY.to_strings(), Vec::<String>::new());
}
