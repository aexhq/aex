//! `observation-export-launcher` composition root (Rust Lambda ZIP).
//!
//! Exclusive responsibility: the durable export claim and the idempotent ECS task launch.
//!
//! This binary is a composition root only. Configuration is validated before anything
//! starts, telemetry is installed through `aex_platform_telemetry`, and the behaviour
//! itself lives in the library crates this deployable composes.

/// Validated start-up configuration for `observation-export-launcher`.
///
/// Nothing here has a default. A variable that identifies a resource must be
/// supplied explicitly, because a defaulted resource identifier silently binds
/// the process to the wrong plane, region or table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Deployment plane this process belongs to (`dev` or `prd`).
    pub plane: String,
    /// `AWS` region this process is bound to.
    pub region: String,
    /// ECS task definition launched for an admitted export.
    pub resource: String,
    /// Maximum export task launches attempted per run.
    pub budget: u32,
}

/// Why `observation-export-launcher` refused to start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
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

/// Why `observation-export-launcher` stopped.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// Start-up configuration was rejected.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// The process holds a capability its role must not.
    #[error("this deployable must not hold the `{capability}` capability")]
    Capability {
        /// The offending capability.
        capability: &'static str,
    },
    /// A declared readiness probe has not passed.
    #[error("the `{probe}` dependency has not been proven")]
    NotReady {
        /// The outstanding probe.
        probe: &'static str,
    },
    /// The behaviour of this deployable has not been implemented yet.
    #[error("`observation-export-launcher` has no implementation yet")]
    NotImplemented,
}

/// Environment variable naming the deployment plane.
pub const PLANE_VAR: &str = "AEX_PLANE";
/// Environment variable naming the bound `AWS` region.
pub const REGION_VAR: &str = "AEX_REGION";
/// Environment variable naming ECS task definition launched for an admitted export.
pub const RESOURCE_VAR: &str = "AEX_EXPORT_TASK_DEFINITION_ARN";
/// Environment variable naming maximum export task launches attempted per run.
pub const BUDGET_VAR: &str = "AEX_MAX_LAUNCHES_PER_RUN";

/// Planes this deployable may be bound to.
const PLANES: [&str; 2] = ["dev", "prd"];

impl Config {
    /// Reads and validates the configuration of `observation-export-launcher` from the process environment.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Missing`] when a required variable is absent or
    /// empty, and [`ConfigError::Invalid`] when a variable is present but does
    /// not parse or is outside its permitted set.
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
        let region = required(&lookup, REGION_VAR)?;
        let resource = required(&lookup, RESOURCE_VAR)?;
        let raw_budget = required(&lookup, BUDGET_VAR)?;
        let budget = raw_budget
            .parse::<u32>()
            .map_err(|error| ConfigError::Invalid {
                name: BUDGET_VAR,
                reason: format!("expected a positive integer, got `{raw_budget}`: {error}"),
            })?;
        if budget == 0 {
            return Err(ConfigError::Invalid {
                name: BUDGET_VAR,
                reason: "expected a positive integer, got `0`".to_owned(),
            });
        }
        Ok(Self {
            plane,
            region,
            resource,
            budget,
        })
    }
}

fn required<F>(lookup: &F, name: &'static str) -> Result<String, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    match lookup(name) {
        Some(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(ConfigError::Missing { name }),
    }
}

/// Runs `observation-export-launcher` until it stops.
///
/// # Errors
///
/// Currently always returns [`RunError::NotImplemented`]: the owning
/// implementation stream lands the body on top of this composition root.
pub fn run(config: &Config, telemetry: &aex_platform_telemetry::Handle) -> Result<(), RunError> {
    telemetry.emit(
        aex_platform_telemetry::Record::event(
            aex_telemetry_schema::generated::EVENT_AEX_PROCESS_STARTED,
        )
        .with(
            aex_telemetry_schema::generated::AEX_PLANE,
            config.plane.clone(),
        )
        .with(
            aex_telemetry_schema::generated::AEX_REGION,
            config.region.clone(),
        ),
    );
    Err(RunError::NotImplemented)
}

fn main() -> std::process::ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("observation-export-launcher: refusing to start: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let settings = aex_platform_telemetry::Settings::default();
    let telemetry = aex_platform_telemetry::Handle::install(&settings, None);
    let outcome = run(&config, &telemetry);
    if let aex_platform_telemetry::FlushOutcome::DeadlineExceeded { pending } =
        telemetry.flush(settings.flush_deadline)
    {
        eprintln!(
            "observation-export-launcher: telemetry flush left {pending} record(s) undelivered"
        );
    }
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("observation-export-launcher: stopped: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BUDGET_VAR, Config, ConfigError, PLANE_VAR, REGION_VAR, RESOURCE_VAR};
    use std::collections::BTreeMap;

    fn complete() -> BTreeMap<&'static str, String> {
        BTreeMap::from([
            (PLANE_VAR, "dev".to_owned()),
            (REGION_VAR, "eu-west-1".to_owned()),
            (
                RESOURCE_VAR,
                "aex-observation_export_launcher-fixture".to_owned(),
            ),
            (BUDGET_VAR, "8".to_owned()),
        ])
    }

    fn read(vars: &BTreeMap<&'static str, String>) -> Result<Config, ConfigError> {
        Config::from_lookup(|name| vars.get(name).cloned())
    }

    #[test]
    fn accepts_a_complete_environment() {
        let config = read(&complete()).expect("complete environment is accepted");
        assert_eq!(config.plane, "dev");
        assert_eq!(config.region, "eu-west-1");
        assert_eq!(config.resource, "aex-observation_export_launcher-fixture");
        assert_eq!(config.budget, 8);
    }

    #[test]
    fn names_each_missing_variable() {
        for name in [PLANE_VAR, REGION_VAR, RESOURCE_VAR, BUDGET_VAR] {
            let mut vars = complete();
            vars.remove(name);
            assert_eq!(
                read(&vars),
                Err(ConfigError::Missing { name }),
                "removing {name}"
            );
        }
    }

    #[test]
    fn rejects_a_blank_variable_as_missing() {
        let mut vars = complete();
        vars.insert(RESOURCE_VAR, "   ".to_owned());
        assert_eq!(
            read(&vars),
            Err(ConfigError::Missing { name: RESOURCE_VAR })
        );
    }

    #[test]
    fn rejects_an_unknown_plane() {
        let mut vars = complete();
        vars.insert(PLANE_VAR, "staging".to_owned());
        let error = read(&vars).expect_err("an unknown plane is rejected");
        assert!(
            matches!(
                error,
                ConfigError::Invalid {
                    name: PLANE_VAR,
                    ..
                }
            ),
            "{error:?}"
        );
    }

    #[test]
    fn rejects_a_non_numeric_budget() {
        let mut vars = complete();
        vars.insert(BUDGET_VAR, "lots".to_owned());
        let error = read(&vars).expect_err("a non-numeric budget is rejected");
        assert!(
            matches!(
                error,
                ConfigError::Invalid {
                    name: BUDGET_VAR,
                    ..
                }
            ),
            "{error:?}"
        );
    }

    #[test]
    fn rejects_a_zero_budget() {
        let mut vars = complete();
        vars.insert(BUDGET_VAR, "0".to_owned());
        let error = read(&vars).expect_err("a zero budget is rejected");
        assert!(
            matches!(
                error,
                ConfigError::Invalid {
                    name: BUDGET_VAR,
                    ..
                }
            ),
            "{error:?}"
        );
    }
}

// --- composition ------------------------------------------------------------

/// The capability grant this deployable is allowed to hold.
///
/// Asserted at startup: a role that observes a capability outside its grant
/// refuses to start rather than running with more authority than it declared.
pub const ROLE: aex_observation_store_aws::composition::Role =
    aex_observation_store_aws::composition::Role::ExportLauncher;

/// The dependencies this deployable proves before it reports ready.
pub const REQUIRED_PROBES: &[aex_observation_store_aws::health::Probe] =
    &[aex_observation_store_aws::health::Probe::ExportCluster];

/// Asserts the observed capability grant and the readiness probe set.
///
/// # Errors
///
/// Returns [`RunError::Capability`] when the process holds a capability its role
/// must not, and [`RunError::NotReady`] when a declared probe has not passed. A
/// probe that has not passed is never assumed.
pub fn compose(
    observed: &[aex_observation_store_aws::composition::Capability],
    passed: &[aex_observation_store_aws::health::Probe],
) -> Result<(), RunError> {
    aex_observation_store_aws::composition::assert_grant(ROLE, observed).map_err(|violation| {
        RunError::Capability {
            capability: violation.capability.as_str(),
        }
    })?;
    match aex_observation_store_aws::health::readiness(REQUIRED_PROBES, passed) {
        aex_observation_store_aws::health::Readiness::Ready => Ok(()),
        aex_observation_store_aws::health::Readiness::NotReady { outstanding } => {
            Err(RunError::NotReady {
                probe: outstanding.as_str(),
            })
        }
    }
}

#[cfg(test)]
mod composition_tests {
    use super::{REQUIRED_PROBES, ROLE, RunError, compose};
    use aex_observation_store_aws::composition::Capability;

    #[test]
    fn its_own_grant_and_a_complete_probe_set_start() {
        compose(ROLE.granted(), REQUIRED_PROBES).expect("the declared composition starts");
    }

    #[test]
    fn a_capability_outside_the_grant_refuses_to_start() {
        for denied in ROLE.denied() {
            let error = compose(&[denied], REQUIRED_PROBES).expect_err("refused");
            assert!(matches!(error, RunError::Capability { .. }), "{error:?}");
        }
    }

    #[test]
    fn an_unproven_probe_is_never_assumed() {
        if let Some(first) = REQUIRED_PROBES.first() {
            let error = compose(ROLE.granted(), &[]).expect_err("refused");
            match error {
                RunError::NotReady { probe } => assert_eq!(probe, first.as_str()),
                other => panic!("expected a readiness failure, got {other:?}"),
            }
        }
        assert!(!ROLE.granted().is_empty());
        assert!(!Capability::ALL.is_empty());
    }
}
