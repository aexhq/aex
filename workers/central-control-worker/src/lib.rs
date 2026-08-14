//! `control-projection-worker` composition (Rust Lambda ZIP).
//!
//! Transaction-wake-driven reconciliation for the central control plane. It
//! projects the personal workspace, billing state, and API-key authorization
//! into the one launch region.
//!
//! Two properties are structural rather than careful:
//!
//! * every launch duty is named in [`Handler::ALL`], while legacy topics are
//!   explicitly rejected and remain claimable rather than being acknowledged;
//! * every accepted wake names an exact Aurora transaction and anchor, so a
//!   pre-commit race, rollback and duplicate delivery have distinct outcomes.
//!
//! # Why this is a library with a short binary
//!
//! The composition — [`handle_event`], the duty table and the drain each one
//! calls — is what a release has to be able to prove, and a `main.rs`-only
//! crate can be proved only by starting a process and reading its stderr.
//! Everything but `fn main` therefore lives here so `tests/composition.rs` can
//! drive both trigger shapes directly.

use std::collections::{BTreeMap, BTreeSet};

use aex_central_http::capability::{
    Capability as _, CapabilityBinding, CompositionError, CompositionManifest, ControlWakeInvoke,
    ControlWrite, Declares,
};
use aex_central_http::config::{CentralServiceId, DeploymentPlane};
use aex_central_http::health::{Dependency, Readiness};
use aex_control_domain::Topic;
use aex_wire::types::Region;

pub mod runtime;

/// The deployable this binary is.
pub const DEPLOYABLE: CentralServiceId = CentralServiceId::ControlWorker;

/// Environment keys, all inside the declared namespace.
pub mod keys {
    /// The plane's account id, for the ARN binding check.
    pub const ACCOUNT_ID: &str = "AEX_CENTRAL_CONTROL_WORKER_ACCOUNT_ID";
    /// The Aurora cluster.
    pub const AURORA_CLUSTER_ARN: &str = "AEX_CENTRAL_CONTROL_WORKER_AURORA_CLUSTER_ARN";
    /// The Aurora credentials secret.
    pub const AURORA_SECRET_ARN: &str = "AEX_CENTRAL_CONTROL_WORKER_AURORA_SECRET_ARN";
    /// The one launch-region authorization projection table.
    pub const AUTHZ_PROJECTION_TABLE: &str = "AEX_CENTRAL_CONTROL_WORKER_AUTHZ_PROJECTION_TABLE";
    /// How many messages one batch claims.
    pub const BATCH_SIZE: &str = "AEX_CENTRAL_CONTROL_WORKER_BATCH_SIZE";
    /// This worker's exact live alias ARN, used for bounded continuation.
    pub const FUNCTION_ARN: &str = "AEX_CENTRAL_CONTROL_WORKER_FUNCTION_ARN";
    /// The database name.
    pub const DATABASE: &str = "AEX_CENTRAL_CONTROL_WORKER_DATABASE";
    /// How long a claim lease lasts.
    pub const LEASE_MS: &str = "AEX_CENTRAL_CONTROL_WORKER_LEASE_MS";
    /// The deployment plane.
    pub const PLANE: &str = "AEX_CENTRAL_CONTROL_WORKER_PLANE";
    /// The bound region.
    pub const REGION: &str = "AEX_CENTRAL_CONTROL_WORKER_REGION";
    /// Every key this binary reads, for the totality test.
    pub const ALL: &[&str] = &[
        ACCOUNT_ID,
        AURORA_CLUSTER_ARN,
        AURORA_SECRET_ARN,
        AUTHZ_PROJECTION_TABLE,
        BATCH_SIZE,
        FUNCTION_ARN,
        DATABASE,
        LEASE_MS,
        PLANE,
        REGION,
    ];
}

/// The largest batch one invocation may claim.
const MAX_BATCH: u64 = 100;
/// The longest a claim lease may last.
const MAX_LEASE_MS: u64 = 900_000;

/// Why `control-projection-worker` refused to start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CentralControlWorkerConfigError {
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
    /// Which deployment plane.
    pub plane: DeploymentPlane,
    /// Which region.
    pub region: Region,
    /// The plane's account id.
    pub account_id: String,
    /// The Aurora cluster.
    pub aurora_cluster_arn: String,
    /// The Aurora credentials secret.
    pub aurora_secret_arn: String,
    /// This worker's exact live alias ARN.
    pub function_arn: String,
    /// The one launch-region authorization projection table.
    pub authz_projection_table: String,
    /// The database name.
    pub database: String,
    /// How many messages one batch claims.
    pub batch_size: u32,
    /// How long a claim lease lasts.
    pub lease_ms: u64,
}

impl Config {
    /// Reads and validates the configuration from the process environment.
    ///
    /// # Errors
    ///
    /// Returns [`CentralControlWorkerConfigError`] naming the first variable it refused.
    pub fn from_env() -> Result<Self, CentralControlWorkerConfigError> {
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
    pub fn from_lookup<F>(lookup: F) -> Result<Self, CentralControlWorkerConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let plane_raw = required(&lookup, keys::PLANE)?;
        let plane = DeploymentPlane::parse(&plane_raw).ok_or_else(|| {
            CentralControlWorkerConfigError::Invalid {
                name: keys::PLANE,
                reason: format!("expected `dev` or `prd`, got `{plane_raw}`"),
            }
        })?;
        let region_raw = required(&lookup, keys::REGION)?;
        let region = Region::from_name(&region_raw).ok_or_else(|| {
            CentralControlWorkerConfigError::Invalid {
                name: keys::REGION,
                reason: format!("expected a launch region, got `{region_raw}`"),
            }
        })?;
        let account_id = required(&lookup, keys::ACCOUNT_ID)?;
        if account_id.len() != 12 || !account_id.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(CentralControlWorkerConfigError::Invalid {
                name: keys::ACCOUNT_ID,
                reason: "expected a 12-digit AWS account id".to_owned(),
            });
        }
        let batch_size = bounded(&lookup, keys::BATCH_SIZE, 1, MAX_BATCH)?;
        let lease_ms = bounded(&lookup, keys::LEASE_MS, 1, MAX_LEASE_MS)?;
        Ok(Self {
            plane,
            region,
            account_id: account_id.clone(),
            aurora_cluster_arn: required(&lookup, keys::AURORA_CLUSTER_ARN)?,
            aurora_secret_arn: required(&lookup, keys::AURORA_SECRET_ARN)?,
            function_arn: lambda_alias_arn(
                keys::FUNCTION_ARN,
                &required(&lookup, keys::FUNCTION_ARN)?,
                plane,
                region,
                &account_id,
            )?,
            authz_projection_table: required(&lookup, keys::AUTHZ_PROJECTION_TABLE)?,
            database: required(&lookup, keys::DATABASE)?,
            batch_size: u32::try_from(batch_size).unwrap_or(1),
            lease_ms,
        })
    }

    /// The resolved values the composition check runs over.
    #[must_use]
    pub fn resolved(&self) -> aex_central_http::capability::ResolvedConfig {
        aex_central_http::capability::ResolvedConfig {
            deployable: DEPLOYABLE.as_str().to_owned(),
            plane: self.plane,
            region: self.region,
            account_id: self.account_id.clone(),
            values: BTreeMap::from([
                (
                    keys::AURORA_CLUSTER_ARN.to_owned(),
                    self.aurora_cluster_arn.clone(),
                ),
                (
                    keys::AURORA_SECRET_ARN.to_owned(),
                    self.aurora_secret_arn.clone(),
                ),
                (keys::FUNCTION_ARN.to_owned(), self.function_arn.clone()),
                (
                    keys::AUTHZ_PROJECTION_TABLE.to_owned(),
                    self.authz_projection_table.clone(),
                ),
            ]),
        }
    }
}

fn required<F>(lookup: &F, name: &'static str) -> Result<String, CentralControlWorkerConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    match lookup(name) {
        Some(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(CentralControlWorkerConfigError::Missing(name)),
    }
}

fn bounded<F>(
    lookup: &F,
    name: &'static str,
    min: u64,
    max: u64,
) -> Result<u64, CentralControlWorkerConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let raw = required(lookup, name)?;
    let value = raw
        .parse::<u64>()
        .map_err(|_| CentralControlWorkerConfigError::Invalid {
            name,
            reason: format!("expected an integer, got `{raw}`"),
        })?;
    if value < min || value > max {
        return Err(CentralControlWorkerConfigError::Invalid {
            name,
            reason: format!("expected {min}..={max}, got `{value}`"),
        });
    }
    Ok(value)
}

fn lambda_alias_arn(
    name: &'static str,
    raw: &str,
    plane: DeploymentPlane,
    region: Region,
    account_id: &str,
) -> Result<String, CentralControlWorkerConfigError> {
    let fields = raw.split(':').collect::<Vec<_>>();
    let expected_function = format!("aex-{}-control-projection-worker", plane.as_str());
    if fields.len() == 8
        && fields[0] == "arn"
        && matches!(
            fields[1],
            "aws" | "aws-cn" | "aws-us-gov" | "aws-iso" | "aws-iso-b"
        )
        && fields[2] == "lambda"
        && fields[3] == region.as_str()
        && fields[4] == account_id
        && fields[5] == "function"
        && fields[6] == expected_function
        && fields[7] == "live"
    {
        Ok(raw.to_owned())
    } else {
        Err(CentralControlWorkerConfigError::Invalid {
            name,
            reason: "expected this plane's exact control-projection-worker `live` alias ARN"
                .to_owned(),
        })
    }
}

/// Every event-wake-driven duty this worker performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Handler {
    /// Publish and activate the personal workspace.
    WorkspaceProvisionReconcile,
    /// Project a finance account state and epoch to one workspace.
    AccountStateProject,
    /// Project a newly minted key's authorization row to its region.
    ApiKeyAuthorizationProject,
    /// Project an advanced revocation epoch to every region.
    AuthorizationEpochProject,
    /// Sweep expired replay records.
    IdempotencyGc,
    /// Sweep dispatched outbox rows.
    OutboxGc,
    /// Check whether a pepper may retire.
    PepperRetireCheck,
}

impl Handler {
    /// Every duty.
    pub const ALL: [Self; 7] = [
        Self::WorkspaceProvisionReconcile,
        Self::AccountStateProject,
        Self::ApiKeyAuthorizationProject,
        Self::AuthorizationEpochProject,
        Self::IdempotencyGc,
        Self::OutboxGc,
        Self::PepperRetireCheck,
    ];

    /// The stable duty name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WorkspaceProvisionReconcile => "workspace.provision.reconcile",
            Self::AccountStateProject => "account.state.project",
            Self::ApiKeyAuthorizationProject => "api_key.authorization.project",
            Self::AuthorizationEpochProject => "authorization.epoch.project",
            Self::IdempotencyGc => "idempotency.gc",
            Self::OutboxGc => "outbox.gc",
            Self::PepperRetireCheck => "pepper.retire.check",
        }
    }

    /// The duty that consumes `topic`.
    ///
    /// Retired topics have no duty in the launch projection worker.
    #[must_use]
    pub const fn for_topic(topic: Topic) -> Option<Self> {
        match topic {
            Topic::WorkspaceProvisionRequested => Some(Self::WorkspaceProvisionReconcile),
            Topic::AccountStateChanged => Some(Self::AccountStateProject),
            // Creation and revocation publish the same row, but they stay
            // separate duties: one duty per topic is what makes a failing
            // publication attributable to the event that caused it.
            Topic::ApiKeyCreated => Some(Self::ApiKeyAuthorizationProject),
            Topic::AuthorizationEpochChanged => Some(Self::AuthorizationEpochProject),
            Topic::WorkspaceDeleteRequested
            | Topic::InvitationEmailRequested
            | Topic::AuthorizationSigningKeyPublished => None,
        }
    }
}

/// This binary's capability declaration.
#[allow(
    dead_code,
    reason = "the declaration is the capability list; its only use is the type-level `Declares` bound"
)]
struct Composition;

impl Declares<ControlWrite> for Composition {}
impl Declares<ControlWakeInvoke> for Composition {}

/// The manifest the start-up check runs against.
#[must_use]
pub fn manifest() -> CompositionManifest {
    CompositionManifest {
        deployable: DEPLOYABLE,
        capabilities: BTreeSet::from([ControlWrite::ID, ControlWakeInvoke::ID]),
        bindings: vec![
            CapabilityBinding::arn(keys::AURORA_CLUSTER_ARN, ControlWrite::ID),
            CapabilityBinding::arn(keys::AURORA_SECRET_ARN, ControlWrite::ID),
            CapabilityBinding::arn(keys::FUNCTION_ARN, ControlWakeInvoke::ID),
            CapabilityBinding::resource(keys::AUTHZ_PROJECTION_TABLE, ControlWrite::ID),
        ],
    }
}

/// The IAM permissions this deployable requires, as a reviewable list.
///
/// No finance role, no object store, no payment provider. The worker repairs the
/// control plane and nothing else.
pub const PERMISSIONS: &[&str] = &[
    "rds-data:BeginTransaction",
    "rds-data:CommitTransaction",
    "rds-data:ExecuteStatement",
    "rds-data:RollbackTransaction",
    "dynamodb:GetItem",
    "dynamodb:PutItem",
    // Lambda's async on-failure delivery assumes this execution role. There is
    // no consumer permission and no event-source mapping for this queue.
    "sqs:SendMessage",
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
    /// `SELECT 1` as `aex_control_worker` succeeded.
    pub aurora: bool,
    /// The exact worker alias is configured for continuation.
    pub wake: bool,
    /// The launch-region authorization projection answered.
    pub projection: bool,
}

impl Probes {
    /// No probe has answered yet.
    pub const NONE: Self = Self {
        aurora: false,
        wake: false,
        projection: false,
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
                name: "control-wake-function",
                resolved: probes.wake,
            },
            Dependency {
                name: "regional-authz-projection",
                resolved: probes.projection,
            },
        ],
    )
}

/// Why `control-projection-worker` stopped.
#[derive(Debug, thiserror::Error)]
pub enum CentralControlWorkerRunError {
    /// Start-up configuration was rejected.
    #[error(transparent)]
    Config(#[from] CentralControlWorkerConfigError),
    /// The composition was refused before any client was opened.
    #[error(transparent)]
    Composition(#[from] CompositionError),
    /// A required authority refused its real startup probe.
    #[error("dependency `{0}` refused startup: {1}")]
    Dependency(&'static str, String),
    /// The runtime stopped.
    #[error("the runtime stopped: {0}")]
    Runtime(String),
}

/// Builds the router this binary serves.
///
/// This worker has no public surface; the two internal probes are the whole
/// mounted set, and they are what the deployment health check polls.
pub fn app(readiness: Readiness) -> axum::Router {
    aex_central_http::health::router(readiness)
}

/// Runs `control-projection-worker` until it stops.
///
/// # Errors
///
/// Returns [`CentralControlWorkerRunError`] when configuration or composition is refused, or when
/// the runtime stops.
pub async fn run(config: &Config) -> Result<(), CentralControlWorkerRunError> {
    aex_central_http::capability::admit(&manifest(), &config.resolved())?;
    tracing::info!(
        target: "aex::diagnostics",
        event_name = "process.started",
        deployable = DEPLOYABLE.as_str(),
        plane = config.plane.as_str(),
        region = config.region.as_str(),
        "process started"
    );
    let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let data_config = aex_rds_data::DataApiConfig::new(
        aex_rds_data::ResourceArn::parse(&config.aurora_cluster_arn).map_err(|error| {
            CentralControlWorkerRunError::Dependency("aurora", error.to_string())
        })?,
        aex_rds_data::SecretArn::parse(&config.aurora_secret_arn).map_err(|error| {
            CentralControlWorkerRunError::Dependency("aurora", error.to_string())
        })?,
        aex_rds_data::DatabaseName::parse(&config.database).map_err(|error| {
            CentralControlWorkerRunError::Dependency("aurora", error.to_string())
        })?,
    );
    let data = aex_rds_data::DataApiClient::new(
        std::sync::Arc::new(aex_rds_data::AwsTransport::new(
            aws_sdk_rdsdata::Client::new(&aws),
            &data_config,
        )),
        data_config,
    );
    data.query::<Ok1>(aex_rds_data::Statement::new(
        aex_control_aurora::sql::READINESS_PROBE,
    ))
    .await
    .map_err(|error| CentralControlWorkerRunError::Dependency("aurora", error.to_string()))?;

    let projection = std::sync::Arc::new(
        aex_session_dynamodb::projection_write::ProjectionWriter::new(
            aws_sdk_dynamodb::Client::new(&aws),
            config.authz_projection_table.clone(),
        ),
    );
    projection.probe().await.map_err(|error| {
        CentralControlWorkerRunError::Dependency("regional-authz-projection", error)
    })?;
    let concrete_store = std::sync::Arc::new(aex_control_aurora::AuroraControlStore::new(data));
    let store: std::sync::Arc<dyn runtime::Store> = concrete_store;
    let worker = std::sync::Arc::new(runtime::Worker::new(
        store,
        std::sync::Arc::new(runtime::LambdaWakeInvoker::new(
            aws_sdk_lambda::Client::new(&aws),
            config.function_arn.clone(),
        )),
        std::sync::Arc::new(runtime::TokioWakeDelay),
        projection,
        runtime::DrainSettings {
            region: config.region,
            owner: format!("{}:{}", DEPLOYABLE.as_str(), config.region.as_str()),
            batch: config.batch_size,
            lease: time::Duration::milliseconds(i64::try_from(config.lease_ms).unwrap_or(i64::MAX)),
        },
        std::sync::Arc::new(aex_central_aws::SystemClock),
    ));

    run_lambda(worker).await
}

async fn run_lambda(
    worker: std::sync::Arc<runtime::Worker>,
) -> Result<(), CentralControlWorkerRunError> {
    lambda_runtime::run(lambda_runtime::service_fn(
        move |event: lambda_runtime::LambdaEvent<serde_json::Value>| {
            let worker = std::sync::Arc::clone(&worker);
            async move { handle_event(worker.as_ref(), event.payload).await }
        },
    ))
    .await
    .map_err(|error| CentralControlWorkerRunError::Runtime(error.to_string()))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScheduledSweep {
    source: String,
    #[serde(rename = "detail-type")]
    detail_type: String,
    #[serde(rename = "detail")]
    _detail: ScheduledSweepDetail,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ScheduledSweepDetail {}

/// Routes an exact Aurora transaction wake or the recovery schedule to the bounded drain.
///
/// # Errors
///
/// Returns an error for malformed events, a pre-commit race, or any failed
/// effect so Lambda's asynchronous retry and on-failure destination observe it.
pub async fn handle_event(
    worker: &runtime::Worker,
    value: serde_json::Value,
) -> Result<serde_json::Value, String> {
    if let Ok(wake) = serde_json::from_value::<runtime::OutboxWake>(value.clone()) {
        worker.wake(&wake).await?;
        return Ok(serde_json::json!({}));
    }
    let sweep = serde_json::from_value::<ScheduledSweep>(value)
        .map_err(|_| "invalid_control_worker_event".to_owned())?;
    if sweep.source != "aex.scheduler" || sweep.detail_type != "aex.control_recovery" {
        return Err("invalid_control_worker_event".to_owned());
    }
    worker.tick().await?;
    Ok(serde_json::json!({}))
}

struct Ok1;

impl aex_rds_data::Row for Ok1 {
    fn from_record(record: &aex_rds_data::Record<'_>) -> Result<Self, aex_rds_data::DecodeError> {
        record.expect_arity(1)?;
        record.i64(0)?;
        Ok(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CentralControlWorkerConfigError, Config, Handler, PERMISSIONS, Probes, app, keys, manifest,
        readiness,
    };
    use aex_central_http::capability::{
        AssertionSign, Capability as _, CapabilityBinding, CompositionError,
    };
    use aex_central_http::health::READY_PATH;
    use aex_control_domain::Topic;
    use aex_wire::types::Region;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::collections::{BTreeMap, BTreeSet};
    use tower::ServiceExt as _;

    fn complete() -> BTreeMap<&'static str, String> {
        BTreeMap::from([
            (keys::PLANE, "prd".to_owned()),
            (keys::REGION, "eu-west-1".to_owned()),
            (keys::ACCOUNT_ID, "000000000000".to_owned()),
            (
                keys::AURORA_CLUSTER_ARN,
                "arn:aws:rds:eu-west-1:000000000000:cluster:aex".to_owned(),
            ),
            (
                keys::AURORA_SECRET_ARN,
                "arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex-worker".to_owned(),
            ),
            (
                keys::FUNCTION_ARN,
                "arn:aws:lambda:eu-west-1:000000000000:function:aex-prd-control-projection-worker:live"
                    .to_owned(),
            ),
            (
                keys::AUTHZ_PROJECTION_TABLE,
                "aex-prd-eu-west-1-regional-authz-projection".to_owned(),
            ),
            (keys::DATABASE, "aex".to_owned()),
            (keys::BATCH_SIZE, "10".to_owned()),
            (keys::LEASE_MS, "60000".to_owned()),
        ])
    }

    fn read(
        vars: &BTreeMap<&'static str, String>,
    ) -> Result<Config, CentralControlWorkerConfigError> {
        Config::from_lookup(|name| vars.get(name).cloned())
    }

    #[test]
    fn a_complete_environment_is_accepted() {
        let config = read(&complete()).expect("a complete environment");
        assert_eq!(config.batch_size, 10);
        assert_eq!(config.region, Region::EuWest1);
        assert_eq!(
            config.authz_projection_table,
            "aex-prd-eu-west-1-regional-authz-projection"
        );
        assert_eq!(
            config.function_arn,
            "arn:aws:lambda:eu-west-1:000000000000:function:aex-prd-control-projection-worker:live",
            "the Aurora continuation target is the deployed alias"
        );
    }

    #[test]
    fn the_wake_target_is_this_planes_exact_live_alias() {
        for arn in [
            "arn:aws:lambda:eu-west-1:111111111111:function:aex-prd-control-projection-worker:live",
            "arn:aws:lambda:eu-west-1:000000000000:function:aex-prd-some-other-worker:live",
            "arn:aws:lambda:eu-west-1:000000000000:function:aex-prd-control-projection-worker:canary",
            "arn:aws:lambda:eu-west-1:000000000000:function:aex-prd-central-control-worker:live",
        ] {
            let mut vars = complete();
            vars.insert(keys::FUNCTION_ARN, arn.to_owned());
            assert!(
                matches!(
                    read(&vars),
                    Err(CentralControlWorkerConfigError::Invalid { name, .. })
                        if name == keys::FUNCTION_ARN
                ),
                "accepted foreign wake target {arn}"
            );
        }
    }

    #[test]
    fn every_variable_is_required_and_named_when_absent() {
        for name in keys::ALL {
            let mut vars = complete();
            vars.remove(name);
            assert_eq!(
                read(&vars),
                Err(CentralControlWorkerConfigError::Missing(name)),
                "removing {name}"
            );
        }
    }

    #[test]
    fn a_batch_or_lease_outside_its_bound_is_refused() {
        for (name, value) in [
            (keys::BATCH_SIZE, "0"),
            (keys::BATCH_SIZE, "101"),
            (keys::LEASE_MS, "0"),
            (keys::LEASE_MS, "900001"),
        ] {
            let mut vars = complete();
            vars.insert(name, value.to_owned());
            assert!(read(&vars).is_err(), "{name}={value}");
        }
    }

    #[test]
    fn every_launch_outbox_topic_has_a_distinct_duty_and_retired_topics_do_not() {
        let handled: BTreeSet<Handler> = Topic::ALL
            .into_iter()
            .filter_map(Handler::for_topic)
            .collect();
        assert_eq!(handled.len(), 4, "two launch topics share one duty");
        for retired in [
            Topic::WorkspaceDeleteRequested,
            Topic::InvitationEmailRequested,
            Topic::AuthorizationSigningKeyPublished,
        ] {
            assert_eq!(Handler::for_topic(retired), None);
        }
        let mut names: Vec<&str> = Handler::ALL.iter().map(|it| it.as_str()).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "two duties share a name");
        for handler in handled {
            assert!(Handler::ALL.contains(&handler));
        }
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
            "AEX_CENTRAL_CONTROL_WORKER_ASSERTION_KEY",
            AssertionSign::ID,
        ));
        assert_eq!(
            aex_central_http::capability::admit(&manifest, &config.resolved()),
            Err(CompositionError::ForbiddenCapability {
                key: "AEX_CENTRAL_CONTROL_WORKER_ASSERTION_KEY",
                capability: AssertionSign::ID
            })
        );
    }

    #[test]
    fn the_permission_list_holds_no_finance_or_object_store_right() {
        for permission in PERMISSIONS {
            assert!(!permission.starts_with("s3:"), "{permission}");
            assert!(!permission.contains("stripe"), "{permission}");
        }
        assert!(PERMISSIONS.contains(&"sqs:SendMessage"));
        assert!(!PERMISSIONS.contains(&"sqs:ReceiveMessage"));
        assert!(!PERMISSIONS.contains(&"sqs:DeleteMessage"));
        assert!(!PERMISSIONS.contains(&"sqs:ChangeMessageVisibility"));
        assert!(PERMISSIONS.contains(&"lambda:InvokeFunction"));
        assert!(
            !PERMISSIONS
                .iter()
                .any(|permission| permission.starts_with("ses:"))
        );
        assert!(
            !PERMISSIONS
                .iter()
                .any(|permission| permission.starts_with("kms:"))
        );
    }

    #[tokio::test]
    async fn readiness_is_fail_closed_until_every_probe_answers() {
        let response = app(readiness(Probes::NONE))
            .oneshot(
                Request::builder()
                    .uri(READY_PATH)
                    .body(Body::empty())
                    .expect("a valid request"),
            )
            .await
            .expect("the router answers");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn a_worker_with_an_unavailable_projection_is_never_ready() {
        assert!(
            !readiness(Probes {
                aurora: true,
                wake: true,
                projection: false,
            })
            .is_ready()
        );
    }
}
