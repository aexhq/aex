//! `brain-mux` composition root (Rust Fargate OCI).
//!
//! Exclusive responsibility: activations, providers, managed tools, MCP, subagents, the
//! Hands adapter and the warm fold cache.
//!
//! This binary is a composition root only. Configuration is validated before anything
//! starts, telemetry is installed through `aex_platform_telemetry`, and the behaviour
//! itself lives in the library crates this deployable composes.

pub mod admission;
pub mod cache;
pub mod compose;
pub mod control;
pub mod drain;
pub mod health;
pub mod measure;
pub mod release_catalog;
pub mod runtime;
pub mod scale;
pub mod task_shape;
pub mod wake;

/// The composed loop's own assertions: one turn end to end, the drain order, and A11-MUX
/// held through the composition rather than only inside the probe.
#[cfg(test)]
#[path = "wake_tests.rs"]
mod wake_tests;

/// Validated start-up configuration for `brain-mux`.
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
    /// The session-authority table holding Brain journals and effects.
    pub resource: String,
    /// The queue Brain wakes are delivered on.
    pub wake_queue_url: String,
    /// The `regional-work` table the due backstop reads.
    pub work_table: String,
    /// The regional custody table holding provider bindings and sealed generations.
    pub secret_custody_table: String,
    /// The exact KMS root key ARN wrapping workspace branch keys.
    pub secret_kms_key_arn: String,
    /// Regional content bucket holding immutable fold snapshots.
    pub content_bucket: String,
    /// Account that must own the regional content bucket.
    pub content_expected_owner: String,
    /// Exact KMS key ARN used by the regional content bucket.
    pub content_kms_key_arn: String,
    /// Runtime-activity table used by Hands.
    pub runtime_activity_table: String,
    /// Compute-authority usage ingress used by runtime-control.
    pub usage_compute_queue_url: String,
    /// Storage-authority usage ingress used by runtime-control.
    pub usage_storage_queue_url: String,
    /// Runtime due-index shard count.
    pub runtime_due_shards: u16,
    /// Runtime due scan budget.
    pub runtime_due_page: aex_runtime_control::store::PageBudget,
    /// Exact pricing version attached to runtime usage drafts.
    pub pricing_version: String,
    /// Maximum concurrently active activations for one task.
    pub budget: u32,
}

/// Why `brain-mux` refused to start.
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

/// Why `brain-mux` stopped.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// Start-up configuration was rejected.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// The configuration does not compose into a usable process.
    #[error(transparent)]
    Composition(#[from] compose::CompositionError),
    /// A runtime or the health listener could not be created.
    #[error("brain-mux could not start: {reason}")]
    Runtime {
        /// What failed.
        reason: String,
    },
    /// Graceful drain could not prove every admitted activation stopped.
    #[error(transparent)]
    Drain(#[from] drain::DrainError),
    /// The production tool peer set is incomplete or invalid.
    #[error(transparent)]
    Tools(#[from] wake::ToolCompositionError),
}

/// Environment variable naming the deployment plane.
pub const PLANE_VAR: &str = "AEX_PLANE";
/// Environment variable naming the bound `AWS` region.
pub const REGION_VAR: &str = "AEX_REGION";
/// Environment variable naming the session-authority table holding Brain journals and effects.
pub const RESOURCE_VAR: &str = "AEX_BRAIN_JOURNAL_TABLE";
/// Environment variable naming the queue Brain wakes are delivered on.
///
/// Required, like every other resource name here. A defaulted queue URL binds the process to
/// somebody else's work, and the symptom is stolen wakes rather than a start-up failure.
pub const WAKE_QUEUE_VAR: &str = "AEX_BRAIN_WAKE_QUEUE_URL";
/// Environment variable naming the `regional-work` table the due backstop reads.
pub const WORK_TABLE_VAR: &str = "AEX_WORK_TABLE";
/// Environment variable naming the regional secret-custody table.
pub const SECRET_CUSTODY_TABLE_VAR: &str = "AEX_SECRET_CUSTODY_TABLE";
/// Environment variable naming the root KMS key for workspace branch keys.
pub const SECRET_KMS_KEY_ARN_VAR: &str = "AEX_SECRET_KMS_KEY_ARN";
/// Environment variable naming the regional content bucket.
pub const CONTENT_BUCKET_VAR: &str = "AEX_CONTENT_BUCKET";
/// Environment variable naming the account that must own the content bucket.
pub const CONTENT_EXPECTED_OWNER_VAR: &str = "AEX_CONTENT_EXPECTED_OWNER";
/// Environment variable naming the regional content KMS key.
pub const CONTENT_KMS_KEY_ARN_VAR: &str = "AEX_CONTENT_KMS_KEY_ARN";
/// Environment variable naming the runtime-activity table.
pub const RUNTIME_ACTIVITY_TABLE_VAR: &str = "AEX_RUNTIME_ACTIVITY_TABLE";
/// Environment variable naming the compute usage ingress.
pub const USAGE_COMPUTE_QUEUE_VAR: &str = "AEX_USAGE_COMPUTE_QUEUE_URL";
/// Environment variable naming the storage usage ingress.
pub const USAGE_STORAGE_QUEUE_VAR: &str = "AEX_USAGE_STORAGE_QUEUE_URL";
/// Environment variable naming the runtime due-index shard count.
pub const RUNTIME_DUE_SHARDS_VAR: &str = "AEX_RUNTIME_DUE_SHARDS";
/// Environment variable naming the maximum items in one runtime due scan.
pub const RUNTIME_DUE_PAGE_ITEMS_VAR: &str = "AEX_RUNTIME_DUE_PAGE_ITEMS";
/// Environment variable naming the maximum reads in one runtime due scan.
pub const RUNTIME_DUE_PAGE_READS_VAR: &str = "AEX_RUNTIME_DUE_PAGE_READS";
/// Environment variable naming the exact runtime pricing version.
pub const PRICING_VERSION_VAR: &str = "AEX_PRICING_VERSION";
/// Environment variable naming maximum concurrently active activations for one task.
pub const BUDGET_VAR: &str = "AEX_MAX_ACTIVE_ACTIVATIONS";

/// Planes this deployable may be bound to.
const PLANES: [&str; 2] = ["dev", "prd"];

impl Config {
    /// Reads and validates the configuration of `brain-mux` from the process environment.
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
        if !aex_wire::types::Region::ALL
            .iter()
            .any(|candidate| candidate.as_str() == region)
        {
            return Err(ConfigError::Invalid {
                name: REGION_VAR,
                reason: format!("unsupported regional placement `{region}`"),
            });
        }
        let resource = required(&lookup, RESOURCE_VAR)?;
        let wake_queue_url = required(&lookup, WAKE_QUEUE_VAR)?;
        let work_table = required(&lookup, WORK_TABLE_VAR)?;
        let secret_custody_table = required(&lookup, SECRET_CUSTODY_TABLE_VAR)?;
        let secret_kms_key_arn = required(&lookup, SECRET_KMS_KEY_ARN_VAR)?;
        validate_kms_arn(SECRET_KMS_KEY_ARN_VAR, &secret_kms_key_arn, &region)?;
        let content_bucket = required(&lookup, CONTENT_BUCKET_VAR)?;
        let content_expected_owner = required(&lookup, CONTENT_EXPECTED_OWNER_VAR)?;
        validate_account_id(CONTENT_EXPECTED_OWNER_VAR, &content_expected_owner)?;
        let content_kms_key_arn = required(&lookup, CONTENT_KMS_KEY_ARN_VAR)?;
        validate_kms_arn(CONTENT_KMS_KEY_ARN_VAR, &content_kms_key_arn, &region)?;
        let runtime_activity_table = required(&lookup, RUNTIME_ACTIVITY_TABLE_VAR)?;
        let usage_compute_queue_url = endpoint(&lookup, USAGE_COMPUTE_QUEUE_VAR, &region)?;
        let usage_storage_queue_url = endpoint(&lookup, USAGE_STORAGE_QUEUE_VAR, &region)?;
        if usage_compute_queue_url == usage_storage_queue_url {
            return Err(ConfigError::Invalid {
                name: USAGE_STORAGE_QUEUE_VAR,
                reason: "compute and storage usage authorities cannot share one queue".to_owned(),
            });
        }
        let runtime_due_shards = positive(&lookup, RUNTIME_DUE_SHARDS_VAR)?;
        let runtime_due_page_items = positive(&lookup, RUNTIME_DUE_PAGE_ITEMS_VAR)?;
        if runtime_due_page_items > 32 {
            return Err(ConfigError::Invalid {
                name: RUNTIME_DUE_PAGE_ITEMS_VAR,
                reason: "must be at most 32 so one due page fits one provider-await wave"
                    .to_owned(),
            });
        }
        let runtime_due_page_reads = positive(&lookup, RUNTIME_DUE_PAGE_READS_VAR)?;
        let pricing_version = required(&lookup, PRICING_VERSION_VAR)?;
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
            wake_queue_url,
            work_table,
            secret_custody_table,
            secret_kms_key_arn,
            content_bucket,
            content_expected_owner,
            content_kms_key_arn,
            runtime_activity_table,
            usage_compute_queue_url,
            usage_storage_queue_url,
            runtime_due_shards,
            runtime_due_page: aex_runtime_control::store::PageBudget {
                max_items: runtime_due_page_items,
                max_reads: runtime_due_page_reads,
            },
            pricing_version,
            budget,
        })
    }

    /// The typed plane bound into every provider-key encryption context.
    #[must_use]
    pub fn secret_plane(&self) -> aex_secret_domain::context::Plane {
        match self.plane.as_str() {
            "dev" => aex_secret_domain::context::Plane::Dev,
            "prd" => aex_secret_domain::context::Plane::Prd,
            _ => unreachable!("configuration admitted only the closed plane set"),
        }
    }

    /// The typed region bound into every provider-key encryption context.
    #[must_use]
    pub fn placement_region(&self) -> aex_wire::types::Region {
        aex_wire::types::Region::ALL
            .into_iter()
            .find(|candidate| candidate.as_str() == self.region)
            .unwrap_or_else(|| unreachable!("configuration admitted only the closed region set"))
    }

    /// The role/process partition for the zeroizing branch-key cache.
    #[must_use]
    pub fn credential_cache_partition(&self) -> String {
        format!(
            "{}:{}:brain-mux:{}",
            self.plane,
            self.region,
            std::process::id()
        )
    }
}

fn validate_kms_arn(name: &'static str, value: &str, region: &str) -> Result<(), ConfigError> {
    let parts = value.splitn(6, ':').collect::<Vec<_>>();
    let expected_partition = if region.starts_with("cn-") {
        "aws-cn"
    } else if region.starts_with("us-gov-") {
        "aws-us-gov"
    } else {
        "aws"
    };
    let valid = matches!(parts.as_slice(), ["arn", partition, "kms", found_region, account, resource]
        if *partition == expected_partition
            && *found_region == region
            && account.len() == 12
            && account.bytes().all(|byte| byte.is_ascii_digit())
            && resource.starts_with("key/")
            && resource.len() > "key/".len());
    if valid {
        Ok(())
    } else {
        Err(ConfigError::Invalid {
            name,
            reason: format!("expected a KMS key ARN in `{region}`"),
        })
    }
}

fn validate_account_id(name: &'static str, value: &str) -> Result<(), ConfigError> {
    if value.len() == 12 && value.bytes().all(|byte| byte.is_ascii_digit()) {
        Ok(())
    } else {
        Err(ConfigError::Invalid {
            name,
            reason: "expected a 12-digit AWS account id".to_owned(),
        })
    }
}

fn endpoint<F>(lookup: &F, name: &'static str, region: &str) -> Result<String, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let value = required(lookup, name)?;
    if !value.starts_with("https://") || !value.contains(region) {
        return Err(ConfigError::Invalid {
            name,
            reason: format!("expected an https endpoint in `{region}`, got `{value}`"),
        });
    }
    Ok(value)
}

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

fn required<F>(lookup: &F, name: &'static str) -> Result<String, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    match lookup(name) {
        Some(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(ConfigError::Missing { name }),
    }
}

/// The port the health responder listens on.
///
/// Part of the image contract rather than configuration: the ALB target group and the task
/// definition both name it, and a defaulted-but-configurable port is a value two places can
/// disagree about with no symptom until a deploy.
pub const HEALTH_PORT: u16 = 9_090;

/// Builds the composition this configuration describes.
///
/// # Errors
///
/// [`RunError::Composition`] when the admission bands or the memory split do not hold
/// together. Both fail startup rather than at the moment the over-commitment matters, which
/// is always the worst moment.
pub fn compose(config: &Config) -> Result<compose::Composition, RunError> {
    let bounds = admission::AdmissionBounds {
        target: config.budget,
        safety_cap: config.budget.saturating_mul(2),
        offered_ceiling: config.budget.saturating_mul(5),
    };
    let policy = aex_brain_application::activation::ActivationPolicy {
        receive_batch: 1,
        max_concurrent_drives: usize::try_from(config.budget).unwrap_or(usize::MAX),
        ..aex_brain_application::activation::ActivationPolicy::default()
    };
    compose::Composition::build(
        bounds,
        compose::Envelope::candidate_launch(),
        cache::CachePolicy::default(),
        scale::ScaleBounds {
            min_tasks: 1,
            max_tasks: 32,
            target_work_seconds_per_task: 10.0,
        },
        runtime::RuntimeShape::for_parallelism(task_shape::task_parallelism()),
        policy,
    )
    .map_err(RunError::Composition)
}

/// Runs `brain-mux` until it stops.
///
/// Production authorities resolve before any thread begins serving. Once resolved, the
/// dedicated control thread starts before work so reactor pressure cannot delay probes.
/// `SIGTERM` starts the drain sequence; the process exits zero once it has quiesced.
///
/// # Errors
///
/// [`RunError::Composition`] when the configuration does not compose, and
/// [`RunError::Runtime`] when a runtime or the health listener cannot be created.
pub fn run(config: &Config, telemetry: &aex_platform_telemetry::Handle) -> Result<(), RunError> {
    // Readiness starts false and is never defaulted true: a process that reported ready
    // before validating its bindings would admit work it cannot serve.
    let composition = std::sync::Arc::new(compose(config)?);
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

    let main_runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(composition.shape.worker_threads)
        .max_blocking_threads(composition.shape.max_blocking_threads)
        .enable_all()
        .thread_name("brain-mux-worker")
        .build()
        .map_err(|error| RunError::Runtime {
            reason: format!("the main runtime could not start: {error}"),
        })?;

    let PumpPorts { ports, bindings } = resolve_production_ports(config, &main_runtime)?;

    composition.health.schema_matched();
    composition.health.catalog_verified();
    composition.health.bindings_validated();

    // The control thread starts only after production composition succeeded. This prevents a
    // startup error from detaching a health thread that can never observe drain.
    let health = std::sync::Arc::clone(&composition.health);
    let control = std::thread::Builder::new()
        .name("brain-mux-control".to_owned())
        .spawn(move || serve_health(&health))
        .map_err(|error| RunError::Runtime {
            reason: format!("the control thread could not start: {error}"),
        })?;

    let drain_result = main_runtime.block_on(async {
        let sampler = tokio::spawn(sample_reactor_delay(std::sync::Arc::clone(&composition)));
        let pump = tokio::spawn(pump(
            std::sync::Arc::clone(&composition),
            config.clone(),
            telemetry.clone(),
            PumpPorts { ports, bindings },
        ));
        wait_for_shutdown().await;
        let stages = drain_sequence(&composition, pump).await;
        sampler.abort();
        let _ = sampler.await;
        stages
    });

    // The control thread stops when the health state reports drain, so joining it is how the
    // process proves it stopped answering rather than merely stopped listening.
    let _ = control.join();
    drain_result?;
    Ok(())
}

fn resolve_production_ports(
    config: &Config,
    runtime: &tokio::runtime::Runtime,
) -> Result<PumpPorts, RunError> {
    // One SDK configuration feeds every AWS client. Per-request tenant authority still comes
    // from each durable ticket; no workspace state is installed on the process.
    let aws = runtime.block_on(wake::aws_bindings(
        &config.region,
        &config.wake_queue_url,
        &config.resource,
        &config.work_table,
    ));
    let provider = wake::provider_binding(
        &aws.sdk,
        std::sync::Arc::clone(&aws.store),
        &config.secret_custody_table,
        &config.secret_kms_key_arn,
        config.secret_plane(),
        config.placement_region(),
        &config.credential_cache_partition(),
    )
    .map_err(|error| RunError::Runtime {
        reason: format!("production provider binding failed: {error}"),
    })?;
    let catalog = bind_release_catalog()?;
    let snapshots = wake::snapshot_binding(
        &aws.sdk,
        aex_brain_store_aws::BrainTables {
            session_authority: config.resource.clone(),
            regional_work: config.work_table.clone(),
        },
        wake::SnapshotBinding {
            bucket: config.content_bucket.clone(),
            expected_owner: config.content_expected_owner.clone(),
            kms_key_arn: config.content_kms_key_arn.clone(),
            plane: config.plane.clone(),
            region: config.region.clone(),
        },
    )
    .map_err(|error| RunError::Runtime {
        reason: format!("production fold-snapshot binding failed: {error}"),
    })?;
    let hands = wake::hands_binding(
        &aws.sdk,
        wake::HandsBinding {
            region: config.placement_region(),
            runtime_activity_table: config.runtime_activity_table.clone(),
            session_authority_table: config.resource.clone(),
            compute_queue_url: config.usage_compute_queue_url.clone(),
            storage_queue_url: config.usage_storage_queue_url.clone(),
            due_shards: config.runtime_due_shards,
            due_page: config.runtime_due_page,
            pricing_version: config.pricing_version.clone(),
        },
    )
    .map_err(|error| RunError::Runtime {
        reason: format!("production Hands binding failed: {error}"),
    })?;

    // These authorities do not yet have safe production implementations. Their absence is
    // a startup error, never a refusal executor installed behind a ready task. Hands is
    // included to prove the available route is wired while the other three are named.
    let tools = wake::ProductionToolExecutors {
        brain_inline: None,
        managed_web: None,
        mcp: None,
        hands: Some(std::sync::Arc::clone(&hands.executor)),
    }
    .compose(catalog.retained_pins())?;
    let peers = wake::ProductionPeers::new(
        provider,
        tools,
        hands.backend,
        std::sync::Arc::clone(&catalog) as std::sync::Arc<_>,
        snapshots,
    );
    let ports = wake::production_ports(aws.store, aws.queue, peers);
    Ok(PumpPorts {
        ports,
        bindings: wake::Bindings::production(),
    })
}

fn bind_release_catalog()
-> Result<std::sync::Arc<aex_brain_provider_gateway::catalog_port::VerifiedCatalogPort>, RunError> {
    // Catalog authority is build/release scoped, never tenant or runtime-env
    // scoped. The exact collection and bounded publisher trust-root set are
    // compiled together and the entire retained chain verifies before lookup.
    let now = aex_wire::types::Timestamp::from_datetime_trunc_ms(time::OffsetDateTime::now_utc())
        .map_err(|error| RunError::Runtime {
        reason: format!("the startup clock is outside the catalog timestamp range: {error}"),
    })?;
    release_catalog::load(now)
        .map(std::sync::Arc::new)
        .map_err(|error| RunError::Runtime {
            reason: format!("production model-catalog binding failed: {error}"),
        })
}

struct PumpPorts {
    ports: aex_brain_application::activation::Ports,
    bindings: wake::Bindings,
}

/// Receives wakes and drives them until drain starts.
///
/// The loop asks admission before every receive. Startup constructs this value only from a
/// complete production peer set; no partial port set can reach this function.
async fn pump(
    composition: std::sync::Arc<compose::Composition>,
    config: Config,
    telemetry: aex_platform_telemetry::Handle,
    ports: PumpPorts,
) {
    let mut policy = composition.policy.clone();
    let aggregate_cap = policy.max_concurrent_drives.max(1);
    // One receive scope owns one activation slot. The outer scheduler owns the aggregate
    // target, so nested batch concurrency remains one and cannot multiply that target.
    policy.max_concurrent_drives = 1;
    let pump = wake::wake_loop(
        ports.ports,
        policy,
        std::sync::Arc::clone(&composition.registry),
        std::sync::Arc::clone(&composition.drain),
        std::sync::Arc::clone(&composition.admission),
        ports.bindings,
    );
    run_wake_scheduler(
        pump,
        std::sync::Arc::clone(&composition.drain),
        aggregate_cap,
        |result| match result {
            Ok(report) => {
                emit_due_isolations(&telemetry, &config, report);
            }
            Err(error) => {
                eprintln!("brain-mux: the wake loop refused: {error}");
            }
        },
    )
    .await;
}

/// Continuously refills independently progressing receive scopes under one aggregate cap.
///
/// Each scope is a structured child future rather than a spawned task. The pump join owns
/// the whole set, so cooperative drain polls admitted activations to settlement and hard
/// abort drops every remaining receive or effect future before exit can be reported.
async fn run_wake_scheduler<F>(
    pump: aex_brain_application::activation::WakeLoop,
    drain: std::sync::Arc<aex_brain_application::kernel::DrainGate>,
    aggregate_cap: usize,
    mut observe: F,
) where
    F: FnMut(
        &Result<
            aex_brain_application::activation::PollReport,
            aex_brain_application::activation::ActivationError,
        >,
    ),
{
    use futures::stream::{FuturesUnordered, StreamExt as _};

    assert_eq!(
        pump.activation().policy().max_concurrent_drives,
        1,
        "each scheduler lane must own exactly one drive slot"
    );
    let aggregate_cap = aggregate_cap.max(1);
    let mut passes = FuturesUnordered::new();
    loop {
        while !drain.is_draining() && pump.receiving_allowed() && passes.len() < aggregate_cap {
            passes.push(poll_after(&pump, core::time::Duration::ZERO));
        }

        if passes.is_empty() {
            if drain.is_draining() {
                break;
            }
            // Bindings or admission currently refuse new receive scopes. Polling at the
            // reactor cadence makes a later capacity change and drain observable without a
            // busy loop or a second notification channel.
            tokio::time::sleep(compose::REACTOR_TICK).await;
            continue;
        }

        let result = passes
            .next()
            .await
            .expect("a non-empty scheduler has one receive scope");
        let retry_delay = match &result {
            Ok(report) if report.received > 0 => core::time::Duration::ZERO,
            Ok(_) | Err(_) => compose::REACTOR_TICK,
        };
        observe(&result);

        // A failed or empty lane backs off independently. Other lanes remain polled, so
        // one queue refusal cannot stall unrelated effects or the due-recovery cadence.
        if !drain.is_draining() && pump.receiving_allowed() {
            passes.push(poll_after(&pump, retry_delay));
        }
    }
}

async fn poll_after(
    pump: &aex_brain_application::activation::WakeLoop,
    delay: core::time::Duration,
) -> Result<
    aex_brain_application::activation::PollReport,
    aex_brain_application::activation::ActivationError,
> {
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }
    pump.poll_once().await
}

fn emit_due_isolations(
    telemetry: &aex_platform_telemetry::Handle,
    config: &Config,
    report: &aex_brain_application::activation::PollReport,
) {
    let count = u32::try_from(report.malformed).unwrap_or(u32::MAX);
    for isolation in &report.isolations {
        let reason = match isolation.reason {
            aex_brain_application::ports::DueRowIsolationReason::MalformedProjection => {
                "malformed_projection"
            }
            aex_brain_application::ports::DueRowIsolationReason::BaseKeyMismatch => {
                "base_key_mismatch"
            }
            aex_brain_application::ports::DueRowIsolationReason::ShardMismatch => "shard_mismatch",
            aex_brain_application::ports::DueRowIsolationReason::DuePositionMismatch => {
                "due_position_mismatch"
            }
        };
        telemetry.emit(
            aex_platform_telemetry::Record::event(
                aex_telemetry_schema::generated::EVENT_AEX_BRAIN_DUE_ROW_ISOLATED,
            )
            .with(
                aex_telemetry_schema::generated::AEX_PLANE,
                config.plane.clone(),
            )
            .with(
                aex_telemetry_schema::generated::AEX_REGION,
                config.region.clone(),
            )
            .with(aex_telemetry_schema::generated::AEX_DEPLOYABLE, "brain-mux")
            .with(aex_telemetry_schema::generated::AEX_ERROR_CLASS, reason)
            .with(aex_telemetry_schema::generated::AEX_ISOLATION_COUNT, count)
            .with(
                aex_telemetry_schema::generated::AEX_ISOLATION_FINGERPRINT,
                isolation.fingerprint.clone(),
            ),
        );
    }
}

/// Performs the seven-stage drain and returns the stages it completed.
///
/// The order is the whole design. Readiness fails first so the load balancer stops sending
/// work; liveness is deliberately untouched, because failing it would have the orchestrator
/// kill the task along with the non-replayable effects it is trying to finish.
///
/// # Errors
///
/// Returns the first drain-stage failure, including an activation pump that
/// panicked before it relinquished receive authority.
pub async fn drain_sequence(
    composition: &std::sync::Arc<compose::Composition>,
    pump: tokio::task::JoinHandle<()>,
) -> Result<Vec<drain::Stage>, drain::DrainError> {
    let mut performed = vec![composition.begin_drain()];

    // Closing the gate prevents every post-signal admission, including deliveries returned
    // by a long-poll already in progress. The join below proves the receive scope eventually
    // stopped; the stage records when its authority to receive was revoked.
    performed.push(drain::Stage::StopReceiving);

    // Replay-safe work is abandoned at once: its lease is released by the activation itself,
    // and a surviving task claims it in milliseconds.
    performed.push(drain::Stage::AbandonReplaySafe);

    // Dispatched non-replayable effects run to the commit margin. Cooperative adapters
    // preserve their dispatch proof: ambiguous work settles unknown, while `NotSent` may
    // re-arm. Anything still pending at the hard deadline is dropped only by the explicit
    // abort-and-join below and remains recoverable from its dispatch-started durable state.
    let deadline = tokio::time::Instant::now() + drain::STOP_TIMEOUT - drain::COMMIT_MARGIN;
    while !composition.is_quiesced() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(compose::REACTOR_TICK).await;
    }
    performed.push(drain::Stage::AwaitNonReplayable);

    // Keep the handle across timeout. Consuming it in `timeout` detaches the task when the
    // deadline expires, allowing the process to report Exit while the pump still owns live
    // activations. The commit margin is the last cooperative window; expiry explicitly
    // aborts and then joins the task, which drops every child future in this structured
    // scope before any later stage is reported.
    join_pump(pump, drain::COMMIT_MARGIN).await?;
    if !composition.is_quiesced() {
        return Err(drain::DrainError::ActivationsRemain {
            in_flight: composition.drain.in_flight(),
        });
    }

    // No activation future remains able to use a lease. Cooperative paths released their
    // exact claim; an explicitly aborted path cannot write again and its 15-second durable
    // lease expires normally. Quiescence is an in-process liveness proof, not a fabricated
    // claim that every best-effort release reached DynamoDB.
    performed.push(drain::Stage::ReleaseLeases);
    performed.push(drain::Stage::Flush);
    performed.push(drain::Stage::Exit);
    Ok(performed)
}

async fn join_pump(
    mut pump: tokio::task::JoinHandle<()>,
    grace: core::time::Duration,
) -> Result<(), drain::DrainError> {
    match tokio::time::timeout(grace, &mut pump).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            return Err(drain::DrainError::PumpFailed {
                reason: error.to_string(),
            });
        }
        Err(_) => {
            pump.abort();
            match pump.await {
                Err(error) if error.is_cancelled() => {}
                Ok(()) => {}
                Err(error) => {
                    return Err(drain::DrainError::PumpFailed {
                        reason: error.to_string(),
                    });
                }
            }
        }
    }
    Ok(())
}

/// Serves `/internal/healthz` and `/internal/readyz` until drain completes.
fn serve_health(health: &std::sync::Arc<control::HealthState>) {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        eprintln!("brain-mux: the control runtime could not start");
        return;
    };
    runtime.block_on(async {
        let Ok(listener) = tokio::net::TcpListener::bind(("0.0.0.0", HEALTH_PORT)).await else {
            eprintln!("brain-mux: the health listener could not bind port {HEALTH_PORT}");
            return;
        };
        loop {
            let accepted =
                tokio::time::timeout(core::time::Duration::from_millis(250), listener.accept())
                    .await;
            // A timeout is the loop's own heartbeat: it is how drain is noticed without a
            // second channel between the two schedulers.
            if let Ok(Ok((stream, _))) = accepted {
                answer(stream, health).await;
            }
            if health.is_draining() {
                return;
            }
        }
    });
}

async fn answer(mut stream: tokio::net::TcpStream, health: &std::sync::Arc<control::HealthState>) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    // Bounded on purpose: a probe request is a request line and a couple of headers, and a
    // responder that read an unbounded body would be a way to stall the one thread that
    // must never stall.
    let mut buffer = [0_u8; 1_024];
    let Ok(read) = stream.read(&mut buffer).await else {
        return;
    };
    let request = String::from_utf8_lossy(&buffer[..read]);
    let response = control::parse_request_line(&request).map_or_else(
        || control::HttpResponse {
            status: 400,
            body: "bad request".to_owned(),
        },
        |(method, path)| health.respond(method, path),
    );
    let _ = stream.write_all(response.render().as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// Samples the reactor's scheduling lateness.
///
/// A 100 ms tick that records how late it actually ran. It is the only way to observe the
/// reactor from inside it, and it is what makes "the reactor is wedged" a measurement rather
/// than an inference from unrelated symptoms.
async fn sample_reactor_delay(composition: std::sync::Arc<compose::Composition>) {
    let mut ticker = tokio::time::interval(compose::REACTOR_TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let expected = tokio::time::Instant::now() + compose::REACTOR_TICK;
        ticker.tick().await;
        let lateness = tokio::time::Instant::now().saturating_duration_since(expected);
        composition
            .health
            .observe_reactor_delay(u32::try_from(lateness.as_millis()).unwrap_or(u32::MAX));
        composition
            .health
            .observe_active(composition.admission.active());
    }
}

/// Resolves when the orchestrator asks the process to stop.
async fn wait_for_shutdown() {
    #[cfg(unix)]
    {
        let mut terminate =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(signal) => signal,
                Err(_) => {
                    let _ = tokio::signal::ctrl_c().await;
                    return;
                }
            };
        tokio::select! {
            _ = terminate.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn main() -> std::process::ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("brain-mux: refusing to start: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let settings = aex_platform_telemetry::Settings::default();
    let telemetry = aex_platform_telemetry::Handle::install(&settings, None);
    let outcome = run(&config, &telemetry);
    if let aex_platform_telemetry::FlushOutcome::DeadlineExceeded { pending } =
        telemetry.flush(settings.flush_deadline)
    {
        eprintln!("brain-mux: telemetry flush left {pending} record(s) undelivered");
    }
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("brain-mux: stopped: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BUDGET_VAR, CONTENT_BUCKET_VAR, CONTENT_EXPECTED_OWNER_VAR, CONTENT_KMS_KEY_ARN_VAR,
        Config, ConfigError, PLANE_VAR, PRICING_VERSION_VAR, REGION_VAR, RESOURCE_VAR,
        RUNTIME_ACTIVITY_TABLE_VAR, RUNTIME_DUE_PAGE_ITEMS_VAR, RUNTIME_DUE_PAGE_READS_VAR,
        RUNTIME_DUE_SHARDS_VAR, SECRET_CUSTODY_TABLE_VAR, SECRET_KMS_KEY_ARN_VAR,
        USAGE_COMPUTE_QUEUE_VAR, USAGE_STORAGE_QUEUE_VAR, WAKE_QUEUE_VAR, WORK_TABLE_VAR, compose,
        emit_due_isolations,
    };
    use aex_brain_application::activation::PollReport;
    use aex_brain_application::ports::{DueRowIsolation, DueRowIsolationReason};
    use aex_platform_telemetry::{AttributeValue, InMemoryExporter};
    use std::collections::BTreeMap;

    fn complete() -> BTreeMap<&'static str, String> {
        BTreeMap::from([
            (PLANE_VAR, "dev".to_owned()),
            (REGION_VAR, "eu-west-1".to_owned()),
            (RESOURCE_VAR, "aex-brain_mux-fixture".to_owned()),
            (
                WAKE_QUEUE_VAR,
                "https://sqs.eu-west-1.amazonaws.com/1/aex-brain-wake".to_owned(),
            ),
            (WORK_TABLE_VAR, "aex-regional-work-fixture".to_owned()),
            (
                SECRET_CUSTODY_TABLE_VAR,
                "aex-regional-secret-custody-fixture".to_owned(),
            ),
            (
                SECRET_KMS_KEY_ARN_VAR,
                "arn:aws:kms:eu-west-1:123456789012:key/fixture".to_owned(),
            ),
            (CONTENT_BUCKET_VAR, "aex-dev-content-fixture".to_owned()),
            (CONTENT_EXPECTED_OWNER_VAR, "123456789012".to_owned()),
            (
                CONTENT_KMS_KEY_ARN_VAR,
                "arn:aws:kms:eu-west-1:123456789012:key/content".to_owned(),
            ),
            (
                RUNTIME_ACTIVITY_TABLE_VAR,
                "aex-dev-runtime-activity".to_owned(),
            ),
            (
                USAGE_COMPUTE_QUEUE_VAR,
                "https://sqs.eu-west-1.amazonaws.com/1/aex-dev-usage-compute".to_owned(),
            ),
            (
                USAGE_STORAGE_QUEUE_VAR,
                "https://sqs.eu-west-1.amazonaws.com/1/aex-dev-usage-storage".to_owned(),
            ),
            (RUNTIME_DUE_SHARDS_VAR, "8".to_owned()),
            (RUNTIME_DUE_PAGE_ITEMS_VAR, "32".to_owned()),
            (RUNTIME_DUE_PAGE_READS_VAR, "100".to_owned()),
            (PRICING_VERSION_VAR, "synthetic-zero-v1".to_owned()),
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
        assert_eq!(config.resource, "aex-brain_mux-fixture");
        assert_eq!(
            config.wake_queue_url,
            "https://sqs.eu-west-1.amazonaws.com/1/aex-brain-wake"
        );
        assert_eq!(config.work_table, "aex-regional-work-fixture");
        assert_eq!(config.budget, 8);
    }

    /// The deployed target is executable capacity, not a tuning hint. The scheduler width
    /// and all per-activation pools prove 48 together; a 49th worst-case restore is refused
    /// before the process can receive a wake.
    #[test]
    fn composition_accepts_the_proven_target_and_refuses_one_more() {
        let mut vars = complete();
        vars.insert(BUDGET_VAR, "48".to_owned());
        let config = read(&vars).expect("the proven target parses");
        let composition = compose(&config).expect("the release task proves target 48");
        assert_eq!(composition.admission.bounds().target, 48);
        assert_eq!(composition.policy.max_concurrent_drives, 48);
        assert_eq!(composition.shape.worker_threads, 2);

        vars.insert(BUDGET_VAR, "49".to_owned());
        let config = read(&vars).expect("capacity is a composition concern");
        let error = compose(&config).expect_err("target 49 has no reserved restore capacity");
        assert!(error.to_string().contains("context capacity"), "{error}");
    }

    #[test]
    fn names_each_missing_variable() {
        for name in [
            PLANE_VAR,
            REGION_VAR,
            RESOURCE_VAR,
            WAKE_QUEUE_VAR,
            WORK_TABLE_VAR,
            SECRET_CUSTODY_TABLE_VAR,
            SECRET_KMS_KEY_ARN_VAR,
            CONTENT_BUCKET_VAR,
            CONTENT_EXPECTED_OWNER_VAR,
            CONTENT_KMS_KEY_ARN_VAR,
            RUNTIME_ACTIVITY_TABLE_VAR,
            USAGE_COMPUTE_QUEUE_VAR,
            USAGE_STORAGE_QUEUE_VAR,
            RUNTIME_DUE_SHARDS_VAR,
            RUNTIME_DUE_PAGE_ITEMS_VAR,
            RUNTIME_DUE_PAGE_READS_VAR,
            PRICING_VERSION_VAR,
            BUDGET_VAR,
        ] {
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
    fn rejects_a_region_outside_the_closed_placement_set() {
        let mut vars = complete();
        vars.insert(REGION_VAR, "eu-central-1".to_owned());
        assert!(matches!(
            read(&vars),
            Err(ConfigError::Invalid {
                name: REGION_VAR,
                ..
            })
        ));
    }

    #[test]
    fn rejects_a_kms_key_from_another_region() {
        let mut vars = complete();
        vars.insert(
            SECRET_KMS_KEY_ARN_VAR,
            "arn:aws:kms:us-east-1:123456789012:key/fixture".to_owned(),
        );
        assert!(matches!(
            read(&vars),
            Err(ConfigError::Invalid {
                name: SECRET_KMS_KEY_ARN_VAR,
                ..
            })
        ));
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

    #[test]
    fn due_isolation_samples_reach_structured_telemetry_without_raw_keys() {
        let exporter = std::sync::Arc::new(InMemoryExporter::new());
        let telemetry = aex_platform_telemetry::Handle::install(
            &aex_platform_telemetry::Settings::default(),
            Some(std::sync::Arc::clone(&exporter) as std::sync::Arc<_>),
        );
        let config = read(&complete()).expect("complete environment");
        let report = PollReport {
            malformed: 17,
            isolations: vec![DueRowIsolation {
                reason: DueRowIsolationReason::ShardMismatch,
                fingerprint: "0123456789abcdef".to_owned(),
            }],
            ..PollReport::default()
        };

        emit_due_isolations(&telemetry, &config, &report);
        let _ = telemetry.flush(core::time::Duration::from_secs(1));
        let delivered = exporter.delivered();
        assert_eq!(delivered.len(), 1);
        assert_eq!(
            delivered[0].name,
            aex_telemetry_schema::generated::EVENT_AEX_BRAIN_DUE_ROW_ISOLATED
        );
        assert_eq!(
            delivered[0].attribute(aex_telemetry_schema::generated::AEX_ERROR_CLASS),
            Some(&AttributeValue::Text("shard_mismatch".to_owned()))
        );
        assert_eq!(
            delivered[0].attribute(aex_telemetry_schema::generated::AEX_ISOLATION_COUNT),
            Some(&AttributeValue::Integer(17))
        );
        assert_eq!(
            delivered[0].attribute(aex_telemetry_schema::generated::AEX_ISOLATION_FINGERPRINT),
            Some(&AttributeValue::Text("0123456789abcdef".to_owned()))
        );
    }
}
