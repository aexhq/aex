//! Validated start-up configuration for `session-operation-worker`.

use aex_regional_http::config::{
    Lookup, RegionalHttpConfigError, bounded_u64, bounded_usize, forbidden, plane_name, queue_url,
    region, required,
};
use aex_wire::types::Region;

/// The deployable this configuration belongs to.
pub const DEPLOYABLE: &str = "session-operation-worker";

/// Deployment plane: `dev` or `prd`.
pub const PLANE: &str = "AEX_PLANE";
/// The region this process is pinned to.
pub const REGION: &str = "AEX_REGION";
/// The release digest reported in every record.
pub const RELEASE_DIGEST: &str = "AEX_RELEASE_DIGEST";
/// The operation hint queue.
pub const OPERATION_QUEUE_URL: &str = "AEX_OPERATION_QUEUE_URL";
/// The operation dead-letter queue.
pub const OPERATION_DLQ_URL: &str = "AEX_OPERATION_DLQ_URL";
/// The `regional-work` table.
pub const WORK_TABLE: &str = "AEX_WORK_TABLE";
/// The `session-authority` table.
pub const SESSION_TABLE: &str = "AEX_SESSION_TABLE";
/// Exact-generation runtime authority table.
pub const RUNTIME_ACTIVITY_TABLE: &str = "AEX_RUNTIME_ACTIVITY_TABLE";
/// Existing runtime-control-worker lifecycle queue.
pub const RUNTIME_LIFECYCLE_QUEUE_URL: &str = "AEX_RUNTIME_LIFECYCLE_QUEUE_URL";
/// The `observation-authority` table.
pub const OBSERVATION_TABLE: &str = "AEX_OBSERVATION_TABLE";
/// The deployed observation deletion-duty shard count.
pub const OBSERVATION_DUTY_SHARDS: &str = "AEX_OBS_DUTY_SHARDS";
/// How many deterministic shards the due scan sweeps.
pub const DUE_SCAN_SHARDS: &str = "AEX_DUE_SCAN_SHARDS";
/// Claim lease in milliseconds.
pub const LEASE_MS: &str = "AEX_LEASE_MS";
/// Per-step deadline in milliseconds.
pub const STEP_DEADLINE_MS: &str = "AEX_STEP_DEADLINE_MS";
/// Attempts before an operation terminalizes to manual review.
pub const MAX_ATTEMPTS: &str = "AEX_MAX_ATTEMPTS";

/// Every variable a healthy `session-operation-worker` requires.
pub const REQUIRED: [&str; 15] = [
    PLANE,
    REGION,
    RELEASE_DIGEST,
    OPERATION_QUEUE_URL,
    OPERATION_DLQ_URL,
    WORK_TABLE,
    SESSION_TABLE,
    RUNTIME_ACTIVITY_TABLE,
    RUNTIME_LIFECYCLE_QUEUE_URL,
    OBSERVATION_TABLE,
    OBSERVATION_DUTY_SHARDS,
    DUE_SCAN_SHARDS,
    LEASE_MS,
    STEP_DEADLINE_MS,
    MAX_ATTEMPTS,
];

/// Variables this binary must never be bound to.
///
/// Physical object deletion belongs to `content-lifecycle-worker`; a secret key
/// bound here would give the purge leg a decrypt capability it has no use for.
pub const FORBIDDEN: [(&str, &str); 2] = [
    (
        "AEX_SECRET_KMS_KEY_ARN",
        "the operation worker never decrypts a secret",
    ),
    (
        "AEX_CONTENT_QUEUE_URL",
        "physical object deletion belongs to content-lifecycle-worker",
    ),
];

/// Resolved configuration. Nothing here has a default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Deployment plane.
    pub plane: aex_identity_domain::assertion::Plane,
    /// Pinned region.
    pub region: Region,
    /// Release digest.
    pub release_digest: String,
    /// Operation hint queue.
    pub operation_queue_url: String,
    /// Operation dead-letter queue.
    pub operation_dlq_url: String,
    /// `regional-work` table.
    pub work_table: String,
    /// `session-authority` table.
    pub session_table: String,
    /// Exact-generation runtime authority table.
    pub runtime_activity_table: String,
    /// Existing runtime-control-worker lifecycle queue.
    pub runtime_lifecycle_queue_url: String,
    /// `observation-authority` table.
    pub observation_table: String,
    /// Deployed observation deletion-duty shard count.
    pub observation_duty_shards: u8,
    /// Deterministic due-scan shard count.
    pub due_scan_shards: u64,
    /// Claim lease in milliseconds.
    pub lease_ms: i64,
    /// Per-step deadline in milliseconds.
    pub step_deadline_ms: u64,
    /// Attempts before manual review.
    pub max_attempts: u32,
}

impl Config {
    /// Reads and validates the configuration from the process environment.
    ///
    /// # Errors
    ///
    /// Returns the first [`RegionalHttpConfigError`], naming the offending variable.
    pub fn from_env() -> Result<Self, RegionalHttpConfigError> {
        Self::read(&aex_regional_http::config::Environment)
    }

    /// Reads and validates the configuration from an arbitrary lookup.
    ///
    /// # Errors
    ///
    /// Identical to [`Config::from_env`].
    pub fn read<L: Lookup + ?Sized>(lookup: &L) -> Result<Self, RegionalHttpConfigError> {
        for (name, reason) in FORBIDDEN {
            forbidden(lookup, name, DEPLOYABLE, reason)?;
        }
        let plane = plane_name(lookup, PLANE)?;
        let region = region(lookup, REGION)?;
        let max_attempts = bounded_usize(lookup, MAX_ATTEMPTS, 1, 64)?;
        let strict_max_attempts =
            usize::try_from(crate::MAX_ATTEMPTS).map_err(|_| RegionalHttpConfigError::Invalid {
                name: MAX_ATTEMPTS,
                reason: "the strict-v1 poison boundary does not fit this target".to_owned(),
            })?;
        if max_attempts != strict_max_attempts {
            return Err(RegionalHttpConfigError::Invalid {
                name: MAX_ATTEMPTS,
                reason: format!(
                    "must equal the strict-v1 poison boundary ({})",
                    crate::MAX_ATTEMPTS
                ),
            });
        }
        let due_scan_shards = bounded_u64(lookup, DUE_SCAN_SHARDS, 1, 4_096)?;
        if due_scan_shards != aex_work_dynamodb::keys::DUE_SHARDS {
            return Err(RegionalHttpConfigError::Invalid {
                name: DUE_SCAN_SHARDS,
                reason: format!(
                    "must equal the regional-work contract ({})",
                    aex_work_dynamodb::keys::DUE_SHARDS
                ),
            });
        }
        let observation_duty_shards =
            u8::try_from(bounded_u64(lookup, OBSERVATION_DUTY_SHARDS, 1, 64)?).map_err(|_| {
                RegionalHttpConfigError::Invalid {
                    name: OBSERVATION_DUTY_SHARDS,
                    reason: "the shard count does not fit the observation due index".to_owned(),
                }
            })?;
        Ok(Self {
            plane,
            region,
            release_digest: required(lookup, RELEASE_DIGEST)?,
            // The queue's own region is checked here rather than on the first
            // event: a cross-region queue is a deployment mistake and must not
            // wait for traffic to surface.
            operation_queue_url: queue_url(lookup, OPERATION_QUEUE_URL, region)?,
            operation_dlq_url: queue_url(lookup, OPERATION_DLQ_URL, region)?,
            work_table: required(lookup, WORK_TABLE)?,
            session_table: required(lookup, SESSION_TABLE)?,
            runtime_activity_table: required(lookup, RUNTIME_ACTIVITY_TABLE)?,
            runtime_lifecycle_queue_url: queue_url(lookup, RUNTIME_LIFECYCLE_QUEUE_URL, region)?,
            observation_table: required(lookup, OBSERVATION_TABLE)?,
            observation_duty_shards,
            due_scan_shards,
            lease_ms: i64::try_from(bounded_u64(lookup, LEASE_MS, 1_000, 900_000)?).map_err(
                |_| RegionalHttpConfigError::Invalid {
                    name: LEASE_MS,
                    reason: "lease does not fit a signed millisecond clock".to_owned(),
                },
            )?,
            step_deadline_ms: bounded_u64(lookup, STEP_DEADLINE_MS, 1_000, 900_000)?,
            max_attempts: u32::try_from(max_attempts).map_err(|_| {
                RegionalHttpConfigError::Invalid {
                    name: MAX_ATTEMPTS,
                    reason: "attempt ceiling does not fit a 32-bit counter".to_owned(),
                }
            })?,
        })
    }
}
