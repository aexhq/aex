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
/// Environment variable naming the Parameter Store cursor signing key ring.
pub const CURSOR_KEY_REF_VAR: &str = "AEX_CURSOR_SIGNING_KEY_REF";
/// Environment variable naming the Secrets Manager credential pepper ring.
///
/// It names the id the issuing authority also reads —
/// `aex/<plane>/central/token-pepper` — and not a regional copy of it. One
/// stored document with two readers cannot drift; two copies can, and a
/// rotation that reached only one of them would leave keys minted under the new
/// version verifying centrally and failing here, silently and only for some
/// keys.
///
/// The peppers are a document read once at cold start, not an inline
/// environment value: the material never enters the environment, the task
/// definition or Terraform state, and a verifier names the version it was
/// computed under so a whole ring has to be resolvable at once. Held for the
/// process lifetime and never re-read, so a version added afterwards is
/// invisible here until this process restarts.
///
/// This replaced `AEX_AUTHZ_FUNCTION_ARN` and `AEX_AUTHZ_VERIFY_KEYS_PARAM`
/// together: there is no `central-authz` invoke to address and no assertion
/// signature to verify, because a presented key is checked in-process against
/// the verifier the control plane replicated onto its authorization row.
pub const CREDENTIAL_PEPPER_REF_VAR: &str = "AEX_CREDENTIAL_PEPPER_REF";
/// Environment variable naming the read-only `regional-authz-projection` table.
///
/// Read on **every** request and never cached. It carries the whole
/// authorization answer — the verifier, the pepper version, the audiences, the
/// scopes, the placement and the revocation floors — so a revoked key or a
/// paused account takes effect on the next request rather than after a window.
pub const AUTHZ_PROJECTION_TABLE_VAR: &str = "AEX_AUTHZ_PROJECTION_TABLE";

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
    CURSOR_KEY_REF_VAR,
    CREDENTIAL_PEPPER_REF_VAR,
    AUTHZ_PROJECTION_TABLE_VAR,
];

/// Planes this deployable may be bound to.
pub const PLANES: [&str; 2] = ["dev", "prd"];

/// Why `regional-observation-api` refused to start.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RegionalObservationApiConfigError {
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
    pub plane: aex_identity_domain::assertion::Plane,
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
    /// The Parameter Store reference for the cursor signing key ring.
    pub cursor_key_ref: String,
    /// The Secrets Manager id holding the credential pepper ring.
    pub credential_pepper_ref: String,
    /// The read-only regional authorization projection table.
    pub authz_projection_table: String,
}

impl Config {
    /// Reads and validates the configuration from the process environment.
    ///
    /// # Errors
    ///
    /// Returns [`RegionalObservationApiConfigError::Missing`] when a required variable is absent or
    /// empty, and [`RegionalObservationApiConfigError::Invalid`] when a variable is present but does
    /// not parse or is outside its permitted range.
    pub fn from_env() -> Result<Self, RegionalObservationApiConfigError> {
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
    pub fn from_lookup<F>(lookup: F) -> Result<Self, RegionalObservationApiConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let raw_plane = required(&lookup, PLANE_VAR)?;
        // The typed plane the assertion envelope binds, not a validated string:
        // the plane this process reports and the plane it will accept an
        // assertion for must be the same value.
        let plane = aex_identity_domain::assertion::Plane::parse(&raw_plane).ok_or_else(|| {
            RegionalObservationApiConfigError::Invalid {
                name: PLANE_VAR,
                reason: format!("expected one of {PLANES:?}, got `{raw_plane}`"),
            }
        })?;
        let raw_region = required(&lookup, REGION_VAR)?;
        let region = Region::from_name(&raw_region).ok_or_else(|| {
            RegionalObservationApiConfigError::Invalid {
                name: REGION_VAR,
                reason: format!("`{raw_region}` is not a regional plane region"),
            }
        })?;

        // The settle window must dominate both index propagation and the
        // admission clock-skew bound, or the pinned snapshot stops being
        // complete by construction.
        let index_settle_ms =
            i64::try_from(positive(&lookup, INDEX_SETTLE_VAR)?).map_err(|_| {
                RegionalObservationApiConfigError::Invalid {
                    name: INDEX_SETTLE_VAR,
                    reason: "the settle window does not fit a 64-bit millisecond count".to_owned(),
                }
            })?;
        if index_settle_ms <= limits::OBS_CLOCK_SKEW_MAX_MS {
            return Err(RegionalObservationApiConfigError::Invalid {
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
            cursor_key_ref: required(&lookup, CURSOR_KEY_REF_VAR)?,
            credential_pepper_ref: required(&lookup, CREDENTIAL_PEPPER_REF_VAR)?,
            authz_projection_table: required(&lookup, AUTHZ_PROJECTION_TABLE_VAR)?,
        })
    }
}

impl Config {
    /// The effective body and page bounds this deployable's edge enforces.
    ///
    /// The edge applies the body bound before anything parses a body. This
    /// deployable owns no OTLP route, so its bound is the AEX JSON one.
    ///
    /// The page bounds are its own query budget rather than a second pair of
    /// numbers: every listing here is planned and paged by
    /// `aex-observation-query` against exactly these, so a shared paginator that
    /// disagreed with the planner would be the drift this projection removes.
    #[must_use]
    pub fn effective_limits(&self) -> aex_regional_http::context::EffectiveLimits {
        aex_regional_http::context::EffectiveLimits {
            json_body_bytes: aex_wire::dispatch::RequestLimits::DEFAULT_JSON_BODY_BYTES,
            otlp_body_bytes: aex_wire::dispatch::RequestLimits::DEFAULT_OTLP_BODY_BYTES,
            query_page_items: self.budget.max_returned as usize,
            query_page_bytes: usize::try_from(self.budget.max_bytes_read).unwrap_or(usize::MAX),
        }
    }
}

/// Reads a required, non-blank variable.
fn required<F>(lookup: &F, name: &'static str) -> Result<String, RegionalObservationApiConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    match lookup(name) {
        Some(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(RegionalObservationApiConfigError::Missing { name }),
    }
}

/// Reads a required, strictly positive count.
fn positive<F>(lookup: &F, name: &'static str) -> Result<u64, RegionalObservationApiConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let raw = required(lookup, name)?;
    let value = raw
        .parse::<u64>()
        .map_err(|error| RegionalObservationApiConfigError::Invalid {
            name,
            reason: format!("expected a positive integer, got `{raw}`: {error}"),
        })?;
    if value == 0 {
        return Err(RegionalObservationApiConfigError::Invalid {
            name,
            reason: "expected a positive integer, got `0`".to_owned(),
        });
    }
    Ok(value)
}

/// Reads a positive count that may not exceed the registered ceiling.
fn bounded<F>(
    lookup: &F,
    name: &'static str,
    ceiling: u64,
) -> Result<u64, RegionalObservationApiConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let value = positive(lookup, name)?;
    if value > ceiling {
        return Err(RegionalObservationApiConfigError::Invalid {
            name,
            reason: format!(
                "{value} exceeds the registered ceiling of {ceiling}; the registry is the \
                 authority and a deployment may only lower it"
            ),
        });
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use aex_observation_domain::limits;

    use super::{
        AUTHZ_PROJECTION_TABLE_VAR, CREDENTIAL_PEPPER_REF_VAR, CURSOR_KEY_REF_VAR, Config,
        INDEX_SETTLE_VAR, METRIC_AGGREGATE_SCAN_VAR, OBSERVATION_BUCKET_VAR, OBSERVATION_TABLE_VAR,
        PLANE_VAR, QUERY_READ_BYTES_VAR, QUERY_SCANNED_ITEMS_VAR, QUERY_SEGMENTS_VAR, REGION_VAR,
        REQUIRED_VARS, RegionalObservationApiConfigError, SESSION_TABLE_VAR,
    };

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
                CURSOR_KEY_REF_VAR,
                "/aex/dev/regional/cursor-signing-key".to_owned(),
            ),
            (
                CREDENTIAL_PEPPER_REF_VAR,
                "aex/dev/central/token-pepper".to_owned(),
            ),
            (
                AUTHZ_PROJECTION_TABLE_VAR,
                "regional-authz-projection".to_owned(),
            ),
        ])
    }

    fn read(
        vars: &BTreeMap<&'static str, String>,
    ) -> Result<Config, RegionalObservationApiConfigError> {
        Config::from_lookup(|name| vars.get(name).cloned())
    }

    #[test]
    fn accepts_a_complete_environment() {
        let config = read(&complete()).expect("a complete environment starts");
        assert_eq!(config.plane, aex_identity_domain::assertion::Plane::Dev);
        assert_eq!(config.region.as_str(), "eu-west-1");
        assert_eq!(config.index_settle_ms, limits::OBS_INDEX_SETTLE_MS);
        assert_eq!(config.budget.max_segments, limits::QUERY_MAX_SEGMENTS);
        assert_eq!(
            config.cursor_key_ref,
            "/aex/dev/regional/cursor-signing-key"
        );
    }

    #[test]
    fn names_every_missing_variable() {
        for name in REQUIRED_VARS {
            let mut vars = complete();
            vars.remove(*name);
            assert_eq!(
                read(&vars),
                Err(RegionalObservationApiConfigError::Missing { name }),
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
            Err(RegionalObservationApiConfigError::Invalid {
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
                matches!(error, RegionalObservationApiConfigError::Invalid { name: named, .. } if named == name),
                "{error:?}"
            );
        }
    }

    #[test]
    fn an_unknown_plane_and_region_are_both_refused() {
        let mut vars = complete();
        vars.insert(PLANE_VAR, "staging".to_owned());
        assert!(matches!(
            read(&vars),
            Err(RegionalObservationApiConfigError::Invalid {
                name: PLANE_VAR,
                ..
            })
        ));
        let mut vars = complete();
        vars.insert(REGION_VAR, "eu-central-9".to_owned());
        assert!(matches!(
            read(&vars),
            Err(RegionalObservationApiConfigError::Invalid {
                name: REGION_VAR,
                ..
            })
        ));
    }
}
