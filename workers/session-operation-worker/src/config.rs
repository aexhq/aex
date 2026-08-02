//! Validated start-up configuration for `session-operation-worker`.

use aex_regional_http::config::{
    ConfigError, Lookup, bounded_u64, bounded_usize, forbidden, plane_name, queue_url, region,
    required,
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
/// The `regional-content` table.
pub const CONTENT_TABLE: &str = "AEX_CONTENT_TABLE";
/// The `regional-registry` table.
pub const REGISTRY_TABLE: &str = "AEX_REGISTRY_TABLE";
/// The regional content bucket.
pub const CONTENT_BUCKET: &str = "AEX_CONTENT_BUCKET";
/// The account that must own the content bucket.
pub const CONTENT_BUCKET_OWNER: &str = "AEX_CONTENT_BUCKET_OWNER";
/// The deletion-denial projection table.
pub const DENIAL_PROJECTION_TABLE: &str = "AEX_DENIAL_PROJECTION_TABLE";
/// How many deterministic shards the due scan sweeps.
pub const DUE_SCAN_SHARDS: &str = "AEX_DUE_SCAN_SHARDS";
/// Claim lease in milliseconds.
pub const LEASE_MS: &str = "AEX_LEASE_MS";
/// Per-step deadline in milliseconds.
pub const STEP_DEADLINE_MS: &str = "AEX_STEP_DEADLINE_MS";
/// Attempts before an operation terminalizes to manual review.
pub const MAX_ATTEMPTS: &str = "AEX_MAX_ATTEMPTS";

/// Every variable a healthy `session-operation-worker` requires.
pub const REQUIRED: [&str; 16] = [
    PLANE,
    REGION,
    RELEASE_DIGEST,
    OPERATION_QUEUE_URL,
    OPERATION_DLQ_URL,
    WORK_TABLE,
    SESSION_TABLE,
    CONTENT_TABLE,
    REGISTRY_TABLE,
    CONTENT_BUCKET,
    CONTENT_BUCKET_OWNER,
    DENIAL_PROJECTION_TABLE,
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
    /// `regional-content` table.
    pub content_table: String,
    /// `regional-registry` table.
    pub registry_table: String,
    /// Regional content bucket.
    pub content_bucket: String,
    /// Expected content-bucket owner account.
    pub content_bucket_owner: String,
    /// Deletion-denial projection table.
    pub denial_projection_table: String,
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
    /// Returns the first [`ConfigError`], naming the offending variable.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::read(&aex_regional_http::config::Environment)
    }

    /// Reads and validates the configuration from an arbitrary lookup.
    ///
    /// # Errors
    ///
    /// Identical to [`Config::from_env`].
    pub fn read<L: Lookup + ?Sized>(lookup: &L) -> Result<Self, ConfigError> {
        for (name, reason) in FORBIDDEN {
            forbidden(lookup, name, DEPLOYABLE, reason)?;
        }
        let plane = plane_name(lookup, PLANE)?;
        let region = region(lookup, REGION)?;
        let max_attempts = bounded_usize(lookup, MAX_ATTEMPTS, 1, 64)?;
        let strict_max_attempts =
            usize::try_from(crate::MAX_ATTEMPTS).map_err(|_| ConfigError::Invalid {
                name: MAX_ATTEMPTS,
                reason: "the strict-v1 poison boundary does not fit this target".to_owned(),
            })?;
        if max_attempts != strict_max_attempts {
            return Err(ConfigError::Invalid {
                name: MAX_ATTEMPTS,
                reason: format!(
                    "must equal the strict-v1 poison boundary ({})",
                    crate::MAX_ATTEMPTS
                ),
            });
        }
        let due_scan_shards = bounded_u64(lookup, DUE_SCAN_SHARDS, 1, 4_096)?;
        if due_scan_shards != aex_work_dynamodb::keys::DUE_SHARDS {
            return Err(ConfigError::Invalid {
                name: DUE_SCAN_SHARDS,
                reason: format!(
                    "must equal the regional-work contract ({})",
                    aex_work_dynamodb::keys::DUE_SHARDS
                ),
            });
        }
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
            content_table: required(lookup, CONTENT_TABLE)?,
            registry_table: required(lookup, REGISTRY_TABLE)?,
            content_bucket: required(lookup, CONTENT_BUCKET)?,
            content_bucket_owner: required(lookup, CONTENT_BUCKET_OWNER)?,
            denial_projection_table: required(lookup, DENIAL_PROJECTION_TABLE)?,
            due_scan_shards,
            lease_ms: i64::try_from(bounded_u64(lookup, LEASE_MS, 1_000, 900_000)?).map_err(
                |_| ConfigError::Invalid {
                    name: LEASE_MS,
                    reason: "lease does not fit a signed millisecond clock".to_owned(),
                },
            )?,
            step_deadline_ms: bounded_u64(lookup, STEP_DEADLINE_MS, 1_000, 900_000)?,
            max_attempts: u32::try_from(max_attempts).map_err(|_| ConfigError::Invalid {
                name: MAX_ATTEMPTS,
                reason: "attempt ceiling does not fit a 32-bit counter".to_owned(),
            })?,
        })
    }
}
