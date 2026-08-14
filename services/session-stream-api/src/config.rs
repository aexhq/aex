//! Validated start-up configuration for `session-api`.
//!
//! Nothing here has a default. Every table, bucket, key and function is named
//! explicitly, and every ARN must resolve to the region this process is pinned
//! to: a resource in another region passes every type check, deploys cleanly and
//! then silently writes a tenant's data outside its declared residency.
//!
//! Provider-credential registration lives here too, so the secret key and
//! branch-key store are explicit required bindings.

use aex_regional_http::config::{
    Arn, Lookup, RegionalHttpConfigError, arn_in_region, bounded_u64, bounded_usize, forbidden,
    one_of, plane_name, region, required,
};
use aex_regional_http::drain::{MAX_DRAIN_DEADLINE_MS, MIN_DRAIN_DEADLINE_MS};
use aex_wire::types::{HttpsUrl, Region};

/// The deployable this configuration belongs to.
pub const DEPLOYABLE: &str = "session-api";

// --- shared: bound identically by both halves --------------------------------

/// Deployment plane: `dev` or `prd`.
pub const PLANE: &str = "AEX_PLANE";
/// The region this process is pinned to.
pub const REGION: &str = "AEX_REGION";
/// The release digest reported by `/internal/readyz`.
pub const RELEASE_DIGEST: &str = "AEX_RELEASE_DIGEST";
/// The port the container listens on.
///
/// The listener port.
pub const PORT: &str = "AEX_PORT";
/// Drain deadline in milliseconds.
///
/// Bound below the task definition's stop timeout by
/// [`aex_regional_http::drain::MAX_DRAIN_DEADLINE_MS`] rather than by a comment:
/// the deadline exists to end a drain before the runtime sends `SIGKILL`, and
/// one set above that timeout is a deadline that never fires.
pub const DRAIN_DEADLINE_MS: &str = "AEX_DRAIN_DEADLINE_MS";
/// The Secrets Manager id holding the credential pepper ring the edge verifies.
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
/// signature to verify, because a presented key is checked against the verifier
/// the control plane replicated onto its authorization row.
pub const CREDENTIAL_PEPPER_REF: &str = "AEX_CREDENTIAL_PEPPER_REF";
/// The regional authorization projection table.
pub const AUTHZ_PROJECTION_TABLE: &str = "AEX_AUTHZ_PROJECTION_TABLE";
/// The `session-authority` table.
pub const SESSION_AUTHORITY_TABLE: &str = "AEX_SESSION_AUTHORITY_TABLE";
/// The regional content bucket.
pub const CONTENT_BUCKET: &str = "AEX_CONTENT_BUCKET";
/// Existing immutable AEX-generated session telemetry bucket.
pub const SESSION_TELEMETRY_BUCKET: &str = "AEX_SESSION_TELEMETRY_BUCKET";
/// Exact KMS key encrypting immutable session telemetry.
pub const SESSION_TELEMETRY_KMS_KEY_ARN: &str = "AEX_SESSION_TELEMETRY_KMS_KEY_ARN";
/// The storage reference of the cursor signing key.
pub const CURSOR_SIGNING_KEY_REF: &str = "AEX_CURSOR_SIGNING_KEY_REF";

// --- session half ------------------------------------------------------------

/// The canonical public base URL for this regional API.
pub const REGIONAL_API_URL: &str = "AEX_REGIONAL_API_URL";
/// The `regional-work` table.
///
/// Reaching this table requires a `Grant<WorkClaim>`; binding the name here does
/// not by itself give any route a write.
pub const WORK_TABLE: &str = "AEX_WORK_TABLE";
/// Exact live alias of the isolated session maintenance worker.
pub const SESSION_MAINTENANCE_WORKER_FUNCTION_ARN: &str =
    "AEX_SESSION_MAINTENANCE_WORKER_FUNCTION_ARN";
/// The latest-only file and immutable-content metadata authority.
pub const FILE_AUTHORITY_TABLE: &str = "AEX_FILE_AUTHORITY_TABLE";
/// The secret KMS key. Never the content key.
pub const SECRET_KMS_KEY_ARN: &str = "AEX_SECRET_KMS_KEY_ARN";
/// Branch-key cache byte budget.
pub const BRANCH_KEY_CACHE_BYTES: &str = "AEX_SECRET_BRANCH_KEY_CACHE_BYTES";
/// Branch-key cache lifetime in milliseconds.
pub const BRANCH_KEY_CACHE_TTL_MS: &str = "AEX_SECRET_BRANCH_KEY_CACHE_TTL_MS";
/// The `runtime-activity` table.
pub const RUNTIME_ACTIVITY_TABLE: &str = "AEX_RUNTIME_ACTIVITY_TABLE";
/// The central billing FIFO used by trusted runtime usage accounting.
pub const USAGE_RATING_QUEUE_URL: &str = "AEX_USAGE_RATING_QUEUE_URL";
/// Runtime due-index shard count (constructor setting; this edge does not scan it).
pub const RUNTIME_DUE_SHARDS: &str = "AEX_RUNTIME_DUE_SHARDS";
/// Runtime due page item ceiling.
pub const RUNTIME_DUE_PAGE_ITEMS: &str = "AEX_RUNTIME_DUE_PAGE_ITEMS";
/// Runtime due page read ceiling.
pub const RUNTIME_DUE_PAGE_READS: &str = "AEX_RUNTIME_DUE_PAGE_READS";
/// Exact pricing version attached to resume usage drafts.
pub const PRICING_VERSION: &str = "AEX_PRICING_VERSION";
/// The account that must own the content bucket.
pub const CONTENT_BUCKET_OWNER: &str = "AEX_CONTENT_BUCKET_OWNER";
/// Exact release-derived five-row Hands image catalog.
pub const HANDS_IMAGE_CATALOG: &str = "AEX_HANDS_IMAGE_CATALOG";
/// Whether this plane binds the managed public-internet egress connector.
pub const HANDS_PUBLIC_INTERNET_EGRESS: &str = "AEX_HANDS_PUBLIC_INTERNET_EGRESS";
/// The content KMS key. Never the secret key.
pub const CONTENT_KMS_KEY_ARN: &str = "AEX_CONTENT_KMS_KEY_ARN";
/// The effective encoded JSON body bound.
pub const MAX_JSON_BODY_BYTES: &str = "AEX_MAX_JSON_BODY_BYTES";
/// The effective page item bound.
pub const MAX_PAGE_ITEMS: &str = "AEX_MAX_PAGE_ITEMS";
/// The effective serialized page byte bound.
pub const MAX_PAGE_BYTES: &str = "AEX_MAX_PAGE_BYTES";

/// Every variable a healthy `session-stream-api` requires.
pub const REQUIRED: [&str; 32] = [
    PLANE,
    REGION,
    RELEASE_DIGEST,
    PORT,
    DRAIN_DEADLINE_MS,
    CREDENTIAL_PEPPER_REF,
    AUTHZ_PROJECTION_TABLE,
    SESSION_AUTHORITY_TABLE,
    CONTENT_BUCKET,
    SESSION_TELEMETRY_BUCKET,
    SESSION_TELEMETRY_KMS_KEY_ARN,
    CURSOR_SIGNING_KEY_REF,
    REGIONAL_API_URL,
    WORK_TABLE,
    SESSION_MAINTENANCE_WORKER_FUNCTION_ARN,
    FILE_AUTHORITY_TABLE,
    SECRET_KMS_KEY_ARN,
    BRANCH_KEY_CACHE_BYTES,
    BRANCH_KEY_CACHE_TTL_MS,
    RUNTIME_ACTIVITY_TABLE,
    USAGE_RATING_QUEUE_URL,
    RUNTIME_DUE_SHARDS,
    RUNTIME_DUE_PAGE_ITEMS,
    RUNTIME_DUE_PAGE_READS,
    PRICING_VERSION,
    CONTENT_BUCKET_OWNER,
    HANDS_IMAGE_CATALOG,
    HANDS_PUBLIC_INTERNET_EGRESS,
    CONTENT_KMS_KEY_ARN,
    MAX_JSON_BODY_BYTES,
    MAX_PAGE_ITEMS,
    MAX_PAGE_BYTES,
];

/// Variables this binary must never be bound to, with the reason.
///
/// The binary publishes no queue message: after committing durable work it
/// asynchronously invokes the exact operation-worker Lambda alias. The worker
/// owns its queue, retries and deletion fan-out; binding that queue here would
/// give the public edge a second dispatch path.
///
pub const FORBIDDEN: [(&str, &str); 2] = [
    (
        "AEX_OPERATION_QUEUE_URL",
        "this edge publishes no queue message; the hint is derived from the work stream",
    ),
    (
        "AEX_CONTENT_QUEUE_URL",
        "this edge publishes no queue message",
    ),
];

/// The number of edges this process builds.
pub const EDGE_COUNT: usize = 1;

/// Resolved configuration. Nothing here has a default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Deployment plane.
    pub plane: aex_identity_domain::assertion::Plane,
    /// Pinned region.
    pub region: Region,
    /// Canonical public base URL for workspace discovery responses.
    pub regional_api_url: HttpsUrl,
    /// Release digest reported by readiness.
    pub release_digest: String,
    /// The `TCP` port the one listener binds.
    pub port: u16,
    /// How long a drain may run before the listener is abandoned.
    pub drain_deadline_ms: u64,
    /// Credential pepper ring Secrets Manager id.
    pub credential_pepper_ref: String,
    /// Regional authorization projection table.
    pub authz_projection_table: String,
    /// `session-authority` table.
    pub session_table: String,
    /// `regional-work` table.
    pub work_table: String,
    /// Exact qualified session-maintenance-worker Lambda ARN.
    pub session_maintenance_worker: Arn,
    /// Latest-only file and immutable-content metadata authority.
    pub file_authority_table: String,
    /// Secret KMS key used only by provider-credential registration.
    pub secret_kms_key: Arn,
    /// Branch-key cache byte budget.
    pub branch_key_cache_bytes: usize,
    /// Branch-key cache lifetime in milliseconds.
    pub branch_key_cache_ttl_ms: u64,
    /// `runtime-activity` table.
    pub runtime_activity_table: String,
    /// Central billing FIFO used by same-generation resume accounting.
    pub usage_rating_queue_url: String,
    /// Runtime-control due-index shard count.
    pub runtime_due_shards: u16,
    /// Runtime-control due scan budget.
    pub runtime_due_page: aex_runtime_control::store::PageBudget,
    /// Exact usage pricing version.
    pub pricing_version: String,
    /// Regional content bucket.
    pub content_bucket: String,
    /// Immutable AEX-generated session telemetry bucket.
    pub session_telemetry_bucket: String,
    /// Exact KMS key encrypting session telemetry.
    pub session_telemetry_kms_key: Arn,
    /// Expected content-bucket owner account.
    pub content_bucket_owner: String,
    /// Verified immutable image and deployment capability facts.
    pub deployment: aex_session_app::DeploymentFacts,
    /// Content KMS key.
    pub content_kms_key: Arn,
    /// Cursor signing key reference.
    pub cursor_signing_key_ref: String,
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
    /// Returns the first [`RegionalHttpConfigError`], naming the offending variable.
    pub fn from_env() -> Result<Self, RegionalHttpConfigError> {
        Self::read(&aex_regional_http::config::Environment)
    }

    /// Reads and validates the configuration from an arbitrary lookup.
    ///
    /// # Errors
    ///
    /// Identical to [`Config::from_env`].
    #[allow(clippy::too_many_lines, reason = "one arm per declared variable")]
    pub fn read<L: Lookup + ?Sized>(lookup: &L) -> Result<Self, RegionalHttpConfigError> {
        for (name, reason) in FORBIDDEN {
            forbidden(lookup, name, DEPLOYABLE, reason)?;
        }
        let plane = plane_name(lookup, PLANE)?;
        let region = region(lookup, REGION)?;
        let regional_api_url =
            HttpsUrl::parse(&required(lookup, REGIONAL_API_URL)?).map_err(|error| {
                RegionalHttpConfigError::Invalid {
                    name: REGIONAL_API_URL,
                    reason: error.to_string(),
                }
            })?;
        let content_kms_key = arn_in_region(lookup, CONTENT_KMS_KEY_ARN, region, "kms")?;
        let session_telemetry_kms_key =
            arn_in_region(lookup, SESSION_TELEMETRY_KMS_KEY_ARN, region, "kms")?;
        let secret_kms_key = arn_in_region(lookup, SECRET_KMS_KEY_ARN, region, "kms")?;
        let session_maintenance_worker = arn_in_region(
            lookup,
            SESSION_MAINTENANCE_WORKER_FUNCTION_ARN,
            region,
            "lambda",
        )?;
        let expected_worker = format!(
            "function:aex-{}-session-maintenance-worker:live",
            plane.as_str()
        );
        if session_maintenance_worker.resource != expected_worker {
            return Err(RegionalHttpConfigError::Invalid {
                name: SESSION_MAINTENANCE_WORKER_FUNCTION_ARN,
                reason: format!(
                    "expected exact live alias `{expected_worker}`, got `{}`",
                    session_maintenance_worker.resource
                ),
            });
        }
        let content_bucket_owner = required(lookup, CONTENT_BUCKET_OWNER)?;
        if content_bucket_owner != content_kms_key.account {
            return Err(RegionalHttpConfigError::Invalid {
                name: CONTENT_BUCKET_OWNER,
                reason: format!(
                    "bucket owner `{content_bucket_owner}` differs from the content key account `{}`",
                    content_kms_key.account
                ),
            });
        }
        if session_maintenance_worker.account != content_bucket_owner {
            return Err(RegionalHttpConfigError::Invalid {
                name: SESSION_MAINTENANCE_WORKER_FUNCTION_ARN,
                reason: "worker alias and regional resources must share one account".to_owned(),
            });
        }
        if session_telemetry_kms_key.account != content_bucket_owner {
            return Err(RegionalHttpConfigError::Invalid {
                name: SESSION_TELEMETRY_KMS_KEY_ARN,
                reason: "session telemetry and regional resources must share one account"
                    .to_owned(),
            });
        }
        if session_telemetry_kms_key.value == content_kms_key.value
            || session_telemetry_kms_key.value == secret_kms_key.value
        {
            return Err(RegionalHttpConfigError::Invalid {
                name: SESSION_TELEMETRY_KMS_KEY_ARN,
                reason: "session telemetry must use its dedicated KMS key".to_owned(),
            });
        }
        let image_catalog_raw = required(lookup, HANDS_IMAGE_CATALOG)?;
        let images = aex_runtime_control::HandsImageCatalog::from_release_json(
            &image_catalog_raw,
            plane.as_str(),
            region.as_str(),
            &content_bucket_owner,
        )
        .map_err(|error| RegionalHttpConfigError::Invalid {
            name: HANDS_IMAGE_CATALOG,
            reason: error.to_string(),
        })?;
        let public_internet_egress =
            one_of(lookup, HANDS_PUBLIC_INTERNET_EGRESS, &["false", "true"])? == "true";
        let deployment = aex_session_app::DeploymentFacts {
            public_internet_egress,
            images,
            package_ecosystems: aex_runtime_control::HANDS_PACKAGE_ECOSYSTEMS
                .into_iter()
                .collect(),
        };
        let usage_rating_queue_url = required(lookup, USAGE_RATING_QUEUE_URL)?;
        let queue_prefix = format!("https://sqs.{}.amazonaws.com/", region.as_str());
        if !usage_rating_queue_url.starts_with(&queue_prefix)
            || !usage_rating_queue_url.ends_with(".fifo")
        {
            return Err(RegionalHttpConfigError::Invalid {
                name: USAGE_RATING_QUEUE_URL,
                reason: format!("must be an SQS FIFO queue URL in `{}`", region.as_str()),
            });
        }
        let runtime_due_shards_raw = bounded_u64(lookup, RUNTIME_DUE_SHARDS, 1, u16::MAX.into())?;
        let runtime_due_shards = u16::try_from(runtime_due_shards_raw).map_err(|_| {
            RegionalHttpConfigError::Invalid {
                name: RUNTIME_DUE_SHARDS,
                reason: "does not fit u16".to_owned(),
            }
        })?;
        let runtime_due_page_items = bounded_u64(lookup, RUNTIME_DUE_PAGE_ITEMS, 1, 32)?;
        let runtime_due_page_reads = bounded_u64(lookup, RUNTIME_DUE_PAGE_READS, 1, 10_000)?;
        let pricing_version = required(lookup, PRICING_VERSION)?;

        let raw_port = bounded_u64(lookup, PORT, 1, 65_535)?;
        let port = u16::try_from(raw_port).map_err(|_| RegionalHttpConfigError::Invalid {
            name: PORT,
            reason: format!("`{raw_port}` is not a TCP port"),
        })?;

        let drain_deadline_ms = bounded_u64(
            lookup,
            DRAIN_DEADLINE_MS,
            MIN_DRAIN_DEADLINE_MS,
            MAX_DRAIN_DEADLINE_MS,
        )?;
        Ok(Self {
            plane,
            region,
            regional_api_url,
            release_digest: required(lookup, RELEASE_DIGEST)?,
            port,
            drain_deadline_ms,
            credential_pepper_ref: required(lookup, CREDENTIAL_PEPPER_REF)?,
            authz_projection_table: required(lookup, AUTHZ_PROJECTION_TABLE)?,
            session_table: required(lookup, SESSION_AUTHORITY_TABLE)?,
            work_table: required(lookup, WORK_TABLE)?,
            session_maintenance_worker,
            file_authority_table: required(lookup, FILE_AUTHORITY_TABLE)?,
            secret_kms_key,
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
            runtime_activity_table: required(lookup, RUNTIME_ACTIVITY_TABLE)?,
            usage_rating_queue_url,
            runtime_due_shards,
            runtime_due_page: aex_runtime_control::store::PageBudget {
                max_items: u32::try_from(runtime_due_page_items).unwrap_or(u32::MAX),
                max_reads: u32::try_from(runtime_due_page_reads).unwrap_or(u32::MAX),
            },
            pricing_version,
            content_bucket: required(lookup, CONTENT_BUCKET)?,
            session_telemetry_bucket: required(lookup, SESSION_TELEMETRY_BUCKET)?,
            session_telemetry_kms_key,
            content_bucket_owner,
            deployment,
            content_kms_key,
            cursor_signing_key_ref: required(lookup, CURSOR_SIGNING_KEY_REF)?,
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

    /// The effective limits this deployable's unary edge enforces.
    #[must_use]
    pub const fn limits(&self) -> aex_regional_http::context::EffectiveLimits {
        aex_regional_http::context::EffectiveLimits {
            json_body_bytes: self.max_json_body_bytes,
            otlp_body_bytes: 4 * 1_024 * 1_024,
            query_page_items: self.max_page_items,
            query_page_bytes: self.max_page_bytes,
        }
    }

    /// Cache partition bound into every provider-credential ciphertext.
    #[must_use]
    pub fn crypto_partition(&self) -> String {
        format!("{}:{}", self.plane.as_str(), self.region.as_str())
    }
}
