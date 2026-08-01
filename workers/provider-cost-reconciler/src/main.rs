//! `provider-cost-reconciler` composition root (Rust Lambda ZIP).
//!
//! Exclusive responsibility: provider invoice and cost comparison plus margin alerts; never
//! a customer charge.
//!
//! This binary is a composition root only. Configuration is validated before anything
//! starts, telemetry is installed through `aex_platform_telemetry`, and the behaviour
//! itself lives in the library crates this deployable composes.

/// Validated start-up configuration for `provider-cost-reconciler`.
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
    /// S3 bucket holding signed provider cost inputs.
    pub resource: String,
    /// Maximum invoice rows normalized per run.
    pub budget: u32,
}

/// Why `provider-cost-reconciler` refused to start.
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

/// Why `provider-cost-reconciler` stopped.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// Start-up configuration was rejected.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// The behaviour of this deployable has not been implemented yet.
    #[error("`provider-cost-reconciler` has no implementation yet")]
    NotImplemented,
}

/// Environment variable naming the deployment plane.
pub const PLANE_VAR: &str = "AEX_PLANE";
/// Environment variable naming the bound `AWS` region.
pub const REGION_VAR: &str = "AEX_REGION";
/// Environment variable naming S3 bucket holding signed provider cost inputs.
pub const RESOURCE_VAR: &str = "AEX_PROVIDER_COST_BUCKET";
/// Environment variable naming maximum invoice rows normalized per run.
pub const BUDGET_VAR: &str = "AEX_MAX_ROWS_PER_RUN";

/// Planes this deployable may be bound to.
const PLANES: [&str; 2] = ["dev", "prd"];

impl Config {
    /// Reads and validates the configuration of `provider-cost-reconciler` from the process environment.
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

/// Runs `provider-cost-reconciler` until it stops.
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
            eprintln!("provider-cost-reconciler: refusing to start: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let settings = aex_platform_telemetry::Settings::default();
    let telemetry = aex_platform_telemetry::Handle::install(&settings, None);
    let outcome = run(&config, &telemetry);
    if let aex_platform_telemetry::FlushOutcome::DeadlineExceeded { pending } =
        telemetry.flush(settings.flush_deadline)
    {
        eprintln!("provider-cost-reconciler: telemetry flush left {pending} record(s) undelivered");
    }
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("provider-cost-reconciler: stopped: {error}");
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
                "aex-provider_cost_reconciler-fixture".to_owned(),
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
        assert_eq!(config.resource, "aex-provider_cost_reconciler-fixture");
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
