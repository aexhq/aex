//! `content-lifecycle-worker` composition root (Rust Lambda ZIP).
//!
//! One binary, four deployed roles: `expiry`, `reconcile`, `marksweep` and
//! `delete` (RS-10). Each role keeps its own IAM role, schedule, concurrency and
//! alarm; only `delete` ever holds the object-delete capability, and the
//! configuration refuses the two mismatches in both directions.

use std::process::ExitCode;

use aex_content_dynamodb::store::{ContentMetadataStore as _, ContentStore, GrantExpiry};
use aex_regional_http::config::ConfigError;
use aex_session_dynamodb::paging::PageBudget;
use aex_wire::types::Timestamp;
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
            config.plane.as_str().to_owned(),
        )
        .with(
            aex_telemetry_schema::generated::AEX_REGION,
            config.region.as_str().to_owned(),
        ),
    );

    let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let dynamodb = aws_sdk_dynamodb::Client::new(&aws);
    let content = ContentStore::new(dynamodb.clone(), config.content_table.clone());

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
        content,
        work_table: config.work_table.clone(),
        expiry_scan_shards: config.expiry_scan_shards,
        expiry_page_items: config.expiry_page_items,
        objects,
    };

    lambda_runtime::run(service_fn(move |event: LambdaEvent<serde_json::Value>| {
        let role = role.clone();
        async move { role.handle(event.payload).await }
    }))
    .await
    .map_err(|error: LambdaError| RunError::Runtime(error.to_string()))
}

/// One deployed role and the adapters it is allowed to hold.
#[derive(Clone)]
struct Role {
    mode: Mode,
    content: ContentStore,
    work_table: String,
    expiry_scan_shards: Option<u16>,
    expiry_page_items: Option<u32>,
    objects: Option<aws_sdk_s3::Client>,
}

impl Role {
    async fn handle(&self, payload: serde_json::Value) -> Result<serde_json::Value, LambdaError> {
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
        if self.mode == Mode::Expiry {
            return self.expire_grants().await;
        }
        Ok(serde_json::json!({
            "mode": self.mode.as_str(),
            "contentTable": self.content.table(),
            "workTable": self.work_table,
        }))
    }

    async fn expire_grants(&self) -> Result<serde_json::Value, LambdaError> {
        let now = now().map_err(|error| LambdaError::from(error.to_string()))?;
        let shards = self
            .expiry_scan_shards
            .ok_or_else(|| LambdaError::from("expiry role has no admitted shard bound"))?;
        let page_items = self
            .expiry_page_items
            .ok_or_else(|| LambdaError::from("expiry role has no admitted page bound"))?;
        let budget =
            PageBudget::new(page_items).map_err(|error| LambdaError::from(error.to_string()))?;
        let mut expired = 0_u64;
        let mut shards_with_more = 0_u64;
        for shard in 0..shards {
            let page = self
                .content
                .scan_expired_grants(shard, now, budget)
                .await
                .map_err(|error| LambdaError::from(error.to_string()))?;
            if page.more {
                shards_with_more += 1;
            }
            expired += self.expire_page(page.grants, now).await?;
        }
        Ok(serde_json::json!({
            "mode": self.mode.as_str(),
            "expiredGrants": expired,
            "shardsScanned": shards,
            "shardsWithMore": shards_with_more,
        }))
    }

    /// Runs every deletion in the selected page, but never lets one scheduled
    /// invocation create an unbounded `DynamoDB` burst.
    async fn expire_page(
        &self,
        grants: Vec<GrantExpiry>,
        now: Timestamp,
    ) -> Result<u64, LambdaError> {
        let mut tasks = tokio::task::JoinSet::new();
        let mut completed = 0_u64;
        let mut first_error: Option<String> = None;
        for grant in grants {
            while tasks.len() >= content_lifecycle_worker::MAX_EXPIRY_WRITES_IN_FLIGHT {
                collect_expiry_result(&mut tasks, &mut completed, &mut first_error).await;
            }
            let content = self.content.clone();
            tasks.spawn(async move { content.expire_grant(&grant, now).await });
        }
        while !tasks.is_empty() {
            collect_expiry_result(&mut tasks, &mut completed, &mut first_error).await;
        }
        match first_error {
            None => Ok(completed),
            Some(error) => Err(LambdaError::from(error)),
        }
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

async fn collect_expiry_result(
    tasks: &mut tokio::task::JoinSet<Result<(), aex_session_dynamodb::error::StoreError>>,
    completed: &mut u64,
    first_error: &mut Option<String>,
) {
    match tasks.join_next().await {
        Some(Ok(Ok(()))) => *completed += 1,
        Some(Ok(Err(error))) => {
            first_error.get_or_insert_with(|| error.to_string());
        }
        Some(Err(error)) => {
            first_error.get_or_insert_with(|| format!("grant expiry task failed: {error}"));
        }
        None => {}
    }
}

/// The host clock, read once per scheduled invocation.
fn now() -> Result<Timestamp, ClockError> {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| ClockError)?;
    let millis = i64::try_from(since.as_millis()).map_err(|_| ClockError)?;
    Timestamp::from_unix_millis(millis).map_err(|_| ClockError)
}

#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("the host clock is not a representable wire instant")]
struct ClockError;

impl std::fmt::Debug for Role {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Role")
            .field("mode", &self.mode.as_str())
            .field("content_table", &self.content.table())
            .field("holds_object_client", &self.objects.is_some())
            .finish_non_exhaustive()
    }
}
