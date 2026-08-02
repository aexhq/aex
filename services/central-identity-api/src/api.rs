//! The two public device-flow routes, over the identity use cases.
//!
//! Both routes are anonymous: no credential is presented, and the edge admits
//! them under the credential-free context. Everything they can go wrong with is
//! therefore a body or a domain-state failure, and every one of those must map
//! onto a code the route *declares* — `aex_wire::server::dispatch_auth` refuses
//! a code the descriptor does not list, so a mapping mistake here is a `500` at
//! the boundary rather than an undeclared code on the wire.
//!
//! The RFC 8628 result codes are the interesting half. `authorization_pending`,
//! `slow_down`, `token_expired` and `access_denied` are four different things a
//! polling device must do differently, and collapsing any of them into
//! `invalid_request` would leave a CLI either spinning or giving up early.

use std::sync::Arc;

use aex_control_domain::ScopeSet;
use aex_identity_app::ports::{Clock, IdFactory, IdentityStore, PepperKeystore, RequestContext};
use aex_identity_app::use_cases::{IdentityDeps, PollDevice, StartDeviceAuthorization};
use aex_identity_app::{IdentityError, RequestId};
use aex_identity_domain::credential::parse as parse_credential;
use aex_identity_domain::{CredentialKind, ParsedCredential, SecretRng};
use aex_wire::error::{ErrorCode, WireError, WireResult};
use aex_wire::models::{
    DeviceAuthorization, DeviceAuthorizationRequest, DeviceToken, DeviceTokenRequest,
};
use aex_wire::server::{AuthApi, Created};
use aex_wire::types::{HttpsUrl, Timestamp};

/// The client identifier the device flow admits.
///
/// One public client, named rather than accepted from the body. A device flow
/// that echoed whatever `clientId` arrived would record an attacker-chosen
/// string against every grant, and there is exactly one first-party CLI.
pub const ALLOWED_CLIENT_ID: &str = "aex-cli";

/// Everything the two routes need, resolved once at start-up.
pub struct AuthService {
    store: Arc<dyn IdentityStore>,
    keystore: Arc<dyn PepperKeystore>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdFactory>,
    rng: Arc<dyn SecretRng>,
    verification_uri: HttpsUrl,
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
    ) -> Self {
        Self {
            store,
            keystore,
            clock,
            ids,
            rng,
            verification_uri,
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

impl AuthApi for AuthService {
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
