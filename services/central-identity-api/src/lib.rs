//! `central-identity-api`: the identity half of the central HTTP surface.
//!
//! The whole credential ceremony: the two public device-flow routes, the
//! decision that moves a grant off `pending`, and the browser-session mint and
//! close. It writes identity DML and holds exactly one control privilege —
//! `control.bump_user_epoch` — so a disabled person's assertions stop verifying
//! in the same transaction that disables them.
//!
//! The mounted public surface is `CentralServiceId::IdentityApi.routes()` and
//! nothing else, which `the_mounted_set_is_exactly_the_declared_one` asserts.
//!
//! # Why this is a library and not only a binary
//!
//! `services/central-api` composes the `central:auth` routes into one
//! long-lived Fargate process alongside the control and billing groups. A
//! deployable whose service type lives in a module private to its own `main.rs`
//! cannot be composed into another binary at all, so [`api::AuthService`], the
//! configuration reader, the capability manifest and the readiness projection
//! are library items and `src/main.rs` is the Lambda composition root over them.

pub mod account;
pub mod api;
pub mod oauth;
mod startup;
pub mod targets;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use aex_central_http::capability::{
    Capability as _, CapabilityBinding, CompositionError, CompositionManifest, Declares,
    IdentityWrite, SignInHandshake,
};
use aex_central_http::config::{CentralServiceId, HttpConfig};
use aex_central_http::health::{Dependency, Readiness};
use aex_central_http::router::{EdgeStack, mount_auth_api, mount_identity_api};
use aex_wire::server::AuthApi;

/// The deployable this crate is.
pub const DEPLOYABLE: CentralServiceId = CentralServiceId::IdentityApi;

/// The one login role this binary may connect as.
const REQUIRED_ROLE: &str = "aex_identity_api";

/// Environment keys, all inside the declared namespace.
pub mod keys {
    /// The plane's account id, for the ARN binding check.
    pub const ACCOUNT_ID: &str = "AEX_CENTRAL_IDENTITY_ACCOUNT_ID";
    /// The Aurora cluster holding the identity schema.
    pub const AURORA_CLUSTER_ARN: &str = "AEX_CENTRAL_IDENTITY_AURORA_CLUSTER_ARN";
    /// The Aurora credentials secret.
    pub const AURORA_SECRET_ARN: &str = "AEX_CENTRAL_IDENTITY_AURORA_SECRET_ARN";
    /// The database name.
    pub const DATABASE: &str = "AEX_CENTRAL_IDENTITY_DATABASE";
    /// Where a person approves a device authorization.
    pub const DEVICE_VERIFICATION_URI: &str = "AEX_CENTRAL_IDENTITY_DEVICE_VERIFICATION_URI";
    /// The largest request body this composition accepts.
    pub const MAX_BODY_BYTES: &str = "AEX_CENTRAL_IDENTITY_MAX_BODY_BYTES";
    /// The identity credential pepper secret.
    pub const PEPPER_SECRET_ID: &str = "AEX_CENTRAL_IDENTITY_PEPPER_SECRET_ID";
    /// The deployment plane.
    pub const PLANE: &str = "AEX_CENTRAL_IDENTITY_PLANE";
    /// The bound region.
    pub const REGION: &str = "AEX_CENTRAL_IDENTITY_REGION";
    /// How long one request may take.
    pub const REQUEST_DEADLINE_MS: &str = "AEX_CENTRAL_IDENTITY_REQUEST_DEADLINE_MS";
    /// The login role. Must be `aex_identity_api`.
    pub const ROLE: &str = "AEX_CENTRAL_IDENTITY_ROLE";
    /// The secret holding GitHub's registered OAuth client.
    ///
    /// A JSON object with `clientId` and `clientSecret`. Bound to
    /// [`SignInHandshake`], so a deployable that did not declare that capability
    /// cannot be handed it.
    pub const GITHUB_OAUTH_SECRET_ID: &str = "AEX_CENTRAL_IDENTITY_GITHUB_OAUTH_SECRET_ID";
    /// The secret holding Google's registered OAuth client, in the same shape.
    ///
    /// One secret per provider rather than one holding both: rotating GitHub's
    /// client must not require touching Google's, and the manifest names each
    /// credential this deployable may hold on its own line.
    pub const GOOGLE_OAUTH_SECRET_ID: &str = "AEX_CENTRAL_IDENTITY_GOOGLE_OAUTH_SECRET_ID";
    /// Where a provider sends the browser back after a person authorizes.
    ///
    /// Configured rather than accepted from the request body, and sent verbatim
    /// to the token endpoint. A caller that could choose it could aim a redeemed
    /// code at any URI the provider happens to have registered; a plane that
    /// names exactly one has nothing to choose. Not a manifest binding: it is a
    /// public URL, not a credential.
    pub const SIGN_IN_REDIRECT_URI: &str = "AEX_CENTRAL_IDENTITY_SIGN_IN_REDIRECT_URI";

    /// Every key this binary reads, for the totality test.
    pub const ALL: &[&str] = &[
        ACCOUNT_ID,
        AURORA_CLUSTER_ARN,
        AURORA_SECRET_ARN,
        DATABASE,
        DEVICE_VERIFICATION_URI,
        GITHUB_OAUTH_SECRET_ID,
        GOOGLE_OAUTH_SECRET_ID,
        MAX_BODY_BYTES,
        PEPPER_SECRET_ID,
        PLANE,
        REGION,
        REQUEST_DEADLINE_MS,
        ROLE,
        SIGN_IN_REDIRECT_URI,
    ];
}

/// Why `central-identity-api` refused to start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CentralIdentityApiConfigError {
    /// A required variable was absent or blank.
    #[error("required environment variable `{0}` is missing")]
    Missing(&'static str),
    /// A required variable was present but unusable.
    #[error("environment variable `{name}` is invalid: {reason}")]
    Invalid {
        /// Which variable.
        name: &'static str,
        /// Why it was refused.
        reason: String,
    },
}

/// Validated start-up configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// The shared `HTTP` composition configuration.
    pub http: HttpConfig,
    /// The plane's account id.
    pub account_id: String,
    /// The Aurora cluster.
    pub aurora_cluster_arn: String,
    /// The Aurora credentials secret.
    pub aurora_secret_arn: String,
    /// The identity pepper secret.
    pub pepper_secret_id: String,
    /// The database name.
    pub database: String,
    /// The login role.
    pub role: String,
    /// Where a person approves a device authorization.
    pub device_verification_uri: String,
    /// The secret holding GitHub's registered OAuth client.
    pub github_oauth_secret_id: String,
    /// The secret holding Google's registered OAuth client.
    pub google_oauth_secret_id: String,
    /// Where a provider sends the browser back after a person authorizes.
    pub sign_in_redirect_uri: String,
}

impl Config {
    /// Reads and validates the configuration from the process environment.
    ///
    /// # Errors
    ///
    /// Returns [`CentralIdentityApiConfigError`] naming the first variable it refused.
    pub fn from_env() -> Result<Self, CentralIdentityApiConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Reads and validates from an arbitrary lookup.
    ///
    /// Tests use this directly: `std::env::set_var` is `unsafe` in edition 2024
    /// and this workspace forbids `unsafe` code.
    ///
    /// # Errors
    ///
    /// Identical to [`Config::from_env`].
    pub fn from_lookup<F>(lookup: F) -> Result<Self, CentralIdentityApiConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let role = required(&lookup, keys::ROLE)?;
        if role != REQUIRED_ROLE {
            return Err(CentralIdentityApiConfigError::Invalid {
                name: keys::ROLE,
                reason: format!("this binary connects only as `{REQUIRED_ROLE}`, got `{role}`"),
            });
        }
        let http = HttpConfig::resolve(
            &required(&lookup, keys::PLANE)?,
            &required(&lookup, keys::REGION)?,
            DEPLOYABLE.as_str(),
            integer(&lookup, keys::MAX_BODY_BYTES)?,
            integer(&lookup, keys::REQUEST_DEADLINE_MS)?,
        )
        .map_err(|reason| CentralIdentityApiConfigError::Invalid {
            name: keys::PLANE,
            reason: reason.to_string(),
        })?;
        Ok(Self {
            http,
            account_id: required(&lookup, keys::ACCOUNT_ID)?,
            aurora_cluster_arn: required(&lookup, keys::AURORA_CLUSTER_ARN)?,
            aurora_secret_arn: required(&lookup, keys::AURORA_SECRET_ARN)?,
            pepper_secret_id: required(&lookup, keys::PEPPER_SECRET_ID)?,
            database: required(&lookup, keys::DATABASE)?,
            role,
            device_verification_uri: required(&lookup, keys::DEVICE_VERIFICATION_URI)?,
            github_oauth_secret_id: required(&lookup, keys::GITHUB_OAUTH_SECRET_ID)?,
            google_oauth_secret_id: required(&lookup, keys::GOOGLE_OAUTH_SECRET_ID)?,
            sign_in_redirect_uri: https_url(&lookup, keys::SIGN_IN_REDIRECT_URI)?,
        })
    }

    /// The resolved values the composition check runs over.
    #[must_use]
    pub fn resolved(&self) -> aex_central_http::capability::ResolvedConfig {
        aex_central_http::capability::ResolvedConfig {
            deployable: DEPLOYABLE.as_str().to_owned(),
            plane: self.http.plane,
            region: self.http.region,
            account_id: self.account_id.clone(),
            values: BTreeMap::from([
                (
                    keys::AURORA_CLUSTER_ARN.to_owned(),
                    self.aurora_cluster_arn.clone(),
                ),
                (
                    keys::PEPPER_SECRET_ID.to_owned(),
                    self.pepper_secret_id.clone(),
                ),
                (
                    keys::GITHUB_OAUTH_SECRET_ID.to_owned(),
                    self.github_oauth_secret_id.clone(),
                ),
                (
                    keys::GOOGLE_OAUTH_SECRET_ID.to_owned(),
                    self.google_oauth_secret_id.clone(),
                ),
            ]),
        }
    }
}

fn required<F>(lookup: &F, name: &'static str) -> Result<String, CentralIdentityApiConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    match lookup(name) {
        Some(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(CentralIdentityApiConfigError::Missing(name)),
    }
}

fn integer<F>(lookup: &F, name: &'static str) -> Result<u64, CentralIdentityApiConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let raw = required(lookup, name)?;
    raw.parse::<u64>()
        .map_err(|_| CentralIdentityApiConfigError::Invalid {
            name,
            reason: format!("expected an integer, got `{raw}`"),
        })
}

/// Reads a value that must be an `https` URL.
///
/// Validated here rather than at the first sign-in: a redirect URI that does not
/// parse is a start-up fact, and discovering it when a person clicks a provider
/// button costs an outage nobody can attribute.
fn https_url<F>(lookup: &F, name: &'static str) -> Result<String, CentralIdentityApiConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let raw = required(lookup, name)?;
    aex_wire::types::HttpsUrl::parse(&raw)
        .map(|url| url.as_str().to_owned())
        .map_err(|error| CentralIdentityApiConfigError::Invalid {
            name,
            reason: error.to_string(),
        })
}

/// This binary's capability declaration.
///
/// Identity DML, one pepper, and the two sign-in providers' OAuth clients. No
/// control write, no queue, no object store, no payment provider and no regional
/// invoke: a binding for any of them is refused at start-up.
///
/// [`SignInHandshake`] is declared separately from [`IdentityWrite`] rather than
/// folded into it, because they are genuinely two rights over two authorities:
/// one writes rows in this platform's own schema, the other holds a credential
/// that authenticates this platform *to somebody else*. A deployable that needs
/// to create a user does not thereby need to be able to speak as AEX at GitHub,
/// and the manifest is where that distinction is enforceable rather than
/// merely stated.
#[allow(
    dead_code,
    reason = "the declaration is the capability list; its only use is the type-level `Declares` bound"
)]
struct Composition;

impl Declares<IdentityWrite> for Composition {}
impl Declares<SignInHandshake> for Composition {}

/// The manifest the start-up check runs against.
#[must_use]
pub fn manifest() -> CompositionManifest {
    CompositionManifest {
        deployable: DEPLOYABLE,
        capabilities: BTreeSet::from([IdentityWrite::ID, SignInHandshake::ID]),
        bindings: vec![
            CapabilityBinding::arn(keys::AURORA_CLUSTER_ARN, IdentityWrite::ID),
            CapabilityBinding::resource(keys::PEPPER_SECRET_ID, IdentityWrite::ID),
            CapabilityBinding::resource(keys::GITHUB_OAUTH_SECRET_ID, SignInHandshake::ID),
            CapabilityBinding::resource(keys::GOOGLE_OAUTH_SECRET_ID, SignInHandshake::ID),
        ],
    }
}

/// The IAM permissions this deployable requires, as a reviewable list.
pub const PERMISSIONS: &[&str] = &[
    "rds-data:BeginTransaction",
    "rds-data:CommitTransaction",
    "rds-data:ExecuteStatement",
    "rds-data:RollbackTransaction",
    "secretsmanager:GetSecretValue",
];

/// What each start-up probe answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Probes {
    /// `SELECT 1` as `aex_identity_api` succeeded.
    pub aurora: bool,
    /// The active identity pepper loaded.
    pub pepper: bool,
    /// Both providers' OAuth clients loaded and parsed.
    ///
    /// A process that cannot load them can never complete a sign-in, and a
    /// browser session is the only thing that can approve a device
    /// authorization — so serving without them means the whole credential
    /// ceremony fails at its second step rather than at start-up.
    pub oauth_clients: bool,
}

impl Probes {
    /// No probe has answered yet.
    pub const NONE: Self = Self {
        aurora: false,
        pepper: false,
        oauth_clients: false,
    };

    /// Every probe answered.
    pub const READY: Self = Self {
        aurora: true,
        pepper: true,
        oauth_clients: true,
    };
}

/// The readiness projection.
#[must_use]
pub fn readiness(probes: Probes) -> Readiness {
    Readiness::new(
        DEPLOYABLE.as_str(),
        vec![
            Dependency {
                name: "aurora",
                resolved: probes.aurora,
            },
            Dependency {
                name: "identity-pepper",
                resolved: probes.pepper,
            },
            Dependency {
                name: "oauth-client-credentials",
                resolved: probes.oauth_clients,
            },
        ],
    )
}

/// Why `central-identity-api` stopped.
#[derive(Debug, thiserror::Error)]
pub enum CentralIdentityApiRunError {
    /// Start-up configuration was rejected.
    #[error(transparent)]
    Config(#[from] CentralIdentityApiConfigError),
    /// The composition was refused before any client was opened.
    #[error(transparent)]
    Composition(#[from] CompositionError),
    /// A start-up probe did not answer, so the process refuses to serve.
    ///
    /// Naming the dependency is the whole point: "not ready" without a name is
    /// a page nobody can action.
    #[error("the `{0}` dependency did not answer: {1}")]
    Dependency(&'static str, String),
    /// The listener stopped.
    #[error("the listener stopped: {0}")]
    Listener(String),
}

/// Builds the router this binary serves.
///
/// The public surface is exactly `CentralServiceId::IdentityApi.groups()`, each
/// mounted by iterating its generated route slice, plus the generated refusal
/// arm for every route this deployable is planned to own and the contract
/// defers, plus the two internal probes.
pub fn app<A: AuthApi, I: aex_wire::server::IdentityApi>(
    api: Arc<A>,
    account: Arc<I>,
    edge: EdgeStack,
    readiness: Readiness,
) -> axum::Router {
    aex_central_http::health::router(readiness)
        .merge(mount_auth_api(api, edge.clone()))
        .merge(mount_identity_api(account, edge))
        .merge(aex_central_http::mount_deferred(DEPLOYABLE))
}

/// Runs `central-identity-api` until it stops.
///
/// # Errors
///
/// Returns [`CentralIdentityApiRunError`] when configuration or composition is refused, or when
/// the listener stops.
pub async fn run<A: AuthApi, I: aex_wire::server::IdentityApi>(
    config: &Config,
    api: Arc<A>,
    account: Arc<I>,
    edge: EdgeStack,
    probes: Probes,
    telemetry: &aex_platform_telemetry::Handle,
) -> Result<(), CentralIdentityApiRunError> {
    aex_central_http::capability::admit(&manifest(), &config.resolved())?;
    telemetry.emit(
        aex_platform_telemetry::Record::event(
            aex_telemetry_schema::generated::EVENT_AEX_PROCESS_STARTED,
        )
        .with(
            aex_telemetry_schema::generated::AEX_PLANE,
            config.http.plane.as_str().to_owned(),
        )
        .with(
            aex_telemetry_schema::generated::AEX_REGION,
            config.http.region.as_str().to_owned(),
        ),
    );
    lambda_http::run(app(api, account, edge, readiness(probes)))
        .await
        .map_err(|error| CentralIdentityApiRunError::Listener(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{
        CentralIdentityApiConfigError, Config, DEPLOYABLE, PERMISSIONS, Probes, app, keys,
        manifest, readiness,
    };
    use aex_central_http::capability::{
        AssertionSign, Capability as _, CapabilityBinding, CompositionError, ControlWrite,
        SignInHandshake,
    };
    use aex_central_http::config::CentralServiceId;
    use aex_central_http::health::{HEALTH_PATH, READY_PATH};
    use aex_central_http::router::EdgeStack;
    use aex_central_http::target::{TargetPath, TargetResolver};
    use aex_control_app::ports::Clock;
    use aex_control_domain::{AccountState, CursorSecret, Resource};
    use aex_wire::error::{ErrorCode, WireError, WireResult};
    use aex_wire::routes::{RouteId, route};
    use aex_wire::server::{AuthApi, Created, RequestContext};
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use time::OffsetDateTime;
    use tower::ServiceExt as _;
    use uuid::Uuid;

    fn complete() -> BTreeMap<&'static str, String> {
        BTreeMap::from([
            (keys::PLANE, "dev".to_owned()),
            (keys::REGION, "eu-west-1".to_owned()),
            (keys::ACCOUNT_ID, "000000000000".to_owned()),
            (
                keys::AURORA_CLUSTER_ARN,
                "arn:aws:rds:eu-west-1:000000000000:cluster:aex".to_owned(),
            ),
            (
                keys::AURORA_SECRET_ARN,
                "arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex-identity".to_owned(),
            ),
            (
                keys::PEPPER_SECRET_ID,
                "aex/dev/identity-pepper/current".to_owned(),
            ),
            (keys::DATABASE, "aex".to_owned()),
            (keys::ROLE, "aex_identity_api".to_owned()),
            (
                keys::DEVICE_VERIFICATION_URI,
                "https://aex.dev/device".to_owned(),
            ),
            (
                keys::GITHUB_OAUTH_SECRET_ID,
                "aex/dev/sign-in/github/current".to_owned(),
            ),
            (
                keys::GOOGLE_OAUTH_SECRET_ID,
                "aex/dev/sign-in/google/current".to_owned(),
            ),
            (
                keys::SIGN_IN_REDIRECT_URI,
                "https://dash.aex.dev/auth/callback".to_owned(),
            ),
            (keys::MAX_BODY_BYTES, "65536".to_owned()),
            (keys::REQUEST_DEADLINE_MS, "5000".to_owned()),
        ])
    }

    fn read(
        vars: &BTreeMap<&'static str, String>,
    ) -> Result<Config, CentralIdentityApiConfigError> {
        Config::from_lookup(|name| vars.get(name).cloned())
    }

    #[derive(Debug)]
    struct FixedClock;

    impl Clock for FixedClock {
        fn now(&self) -> OffsetDateTime {
            OffsetDateTime::UNIX_EPOCH
        }
    }

    #[derive(Debug)]
    struct NoResource;

    #[async_trait]
    impl TargetResolver for NoResource {
        async fn resolve(
            &self,
            _route: RouteId,
            _path: &TargetPath,
        ) -> Result<Option<Resource>, aex_central_http::error::EdgeError> {
            Ok(None)
        }

        async fn account_state(
            &self,
            _organization_id: Uuid,
        ) -> Result<AccountState, aex_central_http::error::EdgeError> {
            Ok(AccountState::Active)
        }
    }

    #[derive(Debug)]
    struct Api;

    /// The account read, refusing rather than answering. The router cases here
    /// prove the route is *mounted*; what it answers is `account.rs`'s suite.
    #[derive(Debug)]
    struct Account;

    impl aex_wire::server::IdentityApi for Account {
        async fn account_get(
            &self,
            _cx: &RequestContext,
            _query: aex_wire::models::AccountGetQuery,
        ) -> WireResult<aex_wire::models::AccountOperationalState> {
            Err(WireError::new(ErrorCode::AccountStateUnavailable))
        }
    }

    impl AuthApi for Api {
        async fn device_authorization_create(
            &self,
            _cx: &RequestContext,
            _body: aex_wire::models::DeviceAuthorizationRequest,
        ) -> WireResult<Created<aex_wire::models::DeviceAuthorization>> {
            Err(WireError::new(ErrorCode::RateLimited))
        }

        async fn device_token_create(
            &self,
            _cx: &RequestContext,
            _body: aex_wire::models::DeviceTokenRequest,
        ) -> WireResult<aex_wire::models::DeviceToken> {
            Err(WireError::new(ErrorCode::RateLimited))
        }

        async fn device_decision_create(
            &self,
            _cx: &RequestContext,
            _body: aex_wire::models::DeviceDecisionRequest,
        ) -> WireResult<aex_wire::models::DeviceDecisionResult> {
            Err(WireError::new(ErrorCode::RateLimited))
        }

        async fn dashboard_session_create(
            &self,
            _cx: &RequestContext,
            _body: aex_wire::models::DashboardSessionRequest,
        ) -> WireResult<Created<aex_wire::models::DashboardSessionCredential>> {
            Err(WireError::new(ErrorCode::RateLimited))
        }

        async fn dashboard_session_delete(
            &self,
            _cx: &RequestContext,
        ) -> WireResult<aex_wire::server::NoContent> {
            Err(WireError::new(ErrorCode::RateLimited))
        }
    }

    fn router() -> axum::Router {
        let config = read(&complete()).expect("a complete environment");
        let edge = EdgeStack::new(
            config.http,
            Arc::new(NoResource),
            Arc::new(FixedClock),
            Arc::new(CursorSecret::new([1_u8; 32])),
        );
        app(
            Arc::new(Api),
            Arc::new(Account),
            edge,
            readiness(Probes::NONE),
        )
    }

    #[test]
    fn a_complete_environment_is_accepted() {
        let config = read(&complete()).expect("a complete environment");
        assert_eq!(config.http.service, CentralServiceId::IdentityApi);
        assert_eq!(config.http.limits.max_json_body_bytes, 65_536);
    }

    #[test]
    fn every_variable_is_required_and_named_when_absent() {
        for name in keys::ALL {
            let mut vars = complete();
            vars.remove(name);
            assert!(read(&vars).is_err(), "removing {name}");
        }
    }

    #[test]
    fn this_binary_refuses_any_role_but_its_own() {
        let mut vars = complete();
        vars.insert(keys::ROLE, "aex_control_api".to_owned());
        assert!(read(&vars).is_err());
    }

    #[test]
    fn a_body_bound_over_the_shared_ceiling_is_refused() {
        let mut vars = complete();
        vars.insert(keys::MAX_BODY_BYTES, "1048576".to_owned());
        assert!(read(&vars).is_err());
    }

    #[test]
    fn the_composition_admits_exactly_its_declared_bindings() {
        let config = read(&complete()).expect("a complete environment");
        assert_eq!(
            aex_central_http::capability::admit(&manifest(), &config.resolved()),
            Ok(())
        );
    }

    /// The shared sign-in exchange secret is gone, not renamed.
    ///
    /// With the authorization-code exchange happening in this process there is
    /// no untrusted caller asserting an identity, so there is nothing left for a
    /// shared secret to prove. This asserts the variable cannot come back by
    /// accident under either deployable's namespace.
    #[test]
    fn no_shared_sign_in_exchange_secret_survives_anywhere_in_the_configuration() {
        for name in keys::ALL {
            assert!(
                !name.contains("SIGN_IN_EXCHANGE"),
                "`{name}` is the deleted shared secret"
            );
        }
        for binding in manifest().bindings {
            assert!(
                !binding.key.contains("SIGN_IN_EXCHANGE"),
                "`{}` binds the deleted shared secret",
                binding.key
            );
        }
    }

    /// Each provider's client is its own binding under its own capability, so
    /// the review list names every credential this deployable may hold.
    #[test]
    fn each_provider_oauth_client_is_bound_to_the_handshake_capability() {
        let bindings = manifest().bindings;
        for key in [keys::GITHUB_OAUTH_SECRET_ID, keys::GOOGLE_OAUTH_SECRET_ID] {
            let binding = bindings
                .iter()
                .find(|it| it.key == key)
                .unwrap_or_else(|| panic!("`{key}` is not bound"));
            assert_eq!(binding.capability, SignInHandshake::ID, "{key}");
            assert!(!binding.arn, "`{key}` names a secret, not an ARN");
        }
    }

    #[test]
    fn a_redirect_uri_that_is_not_https_is_refused_at_start_up() {
        for forged in [
            "http://dash.aex.dev/auth/callback",
            "not-a-url",
            "/callback",
        ] {
            let mut vars = complete();
            vars.insert(keys::SIGN_IN_REDIRECT_URI, forged.to_owned());
            assert!(read(&vars).is_err(), "{forged}");
        }
    }

    #[test]
    fn this_binary_cannot_link_a_control_write_or_signing_capability() {
        let config = read(&complete()).expect("a complete environment");
        for capability in [ControlWrite::ID, AssertionSign::ID] {
            let mut manifest = manifest();
            manifest.bindings.push(CapabilityBinding::resource(
                "AEX_CENTRAL_IDENTITY_X",
                capability,
            ));
            assert_eq!(
                aex_central_http::capability::admit(&manifest, &config.resolved()),
                Err(CompositionError::ForbiddenCapability {
                    key: "AEX_CENTRAL_IDENTITY_X",
                    capability
                })
            );
        }
    }

    #[test]
    fn the_permission_list_touches_no_queue_object_store_or_regional_authority() {
        for permission in PERMISSIONS {
            assert!(!permission.starts_with("sqs:"), "{permission}");
            assert!(!permission.starts_with("s3:"), "{permission}");
            assert!(!permission.starts_with("lambda:"), "{permission}");
        }
    }

    /// The authorizer context `central-authz` produces for a resolved browser
    /// session, for the routes that need one to reach their handler at all.
    fn admitted_session() -> aex_central_http::authorizer::CentralAuthorizerContext {
        // The window is anchored to `FixedClock`, which the edge in this
        // module's router reads. A context minted against the wall clock would
        // be refused as not-yet-current, which is the correct behaviour and a
        // confusing way to fail a mount assertion.
        aex_central_http::authorizer::CentralAuthorizerContext {
            request_id: aex_wire::types::RequestId::parse("req-mount").expect("a request id"),
            kind: aex_central_http::authorizer::ContextPrincipalKind::UserSession,
            principal_id: Uuid::now_v7(),
            credential_id: Some(Uuid::now_v7()),
            workspace_id: None,
            organization_id: None,
            region: None,
            memberships: Vec::new(),
            scopes: aex_control_domain::ScopeSet::from_strings(&["account:read", "account:write"])
                .expect("known scopes"),
            account_state: AccountState::Unavailable,
            issued_at_ms: 0,
            expires_at_ms: 30_000,
        }
    }

    #[tokio::test]
    async fn the_mounted_set_is_exactly_the_declared_one() {
        assert_eq!(DEPLOYABLE.routes().len(), 6);
        for id in DEPLOYABLE.routes() {
            let descriptor = route(id);
            // `account_get` is the one credentialed route here, so an
            // anonymous request is refused by the edge before any handler
            // runs. `401` is therefore its mount evidence: an unmounted route
            // answers `404`, and the difference is the whole assertion.
            if id == RouteId::AccountGet {
                let response = router()
                    .oneshot(
                        Request::builder()
                            .uri(format!(
                                "{}?organizationId=org_01k1jt1p4new3re1r70w3ge1r7",
                                descriptor.template
                            ))
                            .body(Body::empty())
                            .expect("a valid request"),
                    )
                    .await
                    .expect("the router answers");
                assert_eq!(
                    response.status(),
                    StatusCode::UNAUTHORIZED,
                    "`{}` is not mounted",
                    descriptor.operation_id
                );
                continue;
            }
            let body = match id {
                RouteId::DeviceAuthorizationCreate => "{\"clientId\":\"aex-cli\",\"scopes\":[]}",
                RouteId::DeviceTokenCreate => {
                    "{\"clientId\":\"aex-cli\",\"deviceCode\":\"dvc_fixture\"}"
                }
                RouteId::DeviceDecisionCreate => {
                    "{\"userCode\":\"BCDFG-HJKLM\",\"decision\":\"approve\"}"
                }
                RouteId::DashboardSessionCreate => {
                    "{\"provider\":\"github\",\"code\":\"gh-code\",\"state\":\"E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM\",\"codeVerifier\":\"dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk\"}"
                }
                _ => "",
            };
            let mut request = Request::builder()
                .method(descriptor.method.as_str())
                .uri(descriptor.template);
            if descriptor.idempotency == aex_wire::idempotency::IdempotencyKind::IdempotencyKey {
                request = request.header("idempotency-key", "fixture");
            }
            let mut request = request.body(Body::from(body)).expect("a valid request");
            // A route whose alternative principal is a browser session is
            // refused by the edge before any handler runs, so mounting can only
            // be observed with a credential the edge admits.
            if descriptor.alt_principal == Some(aex_wire::idempotency::PrincipalKind::UserSession) {
                request.extensions_mut().insert(admitted_session());
            }
            let response = router().oneshot(request).await.expect("the router answers");
            let status = response.status();
            let bytes = axum::body::to_bytes(response.into_body(), 1 << 16)
                .await
                .expect("a bounded body");
            assert_eq!(
                status,
                StatusCode::TOO_MANY_REQUESTS,
                "`{}` is not mounted: {}",
                descriptor.operation_id,
                String::from_utf8_lossy(&bytes)
            );
        }
    }

    #[tokio::test]
    async fn a_route_this_deployable_does_not_own_is_not_mounted() {
        let response = router()
            .oneshot(
                Request::builder()
                    .uri("/api/organizations")
                    .body(Body::empty())
                    .expect("a valid request"),
            )
            .await
            .expect("the router answers");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn liveness_answers_and_readiness_is_fail_closed() {
        let live = router()
            .oneshot(
                Request::builder()
                    .uri(HEALTH_PATH)
                    .body(Body::empty())
                    .expect("a valid request"),
            )
            .await
            .expect("the router answers");
        assert_eq!(live.status(), StatusCode::OK);
        let ready = router()
            .oneshot(
                Request::builder()
                    .uri(READY_PATH)
                    .body(Body::empty())
                    .expect("a valid request"),
            )
            .await
            .expect("the router answers");
        assert_eq!(ready.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
