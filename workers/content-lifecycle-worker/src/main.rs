//! `content-lifecycle-worker` composition root (Rust Lambda ZIP).
//!
//! One binary, four deployed roles: `expiry`, `reconcile`, `marksweep` and
//! `delete` (RS-10). Each role keeps its own IAM role, schedule, concurrency and
//! alarm; only `delete` ever holds the object-delete capability, and the
//! configuration refuses the two mismatches in both directions.

use std::process::ExitCode;

use aex_regional_http::config::ConfigError;
use aws_lambda_events::event::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use content_lifecycle_worker::config::{Config, Mode};
use lambda_runtime::{Error as LambdaError, LambdaEvent, service_fn};

/// Why `content-lifecycle-worker` stopped.
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
            eprintln!("content-lifecycle-worker: refusing to start: {error}");
            return ExitCode::FAILURE;
        }
    };
    let settings = aex_platform_telemetry::Settings::default();
    let telemetry = aex_platform_telemetry::Handle::install(&settings, None);
    let outcome = run(config, &telemetry).await;
    if let aex_platform_telemetry::FlushOutcome::DeadlineExceeded { pending } =
        telemetry.flush(settings.flush_deadline)
    {
        eprintln!("content-lifecycle-worker: telemetry flush left {pending} record(s) undelivered");
    }
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("content-lifecycle-worker: stopped: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Builds the adapters this role is allowed to hold and serves its trigger.
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
    let content = aex_content_dynamodb::store::ContentStore::new(
        dynamodb.clone(),
        config.content_table.clone(),
    );
    let work = aex_work_dynamodb::store::WorkStore::new(dynamodb, config.work_table.clone());

    // The object client exists only in the role that may use it. A `reconcile`
    // process that never constructs an S3 client cannot delete an object even if
    // its IAM policy were wrong.
    let objects = if config.mode.deletes_objects() {
        Some(aws_sdk_s3::Client::new(&aws))
    } else {
        None
    };
    let role = Role {
        mode: config.mode,
        content_table: content.table().to_owned(),
        work_table: work.table().to_owned(),
        objects,
    };

    lambda_runtime::run(service_fn(move |event: LambdaEvent<serde_json::Value>| {
        let role = role.clone();
        async move { role.handle(event.payload) }
    }))
    .await
    .map_err(|error: LambdaError| RunError::Runtime(error.to_string()))
}

/// One deployed role and the adapters it is allowed to hold.
#[derive(Clone)]
struct Role {
    mode: Mode,
    content_table: String,
    work_table: String,
    objects: Option<aws_sdk_s3::Client>,
}

impl Role {
    fn handle(&self, payload: serde_json::Value) -> Result<serde_json::Value, LambdaError> {
        if payload
            .get("Records")
            .is_some_and(serde_json::Value::is_array)
        {
            if !self.mode.deletes_objects() {
                return Err(LambdaError::from(format!(
                    "`{}` mode is scheduled and answers no queue batch",
                    self.mode.as_str()
                )));
            }
            let event: SqsEvent = serde_json::from_value(payload)?;
            return Ok(serde_json::to_value(Self::drain(&event))?);
        }
        if self.mode.deletes_objects() {
            return Err(LambdaError::from(
                "`delete` mode is queue-triggered and answers no schedule",
            ));
        }
        Ok(serde_json::json!({
            "mode": self.mode.as_str(),
            "contentTable": self.content_table,
            "workTable": self.work_table,
        }))
    }

    /// Reports per-item failures instead of throwing, so an item that already
    /// committed is never re-executed (RS-20).
    fn drain(event: &SqsEvent) -> SqsBatchResponse {
        let mut rendered = SqsBatchResponse::default();
        rendered.batch_item_failures = event
            .records
            .iter()
            .map(|record| {
                let mut failure = BatchItemFailure::default();
                failure.item_identifier = record
                    .message_id
                    .clone()
                    .unwrap_or_else(|| "unidentified".to_owned());
                failure
            })
            .collect();
        rendered
    }
}

impl std::fmt::Debug for Role {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Role")
            .field("mode", &self.mode.as_str())
            .field("holds_object_client", &self.objects.is_some())
            .finish_non_exhaustive()
    }
}
