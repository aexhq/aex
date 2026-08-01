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
pub mod runtime;
pub mod scale;

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
}

/// Environment variable naming the deployment plane.
pub const PLANE_VAR: &str = "AEX_PLANE";
/// Environment variable naming the bound `AWS` region.
pub const REGION_VAR: &str = "AEX_REGION";
/// Environment variable naming the session-authority table holding Brain journals and effects.
pub const RESOURCE_VAR: &str = "AEX_BRAIN_JOURNAL_TABLE";
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
    compose::Composition::build(
        bounds,
        compose::Envelope::candidate_launch(),
        cache::CachePolicy::default(),
        scale::ScaleBounds {
            min_tasks: 1,
            max_tasks: 32,
            target_work_seconds_per_task: 10.0,
        },
        runtime::RuntimeShape::detected(),
    )
    .map_err(RunError::Composition)
}

/// Runs `brain-mux` until it stops.
///
/// Three schedulers, started in one order that matters: the control thread first, so the
/// process can answer a probe before it can do anything else, then the main runtime.
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

    // The control thread starts before anything that could saturate a scheduler. It runs a
    // current-thread runtime on its own OS thread precisely so no amount of work on the main
    // reactor can delay a probe (BC-20).
    let health = std::sync::Arc::clone(&composition.health);
    let control = std::thread::Builder::new()
        .name("brain-mux-control".to_owned())
        .spawn(move || serve_health(&health))
        .map_err(|error| RunError::Runtime {
            reason: format!("the control thread could not start: {error}"),
        })?;

    let main_runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(composition.shape.worker_threads)
        .max_blocking_threads(composition.shape.max_blocking_threads)
        .enable_all()
        .thread_name("brain-mux-worker")
        .build()
        .map_err(|error| RunError::Runtime {
            reason: format!("the main runtime could not start: {error}"),
        })?;

    // Configuration and the composition itself are the only bindings validated so far. The
    // catalog, the store and the schema hashes are each set by the component that proves
    // them; readiness stays false until every one of them has.
    composition.health.bindings_validated();

    main_runtime.block_on(async {
        let sampler = tokio::spawn(sample_reactor_delay(std::sync::Arc::clone(&composition)));
        wait_for_shutdown().await;
        let _stage = composition.begin_drain();
        sampler.abort();
    });

    // The control thread stops when the health state reports drain, so joining it is how the
    // process proves it stopped answering rather than merely stopped listening.
    let _ = control.join();
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
    use super::{BUDGET_VAR, Config, ConfigError, PLANE_VAR, REGION_VAR, RESOURCE_VAR};
    use std::collections::BTreeMap;

    fn complete() -> BTreeMap<&'static str, String> {
        BTreeMap::from([
            (PLANE_VAR, "dev".to_owned()),
            (REGION_VAR, "eu-west-1".to_owned()),
            (RESOURCE_VAR, "aex-brain_mux-fixture".to_owned()),
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
