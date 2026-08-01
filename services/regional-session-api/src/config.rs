//! Validated start-up configuration for `regional-session-api`.
//!
//! Nothing here has a default. Every table, bucket, key and function is named
//! explicitly, and every ARN must resolve to the region this process is pinned
//! to: a resource in another region passes every type check, deploys cleanly and
//! then silently writes a tenant's data outside its declared residency.

use aex_regional_http::config::{
    Arn, ConfigError, Lookup, arn_in_region, bounded_usize, forbidden, plane_name, region, required,
};
use aex_wire::types::Region;

/// The deployable this configuration belongs to.
pub const DEPLOYABLE: &str = "regional-session-api";

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
/// The `session-authority` table.
pub const SESSION_TABLE: &str = "AEX_SESSION_TABLE";
/// The `regional-work` table.
pub const WORK_TABLE: &str = "AEX_WORK_TABLE";
/// The `regional-content` table.
pub const CONTENT_TABLE: &str = "AEX_CONTENT_TABLE";
/// The `regional-registry` table.
pub const REGISTRY_TABLE: &str = "AEX_REGISTRY_TABLE";
/// The `regional-secret-custody` table, read for ciphertext metadata only.
pub const SECRET_CUSTODY_TABLE: &str = "AEX_SECRET_CUSTODY_TABLE";
/// The `runtime-activity` table.
pub const RUNTIME_ACTIVITY_TABLE: &str = "AEX_RUNTIME_ACTIVITY_TABLE";
/// The `usage-query-projection` table.
pub const USAGE_QUERY_TABLE: &str = "AEX_USAGE_QUERY_TABLE";
/// The regional content bucket.
pub const CONTENT_BUCKET: &str = "AEX_CONTENT_BUCKET";
/// The account that must own the content bucket.
pub const CONTENT_BUCKET_OWNER: &str = "AEX_CONTENT_BUCKET_OWNER";
/// The content KMS key. Never the secret key.
pub const CONTENT_KMS_KEY_ARN: &str = "AEX_CONTENT_KMS_KEY_ARN";
/// The storage reference of the cursor signing key.
pub const CURSOR_SIGNING_KEY_REF: &str = "AEX_CURSOR_SIGNING_KEY_REF";
/// Assertion cache byte budget.
pub const ASSERTION_CACHE_BYTES: &str = "AEX_ASSERTION_CACHE_BYTES";
/// The effective encoded JSON body bound.
pub const MAX_JSON_BODY_BYTES: &str = "AEX_MAX_JSON_BODY_BYTES";
/// The effective page item bound.
pub const MAX_PAGE_ITEMS: &str = "AEX_MAX_PAGE_ITEMS";
/// The effective serialized page byte bound.
pub const MAX_PAGE_BYTES: &str = "AEX_MAX_PAGE_BYTES";

/// Every variable a healthy `regional-session-api` requires, in declaration order.
pub const REQUIRED: [&str; 21] = [
    PLANE,
    REGION,
    RELEASE_DIGEST,
    AUTHZ_FUNCTION_ARN,
    AUTHZ_VERIFY_KEYS_PARAM,
    AUTHZ_PROJECTION_TABLE,
    SESSION_TABLE,
    WORK_TABLE,
    CONTENT_TABLE,
    REGISTRY_TABLE,
    SECRET_CUSTODY_TABLE,
    RUNTIME_ACTIVITY_TABLE,
    USAGE_QUERY_TABLE,
    CONTENT_BUCKET,
    CONTENT_BUCKET_OWNER,
    CONTENT_KMS_KEY_ARN,
    CURSOR_SIGNING_KEY_REF,
    ASSERTION_CACHE_BYTES,
    MAX_JSON_BODY_BYTES,
    MAX_PAGE_ITEMS,
    MAX_PAGE_BYTES,
];

/// Variables this binary must never be bound to, with the reason.
///
/// The finite API emits no queue message at all (RS-02): the SQS hint is derived
/// from the `regional-work` stream after the transaction commits, so a queue URL
/// bound here would mean somebody reintroduced a pre-commit hint. It also holds
/// no secret KMS key, because it reads ciphertext metadata and never decrypts.
pub const FORBIDDEN: [(&str, &str); 3] = [
    (
        "AEX_OPERATION_QUEUE_URL",
        "the finite API publishes no queue message; the hint is derived from the work stream",
    ),
    (
        "AEX_CONTENT_QUEUE_URL",
        "the finite API publishes no queue message",
    ),
    (
        "AEX_SECRET_KMS_KEY_ARN",
        "the finite API reads ciphertext metadata and holds no decrypt capability",
    ),
];

/// Resolved configuration. Nothing here has a default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Deployment plane.
    pub plane: aex_identity_domain::assertion::Plane,
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
    /// `session-authority` table.
    pub session_table: String,
    /// `regional-work` table.
    pub work_table: String,
    /// `regional-content` table.
    pub content_table: String,
    /// `regional-registry` table.
    pub registry_table: String,
    /// `regional-secret-custody` table.
    pub secret_custody_table: String,
    /// `runtime-activity` table.
    pub runtime_activity_table: String,
    /// `usage-query-projection` table.
    pub usage_query_table: String,
    /// Regional content bucket.
    pub content_bucket: String,
    /// Expected content-bucket owner account.
    pub content_bucket_owner: String,
    /// Content KMS key.
    pub content_kms_key: Arn,
    /// Cursor signing key reference.
    pub cursor_signing_key_ref: String,
    /// Assertion cache byte budget.
    pub assertion_cache_bytes: usize,
    /// Effective encoded JSON body bound.
    pub max_json_body_bytes: usize,
    /// Effective page item bound.
    pub max_page_items: usize,
    /// Effective serialized page byte bound.
    pub max_page_bytes: usize,
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
        let content_kms_key = arn_in_region(lookup, CONTENT_KMS_KEY_ARN, region, "kms")?;
        let content_bucket_owner = required(lookup, CONTENT_BUCKET_OWNER)?;
        if content_bucket_owner != content_kms_key.account {
            return Err(ConfigError::Invalid {
                name: CONTENT_BUCKET_OWNER,
                reason: format!(
                    "bucket owner `{content_bucket_owner}` differs from the content key account `{}`",
                    content_kms_key.account
                ),
            });
        }
        Ok(Self {
            plane,
            region,
            release_digest: required(lookup, RELEASE_DIGEST)?,
            authz_function: arn_in_region(lookup, AUTHZ_FUNCTION_ARN, region, "lambda")?,
            authz_verify_keys_param: required(lookup, AUTHZ_VERIFY_KEYS_PARAM)?,
            authz_projection_table: required(lookup, AUTHZ_PROJECTION_TABLE)?,
            session_table: required(lookup, SESSION_TABLE)?,
            work_table: required(lookup, WORK_TABLE)?,
            content_table: required(lookup, CONTENT_TABLE)?,
            registry_table: required(lookup, REGISTRY_TABLE)?,
            secret_custody_table: required(lookup, SECRET_CUSTODY_TABLE)?,
            runtime_activity_table: required(lookup, RUNTIME_ACTIVITY_TABLE)?,
            usage_query_table: required(lookup, USAGE_QUERY_TABLE)?,
            content_bucket: required(lookup, CONTENT_BUCKET)?,
            content_bucket_owner,
            content_kms_key,
            cursor_signing_key_ref: required(lookup, CURSOR_SIGNING_KEY_REF)?,
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
            max_page_items: bounded_usize(lookup, MAX_PAGE_ITEMS, 1, 1_000)?,
            max_page_bytes: bounded_usize(lookup, MAX_PAGE_BYTES, 1_024, 8 * 1_024 * 1_024)?,
        })
    }

    /// The effective limits this deployable's edge enforces.
    #[must_use]
    pub const fn limits(&self) -> aex_regional_http::context::EffectiveLimits {
        aex_regional_http::context::EffectiveLimits {
            json_body_bytes: self.max_json_body_bytes,
            query_page_items: self.max_page_items,
            query_page_bytes: self.max_page_bytes,
        }
    }
}
