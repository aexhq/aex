//! Validated start-up configuration for `regional-stream`.
//!
//! The service owns no mutation of any kind, so its configuration binds no
//! queue, no work table and no write capability. Wake mode is explicit rather
//! than inferred, and `ddb_streams` asserts the two-readers-per-shard bound at
//! start-up (RS-05).

use aex_regional_http::config::{
    Arn, ConfigError, Lookup, arn_in_region, bounded_u64, bounded_usize, forbidden, one_of,
    plane_name, region, required,
};
use aex_wire::types::Region;

/// The deployable this configuration belongs to.
pub const DEPLOYABLE: &str = "regional-stream";

/// Deployment plane: `dev` or `prd`.
pub const PLANE: &str = "AEX_PLANE";
/// The region this process is pinned to.
pub const REGION: &str = "AEX_REGION";
/// The release digest reported by `/internal/readyz`.
pub const RELEASE_DIGEST: &str = "AEX_RELEASE_DIGEST";
/// The port the container listens on.
pub const STREAM_PORT: &str = "AEX_STREAM_PORT";
/// The `central-authz` function this edge resolves assertions through.
pub const AUTHZ_FUNCTION_ARN: &str = "AEX_AUTHZ_FUNCTION_ARN";
/// The parameter holding the assertion verification key set.
pub const AUTHZ_VERIFY_KEYS_PARAM: &str = "AEX_AUTHZ_VERIFY_KEYS_PARAM";
/// The regional authorization projection table.
pub const AUTHZ_PROJECTION_TABLE: &str = "AEX_AUTHZ_PROJECTION_TABLE";
/// The `session-authority` table.
pub const SESSION_TABLE: &str = "AEX_SESSION_TABLE";
/// The `observation-authority` table.
pub const OBSERVATION_TABLE: &str = "AEX_OBSERVATION_TABLE";
/// The `session-authority` stream this task tails in `ddb_streams` mode.
pub const SESSION_TABLE_STREAM_ARN: &str = "AEX_SESSION_TABLE_STREAM_ARN";
/// The `observation-authority` stream this task tails in `ddb_streams` mode.
pub const OBSERVATION_TABLE_STREAM_ARN: &str = "AEX_OBSERVATION_TABLE_STREAM_ARN";
/// The endpoint-specific HTTPS origin of the `DynamoDB` Streams interface endpoint.
pub const DYNAMODB_STREAMS_ENDPOINT_URL: &str = "AEX_DYNAMODB_STREAMS_ENDPOINT_URL";
/// The regional content bucket, read for large stored records.
pub const CONTENT_BUCKET: &str = "AEX_CONTENT_BUCKET";
/// The storage reference of the cursor signing key.
pub const CURSOR_SIGNING_KEY_REF: &str = "AEX_CURSOR_SIGNING_KEY_REF";
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
/// Drain deadline in milliseconds.
pub const STREAM_DRAIN_DEADLINE_MS: &str = "AEX_STREAM_DRAIN_DEADLINE_MS";
/// Assertion cache byte budget.
pub const ASSERTION_CACHE_BYTES: &str = "AEX_ASSERTION_CACHE_BYTES";

/// Every variable a healthy `regional-stream` requires in `poll` mode.
pub const REQUIRED: [&str; 25] = [
    PLANE,
    REGION,
    RELEASE_DIGEST,
    STREAM_PORT,
    AUTHZ_FUNCTION_ARN,
    AUTHZ_VERIFY_KEYS_PARAM,
    AUTHZ_PROJECTION_TABLE,
    SESSION_TABLE,
    OBSERVATION_TABLE,
    CONTENT_BUCKET,
    CURSOR_SIGNING_KEY_REF,
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
    STREAM_DRAIN_DEADLINE_MS,
    ASSERTION_CACHE_BYTES,
];

/// Variables this binary must never be bound to.
///
/// `regional-stream` owns no mutation: no queue, no work table, no secret key.
/// Binding one would mean somebody gave a read-only service a write path.
pub const FORBIDDEN: [(&str, &str); 4] = [
    ("AEX_WORK_TABLE", "the stream admits no durable work"),
    (
        "AEX_OPERATION_QUEUE_URL",
        "the stream publishes no queue message",
    ),
    (
        "AEX_CONTENT_QUEUE_URL",
        "the stream publishes no queue message",
    ),
    (
        "AEX_SECRET_KMS_KEY_ARN",
        "the stream never decrypts a secret",
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

/// Resolved configuration. Nothing here has a default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Deployment plane.
    pub plane: aex_identity_domain::assertion::Plane,
    /// Pinned region.
    pub region: Region,
    /// Release digest reported by readiness.
    pub release_digest: String,
    /// Listener port.
    pub port: u16,
    /// `central-authz` function.
    pub authz_function: Arn,
    /// Verification key-set parameter name.
    pub authz_verify_keys_param: String,
    /// Regional authorization projection table.
    pub authz_projection_table: String,
    /// `session-authority` table.
    pub session_table: String,
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
    /// Drain deadline in milliseconds.
    pub drain_deadline_ms: u64,
    /// Assertion cache byte budget.
    pub assertion_cache_bytes: usize,
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
    #[allow(clippy::too_many_lines, reason = "one arm per declared variable")]
    pub fn read<L: Lookup + ?Sized>(lookup: &L) -> Result<Self, ConfigError> {
        for (name, reason) in FORBIDDEN {
            forbidden(lookup, name, DEPLOYABLE, reason)?;
        }
        let plane = plane_name(lookup, PLANE)?;
        let region = region(lookup, REGION)?;
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
            return Err(ConfigError::Invalid {
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
                    return Err(ConfigError::Invalid {
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
                    return Err(ConfigError::Invalid {
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

        let raw_port = bounded_u64(lookup, STREAM_PORT, 1, 65_535)?;
        let port = u16::try_from(raw_port).map_err(|_| ConfigError::Invalid {
            name: STREAM_PORT,
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
            return Err(ConfigError::Invalid {
                name: STREAM_MAX_CONNECTIONS,
                reason: format!(
                    "total `{max_connections}` is below a class budget \
                     (session `{max_connections_session}`, observation `{max_connections_observation}`)"
                ),
            });
        }

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
            .map_err(|_| ConfigError::Invalid {
                name: OBS_QUERY_SCANNED_ITEMS,
                reason: "the admitted scan budget does not fit `u32`".to_owned(),
            })?,
            max_segments: u16::try_from(bounded_u64(
                lookup,
                OBS_QUERY_SEGMENTS,
                1,
                u64::from(aex_observation_domain::limits::QUERY_MAX_SEGMENTS),
            )?)
            .map_err(|_| ConfigError::Invalid {
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
            release_digest: required(lookup, RELEASE_DIGEST)?,
            port,
            authz_function: arn_in_region(lookup, AUTHZ_FUNCTION_ARN, region, "lambda")?,
            authz_verify_keys_param: required(lookup, AUTHZ_VERIFY_KEYS_PARAM)?,
            authz_projection_table: required(lookup, AUTHZ_PROJECTION_TABLE)?,
            session_table: required(lookup, SESSION_TABLE)?,
            observation_table: required(lookup, OBSERVATION_TABLE)?,
            session_stream,
            observation_stream,
            dynamodb_streams_endpoint_url,
            content_bucket: required(lookup, CONTENT_BUCKET)?,
            cursor_signing_key_ref: required(lookup, CURSOR_SIGNING_KEY_REF)?,
            observation_index_settle_ms: i64::try_from(observation_index_settle_ms).map_err(
                |_| ConfigError::Invalid {
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
            write_stall_ms: bounded_u64(lookup, STREAM_WRITE_STALL_MS, 100, 600_000)?,
            drain_deadline_ms: bounded_u64(lookup, STREAM_DRAIN_DEADLINE_MS, 1_000, 600_000)?,
            assertion_cache_bytes: bounded_usize(
                lookup,
                ASSERTION_CACHE_BYTES,
                1_024,
                64 * 1_024 * 1_024,
            )?,
        })
    }
}
