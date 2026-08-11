//! Validated start-up configuration for `observation-reconciler`.
//!
//! Nothing here has a default. A variable that identifies a table, a bucket, a
//! queue, a duty or a region must be supplied explicitly, because a defaulted
//! resource identifier silently binds the process to the wrong plane — and a
//! defaulted *duty* would silently give one deployment another deployment's
//! work under another deployment's capability grant.
//!
//! Every bound is re-checked against the registered ceiling in
//! [`aex_observation_domain::limits`], so a deployment may only lower it.

use aex_observation_domain::keys::ControlDomain;
use aex_observation_domain::limits;
use aex_wire::types::Region;

/// Environment variable naming the deployment plane.
pub const PLANE_VAR: &str = "AEX_PLANE";
/// Environment variable naming the bound `AWS` region.
pub const REGION_VAR: &str = "AEX_REGION";
/// Environment variable naming the observation-authority `DynamoDB` table.
pub const OBSERVATION_TABLE_VAR: &str = "AEX_OBSERVATION_TABLE";
/// Environment variable naming the regional session authority. The deletion
/// duty uses it to cancel an export's canonical operation atomically before
/// removing export payload.
pub const SESSION_TABLE_VAR: &str = "AEX_SESSION_TABLE";
/// Environment variable naming the regional observation `S3` bucket.
pub const OBSERVATION_BUCKET_VAR: &str = "AEX_OBSERVATION_BUCKET";
/// Environment variable naming the duty this deployment runs.
pub const DUTY_VAR: &str = "AEX_OBS_DUTY";
/// Environment variable naming the bounded due-scan page size.
pub const RECONCILE_PAGE_VAR: &str = "AEX_OBS_RECONCILE_PAGE";
/// Environment variable naming how many shards the duty's due index is spread
/// over.
pub const DUTY_SHARDS_VAR: &str = "AEX_OBS_DUTY_SHARDS";
/// Environment variable naming the poison-quarantine attempt threshold.
pub const MAX_ATTEMPTS_VAR: &str = "AEX_OBS_MAX_ATTEMPTS";
/// Environment variable naming the `SQS` queue the storage usage fact is
/// delivered to.
pub const USAGE_QUEUE_URL_VAR: &str = "AEX_USAGE_QUEUE_URL";

/// Every variable this deployable requires, in declaration order.
pub const REQUIRED_VARS: &[&str] = &[
    PLANE_VAR,
    REGION_VAR,
    OBSERVATION_TABLE_VAR,
    SESSION_TABLE_VAR,
    OBSERVATION_BUCKET_VAR,
    DUTY_VAR,
    RECONCILE_PAGE_VAR,
    DUTY_SHARDS_VAR,
    MAX_ATTEMPTS_VAR,
    USAGE_QUEUE_URL_VAR,
];

/// Planes this deployable may be bound to.
pub const PLANES: [&str; 2] = ["dev", "prd"];

/// The largest bounded due-scan page one invocation may read.
pub const PAGE_MAX: u32 = 1_000;

/// The largest number of shards one duty's due index may be spread over.
pub const SHARDS_MAX: u32 = 64;

/// The duty that belongs to another deployable.
///
/// `export.launch` holds `ecs:RunTask` and **no** observation read permission at
/// all, which is why it is a separate deployable rather than a duty of this one.
/// Accepting it here would silently widen this function's grant.
pub const FOREIGN_DUTY: ControlDomain = ControlDomain::ExportLaunch;

/// The deployable that owns [`FOREIGN_DUTY`].
pub const FOREIGN_DUTY_OWNER: &str = "observation-export-launcher";

/// Why `observation-reconciler` refused to start.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ObservationReconcilerConfigError {
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

/// The validated configuration of one `observation-reconciler` deployment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    /// Deployment plane this process belongs to.
    pub plane: String,
    /// Region this process is bound to.
    pub region: Region,
    /// The observation-authority table.
    pub observation_table: String,
    /// The regional session authority holding canonical export operations.
    pub session_table: String,
    /// The regional observation bucket.
    pub observation_bucket: String,
    /// The one duty this deployment runs.
    pub duty: ControlDomain,
    /// How many due items one scan of one shard reads.
    pub page: u16,
    /// How many shards the duty's due index is spread over.
    pub shards: u8,
    /// Attempts before an item is quarantined rather than retried.
    pub max_attempts: u32,
    /// The usage queue the `SPOOL#…/OUTBOX#` storage fact is delivered to.
    pub usage_queue_url: String,
}

impl Config {
    /// Reads and validates the configuration from the process environment.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationReconcilerConfigError::Missing`] when a required variable is absent or
    /// empty, and [`ObservationReconcilerConfigError::Invalid`] when a variable is present but does
    /// not parse or is outside its permitted range.
    pub fn from_env() -> Result<Self, ObservationReconcilerConfigError> {
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
    pub fn from_lookup<F>(lookup: F) -> Result<Self, ObservationReconcilerConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let plane = required(&lookup, PLANE_VAR)?;
        if !PLANES.contains(&plane.as_str()) {
            return Err(ObservationReconcilerConfigError::Invalid {
                name: PLANE_VAR,
                reason: format!("expected one of {PLANES:?}, got `{plane}`"),
            });
        }
        let raw_region = required(&lookup, REGION_VAR)?;
        let region = Region::from_name(&raw_region).ok_or_else(|| {
            ObservationReconcilerConfigError::Invalid {
                name: REGION_VAR,
                reason: format!("`{raw_region}` is not a regional plane region"),
            }
        })?;
        let observation_table = required(&lookup, OBSERVATION_TABLE_VAR)?;
        let session_table = required(&lookup, SESSION_TABLE_VAR)?;
        let observation_bucket = required(&lookup, OBSERVATION_BUCKET_VAR)?;
        let duty = duty(&lookup)?;
        let page = bounded(&lookup, RECONCILE_PAGE_VAR, 1, PAGE_MAX)?;
        let shards = bounded(&lookup, DUTY_SHARDS_VAR, 1, SHARDS_MAX)?;
        let max_attempts = bounded(&lookup, MAX_ATTEMPTS_VAR, 1, limits::OBS_SPOOL_MAX_ATTEMPTS)?;
        let usage_queue_url = url(&lookup, USAGE_QUEUE_URL_VAR)?;

        Ok(Self {
            plane,
            region,
            observation_table,
            session_table,
            observation_bucket,
            duty,
            page: narrow(page, RECONCILE_PAGE_VAR)?,
            shards: narrow(shards, DUTY_SHARDS_VAR)?,
            max_attempts,
            usage_queue_url,
        })
    }
}

/// Reads the duty, refusing both an unknown word and the launcher's duty.
fn duty<F>(lookup: &F) -> Result<ControlDomain, ObservationReconcilerConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let raw = required(lookup, DUTY_VAR)?;
    let parsed = ControlDomain::parse(raw.trim()).ok_or_else(|| {
        ObservationReconcilerConfigError::Invalid {
            name: DUTY_VAR,
            reason: format!(
                "`{raw}` is not one of the {} control domains",
                ControlDomain::ALL.len()
            ),
        }
    })?;
    if parsed == FOREIGN_DUTY {
        return Err(ObservationReconcilerConfigError::Invalid {
            name: DUTY_VAR,
            reason: format!(
                "`{}` belongs to `{FOREIGN_DUTY_OWNER}`, which holds `ecs:RunTask` and no \
                 observation read permission at all; running it here would widen this \
                 deployment's capability grant",
                parsed.as_str()
            ),
        });
    }
    Ok(parsed)
}

/// Reads a required, non-blank variable.
fn required<F>(lookup: &F, name: &'static str) -> Result<String, ObservationReconcilerConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    match lookup(name) {
        Some(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(ObservationReconcilerConfigError::Missing { name }),
    }
}

/// Reads a count inside an inclusive range.
fn bounded<F>(
    lookup: &F,
    name: &'static str,
    low: u32,
    high: u32,
) -> Result<u32, ObservationReconcilerConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let raw = required(lookup, name)?;
    let value =
        raw.trim()
            .parse::<u32>()
            .map_err(|error| ObservationReconcilerConfigError::Invalid {
                name,
                reason: format!("expected an integer in {low}..={high}, got `{raw}`: {error}"),
            })?;
    if value < low || value > high {
        return Err(ObservationReconcilerConfigError::Invalid {
            name,
            reason: format!(
                "{value} is outside the registered range {low}..={high}; the registry is the \
                 authority and a deployment may only stay inside it"
            ),
        });
    }
    Ok(value)
}

/// Narrows an already range-checked count to the width it is stored in.
fn narrow<T>(value: u32, name: &'static str) -> Result<T, ObservationReconcilerConfigError>
where
    T: TryFrom<u32>,
{
    T::try_from(value).map_err(|_| ObservationReconcilerConfigError::Invalid {
        name,
        reason: format!("{value} does not fit the width this bound is stored in"),
    })
}

/// Reads a required `https` endpoint.
fn url<F>(lookup: &F, name: &'static str) -> Result<String, ObservationReconcilerConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let value = required(lookup, name)?;
    if !value.starts_with("https://") {
        return Err(ObservationReconcilerConfigError::Invalid {
            name,
            reason: format!("expected an `https://` endpoint, got `{value}`"),
        });
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use aex_observation_domain::keys::ControlDomain;
    use aex_observation_domain::limits;
    use aex_wire::types::Region;

    use super::{
        Config, DUTY_SHARDS_VAR, DUTY_VAR, FOREIGN_DUTY, MAX_ATTEMPTS_VAR, OBSERVATION_BUCKET_VAR,
        OBSERVATION_TABLE_VAR, ObservationReconcilerConfigError, PAGE_MAX, PLANE_VAR,
        RECONCILE_PAGE_VAR, REGION_VAR, REQUIRED_VARS, SESSION_TABLE_VAR, SHARDS_MAX,
        USAGE_QUEUE_URL_VAR,
    };

    fn complete() -> BTreeMap<&'static str, String> {
        BTreeMap::from([
            (PLANE_VAR, "prd".to_owned()),
            (REGION_VAR, "eu-west-1".to_owned()),
            (OBSERVATION_TABLE_VAR, "observation-authority".to_owned()),
            (SESSION_TABLE_VAR, "session-authority".to_owned()),
            (OBSERVATION_BUCKET_VAR, "aex-prd-observations".to_owned()),
            (DUTY_VAR, "gate.evaluate".to_owned()),
            (RECONCILE_PAGE_VAR, "250".to_owned()),
            (DUTY_SHARDS_VAR, "16".to_owned()),
            (MAX_ATTEMPTS_VAR, "8".to_owned()),
            (
                USAGE_QUEUE_URL_VAR,
                "https://sqs.eu-west-1.amazonaws.com/1/aex-prd-usage.fifo".to_owned(),
            ),
        ])
    }

    fn read(
        vars: &BTreeMap<&'static str, String>,
    ) -> Result<Config, ObservationReconcilerConfigError> {
        Config::from_lookup(|name| vars.get(name).cloned())
    }

    #[test]
    fn accepts_a_complete_environment() {
        let config = read(&complete()).expect("a complete environment starts");
        assert_eq!(config.plane, "prd");
        assert_eq!(config.region, Region::EuWest1);
        assert_eq!(config.session_table, "session-authority");
        assert_eq!(config.duty, ControlDomain::GateEvaluate);
        assert_eq!(config.page, 250);
        assert_eq!(config.shards, 16);
        assert_eq!(config.max_attempts, 8);
        assert!(config.usage_queue_url.starts_with("https://"));
    }

    #[test]
    fn names_every_missing_variable() {
        for name in REQUIRED_VARS {
            let mut vars = complete();
            vars.remove(*name);
            assert_eq!(
                read(&vars),
                Err(ObservationReconcilerConfigError::Missing { name }),
                "removing {name}"
            );
        }
    }

    #[test]
    fn the_bounds_are_the_registered_ones_and_admit_their_own_extremes() {
        for (name, low, high) in [
            (RECONCILE_PAGE_VAR, 1, PAGE_MAX),
            (DUTY_SHARDS_VAR, 1, SHARDS_MAX),
            (MAX_ATTEMPTS_VAR, 1, limits::OBS_SPOOL_MAX_ATTEMPTS),
        ] {
            for value in [low, high] {
                let mut vars = complete();
                vars.insert(name, value.to_string());
                read(&vars).unwrap_or_else(|error| panic!("{name} = {value}: {error}"));
            }
            for value in [low - 1, high + 1] {
                let mut vars = complete();
                vars.insert(name, value.to_string());
                let error = read(&vars).expect_err("outside the range");
                assert!(
                    matches!(error, ObservationReconcilerConfigError::Invalid { name: got, .. } if got == name),
                    "{name} = {value}: {error:?}"
                );
            }
        }
    }

    #[test]
    fn the_launcher_duty_names_the_deployable_that_owns_it() {
        let mut vars = complete();
        vars.insert(DUTY_VAR, FOREIGN_DUTY.as_str().to_owned());
        let error = read(&vars).expect_err("the launcher's duty is refused");
        match error {
            ObservationReconcilerConfigError::Invalid { name, reason } => {
                assert_eq!(name, DUTY_VAR);
                assert!(reason.contains("observation-export-launcher"), "{reason}");
            }
            other @ ObservationReconcilerConfigError::Missing { .. } => {
                panic!("expected an invalid duty, got {other:?}")
            }
        }
    }

    #[test]
    fn a_non_numeric_bound_names_its_own_variable() {
        let mut vars = complete();
        vars.insert(RECONCILE_PAGE_VAR, "lots".to_owned());
        assert!(matches!(
            read(&vars),
            Err(ObservationReconcilerConfigError::Invalid {
                name: RECONCILE_PAGE_VAR,
                ..
            })
        ));
    }
}
