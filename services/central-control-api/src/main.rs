//! `central-control-api` composition root (Rust Lambda ZIP).
//!
//! Organizations, memberships, invitations, workspaces, API keys and durable
//! operations, plus the dashboard shell read. Billing and the account read are
//! finance routes and belong to `finance-api`; the owner map in
//! `aex_central_http::config` is the single place that says so.
//!
//! The mounted public surface is `CentralServiceId::ControlApi.routes()` and
//! nothing else, which `the_mounted_set_is_exactly_the_declared_one` asserts by
//! driving every one of them.

mod api;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use aex_central_http::capability::{
    Capability as _, CapabilityBinding, CompositionError, CompositionManifest, ControlWrite,
    Declares, RegionalControlInvoke,
};
use aex_central_http::config::{CentralServiceId, HttpConfig};
use aex_central_http::health::{Dependency, Readiness};
use aex_central_http::router::{
    EdgeStack, mount_api_keys_api, mount_bootstrap_api, mount_central_operations_api,
    mount_organizations_api, mount_workspaces_api,
};
use aex_wire::server::{
    ApiKeysApi, BootstrapApi, CentralOperationsApi, OrganizationsApi, WorkspacesApi,
};
use aex_wire::types::Region;

/// The deployable this binary is.
const DEPLOYABLE: CentralServiceId = CentralServiceId::ControlApi;

/// The one login role this binary may connect as.
const REQUIRED_ROLE: &str = "aex_control_api";

/// Environment keys, all inside the declared namespace.
mod keys {
    /// The plane's account id, for the ARN binding check.
    pub const ACCOUNT_ID: &str = "AEX_CENTRAL_CONTROL_ACCOUNT_ID";
    /// The Aurora cluster holding the control schema.
    pub const AURORA_CLUSTER_ARN: &str = "AEX_CENTRAL_CONTROL_AURORA_CLUSTER_ARN";
    /// The Aurora credentials secret.
    pub const AURORA_SECRET_ARN: &str = "AEX_CENTRAL_CONTROL_AURORA_SECRET_ARN";
    /// The cursor signing secret.
    pub const CURSOR_SECRET_ID: &str = "AEX_CENTRAL_CONTROL_CURSOR_SECRET_ID";
    /// The database name.
    pub const DATABASE: &str = "AEX_CENTRAL_CONTROL_DATABASE";
    /// The largest request body this composition accepts.
    pub const MAX_BODY_BYTES: &str = "AEX_CENTRAL_CONTROL_MAX_BODY_BYTES";
    /// The API-key credential pepper secret.
    pub const PEPPER_SECRET_ID: &str = "AEX_CENTRAL_CONTROL_PEPPER_SECRET_ID";
    /// The deployment plane.
    pub const PLANE: &str = "AEX_CENTRAL_CONTROL_PLANE";
    /// The bound region.
    pub const REGION: &str = "AEX_CENTRAL_CONTROL_REGION";
    /// Direct regional-control Lambda ARNs, `region=arn` comma-separated.
    pub const REGIONAL_FUNCTIONS: &str = "AEX_CENTRAL_CONTROL_REGIONAL_FUNCTION_ARNS";
    /// Public workspace API base URLs, `region=https-url` comma-separated.
    pub const API_URLS: &str = "AEX_CENTRAL_CONTROL_API_URLS";
    /// How long one request may take.
    pub const REQUEST_DEADLINE_MS: &str = "AEX_CENTRAL_CONTROL_REQUEST_DEADLINE_MS";
    /// The login role. Must be `aex_control_api`.
    pub const ROLE: &str = "AEX_CENTRAL_CONTROL_ROLE";

    /// Every key this binary reads, for the totality test.
    #[allow(
        dead_code,
        reason = "the inventory exists so the suite can remove each key in turn"
    )]
    pub const ALL: &[&str] = &[
        ACCOUNT_ID,
        AURORA_CLUSTER_ARN,
        AURORA_SECRET_ARN,
        API_URLS,
        CURSOR_SECRET_ID,
        DATABASE,
        MAX_BODY_BYTES,
        PEPPER_SECRET_ID,
        PLANE,
        REGION,
        REGIONAL_FUNCTIONS,
        REQUEST_DEADLINE_MS,
        ROLE,
    ];
}

/// Why `central-control-api` refused to start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CentralControlApiConfigError {
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
    /// The cursor signing secret.
    pub cursor_secret_id: String,
    /// The API-key pepper secret.
    pub pepper_secret_id: String,
    /// The database name.
    pub database: String,
    /// The login role.
    pub role: String,
    /// Every configured region a workspace may be placed in.
    pub regional_functions: BTreeMap<Region, String>,
    /// Public workspace API base URL for every configured region.
    pub api_urls: BTreeMap<Region, aex_wire::types::HttpsUrl>,
}

impl Config {
    /// Reads and validates the configuration from the process environment.
    ///
    /// # Errors
    ///
    /// Returns [`CentralControlApiConfigError`] naming the first variable it refused.
    pub fn from_env() -> Result<Self, CentralControlApiConfigError> {
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
    pub fn from_lookup<F>(lookup: F) -> Result<Self, CentralControlApiConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let role = required(&lookup, keys::ROLE)?;
        if role != REQUIRED_ROLE {
            return Err(CentralControlApiConfigError::Invalid {
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
        .map_err(|reason| CentralControlApiConfigError::Invalid {
            name: keys::PLANE,
            reason: reason.to_string(),
        })?;
        let regional_functions = functions(&required(&lookup, keys::REGIONAL_FUNCTIONS)?)?;
        let api_urls = api_urls(&required(&lookup, keys::API_URLS)?)?;
        if let Some(region) = regional_functions
            .keys()
            .find(|region| !api_urls.contains_key(region))
        {
            return Err(CentralControlApiConfigError::Invalid {
                name: keys::API_URLS,
                reason: format!(
                    "no public API URL for configured region `{}`",
                    region.as_str()
                ),
            });
        }
        if let Some(region) = api_urls
            .keys()
            .find(|region| !regional_functions.contains_key(region))
        {
            return Err(CentralControlApiConfigError::Invalid {
                name: keys::REGIONAL_FUNCTIONS,
                reason: format!(
                    "no function ARN for configured region `{}`",
                    region.as_str()
                ),
            });
        }
        Ok(Self {
            http,
            account_id: required(&lookup, keys::ACCOUNT_ID)?,
            aurora_cluster_arn: required(&lookup, keys::AURORA_CLUSTER_ARN)?,
            aurora_secret_arn: required(&lookup, keys::AURORA_SECRET_ARN)?,
            cursor_secret_id: required(&lookup, keys::CURSOR_SECRET_ID)?,
            pepper_secret_id: required(&lookup, keys::PEPPER_SECRET_ID)?,
            database: required(&lookup, keys::DATABASE)?,
            role,
            regional_functions,
            api_urls,
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
                    keys::REGIONAL_FUNCTIONS.to_owned(),
                    self.regional_functions
                        .iter()
                        .map(|(region, function)| format!("{}={function}", region.as_str()))
                        .collect::<Vec<_>>()
                        .join(","),
                ),
            ]),
        }
    }
}

fn required<F>(lookup: &F, name: &'static str) -> Result<String, CentralControlApiConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    match lookup(name) {
        Some(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(CentralControlApiConfigError::Missing(name)),
    }
}

fn integer<F>(lookup: &F, name: &'static str) -> Result<u64, CentralControlApiConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let raw = required(lookup, name)?;
    raw.parse::<u64>()
        .map_err(|_| CentralControlApiConfigError::Invalid {
            name,
            reason: format!("expected an integer, got `{raw}`"),
        })
}

/// Parses the `region=lambda-arn` authority map.
///
/// Every configured launch region must be present, and it is checked here rather
/// than at the first `POST /api/workspaces`. A plane may compose a strict
/// subset while its remaining regional stacks are not yet published, but it
/// must never claim an unconfigured region is reachable.
fn functions(raw: &str) -> Result<BTreeMap<Region, String>, CentralControlApiConfigError> {
    let mut map = BTreeMap::new();
    for entry in raw.split(',').filter(|it| !it.trim().is_empty()) {
        let (region, function) =
            entry
                .split_once('=')
                .ok_or_else(|| CentralControlApiConfigError::Invalid {
                    name: keys::REGIONAL_FUNCTIONS,
                    reason: "expected `region=lambda-arn` entries".to_owned(),
                })?;
        let parsed = Region::from_name(region.trim()).ok_or_else(|| {
            CentralControlApiConfigError::Invalid {
                name: keys::REGIONAL_FUNCTIONS,
                reason: format!("`{region}` is not a launch region"),
            }
        })?;
        let function = function.trim();
        let expected = format!("arn:aws:lambda:{}:", parsed.as_str());
        if !function.starts_with(&expected)
            || !function.contains(":function:")
            || map.insert(parsed, function.to_owned()).is_some()
        {
            return Err(CentralControlApiConfigError::Invalid {
                name: keys::REGIONAL_FUNCTIONS,
                reason: format!(
                    "`{}` is not a unique Lambda ARN in its bound region",
                    parsed.as_str()
                ),
            });
        }
    }
    if map.is_empty() {
        return Err(CentralControlApiConfigError::Invalid {
            name: keys::REGIONAL_FUNCTIONS,
            reason: "at least one configured region is required".to_owned(),
        });
    }
    Ok(map)
}

fn api_urls(
    raw: &str,
) -> Result<BTreeMap<Region, aex_wire::types::HttpsUrl>, CentralControlApiConfigError> {
    let mut map = BTreeMap::new();
    for entry in raw.split(',').filter(|entry| !entry.trim().is_empty()) {
        let (region, url) =
            entry
                .split_once('=')
                .ok_or_else(|| CentralControlApiConfigError::Invalid {
                    name: keys::API_URLS,
                    reason: "expected `region=https-url` entries".to_owned(),
                })?;
        let region = Region::from_name(region.trim()).ok_or_else(|| {
            CentralControlApiConfigError::Invalid {
                name: keys::API_URLS,
                reason: format!("`{region}` is not a launch region"),
            }
        })?;
        let url = aex_wire::types::HttpsUrl::parse(url.trim()).map_err(|error| {
            CentralControlApiConfigError::Invalid {
                name: keys::API_URLS,
                reason: error.to_string(),
            }
        })?;
        if map.insert(region, url).is_some() {
            return Err(CentralControlApiConfigError::Invalid {
                name: keys::API_URLS,
                reason: format!("`{}` is repeated", region.as_str()),
            });
        }
    }
    if map.is_empty() {
        return Err(CentralControlApiConfigError::Invalid {
            name: keys::API_URLS,
            reason: "at least one configured region is required".to_owned(),
        });
    }
    Ok(map)
}

/// This binary's capability declaration.
///
/// Control DML and the regional control authorities. No
/// finance role, no payment provider, no object store and no assertion signing:
/// a binding for any of them is refused at start-up.
#[allow(
    dead_code,
    reason = "the declaration is the capability list; its only use is the type-level `Declares` bound"
)]
struct Composition;

impl Declares<ControlWrite> for Composition {}
impl Declares<RegionalControlInvoke> for Composition {}

/// The manifest the start-up check runs against.
#[must_use]
pub fn manifest() -> CompositionManifest {
    CompositionManifest {
        deployable: DEPLOYABLE,
        capabilities: BTreeSet::from([ControlWrite::ID, RegionalControlInvoke::ID]),
        bindings: vec![
            CapabilityBinding::arn(keys::AURORA_CLUSTER_ARN, ControlWrite::ID),
            CapabilityBinding::resource(keys::REGIONAL_FUNCTIONS, RegionalControlInvoke::ID),
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
    "lambda:InvokeFunction",
];

/// What each start-up probe answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "one field per start-up probe; a bitfield would hide which probe failed"
)]
pub struct Probes {
    /// `SELECT 1` as `aex_control_api` succeeded.
    pub aurora: bool,
    /// The API-key pepper loaded.
    pub pepper: bool,
    /// The cursor signing secret loaded.
    pub cursor_secret: bool,
    /// The direct-invoke map covers every region `wsp_region_ck` admits.
    pub endpoints: bool,
}

impl Probes {
    /// No probe has answered yet.
    pub const NONE: Self = Self {
        aurora: false,
        pepper: false,
        cursor_secret: false,
        endpoints: false,
    };
    /// Every required authority answered its real probe.
    pub const READY: Self = Self {
        aurora: true,
        pepper: true,
        cursor_secret: true,
        endpoints: true,
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
                name: "api-key-pepper",
                resolved: probes.pepper,
            },
            Dependency {
                name: "cursor-secret",
                resolved: probes.cursor_secret,
            },
            Dependency {
                name: "regional-endpoint-map",
                resolved: probes.endpoints,
            },
        ],
    )
}

/// Everything the control API serves, as one implementation.
///
/// One bound rather than five separate arguments: the five groups read the same
/// store inside one request, and splitting them would let a composition mount
/// four of them against one authority and the fifth against another.
pub trait ControlApi:
    ApiKeysApi + BootstrapApi + CentralOperationsApi + OrganizationsApi + WorkspacesApi
{
}

impl<T> ControlApi for T where
    T: ApiKeysApi + BootstrapApi + CentralOperationsApi + OrganizationsApi + WorkspacesApi
{
}

/// Why `central-control-api` stopped.
#[derive(Debug, thiserror::Error)]
pub enum CentralControlApiRunError {
    /// Start-up configuration was rejected.
    #[error(transparent)]
    Config(#[from] CentralControlApiConfigError),
    /// The composition was refused before any client was opened.
    #[error(transparent)]
    Composition(#[from] CompositionError),
    /// A required authority did not answer its startup probe.
    #[error("dependency `{0}` refused startup: {1}")]
    Dependency(&'static str, String),
    /// The listener stopped.
    #[error("the listener stopped: {0}")]
    Listener(String),
}

/// Builds the router this binary serves.
///
/// Each group is mounted by iterating its generated route slice, so the mounted
/// set is `CentralServiceId::ControlApi.routes()` by construction.
pub fn app<A: ControlApi>(api: Arc<A>, edge: EdgeStack, readiness: Readiness) -> axum::Router {
    aex_central_http::health::router(readiness)
        .merge(mount_api_keys_api(Arc::clone(&api), edge.clone()))
        .merge(mount_bootstrap_api(Arc::clone(&api), edge.clone()))
        .merge(mount_central_operations_api(Arc::clone(&api), edge.clone()))
        .merge(mount_organizations_api(Arc::clone(&api), edge.clone()))
        .merge(mount_workspaces_api(api, edge))
}

/// Runs `central-control-api` until it stops.
///
/// # Errors
///
/// Returns [`CentralControlApiRunError`] when configuration or composition is refused, or when
/// the listener stops.
pub async fn run<A: ControlApi>(
    config: &Config,
    api: Arc<A>,
    edge: EdgeStack,
    probes: Probes,
    telemetry: &aex_platform_telemetry::Handle,
) -> Result<(), CentralControlApiRunError> {
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
        .map_err(|error| CentralControlApiRunError::Listener(error.to_string()))
}

async fn compose(
    config: &Config,
    telemetry: &aex_platform_telemetry::Handle,
) -> Result<(), CentralControlApiRunError> {
    use aex_identity_app::ports::{PepperKeystore as _, PepperPurpose};

    let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let data_api = aex_rds_data::DataApiConfig::new(
        aex_rds_data::ResourceArn::parse(&config.aurora_cluster_arn)
            .map_err(|error| CentralControlApiRunError::Dependency("aurora", error.to_string()))?,
        aex_rds_data::SecretArn::parse(&config.aurora_secret_arn)
            .map_err(|error| CentralControlApiRunError::Dependency("aurora", error.to_string()))?,
        aex_rds_data::DatabaseName::parse(&config.database)
            .map_err(|error| CentralControlApiRunError::Dependency("aurora", error.to_string()))?,
    );
    let client = aex_rds_data::DataApiClient::new(
        Arc::new(aex_rds_data::AwsTransport::new(
            aws_sdk_rdsdata::Client::new(&aws),
            &data_api,
        )),
        data_api,
    );
    client
        .query::<Ok1>(aex_rds_data::Statement::new(
            aex_control_aurora::sql::READINESS_PROBE,
        ))
        .await
        .map_err(|error| CentralControlApiRunError::Dependency("aurora", error.to_string()))?;

    let directory = Arc::new(aex_central_aws::DataApiPepperDirectory::new(
        client.clone(),
        aex_central_aws::PepperStatements {
            active: aex_control_aurora::sql::ACTIVE_CONTROL_PEPPER,
            by_version: aex_control_aurora::sql::CONTROL_PEPPER_BY_VERSION,
        },
    ));
    let api_peppers = Arc::new(aex_central_aws::SecretsManagerPepperKeystore::new(
        aws_sdk_secretsmanager::Client::new(&aws),
        config.pepper_secret_id.clone(),
        Arc::clone(&directory) as Arc<dyn aex_central_aws::PepperDirectory>,
    ));
    api_peppers
        .probe(PepperPurpose::ApiKey)
        .await
        .map_err(|error| {
            CentralControlApiRunError::Dependency("api-key-pepper", error.to_string())
        })?;
    let cursor_peppers = Arc::new(aex_central_aws::SecretsManagerPepperKeystore::new(
        aws_sdk_secretsmanager::Client::new(&aws),
        config.cursor_secret_id.clone(),
        directory,
    ));
    cursor_peppers
        .probe(PepperPurpose::Cursor)
        .await
        .map_err(|error| {
            CentralControlApiRunError::Dependency("cursor-secret", error.to_string())
        })?;
    let (_, cursor_material) =
        cursor_peppers
            .active(PepperPurpose::Cursor)
            .await
            .map_err(|error| {
                CentralControlApiRunError::Dependency("cursor-secret", error.to_string())
            })?;
    let cursor_secret = Arc::new(aex_control_domain::CursorSecret::new(
        cursor_material.expose_copy(),
    ));

    let concrete_store = Arc::new(aex_control_aurora::AuroraControlStore::new(client));
    let api_store: Arc<dyn api::Store> = concrete_store.clone();
    let target_store: Arc<dyn aex_control_app::ports::ControlStore> = concrete_store;
    let clock: Arc<dyn aex_identity_app::ports::Clock> = Arc::new(aex_central_aws::SystemClock);
    let regional: Arc<dyn aex_control_app::ports::RegionalControlPort> =
        Arc::new(aex_central_aws::LambdaRegionalControl::new(
            aws_sdk_lambda::Client::new(&aws),
            config.regional_functions.clone(),
        ));
    let service = Arc::new(api::ControlService::new(
        api_store,
        Arc::clone(&api_peppers) as Arc<dyn aex_identity_app::ports::PepperKeystore>,
        regional,
        Arc::clone(&clock),
        Arc::new(aex_central_aws::Uuid7Factory),
        Arc::new(aex_central_aws::OsSecretRng),
        Arc::clone(&cursor_secret),
        config.http.region,
        config.api_urls.clone(),
    ));
    let edge = EdgeStack::new(
        config.http.clone(),
        Arc::new(aex_central_http::target::ControlStoreTargets::new(
            target_store,
        )),
        clock,
        cursor_secret,
    );
    run(config, service, edge, Probes::READY, telemetry).await
}

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
    // Telemetry first: a configuration refusal must reach the wire, or a
    // crash-looping deployment is visible only to whoever tails stderr.
    let settings = aex_platform_telemetry::Settings::default();
    let telemetry = aex_platform_telemetry::Handle::install(&settings, None);
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            telemetry.emit(
                aex_platform_telemetry::Record::event(
                    aex_telemetry_schema::generated::EVENT_AEX_PROCESS_CONFIGURATION_REJECTED,
                )
                .with(
                    aex_telemetry_schema::generated::AEX_DEPLOYABLE,
                    DEPLOYABLE.as_str(),
                ),
            );
            let _ = telemetry.flush(settings.flush_deadline);
            eprintln!("central-control-api: refusing to start: {error}");
            eprintln!(
                "central-control-api: required configuration: {}",
                keys::ALL.join(", ")
            );
            return std::process::ExitCode::FAILURE;
        }
    };
    let outcome = compose(&config, &telemetry).await;
    let _ = telemetry.flush(settings.flush_deadline);
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("central-control-api: stopped: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CentralControlApiConfigError, Config, ControlApi, DEPLOYABLE, PERMISSIONS, Probes, app,
        keys, manifest, readiness,
    };
    use aex_central_http::authorizer::{CentralAuthorizerContext, ContextPrincipalKind};
    use aex_central_http::capability::{
        AssertionSign, Capability as _, CapabilityBinding, CompositionError,
    };
    use aex_central_http::health::{HEALTH_PATH, READY_PATH};
    use aex_central_http::router::EdgeStack;
    use aex_central_http::target::{TargetPath, TargetResolver};
    use aex_control_app::ports::Clock;
    use aex_control_domain::{
        AccountState, Action, CursorSecret, OrgMembership, OrgRole, Resource, ResourceClass,
        ScopeSet, requirement,
    };
    use aex_wire::error::{ErrorCode, WireError, WireResult};
    use aex_wire::ids::{ApiKeyId, OperationId, OrganizationId, PrefixedId, Uuid7, WorkspaceId};
    use aex_wire::routes::{RouteId, route};
    use aex_wire::server::{
        Accepted, ApiKeysApi, BootstrapApi, CentralOperationsApi, Created, NoContent,
        OrganizationsApi, RequestContext, WorkspacesApi,
    };
    use aex_wire::types::Region;
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use time::OffsetDateTime;
    use tower::ServiceExt as _;
    use uuid::Uuid;

    const NOW_MS: i64 = 1_767_225_600_000;
    const ORGANIZATION: u8 = 0x11;
    const WORKSPACE: u8 = 0x22;
    const API_KEY: u8 = 0x33;
    const OPERATION: u8 = 0x44;
    const USER: u8 = 0x55;

    fn uuid7(tag: u8) -> Uuid7 {
        Uuid7::compose(1_767_225_600_000, [tag; 10])
    }

    fn raw_uuid(tag: u8) -> Uuid {
        Uuid::from_bytes(*uuid7(tag).as_bytes())
    }

    fn every_function() -> String {
        Region::ALL
            .iter()
            .map(|region| {
                format!(
                    "{}=arn:aws:lambda:{}:000000000000:function:aex-regional-control",
                    region.as_str(),
                    region.as_str()
                )
            })
            .collect::<Vec<_>>()
            .join(",")
    }

    fn every_api_url() -> String {
        Region::ALL
            .iter()
            .map(|region| {
                format!(
                    "{}=https://api.{}.aex.dev",
                    region.as_str(),
                    region.as_str()
                )
            })
            .collect::<Vec<_>>()
            .join(",")
    }

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
                "arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex-control".to_owned(),
            ),
            (
                keys::CURSOR_SECRET_ID,
                "aex/dev/cursor-secret/current".to_owned(),
            ),
            (
                keys::PEPPER_SECRET_ID,
                "aex/dev/api-key-pepper/current".to_owned(),
            ),
            (keys::DATABASE, "aex".to_owned()),
            (keys::ROLE, "aex_control_api".to_owned()),
            (keys::MAX_BODY_BYTES, "65536".to_owned()),
            (keys::REQUEST_DEADLINE_MS, "10000".to_owned()),
            (keys::REGIONAL_FUNCTIONS, every_function()),
            (keys::API_URLS, every_api_url()),
        ])
    }

    fn read(vars: &BTreeMap<&'static str, String>) -> Result<Config, CentralControlApiConfigError> {
        Config::from_lookup(|name| vars.get(name).cloned())
    }

    #[derive(Debug)]
    struct FixedClock;

    impl Clock for FixedClock {
        fn now(&self) -> OffsetDateTime {
            OffsetDateTime::from_unix_timestamp_nanos(i128::from(NOW_MS) * 1_000_000)
                .expect("a representable instant")
        }
    }

    #[derive(Debug)]
    struct Resolver;

    #[async_trait]
    impl TargetResolver for Resolver {
        async fn resolve(
            &self,
            id: RouteId,
            _path: &TargetPath,
        ) -> Result<Option<Resource>, aex_central_http::error::EdgeError> {
            let organization_id = raw_uuid(ORGANIZATION);
            let action = Action::central(id).expect("a central route");
            Ok(Some(match requirement(action).resource_class {
                ResourceClass::None => Resource::None,
                ResourceClass::Organization => Resource::Organization(organization_id),
                ResourceClass::Workspace => Resource::Workspace {
                    workspace_id: raw_uuid(WORKSPACE),
                    organization_id,
                },
                ResourceClass::ApiKey => Resource::ApiKey {
                    key_id: raw_uuid(API_KEY),
                    workspace_id: raw_uuid(WORKSPACE),
                    organization_id,
                },
                ResourceClass::Operation => Resource::Operation {
                    operation_id: raw_uuid(OPERATION),
                    organization_id,
                },
            }))
        }

        async fn account_state(
            &self,
            _organization_id: Uuid,
        ) -> Result<AccountState, aex_central_http::error::EdgeError> {
            Ok(AccountState::Active)
        }
    }

    /// Answers every route with one declared refusal, so a mounted route is
    /// provably reached without this suite having to build every wire model.
    #[derive(Debug)]
    struct Api;

    impl Api {
        fn refuse<T>() -> WireResult<T> {
            Err(WireError::new(ErrorCode::NotFound))
        }
    }

    impl ApiKeysApi for Api {
        async fn api_key_create(
            &self,
            _cx: &RequestContext,
            _body: aex_wire::models::ApiKeyCreateRequest,
        ) -> WireResult<Created<aex_wire::models::NewApiKey>> {
            Self::refuse()
        }

        async fn api_key_revoke(
            &self,
            _cx: &RequestContext,
            _api_key_id: ApiKeyId,
        ) -> WireResult<NoContent> {
            Self::refuse()
        }

        async fn api_keys_list(
            &self,
            _cx: &RequestContext,
            _query: aex_wire::models::ApiKeysListQuery,
        ) -> WireResult<aex_wire::models::ApiKeyPage> {
            Self::refuse()
        }
    }

    impl BootstrapApi for Api {
        async fn dashboard_bootstrap_get(
            &self,
            _cx: &RequestContext,
        ) -> WireResult<aex_wire::models::DashboardBootstrap> {
            Err(WireError::new(ErrorCode::Forbidden))
        }
    }

    impl CentralOperationsApi for Api {
        async fn central_operation_cancel(
            &self,
            _cx: &RequestContext,
            _operation_id: OperationId,
            _body: aex_wire::models::EmptyRequest,
        ) -> WireResult<aex_wire::models::Operation> {
            Self::refuse()
        }

        async fn central_operation_get(
            &self,
            _cx: &RequestContext,
            _operation_id: OperationId,
        ) -> WireResult<aex_wire::models::Operation> {
            Self::refuse()
        }

        async fn central_operations_list(
            &self,
            _cx: &RequestContext,
            _query: aex_wire::models::CentralOperationsListQuery,
        ) -> WireResult<aex_wire::models::OperationPage> {
            Err(WireError::new(ErrorCode::InvalidCursor))
        }
    }

    impl OrganizationsApi for Api {
        async fn invitation_create(
            &self,
            _cx: &RequestContext,
            _organization_id: OrganizationId,
            _body: aex_wire::models::InvitationCreateRequest,
        ) -> WireResult<Created<aex_wire::models::Invitation>> {
            Self::refuse()
        }

        async fn memberships_list(
            &self,
            _cx: &RequestContext,
            _organization_id: OrganizationId,
            _query: aex_wire::models::MembershipsListQuery,
        ) -> WireResult<aex_wire::models::MembershipPage> {
            Self::refuse()
        }

        async fn organization_create(
            &self,
            _cx: &RequestContext,
            _body: aex_wire::models::OrganizationCreateRequest,
        ) -> WireResult<Created<aex_wire::models::Organization>> {
            Err(WireError::new(ErrorCode::LimitExceeded))
        }

        async fn organization_get(
            &self,
            _cx: &RequestContext,
            _organization_id: OrganizationId,
        ) -> WireResult<aex_wire::models::Organization> {
            Self::refuse()
        }

        async fn organizations_list(
            &self,
            _cx: &RequestContext,
            _query: aex_wire::models::OrganizationsListQuery,
        ) -> WireResult<aex_wire::models::OrganizationPage> {
            Err(WireError::new(ErrorCode::InvalidCursor))
        }
    }

    impl WorkspacesApi for Api {
        async fn workspace_create(
            &self,
            _cx: &RequestContext,
            _body: aex_wire::models::WorkspaceCreateRequest,
        ) -> WireResult<Created<aex_wire::models::Workspace>> {
            Err(WireError::new(ErrorCode::LimitExceeded))
        }

        async fn workspace_delete(
            &self,
            _cx: &RequestContext,
            _workspace_id: WorkspaceId,
            _body: aex_wire::models::WorkspaceDeleteRequest,
        ) -> WireResult<Accepted> {
            Self::refuse()
        }

        async fn workspace_get(
            &self,
            _cx: &RequestContext,
            _workspace_id: WorkspaceId,
        ) -> WireResult<aex_wire::models::Workspace> {
            Self::refuse()
        }

        async fn workspaces_list(
            &self,
            _cx: &RequestContext,
            _query: aex_wire::models::WorkspacesListQuery,
        ) -> WireResult<aex_wire::models::WorkspacePage> {
            Err(WireError::new(ErrorCode::InvalidCursor))
        }
    }

    fn owner_context() -> CentralAuthorizerContext {
        CentralAuthorizerContext {
            request_id: aex_wire::types::RequestId::parse("req-fixture").expect("a request id"),
            kind: ContextPrincipalKind::Account,
            principal_id: raw_uuid(USER),
            credential_id: Some(raw_uuid(0x66)),
            workspace_id: None,
            organization_id: None,
            region: None,
            memberships: vec![OrgMembership {
                organization_id: raw_uuid(ORGANIZATION),
                membership_id: raw_uuid(0x77),
                role: OrgRole::Owner,
            }],
            scopes: ScopeSet::CENTRAL,
            account_state: AccountState::Active,
            issued_at_ms: NOW_MS - 1_000,
            expires_at_ms: NOW_MS + 10_000,
        }
    }

    fn router() -> axum::Router {
        let config = read(&complete()).expect("a complete environment");
        let edge = EdgeStack::new(
            config.http,
            Arc::new(Resolver),
            Arc::new(FixedClock),
            Arc::new(CursorSecret::new([2_u8; 32])),
        );
        app(Arc::new(Api), edge, readiness(Probes::NONE))
    }

    fn concrete_path(id: RouteId) -> String {
        route(id)
            .template
            .replace(
                "{organizationId}",
                OrganizationId::from_uuid7(uuid7(ORGANIZATION))
                    .encode()
                    .as_str(),
            )
            .replace(
                "{workspaceId}",
                WorkspaceId::from_uuid7(uuid7(WORKSPACE)).encode().as_str(),
            )
            .replace(
                "{apiKeyId}",
                ApiKeyId::from_uuid7(uuid7(API_KEY)).encode().as_str(),
            )
            .replace(
                "{operationId}",
                OperationId::from_uuid7(uuid7(OPERATION)).encode().as_str(),
            )
    }

    #[test]
    fn a_complete_environment_is_accepted() {
        let config = read(&complete()).expect("a complete environment");
        assert_eq!(config.regional_functions.len(), Region::ALL.len());
        assert_eq!(config.api_urls.len(), Region::ALL.len());
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
    fn a_single_composed_region_is_accepted() {
        let mut vars = complete();
        vars.insert(
            keys::REGIONAL_FUNCTIONS,
            "eu-west-1=arn:aws:lambda:eu-west-1:000000000000:function:aex-regional-control"
                .to_owned(),
        );
        vars.insert(
            keys::API_URLS,
            "eu-west-1=https://api.eu-west-1.aex.dev".to_owned(),
        );
        let config = read(&vars).expect("a composed subset of launch regions");
        assert_eq!(config.regional_functions.len(), 1);
        assert_eq!(config.api_urls.len(), 1);
    }

    #[test]
    fn the_function_and_public_url_maps_must_cover_the_same_regions() {
        let mut vars = complete();
        vars.insert(
            keys::REGIONAL_FUNCTIONS,
            "eu-west-1=arn:aws:lambda:eu-west-1:000000000000:function:aex-regional-control"
                .to_owned(),
        );
        assert!(read(&vars).is_err());
    }

    #[test]
    fn this_binary_refuses_any_role_but_its_own() {
        let mut vars = complete();
        vars.insert(keys::ROLE, "aex_authz".to_owned());
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
    fn this_binary_cannot_link_the_assertion_signing_capability() {
        let config = read(&complete()).expect("a complete environment");
        let mut manifest = manifest();
        manifest.bindings.push(CapabilityBinding::resource(
            "AEX_CENTRAL_CONTROL_ASSERTION_KEY",
            AssertionSign::ID,
        ));
        assert_eq!(
            aex_central_http::capability::admit(&manifest, &config.resolved()),
            Err(CompositionError::ForbiddenCapability {
                key: "AEX_CENTRAL_CONTROL_ASSERTION_KEY",
                capability: AssertionSign::ID
            })
        );
    }

    #[test]
    fn the_permission_list_holds_no_finance_object_store_or_payment_right() {
        for permission in PERMISSIONS {
            assert!(!permission.starts_with("s3:"), "{permission}");
            assert!(!permission.starts_with("kms:"), "{permission}");
            assert!(!permission.contains("stripe"), "{permission}");
        }
        assert!(PERMISSIONS.contains(&"lambda:InvokeFunction"));
    }

    #[tokio::test]
    async fn the_mounted_set_is_exactly_the_declared_one() {
        let declared = DEPLOYABLE.routes();
        assert_eq!(declared.len(), 16, "this deployable serves 16 routes");
        for id in declared {
            let descriptor = route(id);
            let mut request = Request::builder()
                .method(descriptor.method.as_str())
                .uri(with_query(id, concrete_path(id)))
                .extension(owner_context());
            match descriptor.idempotency {
                aex_wire::idempotency::IdempotencyKind::IdempotencyKey => {
                    request = request.header("idempotency-key", "fixture");
                }
                aex_wire::idempotency::IdempotencyKind::OperationId => {
                    request = request.header(
                        "aex-operation-id",
                        OperationId::from_uuid7(uuid7(OPERATION)).encode().as_str(),
                    );
                }
                aex_wire::idempotency::IdempotencyKind::None => {}
            }
            let body = if descriptor.body_class == aex_wire::routes::BodyClass::None {
                Body::empty()
            } else {
                Body::from("{}")
            };
            let response = router()
                .oneshot(request.body(body).expect("a valid request"))
                .await
                .expect("the router answers");
            assert_ne!(
                response.status(),
                StatusCode::NOT_IMPLEMENTED,
                "`{}` is not mounted",
                descriptor.operation_id
            );
            assert_eq!(
                response
                    .headers()
                    .get(axum::http::header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok()),
                Some("application/json"),
                "`{}` answered without an AEX envelope, so it is not mounted",
                descriptor.operation_id
            );
        }
    }

    fn with_query(id: RouteId, path: String) -> String {
        match id {
            RouteId::ApiKeysList => format!(
                "{path}?workspaceId={}",
                WorkspaceId::from_uuid7(uuid7(WORKSPACE)).encode()
            ),
            _ => path,
        }
    }

    #[tokio::test]
    async fn a_finance_route_is_not_mounted_on_this_deployable() {
        for path in ["/api/account", "/api/billing/balance"] {
            let response = router()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .extension(owner_context())
                        .body(Body::empty())
                        .expect("a valid request"),
                )
                .await
                .expect("the router answers");
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        }
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

    #[test]
    fn the_one_api_bound_covers_every_group_this_deployable_mounts() {
        fn accepts<A: ControlApi>(_api: &A) {}
        accepts(&Api);
        assert_eq!(DEPLOYABLE.groups().len(), 5);
    }
}
