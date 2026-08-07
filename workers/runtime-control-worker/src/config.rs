//! Start-up configuration.
//!
//! Nothing here has a default. A variable that identifies a table, a queue, a
//! region must be supplied explicitly, because a defaulted resource identifier
//! silently binds the process to the wrong plane, region or table — and for this
//! worker "the wrong table" means suspending another plane's generations. The
//! provider endpoint is the sole exception: the official SDK's regional endpoint
//! is the production default, and the variable is only an explicit test or
//! compatibility override.
//!
//! What is deliberately **not** configurable: the 180000 ms true-idle threshold and
//! the eight-hour lifetime margins. They are pinned constants in
//! `aex-runtime-control`. HR-21 puts jitter on the evaluation *schedule* and never
//! on the decision, and a deployment-tunable threshold would make the boundary a
//! property of an environment variable rather than of the model.

use aex_runtime_control::catalog::{HandsImageCatalog, HandsImageCatalogEntry};
use aex_runtime_control::store::PageBudget;
use aex_wire::types::Region;

/// Environment variable naming the deployment plane.
pub const PLANE_VAR: &str = "AEX_PLANE";
/// Environment variable naming the bound `AWS` region.
pub const REGION_VAR: &str = "AEX_REGION";
/// Environment variable naming the one AWS account this process may control.
pub const ACCOUNT_ID_VAR: &str = "AEX_ACCOUNT_ID";
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
/// Environment variable containing the eight immutable Hands image identities.
pub const IMAGE_CATALOG_VAR: &str = "AEX_HANDS_IMAGE_CATALOG";
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
    ACCOUNT_ID_VAR,
    RUNTIME_ACTIVITY_TABLE_VAR,
    SESSION_AUTHORITY_TABLE_VAR,
    LIFECYCLE_QUEUE_VAR,
    COMPUTE_QUEUE_VAR,
    STORAGE_QUEUE_VAR,
    IMAGE_CATALOG_VAR,
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

/// Largest due page that fits one bounded 30-second provider-await wave.
pub const MAX_DUE_PAGE_ITEMS: u32 = 32;

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
    /// The account every image ARN must name.
    pub account_id: String,
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
    /// Optional endpoint override for an explicit test or compatibility endpoint.
    /// Production normally uses the official SDK region endpoint.
    pub provider_endpoint: Option<String>,
    /// The exact published release catalog.
    pub image_catalog: HandsImageCatalog,
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
        let account_id = required(&lookup, ACCOUNT_ID_VAR)?;
        if account_id.len() != 12 || !account_id.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(ConfigError::Invalid {
                name: ACCOUNT_ID_VAR,
                reason: "expected exactly twelve decimal digits".to_owned(),
            });
        }
        let image_catalog = catalog(&lookup, &plane, region, &account_id)?;
        let page_items = positive(&lookup, DUE_PAGE_ITEMS_VAR)?;
        if page_items > MAX_DUE_PAGE_ITEMS {
            return Err(ConfigError::Invalid {
                name: DUE_PAGE_ITEMS_VAR,
                reason: format!(
                    "must be at most {MAX_DUE_PAGE_ITEMS} so one due page fits one provider-await wave"
                ),
            });
        }
        let config = Self {
            plane,
            region,
            account_id,
            runtime_activity_table: required(&lookup, RUNTIME_ACTIVITY_TABLE_VAR)?,
            session_authority_table: required(&lookup, SESSION_AUTHORITY_TABLE_VAR)?,
            lifecycle_queue_url: endpoint(&lookup, LIFECYCLE_QUEUE_VAR, region)?,
            compute_queue_url: endpoint(&lookup, COMPUTE_QUEUE_VAR, region)?,
            storage_queue_url: endpoint(&lookup, STORAGE_QUEUE_VAR, region)?,
            provider_endpoint: lookup(PROVIDER_ENDPOINT_VAR)
                .map(|_| endpoint(&lookup, PROVIDER_ENDPOINT_VAR, region))
                .transpose()?,
            image_catalog,
            due_shards: positive(&lookup, DUE_SHARDS_VAR)?,
            page: PageBudget {
                max_items: page_items,
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

/// Parses the closed release catalog and binds every provider ARN to this process.
fn catalog<F>(
    lookup: &F,
    plane: &str,
    region: Region,
    account_id: &str,
) -> Result<HandsImageCatalog, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let raw = required(lookup, IMAGE_CATALOG_VAR)?;
    let entries =
        serde_json::from_str::<std::collections::BTreeMap<String, HandsImageCatalogEntry>>(&raw)
            .map_err(|error| ConfigError::Invalid {
                name: IMAGE_CATALOG_VAR,
                reason: format!("expected the closed release JSON catalog: {error}"),
            })?;
    let catalog =
        HandsImageCatalog::from_entries(entries).map_err(|error| ConfigError::Invalid {
            name: IMAGE_CATALOG_VAR,
            reason: error.to_string(),
        })?;
    let prefix = format!(
        "arn:aws:lambda:{}:{account_id}:microvm-image:aex-{plane}-",
        region.as_str()
    );
    for identifier in catalog.image_identifiers() {
        let Some(suffix) = identifier.0.strip_prefix(&prefix) else {
            return Err(ConfigError::Invalid {
                name: IMAGE_CATALOG_VAR,
                reason: format!(
                    "image ARN `{}` is outside plane `{plane}`, account `{account_id}` or region `{}`",
                    identifier.0,
                    region.as_str()
                ),
            });
        };
        if suffix.len() != 52
            || !suffix
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || (b'2'..=b'7').contains(&byte))
        {
            return Err(ConfigError::Invalid {
                name: IMAGE_CATALOG_VAR,
                reason: format!("image ARN `{}` is not content-addressed", identifier.0),
            });
        }
    }
    Ok(catalog)
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
        ACCOUNT_ID_VAR, COMPUTE_QUEUE_VAR, Config, ConfigError, DUE_PAGE_ITEMS_VAR, DUE_SHARDS_VAR,
        FORBIDDEN_VARS, IMAGE_CATALOG_VAR, LIFECYCLE_QUEUE_VAR, PLANE_VAR, PROVIDER_ENDPOINT_VAR,
        REGION_VAR, REQUIRED_VARS, STORAGE_QUEUE_VAR,
    };
    use aex_wire::types::Region;
    use std::collections::BTreeMap;

    fn complete() -> BTreeMap<&'static str, String> {
        BTreeMap::from([
            (PLANE_VAR, "dev".to_owned()),
            (REGION_VAR, "eu-west-1".to_owned()),
            (ACCOUNT_ID_VAR, "522921482290".to_owned()),
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
            (
                IMAGE_CATALOG_VAR,
                catalog_json("dev", "eu-west-1", "522921482290"),
            ),
            (DUE_SHARDS_VAR, "8".to_owned()),
            (DUE_PAGE_ITEMS_VAR, "32".to_owned()),
            (super::DUE_PAGE_READS_VAR, "100".to_owned()),
            (super::PRICING_VERSION_VAR, "synthetic-zero-v1".to_owned()),
        ])
    }

    fn catalog_json(plane: &str, region: &str, account: &str) -> String {
        // The published set. Browser variants are excluded during prelaunch, so a
        // fixture carrying them describes no release this worker can be given.
        let variants = [
            ("512mb", 512, false),
            ("1gb", 1_024, false),
            ("2gb", 2_048, false),
            ("4gb", 4_096, false),
            ("8gb", 8_192, false),
        ];
        let rows = variants
            .into_iter()
            .enumerate()
            .map(|(index, (variant, memory, browser))| {
                (
                    variant,
                    serde_json::json!({
                        "imageArn": format!(
                            "arn:aws:lambda:{region}:{account}:microvm-image:aex-{plane}-{}",
                            char::from(b'a' + u8::try_from(index).expect("eight rows")).to_string().repeat(52),
                        ),
                        "imageVersion": (index + 1).to_string(),
                        "artifactDigest": format!("sha256:{index:064x}"),
                        "minimumMemoryMiB": memory,
                        "browser": browser,
                    }),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        serde_json::to_string(&rows).expect("catalog JSON")
    }

    fn read(vars: &BTreeMap<&'static str, String>) -> Result<Config, ConfigError> {
        Config::from_lookup(|name| vars.get(name).cloned())
    }

    #[test]
    fn a_complete_environment_is_accepted_and_fully_resolved() {
        let config = read(&complete()).expect("a complete environment is accepted");
        assert_eq!(config.plane, "dev");
        assert_eq!(config.region, Region::EuWest1);
        assert_eq!(config.account_id, "522921482290");
        assert_eq!(config.image_catalog.image_identifiers().len(), 5);
        assert_eq!(config.runtime_activity_table, "aex-dev-runtime-activity");
        assert_eq!(config.due_shards, 8);
        assert_eq!(config.page.max_items, 32);
        assert_eq!(config.page.max_reads, 100);
    }

    #[test]
    fn every_required_variable_is_named_when_it_is_missing() {
        assert_eq!(
            REQUIRED_VARS.len(),
            complete().len() - 1,
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
    fn provider_endpoint_override_is_optional_and_the_sdk_default_is_preferred() {
        let mut vars = complete();
        vars.remove(PROVIDER_ENDPOINT_VAR);
        let config = read(&vars).expect("the official regional SDK endpoint is sufficient");
        assert!(config.provider_endpoint.is_none());
        assert!(!REQUIRED_VARS.contains(&PROVIDER_ENDPOINT_VAR));
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
    fn the_catalog_is_exact_and_bound_to_plane_account_and_region() {
        for (name, replacement) in [
            (PLANE_VAR, "prd"),
            (REGION_VAR, "us-east-1"),
            (ACCOUNT_ID_VAR, "000000000000"),
        ] {
            let mut vars = complete();
            vars.insert(name, replacement.to_owned());
            assert!(matches!(
                read(&vars),
                Err(ConfigError::Invalid {
                    name: IMAGE_CATALOG_VAR,
                    ..
                })
            ));
        }

        let mut partial = complete();
        let mut catalog: serde_json::Value =
            serde_json::from_str(&partial[IMAGE_CATALOG_VAR]).expect("catalog");
        catalog.as_object_mut().expect("object").remove("8gb");
        partial.insert(IMAGE_CATALOG_VAR, catalog.to_string());
        assert!(matches!(
            read(&partial),
            Err(ConfigError::Invalid {
                name: IMAGE_CATALOG_VAR,
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
    fn a_due_page_larger_than_one_provider_await_wave_is_refused() {
        assert_eq!(
            usize::try_from(super::MAX_DUE_PAGE_ITEMS).expect("32 fits usize"),
            aex_runtime_control_aws::worker::ITEM_CONCURRENCY,
            "the config ceiling and the engine's bounded wave must move together"
        );
        let mut vars = complete();
        vars.insert(DUE_PAGE_ITEMS_VAR, "33".to_owned());
        assert!(matches!(
            read(&vars),
            Err(ConfigError::Invalid {
                name: DUE_PAGE_ITEMS_VAR,
                ..
            })
        ));
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
