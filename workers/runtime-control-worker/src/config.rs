//! Start-up configuration.
//!
//! Nothing here has a default. A variable that identifies a table, a queue, a
//! provider endpoint or a region must be supplied explicitly, because a defaulted
//! resource identifier silently binds the process to the wrong plane, region or
//! table — and for this worker "the wrong table" means suspending another plane's
//! generations.
//!
//! What is deliberately **not** configurable: the 180000 ms true-idle threshold and
//! the eight-hour lifetime margins. They are pinned constants in
//! `aex-runtime-control`. HR-21 puts jitter on the evaluation *schedule* and never
//! on the decision, and a deployment-tunable threshold would make the boundary a
//! property of an environment variable rather than of the model.

use aex_runtime_control::store::PageBudget;
use aex_wire::types::Region;

/// Environment variable naming the deployment plane.
pub const PLANE_VAR: &str = "AEX_PLANE";
/// Environment variable naming the bound `AWS` region.
pub const REGION_VAR: &str = "AEX_REGION";
/// Environment variable naming the runtime-activity `DynamoDB` table.
pub const RUNTIME_ACTIVITY_TABLE_VAR: &str = "AEX_RUNTIME_ACTIVITY_TABLE";
/// Environment variable naming the session-authority table the recount reads.
pub const SESSION_AUTHORITY_TABLE_VAR: &str = "AEX_SESSION_AUTHORITY_TABLE";
/// Environment variable naming the lifecycle queue this worker consumes.
pub const LIFECYCLE_QUEUE_VAR: &str = "AEX_RUNTIME_LIFECYCLE_QUEUE_URL";
/// Environment variable naming the compute-authority usage ingress.
pub const COMPUTE_QUEUE_VAR: &str = "AEX_USAGE_COMPUTE_QUEUE_URL";
/// Environment variable naming the storage-authority usage ingress.
pub const STORAGE_QUEUE_VAR: &str = "AEX_USAGE_STORAGE_QUEUE_URL";
/// Environment variable naming the `MicroVM` control-plane endpoint.
pub const PROVIDER_ENDPOINT_VAR: &str = "AEX_MICROVM_CONTROL_ENDPOINT";
/// Environment variable naming the published Hands image the orphan sweep scopes to.
pub const IMAGE_IDENTIFIER_VAR: &str = "AEX_HANDS_IMAGE_IDENTIFIER";
/// Environment variable naming how many shards the due index is spread over.
pub const DUE_SHARDS_VAR: &str = "AEX_RUNTIME_DUE_SHARDS";
/// Environment variable naming how many items one due scan may return.
pub const DUE_PAGE_ITEMS_VAR: &str = "AEX_RUNTIME_DUE_PAGE_ITEMS";
/// Environment variable naming how many provider-side reads one due scan may spend.
pub const DUE_PAGE_READS_VAR: &str = "AEX_RUNTIME_DUE_PAGE_READS";
/// Environment variable naming the rate book facts are priced against.
pub const PRICING_VERSION_VAR: &str = "AEX_PRICING_VERSION";

/// Every variable this worker requires, in the order it validates them.
pub const REQUIRED_VARS: [&str; 13] = [
    PLANE_VAR,
    REGION_VAR,
    RUNTIME_ACTIVITY_TABLE_VAR,
    SESSION_AUTHORITY_TABLE_VAR,
    LIFECYCLE_QUEUE_VAR,
    COMPUTE_QUEUE_VAR,
    STORAGE_QUEUE_VAR,
    PROVIDER_ENDPOINT_VAR,
    IMAGE_IDENTIFIER_VAR,
    DUE_SHARDS_VAR,
    DUE_PAGE_ITEMS_VAR,
    DUE_PAGE_READS_VAR,
    PRICING_VERSION_VAR,
];

/// Variables whose presence is itself a configuration error.
///
/// `AEX_USAGE_TRANSFER_QUEUE_URL`: Hands Internet egress is not charged at launch
/// (OD-26) and snapshot I/O is zero-dollar observability (OD-25), so this worker
/// holds no transfer-authority binding at all. Refusing to start when one is
/// supplied is stronger than accepting it and not using it — the second form
/// leaves a live queue URL in the environment for a later change to pick up.
///
/// `AEX_TRUE_IDLE_MILLIS`: the threshold is a pinned constant. Accepting the
/// variable and ignoring it would let an operator believe they had tuned a
/// boundary that never moved.
pub const FORBIDDEN_VARS: [&str; 2] = ["AEX_USAGE_TRANSFER_QUEUE_URL", "AEX_TRUE_IDLE_MILLIS"];

/// Planes this deployable may be bound to.
const PLANES: [&str; 2] = ["dev", "prd"];

/// Why `runtime-control-worker` refused to start.
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
    /// A variable that must never be supplied was supplied.
    #[error("environment variable `{name}` must not be set: {reason}")]
    Forbidden {
        /// The variable that was supplied.
        name: &'static str,
        /// Why it may not be.
        reason: &'static str,
    },
}

/// Validated start-up configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Deployment plane this process belongs to.
    pub plane: String,
    /// The region every resource must live in and every fact is attributed to.
    pub region: Region,
    /// The runtime-activity `DynamoDB` table.
    pub runtime_activity_table: String,
    /// The session-authority table the authoritative recount reads.
    pub session_authority_table: String,
    /// The lifecycle queue this worker consumes.
    pub lifecycle_queue_url: String,
    /// The compute-authority usage ingress.
    pub compute_queue_url: String,
    /// The storage-authority usage ingress.
    pub storage_queue_url: String,
    /// The `MicroVM` control-plane endpoint.
    pub provider_endpoint: String,
    /// The published Hands image the orphan sweep scopes to.
    pub image_identifier: String,
    /// How many shards the due index is spread over.
    pub due_shards: u16,
    /// How much one due scan may read.
    pub page: PageBudget,
    /// The rate book facts are priced against.
    pub pricing_version: String,
}

impl Config {
    /// Reads and validates the configuration from the process environment.
    ///
    /// # Errors
    ///
    /// See [`ConfigError`].
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Reads and validates the configuration from an arbitrary lookup.
    ///
    /// Tests use this directly: `std::env::set_var` is `unsafe` in edition 2024 and
    /// this workspace forbids `unsafe` code.
    ///
    /// # Errors
    ///
    /// See [`ConfigError`].
    pub fn from_lookup<F>(lookup: F) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        for name in FORBIDDEN_VARS {
            if lookup(name).is_some() {
                return Err(ConfigError::Forbidden {
                    name,
                    reason: forbidden_reason(name),
                });
            }
        }
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
            reason: format!("`{raw_region}` is not one of the five offered regions"),
        })?;
        let config = Self {
            plane,
            region,
            runtime_activity_table: required(&lookup, RUNTIME_ACTIVITY_TABLE_VAR)?,
            session_authority_table: required(&lookup, SESSION_AUTHORITY_TABLE_VAR)?,
            lifecycle_queue_url: endpoint(&lookup, LIFECYCLE_QUEUE_VAR, region)?,
            compute_queue_url: endpoint(&lookup, COMPUTE_QUEUE_VAR, region)?,
            storage_queue_url: endpoint(&lookup, STORAGE_QUEUE_VAR, region)?,
            provider_endpoint: endpoint(&lookup, PROVIDER_ENDPOINT_VAR, region)?,
            image_identifier: required(&lookup, IMAGE_IDENTIFIER_VAR)?,
            due_shards: positive(&lookup, DUE_SHARDS_VAR)?,
            page: PageBudget {
                max_items: positive(&lookup, DUE_PAGE_ITEMS_VAR)?,
                max_reads: positive(&lookup, DUE_PAGE_READS_VAR)?,
            },
            pricing_version: required(&lookup, PRICING_VERSION_VAR)?,
        };
        if config.compute_queue_url == config.storage_queue_url {
            return Err(ConfigError::Invalid {
                name: STORAGE_QUEUE_VAR,
                reason: "the compute and storage ingresses are two authorities and cannot be one \
                         queue"
                    .to_owned(),
            });
        }
        Ok(config)
    }
}

/// Why a forbidden variable is forbidden.
const fn forbidden_reason(name: &str) -> &'static str {
    if name.as_bytes()[4] == b'U' {
        "Hands egress is not charged at launch and snapshot I/O is zero-dollar \
         observability, so this worker holds no transfer-authority binding"
    } else {
        "the 180000 ms true-idle threshold is a pinned constant, not a deployment setting"
    }
}

/// A required, non-blank value.
fn required<F>(lookup: &F, name: &'static str) -> Result<String, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    match lookup(name) {
        Some(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(ConfigError::Missing { name }),
    }
}

/// A required HTTPS endpoint that must live in the configured region.
///
/// A queue or control endpoint in another region is a cross-region write nobody
/// notices until the bill, so the region is checked here rather than assumed.
fn endpoint<F>(lookup: &F, name: &'static str, region: Region) -> Result<String, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let value = required(lookup, name)?;
    if !value.starts_with("https://") {
        return Err(ConfigError::Invalid {
            name,
            reason: format!("expected an https:// endpoint, got `{value}`"),
        });
    }
    if !value.contains(region.as_str()) {
        return Err(ConfigError::Invalid {
            name,
            reason: format!("`{value}` is not in region `{}`", region.as_str()),
        });
    }
    Ok(value)
}

/// A required positive integer.
fn positive<F, T>(lookup: &F, name: &'static str) -> Result<T, ConfigError>
where
    F: Fn(&str) -> Option<String>,
    T: core::str::FromStr + PartialEq + Default,
    T::Err: core::fmt::Display,
{
    let raw = required(lookup, name)?;
    let value = raw.parse::<T>().map_err(|error| ConfigError::Invalid {
        name,
        reason: format!("expected a positive integer, got `{raw}`: {error}"),
    })?;
    if value == T::default() {
        return Err(ConfigError::Invalid {
            name,
            reason: "expected a positive integer, got `0`".to_owned(),
        });
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::{
        COMPUTE_QUEUE_VAR, Config, ConfigError, DUE_PAGE_ITEMS_VAR, DUE_SHARDS_VAR, FORBIDDEN_VARS,
        LIFECYCLE_QUEUE_VAR, PLANE_VAR, PROVIDER_ENDPOINT_VAR, REGION_VAR, REQUIRED_VARS,
        STORAGE_QUEUE_VAR,
    };
    use aex_wire::types::Region;
    use std::collections::BTreeMap;

    fn complete() -> BTreeMap<&'static str, String> {
        BTreeMap::from([
            (PLANE_VAR, "dev".to_owned()),
            (REGION_VAR, "eu-west-1".to_owned()),
            (
                super::RUNTIME_ACTIVITY_TABLE_VAR,
                "aex-dev-runtime-activity".to_owned(),
            ),
            (
                super::SESSION_AUTHORITY_TABLE_VAR,
                "aex-dev-session-authority".to_owned(),
            ),
            (
                LIFECYCLE_QUEUE_VAR,
                "https://sqs.eu-west-1.amazonaws.com/1/aex-dev-runtime-lifecycle".to_owned(),
            ),
            (
                COMPUTE_QUEUE_VAR,
                "https://sqs.eu-west-1.amazonaws.com/1/aex-dev-usage-compute".to_owned(),
            ),
            (
                STORAGE_QUEUE_VAR,
                "https://sqs.eu-west-1.amazonaws.com/1/aex-dev-usage-storage".to_owned(),
            ),
            (
                PROVIDER_ENDPOINT_VAR,
                "https://lambda.eu-west-1.amazonaws.com".to_owned(),
            ),
            (super::IMAGE_IDENTIFIER_VAR, "aex-hands-1gb".to_owned()),
            (DUE_SHARDS_VAR, "8".to_owned()),
            (DUE_PAGE_ITEMS_VAR, "50".to_owned()),
            (super::DUE_PAGE_READS_VAR, "100".to_owned()),
            (super::PRICING_VERSION_VAR, "synthetic-zero-v1".to_owned()),
        ])
    }

    fn read(vars: &BTreeMap<&'static str, String>) -> Result<Config, ConfigError> {
        Config::from_lookup(|name| vars.get(name).cloned())
    }

    #[test]
    fn a_complete_environment_is_accepted_and_fully_resolved() {
        let config = read(&complete()).expect("a complete environment is accepted");
        assert_eq!(config.plane, "dev");
        assert_eq!(config.region, Region::EuWest1);
        assert_eq!(config.runtime_activity_table, "aex-dev-runtime-activity");
        assert_eq!(config.due_shards, 8);
        assert_eq!(config.page.max_items, 50);
        assert_eq!(config.page.max_reads, 100);
    }

    #[test]
    fn every_required_variable_is_named_when_it_is_missing() {
        assert_eq!(
            REQUIRED_VARS.len(),
            complete().len(),
            "the fixture and the required set must not drift"
        );
        for name in REQUIRED_VARS {
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
    fn a_blank_value_is_missing_rather_than_present() {
        let mut vars = complete();
        vars.insert(super::RUNTIME_ACTIVITY_TABLE_VAR, "   ".to_owned());
        assert_eq!(
            read(&vars),
            Err(ConfigError::Missing {
                name: super::RUNTIME_ACTIVITY_TABLE_VAR
            })
        );
    }

    #[test]
    fn an_unknown_plane_or_region_is_rejected_rather_than_guessed() {
        let mut plane = complete();
        plane.insert(PLANE_VAR, "staging".to_owned());
        assert!(matches!(
            read(&plane),
            Err(ConfigError::Invalid {
                name: PLANE_VAR,
                ..
            })
        ));

        let mut region = complete();
        region.insert(REGION_VAR, "eu-west-9".to_owned());
        assert!(matches!(
            read(&region),
            Err(ConfigError::Invalid {
                name: REGION_VAR,
                ..
            })
        ));
    }

    #[test]
    fn a_queue_or_endpoint_outside_the_configured_region_is_refused() {
        for name in [
            LIFECYCLE_QUEUE_VAR,
            COMPUTE_QUEUE_VAR,
            STORAGE_QUEUE_VAR,
            PROVIDER_ENDPOINT_VAR,
        ] {
            let mut vars = complete();
            vars.insert(
                name,
                "https://sqs.us-east-1.amazonaws.com/1/elsewhere".to_owned(),
            );
            assert!(
                matches!(read(&vars), Err(ConfigError::Invalid { name: named, .. }) if named == name),
                "a cross-region {name} is a write nobody notices until the bill"
            );

            let mut plain = complete();
            plain.insert(name, "http://sqs.eu-west-1.amazonaws.com/1/x".to_owned());
            assert!(
                matches!(read(&plain), Err(ConfigError::Invalid { name: named, .. }) if named == name),
                "{name} must be https"
            );
        }
    }

    #[test]
    fn a_non_numeric_or_zero_budget_is_refused() {
        for name in [
            DUE_SHARDS_VAR,
            DUE_PAGE_ITEMS_VAR,
            super::DUE_PAGE_READS_VAR,
        ] {
            for value in ["lots", "0"] {
                let mut vars = complete();
                vars.insert(name, value.to_owned());
                assert!(
                    matches!(read(&vars), Err(ConfigError::Invalid { name: named, .. }) if named == name),
                    "{name} = {value}"
                );
            }
        }
    }

    #[test]
    fn the_two_usage_ingresses_cannot_be_the_same_queue() {
        let mut vars = complete();
        let compute = vars[COMPUTE_QUEUE_VAR].clone();
        vars.insert(STORAGE_QUEUE_VAR, compute);
        assert!(matches!(
            read(&vars),
            Err(ConfigError::Invalid {
                name: STORAGE_QUEUE_VAR,
                ..
            })
        ));
    }

    #[test]
    fn a_transfer_binding_or_a_tuned_threshold_refuses_the_start() {
        for name in FORBIDDEN_VARS {
            let mut vars = complete();
            vars.insert(name, "anything".to_owned());
            assert!(
                matches!(read(&vars), Err(ConfigError::Forbidden { name: named, .. }) if named == name),
                "{name} must refuse the start rather than be quietly ignored"
            );
        }
        assert!(
            !REQUIRED_VARS
                .iter()
                .any(|name| name.contains("TRANSFER") || name.contains("TRUE_IDLE")),
            "neither a transfer binding nor a threshold is part of this worker's configuration"
        );
    }
}
