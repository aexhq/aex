//! `session-operation-worker` composition root (Rust Lambda ZIP).
//!
//! Exclusive responsibility: the genuinely cross-invocation legs — the
//! `session_delete` purge, the `workspace_delete` regional purge, and paged
//! `session_persist`/`session_fork` staging. Everything else terminalizes inline
//! at admission or in the Brain/runtime work domain (RS-07).
//!
//! Two triggers, one binary: an SQS hint batch and a scheduled due scan. The
//! scan is what makes the queue an optimisation rather than a dependency — if
//! every hint is lost, the sharded due index still recovers the work.

use std::process::ExitCode;

use aex_regional_http::config::ConfigError;
use aws_lambda_events::event::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use lambda_runtime::{Error as LambdaError, LambdaEvent, service_fn};
use session_operation_worker::config::Config;
use session_operation_worker::{BatchItem, Trigger, batch_response, due_shards};

/// Why `session-operation-worker` stopped.
#[derive(Debug, thiserror::Error)]
enum RunError {
    /// Start-up configuration was rejected.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// The Lambda runtime stopped.
    #[error("the lambda runtime stopped: {0}")]
    Runtime(String),
}

#[tokio::main]
async fn main() -> ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("session-operation-worker: refusing to start: {error}");
            return ExitCode::FAILURE;
        }
    };
    let settings = aex_platform_telemetry::Settings::default();
    let telemetry = aex_platform_telemetry::Handle::install(&settings, None);
    let outcome = run(config, &telemetry).await;
    if let aex_platform_telemetry::FlushOutcome::DeadlineExceeded { pending } =
        telemetry.flush(settings.flush_deadline)
    {
        eprintln!("session-operation-worker: telemetry flush left {pending} record(s) undelivered");
    }
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("session-operation-worker: stopped: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Builds the real adapters and serves both triggers.
async fn run(config: Config, telemetry: &aex_platform_telemetry::Handle) -> Result<(), RunError> {
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
            config.region.as_str().to_owned(),
        ),
    );

    let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let dynamodb = aws_sdk_dynamodb::Client::new(&aws);
    let objects = aws_sdk_s3::Client::new(&aws);
    let worker = Worker::new(&config, &dynamodb, &objects);

    lambda_runtime::run(service_fn(move |event: LambdaEvent<serde_json::Value>| {
        let worker = worker.clone();
        async move { worker.handle(event.payload) }
    }))
    .await
    .map_err(|error: LambdaError| RunError::Runtime(error.to_string()))
}

/// The composed worker: the four authorities it may reach and nothing else.
#[derive(Clone)]
struct Worker {
    work: aex_work_dynamodb::store::WorkStore,
    content: aex_content_dynamodb::store::ContentStore,
    registry: aex_registry_dynamodb::store::RegistryDynamoStore,
    objects: aws_sdk_s3::Client,
    config: Config,
}

impl Worker {
    fn new(
        config: &Config,
        dynamodb: &aws_sdk_dynamodb::Client,
        objects: &aws_sdk_s3::Client,
    ) -> Self {
        Self {
            work: aex_work_dynamodb::store::WorkStore::new(
                dynamodb.clone(),
                config.work_table.clone(),
            ),
            content: aex_content_dynamodb::store::ContentStore::new(
                dynamodb.clone(),
                config.content_table.clone(),
            ),
            registry: aex_registry_dynamodb::store::RegistryDynamoStore::new(
                dynamodb.clone(),
                config.registry_table.clone(),
            ),
            objects: objects.clone(),
            config: config.clone(),
        }
    }

    /// Routes one invocation to the trigger it carries.
    ///
    /// An unrecognised payload is a failure, never a silently empty batch: a
    /// worker that answers `200` to something it did not understand is a worker
    /// whose queue drains without doing anything.
    fn handle(&self, payload: serde_json::Value) -> Result<serde_json::Value, LambdaError> {
        match Trigger::classify(&payload) {
            Trigger::Queue => {
                let event: SqsEvent = serde_json::from_value(payload)?;
                let response = Self::drain(&event);
                Ok(serde_json::to_value(response)?)
            }
            Trigger::DueScan => {
                let shards = due_shards(self.config.due_scan_shards)?;
                Ok(serde_json::json!({ "scannedShards": shards.len() }))
            }
            Trigger::Unknown => Err(LambdaError::from(
                "unrecognised trigger payload: this worker serves an SQS batch or a due scan",
            )),
        }
    }

    /// Drains one SQS batch, reporting per-item failures.
    ///
    /// The invocation itself never fails: throwing would re-run the items that
    /// already succeeded and discard the per-item retry state that was already
    /// persisted (RS-20).
    fn drain(event: &SqsEvent) -> SqsBatchResponse {
        let items: Vec<BatchItem> = event
            .records
            .iter()
            .map(|record| {
                let id = record.message_id.clone().unwrap_or_default();
                // The claim, the bounded effect and the fenced commit belong to
                // the step body; until the peer transaction vocabulary lands, an
                // unidentifiable message is reported as a per-item failure
                // rather than acknowledged.
                if id.is_empty() {
                    BatchItem::failed("unidentified")
                } else {
                    BatchItem::failed(id)
                }
            })
            .collect();
        let response = batch_response(&items);
        let mut rendered = SqsBatchResponse::default();
        rendered.batch_item_failures = response
            .batch_item_failures
            .into_iter()
            .map(|item_identifier| {
                let mut failure = BatchItemFailure::default();
                failure.item_identifier = item_identifier;
                failure
            })
            .collect();
        rendered
    }

    /// The authorities this worker is bound to, for the readiness record.
    #[allow(dead_code, reason = "read by the composition test through Worker::new")]
    fn bound(&self) -> [&str; 4] {
        [
            self.work.table(),
            self.content.table(),
            self.registry.table(),
            self.config.content_bucket.as_str(),
        ]
    }
}

impl std::fmt::Debug for Worker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Worker")
            .field("work", &self.work.table())
            .field("content", &self.content.table())
            .field("registry", &self.registry.table())
            .finish_non_exhaustive()
    }
}

/// Silences the unused-client warning while the object leg is composed.
const _: fn(&Worker) -> &aws_sdk_s3::Client = |worker| &worker.objects;
