//! Validated start-up configuration for `regional-secret-api`.
//!
//! This deployable is the only one that ever holds secret plaintext, so its
//! configuration is deliberately narrow: it binds the two secret tables and the
//! secret KMS key and **refuses to start** when a session, content, work, queue
//! or registry variable is bound to it. That is the configuration half of
//! capability admission (RS-09) — IAM is the other half, and neither is a single
//! point of failure.

use aex_regional_http::config::{
    Arn, Lookup, RegionalHttpConfigError, arn_in_region, bounded_u64, bounded_usize, forbidden,
    plane_name, region, required,
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
/// The Secrets Manager id holding the credential pepper ring this edge verifies
/// against.
///
/// It names the id the issuing authority also reads —
/// `aex/<plane>/central/token-pepper` — and not a regional copy of it. One
/// stored document with two readers cannot drift; two copies can, and a
/// rotation that reached only one of them would leave keys minted under the new
/// version verifying centrally and failing here, silently and only for some
/// keys. Read once at cold start and held, so a version added afterwards is
/// invisible here until this process restarts.
///
/// This replaced `AEX_AUTHZ_FUNCTION_ARN` and `AEX_AUTHZ_VERIFY_KEYS_PARAM`
/// together: there is no `central-authz` invoke to address and no assertion
/// signature to verify, because a presented key is checked in-process against
/// the verifier the control plane replicated onto its authorization row.
pub const CREDENTIAL_PEPPER_REF: &str = "AEX_CREDENTIAL_PEPPER_REF";
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
/// The effective encoded JSON body bound.
pub const MAX_JSON_BODY_BYTES: &str = "AEX_MAX_JSON_BODY_BYTES";

/// Every variable a healthy `regional-secret-api` requires, in declaration order.
pub const REQUIRED: [&str; 11] = [
    PLANE,
    REGION,
    RELEASE_DIGEST,
    CREDENTIAL_PEPPER_REF,
    AUTHZ_PROJECTION_TABLE,
    SECRET_CUSTODY_TABLE,
    SECRET_KEYSTORE_TABLE,
    SECRET_KMS_KEY_ARN,
    BRANCH_KEY_CACHE_BYTES,
    BRANCH_KEY_CACHE_TTL_MS,
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
    pub plane: aex_identity_domain::assertion::Plane,
    /// Pinned region.
    pub region: Region,
    /// Release digest reported by readiness.
    pub release_digest: String,
    /// Credential pepper ring Secrets Manager id.
    pub credential_pepper_ref: String,
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
    /// Effective encoded JSON body bound.
    pub max_json_body_bytes: usize,
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
        Ok(Self {
            plane,
            region,
            release_digest: required(lookup, RELEASE_DIGEST)?,
            credential_pepper_ref: required(lookup, CREDENTIAL_PEPPER_REF)?,
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
            max_json_body_bytes: bounded_usize(
                lookup,
                MAX_JSON_BODY_BYTES,
                1_024,
                10 * 1_024 * 1_024,
            )?,
        })
    }

    /// The effective limits this deployable's edge enforces.
    ///
    /// `regional-secret-api` owns one route and it is not a listing, so
    /// the two page bounds are not configurable here and are zero. Zero is the
    /// honest value: it is not a page size this deployable would ever use, so a
    /// handler that started paginating would fail its own budget check rather
    /// than silently inherit a number nobody chose.
    /// `a_served_route_never_paginates` holds the premise.
    #[must_use]
    pub const fn limits(&self) -> aex_regional_http::context::EffectiveLimits {
        aex_regional_http::context::EffectiveLimits {
            json_body_bytes: self.max_json_body_bytes,
            otlp_body_bytes: 4 * 1_024 * 1_024,
            query_page_items: 0,
            query_page_bytes: 0,
        }
    }

    /// The cache partition every sealed value is bound to.
    ///
    /// Plane and region are part of it so a `dev` branch key can never open a
    /// `prd` value even if both ever reached the same process.
    #[must_use]
    pub fn crypto_partition(&self) -> String {
        format!("{}:{}", self.plane.as_str(), self.region.as_str())
    }
}
