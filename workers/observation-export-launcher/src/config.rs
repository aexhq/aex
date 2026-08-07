//! Validated start-up configuration for `observation-export-launcher`.
//!
//! Nothing here has a default. A variable that identifies a table, a cluster, a
//! task definition, a subnet, a security group or a region must be supplied
//! explicitly, because a defaulted resource identifier silently binds the
//! process to the wrong plane — and this deployable's whole job is to start real
//! Fargate tasks, so a wrong binding is a wrong launch rather than a wrong read.
//!
//! Both `ECS` identifiers are checked against the region this process is bound
//! to. A cross-region cluster or task definition can never be launched into, so
//! discovering it at start-up is strictly better than discovering it as a
//! `RunTask` failure with a claimed export already fenced.

use std::time::Duration;

use aex_wire::types::Region;

/// Environment variable naming the deployment plane.
pub const PLANE_VAR: &str = "AEX_PLANE";
/// Environment variable naming the bound `AWS` region.
pub const REGION_VAR: &str = "AEX_REGION";
/// Environment variable naming the observation-authority `DynamoDB` table.
///
/// The launcher reads and writes only `EXPORT#` and `CTRL#` rows on it.
pub const OBSERVATION_TABLE_VAR: &str = "AEX_OBSERVATION_TABLE";
/// Environment variable naming the export `ECS` cluster `ARN`.
pub const EXPORT_CLUSTER_VAR: &str = "AEX_EXPORT_CLUSTER";
/// Environment variable naming the export task-definition `ARN`.
pub const EXPORT_TASK_DEFINITION_VAR: &str = "AEX_EXPORT_TASK_DEFINITION";
/// Environment variable naming the subnets an export task is placed in.
pub const EXPORT_SUBNETS_VAR: &str = "AEX_EXPORT_SUBNETS";
/// Environment variable naming the security groups an export task runs under.
pub const EXPORT_SECURITY_GROUPS_VAR: &str = "AEX_EXPORT_SECURITY_GROUPS";
/// Environment variable naming how many exports one sweep may launch.
pub const EXPORT_MAX_CONCURRENT_VAR: &str = "AEX_EXPORT_MAX_CONCURRENT";
/// Environment variable naming how many control shards one sweep scans.
pub const EXPORT_LAUNCH_SHARDS_VAR: &str = "AEX_EXPORT_LAUNCH_SHARDS";
/// Environment variable naming the claim lease duration in milliseconds.
pub const EXPORT_LEASE_MS_VAR: &str = "AEX_EXPORT_LEASE_MS";

/// Every variable this deployable requires, in declaration order.
pub const REQUIRED_VARS: &[&str] = &[
    PLANE_VAR,
    REGION_VAR,
    OBSERVATION_TABLE_VAR,
    EXPORT_CLUSTER_VAR,
    EXPORT_TASK_DEFINITION_VAR,
    EXPORT_SUBNETS_VAR,
    EXPORT_SECURITY_GROUPS_VAR,
    EXPORT_MAX_CONCURRENT_VAR,
    EXPORT_LAUNCH_SHARDS_VAR,
    EXPORT_LEASE_MS_VAR,
];

/// Planes this deployable may be bound to.
pub const PLANES: [&str; 2] = ["dev", "prd"];

/// The `ARN` resource type of an `ECS` cluster.
pub const CLUSTER_RESOURCE: &str = "cluster";
/// The `ARN` resource type of an `ECS` task definition.
pub const TASK_DEFINITION_RESOURCE: &str = "task-definition";

/// The inclusive bounds on how many exports one sweep may launch.
pub const MAX_CONCURRENT_RANGE: (usize, usize) = (1, 100);
/// The inclusive bounds on how many control shards one sweep scans.
pub const LAUNCH_SHARDS_RANGE: (u8, u8) = (1, 64);
/// The inclusive bounds on the claim lease, in milliseconds.
pub const LEASE_MS_RANGE: (u64, u64) = (1_000, 900_000);

/// Why `observation-export-launcher` refused to start.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ObservationExportLauncherConfigError {
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

/// The validated configuration of one `observation-export-launcher` process.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    /// Deployment plane this process belongs to.
    pub plane: String,
    /// Region this process is bound to.
    pub region: Region,
    /// The observation-authority table holding the `EXPORT#` and `CTRL#` rows.
    pub observation_table: String,
    /// The export `ECS` cluster `ARN`.
    pub export_cluster: String,
    /// The export task-definition `ARN`.
    pub export_task_definition: String,
    /// The container the overrides are addressed to, taken from the family the
    /// task-definition `ARN` names.
    pub export_container: String,
    /// The subnets an export task is placed in.
    pub export_subnets: Vec<String>,
    /// The security groups an export task runs under.
    pub export_security_groups: Vec<String>,
    /// How many exports one sweep may launch.
    pub max_concurrent: usize,
    /// How many control shards one sweep scans.
    pub launch_shards: u8,
    /// How long a launch claim is leased for.
    pub lease: Duration,
}

impl Config {
    /// Reads and validates the configuration from the process environment.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationExportLauncherConfigError::Missing`] when a required variable is absent or
    /// empty, and [`ObservationExportLauncherConfigError::Invalid`] when a variable is present but does
    /// not parse, is outside its permitted range, or names a resource in a
    /// region other than the one this process is bound to.
    pub fn from_env() -> Result<Self, ObservationExportLauncherConfigError> {
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
    pub fn from_lookup<F>(lookup: F) -> Result<Self, ObservationExportLauncherConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let plane = required(&lookup, PLANE_VAR)?;
        if !PLANES.contains(&plane.as_str()) {
            return Err(ObservationExportLauncherConfigError::Invalid {
                name: PLANE_VAR,
                reason: format!("expected one of {PLANES:?}, got `{plane}`"),
            });
        }
        let raw_region = required(&lookup, REGION_VAR)?;
        let region = Region::from_name(&raw_region).ok_or_else(|| ObservationExportLauncherConfigError::Invalid {
            name: REGION_VAR,
            reason: format!("`{raw_region}` is not a regional plane region"),
        })?;

        let observation_table = required(&lookup, OBSERVATION_TABLE_VAR)?;
        let export_cluster = ecs_arn(&lookup, EXPORT_CLUSTER_VAR, region, CLUSTER_RESOURCE)?;
        let export_task_definition = ecs_arn(
            &lookup,
            EXPORT_TASK_DEFINITION_VAR,
            region,
            TASK_DEFINITION_RESOURCE,
        )?;
        let export_container = task_definition_family(&export_task_definition)
            .ok_or_else(|| ObservationExportLauncherConfigError::Invalid {
                name: EXPORT_TASK_DEFINITION_VAR,
                reason: format!("`{export_task_definition}` names no task-definition family"),
            })?
            .to_owned();

        let max_concurrent = bounded(
            &lookup,
            EXPORT_MAX_CONCURRENT_VAR,
            MAX_CONCURRENT_RANGE.0,
            MAX_CONCURRENT_RANGE.1,
        )?;
        let launch_shards = bounded(
            &lookup,
            EXPORT_LAUNCH_SHARDS_VAR,
            LAUNCH_SHARDS_RANGE.0,
            LAUNCH_SHARDS_RANGE.1,
        )?;
        let lease_ms = bounded(
            &lookup,
            EXPORT_LEASE_MS_VAR,
            LEASE_MS_RANGE.0,
            LEASE_MS_RANGE.1,
        )?;

        Ok(Self {
            plane,
            region,
            observation_table,
            export_cluster,
            export_task_definition,
            export_container,
            export_subnets: id_list(&lookup, EXPORT_SUBNETS_VAR, "subnet-")?,
            export_security_groups: id_list(&lookup, EXPORT_SECURITY_GROUPS_VAR, "sg-")?,
            max_concurrent,
            launch_shards,
            lease: Duration::from_millis(lease_ms),
        })
    }
}

/// Reads a required, non-blank variable.
fn required<F>(lookup: &F, name: &'static str) -> Result<String, ObservationExportLauncherConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    match lookup(name) {
        Some(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(ObservationExportLauncherConfigError::Missing { name }),
    }
}

/// Reads a count inside an inclusive range.
///
/// Both ends are named in the failure, because "out of range" without the range
/// is a message an operator cannot act on.
fn bounded<F, T>(lookup: &F, name: &'static str, low: T, high: T) -> Result<T, ObservationExportLauncherConfigError>
where
    F: Fn(&str) -> Option<String>,
    T: std::str::FromStr + PartialOrd + std::fmt::Display + Copy,
{
    let raw = required(lookup, name)?;
    let value = raw.trim().parse::<T>().map_err(|_| ObservationExportLauncherConfigError::Invalid {
        name,
        reason: format!("expected an integer in {low}..={high}, got `{raw}`"),
    })?;
    if value < low || value > high {
        return Err(ObservationExportLauncherConfigError::Invalid {
            name,
            reason: format!("expected an integer in {low}..={high}, got `{value}`"),
        });
    }
    Ok(value)
}

/// Reads an `ECS` `ARN` and proves it names a resource of this process's region.
fn ecs_arn<F>(
    lookup: &F,
    name: &'static str,
    region: Region,
    resource: &'static str,
) -> Result<String, ObservationExportLauncherConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let value = required(lookup, name)?;
    let invalid = |reason: String| ObservationExportLauncherConfigError::Invalid { name, reason };
    let mut fields = value.splitn(6, ':');
    let scheme = fields.next().unwrap_or_default();
    let partition = fields.next().unwrap_or_default();
    let service = fields.next().unwrap_or_default();
    let arn_region = fields.next().unwrap_or_default();
    let account = fields.next().unwrap_or_default();
    let tail = fields.next().unwrap_or_default();

    if scheme != "arn" || partition.is_empty() || service != "ecs" {
        return Err(invalid(format!(
            "`{value}` is not an `arn:<partition>:ecs:...` identifier"
        )));
    }
    if arn_region != region.as_str() {
        return Err(invalid(format!(
            "the ARN names region `{arn_region}` and this process is bound to `{}`; \
             a cross-region {resource} can never be launched into",
            region.as_str()
        )));
    }
    if account.len() != 12 || !account.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid(format!(
            "`{account}` is not a 12-digit AWS account id"
        )));
    }
    let named = tail
        .strip_prefix(resource)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or_default();
    if named.is_empty() {
        return Err(invalid(format!(
            "`{tail}` is not a `{resource}/...` resource"
        )));
    }
    Ok(value)
}

/// The family a task-definition `ARN` names.
///
/// The export task definition holds exactly one container and the family names
/// it, so this is also the name every container override is addressed to. There
/// is no separate variable for it: two spellings of one fact drift.
#[must_use]
pub fn task_definition_family(arn: &str) -> Option<&str> {
    let (_, tail) = arn.split_once(":task-definition/")?;
    let family = tail.split(':').next()?;
    (!family.is_empty()).then_some(family)
}

/// Reads a comma-separated list of `AWS` resource ids sharing one prefix.
fn id_list<F>(
    lookup: &F,
    name: &'static str,
    prefix: &'static str,
) -> Result<Vec<String>, ObservationExportLauncherConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let raw = required(lookup, name)?;
    let invalid = |reason: String| ObservationExportLauncherConfigError::Invalid { name, reason };
    let mut ids = Vec::new();
    for entry in raw.split(',') {
        let entry = entry.trim();
        let Some(suffix) = entry.strip_prefix(prefix) else {
            return Err(invalid(format!(
                "`{entry}` is not a `{prefix}...` identifier"
            )));
        };
        let well_formed = (8..=17).contains(&suffix.len())
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
        if !well_formed {
            return Err(invalid(format!(
                "`{entry}` is not `{prefix}` followed by 8 to 17 lowercase hex digits"
            )));
        }
        ids.push(entry.to_owned());
    }
    if ids.is_empty() {
        return Err(invalid("at least one identifier is required".to_owned()));
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        Config, ObservationExportLauncherConfigError, EXPORT_CLUSTER_VAR, EXPORT_LAUNCH_SHARDS_VAR, EXPORT_LEASE_MS_VAR,
        EXPORT_MAX_CONCURRENT_VAR, EXPORT_SECURITY_GROUPS_VAR, EXPORT_SUBNETS_VAR,
        EXPORT_TASK_DEFINITION_VAR, OBSERVATION_TABLE_VAR, PLANE_VAR, REGION_VAR, REQUIRED_VARS,
        task_definition_family,
    };

    fn complete() -> BTreeMap<&'static str, String> {
        BTreeMap::from([
            (PLANE_VAR, "dev".to_owned()),
            (REGION_VAR, "eu-west-1".to_owned()),
            (OBSERVATION_TABLE_VAR, "observation-authority".to_owned()),
            (
                EXPORT_CLUSTER_VAR,
                "arn:aws:ecs:eu-west-1:123456789012:cluster/aex-dev-export".to_owned(),
            ),
            (
                EXPORT_TASK_DEFINITION_VAR,
                "arn:aws:ecs:eu-west-1:123456789012:task-definition/observation-export-task:7"
                    .to_owned(),
            ),
            (
                EXPORT_SUBNETS_VAR,
                "subnet-0a1b2c3d,subnet-4e5f6a7b".to_owned(),
            ),
            (EXPORT_SECURITY_GROUPS_VAR, "sg-0123abcd".to_owned()),
            (EXPORT_MAX_CONCURRENT_VAR, "20".to_owned()),
            (EXPORT_LAUNCH_SHARDS_VAR, "8".to_owned()),
            (EXPORT_LEASE_MS_VAR, "120000".to_owned()),
        ])
    }

    fn read(vars: &BTreeMap<&'static str, String>) -> Result<Config, ObservationExportLauncherConfigError> {
        Config::from_lookup(|name| vars.get(name).cloned())
    }

    #[test]
    fn accepts_a_complete_environment() {
        let config = read(&complete()).expect("a complete environment starts");
        assert_eq!(config.plane, "dev");
        assert_eq!(config.region.as_str(), "eu-west-1");
        assert_eq!(config.observation_table, "observation-authority");
        assert_eq!(config.export_container, "observation-export-task");
        assert_eq!(config.export_subnets.len(), 2);
        assert_eq!(
            config.export_security_groups,
            vec!["sg-0123abcd".to_owned()]
        );
        assert_eq!(config.max_concurrent, 20);
        assert_eq!(config.launch_shards, 8);
        assert_eq!(config.lease.as_millis(), 120_000);
    }

    #[test]
    fn names_every_missing_variable() {
        for name in REQUIRED_VARS {
            let mut vars = complete();
            vars.remove(*name);
            assert_eq!(
                read(&vars),
                Err(ObservationExportLauncherConfigError::Missing { name }),
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
            Err(ObservationExportLauncherConfigError::Missing {
                name: OBSERVATION_TABLE_VAR
            })
        );
    }

    #[test]
    fn an_off_region_cluster_arn_is_refused_by_name() {
        let mut vars = complete();
        vars.insert(
            EXPORT_CLUSTER_VAR,
            "arn:aws:ecs:us-east-1:123456789012:cluster/aex-dev-export".to_owned(),
        );
        let error = read(&vars).expect_err("a cross-region cluster is refused");
        match error {
            ObservationExportLauncherConfigError::Invalid { name, reason } => {
                assert_eq!(name, EXPORT_CLUSTER_VAR);
                assert!(reason.contains("us-east-1"), "{reason}");
                assert!(reason.contains("eu-west-1"), "{reason}");
            }
            other @ ObservationExportLauncherConfigError::Missing { .. } => {
                panic!("expected an invalid-cluster failure, got {other:?}")
            }
        }
    }

    #[test]
    fn an_off_region_task_definition_arn_is_refused_by_name() {
        let mut vars = complete();
        vars.insert(
            EXPORT_TASK_DEFINITION_VAR,
            "arn:aws:ecs:ap-northeast-1:123456789012:task-definition/observation-export-task:1"
                .to_owned(),
        );
        assert!(matches!(
            read(&vars),
            Err(ObservationExportLauncherConfigError::Invalid {
                name: EXPORT_TASK_DEFINITION_VAR,
                ..
            })
        ));
    }

    #[test]
    fn a_cluster_arn_of_another_service_or_account_shape_is_refused() {
        for hostile in [
            "arn:aws:ec2:eu-west-1:123456789012:cluster/aex-dev-export",
            "arn:aws:ecs:eu-west-1:12345:cluster/aex-dev-export",
            "arn:aws:ecs:eu-west-1:123456789012:task-definition/aex-dev-export",
            "arn:aws:ecs:eu-west-1:123456789012:cluster/",
            "aex-dev-export",
        ] {
            let mut vars = complete();
            vars.insert(EXPORT_CLUSTER_VAR, hostile.to_owned());
            assert!(
                matches!(
                    read(&vars),
                    Err(ObservationExportLauncherConfigError::Invalid {
                        name: EXPORT_CLUSTER_VAR,
                        ..
                    })
                ),
                "`{hostile}` was admitted"
            );
        }
    }

    #[test]
    fn a_malformed_subnet_id_is_refused_by_name() {
        for hostile in [
            "subnet-0a1b2c3d,sg-0123abcd",
            "subnet-0a1b",
            "subnet-0A1B2C3D",
            "subnet-",
            "vpc-0a1b2c3d",
            "subnet-0a1b2c3d,",
        ] {
            let mut vars = complete();
            vars.insert(EXPORT_SUBNETS_VAR, hostile.to_owned());
            assert!(
                matches!(
                    read(&vars),
                    Err(ObservationExportLauncherConfigError::Invalid {
                        name: EXPORT_SUBNETS_VAR,
                        ..
                    })
                ),
                "`{hostile}` was admitted"
            );
        }
    }

    #[test]
    fn an_empty_security_group_list_is_refused_by_name() {
        // Blank is missing; a list that carries only separators is invalid.
        // Either way the variable is named, because a task launched with no
        // security group is a task with no reachable egress policy.
        let mut vars = complete();
        vars.insert(EXPORT_SECURITY_GROUPS_VAR, String::new());
        assert_eq!(
            read(&vars),
            Err(ObservationExportLauncherConfigError::Missing {
                name: EXPORT_SECURITY_GROUPS_VAR
            })
        );

        let mut vars = complete();
        vars.insert(EXPORT_SECURITY_GROUPS_VAR, ",".to_owned());
        assert!(matches!(
            read(&vars),
            Err(ObservationExportLauncherConfigError::Invalid {
                name: EXPORT_SECURITY_GROUPS_VAR,
                ..
            })
        ));
    }

    #[test]
    fn every_bounded_count_refuses_both_ends_of_its_range() {
        for (name, low, high) in [
            (EXPORT_MAX_CONCURRENT_VAR, "0", "101"),
            (EXPORT_LAUNCH_SHARDS_VAR, "0", "65"),
            (EXPORT_LEASE_MS_VAR, "999", "900001"),
        ] {
            for value in [low, high, "lots", "-1"] {
                let mut vars = complete();
                vars.insert(name, value.to_owned());
                let error = read(&vars).expect_err("out of range");
                assert!(
                    matches!(error, ObservationExportLauncherConfigError::Invalid { name: named, .. } if named == name),
                    "`{name}` = `{value}` produced {error:?}"
                );
            }
        }
    }

    #[test]
    fn an_unknown_plane_and_region_are_both_refused() {
        let mut vars = complete();
        vars.insert(PLANE_VAR, "staging".to_owned());
        assert!(matches!(
            read(&vars),
            Err(ObservationExportLauncherConfigError::Invalid {
                name: PLANE_VAR,
                ..
            })
        ));
        let mut vars = complete();
        vars.insert(REGION_VAR, "eu-central-9".to_owned());
        assert!(matches!(
            read(&vars),
            Err(ObservationExportLauncherConfigError::Invalid {
                name: REGION_VAR,
                ..
            })
        ));
    }

    #[test]
    fn the_container_name_is_the_task_definition_family() {
        assert_eq!(
            task_definition_family("arn:aws:ecs:eu-west-1:123456789012:task-definition/family:3"),
            Some("family")
        );
        assert_eq!(
            task_definition_family("arn:aws:ecs:eu-west-1:123456789012:task-definition/family"),
            Some("family")
        );
        assert_eq!(
            task_definition_family("arn:aws:ecs:eu-west-1:123456789012:cluster/family"),
            None
        );
        assert_eq!(
            task_definition_family("arn:aws:ecs:eu-west-1:123456789012:task-definition/"),
            None
        );
    }
}
