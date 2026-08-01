//! Validated start-up configuration for `regional-observation-api`.
//!
//! Nothing here has a default. A variable that identifies a table, a bucket, a
//! key or a region must be supplied explicitly, because a defaulted resource
//! identifier silently binds the process to the wrong plane. Every query budget
//! is re-checked against the registered ceiling, so a deployment can lower a
//! bound but never raise it past the registry.

use aex_observation_domain::limits;
use aex_observation_query::plan::Budget;
use aex_wire::types::Region;

/// Environment variable naming the deployment plane.
pub const PLANE_VAR: &str = "AEX_PLANE";
/// Environment variable naming the bound `AWS` region.
pub const REGION_VAR: &str = "AEX_REGION";
/// Environment variable naming the observation-authority `DynamoDB` table.
pub const OBSERVATION_TABLE_VAR: &str = "AEX_OBSERVATION_TABLE";
/// Environment variable naming the regional observation `S3` bucket.
pub const OBSERVATION_BUCKET_VAR: &str = "AEX_OBSERVATION_BUCKET";
/// Environment variable naming the read-only `session-authority` table.
pub const SESSION_TABLE_VAR: &str = "AEX_SESSION_TABLE";
/// Environment variable naming the secondary-index settle window.
pub const INDEX_SETTLE_VAR: &str = "AEX_OBS_INDEX_SETTLE_MS";
/// Environment variable naming the per-page scanned-item budget.
pub const QUERY_SCANNED_ITEMS_VAR: &str = "AEX_OBS_QUERY_SCANNED_ITEMS";
/// Environment variable naming the per-page segment budget.
pub const QUERY_SEGMENTS_VAR: &str = "AEX_OBS_QUERY_SEGMENTS";
/// Environment variable naming the per-page read-byte budget.
pub const QUERY_READ_BYTES_VAR: &str = "AEX_OBS_QUERY_READ_BYTES";
/// Environment variable naming the metric-aggregation scan budget.
pub const METRIC_AGGREGATE_SCAN_VAR: &str = "AEX_OBS_METRIC_AGGREGATE_SCAN";
/// Environment variable naming the export ECS cluster the admission records.
pub const EXPORT_CLUSTER_VAR: &str = "AEX_EXPORT_CLUSTER";
/// Environment variable naming the cursor signing key.
pub const CURSOR_KEY_VAR: &str = "AEX_OBS_CURSOR_KEY";
/// Environment variable naming the central authorization endpoint.
pub const CENTRAL_AUTHZ_URL_VAR: &str = "AEX_CENTRAL_AUTHZ_URL";
/// Environment variable naming the assertion trust anchors.
pub const TRUST_ANCHORS_VAR: &str = "AEX_ASSERTION_TRUST_ANCHORS";
/// Environment variable naming the assertion cache byte budget.
pub const ASSERTION_CACHE_BYTES_VAR: &str = "AEX_ASSERTION_CACHE_BYTES";

/// Every variable this deployable requires, in declaration order.
pub const REQUIRED_VARS: &[&str] = &[
    PLANE_VAR,
    REGION_VAR,
    OBSERVATION_TABLE_VAR,
    OBSERVATION_BUCKET_VAR,
    SESSION_TABLE_VAR,
    INDEX_SETTLE_VAR,
    QUERY_SCANNED_ITEMS_VAR,
    QUERY_SEGMENTS_VAR,
    QUERY_READ_BYTES_VAR,
    METRIC_AGGREGATE_SCAN_VAR,
    EXPORT_CLUSTER_VAR,
    CURSOR_KEY_VAR,
    CENTRAL_AUTHZ_URL_VAR,
    TRUST_ANCHORS_VAR,
    ASSERTION_CACHE_BYTES_VAR,
];

/// Planes this deployable may be bound to.
pub const PLANES: [&str; 2] = ["dev", "prd"];

/// Why `regional-observation-api` refused to start.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ConfigError {
    /// A required variable was absent or empty.
    #[error("required environment variable `{name}` is missing")]
    Missing {
        /// The variable that must be supplied.
        name: &'static str,
    },
    /// A required variable was present but unusable.
    #[error("environment variable `{name}` is invalid: {reason}")]
    Invalid {
        /// The variable that was rejected.
        name: &'static str,
        /// Why the supplied value was rejected.
        reason: String,
    },
}

/// The validated configuration of one `regional-observation-api` process.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    /// Deployment plane this process belongs to.
    pub plane: String,
    /// Region this process is bound to.
    pub region: Region,
    /// The observation-authority table.
    pub observation_table: String,
    /// The regional observation bucket.
    pub observation_bucket: String,
    /// The `session-authority` table, read-only, for the `events` signal.
    pub session_table: String,
    /// The secondary-index settle window, in milliseconds.
    pub index_settle_ms: i64,
    /// The per-page budgets.
    pub budget: Budget,
    /// The metric-aggregation scan budget.
    pub metric_aggregate_scan: u64,
    /// The ECS cluster an admitted export names.
    pub export_cluster: String,
    /// The cursor signing key.
    pub cursor_key: Vec<u8>,
    /// The central authorization endpoint that issues assertions.
    pub central_authz_url: String,
    /// The assertion trust anchors, as `key id` to raw Ed25519 public key.
    pub trust_anchors: Vec<(String, Vec<u8>)>,
    /// The assertion cache byte budget.
    pub assertion_cache_bytes: usize,
}

impl Config {
    /// Reads and validates the configuration from the process environment.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Missing`] when a required variable is absent or
    /// empty, and [`ConfigError::Invalid`] when a variable is present but does
    /// not parse or is outside its permitted range.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Reads and validates the configuration from an arbitrary lookup.
    ///
    /// Tests use this directly: `std::env::set_var` is `unsafe` in edition 2024
    /// and this workspace forbids `unsafe` code.
    ///
    /// # Errors
    ///
    /// Identical to [`Config::from_env`].
    pub fn from_lookup<F>(lookup: F) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let plane = required(&lookup, PLANE_VAR)?;
        if !PLANES.contains(&plane.as_str()) {
            return Err(ConfigError::Invalid {
                name: PLANE_VAR,
                reason: format!("expected one of {PLANES:?}, got `{plane}`"),
            });
        }
        let raw_region = required(&lookup, REGION_VAR)?;
        let region = Region::from_name(&raw_region).ok_or_else(|| ConfigError::Invalid {
            name: REGION_VAR,
            reason: format!("`{raw_region}` is not a regional plane region"),
        })?;

        // The settle window must dominate both index propagation and the
        // admission clock-skew bound, or the pinned snapshot stops being
        // complete by construction.
        let index_settle_ms =
            i64::try_from(positive(&lookup, INDEX_SETTLE_VAR)?).map_err(|_| {
                ConfigError::Invalid {
                    name: INDEX_SETTLE_VAR,
                    reason: "the settle window does not fit a 64-bit millisecond count".to_owned(),
                }
            })?;
        if index_settle_ms <= limits::OBS_CLOCK_SKEW_MAX_MS {
            return Err(ConfigError::Invalid {
                name: INDEX_SETTLE_VAR,
                reason: format!(
                    "a settle window of {index_settle_ms} ms does not dominate the \
                     {} ms admission clock-skew bound",
                    limits::OBS_CLOCK_SKEW_MAX_MS
                ),
            });
        }

        let budget = Budget {
            max_returned: limits::QUERY_MAX_LIMIT,
            max_items_scanned: u32::try_from(bounded(
                &lookup,
                QUERY_SCANNED_ITEMS_VAR,
                u64::from(limits::QUERY_MAX_ITEMS_SCANNED),
            )?)
            .unwrap_or(limits::QUERY_MAX_ITEMS_SCANNED),
            max_segments: u16::try_from(bounded(
                &lookup,
                QUERY_SEGMENTS_VAR,
                u64::from(limits::QUERY_MAX_SEGMENTS),
            )?)
            .unwrap_or(limits::QUERY_MAX_SEGMENTS),
            max_bytes_read: bounded(&lookup, QUERY_READ_BYTES_VAR, limits::QUERY_MAX_BYTES_READ)?,
        };

        Ok(Self {
            plane,
            region,
            observation_table: required(&lookup, OBSERVATION_TABLE_VAR)?,
            observation_bucket: required(&lookup, OBSERVATION_BUCKET_VAR)?,
            session_table: required(&lookup, SESSION_TABLE_VAR)?,
            index_settle_ms,
            budget,
            metric_aggregate_scan: bounded(
                &lookup,
                METRIC_AGGREGATE_SCAN_VAR,
                limits::METRIC_AGGREGATE_SCAN,
            )?,
            export_cluster: required(&lookup, EXPORT_CLUSTER_VAR)?,
            cursor_key: key(&lookup, CURSOR_KEY_VAR)?,
            central_authz_url: url(&lookup, CENTRAL_AUTHZ_URL_VAR)?,
            trust_anchors: trust_anchors(&lookup, TRUST_ANCHORS_VAR)?,
            assertion_cache_bytes: usize::try_from(positive(&lookup, ASSERTION_CACHE_BYTES_VAR)?)
                .unwrap_or(usize::MAX),
        })
    }
}

/// Reads a required, non-blank variable.
fn required<F>(lookup: &F, name: &'static str) -> Result<String, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    match lookup(name) {
        Some(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(ConfigError::Missing { name }),
    }
}

/// Reads a required, strictly positive count.
fn positive<F>(lookup: &F, name: &'static str) -> Result<u64, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let raw = required(lookup, name)?;
    let value = raw.parse::<u64>().map_err(|error| ConfigError::Invalid {
        name,
        reason: format!("expected a positive integer, got `{raw}`: {error}"),
    })?;
    if value == 0 {
        return Err(ConfigError::Invalid {
            name,
            reason: "expected a positive integer, got `0`".to_owned(),
        });
    }
    Ok(value)
}

/// Reads a positive count that may not exceed the registered ceiling.
fn bounded<F>(lookup: &F, name: &'static str, ceiling: u64) -> Result<u64, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let value = positive(lookup, name)?;
    if value > ceiling {
        return Err(ConfigError::Invalid {
            name,
            reason: format!(
                "{value} exceeds the registered ceiling of {ceiling}; the registry is the \
                 authority and a deployment may only lower it"
            ),
        });
    }
    Ok(value)
}

/// Reads a required `https` endpoint.
fn url<F>(lookup: &F, name: &'static str) -> Result<String, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let value = required(lookup, name)?;
    if !value.starts_with("https://") {
        return Err(ConfigError::Invalid {
            name,
            reason: format!("expected an `https://` endpoint, got `{value}`"),
        });
    }
    Ok(value)
}

/// Reads a base64 symmetric key of at least 32 bytes.
fn key<F>(lookup: &F, name: &'static str) -> Result<Vec<u8>, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    use base64::Engine as _;

    let raw = required(lookup, name)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(raw.trim())
        .map_err(|error| ConfigError::Invalid {
            name,
            reason: format!("expected base64 key material: {error}"),
        })?;
    if bytes.len() < aex_regional_http::cursor::MIN_KEY_BYTES {
        return Err(ConfigError::Invalid {
            name,
            reason: format!(
                "a signing key must be at least {} bytes",
                aex_regional_http::cursor::MIN_KEY_BYTES
            ),
        });
    }
    Ok(bytes)
}

/// Reads the assertion trust anchors.
///
/// The spelling is `<key id>:<base64 raw Ed25519 public key>`, comma separated.
/// A malformed anchor fails start-up: an edge that cannot verify an assertion
/// must never start, because it would answer every request `401` instead.
fn trust_anchors<F>(lookup: &F, name: &'static str) -> Result<Vec<(String, Vec<u8>)>, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    use base64::Engine as _;

    let raw = required(lookup, name)?;
    let mut anchors = Vec::new();
    for entry in raw.split(',') {
        let entry = entry.trim();
        let Some((key_id, material)) = entry.split_once(':') else {
            return Err(ConfigError::Invalid {
                name,
                reason: format!("`{entry}` is not `<key id>:<base64 key>`"),
            });
        };
        if key_id.is_empty() || key_id.len() > 128 {
            return Err(ConfigError::Invalid {
                name,
                reason: format!("`{key_id}` is not a usable key id"),
            });
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(material)
            .map_err(|error| ConfigError::Invalid {
                name,
                reason: format!("key `{key_id}` is not base64: {error}"),
            })?;
        if bytes.len() != 32 {
            return Err(ConfigError::Invalid {
                name,
                reason: format!(
                    "key `{key_id}` is {} bytes; an Ed25519 public key is 32",
                    bytes.len()
                ),
            });
        }
        anchors.push((key_id.to_owned(), bytes));
    }
    if anchors.is_empty() {
        return Err(ConfigError::Invalid {
            name,
            reason: "at least one trust anchor is required".to_owned(),
        });
    }
    Ok(anchors)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use aex_observation_domain::limits;

    use super::{
        ASSERTION_CACHE_BYTES_VAR, CENTRAL_AUTHZ_URL_VAR, CURSOR_KEY_VAR, Config, ConfigError,
        EXPORT_CLUSTER_VAR, INDEX_SETTLE_VAR, METRIC_AGGREGATE_SCAN_VAR, OBSERVATION_BUCKET_VAR,
        OBSERVATION_TABLE_VAR, PLANE_VAR, QUERY_READ_BYTES_VAR, QUERY_SCANNED_ITEMS_VAR,
        QUERY_SEGMENTS_VAR, REGION_VAR, REQUIRED_VARS, SESSION_TABLE_VAR, TRUST_ANCHORS_VAR,
    };

    fn base64(bytes: &[u8]) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn complete() -> BTreeMap<&'static str, String> {
        BTreeMap::from([
            (PLANE_VAR, "dev".to_owned()),
            (REGION_VAR, "eu-west-1".to_owned()),
            (OBSERVATION_TABLE_VAR, "observation-authority".to_owned()),
            (OBSERVATION_BUCKET_VAR, "aex-dev-observations".to_owned()),
            (SESSION_TABLE_VAR, "session-authority".to_owned()),
            (INDEX_SETTLE_VAR, "2000".to_owned()),
            (QUERY_SCANNED_ITEMS_VAR, "50000".to_owned()),
            (QUERY_SEGMENTS_VAR, "64".to_owned()),
            (QUERY_READ_BYTES_VAR, (32 * 1024 * 1024).to_string()),
            (METRIC_AGGREGATE_SCAN_VAR, "2000000".to_owned()),
            (
                EXPORT_CLUSTER_VAR,
                "arn:aws:ecs:eu-west-1:000000000000:cluster/aex-dev-export".to_owned(),
            ),
            (CURSOR_KEY_VAR, base64(&[5u8; 32])),
            (CENTRAL_AUTHZ_URL_VAR, "https://authz.aex.dev".to_owned()),
            (TRUST_ANCHORS_VAR, format!("kid:{}", base64(&[7u8; 32]))),
            (ASSERTION_CACHE_BYTES_VAR, (1024 * 1024).to_string()),
        ])
    }

    fn read(vars: &BTreeMap<&'static str, String>) -> Result<Config, ConfigError> {
        Config::from_lookup(|name| vars.get(name).cloned())
    }

    #[test]
    fn accepts_a_complete_environment() {
        let config = read(&complete()).expect("a complete environment starts");
        assert_eq!(config.plane, "dev");
        assert_eq!(config.region.as_str(), "eu-west-1");
        assert_eq!(config.index_settle_ms, limits::OBS_INDEX_SETTLE_MS);
        assert_eq!(config.budget.max_segments, limits::QUERY_MAX_SEGMENTS);
        assert_eq!(config.cursor_key.len(), 32);
    }

    #[test]
    fn names_every_missing_variable() {
        for name in REQUIRED_VARS {
            let mut vars = complete();
            vars.remove(*name);
            assert_eq!(
                read(&vars),
                Err(ConfigError::Missing { name }),
                "removing {name}"
            );
        }
    }

    #[test]
    fn every_required_variable_is_declared() {
        let vars = complete();
        for name in REQUIRED_VARS {
            assert!(vars.contains_key(*name), "{name} has no fixture value");
        }
        assert_eq!(vars.len(), REQUIRED_VARS.len());
    }

    #[test]
    fn a_settle_window_inside_the_clock_skew_bound_is_refused() {
        // The window has to dominate skew, or everything at or below the pinned
        // snapshot stops being complete by construction.
        let mut vars = complete();
        vars.insert(INDEX_SETTLE_VAR, limits::OBS_CLOCK_SKEW_MAX_MS.to_string());
        assert!(matches!(
            read(&vars),
            Err(ConfigError::Invalid {
                name: INDEX_SETTLE_VAR,
                ..
            })
        ));
    }

    #[test]
    fn a_budget_above_the_registered_ceiling_is_refused() {
        for (name, over) in [
            (
                QUERY_SCANNED_ITEMS_VAR,
                u64::from(limits::QUERY_MAX_ITEMS_SCANNED) + 1,
            ),
            (
                QUERY_SEGMENTS_VAR,
                u64::from(limits::QUERY_MAX_SEGMENTS) + 1,
            ),
            (QUERY_READ_BYTES_VAR, limits::QUERY_MAX_BYTES_READ + 1),
            (METRIC_AGGREGATE_SCAN_VAR, limits::METRIC_AGGREGATE_SCAN + 1),
        ] {
            let mut vars = complete();
            vars.insert(name, over.to_string());
            let error = read(&vars).expect_err("a raised ceiling is refused");
            assert!(
                matches!(error, ConfigError::Invalid { name: named, .. } if named == name),
                "{error:?}"
            );
        }
    }

    #[test]
    fn a_short_cursor_key_refuses_to_start() {
        let mut vars = complete();
        vars.insert(CURSOR_KEY_VAR, base64(&[1u8; 8]));
        assert!(matches!(
            read(&vars),
            Err(ConfigError::Invalid {
                name: CURSOR_KEY_VAR,
                ..
            })
        ));
    }

    #[test]
    fn an_unknown_plane_and_region_are_both_refused() {
        let mut vars = complete();
        vars.insert(PLANE_VAR, "staging".to_owned());
        assert!(matches!(
            read(&vars),
            Err(ConfigError::Invalid {
                name: PLANE_VAR,
                ..
            })
        ));
        let mut vars = complete();
        vars.insert(REGION_VAR, "eu-central-9".to_owned());
        assert!(matches!(
            read(&vars),
            Err(ConfigError::Invalid {
                name: REGION_VAR,
                ..
            })
        ));
    }
}
