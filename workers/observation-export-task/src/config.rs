//! Validated start-up configuration for `observation-export-task`.
//!
//! Nothing here has a default. A variable that identifies a table, a bucket, a
//! key, a region, a workspace or the export itself must be supplied explicitly,
//! because a defaulted resource identifier silently binds a one-shot task to the
//! wrong plane — and this task writes an artifact a customer downloads.
//!
//! Every bound is a refusal at start-up rather than a discovery at runtime. The
//! part size is the sharpest case: S3 refuses a non-final part under 5 MiB, so a
//! smaller value could never complete an upload and must never reach the loop.

use std::time::Duration;

use aex_wire::ids::{ExportId, WorkspaceId};
use aex_wire::types::Region;

use crate::budget::MemoryPlan;

/// Environment variable naming the deployment plane.
pub const PLANE_VAR: &str = "AEX_PLANE";
/// Environment variable naming the bound `AWS` region.
pub const REGION_VAR: &str = "AEX_REGION";
/// Environment variable naming the export this task produces.
pub const EXPORT_ID_VAR: &str = "AEX_EXPORT_ID";
/// Environment variable naming the workspace the export belongs to.
pub const WORKSPACE_ID_VAR: &str = "AEX_WORKSPACE_ID";
/// Environment variable naming the observation-authority `DynamoDB` table.
pub const OBSERVATION_TABLE_VAR: &str = "AEX_OBSERVATION_TABLE";
/// Environment variable naming the regional observation `S3` bucket.
pub const OBSERVATION_BUCKET_VAR: &str = "AEX_OBSERVATION_BUCKET";
/// Environment variable naming the whole working memory budget, in bytes.
pub const MEMORY_BUDGET_VAR: &str = "AEX_EXPORT_MEMORY_BUDGET_BYTES";
/// Environment variable naming the multipart part size, in bytes.
pub const PART_BYTES_VAR: &str = "AEX_EXPORT_PART_BYTES";
/// Environment variable naming the row-group buffer, in bytes.
pub const ROWGROUP_BYTES_VAR: &str = "AEX_EXPORT_ROWGROUP_BYTES";
/// Environment variable naming how many observations one page may carry.
pub const PAGE_LIMIT_VAR: &str = "AEX_EXPORT_PAGE_LIMIT";
/// Environment variable naming the export lease, in milliseconds.
pub const LEASE_MS_VAR: &str = "AEX_EXPORT_LEASE_MS";
/// Optional environment variable naming the loopback health port.
///
/// The Fargate row declares `port = 0`, so a one-shot task normally binds no
/// listener at all and the same JSON is available through
/// [`crate::health::health_body`] and [`crate::health::readiness_body`].
pub const HEALTH_PORT_VAR: &str = "AEX_HEALTH_PORT";

/// Every variable this deployable requires, in declaration order.
pub const REQUIRED_VARS: &[&str] = &[
    PLANE_VAR,
    REGION_VAR,
    EXPORT_ID_VAR,
    WORKSPACE_ID_VAR,
    OBSERVATION_TABLE_VAR,
    OBSERVATION_BUCKET_VAR,
    MEMORY_BUDGET_VAR,
    PART_BYTES_VAR,
    ROWGROUP_BYTES_VAR,
    PAGE_LIMIT_VAR,
    LEASE_MS_VAR,
];

/// Planes this deployable may be bound to.
pub const PLANES: [&str; 2] = ["dev", "prd"];

/// The smallest multipart part S3 will accept for a non-final part.
pub const PART_BYTES_MIN: usize = 5 * 1024 * 1024;
/// The largest part size this task will buffer.
pub const PART_BYTES_MAX: usize = 64 * 1024 * 1024;
/// The smallest row-group buffer.
pub const ROWGROUP_BYTES_MIN: usize = 1024 * 1024;
/// The largest row-group buffer.
pub const ROWGROUP_BYTES_MAX: usize = 256 * 1024 * 1024;
/// The smallest page.
pub const PAGE_LIMIT_MIN: usize = 1;
/// The largest page.
pub const PAGE_LIMIT_MAX: usize = 1_000;
/// The shortest usable lease, in milliseconds.
pub const LEASE_MS_MIN: u64 = 1_000;
/// The longest usable lease, in milliseconds.
pub const LEASE_MS_MAX: u64 = 900_000;

/// Why the part-size floor exists, quoted in the refusal.
pub const PART_FLOOR_REASON: &str = "S3 refuses a non-final part under 5 MiB, so an upload built from smaller parts \
     could never be completed";

/// Why `observation-export-task` refused to start.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ObservationExportTaskConfigError {
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

/// The validated configuration of one `observation-export-task` run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    /// Deployment plane this process belongs to.
    pub plane: String,
    /// Region this process is bound to.
    pub region: Region,
    /// The export this task produces, exactly one.
    pub export_id: ExportId,
    /// The workspace the export belongs to.
    pub workspace_id: WorkspaceId,
    /// The observation-authority table.
    pub observation_table: String,
    /// The regional observation bucket, which also holds `exports/`.
    pub observation_bucket: String,
    /// The whole working memory budget, in bytes.
    pub memory_budget_bytes: usize,
    /// The multipart part size, in bytes.
    pub part_bytes: usize,
    /// The row-group buffer, in bytes.
    pub rowgroup_bytes: usize,
    /// How many observations one page may carry.
    pub page_limit: usize,
    /// How long the export lease is held for.
    pub lease: Duration,
    /// The loopback health port, when one is configured.
    pub health_port: Option<u16>,
}

impl Config {
    /// Reads and validates the configuration from the process environment.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationExportTaskConfigError::Missing`] when a required variable is absent or
    /// empty, and [`ObservationExportTaskConfigError::Invalid`] when a variable is present but does
    /// not parse or is outside its permitted range.
    pub fn from_env() -> Result<Self, ObservationExportTaskConfigError> {
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
    pub fn from_lookup<F>(lookup: F) -> Result<Self, ObservationExportTaskConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let plane = required(&lookup, PLANE_VAR)?;
        if !PLANES.contains(&plane.as_str()) {
            return Err(ObservationExportTaskConfigError::Invalid {
                name: PLANE_VAR,
                reason: format!("expected one of {PLANES:?}, got `{plane}`"),
            });
        }
        let raw_region = required(&lookup, REGION_VAR)?;
        let region = Region::from_name(&raw_region).ok_or_else(|| ObservationExportTaskConfigError::Invalid {
            name: REGION_VAR,
            reason: format!("`{raw_region}` is not a regional plane region"),
        })?;
        let export_id = identifier::<ExportId, F>(&lookup, EXPORT_ID_VAR)?;
        let workspace_id = identifier::<WorkspaceId, F>(&lookup, WORKSPACE_ID_VAR)?;

        let part_bytes = ranged(
            &lookup,
            PART_BYTES_VAR,
            (PART_BYTES_MIN, PART_BYTES_MAX),
            PART_FLOOR_REASON,
        )?;
        let rowgroup_bytes = ranged(
            &lookup,
            ROWGROUP_BYTES_VAR,
            (ROWGROUP_BYTES_MIN, ROWGROUP_BYTES_MAX),
            "a row group outside this range cannot be reserved up front",
        )?;
        let page_limit = ranged(
            &lookup,
            PAGE_LIMIT_VAR,
            (PAGE_LIMIT_MIN, PAGE_LIMIT_MAX),
            "the page buffer is reserved as `page limit x 64 KiB` before the loop starts",
        )?;
        let lease_ms = ranged(
            &lookup,
            LEASE_MS_VAR,
            (
                usize::try_from(LEASE_MS_MIN).unwrap_or(usize::MAX),
                usize::try_from(LEASE_MS_MAX).unwrap_or(usize::MAX),
            ),
            "a lease outside this range either expires mid-part or outlives the task",
        )?;
        let memory_budget_bytes = memory_budget(
            &lookup,
            &MemoryPlan::new(page_limit, part_bytes, rowgroup_bytes),
        )?;

        Ok(Self {
            plane,
            region,
            export_id,
            workspace_id,
            observation_table: required(&lookup, OBSERVATION_TABLE_VAR)?,
            observation_bucket: required(&lookup, OBSERVATION_BUCKET_VAR)?,
            memory_budget_bytes,
            part_bytes,
            rowgroup_bytes,
            page_limit,
            lease: Duration::from_millis(u64::try_from(lease_ms).map_err(|_| {
                ObservationExportTaskConfigError::Invalid {
                    name: LEASE_MS_VAR,
                    reason: "the lease does not fit a 64-bit millisecond count".to_owned(),
                }
            })?),
            health_port: health_port(&lookup)?,
        })
    }

    /// The memory plan this configuration implies.
    #[must_use]
    pub const fn memory_plan(&self) -> MemoryPlan {
        MemoryPlan::new(self.page_limit, self.part_bytes, self.rowgroup_bytes)
    }
}

/// Reads a required, non-blank variable.
fn required<F>(lookup: &F, name: &'static str) -> Result<String, ObservationExportTaskConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    match lookup(name) {
        Some(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(ObservationExportTaskConfigError::Missing { name }),
    }
}

/// Reads a required identifier of one exact resource kind.
fn identifier<I, F>(lookup: &F, name: &'static str) -> Result<I, ObservationExportTaskConfigError>
where
    I: std::str::FromStr<Err = aex_wire::ids::IdParseError>,
    F: Fn(&str) -> Option<String>,
{
    let raw = required(lookup, name)?;
    raw.trim()
        .parse::<I>()
        .map_err(|error| ObservationExportTaskConfigError::Invalid {
            name,
            reason: format!("`{raw}` is not the identifier this variable names: {error}"),
        })
}

/// Reads a required, strictly positive count.
fn positive<F>(lookup: &F, name: &'static str) -> Result<usize, ObservationExportTaskConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let raw = required(lookup, name)?;
    let value = raw
        .trim()
        .parse::<usize>()
        .map_err(|error| ObservationExportTaskConfigError::Invalid {
            name,
            reason: format!("expected a positive integer, got `{raw}`: {error}"),
        })?;
    if value == 0 {
        return Err(ObservationExportTaskConfigError::Invalid {
            name,
            reason: "expected a positive integer, got `0`".to_owned(),
        });
    }
    Ok(value)
}

/// Reads a positive count that must fall inside an inclusive range.
fn ranged<F>(
    lookup: &F,
    name: &'static str,
    bounds: (usize, usize),
    because: &str,
) -> Result<usize, ObservationExportTaskConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let (low, high) = bounds;
    let value = positive(lookup, name)?;
    if value < low || value > high {
        return Err(ObservationExportTaskConfigError::Invalid {
            name,
            reason: format!("{value} is outside the permitted {low}..={high} range: {because}"),
        });
    }
    Ok(value)
}

/// Reads the working budget and proves it covers every reservation.
fn memory_budget<F>(lookup: &F, plan: &MemoryPlan) -> Result<usize, ObservationExportTaskConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let value = positive(lookup, MEMORY_BUDGET_VAR)?;
    let total = plan.total_bytes();
    if value < total {
        return Err(ObservationExportTaskConfigError::Invalid {
            name: MEMORY_BUDGET_VAR,
            reason: format!(
                "a budget of {value} bytes cannot cover the {total} bytes this export reserves \
                 before its producing loop ({}), so every run would fail `{}`",
                plan.describe(),
                crate::budget::CAPACITY_CODE
            ),
        });
    }
    Ok(value)
}

/// Reads the optional loopback health port.
fn health_port<F>(lookup: &F) -> Result<Option<u16>, ObservationExportTaskConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let Some(raw) = lookup(HEALTH_PORT_VAR) else {
        return Ok(None);
    };
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let port = raw
        .trim()
        .parse::<u16>()
        .map_err(|error| ObservationExportTaskConfigError::Invalid {
            name: HEALTH_PORT_VAR,
            reason: format!("expected a TCP port, got `{raw}`: {error}"),
        })?;
    if port == 0 {
        // `port = 0` in the unit row means "no listener", not "any port".
        return Ok(None);
    }
    Ok(Some(port))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        Config, ObservationExportTaskConfigError, EXPORT_ID_VAR, HEALTH_PORT_VAR, LEASE_MS_VAR, MEMORY_BUDGET_VAR,
        OBSERVATION_BUCKET_VAR, OBSERVATION_TABLE_VAR, PAGE_LIMIT_VAR, PART_BYTES_VAR, PLANE_VAR,
        REGION_VAR, REQUIRED_VARS, ROWGROUP_BYTES_VAR, WORKSPACE_ID_VAR,
    };

    pub(crate) const EXPORT_FIXTURE: &str = "exp_0000000001e40r2081040g2081";
    pub(crate) const WORKSPACE_FIXTURE: &str = "wsp_0000000001e40r2081040g2081";

    pub(crate) fn complete() -> BTreeMap<&'static str, String> {
        BTreeMap::from([
            (PLANE_VAR, "dev".to_owned()),
            (REGION_VAR, "eu-west-1".to_owned()),
            (EXPORT_ID_VAR, EXPORT_FIXTURE.to_owned()),
            (WORKSPACE_ID_VAR, WORKSPACE_FIXTURE.to_owned()),
            (OBSERVATION_TABLE_VAR, "observation-authority".to_owned()),
            (OBSERVATION_BUCKET_VAR, "aex-dev-observations".to_owned()),
            (MEMORY_BUDGET_VAR, (512 * 1024 * 1024).to_string()),
            (PART_BYTES_VAR, (16 * 1024 * 1024).to_string()),
            (ROWGROUP_BYTES_VAR, (64 * 1024 * 1024).to_string()),
            (PAGE_LIMIT_VAR, "500".to_owned()),
            (LEASE_MS_VAR, "300000".to_owned()),
        ])
    }

    fn read(vars: &BTreeMap<&'static str, String>) -> Result<Config, ObservationExportTaskConfigError> {
        Config::from_lookup(|name| vars.get(name).cloned())
    }

    #[test]
    fn accepts_a_complete_environment() {
        let config = read(&complete()).expect("a complete environment starts");
        assert_eq!(config.plane, "dev");
        assert_eq!(config.region.as_str(), "eu-west-1");
        assert_eq!(config.export_id.to_string(), EXPORT_FIXTURE);
        assert_eq!(config.workspace_id.to_string(), WORKSPACE_FIXTURE);
        assert_eq!(config.part_bytes, 16 * 1024 * 1024);
        assert_eq!(config.page_limit, 500);
        assert_eq!(config.lease.as_millis(), 300_000);
        assert_eq!(config.health_port, None);
        assert!(config.memory_budget_bytes >= config.memory_plan().total_bytes());
    }

    #[test]
    fn names_every_missing_variable() {
        for name in REQUIRED_VARS {
            let mut vars = complete();
            vars.remove(*name);
            assert_eq!(
                read(&vars),
                Err(ObservationExportTaskConfigError::Missing { name }),
                "removing {name}"
            );
        }
    }

    #[test]
    fn every_required_variable_has_a_fixture_and_nothing_else_is_required() {
        let vars = complete();
        for name in REQUIRED_VARS {
            assert!(vars.contains_key(*name), "{name} has no fixture value");
        }
        assert_eq!(vars.len(), REQUIRED_VARS.len());
    }

    #[test]
    fn a_blank_resource_is_missing_rather_than_empty() {
        let mut vars = complete();
        vars.insert(OBSERVATION_BUCKET_VAR, "   ".to_owned());
        assert_eq!(
            read(&vars),
            Err(ObservationExportTaskConfigError::Missing {
                name: OBSERVATION_BUCKET_VAR
            })
        );
    }

    #[test]
    fn a_part_size_under_five_mebibytes_is_refused_by_name_and_reason() {
        let mut vars = complete();
        vars.insert(PART_BYTES_VAR, (4 * 1024 * 1024).to_string());
        let error = read(&vars).expect_err("a part S3 cannot complete is refused");
        match error {
            ObservationExportTaskConfigError::Invalid { name, reason } => {
                assert_eq!(name, PART_BYTES_VAR);
                assert!(reason.contains("5 MiB"), "{reason}");
                assert!(reason.contains("completed"), "{reason}");
            }
            other @ ObservationExportTaskConfigError::Missing { .. } => {
                panic!("expected an invalid-part-size failure, got {other:?}")
            }
        }
        // The ceiling is refused too, so the reservation stays inside the task.
        let mut vars = complete();
        vars.insert(PART_BYTES_VAR, (65 * 1024 * 1024).to_string());
        assert!(matches!(
            read(&vars),
            Err(ObservationExportTaskConfigError::Invalid {
                name: PART_BYTES_VAR,
                ..
            })
        ));
    }

    #[test]
    fn every_numeric_bound_is_enforced_at_both_ends() {
        for (name, below, above) in [
            (
                ROWGROUP_BYTES_VAR,
                (1024 * 1024 - 1).to_string(),
                (257 * 1024 * 1024).to_string(),
            ),
            (PAGE_LIMIT_VAR, "0".to_owned(), "1001".to_owned()),
            (LEASE_MS_VAR, "999".to_owned(), "900001".to_owned()),
        ] {
            for value in [below, above] {
                let mut vars = complete();
                vars.insert(name, value.clone());
                let error = read(&vars).expect_err("an out-of-range bound is refused");
                assert!(
                    matches!(error, ObservationExportTaskConfigError::Invalid { name: named, .. } if named == name),
                    "{name}={value}: {error:?}"
                );
            }
        }
    }

    #[test]
    fn a_budget_that_cannot_cover_the_reservations_is_refused_at_start_up() {
        let mut vars = complete();
        vars.insert(MEMORY_BUDGET_VAR, (8 * 1024 * 1024).to_string());
        let error = read(&vars).expect_err("a budget below the reservations is refused");
        match error {
            ObservationExportTaskConfigError::Invalid { name, reason } => {
                assert_eq!(name, MEMORY_BUDGET_VAR);
                assert!(reason.contains("export_capacity"), "{reason}");
            }
            other @ ObservationExportTaskConfigError::Missing { .. } => {
                panic!("expected an invalid-budget failure, got {other:?}")
            }
        }
    }

    #[test]
    fn an_identifier_of_the_wrong_kind_is_refused() {
        let mut vars = complete();
        vars.insert(EXPORT_ID_VAR, WORKSPACE_FIXTURE.to_owned());
        assert!(matches!(
            read(&vars),
            Err(ObservationExportTaskConfigError::Invalid {
                name: EXPORT_ID_VAR,
                ..
            })
        ));
        let mut vars = complete();
        vars.insert(WORKSPACE_ID_VAR, EXPORT_FIXTURE.to_owned());
        assert!(matches!(
            read(&vars),
            Err(ObservationExportTaskConfigError::Invalid {
                name: WORKSPACE_ID_VAR,
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
            Err(ObservationExportTaskConfigError::Invalid {
                name: PLANE_VAR,
                ..
            })
        ));
        let mut vars = complete();
        vars.insert(REGION_VAR, "eu-central-9".to_owned());
        assert!(matches!(
            read(&vars),
            Err(ObservationExportTaskConfigError::Invalid {
                name: REGION_VAR,
                ..
            })
        ));
    }

    #[test]
    fn the_health_port_is_optional_and_zero_means_no_listener() {
        let mut vars = complete();
        vars.insert(HEALTH_PORT_VAR, "0".to_owned());
        assert_eq!(read(&vars).expect("zero is accepted").health_port, None);

        let mut vars = complete();
        vars.insert(HEALTH_PORT_VAR, "8080".to_owned());
        assert_eq!(
            read(&vars).expect("a real port is accepted").health_port,
            Some(8080)
        );

        let mut vars = complete();
        vars.insert(HEALTH_PORT_VAR, "not-a-port".to_owned());
        assert!(matches!(
            read(&vars),
            Err(ObservationExportTaskConfigError::Invalid {
                name: HEALTH_PORT_VAR,
                ..
            })
        ));
    }

    #[test]
    fn no_resource_identifier_has_a_default() {
        // Removing everything leaves the first required variable named, and no
        // field is silently filled in from a constant.
        let empty = BTreeMap::new();
        assert_eq!(read(&empty), Err(ObservationExportTaskConfigError::Missing { name: PLANE_VAR }));
        let mut vars = complete();
        vars.remove(OBSERVATION_TABLE_VAR);
        assert_eq!(
            read(&vars),
            Err(ObservationExportTaskConfigError::Missing {
                name: OBSERVATION_TABLE_VAR
            })
        );
    }
}
