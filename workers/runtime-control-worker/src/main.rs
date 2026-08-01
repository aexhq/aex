//! `runtime-control-worker` composition root (Rust Lambda ZIP).
//!
//! Exclusive responsibility: the exact Hands generation lifecycle — the due-index
//! reaper, true-idle suspend, resume, terminate, active eight-hour lifetime
//! enforcement, and the usage receipts each closed interval produces.
//!
//! It is the **only** ordinary `MicroVM` control role. Nothing else suspends,
//! resumes or terminates a generation, which is what makes one durable suspend
//! fence sufficient and what makes a worker outage safe: it delays cost saving and
//! raises an alarm, and it can never pause an authority-open background job.
//!
//! This binary is a composition root only. Configuration is validated before
//! anything starts, telemetry is installed through `aex_platform_telemetry`, and
//! every decision lives in `aex-runtime-control` and `aex-runtime-control-aws`.

mod config;
mod event;
mod health;

use std::sync::Arc;

use aex_hands_control_aws::provider::MicrovmControlApi;
use aex_runtime_control::store::{OpenEffectCounter, RuntimeActivityStore};
use aex_runtime_control::usage::UsageFactSink;
use aex_runtime_control_aws::worker::{Pace, RuntimeControl, RuntimePorts, RuntimeSettings};
use aex_wire::types::Timestamp;
use config::{Config, ConfigError};
use health::{Bindings, Dependency};

/// Jitter added to an evaluation schedule.
///
/// Drawn per invocation and clamped by the model. It applies to the *schedule*,
/// never to the 180000 ms decision (HR-21).
const SCHEDULE_JITTER_MS: u64 = 0;

/// Why `runtime-control-worker` stopped.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// Start-up configuration was rejected.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// A declared port has no adapter bound.
    ///
    /// The worker refuses to start rather than serving with a port missing: a
    /// lifecycle worker that cannot recount open effects would suspend running
    /// jobs, and one that cannot reach a usage ingress would terminate generations
    /// whose receipts are never written.
    #[error("no adapter is bound for: {}", .missing.join(", "))]
    Unbound {
        /// The ports with no adapter.
        missing: Vec<&'static str>,
    },
    /// The Lambda runtime stopped.
    #[error("the Lambda runtime stopped: {reason}")]
    Runtime {
        /// Why.
        reason: String,
    },
}

/// Real elapsed time between provider polls.
struct TokioPace;

impl Pace for TokioPace {
    fn sleep(&self, millis: u64) -> core::pin::Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(tokio::time::sleep(core::time::Duration::from_millis(
            millis,
        )))
    }
}

/// The adapters this composition binds.
///
/// Each field is an `Option` because each is implemented by a package another
/// stream ships, and this run deploys and credentials nothing (OD-07). An absent
/// adapter is named by `readyz` and refuses the start; it is never substituted by
/// a stub that would make the worker look healthy while suspending nothing.
///
/// `TODO(cross-stream) regional stores`: `store` wants
/// `aex-runtime-activity-dynamodb` over `aex_runtime_control::store::RuntimeActivityStore`
/// — the crate currently declares its own trait of the same name and already
/// carries the `TODO` saying so. `effects` wants the bounded strongly-consistent
/// open-Hands-effect query in `aex-session-dynamodb`.
///
/// `TODO(cross-stream) hands`: `provider` wants the `MicrovmControlApi`
/// implementation, which is blocked on whether `aws-sdk-lambdamicrovms` exists;
/// the seam and its narrow-SigV4 fallback are recorded in plan 10 §14.
///
/// `TODO(cross-stream) usage`: `compute` and `storage` want the compute and
/// storage category ingresses. There is deliberately no third field.
#[derive(Default)]
pub struct Adapters {
    /// The runtime-activity authority.
    pub store: Option<Arc<dyn RuntimeActivityStore>>,
    /// The authoritative open-Hands-effect count.
    pub effects: Option<Arc<dyn OpenEffectCounter>>,
    /// The trusted provider control plane.
    pub provider: Option<Arc<dyn MicrovmControlApi>>,
    /// The compute-authority usage ingress.
    pub compute: Option<Arc<dyn UsageFactSink>>,
    /// The storage-authority usage ingress.
    pub storage: Option<Arc<dyn UsageFactSink>>,
}

impl core::fmt::Debug for Adapters {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Adapters")
            .field("bound", &self.bindings())
            .finish()
    }
}

impl Adapters {
    /// Which dependencies are bound.
    #[must_use]
    pub fn bindings(&self) -> Bindings {
        let present = [
            (Dependency::RuntimeActivity, self.store.is_some()),
            (Dependency::SessionEffects, self.effects.is_some()),
            (Dependency::MicrovmControl, self.provider.is_some()),
            (Dependency::ComputeSink, self.compute.is_some()),
            (Dependency::StorageSink, self.storage.is_some()),
        ];
        present
            .into_iter()
            .filter(|(_, bound)| *bound)
            .fold(Bindings::default(), |bindings, (dependency, _)| {
                bindings.with(dependency)
            })
    }

    /// Composes the engine, or names every port that has no adapter.
    ///
    /// # Errors
    ///
    /// Returns [`RunError::Unbound`] naming every missing port.
    pub fn compose(self, config: &Config) -> Result<RuntimeControl, RunError> {
        let health::Readiness::Ready = self.bindings().readiness() else {
            let health::Readiness::NotReady { missing } = self.bindings().readiness() else {
                unreachable!("readiness is one of exactly two shapes");
            };
            return Err(RunError::Unbound { missing });
        };
        let (Some(store), Some(effects), Some(provider), Some(compute), Some(storage)) = (
            self.store,
            self.effects,
            self.provider,
            self.compute,
            self.storage,
        ) else {
            unreachable!("readiness is derived from exactly these five options");
        };
        Ok(RuntimeControl::new(
            RuntimePorts {
                store,
                effects,
                provider,
                compute,
                storage,
                pace: Arc::new(TokioPace),
            },
            RuntimeSettings {
                region: config.region,
                pricing_version: aex_internal_contracts::PricingVersion(
                    config.pricing_version.clone(),
                ),
                shards: config.due_shards,
                page: config.page,
                schedule_jitter_ms: SCHEDULE_JITTER_MS,
            },
        ))
    }
}

/// Resolves the adapters available to this build.
///
/// Nothing is bound today, and that is stated rather than hidden: every port is
/// owned by a package another stream ships, no AWS client is constructed, and no
/// credential is read. The moment a peer's adapter lands, each becomes one line.
fn resolve(_config: &Config) -> Adapters {
    Adapters::default()
}

/// Runs `runtime-control-worker` until it stops.
///
/// # Errors
///
/// Returns [`RunError::Unbound`] when a declared port has no adapter, and
/// [`RunError::Runtime`] when the Lambda runtime stops.
pub async fn run(
    config: &Config,
    telemetry: &aex_platform_telemetry::Handle,
) -> Result<(), RunError> {
    telemetry.emit(
        aex_platform_telemetry::Record::event(
            aex_telemetry_schema::generated::EVENT_AEX_PROCESS_STARTED,
        )
        .with(
            aex_telemetry_schema::generated::AEX_DEPLOYABLE,
            "runtime-control-worker",
        )
        .with(
            aex_telemetry_schema::generated::AEX_PLANE,
            config.plane.clone(),
        )
        .with(
            aex_telemetry_schema::generated::AEX_REGION,
            config.region.as_str(),
        ),
    );
    let adapters = resolve(config);
    let bindings = Arc::new(adapters.bindings());
    let control = Arc::new(adapters.compose(config)?);
    lambda_runtime::run(lambda_runtime::service_fn(
        move |invocation: lambda_runtime::LambdaEvent<serde_json::Value>| {
            let control = Arc::clone(&control);
            let bindings = Arc::clone(&bindings);
            async move { serve(&control, &bindings, &invocation.payload).await }
        },
    ))
    .await
    .map_err(|error| RunError::Runtime {
        reason: error.to_string(),
    })
}

/// Serves one invocation.
async fn serve(
    control: &RuntimeControl,
    bindings: &Bindings,
    payload: &serde_json::Value,
) -> Result<serde_json::Value, lambda_runtime::Error> {
    let decoded = event::WorkerEvent::decode(payload)
        .map_err(|error| lambda_runtime::Error::from(error.to_string()))?;
    let instant = now().map_err(|error| lambda_runtime::Error::from(error.to_string()))?;
    let handled = event::handle(control, bindings, &decoded, instant)
        .await
        .map_err(|error| lambda_runtime::Error::from(error.to_string()))?;
    for item in &handled.quarantined {
        // An operator record, on the one channel a Lambda always has. A poison item
        // is never redriven hot, so this line is the only thing that will say it
        // existed.
        eprintln!(
            "runtime-control-worker: quarantined message {} after {} deliveries: {}",
            item.message_id, item.receive_count, item.reason
        );
    }
    Ok(handled.response)
}

/// The host clock could not be read as a wire instant.
///
/// Never defaulted. A lifecycle decision taken at the wrong instant suspends a
/// running generation or terminates one an hour early, so an unreadable clock
/// fails the invocation instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the host clock is not a representable wire instant")]
pub struct ClockError;

/// The wall clock, read exactly once per invocation.
///
/// # Errors
///
/// See [`ClockError`].
fn now() -> Result<Timestamp, ClockError> {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| ClockError)?;
    let millis = i64::try_from(since.as_millis()).map_err(|_| ClockError)?;
    Timestamp::from_unix_millis(millis).map_err(|_| ClockError)
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("runtime-control-worker: refusing to start: {error}");
            eprintln!(
                "runtime-control-worker: required variables are {}",
                config::REQUIRED_VARS.join(", ")
            );
            return std::process::ExitCode::FAILURE;
        }
    };
    let settings = aex_platform_telemetry::Settings::default();
    let telemetry = aex_platform_telemetry::Handle::install(&settings, None);
    let outcome = run(&config, &telemetry).await;
    if let aex_platform_telemetry::FlushOutcome::DeadlineExceeded { pending } =
        telemetry.flush(settings.flush_deadline)
    {
        eprintln!("runtime-control-worker: telemetry flush left {pending} record(s) undelivered");
    }
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("runtime-control-worker: stopped: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Adapters, RunError, config::Config, health::Readiness, now, resolve};
    use std::collections::BTreeMap;

    fn config() -> Config {
        let vars = BTreeMap::from([
            ("AEX_PLANE", "dev"),
            ("AEX_REGION", "eu-west-1"),
            ("AEX_RUNTIME_ACTIVITY_TABLE", "aex-dev-runtime-activity"),
            ("AEX_SESSION_AUTHORITY_TABLE", "aex-dev-session-authority"),
            (
                "AEX_RUNTIME_LIFECYCLE_QUEUE_URL",
                "https://sqs.eu-west-1.amazonaws.com/1/lifecycle",
            ),
            (
                "AEX_USAGE_COMPUTE_QUEUE_URL",
                "https://sqs.eu-west-1.amazonaws.com/1/compute",
            ),
            (
                "AEX_USAGE_STORAGE_QUEUE_URL",
                "https://sqs.eu-west-1.amazonaws.com/1/storage",
            ),
            (
                "AEX_MICROVM_CONTROL_ENDPOINT",
                "https://lambda.eu-west-1.amazonaws.com",
            ),
            ("AEX_HANDS_IMAGE_IDENTIFIER", "aex-hands-1gb"),
            ("AEX_RUNTIME_DUE_SHARDS", "8"),
            ("AEX_RUNTIME_DUE_PAGE_ITEMS", "50"),
            ("AEX_RUNTIME_DUE_PAGE_READS", "100"),
            ("AEX_PRICING_VERSION", "synthetic-zero-v1"),
        ]);
        Config::from_lookup(|name| vars.get(name).map(|value| (*value).to_owned()))
            .expect("the fixture environment is complete")
    }

    #[test]
    fn an_unbound_port_refuses_the_start_and_says_which_one() {
        let error = resolve(&config())
            .compose(&config())
            .expect_err("nothing is bound in this build");
        let RunError::Unbound { missing } = error else {
            panic!("an unbound port is its own error class, not a generic failure");
        };
        assert_eq!(
            missing,
            vec![
                "runtime-activity",
                "session-open-effects",
                "microvm-control",
                "usage-compute-sink",
                "usage-storage-sink"
            ],
            "every missing port is named, so readiness and the start refusal agree"
        );
    }

    #[test]
    fn readiness_and_the_composition_read_the_same_five_options() {
        let adapters = Adapters::default();
        let Readiness::NotReady { missing } = adapters.bindings().readiness() else {
            panic!("an empty adapter set is not ready");
        };
        assert_eq!(
            missing.len(),
            5,
            "the readiness surface and the composition cannot disagree about the port count"
        );
        // Debug renders the binding state rather than the adapters themselves,
        // because a bound adapter may hold a client with credentials in it.
        assert!(format!("{adapters:?}").starts_with("Adapters { bound:"));
    }

    #[test]
    fn the_invocation_clock_is_read_rather_than_defaulted() {
        let first = now().expect("the host clock is a representable wire instant");
        let second = now().expect("the host clock is a representable wire instant");
        assert!(
            second.unix_millis() >= first.unix_millis(),
            "the invocation clock is monotone within one process"
        );
        assert!(
            first.unix_millis() > 1_700_000_000_000,
            "a defaulted epoch would take every lifecycle decision eight hours in the past"
        );
    }
}
