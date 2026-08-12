//! Validated start-up configuration for `session-stream-api`.
//!
//! Nothing here has a default. Every table, bucket, key and function is named
//! explicitly, and every ARN must resolve to the region this process is pinned
//! to: a resource in another region passes every type check, deploys cleanly and
//! then silently writes a tenant's data outside its declared residency.
//!
//! # What merging cost, and what replaced it
//!
//! `regional-stream` used to forbid `AEX_WORK_TABLE` outright, on the ground
//! that "the stream admits no durable work". One process cannot both require and
//! forbid a variable, so that guard is gone. It is replaced by a stronger claim
//! made at the type level rather than at the environment: the write handle lives
//! behind [`crate::capability::Composition`]'s `Grant<WorkClaim>`, the stream's
//! `AppState` has no field that can produce one, and no amount of environment
//! binding gives a stream handler a path to a write. See [`crate::capability`].
//!
//! The remaining refusals were forbidden by both original halves and are
//! unchanged. Provider-credential registration now lives here too, so the
//! secret key and branch-key store are explicit required bindings.

use aex_regional_http::config::{
    Arn, Lookup, RegionalHttpConfigError, arn_in_region, bounded_u64, bounded_usize, forbidden,
    one_of, plane_name, region, required,
};
use aex_regional_http::drain::{MAX_DRAIN_DEADLINE_MS, MIN_DRAIN_DEADLINE_MS};
use aex_wire::types::{HttpsUrl, Region};

/// The deployable this configuration belongs to.
pub const DEPLOYABLE: &str = "session-stream-api";

// --- shared: bound identically by both halves --------------------------------

/// Deployment plane: `dev` or `prd`.
pub const PLANE: &str = "AEX_PLANE";
/// The region this process is pinned to.
pub const REGION: &str = "AEX_REGION";
/// The release digest reported by `/internal/readyz`.
pub const RELEASE_DIGEST: &str = "AEX_RELEASE_DIGEST";
/// The port the container listens on.
///
/// One listener serves both halves, so there is one port. This replaces
/// `AEX_SESSION_PORT` and `AEX_STREAM_PORT`, which named the same container port
/// twice and could disagree.
pub const PORT: &str = "AEX_PORT";
/// Drain deadline in milliseconds.
///
/// Bound below the task definition's stop timeout by
/// [`aex_regional_http::drain::MAX_DRAIN_DEADLINE_MS`] rather than by a comment:
/// the deadline exists to end a drain before the runtime sends `SIGKILL`, and
/// one set above that timeout is a deadline that never fires. This replaces
/// `AEX_SESSION_DRAIN_DEADLINE_MS` and `AEX_STREAM_DRAIN_DEADLINE_MS`, which
/// drained one process each and now drain one.
pub const DRAIN_DEADLINE_MS: &str = "AEX_DRAIN_DEADLINE_MS";
/// The Secrets Manager id holding the credential pepper ring both edges verify
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
/// signature to verify, because a presented key is checked against the verifier
/// the control plane replicated onto its authorization row.
pub const CREDENTIAL_PEPPER_REF: &str = "AEX_CREDENTIAL_PEPPER_REF";
/// The regional authorization projection table.
pub const AUTHZ_PROJECTION_TABLE: &str = "AEX_AUTHZ_PROJECTION_TABLE";
/// The `session-authority` table.
pub const SESSION_TABLE: &str = "AEX_SESSION_TABLE";
/// The regional content bucket.
pub const CONTENT_BUCKET: &str = "AEX_CONTENT_BUCKET";
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
/// Exact live alias of the isolated session operation worker.
pub const SESSION_OPERATION_WORKER_FUNCTION_ARN: &str = "AEX_SESSION_OPERATION_WORKER_FUNCTION_ARN";
/// The `regional-content` table.
pub const CONTENT_TABLE: &str = "AEX_CONTENT_TABLE";
/// The `regional-registry` table.
pub const REGISTRY_TABLE: &str = "AEX_REGISTRY_TABLE";
/// The `regional-secret-custody` table, read for ciphertext metadata only.
pub const SECRET_CUSTODY_TABLE: &str = "AEX_SECRET_CUSTODY_TABLE";
/// The `regional-secret-keystore` table used by credential registration.
pub const SECRET_KEYSTORE_TABLE: &str = "AEX_SECRET_KEYSTORE_TABLE";
/// The secret KMS key. Never the content key.
pub const SECRET_KMS_KEY_ARN: &str = "AEX_SECRET_KMS_KEY_ARN";
/// Branch-key cache byte budget.
pub const BRANCH_KEY_CACHE_BYTES: &str = "AEX_SECRET_BRANCH_KEY_CACHE_BYTES";
/// Branch-key cache lifetime in milliseconds.
pub const BRANCH_KEY_CACHE_TTL_MS: &str = "AEX_SECRET_BRANCH_KEY_CACHE_TTL_MS";
/// The `runtime-activity` table.
pub const RUNTIME_ACTIVITY_TABLE: &str = "AEX_RUNTIME_ACTIVITY_TABLE";
/// Compute usage ingress shared with runtime-control resume accounting.
pub const USAGE_COMPUTE_QUEUE_URL: &str = "AEX_USAGE_COMPUTE_QUEUE_URL";
/// Storage usage ingress shared with runtime-control resume accounting.
pub const USAGE_STORAGE_QUEUE_URL: &str = "AEX_USAGE_STORAGE_QUEUE_URL";
/// Runtime due-index shard count (constructor setting; this edge does not scan it).
pub const RUNTIME_DUE_SHARDS: &str = "AEX_RUNTIME_DUE_SHARDS";
/// Runtime due page item ceiling.
pub const RUNTIME_DUE_PAGE_ITEMS: &str = "AEX_RUNTIME_DUE_PAGE_ITEMS";
/// Runtime due page read ceiling.
pub const RUNTIME_DUE_PAGE_READS: &str = "AEX_RUNTIME_DUE_PAGE_READS";
/// Exact pricing version attached to resume usage drafts.
pub const PRICING_VERSION: &str = "AEX_PRICING_VERSION";
/// The `usage-query-projection` table.
pub const USAGE_QUERY_TABLE: &str = "AEX_USAGE_QUERY_TABLE";
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

// --- stream half -------------------------------------------------------------

/// The `observation-authority` table.
pub const OBSERVATION_TABLE: &str = "AEX_OBSERVATION_TABLE";
/// The `session-authority` stream this task tails in `ddb_streams` mode.
pub const SESSION_TABLE_STREAM_ARN: &str = "AEX_SESSION_TABLE_STREAM_ARN";
/// The `observation-authority` stream this task tails in `ddb_streams` mode.
pub const OBSERVATION_TABLE_STREAM_ARN: &str = "AEX_OBSERVATION_TABLE_STREAM_ARN";
/// The endpoint-specific HTTPS origin of the `DynamoDB` Streams interface endpoint.
pub const DYNAMODB_STREAMS_ENDPOINT_URL: &str = "AEX_DYNAMODB_STREAMS_ENDPOINT_URL";
/// Secondary-index settle window used to pin an honest observation snapshot.
pub const OBS_INDEX_SETTLE_MS: &str = "AEX_OBS_INDEX_SETTLE_MS";
/// Maximum authority items scanned for one emitted page.
pub const OBS_QUERY_SCANNED_ITEMS: &str = "AEX_OBS_QUERY_SCANNED_ITEMS";
/// Maximum authority segments visited for one emitted page.
pub const OBS_QUERY_SEGMENTS: &str = "AEX_OBS_QUERY_SEGMENTS";
/// Maximum authority bytes read for one emitted page.
pub const OBS_QUERY_READ_BYTES: &str = "AEX_OBS_QUERY_READ_BYTES";
/// How the tail phase is woken: `ddb_streams` or `poll`.
pub const STREAM_WAKE_MODE: &str = "AEX_STREAM_WAKE_MODE";
/// How many tasks this service runs, which bounds shard readers.
pub const STREAM_MAX_TASKS: &str = "AEX_STREAM_MAX_TASKS";
/// Total socket ceiling per task.
pub const STREAM_MAX_CONNECTIONS: &str = "AEX_STREAM_MAX_CONNECTIONS";
/// Session-class socket ceiling per task.
pub const STREAM_MAX_CONNECTIONS_SESSION: &str = "AEX_STREAM_MAX_CONNECTIONS_SESSION";
/// Observation-class socket ceiling per task.
pub const STREAM_MAX_CONNECTIONS_OBSERVATION: &str = "AEX_STREAM_MAX_CONNECTIONS_OBSERVATION";
/// Per-workspace socket ceiling.
pub const STREAM_MAX_CONNECTIONS_PER_WORKSPACE: &str = "AEX_STREAM_MAX_CONNECTIONS_PER_WORKSPACE";
/// Per-connection outbound buffer budget in bytes.
pub const STREAM_CONNECTION_BUFFER_BYTES: &str = "AEX_STREAM_CONNECTION_BUFFER_BYTES";
/// Write stall deadline in milliseconds.
pub const STREAM_WRITE_STALL_MS: &str = "AEX_STREAM_WRITE_STALL_MS";

/// Every variable a healthy `session-stream-api` requires in `poll` mode.
pub const REQUIRED: [&str; 48] = [
    PLANE,
    REGION,
    RELEASE_DIGEST,
    PORT,
    DRAIN_DEADLINE_MS,
    CREDENTIAL_PEPPER_REF,
    AUTHZ_PROJECTION_TABLE,
    SESSION_TABLE,
    CONTENT_BUCKET,
    CURSOR_SIGNING_KEY_REF,
    REGIONAL_API_URL,
    WORK_TABLE,
    SESSION_OPERATION_WORKER_FUNCTION_ARN,
    CONTENT_TABLE,
    REGISTRY_TABLE,
    SECRET_CUSTODY_TABLE,
    SECRET_KEYSTORE_TABLE,
    SECRET_KMS_KEY_ARN,
    BRANCH_KEY_CACHE_BYTES,
    BRANCH_KEY_CACHE_TTL_MS,
    RUNTIME_ACTIVITY_TABLE,
    USAGE_COMPUTE_QUEUE_URL,
    USAGE_STORAGE_QUEUE_URL,
    RUNTIME_DUE_SHARDS,
    RUNTIME_DUE_PAGE_ITEMS,
    RUNTIME_DUE_PAGE_READS,
    PRICING_VERSION,
    USAGE_QUERY_TABLE,
    CONTENT_BUCKET_OWNER,
    HANDS_IMAGE_CATALOG,
    HANDS_PUBLIC_INTERNET_EGRESS,
    CONTENT_KMS_KEY_ARN,
    MAX_JSON_BODY_BYTES,
    MAX_PAGE_ITEMS,
    MAX_PAGE_BYTES,
    OBSERVATION_TABLE,
    OBS_INDEX_SETTLE_MS,
    OBS_QUERY_SCANNED_ITEMS,
    OBS_QUERY_SEGMENTS,
    OBS_QUERY_READ_BYTES,
    STREAM_WAKE_MODE,
    STREAM_MAX_TASKS,
    STREAM_MAX_CONNECTIONS,
    STREAM_MAX_CONNECTIONS_SESSION,
    STREAM_MAX_CONNECTIONS_OBSERVATION,
    STREAM_MAX_CONNECTIONS_PER_WORKSPACE,
    STREAM_CONNECTION_BUFFER_BYTES,
    STREAM_WRITE_STALL_MS,
];

/// Variables this binary must never be bound to, with the reason.
///
/// The binary publishes no queue message: after committing durable work it
/// asynchronously invokes the exact operation-worker Lambda alias. The worker
/// owns its queue, retries and deletion fan-out; binding that queue here would
/// give the public edge a second dispatch path.
///
/// `AEX_WORK_TABLE` is deliberately absent: the session half requires it. The
/// guarantee it used to carry for the stream half is now [`crate::capability`]'s
/// job, where the compiler enforces it instead of the environment.
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

/// How the tail phase is woken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeMode {
    /// One shared reader per task over the two authority table streams.
    DdbStreams,
    /// Adaptive polling of the authority.
    Poll,
}

impl WakeMode {
    /// The closed wake vocabulary.
    pub const ALL: [&'static str; 2] = ["ddb_streams", "poll"];

    /// The stable spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DdbStreams => "ddb_streams",
            Self::Poll => "poll",
        }
    }
}

/// The AWS guidance this service respects: at most two readers per shard.
pub const MAX_STREAM_READER_TASKS: u64 = 2;

/// The number of edges this process builds.
///
/// One per audience:
/// [`aex_internal_contracts::assertion::AssertionAudience::RegionalSession`] and
/// [`aex_internal_contracts::assertion::AssertionAudience::RegionalStream`].
/// They no longer cost memory to hold — there is nothing cached between requests
/// — but they remain two distinct trust boundaries sharing one process, which is
/// why each is bound to exactly one audience the key row must name.
pub const EDGE_COUNT: usize = 2;

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
    /// Exact qualified session-operation-worker Lambda ARN.
    pub session_operation_worker: Arn,
    /// `regional-content` table.
    pub content_table: String,
    /// `regional-registry` table.
    pub registry_table: String,
    /// `regional-secret-custody` table.
    pub secret_custody_table: String,
    /// `regional-secret-keystore` table.
    pub secret_keystore_table: String,
    /// Secret KMS key used only by provider-credential registration.
    pub secret_kms_key: Arn,
    /// Branch-key cache byte budget.
    pub branch_key_cache_bytes: usize,
    /// Branch-key cache lifetime in milliseconds.
    pub branch_key_cache_ttl_ms: u64,
    /// `runtime-activity` table.
    pub runtime_activity_table: String,
    /// Compute usage ingress used by same-generation resume.
    pub usage_compute_queue_url: String,
    /// Storage usage ingress used by same-generation resume.
    pub usage_storage_queue_url: String,
    /// Runtime-control due-index shard count.
    pub runtime_due_shards: u16,
    /// Runtime-control due scan budget.
    pub runtime_due_page: aex_runtime_control::store::PageBudget,
    /// Exact usage pricing version.
    pub pricing_version: String,
    /// `usage-query-projection` table.
    pub usage_query_table: String,
    /// `observation-authority` table.
    pub observation_table: String,
    /// `session-authority` stream, in `ddb_streams` mode.
    pub session_stream: Option<Arn>,
    /// `observation-authority` stream, in `ddb_streams` mode.
    pub observation_stream: Option<Arn>,
    /// Endpoint-specific `DynamoDB` Streams origin, in `ddb_streams` mode.
    pub dynamodb_streams_endpoint_url: Option<String>,
    /// Regional content bucket.
    pub content_bucket: String,
    /// Expected content-bucket owner account.
    pub content_bucket_owner: String,
    /// Verified immutable image and deployment capability facts.
    pub deployment: aex_session_app::DeploymentFacts,
    /// Content KMS key.
    pub content_kms_key: Arn,
    /// Cursor signing key reference.
    pub cursor_signing_key_ref: String,
    /// Conservative GSI settle window used by snapshot pinning.
    pub observation_index_settle_ms: i64,
    /// Per-page authority read budget.
    pub observation_budget: aex_observation_query::plan::Budget,
    /// How the tail phase is woken.
    pub wake_mode: WakeMode,
    /// How many tasks this service runs.
    pub max_tasks: u64,
    /// Total socket ceiling per task.
    pub max_connections: u64,
    /// Session-class socket ceiling.
    pub max_connections_session: u64,
    /// Observation-class socket ceiling.
    pub max_connections_observation: u64,
    /// Per-workspace socket ceiling.
    pub max_connections_per_workspace: u64,
    /// Per-connection outbound buffer budget.
    pub connection_buffer_bytes: usize,
    /// Write stall deadline in milliseconds.
    pub write_stall_ms: u64,
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
        let secret_kms_key = arn_in_region(lookup, SECRET_KMS_KEY_ARN, region, "kms")?;
        let session_operation_worker = arn_in_region(
            lookup,
            SESSION_OPERATION_WORKER_FUNCTION_ARN,
            region,
            "lambda",
        )?;
        let expected_worker = format!(
            "function:aex-{}-session-operation-worker:live",
            plane.as_str()
        );
        if session_operation_worker.resource != expected_worker {
            return Err(RegionalHttpConfigError::Invalid {
                name: SESSION_OPERATION_WORKER_FUNCTION_ARN,
                reason: format!(
                    "expected exact live alias `{expected_worker}`, got `{}`",
                    session_operation_worker.resource
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
        if session_operation_worker.account != content_bucket_owner {
            return Err(RegionalHttpConfigError::Invalid {
                name: SESSION_OPERATION_WORKER_FUNCTION_ARN,
                reason: "worker alias and regional resources must share one account".to_owned(),
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
        let usage_compute_queue_url = required(lookup, USAGE_COMPUTE_QUEUE_URL)?;
        let usage_storage_queue_url = required(lookup, USAGE_STORAGE_QUEUE_URL)?;
        let queue_prefix = format!("https://sqs.{}.amazonaws.com/", region.as_str());
        for (name, value) in [
            (USAGE_COMPUTE_QUEUE_URL, &usage_compute_queue_url),
            (USAGE_STORAGE_QUEUE_URL, &usage_storage_queue_url),
        ] {
            if !value.starts_with(&queue_prefix) {
                return Err(RegionalHttpConfigError::Invalid {
                    name,
                    reason: format!("must be an SQS queue URL in `{}`", region.as_str()),
                });
            }
        }
        if usage_compute_queue_url == usage_storage_queue_url {
            return Err(RegionalHttpConfigError::Invalid {
                name: USAGE_STORAGE_QUEUE_URL,
                reason: "compute and storage usage authorities cannot share one queue".to_owned(),
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

        let raw_wake = one_of(lookup, STREAM_WAKE_MODE, &WakeMode::ALL)?;
        let wake_mode = if raw_wake == "ddb_streams" {
            WakeMode::DdbStreams
        } else {
            WakeMode::Poll
        };
        let max_tasks = bounded_u64(lookup, STREAM_MAX_TASKS, 1, 1_000)?;
        // A DynamoDB shard tolerates two readers before it starts throttling the
        // ones that were already there, so a third task is refused rather than
        // discovered as intermittent stream lag.
        if wake_mode == WakeMode::DdbStreams && max_tasks > MAX_STREAM_READER_TASKS {
            return Err(RegionalHttpConfigError::Invalid {
                name: STREAM_MAX_TASKS,
                reason: format!(
                    "`ddb_streams` admits at most {MAX_STREAM_READER_TASKS} tasks per shard, got `{max_tasks}`"
                ),
            });
        }
        let (session_stream, observation_stream, dynamodb_streams_endpoint_url) =
            if wake_mode == WakeMode::DdbStreams {
                let endpoint = required(lookup, DYNAMODB_STREAMS_ENDPOINT_URL)?;
                let Some(host) = endpoint.strip_prefix("https://") else {
                    return Err(RegionalHttpConfigError::Invalid {
                        name: DYNAMODB_STREAMS_ENDPOINT_URL,
                        reason: "must be the HTTPS origin of the private DynamoDB Streams endpoint"
                            .to_owned(),
                    });
                };
                if host.is_empty()
                    || host.contains('/')
                    || !host.contains(".dynamodb")
                    || !host.ends_with(".vpce.amazonaws.com")
                {
                    return Err(RegionalHttpConfigError::Invalid {
                        name: DYNAMODB_STREAMS_ENDPOINT_URL,
                        reason:
                            "must be a bare DynamoDB endpoint-specific `.vpce.amazonaws.com` origin"
                                .to_owned(),
                    });
                }
                (
                    Some(arn_in_region(
                        lookup,
                        SESSION_TABLE_STREAM_ARN,
                        region,
                        "dynamodb",
                    )?),
                    Some(arn_in_region(
                        lookup,
                        OBSERVATION_TABLE_STREAM_ARN,
                        region,
                        "dynamodb",
                    )?),
                    Some(endpoint),
                )
            } else {
                (None, None, None)
            };

        let raw_port = bounded_u64(lookup, PORT, 1, 65_535)?;
        let port = u16::try_from(raw_port).map_err(|_| RegionalHttpConfigError::Invalid {
            name: PORT,
            reason: format!("`{raw_port}` is not a TCP port"),
        })?;

        let max_connections = bounded_u64(lookup, STREAM_MAX_CONNECTIONS, 1, 100_000)?;
        let max_connections_session =
            bounded_u64(lookup, STREAM_MAX_CONNECTIONS_SESSION, 1, 100_000)?;
        let max_connections_observation =
            bounded_u64(lookup, STREAM_MAX_CONNECTIONS_OBSERVATION, 1, 100_000)?;
        // The unified `telemetry` signal is charged against both class budgets,
        // so a total below either class would make one budget unreachable.
        if max_connections < max_connections_session
            || max_connections < max_connections_observation
        {
            return Err(RegionalHttpConfigError::Invalid {
                name: STREAM_MAX_CONNECTIONS,
                reason: format!(
                    "total `{max_connections}` is below a class budget \
                     (session `{max_connections_session}`, observation `{max_connections_observation}`)"
                ),
            });
        }

        let drain_deadline_ms = bounded_u64(
            lookup,
            DRAIN_DEADLINE_MS,
            MIN_DRAIN_DEADLINE_MS,
            MAX_DRAIN_DEADLINE_MS,
        )?;
        let write_stall_ms = bounded_u64(lookup, STREAM_WRITE_STALL_MS, 100, 600_000)?;
        drain_fits(drain_deadline_ms, write_stall_ms)?;

        let observation_index_settle_ms = bounded_u64(
            lookup,
            OBS_INDEX_SETTLE_MS,
            u64::try_from(aex_observation_domain::limits::OBS_CLOCK_SKEW_MAX_MS + 1).unwrap_or(1),
            60_000,
        )?;
        let observation_budget = aex_observation_query::plan::Budget {
            max_returned: aex_observation_domain::limits::QUERY_MAX_LIMIT,
            max_items_scanned: u32::try_from(bounded_u64(
                lookup,
                OBS_QUERY_SCANNED_ITEMS,
                1,
                u64::from(aex_observation_domain::limits::QUERY_MAX_ITEMS_SCANNED),
            )?)
            .map_err(|_| RegionalHttpConfigError::Invalid {
                name: OBS_QUERY_SCANNED_ITEMS,
                reason: "the admitted scan budget does not fit `u32`".to_owned(),
            })?,
            max_segments: u16::try_from(bounded_u64(
                lookup,
                OBS_QUERY_SEGMENTS,
                1,
                u64::from(aex_observation_domain::limits::QUERY_MAX_SEGMENTS),
            )?)
            .map_err(|_| RegionalHttpConfigError::Invalid {
                name: OBS_QUERY_SEGMENTS,
                reason: "the admitted segment budget does not fit `u16`".to_owned(),
            })?,
            max_bytes_read: bounded_u64(
                lookup,
                OBS_QUERY_READ_BYTES,
                1,
                aex_observation_domain::limits::QUERY_MAX_BYTES_READ,
            )?,
        };

        Ok(Self {
            plane,
            region,
            regional_api_url,
            release_digest: required(lookup, RELEASE_DIGEST)?,
            port,
            drain_deadline_ms,
            credential_pepper_ref: required(lookup, CREDENTIAL_PEPPER_REF)?,
            authz_projection_table: required(lookup, AUTHZ_PROJECTION_TABLE)?,
            session_table: required(lookup, SESSION_TABLE)?,
            work_table: required(lookup, WORK_TABLE)?,
            session_operation_worker,
            content_table: required(lookup, CONTENT_TABLE)?,
            registry_table: required(lookup, REGISTRY_TABLE)?,
            secret_custody_table: required(lookup, SECRET_CUSTODY_TABLE)?,
            secret_keystore_table: required(lookup, SECRET_KEYSTORE_TABLE)?,
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
            usage_compute_queue_url,
            usage_storage_queue_url,
            runtime_due_shards,
            runtime_due_page: aex_runtime_control::store::PageBudget {
                max_items: u32::try_from(runtime_due_page_items).unwrap_or(u32::MAX),
                max_reads: u32::try_from(runtime_due_page_reads).unwrap_or(u32::MAX),
            },
            pricing_version,
            usage_query_table: required(lookup, USAGE_QUERY_TABLE)?,
            observation_table: required(lookup, OBSERVATION_TABLE)?,
            session_stream,
            observation_stream,
            dynamodb_streams_endpoint_url,
            content_bucket: required(lookup, CONTENT_BUCKET)?,
            content_bucket_owner,
            deployment,
            content_kms_key,
            cursor_signing_key_ref: required(lookup, CURSOR_SIGNING_KEY_REF)?,
            observation_index_settle_ms: i64::try_from(observation_index_settle_ms).map_err(
                |_| RegionalHttpConfigError::Invalid {
                    name: OBS_INDEX_SETTLE_MS,
                    reason: "the settle window does not fit `i64`".to_owned(),
                },
            )?,
            observation_budget,
            wake_mode,
            max_tasks,
            max_connections,
            max_connections_session,
            max_connections_observation,
            max_connections_per_workspace: bounded_u64(
                lookup,
                STREAM_MAX_CONNECTIONS_PER_WORKSPACE,
                1,
                100_000,
            )?,
            connection_buffer_bytes: bounded_usize(
                lookup,
                STREAM_CONNECTION_BUFFER_BYTES,
                4_096,
                64 * 1_024 * 1_024,
            )?,
            write_stall_ms,
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

/// Refuses a drain deadline a stream producer provably cannot meet.
///
/// The drain flag is only observed at the top of a producer loop whose wait runs
/// up to [`regional_observation_api::api::LISTEN_POLL_MAX`], after which the
/// producer may still block up to `AEX_STREAM_WRITE_STALL_MS` writing to a
/// wedged reader. A deadline no larger than the sum of those two is a deadline
/// the last socket cannot reach, and the merged process turns that overrun into
/// a failed exit that takes in-flight *session* requests with it.
///
/// Before the merge this was nobody's check: the stream sized its own deadline
/// and the session API asserted `< 30_000` once in a test fixture.
///
/// # Errors
///
/// Returns [`RegionalHttpConfigError::Invalid`] naming `AEX_STREAM_WRITE_STALL_MS`,
/// because the deadline is pinned by the stop timeout and the stall is the term
/// an operator can actually lower.
fn drain_fits(drain_deadline_ms: u64, write_stall_ms: u64) -> Result<(), RegionalHttpConfigError> {
    let poll_max_ms = u64::try_from(regional_observation_api::api::LISTEN_POLL_MAX.as_millis())
        .unwrap_or(u64::MAX);
    let worst_case_ms = poll_max_ms.saturating_add(write_stall_ms);
    if worst_case_ms >= drain_deadline_ms {
        return Err(RegionalHttpConfigError::Invalid {
            name: STREAM_WRITE_STALL_MS,
            reason: format!(
                "a producer needs up to {poll_max_ms} ms to observe the drain flag plus \
                 {write_stall_ms} ms to give up on a wedged reader, which does not fit \
                 inside the {drain_deadline_ms} ms drain deadline"
            ),
        });
    }
    Ok(())
}
