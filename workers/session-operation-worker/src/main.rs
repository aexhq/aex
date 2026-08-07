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

use aex_regional_http::config::RegionalHttpConfigError;
use aex_session_dynamodb::paging::PageBudget;
use aex_wire::types::Timestamp;
use aws_lambda_events::event::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use futures::{StreamExt as _, stream};
use lambda_runtime::{Error as LambdaError, LambdaEvent, service_fn};
use session_operation_worker::config::Config;
use session_operation_worker::{
    BatchItem, DUE_SHARD_CONCURRENCY, DynamoOperationPort, OperationReconciler,
    ReconcileDisposition, Trigger, WorkHint, WorkPort, batch_response, due_shards, next_due_cursor,
};

/// Bounded work rows read from each due shard in one scheduled invocation.
const DUE_PAGE_ITEMS: u32 = 25;
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ShardOutcome {
    retired: u64,
    already_retired: u64,
    deferred: u64,
    failed: u64,
    cursor_advanced: bool,
}

/// Why `session-operation-worker` stopped.
#[derive(Debug, thiserror::Error)]
enum SessionOperationWorkerRunError {
    /// Start-up configuration was rejected.
    #[error(transparent)]
    Config(#[from] RegionalHttpConfigError),
    /// The Lambda runtime stopped.
    #[error("the lambda runtime stopped: {0}")]
    Runtime(String),
    /// The declared authorities could not be composed.
    #[error("the worker composition was rejected: {0}")]
    Composition(String),
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
async fn run(config: Config, telemetry: &aex_platform_telemetry::Handle) -> Result<(), SessionOperationWorkerRunError> {
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
    let worker = Worker::new(&config, &dynamodb)
        .map_err(|error| SessionOperationWorkerRunError::Composition(error.to_string()))?;

    lambda_runtime::run(service_fn(move |event: LambdaEvent<serde_json::Value>| {
        let worker = worker.clone();
        async move { worker.handle(event.payload).await }
    }))
    .await
    .map_err(|error: LambdaError| SessionOperationWorkerRunError::Runtime(error.to_string()))
}

/// The composed operation-reconciliation slice.
///
/// Nonterminal effect ports are not represented here until their authority
/// transactions exist. This keeps startup honest: constructed clients are
/// exactly the clients this slice calls.
#[derive(Clone)]
struct Worker {
    reconciler: OperationReconciler<aex_work_dynamodb::store::WorkStore, DynamoOperationPort>,
    config: Config,
}

impl Worker {
    fn new(config: &Config, dynamodb: &aws_sdk_dynamodb::Client) -> Result<Self, LambdaError> {
        let work =
            aex_work_dynamodb::store::WorkStore::new(dynamodb.clone(), config.work_table.clone());
        let operations = DynamoOperationPort::new(
            dynamodb.clone(),
            config.session_table.clone(),
            config.work_table.clone(),
        );
        let reconciler = OperationReconciler::new(
            work,
            operations,
            format!("session-operation-worker:{}", config.release_digest),
            config.lease_ms,
        )
        .map_err(|error| LambdaError::from(error.to_string()))?;
        Ok(Self {
            reconciler,
            config: config.clone(),
        })
    }

    /// Routes one invocation to the trigger it carries.
    ///
    /// An unrecognised payload is a failure, never a silently empty batch: a
    /// worker that answers `200` to something it did not understand is a worker
    /// whose queue drains without doing anything.
    async fn handle(&self, payload: serde_json::Value) -> Result<serde_json::Value, LambdaError> {
        match Trigger::classify(&payload) {
            Trigger::Queue => {
                let event: SqsEvent = serde_json::from_value(payload)?;
                let response = self.drain(&event).await?;
                Ok(serde_json::to_value(response)?)
            }
            Trigger::DueScan => self.scan_due().await,
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
    async fn drain(&self, event: &SqsEvent) -> Result<SqsBatchResponse, LambdaError> {
        // A record without an id cannot be represented in Lambda's partial
        // batch response. Refuse the invocation before touching any row rather
        // than acknowledging an unaddressable record.
        if event
            .records
            .iter()
            .any(|record| record.message_id.as_deref().is_none_or(str::is_empty))
        {
            return Err(LambdaError::from("an SQS record carried no messageId"));
        }
        let now = now()?;
        let mut items = Vec::with_capacity(event.records.len());
        for record in &event.records {
            let id = record.message_id.as_deref().unwrap_or_default();
            let outcome = record
                .body
                .as_deref()
                .ok_or(session_operation_worker::ReconcileError::InvalidHint)
                .and_then(WorkHint::decode);
            let item = match outcome {
                Ok(hint) => match self.reconcile(&hint, now).await {
                    Ok(ReconcileDisposition::Retired | ReconcileDisposition::AlreadyRetired) => {
                        BatchItem::succeeded(id)
                    }
                    Ok(ReconcileDisposition::Deferred) | Err(_) => BatchItem::failed(id),
                },
                Err(_) => BatchItem::failed(id),
            };
            items.push(item);
        }
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
        Ok(rendered)
    }

    /// Sweeps one bounded page from every session-operation due shard.
    async fn scan_due(&self) -> Result<serde_json::Value, LambdaError> {
        let now = now()?;
        let budget = PageBudget::new(DUE_PAGE_ITEMS)
            .map_err(|error| LambdaError::from(error.to_string()))?;
        let shards = due_shards(self.config.due_scan_shards)?;
        let outcomes = stream::iter(shards.iter().copied())
            .map(|shard| async move {
                let shard = u16::try_from(shard)
                    .map_err(|_| LambdaError::from("a due shard does not fit the store key"))?;
                self.scan_shard(shard, now, budget).await
            })
            .buffer_unordered(DUE_SHARD_CONCURRENCY)
            .collect::<Vec<_>>()
            .await;
        let mut retired = 0_u64;
        let mut already_retired = 0_u64;
        let mut deferred = 0_u64;
        let mut failed = 0_u64;
        let mut advanced_shards = 0_u64;
        for outcome in outcomes {
            let outcome = outcome?;
            retired += outcome.retired;
            already_retired += outcome.already_retired;
            deferred += outcome.deferred;
            failed += outcome.failed;
            advanced_shards += u64::from(outcome.cursor_advanced);
        }
        if deferred != 0 || failed != 0 {
            return Err(LambdaError::from(format!(
                "due scan advanced {advanced_shards} shard cursor(s) but left {deferred} \
                 nonterminal and {failed} invalid operation step(s) uncompleted"
            )));
        }
        Ok(serde_json::json!({
            "scannedShards": shards.len(),
            "advancedShards": advanced_shards,
            "retired": retired,
            "alreadyRetired": already_retired,
        }))
    }

    async fn reconcile(
        &self,
        hint: &WorkHint,
        now: Timestamp,
    ) -> Result<ReconcileDisposition, LambdaError> {
        tokio::time::timeout(
            std::time::Duration::from_millis(self.config.step_deadline_ms),
            self.reconciler.reconcile(hint, now),
        )
        .await
        .map_err(|_| LambdaError::from("an operation step exceeded AEX_STEP_DEADLINE_MS"))?
        .map_err(|error| LambdaError::from(error.to_string()))
    }

    async fn scan_shard(
        &self,
        shard: u16,
        now: Timestamp,
        budget: PageBudget,
    ) -> Result<ShardOutcome, LambdaError> {
        let cursor = self
            .reconciler
            .work()
            .load_cursor(shard)
            .await
            .map_err(|error| LambdaError::from(error.to_string()))?;
        let page = self
            .reconciler
            .work()
            .scan_due_after(shard, now, budget, cursor.as_ref())
            .await
            .map_err(|error| LambdaError::from(error.to_string()))?;
        let mut outcome = ShardOutcome::default();
        let next_cursor = next_due_cursor(shard, cursor.as_ref(), &page, now)
            .map_err(|error| LambdaError::from(error.to_string()))?;
        for due in page.items {
            if due.kind != "operation.step" {
                continue;
            }
            let hint = WorkHint {
                work_id: due.work_id,
                workspace: due.workspace,
            };
            match self.reconcile(&hint, now).await {
                Ok(ReconcileDisposition::Retired) => outcome.retired += 1,
                Ok(ReconcileDisposition::AlreadyRetired) => outcome.already_retired += 1,
                Ok(ReconcileDisposition::Deferred) => outcome.deferred += 1,
                Err(_) => outcome.failed += 1,
            }
        }

        if let Some(next) = next_cursor {
            self.reconciler
                .work()
                .advance_cursor(&next)
                .await
                .map_err(|error| LambdaError::from(error.to_string()))?;
            outcome.cursor_advanced = true;
        }
        Ok(outcome)
    }
}

impl std::fmt::Debug for Worker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Worker")
            .field("work", &self.config.work_table)
            .field("operations", &self.config.session_table)
            .finish_non_exhaustive()
    }
}

fn now() -> Result<Timestamp, LambdaError> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| LambdaError::from("the host clock is before the Unix epoch"))?;
    let millis = i64::try_from(elapsed.as_millis())
        .map_err(|_| LambdaError::from("the host clock does not fit the wire instant"))?;
    Timestamp::from_unix_millis(millis).map_err(|error| LambdaError::from(error.to_string()))
}
