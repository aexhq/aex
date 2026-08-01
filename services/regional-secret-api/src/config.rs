//! Validated start-up configuration for `regional-secret-api`.
//!
//! This deployable is the only one that ever holds secret plaintext, so its
//! configuration is deliberately narrow: it binds the two secret tables and the
//! secret KMS key and **refuses to start** when a session, content, work, queue
//! or registry variable is bound to it. That is the configuration half of
//! capability admission (RS-09) — IAM is the other half, and neither is a single
//! point of failure.

use aex_regional_http::config::{
    Arn, ConfigError, Lookup, arn_in_region, bounded_u64, bounded_usize, forbidden, plane_name,
    region, required,
};
use aex_wire::types::Region;

/// The deployable this configuration belongs to.
pub const DEPLOYABLE: &str = "regional-secret-api";

/// Deployment plane: `dev` or `prd`.
pub const PLANE: &str = "AEX_PLANE";
/// The region this process is pinned to.
pub const REGION: &str = "AEX_REGION";
/// The release digest reported by `/internal/readyz`.
pub const RELEASE_DIGEST: &str = "AEX_RELEASE_DIGEST";
/// The `central-authz` function this edge resolves assertions through.
pub const AUTHZ_FUNCTION_ARN: &str = "AEX_AUTHZ_FUNCTION_ARN";
/// The parameter holding the assertion verification key set.
pub const AUTHZ_VERIFY_KEYS_PARAM: &str = "AEX_AUTHZ_VERIFY_KEYS_PARAM";
/// The regional authorization projection table.
pub const AUTHZ_PROJECTION_TABLE: &str = "AEX_AUTHZ_PROJECTION_TABLE";
/// The `regional-secret-custody` table.
pub const SECRET_CUSTODY_TABLE: &str = "AEX_SECRET_CUSTODY_TABLE";
/// The `regional-secret-keystore` table.
pub const SECRET_KEYSTORE_TABLE: &str = "AEX_SECRET_KEYSTORE_TABLE";
/// The secret KMS key. Never the content key.
pub const SECRET_KMS_KEY_ARN: &str = "AEX_SECRET_KMS_KEY_ARN";
/// Branch-key cache byte budget.
pub const BRANCH_KEY_CACHE_BYTES: &str = "AEX_SECRET_BRANCH_KEY_CACHE_BYTES";
/// Branch-key cache lifetime in milliseconds.
pub const BRANCH_KEY_CACHE_TTL_MS: &str = "AEX_SECRET_BRANCH_KEY_CACHE_TTL_MS";
/// Assertion cache byte budget.
pub const ASSERTION_CACHE_BYTES: &str = "AEX_ASSERTION_CACHE_BYTES";
/// The effective encoded JSON body bound.
pub const MAX_JSON_BODY_BYTES: &str = "AEX_MAX_JSON_BODY_BYTES";

/// Every variable a healthy `regional-secret-api` requires, in declaration order.
pub const REQUIRED: [&str; 13] = [
    PLANE,
    REGION,
    RELEASE_DIGEST,
    AUTHZ_FUNCTION_ARN,
    AUTHZ_VERIFY_KEYS_PARAM,
    AUTHZ_PROJECTION_TABLE,
    SECRET_CUSTODY_TABLE,
    SECRET_KEYSTORE_TABLE,
    SECRET_KMS_KEY_ARN,
    BRANCH_KEY_CACHE_BYTES,
    BRANCH_KEY_CACHE_TTL_MS,
    ASSERTION_CACHE_BYTES,
    MAX_JSON_BODY_BYTES,
];

/// Variables this binary must never be bound to, with the reason.
///
/// A secret edge that can also reach the session authority, the content bucket
/// or a queue is one bug away from writing plaintext somewhere it is not
/// encrypted, so the binding is refused at start-up rather than reviewed.
pub const FORBIDDEN: [(&str, &str); 6] = [
    (
        "AEX_SESSION_TABLE",
        "the secret edge holds no session binding",
    ),
    (
        "AEX_CONTENT_TABLE",
        "the secret edge holds no content binding",
    ),
    (
        "AEX_CONTENT_BUCKET",
        "the secret edge holds no object store",
    ),
    ("AEX_WORK_TABLE", "the secret edge admits no durable work"),
    (
        "AEX_REGISTRY_TABLE",
        "the secret edge holds no registry binding",
    ),
    (
        "AEX_OPERATION_QUEUE_URL",
        "the secret edge publishes no queue message",
    ),
];

/// Resolved configuration. Nothing here has a default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Deployment plane.
    pub plane: String,
    /// Pinned region.
    pub region: Region,
    /// Release digest reported by readiness.
    pub release_digest: String,
    /// `central-authz` function.
    pub authz_function: Arn,
    /// Verification key-set parameter name.
    pub authz_verify_keys_param: String,
    /// Regional authorization projection table.
    pub authz_projection_table: String,
    /// `regional-secret-custody` table.
    pub secret_custody_table: String,
    /// `regional-secret-keystore` table.
    pub secret_keystore_table: String,
    /// The secret KMS key.
    pub secret_kms_key: Arn,
    /// Branch-key cache byte budget.
    pub branch_key_cache_bytes: usize,
    /// Branch-key cache lifetime in milliseconds.
    pub branch_key_cache_ttl_ms: u64,
    /// Assertion cache byte budget.
    pub assertion_cache_bytes: usize,
    /// Effective encoded JSON body bound.
    pub max_json_body_bytes: usize,
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
        Ok(Self {
            plane,
            region,
            release_digest: required(lookup, RELEASE_DIGEST)?,
            authz_function: arn_in_region(lookup, AUTHZ_FUNCTION_ARN, region, "lambda")?,
            authz_verify_keys_param: required(lookup, AUTHZ_VERIFY_KEYS_PARAM)?,
            authz_projection_table: required(lookup, AUTHZ_PROJECTION_TABLE)?,
            secret_custody_table: required(lookup, SECRET_CUSTODY_TABLE)?,
            secret_keystore_table: required(lookup, SECRET_KEYSTORE_TABLE)?,
            secret_kms_key: arn_in_region(lookup, SECRET_KMS_KEY_ARN, region, "kms")?,
            branch_key_cache_bytes: bounded_usize(
                lookup,
                BRANCH_KEY_CACHE_BYTES,
                1_024,
                64 * 1_024 * 1_024,
            )?,
            branch_key_cache_ttl_ms: bounded_u64(
                lookup,
                BRANCH_KEY_CACHE_TTL_MS,
                1_000,
                3_600_000,
            )?,
            assertion_cache_bytes: bounded_usize(
                lookup,
                ASSERTION_CACHE_BYTES,
                1_024,
                64 * 1_024 * 1_024,
            )?,
            max_json_body_bytes: bounded_usize(
                lookup,
                MAX_JSON_BODY_BYTES,
                1_024,
                10 * 1_024 * 1_024,
            )?,
        })
    }

    /// The cache partition every sealed value is bound to.
    ///
    /// Plane and region are part of it so a `dev` branch key can never open a
    /// `prd` value even if both ever reached the same process.
    #[must_use]
    pub fn crypto_partition(&self) -> String {
        format!("{}:{}", self.plane, self.region.as_str())
    }
}
