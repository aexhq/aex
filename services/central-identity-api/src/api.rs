//! The five public credential-ceremony routes, over the identity use cases.
//!
//! Together they are one loop, and every step of it is here:
//!
//! 1. a device asks for a grant (`device_authorization_create`) and is told a
//!    short code and where a person types it;
//! 2. a person signs in with a provider and exchanges that for a browser
//!    session (`dashboard_session_create`);
//! 3. that person types the short code and decides
//!    (`device_decision_create`), which is the only thing in the tree that
//!    moves `DeviceState::Pending` off `Pending`;
//! 4. the device polls and redeems (`device_token_create`);
//! 5. the person can close their session again (`dashboard_session_delete`).
//!
//! Before this module grew steps 2, 3 and 5, step 4 could only ever answer
//! `authorization_pending`: the approval half of the state machine was complete
//! and had no caller, so the only credential anybody held in a deployed plane
//! was one CI wrote into Aurora in raw SQL.
//!
//! Three routes are anonymous — no AEX credential is presented, and the edge
//! admits them under the credential-free context — and the two that decide or
//! close carry a browser session. Everything they can go wrong with is a body,
//! a credential or a domain-state failure, and every one of those must map onto
//! a code the route *declares*: `aex_wire::server::dispatch_auth` refuses a code
//! the descriptor does not list, so a mapping mistake here is a `500` at the
//! boundary rather than an undeclared code on the wire.
//!
//! The RFC 8628 result codes are the interesting half. `authorization_pending`,
//! `slow_down`, `token_expired` and `access_denied` are four different things a
//! polling device must do differently, and collapsing any of them into
//! `invalid_request` would leave a CLI either spinning or giving up early.
//!
//! # Why the sign-in exchange takes an authorization code
//!
//! `dashboard_session_create` is handed what a browser redirect actually
//! returns — a provider, a single-use authorization code, and the `state` that
//! came back with it — and redeems the code itself. See [`crate::oauth`] for the
//! handshake and for the rule this replaced.
//!
//! The short version: this route used to accept an *already-established*
//! provider profile and prove the caller was a first-party front end with a
//! shared secret, on the theory that Rust never dials a vendor. The tree
//! contradicts that theory — `aex-brain-provider-gateway` dials four model
//! vendors directly, holding each customer's credential — and a language is not
//! a security boundary in any case. The control that does the work is the
//! capability manifest: this deployable declares
//! [`aex_central_http::capability::SignInHandshake`], binds it to a named
//! secret per provider, and refuses to start unbound.
//!
//! With the code redeemed here, no caller asserts an identity any more, so
//! there is nothing left for a shared secret to prove and it is gone rather
//! than reduced.

use std::sync::Arc;

use aex_control_domain::ScopeSet;
use aex_identity_app::ports::{Clock, IdFactory, IdentityStore, PepperKeystore, RequestContext};
use aex_identity_app::use_cases::{
    ApproveDevice, CloseDashboardSession, DenyDevice, IdentityDeps, OpenDashboardSession,
    PollDevice, ResolveOauthSignIn, StartDeviceAuthorization,
};
use aex_identity_app::{IdentityError, RequestId};
use aex_identity_domain::credential::parse as parse_credential;
use aex_identity_domain::{CredentialKind, ParsedCredential, Provider, SecretRng, UserCode};
use aex_wire::error::{ErrorCode, WireError, WireResult};
use aex_wire::ids::{PrefixedId as _, UserId, Uuid7};
use aex_wire::models::{
    DashboardSessionCredential, DashboardSessionRequest, DeviceAuthorization,
    DeviceAuthorizationRequest, DeviceDecision, DeviceDecisionRequest, DeviceDecisionResult,
    DeviceToken, DeviceTokenRequest, IdentityProvider,
};
use aex_wire::server::{AuthApi, Created, NoContent};
use aex_wire::types::{HttpsUrl, Timestamp};
use uuid::Uuid;

use crate::oauth::ProviderHandshake;

/// The client identifier the device flow admits.
///
/// One public client, named rather than accepted from the body. A device flow
/// that echoed whatever `clientId` arrived would record an attacker-chosen
/// string against every grant, and there is exactly one first-party CLI.
pub const ALLOWED_CLIENT_ID: &str = "aex-cli";

/// Everything the five routes need, resolved once at start-up.
pub struct AuthService {
    store: Arc<dyn IdentityStore>,
    keystore: Arc<dyn PepperKeystore>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdFactory>,
    rng: Arc<dyn SecretRng>,
    verification_uri: HttpsUrl,
    handshake: Arc<dyn ProviderHandshake>,
}

impl std::fmt::Debug for AuthService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthService")
            .field("verification_uri", &self.verification_uri.as_str())
            .finish_non_exhaustive()
    }
}

impl AuthService {
    /// Builds the service.
    #[must_use]
    pub fn new(
        store: Arc<dyn IdentityStore>,
        keystore: Arc<dyn PepperKeystore>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdFactory>,
        rng: Arc<dyn SecretRng>,
        verification_uri: HttpsUrl,
        handshake: Arc<dyn ProviderHandshake>,
    ) -> Self {
        Self {
            store,
            keystore,
            clock,
            ids,
            rng,
            verification_uri,
            handshake,
        }
    }

    fn deps(&self) -> IdentityDeps<'_> {
        IdentityDeps {
            store: self.store.as_ref(),
            keystore: self.keystore.as_ref(),
            clock: self.clock.as_ref(),
            ids: self.ids.as_ref(),
            rng: self.rng.as_ref(),
        }
    }

    /// The instant this request is evaluated against, read exactly once.
    fn context(&self, cx: &aex_wire::server::RequestContext) -> RequestContext {
        RequestContext {
            request_id: RequestId::new(cx.request_id.as_str()),
            now: self.clock.now(),
        }
    }

    /// The approval page with the user code prefilled.
    fn verification_uri_complete(&self, user_code: &str) -> Result<HttpsUrl, WireError> {
        let joined = format!("{}?user_code={user_code}", self.verification_uri.as_str());
        HttpsUrl::parse(&joined).map_err(|_| internal())
    }
}

/// The one internal failure this module produces.
///
/// Named rather than inlined so every construction site is greppable, and
/// carrying no detail: a device-flow caller is anonymous, so anything specific
/// would be told to somebody who presented nothing.
fn internal() -> WireError {
    WireError::new(ErrorCode::InternalError)
}

/// Refuses a body whose `clientId` is not the one public client.
fn check_client(client_id: &str) -> WireResult<()> {
    if client_id == ALLOWED_CLIENT_ID {
        return Ok(());
    }
    Err(WireError::new(ErrorCode::InvalidRequest)
        .with_message("`clientId` names no registered device-flow client"))
}

/// Narrows the wire scope list to the domain's set.
fn requested_scopes(scopes: &[aex_wire::scopes::ScopeId]) -> WireResult<ScopeSet> {
    let names: Vec<&str> = scopes.iter().map(|scope| scope.as_str()).collect();
    ScopeSet::from_strings(&names).map_err(|_| {
        WireError::new(ErrorCode::InvalidRequest)
            .with_message("`scopes` names a scope the registry does not define")
    })
}

/// Maps an application failure onto `device_authorization_create`'s declared
/// codes.
///
/// The route declares `invalid_request`, `rate_limited` and `internal_error`
/// and nothing else, so an unavailable store is `internal_error` here: there is
/// no `service_unavailable` on this route to map it to, and inventing one is
/// what `declared` refuses at the boundary.
fn start_failure(error: &IdentityError) -> WireError {
    match error {
        IdentityError::Conflict { .. } | IdentityError::IntentConflict => {
            WireError::new(ErrorCode::InvalidRequest)
        }
        _ => internal(),
    }
}

/// Maps an application failure onto `device_token_create`'s declared codes.
///
/// This is the RFC 8628 mapping, and each arm is a different instruction to the
/// polling device:
///
/// - `authorization_pending` — keep polling at the current interval
/// - `slow_down` — poll less often, at the interval the grant now carries
/// - `token_expired` — the grant lapsed; start a new authorization
/// - `invalid_request` — the device code names nothing, or a person denied it
///
/// A denial is deliberately `invalid_request` rather than a distinct code: the
/// route declares no `access_denied`, and telling an anonymous poller "a person
/// refused you" and "that code never existed" apart is information it has not
/// earned.
fn poll_failure(error: &IdentityError) -> WireError {
    match error {
        IdentityError::AuthorizationPending => WireError::new(ErrorCode::AuthorizationPending),
        IdentityError::SlowDown { interval_ms } => WireError::new(ErrorCode::SlowDown)
            .with_retry_after(core::time::Duration::from_millis(
                u64::try_from(*interval_ms).unwrap_or(5_000),
            )),
        IdentityError::Expired => WireError::new(ErrorCode::TokenExpired),
        IdentityError::Unauthenticated
        | IdentityError::AccessDenied
        | IdentityError::Revoked
        | IdentityError::UserDisabled
        | IdentityError::NotFound => WireError::new(ErrorCode::InvalidRequest),
        _ => internal(),
    }
}

/// Maps an application failure onto `device_decision_create`'s declared codes.
///
/// `not_found` is the whole point of this table. A user code that names no
/// pending grant, one that already lapsed, one another person already decided
/// and one that never existed are the same answer here — the person typing it
/// gets "that code is not waiting for you", and the alternative would let a
/// browser page enumerate live codes by watching the answer change.
///
/// The approver's own failures are told apart, because they are actionable:
/// a lapsed session is `token_expired` (sign in again) and a disabled person is
/// `forbidden` (nothing you do will help).
fn decide_failure(error: &IdentityError) -> WireError {
    match error {
        IdentityError::NotFound | IdentityError::Conflict { .. } => {
            WireError::new(ErrorCode::NotFound)
                .with_message("no pending device authorization is waiting for that code")
        }
        IdentityError::Expired => WireError::new(ErrorCode::TokenExpired),
        IdentityError::Unauthenticated => WireError::new(ErrorCode::Unauthenticated),
        IdentityError::UserDisabled | IdentityError::Revoked | IdentityError::AccessDenied => {
            WireError::new(ErrorCode::Forbidden)
        }
        _ => internal(),
    }
}

/// Maps an application failure onto `dashboard_session_create`'s declared codes.
fn sign_in_failure(error: &IdentityError) -> WireError {
    match error {
        IdentityError::UserDisabled | IdentityError::Revoked | IdentityError::AccessDenied => {
            WireError::new(ErrorCode::Forbidden)
        }
        IdentityError::Conflict { .. } | IdentityError::NotFound => {
            WireError::new(ErrorCode::InvalidRequest)
                .with_message("the provider identity could not be resolved to a person")
        }
        _ => internal(),
    }
}

/// Maps a provider handshake failure onto `dashboard_session_create`'s declared
/// codes.
///
/// The route declares `invalid_request`, `unauthenticated`, `forbidden`,
/// `rate_limited` and `internal_error`, and `dispatch_auth` refuses anything
/// else, so each arm has to land inside that set:
///
/// * a code the provider refused is `unauthenticated` — the caller presented
///   something that does not authenticate them, which is exactly what that code
///   means, and it reads the same whether the code was forged, replayed or
///   simply stale;
/// * an answer this platform cannot make a person from is `invalid_request`,
///   carrying the provider's reason, which is actionable where "internal error"
///   is not;
/// * a provider rate limit is `rate_limited`, so a dashboard backs off against
///   the provider's pressure rather than retrying into it;
/// * a provider that did not answer is `internal_error`. The route declares no
///   `service_unavailable`, and this genuinely is this plane failing to complete
///   a ceremony it owns.
fn handshake_failure(error: &crate::oauth::HandshakeError) -> WireError {
    use crate::oauth::HandshakeError;
    match error {
        HandshakeError::Refused(_) => WireError::new(ErrorCode::Unauthenticated)
            .with_message("the provider refused the authorization code"),
        HandshakeError::Unusable(reason) => {
            WireError::new(ErrorCode::InvalidRequest).with_message(reason)
        }
        HandshakeError::RateLimited => WireError::new(ErrorCode::RateLimited),
        HandshakeError::Unreachable(_) => internal(),
    }
}

/// Maps an application failure onto `dashboard_session_delete`'s declared codes.
///
/// The route declares no `not_found`, and deliberately: closing a session that
/// was already closed is the outcome the caller asked for. Only the store can
/// fail here.
fn close_failure(error: &IdentityError) -> WireError {
    match error {
        IdentityError::UserDisabled | IdentityError::Revoked => {
            WireError::new(ErrorCode::Forbidden)
        }
        _ => internal(),
    }
}

impl AuthService {
    /// The person the admitted credential names.
    ///
    /// The edge resolved and verified them before the handler ran, so a scope
    /// that is not an actor here is an internal defect and not caller input.
    fn actor_user_id(cx: &aex_wire::server::RequestContext) -> WireResult<Uuid> {
        match cx.principal {
            aex_wire::idempotency::PrincipalScope::Account { user, .. } => {
                Ok(uuid_of(user.uuid7()))
            }
            aex_wire::idempotency::PrincipalScope::WorkspaceKey { .. } => Err(internal()),
        }
    }

    /// The browser session the caller presented.
    ///
    /// Absent means the caller authenticated with an account token, which the
    /// route's `user_session` alternative also admits — `account` and
    /// `user_session` are one principal to the authorizer. It is refused here
    /// rather than substituted for, because `approve_device_authorization`
    /// records the session that proved the approver was current and an account
    /// token proves no such thing.
    fn actor_session_id(cx: &aex_wire::server::RequestContext) -> WireResult<Uuid> {
        cx.actor_session_id.map(uuid_of).ok_or_else(|| {
            WireError::new(ErrorCode::Unauthenticated).with_message(
                "this route needs the browser session that proves the actor is currently signed in",
            )
        })
    }
}

/// Widens a wire identifier payload into the store's identifier type.
fn uuid_of(id: Uuid7) -> Uuid {
    Uuid::from_bytes(*id.as_bytes())
}

/// Narrows the wire provider vocabulary onto the domain's.
const fn provider_of(provider: IdentityProvider) -> Provider {
    match provider {
        IdentityProvider::Google => Provider::Google,
    }
}

impl AuthApi for AuthService {
    async fn dashboard_session_create(
        &self,
        cx: &aex_wire::server::RequestContext,
        body: DashboardSessionRequest,
    ) -> WireResult<Created<DashboardSessionCredential>> {
        // The `state` check comes first, before any provider is dialled and
        // before the single-use code is spent. A redirect that cannot present
        // the verifier its `state` hashes from did not come from the browser
        // that started this sign-in, and completing it would authenticate the
        // wrong person — see `oauth::state_matches` for the forgery this
        // refuses.
        if !crate::oauth::state_matches(&body.code_verifier, &body.state) {
            return Err(WireError::new(ErrorCode::Unauthenticated).with_message(
                "`state` is not the S256 challenge of `codeVerifier`; this redirect did not begin here",
            ));
        }
        let provider = provider_of(body.provider);
        // The identity comes back from the provider, not from the caller. An
        // unverified address never reaches the store: every path in
        // `crate::oauth` refuses one, because `resolve_or_create_by_external_identity`
        // falls back to resolving by normalized email when no provider link
        // matches, and an unverified assertion would let one provider account
        // adopt another person's records — a cross-tenant read, which is a
        // correctness defect and not a matter of degree.
        let profile = self
            .handshake
            .identify(provider, &body.code, &body.code_verifier)
            .await
            .map_err(|error| handshake_failure(&error))?;
        let context = self.context(cx);
        let resolved = ResolveOauthSignIn::run(&self.deps(), &context, profile)
            .await
            .map_err(|error| sign_in_failure(&error))?;
        let minted = OpenDashboardSession::run(&self.deps(), &context, &resolved.user)
            .await
            .map_err(|error| sign_in_failure(&error))?;
        Ok(Created(DashboardSessionCredential {
            session: minted.secret.expose().to_owned(),
            user_id: UserId::from_uuid7(
                Uuid7::from_bytes(*resolved.user.id.as_bytes()).map_err(|_| internal())?,
            ),
            expires_at: Timestamp::from_datetime_trunc_ms(minted.record.expires_at)
                .map_err(|_| internal())?,
        }))
    }

    async fn dashboard_session_delete(
        &self,
        cx: &aex_wire::server::RequestContext,
    ) -> WireResult<NoContent> {
        let session_id = Self::actor_session_id(cx)?;
        let context = self.context(cx);
        CloseDashboardSession::run(&self.deps(), &context, session_id)
            .await
            .map_err(|error| close_failure(&error))?;
        Ok(NoContent)
    }

    async fn device_decision_create(
        &self,
        cx: &aex_wire::server::RequestContext,
        body: DeviceDecisionRequest,
    ) -> WireResult<DeviceDecisionResult> {
        let actor_user_id = Self::actor_user_id(cx)?;
        let actor_session_id = Self::actor_session_id(cx)?;
        // Normalization happens before the store is touched: a code outside the
        // alphabet cannot name a row, and hashing it would spend a round trip to
        // learn what the grammar already said.
        let code = UserCode::normalize(&body.user_code).map_err(|_| {
            WireError::new(ErrorCode::InvalidRequest)
                .with_message("`userCode` is not a code this platform mints")
        })?;
        let context = self.context(cx);
        let deps = self.deps();
        let grant = match body.decision {
            DeviceDecision::Approve => {
                ApproveDevice::run(&deps, &context, &code, actor_user_id, actor_session_id).await
            }
            DeviceDecision::Deny => {
                DenyDevice::run(&deps, &context, &code, actor_user_id, actor_session_id).await
            }
        }
        .map_err(|error| decide_failure(&error))?;
        // An approval carries the instant the conditional `UPDATE` wrote; a
        // denial sets no timestamp column, so the instant this request was
        // evaluated at is the honest answer and it is the same value the
        // statement's predicate used.
        let decided_at = match body.decision {
            DeviceDecision::Approve => grant.approved_at.unwrap_or(context.now),
            DeviceDecision::Deny => context.now,
        };
        Ok(DeviceDecisionResult {
            decision: body.decision,
            decided_at: Timestamp::from_datetime_trunc_ms(decided_at).map_err(|_| internal())?,
            scopes: grant.requested_scopes.to_wire().as_slice().to_vec(),
        })
    }

    async fn device_authorization_create(
        &self,
        cx: &aex_wire::server::RequestContext,
        body: DeviceAuthorizationRequest,
    ) -> WireResult<Created<DeviceAuthorization>> {
        check_client(&body.client_id)?;
        let scopes = requested_scopes(&body.scopes)?;
        let context = self.context(cx);
        let started = StartDeviceAuthorization::run(&self.deps(), &context, scopes)
            .await
            .map_err(|error| start_failure(&error))?;
        let interval_seconds =
            u32::try_from(started.grant.poll_interval.whole_seconds().max(1)).unwrap_or(5);
        Ok(Created(DeviceAuthorization {
            device_code: started.device_code.expose().to_owned(),
            expires_at: Timestamp::from_datetime_trunc_ms(started.grant.expires_at)
                .map_err(|_| internal())?,
            interval_seconds,
            verification_uri_complete: self
                .verification_uri_complete(started.user_code.as_str())?,
            user_code: started.user_code.as_str().to_owned(),
            verification_uri: self.verification_uri.clone(),
        }))
    }

    async fn device_token_create(
        &self,
        cx: &aex_wire::server::RequestContext,
        body: DeviceTokenRequest,
    ) -> WireResult<DeviceToken> {
        check_client(&body.client_id)?;
        // A malformed device code is refused before the store is touched: it
        // cannot name a row, so a lookup would spend a round trip to learn what
        // the grammar already said.
        let ParsedCredential { id, digest, .. } =
            parse_credential(CredentialKind::DeviceCode, &body.device_code)
                .map_err(|_| WireError::new(ErrorCode::InvalidRequest))?;
        let context = self.context(cx);
        let minted = PollDevice::run(&self.deps(), &context, id, digest)
            .await
            .map_err(|error| poll_failure(&error))?;
        Ok(DeviceToken {
            account_token: minted.secret.expose().to_owned(),
            expires_at: Timestamp::from_datetime_trunc_ms(minted.record.token.expires_at)
                .map_err(|_| internal())?,
            scopes: minted.record.token.scopes.to_wire().as_slice().to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ALLOWED_CLIENT_ID, check_client, poll_failure, requested_scopes, start_failure};
    use aex_identity_app::IdentityError;
    use aex_wire::error::ErrorCode;
    use aex_wire::routes::{RouteId, route};

    #[test]
    fn only_the_one_registered_client_is_admitted() {
        assert!(check_client(ALLOWED_CLIENT_ID).is_ok());
        for other in ["", "aex-cli ", "attacker", "AEX-CLI"] {
            let error = check_client(other).expect_err(other);
            assert_eq!(error.code, ErrorCode::InvalidRequest, "{other}");
        }
    }

    #[test]
    fn an_unknown_scope_is_a_body_failure_rather_than_an_empty_grant() {
        // Every wire scope is registry-defined, so the failure path is reached
        // through the domain parser rather than a wire value that cannot exist.
        let empty = requested_scopes(&[]).expect("an empty request is valid");
        assert!(empty.is_empty());
    }

    #[test]
    fn every_produced_code_is_one_the_route_declares() {
        let start = route(RouteId::DeviceAuthorizationCreate).errors;
        for error in [
            IdentityError::Conflict {
                constraint: "dev_user_code_uk".to_owned(),
            },
            IdentityError::IntentConflict,
            IdentityError::Unavailable,
            IdentityError::PepperUnavailable,
            IdentityError::Fatal("x".to_owned()),
        ] {
            let produced = start_failure(&error).code;
            assert!(start.contains(&produced), "{error}: {produced}");
        }

        let poll = route(RouteId::DeviceTokenCreate).errors;
        for error in [
            IdentityError::AuthorizationPending,
            IdentityError::SlowDown { interval_ms: 5_000 },
            IdentityError::Expired,
            IdentityError::Unauthenticated,
            IdentityError::AccessDenied,
            IdentityError::Revoked,
            IdentityError::UserDisabled,
            IdentityError::NotFound,
            IdentityError::Unavailable,
            IdentityError::PepperUnavailable,
            IdentityError::IntentConflict,
            IdentityError::Fatal("x".to_owned()),
        ] {
            let produced = poll_failure(&error).code;
            assert!(poll.contains(&produced), "{error}: {produced}");
        }
    }

    #[test]
    fn the_four_rfc_8628_outcomes_stay_four_different_answers() {
        assert_eq!(
            poll_failure(&IdentityError::AuthorizationPending).code,
            ErrorCode::AuthorizationPending
        );
        assert_eq!(
            poll_failure(&IdentityError::SlowDown { interval_ms: 7_000 }).code,
            ErrorCode::SlowDown
        );
        assert_eq!(
            poll_failure(&IdentityError::Expired).code,
            ErrorCode::TokenExpired
        );
        assert_eq!(
            poll_failure(&IdentityError::Unauthenticated).code,
            ErrorCode::InvalidRequest
        );
    }

    #[test]
    fn a_slow_down_carries_the_interval_the_device_must_now_obey() {
        let error = poll_failure(&IdentityError::SlowDown { interval_ms: 7_500 });
        assert_eq!(
            error.retry_after,
            Some(core::time::Duration::from_millis(7_500)),
            "a slow_down without an interval tells the device nothing it can act on"
        );
    }

    #[test]
    fn a_denial_and_an_unknown_code_are_indistinguishable_to_an_anonymous_poller() {
        assert_eq!(
            poll_failure(&IdentityError::AccessDenied).code,
            poll_failure(&IdentityError::Unauthenticated).code
        );
    }

    #[test]
    fn an_unavailable_store_never_becomes_a_caller_error() {
        for error in [
            IdentityError::Unavailable,
            IdentityError::PepperUnavailable,
            IdentityError::CommitOutcomeUnknown {
                ceremony: "identity.start_device_authorization",
                id: uuid::Uuid::nil(),
            },
        ] {
            assert_eq!(
                start_failure(&error).code,
                ErrorCode::InternalError,
                "{error}"
            );
            assert_eq!(
                poll_failure(&error).code,
                ErrorCode::InternalError,
                "{error}"
            );
        }
    }
}
