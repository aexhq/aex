//! Validated start-up configuration for `regional-otlp`.
//!
//! Nothing here has a default. A variable that identifies a table, a bucket, a
//! key or a region must be supplied explicitly, because a defaulted resource
//! identifier silently binds the process to the wrong plane. Every bound is
//! re-checked against the registered ceiling, so a deployment cannot raise the
//! encoded ceiling past the 4 MiB the registry pins.

use std::time::Duration;

use aex_otlp_admission::OtlpLimits;
use aex_wire::types::Region;

/// Environment variable naming the deployment plane.
pub const PLANE_VAR: &str = "AEX_PLANE";
/// Environment variable naming the bound `AWS` region.
pub const REGION_VAR: &str = "AEX_REGION";
/// Environment variable naming the observation-authority `DynamoDB` table.
pub const OBSERVATION_TABLE_VAR: &str = "AEX_OBSERVATION_TABLE";
/// Environment variable naming the regional observation `S3` bucket.
pub const OBSERVATION_BUCKET_VAR: &str = "AEX_OBSERVATION_BUCKET";
/// Environment variable naming the regional secret-custody `DynamoDB` table.
pub const SECRET_CUSTODY_TABLE_VAR: &str = "AEX_SECRET_CUSTODY_TABLE";
/// Environment variable naming the regional redaction key secret.
pub const REDACTION_KEY_REF_VAR: &str = "AEX_OBS_REDACTION_KEY_REF";
/// Environment variable naming the maximum encoded OTLP body in bytes.
pub const ENCODED_MAX_VAR: &str = "AEX_OTLP_ENCODED_MAX";
/// Environment variable naming the maximum decoded OTLP payload in bytes.
pub const DECODED_MAX_VAR: &str = "AEX_OTLP_DECODED_MAX";
/// Environment variable naming the maximum records in one OTLP request.
pub const MAX_RECORDS_VAR: &str = "AEX_OTLP_MAX_RECORDS";
/// Environment variable naming the process-wide decode-memory budget in bytes.
pub const MEMORY_BUDGET_VAR: &str = "AEX_OTLP_MEMORY_BUDGET_BYTES";
/// Environment variable naming how long a request waits for decode memory.
pub const RESERVE_WAIT_VAR: &str = "AEX_OTLP_RESERVE_WAIT_MS";
/// Environment variable naming the reserved concurrency this function runs at.
pub const RESERVED_CONCURRENCY_VAR: &str = "AEX_OTLP_RESERVED_CONCURRENCY";
/// Environment variable naming the `central-authz` function this edge invokes.
///
/// `central-authz` is IAM-invoked, not routed. There is no URL: a role without
/// `lambda:InvokeFunction` on this one ARN cannot resolve an assertion at all.
pub const AUTHZ_FUNCTION_ARN_VAR: &str = "AEX_AUTHZ_FUNCTION_ARN";
/// Environment variable naming the Parameter Store trust-anchor document.
///
/// The anchors are a `SecureString`-capable document read once at cold start,
/// not an inline environment value: rotating a signing key must not require a
/// redeploy of every regional service.
pub const AUTHZ_VERIFY_KEYS_PARAM_VAR: &str = "AEX_AUTHZ_VERIFY_KEYS_PARAM";
/// Environment variable naming the read-only `regional-authz-projection` table.
///
/// Read on **every** request and never cached. It is the only thing that makes a
/// 30-second assertion safe: a revoked key or a paused account takes effect
/// inside the window rather than after it.
pub const AUTHZ_PROJECTION_TABLE_VAR: &str = "AEX_AUTHZ_PROJECTION_TABLE";
/// Environment variable naming the assertion cache byte budget.
pub const ASSERTION_CACHE_BYTES_VAR: &str = "AEX_ASSERTION_CACHE_BYTES";

/// Every variable this deployable requires, in declaration order.
pub const REQUIRED_VARS: &[&str] = &[
    PLANE_VAR,
    REGION_VAR,
    OBSERVATION_TABLE_VAR,
    OBSERVATION_BUCKET_VAR,
    SECRET_CUSTODY_TABLE_VAR,
    REDACTION_KEY_REF_VAR,
    ENCODED_MAX_VAR,
    DECODED_MAX_VAR,
    MAX_RECORDS_VAR,
    MEMORY_BUDGET_VAR,
    RESERVE_WAIT_VAR,
    RESERVED_CONCURRENCY_VAR,
    AUTHZ_FUNCTION_ARN_VAR,
    AUTHZ_VERIFY_KEYS_PARAM_VAR,
    AUTHZ_PROJECTION_TABLE_VAR,
    ASSERTION_CACHE_BYTES_VAR,
];

/// Planes this deployable may be bound to.
pub const PLANES: [&str; 2] = ["dev", "prd"];

/// Why `regional-otlp` refused to start.
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

/// The validated configuration of one `regional-otlp` process.
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
    /// The regional secret-custody table holding redaction manifests.
    pub secret_custody_table: String,
    /// The secret id of the regional redaction key.
    pub redaction_key_ref: String,
    /// The effective admission limits.
    pub limits: OtlpLimits,
    /// The process-wide decode-memory budget, in bytes.
    pub memory_budget_bytes: usize,
    /// How long a request waits for decode memory before failing retryably.
    pub reserve_wait: Duration,
    /// The reserved concurrency this function is deployed at.
    pub reserved_concurrency: u32,
    /// The `central-authz` function this edge invokes for an assertion.
    pub authz_function_arn: String,
    /// The Parameter Store name holding the assertion trust anchors.
    pub authz_verify_keys_param: String,
    /// The read-only regional authorization projection table.
    pub authz_projection_table: String,
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
        let raw_plane = required(&lookup, PLANE_VAR)?;
        // The typed plane the assertion envelope binds, not a validated string:
        // the plane this process reports and the plane it will accept an
        // assertion for must be the same value.
        let plane = aex_identity_domain::assertion::Plane::parse(&raw_plane).ok_or_else(|| {
            ConfigError::Invalid {
                name: PLANE_VAR,
                reason: format!("expected one of {PLANES:?}, got `{raw_plane}`"),
            }
        })?;
        let raw_region = required(&lookup, REGION_VAR)?;
        let region = Region::from_name(&raw_region).ok_or_else(|| ConfigError::Invalid {
            name: REGION_VAR,
            reason: format!("`{raw_region}` is not a regional plane region"),
        })?;

        let registered = OtlpLimits::REGISTERED;
        let encoded_max = bounded(&lookup, ENCODED_MAX_VAR, registered.encoded_max)?;
        let decoded_max = bounded(&lookup, DECODED_MAX_VAR, registered.decoded_max)?;
        let max_records = bounded(&lookup, MAX_RECORDS_VAR, registered.max_records)?;
        let limits = OtlpLimits {
            encoded_max,
            decoded_max,
            max_records,
            ..registered
        };
        if encoded_max > decoded_max {
            return Err(ConfigError::Invalid {
                name: ENCODED_MAX_VAR,
                reason: format!(
                    "the encoded ceiling of {encoded_max} exceeds the decoded ceiling of \
                     {decoded_max}, so no admissible request could ever be decoded"
                ),
            });
        }

        let memory_budget_bytes = positive(&lookup, MEMORY_BUDGET_VAR)?;
        if memory_budget_bytes < decoded_max {
            return Err(ConfigError::Invalid {
                name: MEMORY_BUDGET_VAR,
                reason: format!(
                    "a budget of {memory_budget_bytes} bytes cannot cover one \
                     {decoded_max}-byte decode, so every request would fail closed"
                ),
            });
        }
        let reserve_wait_ms = positive(&lookup, RESERVE_WAIT_VAR)?;
        let reserved_concurrency = positive(&lookup, RESERVED_CONCURRENCY_VAR)?;

        Ok(Self {
            plane,
            region,
            observation_table: required(&lookup, OBSERVATION_TABLE_VAR)?,
            observation_bucket: required(&lookup, OBSERVATION_BUCKET_VAR)?,
            secret_custody_table: required(&lookup, SECRET_CUSTODY_TABLE_VAR)?,
            redaction_key_ref: required(&lookup, REDACTION_KEY_REF_VAR)?,
            limits,
            memory_budget_bytes,
            reserve_wait: Duration::from_millis(reserve_wait_ms as u64),
            reserved_concurrency: u32::try_from(reserved_concurrency).map_err(|_| {
                ConfigError::Invalid {
                    name: RESERVED_CONCURRENCY_VAR,
                    reason: "reserved concurrency does not fit a 32-bit count".to_owned(),
                }
            })?,
            authz_function_arn: required(&lookup, AUTHZ_FUNCTION_ARN_VAR)?,
            authz_verify_keys_param: required(&lookup, AUTHZ_VERIFY_KEYS_PARAM_VAR)?,
            authz_projection_table: required(&lookup, AUTHZ_PROJECTION_TABLE_VAR)?,
            assertion_cache_bytes: positive(&lookup, ASSERTION_CACHE_BYTES_VAR)?,
        })
    }

    /// The whole-region decode-memory ceiling this deployment implies.
    ///
    /// `reserved concurrency × decoded ceiling` is the number O-ROLES exists to
    /// bound: it is why OTLP decompression can never starve query or export.
    #[must_use]
    pub const fn regional_decode_ceiling_bytes(&self) -> u128 {
        self.reserved_concurrency as u128 * self.limits.decoded_max as u128
    }
}

impl Config {
    /// The effective body bound this deployable's edge enforces.
    ///
    /// Every route this binary owns carries an OTLP body, so the edge's single
    /// body bound is the OTLP encoded ceiling. A deployable that owned both
    /// classes would need the edge to take the class from the route table; this
    /// one does not, and pretending otherwise would add a knob nothing reads.
    ///
    /// It owns no listing, so the page bounds are zero: a handler that started
    /// paginating fails its own budget check rather than inheriting a number
    /// nobody chose (SD-08).
    #[must_use]
    pub const fn effective_limits(&self) -> aex_regional_http::context::EffectiveLimits {
        aex_regional_http::context::EffectiveLimits {
            json_body_bytes: self.limits.encoded_max,
            query_page_items: 0,
            query_page_bytes: 0,
        }
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
fn positive<F>(lookup: &F, name: &'static str) -> Result<usize, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let raw = required(lookup, name)?;
    let value = raw.parse::<usize>().map_err(|error| ConfigError::Invalid {
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
fn bounded<F>(lookup: &F, name: &'static str, ceiling: usize) -> Result<usize, ConfigError>
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        ASSERTION_CACHE_BYTES_VAR, AUTHZ_FUNCTION_ARN_VAR, AUTHZ_PROJECTION_TABLE_VAR,
        AUTHZ_VERIFY_KEYS_PARAM_VAR, Config, ConfigError, DECODED_MAX_VAR, ENCODED_MAX_VAR,
        MAX_RECORDS_VAR, MEMORY_BUDGET_VAR, OBSERVATION_BUCKET_VAR, OBSERVATION_TABLE_VAR,
        PLANE_VAR, REDACTION_KEY_REF_VAR, REGION_VAR, REQUIRED_VARS, RESERVE_WAIT_VAR,
        RESERVED_CONCURRENCY_VAR, SECRET_CUSTODY_TABLE_VAR,
    };

    fn complete() -> BTreeMap<&'static str, String> {
        BTreeMap::from([
            (PLANE_VAR, "dev".to_owned()),
            (REGION_VAR, "eu-west-1".to_owned()),
            (OBSERVATION_TABLE_VAR, "observation-authority".to_owned()),
            (OBSERVATION_BUCKET_VAR, "aex-dev-observations".to_owned()),
            (
                SECRET_CUSTODY_TABLE_VAR,
                "regional-secret-custody".to_owned(),
            ),
            (
                REDACTION_KEY_REF_VAR,
                "aex/dev/observation/redact".to_owned(),
            ),
            (ENCODED_MAX_VAR, (4 * 1024 * 1024).to_string()),
            (DECODED_MAX_VAR, (16 * 1024 * 1024).to_string()),
            (MAX_RECORDS_VAR, "2000".to_owned()),
            (MEMORY_BUDGET_VAR, (1_600 * 1024 * 1024).to_string()),
            (RESERVE_WAIT_VAR, "50".to_owned()),
            (RESERVED_CONCURRENCY_VAR, "20".to_owned()),
            (
                AUTHZ_FUNCTION_ARN_VAR,
                "arn:aws:lambda:eu-west-1:000000000000:function:central-authz".to_owned(),
            ),
            (
                AUTHZ_VERIFY_KEYS_PARAM_VAR,
                "/aex/dev/authz/verify-keys".to_owned(),
            ),
            (
                AUTHZ_PROJECTION_TABLE_VAR,
                "regional-authz-projection".to_owned(),
            ),
            (ASSERTION_CACHE_BYTES_VAR, (1024 * 1024).to_string()),
        ])
    }

    fn read(vars: &BTreeMap<&'static str, String>) -> Result<Config, ConfigError> {
        Config::from_lookup(|name| vars.get(name).cloned())
    }

    #[test]
    fn accepts_a_complete_environment() {
        let config = read(&complete()).expect("a complete environment starts");
        assert_eq!(config.plane, aex_identity_domain::assertion::Plane::Dev);
        assert_eq!(config.region.as_str(), "eu-west-1");
        assert_eq!(config.limits.encoded_max, 4 * 1024 * 1024);
        assert_eq!(config.limits.decoded_max, 16 * 1024 * 1024);
        assert_eq!(config.limits.max_records, 2_000);
        assert_eq!(config.authz_verify_keys_param, "/aex/dev/authz/verify-keys");
        assert_eq!(
            config.regional_decode_ceiling_bytes(),
            20 * 16 * 1024 * 1024
        );
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
    fn a_blank_resource_is_missing_rather_than_empty() {
        let mut vars = complete();
        vars.insert(OBSERVATION_TABLE_VAR, "   ".to_owned());
        assert_eq!(
            read(&vars),
            Err(ConfigError::Missing {
                name: OBSERVATION_TABLE_VAR
            })
        );
    }

    #[test]
    fn the_legacy_six_mebibyte_encoded_ceiling_is_refused() {
        let mut vars = complete();
        vars.insert(ENCODED_MAX_VAR, (6 * 1024 * 1024).to_string());
        let error = read(&vars).expect_err("6 MiB contradicts the registry");
        assert!(
            matches!(
                error,
                ConfigError::Invalid {
                    name: ENCODED_MAX_VAR,
                    ..
                }
            ),
            "{error:?}"
        );
    }

    #[test]
    fn a_record_ceiling_above_two_thousand_is_refused() {
        let mut vars = complete();
        vars.insert(MAX_RECORDS_VAR, "2001".to_owned());
        assert!(matches!(
            read(&vars),
            Err(ConfigError::Invalid {
                name: MAX_RECORDS_VAR,
                ..
            })
        ));
    }

    #[test]
    fn a_budget_that_cannot_cover_one_decode_is_refused() {
        let mut vars = complete();
        vars.insert(MEMORY_BUDGET_VAR, (1024 * 1024).to_string());
        assert!(matches!(
            read(&vars),
            Err(ConfigError::Invalid {
                name: MEMORY_BUDGET_VAR,
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

    #[test]
    fn this_deployable_names_the_authority_and_never_carries_its_key_material() {
        // The trust anchors are a Parameter Store document read once at cold
        // start, not an inline environment value: rotating a signing key must
        // not require redeploying every regional service, and a `SecureString`
        // is not something to paste into a task definition.
        let config = read(&complete()).expect("a complete environment starts");
        assert!(config.authz_verify_keys_param.starts_with('/'));
        assert!(!config.authz_verify_keys_param.contains(':'));
        // `central-authz` is IAM-invoked, so what is named is a function ARN.
        // There is no endpoint, no TLS decision and no second authentication hop.
        assert!(
            config.authz_function_arn.starts_with("arn:aws:lambda:"),
            "{}",
            config.authz_function_arn
        );
    }
}
