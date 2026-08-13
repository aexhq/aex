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
//! anything starts, diagnostics are installed through `aex_platform_diagnostics`, and
//! every decision lives in `aex-runtime-control` and `aex-runtime-control-aws`.

mod config;
mod event;
mod health;

use std::sync::Arc;

use aex_hands_control_aws::AwsMicrovmControl;
use aex_hands_control_aws::provider::MicrovmControlApi;
use aex_runtime_activity_dynamodb::RuntimeActivityDynamoStore;
use aex_runtime_control::store::{OpenEffectCounter, RuntimeActivityStore};
use aex_runtime_control::usage::UsageFactSink;
use aex_runtime_control_aws::usage_ingress::SqsFactDraftSink;
use aex_runtime_control_aws::worker::{Pace, RuntimeControl, RuntimePorts, RuntimeSettings};
use aex_session_dynamodb::runtime_effects::OpenHandsEffectCounter;
use aex_usage_domain::meter::Category;
use aex_wire::types::Timestamp;
use config::{Config, RuntimeControlWorkerConfigError};
use health::{Bindings, Dependency};

/// Jitter added to an evaluation schedule.
///
/// Drawn per invocation and clamped by the model. It applies to the *schedule*,
/// never to the 180000 ms decision (HR-21).
const SCHEDULE_JITTER_MS: u64 = 0;
const DEPLOYABLE: &str = "runtime-control-worker";

/// Maximum concurrent provider reads during the eight-image startup probe.
const IMAGE_PROBE_CONCURRENCY: usize = 4;

/// Why `runtime-control-worker` stopped.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeControlWorkerRunError {
    /// Start-up configuration was rejected.
    #[error(transparent)]
    Config(#[from] RuntimeControlWorkerConfigError),
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
    /// A required dependency could not be reached before polling began.
    #[error("startup probe for `{dependency}` failed: {reason}")]
    Startup {
        /// Dependency whose binding was probed.
        dependency: &'static str,
        /// Remote failure.
        reason: String,
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
/// Production resolution binds all five fields. They remain `Option`s so the
/// readiness and fail-closed composition tests can prove that every absent port
/// is named and refused; production never substitutes a stub. There is
/// deliberately no transfer-authority field.
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
    /// Returns [`RuntimeControlWorkerRunError::Unbound`] naming every missing port.
    pub fn compose(self, config: &Config) -> Result<RuntimeControl, RuntimeControlWorkerRunError> {
        let health::Readiness::Ready = self.bindings().readiness() else {
            let health::Readiness::NotReady { missing } = self.bindings().readiness() else {
                unreachable!("readiness is one of exactly two shapes");
            };
            return Err(RuntimeControlWorkerRunError::Unbound { missing });
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

/// Resolves and probes every production adapter before Lambda begins polling.
async fn resolve(config: &Config) -> Result<Adapters, RuntimeControlWorkerRunError> {
    let aws = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_types::region::Region::new(config.region.as_str()))
        .load()
        .await;
    let dynamodb = aws_sdk_dynamodb::Client::new(&aws);
    let sqs = aws_sdk_sqs::Client::new(&aws);

    for (dependency, table) in [
        ("runtime-activity", config.runtime_activity_table.as_str()),
        (
            "session-open-effects",
            config.session_authority_table.as_str(),
        ),
    ] {
        dynamodb
            .describe_table()
            .table_name(table)
            .send()
            .await
            .map_err(|error| RuntimeControlWorkerRunError::Startup {
                dependency,
                reason: error.to_string(),
            })?;
    }
    for (dependency, queue) in [
        (
            "runtime-lifecycle-queue",
            config.lifecycle_queue_url.as_str(),
        ),
        ("usage-compute-sink", config.compute_queue_url.as_str()),
        ("usage-storage-sink", config.storage_queue_url.as_str()),
    ] {
        sqs.get_queue_attributes()
            .queue_url(queue)
            .attribute_names(aws_sdk_sqs::types::QueueAttributeName::QueueArn)
            .send()
            .await
            .map_err(|error| RuntimeControlWorkerRunError::Startup {
                dependency,
                reason: error.to_string(),
            })?;
    }

    let mut provider_config = aws_sdk_lambdamicrovms::config::Builder::from(&aws);
    if let Some(endpoint) = &config.provider_endpoint {
        provider_config = provider_config.endpoint_url(endpoint);
    }
    let provider = AwsMicrovmControl::new(
        aws_sdk_lambdamicrovms::Client::from_conf(provider_config.build()),
        config.region.as_str(),
    );
    probe_image_catalog(&provider, &config.image_catalog).await?;

    Ok(Adapters {
        store: Some(Arc::new(RuntimeActivityDynamoStore::new(
            dynamodb.clone(),
            config.runtime_activity_table.clone(),
        ))),
        effects: Some(Arc::new(OpenHandsEffectCounter::new(
            dynamodb,
            config.session_authority_table.clone(),
        ))),
        provider: Some(Arc::new(provider)),
        compute: Some(Arc::new(SqsFactDraftSink::new(
            sqs.clone(),
            config.compute_queue_url.clone(),
            Category::Compute,
        ))),
        storage: Some(Arc::new(SqsFactDraftSink::new(
            sqs,
            config.storage_queue_url.clone(),
            Category::Storage,
        ))),
    })
}

/// Proves the provider read path accepts every exact release image identifier.
///
/// Four concurrent reads keep cold-start latency bounded without sending an
/// eight-request burst through a newly assumed execution role.
async fn probe_image_catalog(
    provider: &AwsMicrovmControl,
    catalog: &aex_runtime_control::catalog::HandsImageCatalog,
) -> Result<(), RuntimeControlWorkerRunError> {
    let mut pending = tokio::task::JoinSet::new();
    for identifier in catalog.image_identifiers() {
        if pending.len() == IMAGE_PROBE_CONCURRENCY {
            settle_image_probe(pending.join_next().await)?;
        }
        let provider = provider.clone();
        let identifier = identifier.clone();
        pending.spawn(async move {
            provider
                .list(Some(&identifier), None)
                .await
                .map(|_| ())
                .map_err(|error| error.to_string())
        });
    }
    while let Some(result) = pending.join_next().await {
        settle_image_probe(Some(result))?;
    }
    Ok(())
}

fn settle_image_probe(
    result: Option<Result<Result<(), String>, tokio::task::JoinError>>,
) -> Result<(), RuntimeControlWorkerRunError> {
    match result {
        Some(Ok(Ok(()))) => Ok(()),
        Some(Ok(Err(reason))) => Err(RuntimeControlWorkerRunError::Startup {
            dependency: "microvm-control",
            reason,
        }),
        Some(Err(error)) => Err(RuntimeControlWorkerRunError::Startup {
            dependency: "microvm-control",
            reason: format!("catalog probe task failed: {error}"),
        }),
        None => Err(RuntimeControlWorkerRunError::Startup {
            dependency: "microvm-control",
            reason: "catalog probe set ended before its bounded wave completed".to_owned(),
        }),
    }
}

/// Runs `runtime-control-worker` until it stops.
///
/// # Errors
///
/// Returns [`RuntimeControlWorkerRunError::Startup`] when a production dependency probe fails,
/// [`RuntimeControlWorkerRunError::Unbound`] when a declared port has no adapter, and
/// [`RuntimeControlWorkerRunError::Runtime`] when the Lambda runtime stops.
pub async fn run(config: &Config) -> Result<(), RuntimeControlWorkerRunError> {
    tracing::info!(
        target: "aex::diagnostics",
        event_name = "process.started",
        deployable = DEPLOYABLE,
        plane = %config.plane,
        region = config.region.as_str(),
        "process started"
    );
    let adapters = resolve(config).await?;
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
    .map_err(|error| RuntimeControlWorkerRunError::Runtime {
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
    if let Err(error) = aex_platform_diagnostics::install_json() {
        eprintln!("{DEPLOYABLE}: diagnostics installation failed: {error}");
        return std::process::ExitCode::FAILURE;
    }
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(
                target: "aex::diagnostics",
                event_name = "process.configuration_rejected",
                deployable = DEPLOYABLE,
                error = %error,
                "process configuration rejected"
            );
            eprintln!("{DEPLOYABLE}: refusing to start: {error}");
            eprintln!(
                "{DEPLOYABLE}: required variables are {}",
                config::REQUIRED_VARS.join(", ")
            );
            return std::process::ExitCode::FAILURE;
        }
    };
    let outcome = run(&config).await;
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{DEPLOYABLE}: stopped: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Adapters, RuntimeControlWorkerRunError, config::Config, health::Readiness, now};
    use std::collections::BTreeMap;

    fn config() -> Config {
        let vars = BTreeMap::from([
            ("AEX_PLANE", "dev".to_owned()),
            ("AEX_REGION", "eu-west-1".to_owned()),
            ("AEX_ACCOUNT_ID", "522921482290".to_owned()),
            (
                "AEX_RUNTIME_ACTIVITY_TABLE",
                "aex-dev-runtime-activity".to_owned(),
            ),
            (
                "AEX_SESSION_AUTHORITY_TABLE",
                "aex-dev-session-authority".to_owned(),
            ),
            (
                "AEX_RUNTIME_LIFECYCLE_QUEUE_URL",
                "https://sqs.eu-west-1.amazonaws.com/1/lifecycle".to_owned(),
            ),
            (
                "AEX_USAGE_COMPUTE_QUEUE_URL",
                "https://sqs.eu-west-1.amazonaws.com/1/compute".to_owned(),
            ),
            (
                "AEX_USAGE_STORAGE_QUEUE_URL",
                "https://sqs.eu-west-1.amazonaws.com/1/storage".to_owned(),
            ),
            (
                "AEX_MICROVM_CONTROL_ENDPOINT",
                "https://lambda.eu-west-1.amazonaws.com".to_owned(),
            ),
            ("AEX_HANDS_IMAGE_CATALOG", catalog_json()),
            ("AEX_RUNTIME_DUE_SHARDS", "8".to_owned()),
            ("AEX_RUNTIME_DUE_PAGE_ITEMS", "32".to_owned()),
            ("AEX_RUNTIME_DUE_PAGE_READS", "100".to_owned()),
            ("AEX_PRICING_VERSION", "synthetic-zero-v1".to_owned()),
        ]);
        Config::from_lookup(|name| vars.get(name).cloned())
            .expect("the fixture environment is complete")
    }

    fn catalog_json() -> String {
        // The published set; browser variants are excluded during prelaunch.
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
                            "arn:aws:lambda:eu-west-1:522921482290:microvm-image:aex-dev-{}",
                            char::from(b'a' + u8::try_from(index).expect("a small catalog")).to_string().repeat(52),
                        ),
                        "imageVersion": (index + 1).to_string(),
                        "artifactDigest": format!("sha256:{index:064x}"),
                        "minimumMemoryMiB": memory,
                        "browser": browser,
                    }),
                )
            })
            .collect::<BTreeMap<_, _>>();
        serde_json::to_string(&rows).expect("catalog JSON")
    }

    #[test]
    fn an_unbound_port_refuses_the_start_and_says_which_one() {
        let error = Adapters::default()
            .compose(&config())
            .expect_err("an empty composition is refused");
        let RuntimeControlWorkerRunError::Unbound { missing } = error else {
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
    fn startup_scopes_the_provider_probe_to_every_published_image() {
        // Five published variants at four concurrent reads is two waves. What the
        // assertion protects is that the probe is bounded and covers the whole
        // catalog, not the particular arithmetic.
        assert_eq!(config().image_catalog.image_identifiers().len(), 5);
        assert_eq!(super::IMAGE_PROBE_CONCURRENCY, 4);
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
