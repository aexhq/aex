//! Google browser-session authentication for the central control service.

use std::sync::Arc;

use aex_control_app::ControlError;
use aex_control_app::personal_account::{PersonalAccountProvisioner, ProvisionPersonalAccount};
use aex_identity_app::ports::{Clock, IdFactory, IdentityStore, PepperKeystore, RequestContext};
use aex_identity_app::use_cases::{
    CloseDashboardSession, IdentityDeps, OpenDashboardSession, ResolveOauthSignIn,
};
use aex_identity_app::{IdentityError, RequestId};
use aex_identity_domain::SecretRng;
use aex_wire::error::{ErrorCode, WireError, WireResult};
use aex_wire::ids::{PrefixedId as _, UserId, Uuid7};
use aex_wire::models::{CliAuthConfig, DashboardSessionCredential, DashboardSessionRequest};
use aex_wire::server::{AuthApi, Created, NoContent};
use aex_wire::types::Timestamp;

use crate::identity_oauth::{
    CLI_REDIRECT_URI, GOOGLE_AUTHORIZATION_ENDPOINT, HandshakeError, ProviderHandshake,
};

/// The public Google configuration and two session operations.
pub struct AuthService {
    store: Arc<dyn IdentityStore>,
    provisioner: Arc<dyn PersonalAccountProvisioner>,
    keystore: Arc<dyn PepperKeystore>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdFactory>,
    rng: Arc<dyn SecretRng>,
    handshake: Arc<dyn ProviderHandshake>,
}

impl std::fmt::Debug for AuthService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthService")
            .finish_non_exhaustive()
    }
}

impl AuthService {
    /// Builds the service over the identity authority and Google handshake.
    #[must_use]
    pub fn new(
        store: Arc<dyn IdentityStore>,
        provisioner: Arc<dyn PersonalAccountProvisioner>,
        keystore: Arc<dyn PepperKeystore>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdFactory>,
        rng: Arc<dyn SecretRng>,
        handshake: Arc<dyn ProviderHandshake>,
    ) -> Self {
        Self {
            store,
            provisioner,
            keystore,
            clock,
            ids,
            rng,
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

    fn context(&self, cx: &aex_wire::server::RequestContext) -> RequestContext {
        RequestContext {
            request_id: RequestId::new(cx.request_id.as_str()),
            now: self.clock.now(),
        }
    }

    fn actor_session_id(cx: &aex_wire::server::RequestContext) -> WireResult<uuid::Uuid> {
        cx.actor_session_id
            .map(|id| uuid::Uuid::from_bytes(*id.as_bytes()))
            .ok_or_else(|| {
                WireError::new(ErrorCode::Unauthenticated)
                    .with_message("the current browser session is required")
            })
    }
}

fn cli_auth_config(client_id: &str) -> WireResult<CliAuthConfig> {
    Ok(CliAuthConfig {
        client_id: client_id.to_owned(),
        authorization_url: aex_wire::types::HttpsUrl::parse(GOOGLE_AUTHORIZATION_ENDPOINT)
            .map_err(|error| {
                WireError::new(ErrorCode::InternalError).with_message(error.to_string())
            })?,
        redirect_uri: CLI_REDIRECT_URI.to_owned(),
    })
}

impl AuthApi for AuthService {
    async fn auth_config_get(
        &self,
        _cx: &aex_wire::server::RequestContext,
    ) -> WireResult<CliAuthConfig> {
        cli_auth_config(self.handshake.public_client_id())
    }

    async fn dashboard_session_create(
        &self,
        cx: &aex_wire::server::RequestContext,
        body: DashboardSessionRequest,
    ) -> WireResult<Created<DashboardSessionCredential>> {
        if !crate::identity_oauth::state_matches(&body.code_verifier, &body.state) {
            return Err(WireError::new(ErrorCode::Unauthenticated)
                .with_message("the PKCE verifier does not match state"));
        }
        let profile = self
            .handshake
            .identify(body.client, &body.code, &body.code_verifier)
            .await
            .map_err(handshake_failure)?;
        let context = self.context(cx);
        let resolved = ResolveOauthSignIn::run(&self.deps(), &context, profile)
            .await
            .map_err(|error| identity_failure(&error))?;
        ProvisionPersonalAccount::run(
            self.provisioner.as_ref(),
            self.ids.as_ref(),
            resolved.user.id,
            context.now,
        )
        .await
        .map_err(|error| personal_account_failure(&error))?;
        let minted = OpenDashboardSession::run(&self.deps(), &context, &resolved.user)
            .await
            .map_err(|error| identity_failure(&error))?;
        Ok(Created(DashboardSessionCredential {
            session: minted.secret.expose().to_owned(),
            user_id: UserId::from_uuid7(Uuid7::from_bytes(*resolved.user.id.as_bytes()).map_err(
                |error| WireError::new(ErrorCode::InternalError).with_message(error.to_string()),
            )?),
            expires_at: Timestamp::from_datetime_trunc_ms(minted.record.expires_at).map_err(
                |error| WireError::new(ErrorCode::InternalError).with_message(error.to_string()),
            )?,
        }))
    }

    async fn dashboard_session_delete(
        &self,
        cx: &aex_wire::server::RequestContext,
    ) -> WireResult<NoContent> {
        let session = Self::actor_session_id(cx)?;
        CloseDashboardSession::run(&self.deps(), &self.context(cx), session)
            .await
            .map_err(|error| identity_failure(&error))?;
        Ok(NoContent)
    }
}

fn personal_account_failure(_error: &ControlError) -> WireError {
    WireError::new(ErrorCode::InternalError)
}

fn identity_failure(error: &IdentityError) -> WireError {
    match error {
        IdentityError::UserDisabled | IdentityError::Revoked => {
            WireError::new(ErrorCode::Forbidden)
        }
        IdentityError::Unauthenticated => WireError::new(ErrorCode::Unauthenticated),
        IdentityError::Conflict { .. } | IdentityError::NotFound => {
            WireError::new(ErrorCode::InvalidRequest)
        }
        _ => WireError::new(ErrorCode::InternalError),
    }
}

fn handshake_failure(error: HandshakeError) -> WireError {
    match error {
        HandshakeError::Refused(_) => WireError::new(ErrorCode::Unauthenticated),
        HandshakeError::Unusable(reason) => {
            WireError::new(ErrorCode::InvalidRequest).with_message(reason)
        }
        HandshakeError::RateLimited => WireError::new(ErrorCode::RateLimited),
        HandshakeError::Unreachable(_) => WireError::new(ErrorCode::InternalError),
    }
}

#[cfg(test)]
mod tests {
    use aex_control_app::ControlError;
    use aex_wire::error::ErrorCode;
    use aex_wire::routes::{RouteId, route};

    use super::{cli_auth_config, personal_account_failure};

    #[test]
    fn provisioning_failure_uses_a_code_declared_by_google_sign_in() {
        let produced = personal_account_failure(&ControlError::Unavailable).code;
        assert_eq!(produced, ErrorCode::InternalError);
        assert!(
            route(RouteId::DashboardSessionCreate)
                .errors
                .contains(&produced)
        );
    }

    #[test]
    fn cli_discovery_returns_only_public_fixed_google_configuration() {
        let config = cli_auth_config("google-client.apps.googleusercontent.com")
            .expect("compiled configuration is valid");
        assert_eq!(config.client_id, "google-client.apps.googleusercontent.com");
        assert_eq!(
            config.authorization_url.as_str(),
            crate::identity_oauth::GOOGLE_AUTHORIZATION_ENDPOINT
        );
        assert_eq!(config.redirect_uri, crate::identity_oauth::CLI_REDIRECT_URI);
        let rendered = serde_json::to_value(config).expect("configuration serializes");
        assert_eq!(
            rendered.as_object().expect("object").keys().count(),
            3,
            "the public response must not grow secret or mutable callback fields"
        );
    }
}
