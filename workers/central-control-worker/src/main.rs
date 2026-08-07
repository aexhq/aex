//! `central-control-worker` composition root (Rust Lambda ZIP).
//!
//! Queue- and schedule-driven reconciliation for the central control plane. It
//! is the only role that reclaims expired rows, the only one that administers
//! assertion signing keys, and the only one that sends mail.
//!
//! Two properties are structural rather than careful:
//!
//! * every duty is named in [`Handler::ALL`] and [`Handler::for_topic`] is total
//!   over [`Topic`], so an outbox topic nobody wrote a duty for is a compile
//!   error rather than a message that is quietly deleted;
//! * a partial-batch response names only the items that did **not** commit, so
//!   a failure inside one batch never re-runs an item that already landed.

use std::collections::{BTreeMap, BTreeSet};

use aex_central_http::capability::{
    Capability as _, CapabilityBinding, CompositionError, CompositionManifest, ControlQueueConsume,
    ControlWrite, Declares, MailSend, RegionalControlInvoke, SigningKeyAdminister,
};
use aex_central_http::config::{CentralServiceId, DeploymentPlane};
use aex_central_http::health::{Dependency, Readiness};
use aex_control_domain::Topic;
use aex_wire::types::Region;

mod runtime;

/// The deployable this binary is.
const DEPLOYABLE: CentralServiceId = CentralServiceId::ControlWorker;

/// The one login role this binary may connect as.
const REQUIRED_ROLE: &str = "aex_control_worker";

/// Environment keys, all inside the declared namespace.
mod keys {
    /// The plane's account id, for the ARN binding check.
    pub const ACCOUNT_ID: &str = "AEX_CENTRAL_CONTROL_WORKER_ACCOUNT_ID";
    /// The Aurora cluster.
    pub const AURORA_CLUSTER_ARN: &str = "AEX_CENTRAL_CONTROL_WORKER_AURORA_CLUSTER_ARN";
    /// The Aurora credentials secret.
    pub const AURORA_SECRET_ARN: &str = "AEX_CENTRAL_CONTROL_WORKER_AURORA_SECRET_ARN";
    /// How many messages one batch claims.
    pub const BATCH_SIZE: &str = "AEX_CENTRAL_CONTROL_WORKER_BATCH_SIZE";
    /// The control FIFO queue.
    pub const CONTROL_QUEUE_ARN: &str = "AEX_CENTRAL_CONTROL_WORKER_CONTROL_QUEUE_ARN";
    /// Queue URL used only for the startup reachability probe.
    pub const CONTROL_QUEUE_URL: &str = "AEX_CENTRAL_CONTROL_WORKER_CONTROL_QUEUE_URL";
    /// The database name.
    pub const DATABASE: &str = "AEX_CENTRAL_CONTROL_WORKER_DATABASE";
    /// How long a claim lease lasts.
    pub const LEASE_MS: &str = "AEX_CENTRAL_CONTROL_WORKER_LEASE_MS";
    /// The deployment plane.
    pub const PLANE: &str = "AEX_CENTRAL_CONTROL_WORKER_PLANE";
    /// The bound region.
    pub const REGION: &str = "AEX_CENTRAL_CONTROL_WORKER_REGION";
    /// Direct regional control Lambda ARNs, `region=arn` comma-separated.
    pub const REGIONAL_FUNCTIONS: &str = "AEX_CENTRAL_CONTROL_WORKER_REGIONAL_FUNCTION_ARNS";
    /// Regional authz projection tables, `region=table` comma-separated.
    pub const REGIONAL_PROJECTIONS: &str = "AEX_CENTRAL_CONTROL_WORKER_REGIONAL_PROJECTION_TABLES";
    /// The login role. Must be `aex_control_worker`.
    pub const ROLE: &str = "AEX_CENTRAL_CONTROL_WORKER_ROLE";
    /// The sender identity bound to the mail capability.
    pub const SES_IDENTITY_ARN: &str = "AEX_CENTRAL_CONTROL_WORKER_SES_IDENTITY_ARN";
    /// The verified RFC 5322 sender address.
    pub const MAIL_FROM: &str = "AEX_CENTRAL_CONTROL_WORKER_MAIL_FROM";
    /// The signing-key secret prefix this worker rotates.
    pub const SIGNING_SECRET_PREFIX: &str = "AEX_CENTRAL_CONTROL_WORKER_SIGNING_SECRET_PREFIX";

    /// Every key this binary reads, for the totality test.
    #[allow(
        dead_code,
        reason = "the inventory exists so the suite can remove each key in turn"
    )]
    pub const ALL: &[&str] = &[
        ACCOUNT_ID,
        AURORA_CLUSTER_ARN,
        AURORA_SECRET_ARN,
        BATCH_SIZE,
        CONTROL_QUEUE_ARN,
        CONTROL_QUEUE_URL,
        DATABASE,
        LEASE_MS,
        PLANE,
        REGION,
        REGIONAL_FUNCTIONS,
        REGIONAL_PROJECTIONS,
        ROLE,
        SES_IDENTITY_ARN,
        MAIL_FROM,
        SIGNING_SECRET_PREFIX,
    ];
}

/// The largest batch one invocation may claim.
const MAX_BATCH: u64 = 100;
/// The longest a claim lease may last.
const MAX_LEASE_MS: u64 = 900_000;

/// Why `central-control-worker` refused to start.
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
    /// The control FIFO queue.
    pub control_queue_arn: String,
    /// The control queue URL for startup probing.
    pub control_queue_url: String,
    /// The sender identity bound to the mail capability.
    pub ses_identity_arn: String,
    /// Verified sender address.
    pub mail_from: String,
    /// The signing-key secret prefix.
    pub signing_secret_prefix: String,
    /// The database name.
    pub database: String,
    /// The login role.
    pub role: String,
    /// How many messages one batch claims.
    pub batch_size: u32,
    /// How long a claim lease lasts.
    pub lease_ms: u64,
    /// Every region this worker may dispatch to.
    pub regional_functions: BTreeMap<Region, String>,
    /// Regional authorization projection tables.
    pub regional_projections: BTreeMap<Region, String>,
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
        let role = required(&lookup, keys::ROLE)?;
        if role != REQUIRED_ROLE {
            return Err(CentralControlWorkerConfigError::Invalid {
                name: keys::ROLE,
                reason: format!("this binary connects only as `{REQUIRED_ROLE}`, got `{role}`"),
            });
        }
        let batch_size = bounded(&lookup, keys::BATCH_SIZE, 1, MAX_BATCH)?;
        let lease_ms = bounded(&lookup, keys::LEASE_MS, 1, MAX_LEASE_MS)?;
        let regional_functions = functions(&required(&lookup, keys::REGIONAL_FUNCTIONS)?)?;
        let regional_projections = projections(&required(&lookup, keys::REGIONAL_PROJECTIONS)?)?;
        Ok(Self {
            plane,
            region,
            account_id: required(&lookup, keys::ACCOUNT_ID)?,
            aurora_cluster_arn: required(&lookup, keys::AURORA_CLUSTER_ARN)?,
            aurora_secret_arn: required(&lookup, keys::AURORA_SECRET_ARN)?,
            control_queue_arn: required(&lookup, keys::CONTROL_QUEUE_ARN)?,
            control_queue_url: required(&lookup, keys::CONTROL_QUEUE_URL)?,
            ses_identity_arn: required(&lookup, keys::SES_IDENTITY_ARN)?,
            mail_from: mail_from(&required(&lookup, keys::MAIL_FROM)?)?,
            signing_secret_prefix: required(&lookup, keys::SIGNING_SECRET_PREFIX)?,
            database: required(&lookup, keys::DATABASE)?,
            role,
            batch_size: u32::try_from(batch_size).unwrap_or(1),
            lease_ms,
            regional_functions,
            regional_projections,
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
                    keys::CONTROL_QUEUE_ARN.to_owned(),
                    self.control_queue_arn.clone(),
                ),
                (
                    keys::SES_IDENTITY_ARN.to_owned(),
                    self.ses_identity_arn.clone(),
                ),
                (
                    keys::SIGNING_SECRET_PREFIX.to_owned(),
                    self.signing_secret_prefix.clone(),
                ),
                (
                    keys::REGIONAL_FUNCTIONS.to_owned(),
                    self.regional_functions
                        .iter()
                        .map(|(region, function)| format!("{}={function}", region.as_str()))
                        .collect::<Vec<_>>()
                        .join(","),
                ),
                (
                    keys::REGIONAL_PROJECTIONS.to_owned(),
                    self.regional_projections
                        .iter()
                        .map(|(region, table)| format!("{}={table}", region.as_str()))
                        .collect::<Vec<_>>()
                        .join(","),
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

/// Parses the `region=lambda-arn` direct-invoke map.
///
/// Every launch region must be present. A worker that can dispatch to four of
/// five regions is one that silently strands every workspace in the fifth.
fn functions(raw: &str) -> Result<BTreeMap<Region, String>, CentralControlWorkerConfigError> {
    let mut map = BTreeMap::new();
    for entry in raw.split(',').filter(|it| !it.trim().is_empty()) {
        let (region, function) =
            entry
                .split_once('=')
                .ok_or_else(|| CentralControlWorkerConfigError::Invalid {
                    name: keys::REGIONAL_FUNCTIONS,
                    reason: "expected `region=lambda-arn` entries".to_owned(),
                })?;
        let parsed = Region::from_name(region.trim()).ok_or_else(|| {
            CentralControlWorkerConfigError::Invalid {
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
            return Err(CentralControlWorkerConfigError::Invalid {
                name: keys::REGIONAL_FUNCTIONS,
                reason: format!(
                    "`{}` is not a unique Lambda ARN in its region",
                    parsed.as_str()
                ),
            });
        }
    }
    if let Some(missing) = Region::ALL.iter().find(|region| !map.contains_key(region)) {
        return Err(CentralControlWorkerConfigError::Invalid {
            name: keys::REGIONAL_FUNCTIONS,
            reason: format!("no function for `{}`", missing.as_str()),
        });
    }
    Ok(map)
}

fn projections(raw: &str) -> Result<BTreeMap<Region, String>, CentralControlWorkerConfigError> {
    let mut map = BTreeMap::new();
    for entry in raw.split(',').filter(|entry| !entry.trim().is_empty()) {
        let (region, table) =
            entry
                .split_once('=')
                .ok_or_else(|| CentralControlWorkerConfigError::Invalid {
                    name: keys::REGIONAL_PROJECTIONS,
                    reason: "expected `region=table` entries".to_owned(),
                })?;
        let region = Region::from_name(region.trim()).ok_or_else(|| {
            CentralControlWorkerConfigError::Invalid {
                name: keys::REGIONAL_PROJECTIONS,
                reason: format!("`{region}` is not a launch region"),
            }
        })?;
        if table.trim().is_empty() || map.insert(region, table.trim().to_owned()).is_some() {
            return Err(CentralControlWorkerConfigError::Invalid {
                name: keys::REGIONAL_PROJECTIONS,
                reason: format!("`{}` is empty or repeated", region.as_str()),
            });
        }
    }
    if let Some(missing) = Region::ALL.iter().find(|region| !map.contains_key(region)) {
        return Err(CentralControlWorkerConfigError::Invalid {
            name: keys::REGIONAL_PROJECTIONS,
            reason: format!("no projection table for `{}`", missing.as_str()),
        });
    }
    Ok(map)
}

fn mail_from(raw: &str) -> Result<String, CentralControlWorkerConfigError> {
    if raw.contains('@') && !raw.contains(char::is_whitespace) {
        Ok(raw.to_owned())
    } else {
        Err(CentralControlWorkerConfigError::Invalid {
            name: keys::MAIL_FROM,
            reason: "expected one verified sender address".to_owned(),
        })
    }
}

/// Every scheduled or queue-driven duty this worker performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Handler {
    /// Finish a workspace whose regional half may already exist.
    WorkspaceProvisionReconcile,
    /// Ask a region to remove a workspace's regional half.
    WorkspaceDeleteDispatch,
    /// Project a finance account state and epoch to one workspace.
    AccountStateProject,
    /// Send one invitation notification.
    InvitationEmailDeliver,
    /// Project a newly minted key's authorization row to its region.
    ApiKeyAuthorizationProject,
    /// Project an advanced revocation epoch to every region.
    AuthorizationEpochProject,
    /// Publish a rotated assertion signing key.
    AuthorizationSigningKeyRotate,
    /// Claim operations whose lease lapsed or which never ran.
    OperationDueScan,
    /// Sweep expired replay records.
    IdempotencyGc,
    /// Sweep dispatched outbox rows.
    OutboxGc,
    /// Check whether a pepper may retire.
    PepperRetireCheck,
}

impl Handler {
    /// Every duty.
    pub const ALL: [Self; 11] = [
        Self::WorkspaceProvisionReconcile,
        Self::WorkspaceDeleteDispatch,
        Self::AccountStateProject,
        Self::InvitationEmailDeliver,
        Self::ApiKeyAuthorizationProject,
        Self::AuthorizationEpochProject,
        Self::AuthorizationSigningKeyRotate,
        Self::OperationDueScan,
        Self::IdempotencyGc,
        Self::OutboxGc,
        Self::PepperRetireCheck,
    ];

    /// The stable duty name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WorkspaceProvisionReconcile => "workspace.provision.reconcile",
            Self::WorkspaceDeleteDispatch => "workspace.delete.dispatch",
            Self::AccountStateProject => "account.state.project",
            Self::InvitationEmailDeliver => "invitation.email.deliver",
            Self::ApiKeyAuthorizationProject => "api_key.authorization.project",
            Self::AuthorizationEpochProject => "authorization.epoch.project",
            Self::AuthorizationSigningKeyRotate => "authorization.signing_key.rotate",
            Self::OperationDueScan => "operation.due.scan",
            Self::IdempotencyGc => "idempotency.gc",
            Self::OutboxGc => "outbox.gc",
            Self::PepperRetireCheck => "pepper.retire.check",
        }
    }

    /// The duty that consumes `topic`.
    ///
    /// Total over [`Topic`]: adding a topic without a duty is a compile error
    /// here, which is the point.
    #[must_use]
    pub const fn for_topic(topic: Topic) -> Self {
        match topic {
            Topic::WorkspaceProvisionRequested => Self::WorkspaceProvisionReconcile,
            Topic::WorkspaceDeleteRequested => Self::WorkspaceDeleteDispatch,
            Topic::AccountStateChanged => Self::AccountStateProject,
            Topic::InvitationEmailRequested => Self::InvitationEmailDeliver,
            // Creation and revocation publish the same row, but they stay
            // separate duties: one duty per topic is what makes a failing
            // publication attributable to the event that caused it.
            Topic::ApiKeyCreated => Self::ApiKeyAuthorizationProject,
            Topic::AuthorizationEpochChanged => Self::AuthorizationEpochProject,
            Topic::AuthorizationSigningKeyPublished => Self::AuthorizationSigningKeyRotate,
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
impl Declares<ControlQueueConsume> for Composition {}
impl Declares<RegionalControlInvoke> for Composition {}
impl Declares<MailSend> for Composition {}
impl Declares<SigningKeyAdminister> for Composition {}

/// The manifest the start-up check runs against.
#[must_use]
pub fn manifest() -> CompositionManifest {
    CompositionManifest {
        deployable: DEPLOYABLE,
        capabilities: BTreeSet::from([
            ControlWrite::ID,
            ControlQueueConsume::ID,
            RegionalControlInvoke::ID,
            MailSend::ID,
            SigningKeyAdminister::ID,
        ]),
        bindings: vec![
            CapabilityBinding::arn(keys::AURORA_CLUSTER_ARN, ControlWrite::ID),
            CapabilityBinding::arn(keys::CONTROL_QUEUE_ARN, ControlQueueConsume::ID),
            CapabilityBinding::arn(keys::SES_IDENTITY_ARN, MailSend::ID),
            CapabilityBinding::resource(keys::SIGNING_SECRET_PREFIX, SigningKeyAdminister::ID),
            CapabilityBinding::resource(keys::REGIONAL_FUNCTIONS, RegionalControlInvoke::ID),
            CapabilityBinding::resource(keys::REGIONAL_PROJECTIONS, ControlWrite::ID),
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
    "sqs:ChangeMessageVisibility",
    "sqs:DeleteMessage",
    "sqs:ReceiveMessage",
    "sqs:GetQueueAttributes",
    "secretsmanager:CreateSecret",
    "secretsmanager:DeleteSecret",
    "secretsmanager:GetSecretValue",
    "secretsmanager:PutSecretValue",
    "kms:Decrypt",
    "kms:GenerateDataKey",
    "ses:SendEmail",
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
    /// The control queue is reachable.
    pub queue: bool,
    /// Every direct regional function is configured.
    pub functions: bool,
    /// Every regional authorization projection answered.
    pub projections: bool,
    /// The signing secret prefix is listable.
    pub signing: bool,
}

impl Probes {
    /// No probe has answered yet.
    pub const NONE: Self = Self {
        aurora: false,
        queue: false,
        functions: false,
        projections: false,
        signing: false,
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
                name: "control-queue",
                resolved: probes.queue,
            },
            Dependency {
                name: "regional-function-map",
                resolved: probes.functions,
            },
            Dependency {
                name: "regional-authz-projections",
                resolved: probes.projections,
            },
            Dependency {
                name: "signing-secret-prefix",
                resolved: probes.signing,
            },
        ],
    )
}

/// One batch item's outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemOutcome {
    /// The queue message identifier.
    pub message_id: String,
    /// Whether the item committed.
    pub committed: bool,
}

/// The partial-batch response the queue expects.
///
/// Only uncommitted items are named. Reporting a committed item would re-run
/// work that already landed, which for a fenced regional effect means a second
/// dispatch under a stale fence.
#[must_use]
pub fn partial_batch_failures(outcomes: &[ItemOutcome]) -> Vec<String> {
    outcomes
        .iter()
        .filter(|outcome| !outcome.committed)
        .map(|outcome| outcome.message_id.clone())
        .collect()
}

/// Why `central-control-worker` stopped.
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
/// A queue worker has no public surface; the two internal probes are the whole
/// mounted set, and they are what the deployment health check polls.
pub fn app(readiness: Readiness) -> axum::Router {
    aex_central_http::health::router(readiness)
}

/// Runs `central-control-worker` until it stops.
///
/// # Errors
///
/// Returns [`CentralControlWorkerRunError`] when configuration or composition is refused, or when
/// the runtime stops.
pub async fn run(
    config: &Config,
    telemetry: &aex_platform_telemetry::Handle,
) -> Result<(), CentralControlWorkerRunError> {
    aex_central_http::capability::admit(&manifest(), &config.resolved())?;
    telemetry.emit(
        aex_platform_telemetry::Record::event(
            aex_telemetry_schema::generated::EVENT_AEX_PROCESS_STARTED,
        )
        .with(
            aex_telemetry_schema::generated::AEX_PLANE,
            config.plane.as_str().to_owned(),
        )
        .with(
            aex_telemetry_schema::generated::AEX_REGION,
            config.region.as_str().to_owned(),
        ),
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

    aws_sdk_sqs::Client::new(&aws)
        .get_queue_attributes()
        .queue_url(&config.control_queue_url)
        .attribute_names(aws_sdk_sqs::types::QueueAttributeName::QueueArn)
        .send()
        .await
        .map_err(|error| {
            CentralControlWorkerRunError::Dependency("control-queue", error.to_string())
        })?;
    let ses = aws_sdk_sesv2::Client::new(&aws);

    let signing = std::sync::Arc::new(runtime::SecretsSigningAdmin::new(
        aws_sdk_secretsmanager::Client::new(&aws),
        config.signing_secret_prefix.clone(),
    ));
    signing.probe().await.map_err(|error| {
        CentralControlWorkerRunError::Dependency("signing-secret-prefix", error)
    })?;

    let mut projections = BTreeMap::new();
    for (region, table) in &config.regional_projections {
        let regional_aws = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(region.as_str()))
            .load()
            .await;
        let writer = aex_session_dynamodb::projection_write::ProjectionWriter::new(
            aws_sdk_dynamodb::Client::new(&regional_aws),
            table.clone(),
        );
        writer.probe().await.map_err(|error| {
            CentralControlWorkerRunError::Dependency("regional-authz-projection", error)
        })?;
        projections.insert(*region, writer);
    }
    let concrete_store = std::sync::Arc::new(aex_control_aurora::AuroraControlStore::new(data));
    let store: std::sync::Arc<dyn runtime::Store> = concrete_store;
    let regional: std::sync::Arc<dyn aex_control_app::ports::RegionalControlPort> =
        std::sync::Arc::new(aex_central_aws::LambdaRegionalControl::new(
            aws_sdk_lambda::Client::new(&aws),
            config.regional_functions.clone(),
        ));
    let worker = std::sync::Arc::new(runtime::Worker::new(
        store,
        regional,
        projections,
        std::sync::Arc::new(runtime::SesMail::new(ses, config.mail_from.clone())),
        signing,
        std::sync::Arc::new(aex_central_aws::SystemClock),
        format!("{}:{}", DEPLOYABLE.as_str(), config.region.as_str()),
        config.batch_size,
        time::Duration::milliseconds(i64::try_from(config.lease_ms).unwrap_or(i64::MAX)),
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

async fn handle_event(
    worker: &runtime::Worker,
    value: serde_json::Value,
) -> Result<serde_json::Value, String> {
    use aws_lambda_events::event::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};

    if value.get("Records").is_some() {
        let Ok(event) = serde_json::from_value::<SqsEvent>(value) else {
            return Ok(serde_json::json!({
                "batchItemFailures": [{ "itemIdentifier": "malformed-sqs-event" }]
            }));
        };
        let mut response = SqsBatchResponse::default();
        for record in event.records {
            if worker.tick().await.is_err() {
                let mut failure = BatchItemFailure::default();
                failure.item_identifier = record
                    .message_id
                    .unwrap_or_else(|| "missing-message-id".to_owned());
                response.batch_item_failures.push(failure);
            }
        }
        return Ok(serde_json::to_value(response).unwrap_or_else(|_| {
            serde_json::json!({
                "batchItemFailures": [{ "itemIdentifier": "response-encode-failed" }]
            })
        }));
    }
    worker.scheduled().await?;
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

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("central-control-worker: refusing to start: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let settings = aex_platform_telemetry::Settings::default();
    let telemetry = aex_platform_telemetry::Handle::install(&settings, None);
    let outcome = run(&config, &telemetry).await;
    if let aex_platform_telemetry::FlushOutcome::DeadlineExceeded { pending } =
        telemetry.flush(settings.flush_deadline)
    {
        eprintln!("central-control-worker: telemetry flush left {pending} record(s) undelivered");
    }
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("central-control-worker: stopped: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CentralControlWorkerConfigError, Config, Handler, ItemOutcome, PERMISSIONS, Probes, app,
        keys, manifest, partial_batch_failures, readiness,
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

    fn every_projection() -> String {
        Region::ALL
            .iter()
            .map(|region| format!("{}=aex-prd-authz-projection", region.as_str()))
            .collect::<Vec<_>>()
            .join(",")
    }

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
                keys::CONTROL_QUEUE_ARN,
                "arn:aws:sqs:eu-west-1:000000000000:aex-control.fifo".to_owned(),
            ),
            (
                keys::CONTROL_QUEUE_URL,
                "https://sqs.eu-west-1.amazonaws.com/000000000000/aex-control.fifo".to_owned(),
            ),
            (
                keys::SES_IDENTITY_ARN,
                "arn:aws:ses:eu-west-1:000000000000:identity/aex.dev".to_owned(),
            ),
            (keys::MAIL_FROM, "no-reply@aex.dev".to_owned()),
            (
                keys::SIGNING_SECRET_PREFIX,
                "aex/prd/authz-signing/".to_owned(),
            ),
            (keys::DATABASE, "aex".to_owned()),
            (keys::ROLE, "aex_control_worker".to_owned()),
            (keys::BATCH_SIZE, "10".to_owned()),
            (keys::LEASE_MS, "60000".to_owned()),
            (keys::REGIONAL_FUNCTIONS, every_function()),
            (keys::REGIONAL_PROJECTIONS, every_projection()),
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
        assert_eq!(config.regional_functions.len(), Region::ALL.len());
        assert_eq!(config.regional_projections.len(), Region::ALL.len());
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
    fn a_function_map_missing_a_region_is_refused() {
        let mut vars = complete();
        vars.insert(
            keys::REGIONAL_FUNCTIONS,
            "eu-west-1=arn:aws:lambda:eu-west-1:000000000000:function:aex-regional-control"
                .to_owned(),
        );
        let error = read(&vars).expect_err("an incomplete map strands a region");
        assert!(
            matches!(error, CentralControlWorkerConfigError::Invalid { name, .. } if name == keys::REGIONAL_FUNCTIONS)
        );
    }

    #[test]
    fn a_repeated_or_unknown_region_is_refused() {
        for raw in [
            "mars-central-1=arn:aws:lambda:mars-central-1:0:function:x",
            "eu-west-1=arn:aws:lambda:eu-west-1:0:function:a,eu-west-1=arn:aws:lambda:eu-west-1:0:function:b",
            "eu-west-1",
        ] {
            let mut vars = complete();
            vars.insert(keys::REGIONAL_FUNCTIONS, raw.to_owned());
            assert!(read(&vars).is_err(), "{raw}");
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
    fn this_binary_refuses_any_role_but_the_worker_one() {
        let mut vars = complete();
        vars.insert(keys::ROLE, "aex_control_api".to_owned());
        assert!(read(&vars).is_err());
    }

    #[test]
    fn every_outbox_topic_has_a_duty_and_every_duty_has_a_name() {
        let handled: BTreeSet<Handler> = Topic::ALL.into_iter().map(Handler::for_topic).collect();
        assert_eq!(handled.len(), Topic::ALL.len(), "two topics share one duty");
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
        assert!(PERMISSIONS.contains(&"sqs:ReceiveMessage"));
        assert!(PERMISSIONS.contains(&"ses:SendEmail"));
    }

    #[test]
    fn a_partial_batch_names_only_the_items_that_did_not_commit() {
        let outcomes = vec![
            ItemOutcome {
                message_id: "a".to_owned(),
                committed: true,
            },
            ItemOutcome {
                message_id: "b".to_owned(),
                committed: false,
            },
            ItemOutcome {
                message_id: "c".to_owned(),
                committed: true,
            },
        ];
        assert_eq!(partial_batch_failures(&outcomes), vec!["b".to_owned()]);
        assert!(partial_batch_failures(&[]).is_empty());
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
    fn a_worker_with_an_incomplete_projection_map_is_never_ready() {
        assert!(
            !readiness(Probes {
                aurora: true,
                queue: true,
                functions: true,
                projections: false,
                signing: true,
            })
            .is_ready()
        );
    }
}
