//! The composed binary, driven end to end over in-memory ports.
//!
//! The rest of this binary's suite proves the *shape*: which routes are
//! mounted, which capabilities admit, which variables are required. None of it
//! proves the composition **serves** — a router mounted over a stub that
//! answers `429` says nothing about whether the real service can mint a device
//! code and redeem it.
//!
//! So this module runs the real [`crate::api::AuthService`] over the real
//! [`crate::app`] router, with only the two substrate ports replaced: an
//! in-memory identity store and a fixed pepper. Everything between the HTTP
//! bytes and the store — route matching, the anonymous context, the body bound,
//! the replay identity, the use cases, the credential grammar, the RFC 8628
//! decision table and the response encoding — is the code that ships.

#![cfg(test)]

use std::sync::{Arc, Mutex};

use aex_central_http::health::{HEALTH_PATH, READY_PATH};
use aex_central_http::router::EdgeStack;
use aex_control_domain::{CursorSecret, ScopeSet};
use aex_identity_app::ports::{
    ConsumeDeviceAuthorizationCommand, ConsumeEmailChallengeCommand, CreateDashboardSessionCommand,
    CreateDeviceAuthorizationCommand, DecideDeviceAuthorizationCommand, DeviceConsumeOutcome,
    IdentityStore, IssueEmailChallengeCommand, PepperKeystore, PepperPurpose,
    ResolveDashboardSessionQuery, ResolveExternalIdentity, ResolvedUser, RevokeAccountTokenCommand,
    RevokeDashboardSessionCommand, SetUserStatusCommand, StoreError, TxOutcome,
    UnlinkExternalIdentityCommand,
};
use aex_identity_domain::{
    ACCOUNT_TOKEN_TTL, AccountToken, DashboardSession, DeviceAuthorization, DeviceState,
    EmailChallenge, Pepper, PepperVersion, PresentedDigest, TokenOrigin, User, Verifier, verify,
};
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

/// One recorded device grant and the verifier it was minted under.
#[derive(Clone)]
struct Grant {
    grant: DeviceAuthorization,
    device_verifier: Verifier,
}

/// The two device ceremonies, in memory.
///
/// Every other `IdentityStore` method answers [`StoreError::Fatal`]. That is
/// not a shortcut: `CentralServiceId::IdentityApi` mounts exactly the two
/// device-flow routes, so a fixture method that ever ran would mean this binary
/// had grown a surface its own composition test denies.
#[derive(Debug, Default)]
struct MemoryIdentity {
    grants: Mutex<Vec<Grant>>,
}

impl std::fmt::Debug for Grant {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Grant").finish_non_exhaustive()
    }
}

impl MemoryIdentity {
    fn unmounted(method: &'static str) -> StoreError {
        StoreError::Fatal(format!(
            "`{method}` is not reachable from the two routes this deployable mounts"
        ))
    }

    /// Marks the one recorded grant approved, as a person would.
    fn approve(&self) {
        let mut grants = self.grants.lock().expect("not poisoned");
        for recorded in grants.iter_mut() {
            recorded.grant.state = DeviceState::Approved;
            recorded.grant.approved_at = Some(OffsetDateTime::now_utc());
            recorded.grant.approved_by = Some(Uuid::now_v7());
            // Clear the poll bookkeeping so the next poll is not a slow-down.
            recorded.grant.last_polled_at = None;
        }
    }

    fn find(&self, device_id: Uuid, digest: &PresentedDigest) -> Option<DeviceAuthorization> {
        let grants = self.grants.lock().expect("not poisoned");
        let recorded = grants.iter().find(|it| it.grant.id == device_id)?;
        verify(&Pepper::new([7_u8; 32]), digest, &recorded.device_verifier)
            .then(|| recorded.grant.clone())
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
            grant: grant.clone(),
            device_verifier: command.device_verifier,
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
            it.grant.id == command.device_id
                && verify(
                    &Pepper::new([7_u8; 32]),
                    &command.digest,
                    &it.device_verifier,
                )
        }) else {
            return Err(StoreError::NotFound);
        };
        if recorded.grant.state != DeviceState::Approved {
            return Err(StoreError::NotFound);
        }
        recorded.grant.state = DeviceState::Consumed;
        recorded.grant.consumed_at = Some(command.now);
        recorded.grant.account_token_id = Some(command.preassigned_token_id);
        let grant = recorded.grant.clone();
        let token = AccountToken {
            id: command.preassigned_token_id,
            user_id: grant.approved_by.unwrap_or_else(Uuid::now_v7),
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

    async fn resolve_or_create_by_external_identity(
        &self,
        _command: &ResolveExternalIdentity,
    ) -> Result<TxOutcome<ResolvedUser>, StoreError> {
        Err(Self::unmounted("resolve_or_create_by_external_identity"))
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

    async fn create_dashboard_session(
        &self,
        _command: &CreateDashboardSessionCommand,
    ) -> Result<TxOutcome<DashboardSession>, StoreError> {
        Err(Self::unmounted("create_dashboard_session"))
    }

    async fn resolve_dashboard_session(
        &self,
        _query: &ResolveDashboardSessionQuery,
    ) -> Result<Option<(DashboardSession, User)>, StoreError> {
        Err(Self::unmounted("resolve_dashboard_session"))
    }

    async fn revoke_dashboard_session(
        &self,
        _command: &RevokeDashboardSessionCommand,
    ) -> Result<TxOutcome<()>, StoreError> {
        Err(Self::unmounted("revoke_dashboard_session"))
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

    async fn approve_device_authorization(
        &self,
        _command: &DecideDeviceAuthorizationCommand,
    ) -> Result<TxOutcome<DeviceAuthorization>, StoreError> {
        Err(Self::unmounted("approve_device_authorization"))
    }

    async fn deny_device_authorization(
        &self,
        _command: &DecideDeviceAuthorizationCommand,
    ) -> Result<TxOutcome<DeviceAuthorization>, StoreError> {
        Err(Self::unmounted("deny_device_authorization"))
    }

    async fn revoke_account_token(
        &self,
        _command: &RevokeAccountTokenCommand,
    ) -> Result<TxOutcome<()>, StoreError> {
        Err(Self::unmounted("revoke_account_token"))
    }
}

/// The composed router, over the real service and the two in-memory ports.
fn composed(probes: Probes) -> (axum::Router, Arc<MemoryIdentity>) {
    let store = Arc::new(MemoryIdentity::default());
    let clock: Arc<dyn aex_identity_app::ports::Clock> = Arc::new(aex_central_runtime::SystemClock);
    let service = AuthService::new(
        Arc::clone(&store) as Arc<dyn IdentityStore>,
        Arc::new(FixedPepper),
        Arc::clone(&clock),
        Arc::new(aex_central_runtime::Uuid7Factory),
        Arc::new(aex_central_runtime::OsSecretRng),
        aex_wire::types::HttpsUrl::parse("https://aex.dev/device").expect("a valid URL"),
    );
    let config = crate::Config::from_lookup(|name| environment(name).map(str::to_owned))
        .expect("a complete environment");
    let edge = EdgeStack::new(
        config.http,
        Arc::new(NoOrganizationTargets),
        clock,
        Arc::new(CursorSecret::new([3_u8; 32])),
    );
    (app(Arc::new(service), edge, readiness(probes)), store)
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
        "AEX_CENTRAL_IDENTITY_DATABASE" => "aex",
        "AEX_CENTRAL_IDENTITY_ROLE" => "aex_identity_api",
        "AEX_CENTRAL_IDENTITY_DEVICE_VERIFICATION_URI" => "https://aex.dev/device",
        "AEX_CENTRAL_IDENTITY_VERCEL_ISSUER" => "https://oidc.vercel.com/aexhq",
        "AEX_CENTRAL_IDENTITY_VERCEL_EXPECTED_SUBJECT" => "owner:aexhq:project:dashboard",
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

#[tokio::test]
async fn the_composed_binary_mints_a_device_code_a_person_can_approve() {
    let (router, _store) = composed(Probes::READY);
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
    let (router, _store) = composed(Probes::READY);
    let (_, started) = call(&router, start_request()).await;
    let device_code = started["deviceCode"].as_str().expect("a device code");

    let (status, body) = call(&router, token_request(device_code)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        body["error"]["code"], "authorization_pending",
        "a pending grant must not read as a failure the CLI gives up on: {body}"
    );
}

#[tokio::test]
async fn an_approved_grant_redeems_into_exactly_one_account_token() {
    let (router, store) = composed(Probes::READY);
    let (_, started) = call(&router, start_request()).await;
    let device_code = started["deviceCode"]
        .as_str()
        .expect("a device code")
        .to_owned();

    store.approve();
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
async fn a_device_code_that_is_not_this_platform_s_grammar_never_reaches_the_store() {
    let (router, store) = composed(Probes::READY);
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
    let (router, store) = composed(Probes::READY);
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
    let (unready, _) = composed(Probes::NONE);
    let (status, body) = call(
        &unready,
        Request::builder()
            .uri(READY_PATH)
            .body(Body::empty())
            .expect("a valid request"),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");

    let (ready, _) = composed(Probes::READY);
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
    let (router, _) = composed(Probes::READY);
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
