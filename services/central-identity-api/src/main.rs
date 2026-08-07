//! `central-identity-api` composition root (Rust Lambda ZIP).
//!
//! The browser ceremony exchange and the identity lifecycle, plus the two public
//! device-flow routes. It writes identity DML and holds exactly one control
//! privilege — `control.bump_user_epoch` — so a disabled person's assertions
//! stop verifying in the same transaction that disables them.
//!
//! The mounted public surface is `CentralServiceId::IdentityApi.routes()` and
//! nothing else, which `the_mounted_set_is_exactly_the_declared_one` asserts.

mod api;
mod startup;
mod targets;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use aex_central_http::capability::{
    Capability as _, CapabilityBinding, CompositionError, CompositionManifest, Declares,
    IdentityWrite,
};
use aex_central_http::config::{CentralServiceId, HttpConfig};
use aex_central_http::health::{Dependency, Readiness};
use aex_central_http::router::{EdgeStack, mount_auth_api};
use aex_wire::server::AuthApi;

/// The deployable this binary is.
const DEPLOYABLE: CentralServiceId = CentralServiceId::IdentityApi;

/// The one login role this binary may connect as.
const REQUIRED_ROLE: &str = "aex_identity_api";

/// Environment keys, all inside the declared namespace.
mod keys {
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
    /// The dashboard BFF's expected `OIDC` subject.
    pub const VERCEL_EXPECTED_SUBJECT: &str = "AEX_CENTRAL_IDENTITY_VERCEL_EXPECTED_SUBJECT";
    /// The dashboard BFF's `OIDC` issuer.
    pub const VERCEL_ISSUER: &str = "AEX_CENTRAL_IDENTITY_VERCEL_ISSUER";

    /// Every key this binary reads, for the totality test.
    #[allow(
        dead_code,
        reason = "the inventory exists so the suite can remove each key in turn"
    )]
    pub const ALL: &[&str] = &[
        ACCOUNT_ID,
        AURORA_CLUSTER_ARN,
        AURORA_SECRET_ARN,
        DATABASE,
        DEVICE_VERIFICATION_URI,
        MAX_BODY_BYTES,
        PEPPER_SECRET_ID,
        PLANE,
        REGION,
        REQUEST_DEADLINE_MS,
        ROLE,
        VERCEL_EXPECTED_SUBJECT,
        VERCEL_ISSUER,
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
    /// The dashboard BFF's `OIDC` issuer.
    pub vercel_issuer: String,
    /// The dashboard BFF's expected `OIDC` subject.
    pub vercel_expected_subject: String,
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
            vercel_issuer: required(&lookup, keys::VERCEL_ISSUER)?,
            vercel_expected_subject: required(&lookup, keys::VERCEL_EXPECTED_SUBJECT)?,
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

/// This binary's capability declaration.
///
/// Identity DML and one pepper. No control write, no queue, no object store, no
/// payment provider and no regional invoke: a binding for any of them is refused
/// at start-up.
#[allow(
    dead_code,
    reason = "the declaration is the capability list; its only use is the type-level `Declares` bound"
)]
struct Composition;

impl Declares<IdentityWrite> for Composition {}

/// The manifest the start-up check runs against.
#[must_use]
pub fn manifest() -> CompositionManifest {
    CompositionManifest {
        deployable: DEPLOYABLE,
        capabilities: BTreeSet::from([IdentityWrite::ID]),
        bindings: vec![
            CapabilityBinding::arn(keys::AURORA_CLUSTER_ARN, IdentityWrite::ID),
            CapabilityBinding::resource(keys::PEPPER_SECRET_ID, IdentityWrite::ID),
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
}

impl Probes {
    /// No probe has answered yet.
    pub const NONE: Self = Self {
        aurora: false,
        pepper: false,
    };

    /// Every probe answered.
    pub const READY: Self = Self {
        aurora: true,
        pepper: true,
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
/// mounted by iterating its generated route slice, plus the two internal probes.
pub fn app<A: AuthApi>(api: Arc<A>, edge: EdgeStack, readiness: Readiness) -> axum::Router {
    aex_central_http::health::router(readiness).merge(mount_auth_api(api, edge))
}

/// Runs `central-identity-api` until it stops.
///
/// # Errors
///
/// Returns [`CentralIdentityApiRunError`] when configuration or composition is refused, or when
/// the listener stops.
pub async fn run<A: AuthApi>(
    config: &Config,
    api: Arc<A>,
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
    lambda_http::run(app(api, edge, readiness(probes)))
        .await
        .map_err(|error| CentralIdentityApiRunError::Listener(error.to_string()))
}

/// Builds the real adapters, proves each one answers, and serves.
///
/// Both probes run **before** the listener binds, and either failing refuses the
/// process. The alternative — serving with a false readiness flag — admits
/// requests this deployable can only fail, and a device-flow route that answers
/// without an identity store is one that mints nothing and says it did.
async fn compose(
    config: &Config,
    telemetry: &aex_platform_telemetry::Handle,
) -> Result<(), CentralIdentityApiRunError> {
    let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let data_api = aex_rds_data::DataApiConfig::new(
        aex_rds_data::ResourceArn::parse(&config.aurora_cluster_arn)
            .map_err(|error| CentralIdentityApiRunError::Dependency("aurora", error.to_string()))?,
        aex_rds_data::SecretArn::parse(&config.aurora_secret_arn)
            .map_err(|error| CentralIdentityApiRunError::Dependency("aurora", error.to_string()))?,
        aex_rds_data::DatabaseName::parse(&config.database)
            .map_err(|error| CentralIdentityApiRunError::Dependency("aurora", error.to_string()))?,
    );
    let client = aex_rds_data::DataApiClient::new(
        Arc::new(aex_rds_data::AwsTransport::new(
            aws_sdk_rdsdata::Client::new(&aws),
            &data_api,
        )),
        data_api,
    );

    // Probe one: the login role can read. `SELECT 1` as `aex_identity_api` is
    // the cheapest statement that proves the cluster is awake, the credential
    // resolves and the role exists.
    client
        .query::<Ok1>(aex_rds_data::Statement::new(
            aex_identity_aurora::sql::READINESS_PROBE,
        ))
        .await
        .map_err(|error| CentralIdentityApiRunError::Dependency("aurora", error.to_string()))?;

    let peppers = Arc::new(aex_central_aws::SecretsManagerPepperKeystore::new(
        aws_sdk_secretsmanager::Client::new(&aws),
        config.pepper_secret_id.clone(),
        Arc::new(aex_central_aws::DataApiPepperDirectory::new(
            client.clone(),
            aex_central_aws::PepperStatements {
                active: aex_identity_aurora::sql::ACTIVE_IDENTITY_PEPPER,
                by_version: aex_identity_aurora::sql::IDENTITY_PEPPER_BY_VERSION,
            },
        )),
    ));

    // Probe two: the active identity pepper resolves, end to end — the
    // lifecycle row, then its material by that row's version id. A process that
    // cannot load it can verify nothing and mint nothing.
    peppers
        .probe(aex_identity_app::ports::PepperPurpose::Identity)
        .await
        .map_err(|error| {
            CentralIdentityApiRunError::Dependency("identity-pepper", error.to_string())
        })?;

    let clock: Arc<dyn aex_identity_app::ports::Clock> = Arc::new(aex_central_aws::SystemClock);
    let store = Arc::new(aex_identity_aurora::AuroraIdentityStore::new(
        client,
        Arc::clone(&peppers) as Arc<dyn aex_identity_app::ports::PepperKeystore>,
    ));
    let verification_uri = aex_wire::types::HttpsUrl::parse(&config.device_verification_uri)
        .map_err(|error| {
            CentralIdentityApiRunError::Dependency("device-verification-uri", error.to_string())
        })?;
    let api = Arc::new(api::AuthService::new(
        store,
        Arc::clone(&peppers) as Arc<dyn aex_identity_app::ports::PepperKeystore>,
        Arc::clone(&clock),
        Arc::new(aex_central_aws::Uuid7Factory),
        Arc::new(aex_central_aws::OsSecretRng),
        verification_uri,
    ));

    // The cursor secret is per-process and never leaves it: this deployable
    // mounts no paged route, so a cursor it minted could only be redeemed by
    // itself, and a configured shared secret would be one more credential to
    // hold for no reader.
    let mut cursor_bytes = [0_u8; 32];
    aex_identity_domain::SecretRng::fill(&aex_central_aws::OsSecretRng, &mut cursor_bytes);
    let edge = EdgeStack::new(
        config.http.clone(),
        Arc::new(targets::NoOrganizationTargets),
        clock,
        Arc::new(aex_control_domain::CursorSecret::new(cursor_bytes)),
    );

    run(config, api, edge, Probes::READY, telemetry).await
}

/// The one-column `SELECT 1` the readiness probe issues.
struct Ok1;

impl aex_rds_data::Row for Ok1 {
    fn from_record(record: &aex_rds_data::Record<'_>) -> Result<Self, aex_rds_data::DecodeError> {
        record.expect_arity(1)?;
        record.i64(0)?;
        Ok(Self)
    }
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("central-identity-api: refusing to start: {error}");
            eprintln!(
                "central-identity-api: required configuration: {}",
                keys::ALL.join(", ")
            );
            return std::process::ExitCode::FAILURE;
        }
    };
    let settings = aex_platform_telemetry::Settings::default();
    let telemetry = aex_platform_telemetry::Handle::install(&settings, None);
    let outcome = compose(&config, &telemetry).await;
    if let aex_platform_telemetry::FlushOutcome::DeadlineExceeded { pending } =
        telemetry.flush(settings.flush_deadline)
    {
        eprintln!("central-identity-api: telemetry flush left {pending} record(s) undelivered");
    }
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("central-identity-api: stopped: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CentralIdentityApiConfigError, Config, DEPLOYABLE, PERMISSIONS, Probes, app, keys,
        manifest, readiness,
    };
    use aex_central_http::capability::{
        AssertionSign, Capability as _, CapabilityBinding, CompositionError, ControlWrite,
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
                keys::VERCEL_ISSUER,
                "https://oidc.vercel.com/aexhq".to_owned(),
            ),
            (
                keys::VERCEL_EXPECTED_SUBJECT,
                "owner:aexhq:project:dashboard".to_owned(),
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
    }

    fn router() -> axum::Router {
        let config = read(&complete()).expect("a complete environment");
        let edge = EdgeStack::new(
            config.http,
            Arc::new(NoResource),
            Arc::new(FixedClock),
            Arc::new(CursorSecret::new([1_u8; 32])),
        );
        app(Arc::new(Api), edge, readiness(Probes::NONE))
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

    #[tokio::test]
    async fn the_mounted_set_is_exactly_the_declared_one() {
        assert_eq!(DEPLOYABLE.routes().len(), 2);
        for id in DEPLOYABLE.routes() {
            let descriptor = route(id);
            let body = match id {
                RouteId::DeviceAuthorizationCreate => "{\"clientId\":\"aex-cli\",\"scopes\":[]}",
                _ => "{\"clientId\":\"aex-cli\",\"deviceCode\":\"dvc_fixture\"}",
            };
            let mut request = Request::builder()
                .method(descriptor.method.as_str())
                .uri(descriptor.template);
            if descriptor.idempotency == aex_wire::idempotency::IdempotencyKind::IdempotencyKey {
                request = request.header("idempotency-key", "fixture");
            }
            let response = router()
                .oneshot(request.body(Body::from(body)).expect("a valid request"))
                .await
                .expect("the router answers");
            assert_eq!(
                response.status(),
                StatusCode::TOO_MANY_REQUESTS,
                "`{}` is not mounted",
                descriptor.operation_id
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
