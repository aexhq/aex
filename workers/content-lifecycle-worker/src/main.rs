//! `content-lifecycle-worker` composition root (Rust Lambda ZIP).
//!
//! One binary, five deployed roles: `expiry`, `uploadexpiry`, `reconcile`,
//! `marksweep` and `delete` (RS-10, extended by E D-8). Each role keeps its own
//! IAM role, schedule, concurrency and alarm; only `delete` ever holds the
//! object-delete capability, and the configuration refuses the two mismatches in
//! both directions.
//!
//! Every role now has a real body. `reconcile`, `marksweep` and `delete` used to
//! fall through to a JSON echo, and `delete` reported every queue record as a
//! batch-item failure without ever calling S3 — so the collector these other
//! decisions depend on for their safety argument never reclaimed anything.

use std::process::ExitCode;

use aex_content_dynamodb::store::ContentStore;
use aex_regional_http::config::RegionalHttpConfigError;
use aex_session_dynamodb::paging::PageBudget;
use aex_wire::ids::{OrganizationId, PrefixedId as _};
use aex_wire::types::Timestamp;
use aws_lambda_events::event::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use content_lifecycle_worker::config::{Config, Mode};
use content_lifecycle_worker::expiry::expire_due_grants;
use content_lifecycle_worker::gc::{self, DeletableObjects, SweepRequest};
use content_lifecycle_worker::upload_expiry::{self, UploadObjects, UploadRows};
use content_lifecycle_worker::{DeleteIntent, ObjectDeleteResult};
use lambda_runtime::{Error as LambdaError, LambdaEvent, service_fn};

/// Why `content-lifecycle-worker` stopped.
#[derive(Debug, thiserror::Error)]
enum ContentLifecycleWorkerRunError {
    /// Start-up configuration was rejected.
    #[error(transparent)]
    Config(#[from] RegionalHttpConfigError),
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
async fn run(
    config: Config,
    telemetry: &aex_platform_telemetry::Handle,
) -> Result<(), ContentLifecycleWorkerRunError> {
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

    // The object client exists only in the roles that reach S3 at all. A
    // `reconcile` process that never constructs an S3 client cannot delete an
    // object even if its IAM policy were wrong. `uploadexpiry` holds one for
    // `HeadObject` and `AbortMultipartUpload` and still cannot delete, because
    // the delete capability is refused to it at start-up.
    let objects = if config.mode.reaches_objects() {
        Some(aws_sdk_s3::Client::new(&aws))
    } else {
        None
    };
    let registry = aex_registry_dynamodb::store::RegistryDynamoStore::new(
        dynamodb.clone(),
        config.registry_table.clone(),
    );
    let role = Role {
        mode: config.mode,
        content,
        work: aex_work_dynamodb::store::WorkStore::new(dynamodb.clone(), config.work_table.clone()),
        registry,
        work_table: config.work_table.clone(),
        content_bucket: config.content_bucket.clone(),
        content_bucket_owner: config.content_bucket_owner.clone(),
        expiry_scan_shards: config.expiry_scan_shards,
        expiry_page_items: config.expiry_page_items,
        mark_page_items: config.mark_page_items,
        sweep_page_items: config.sweep_page_items,
        objects,
    };

    lambda_runtime::run(service_fn(move |event: LambdaEvent<serde_json::Value>| {
        let role = role.clone();
        async move { role.handle(event.payload).await }
    }))
    .await
    .map_err(|error: LambdaError| ContentLifecycleWorkerRunError::Runtime(error.to_string()))
}

/// One deployed role and the adapters it is allowed to hold.
#[derive(Clone)]
struct Role {
    mode: Mode,
    content: ContentStore,
    work: aex_work_dynamodb::store::WorkStore,
    registry: aex_registry_dynamodb::store::RegistryDynamoStore,
    work_table: String,
    content_bucket: String,
    content_bucket_owner: String,
    expiry_scan_shards: Option<u16>,
    expiry_page_items: Option<u32>,
    mark_page_items: Option<u64>,
    sweep_page_items: Option<u64>,
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
            return self.drain(&event).await;
        }
        if self.mode.deletes_objects() {
            return Err(LambdaError::from(
                "`delete` mode is queue-triggered and answers no schedule",
            ));
        }
        match self.mode {
            Mode::Expiry => self.expire_grants().await,
            Mode::UploadExpiry => self.expire_uploads().await,
            Mode::Reconcile => self.reconcile(payload).await,
            Mode::MarkSweep => self.mark_sweep(payload).await,
            Mode::Delete => unreachable!("the queue arm above answers every delete invocation"),
        }
    }

    /// Walks the workspaces the schedule names, rechecking staged rows against
    /// the exact grace boundary and their live reachability.
    async fn reconcile(
        &self,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, LambdaError> {
        let request = sweep_request(payload)?;
        let budget = PageBudget::new(page_bound(self.mark_page_items))
            .map_err(|error| LambdaError::from(error.to_string()))?;
        let now = now().map_err(|error| LambdaError::from(error.to_string()))?;
        let report = gc::reconcile(&self.content, &request, now, budget)
            .await
            .map_err(|error| LambdaError::from(error.to_string()))?;
        Ok(serde_json::json!({
            "mode": self.mode.as_str(),
            "report": report,
        }))
    }

    /// Walks reachability and stages what nothing points at.
    async fn mark_sweep(
        &self,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, LambdaError> {
        let request = sweep_request(payload.clone())?;
        let organization = payload
            .get("organizationId")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                LambdaError::from(
                    "a mark-sweep invocation must name the organization the descriptors are \
                     re-checked against",
                )
            })
            .and_then(|id| {
                OrganizationId::parse(id).map_err(|error| LambdaError::from(error.to_string()))
            })?;
        let budget = PageBudget::new(page_bound(self.sweep_page_items))
            .map_err(|error| LambdaError::from(error.to_string()))?;
        let now = now().map_err(|error| LambdaError::from(error.to_string()))?;
        let report = gc::mark_sweep(&self.content, &request, organization, now, budget)
            .await
            .map_err(|error| LambdaError::from(error.to_string()))?;
        Ok(serde_json::json!({
            "mode": self.mode.as_str(),
            "report": report,
        }))
    }

    /// Sweeps every due pending upload off the sharded `registry.upload_expiry`
    /// index.
    ///
    /// The index kind and its payload keys were declared and unused; this role is
    /// their first consumer (E D-8). The shape is the grant-expiry shape: a
    /// per-shard durable cursor, a bounded page, and every write settled before
    /// the cursor advances.
    async fn expire_uploads(&self) -> Result<serde_json::Value, LambdaError> {
        let now = now().map_err(|error| LambdaError::from(error.to_string()))?;
        let shards = self
            .expiry_scan_shards
            .ok_or_else(|| LambdaError::from("upload expiry has no admitted shard bound"))?;
        let page_items = self
            .expiry_page_items
            .ok_or_else(|| LambdaError::from("upload expiry has no admitted page bound"))?;
        let budget =
            PageBudget::new(page_items).map_err(|error| LambdaError::from(error.to_string()))?;
        let objects = ObjectAdapter {
            client: self
                .objects
                .clone()
                .ok_or_else(|| LambdaError::from("upload expiry holds no object client"))?,
            bucket: self.content_bucket.clone(),
            expected_owner: self.content_bucket_owner.clone(),
        };
        let rows = RowAdapter {
            registry: self.registry.clone(),
        };
        let report = upload_expiry::expire_due_uploads(
            &self.work,
            &rows,
            &objects,
            LEASE_OWNER,
            shards,
            now,
            budget,
        )
        .await
        .map_err(|error| LambdaError::from(error.to_string()))?;
        Ok(serde_json::json!({
            "mode": self.mode.as_str(),
            "report": report,
        }))
    }

    /// Expires lapsed download grants and their pins.
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
        let report = expire_due_grants(self.content.clone(), shards, now, budget)
            .await
            .map_err(|error| LambdaError::from(error.to_string()))?;
        Ok(serde_json::json!({
            "mode": self.mode.as_str(),
            "expiredGrants": report.expired_grants,
            "shardsScanned": report.shards_scanned,
            "shardsWithMore": report.shards_with_more,
        }))
    }

    /// Executes the batch, then reports per-item failures instead of throwing, so
    /// an item that already committed is never re-executed (RS-20).
    ///
    /// Only records that genuinely have to be retried are reported. Reporting
    /// every record unconditionally — which is what this did before — meant the
    /// delete queue drained to its dead-letter queue and no object was ever
    /// reclaimed, while the worker's own metrics looked like it was working.
    async fn drain(&self, event: &SqsEvent) -> Result<serde_json::Value, LambdaError> {
        let objects = ObjectAdapter {
            client: self
                .objects
                .clone()
                .ok_or_else(|| LambdaError::from("delete mode holds no object client"))?,
            bucket: self.content_bucket.clone(),
            expected_owner: self.content_bucket_owner.clone(),
        };
        let records: Vec<(String, String)> = event
            .records
            .iter()
            .map(|record| {
                (
                    record
                        .message_id
                        .clone()
                        .unwrap_or_else(|| "unidentified".to_owned()),
                    record.body.clone().unwrap_or_default(),
                )
            })
            .collect();
        let now = now().map_err(|error| LambdaError::from(error.to_string()))?;
        let report = gc::run_delete_batch(
            &self.content,
            &objects,
            &self.content_bucket_owner,
            &records,
            now,
        )
        .await;

        let mut rendered = SqsBatchResponse::default();
        rendered.batch_item_failures = report
            .failed
            .iter()
            .map(|id| {
                let mut failure = BatchItemFailure::default();
                failure.item_identifier.clone_from(id);
                failure
            })
            .collect();
        let mut response = serde_json::to_value(&rendered)?;
        if let Some(members) = response.as_object_mut() {
            members.insert("report".to_owned(), serde_json::to_value(&report)?);
        }
        Ok(response)
    }
}

/// Who this process claims a due item as.
///
/// One name per role rather than per invocation: a lease taken by a process that
/// then died has to be recognisable as this role's, and a random owner would make
/// every expired lease anonymous.
const LEASE_OWNER: &str = "content-lifecycle-uploadexpiry";

/// The S3 half of both object-reaching roles.
///
/// It carries `delete_fenced`, `head` and `abort_multipart` and **nothing else**.
/// `delete_fenced` always sends `If-Match`, which the bucket policy also requires:
/// a blind delete fails closed at the service, not only in this code.
#[derive(Clone)]
struct ObjectAdapter {
    client: aws_sdk_s3::Client,
    bucket: String,
    expected_owner: String,
}

#[async_trait::async_trait]
impl DeletableObjects for ObjectAdapter {
    async fn delete_fenced(&self, intent: &DeleteIntent) -> Result<ObjectDeleteResult, String> {
        let outcome = self
            .client
            .delete_object()
            .bucket(&self.bucket)
            .key(&intent.key)
            .if_match(&intent.if_match)
            .expected_bucket_owner(&intent.expected_bucket_owner)
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(ObjectDeleteResult::Deleted),
            Err(error) => {
                let code = error
                    .as_service_error()
                    .and_then(aws_sdk_s3::error::ProvideErrorMetadata::code)
                    .map(str::to_owned)
                    .or_else(|| {
                        aws_sdk_s3::error::ProvideErrorMetadata::code(&error).map(str::to_owned)
                    });
                match code.as_deref() {
                    // The object changed since it was marked: the candidate is
                    // dropped and the body survives.
                    Some("PreconditionFailed") => Ok(ObjectDeleteResult::PreconditionFailed),
                    Some("NoSuchKey" | "NotFound") => Ok(ObjectDeleteResult::Missing),
                    _ => Ok(ObjectDeleteResult::Retryable),
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl UploadObjects for ObjectAdapter {
    async fn head(
        &self,
        upload: &aex_workspace_domain::upload::Upload,
    ) -> aex_workspace_domain::upload::HeadOracle {
        use aex_workspace_domain::upload::{CompletionEvidence, HeadOracle};
        let outcome = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(&upload.object_key)
            .expected_bucket_owner(&self.expected_owner)
            .send()
            .await;
        match outcome {
            Ok(response) => HeadOracle::Present {
                content_length: u64::try_from(response.content_length.unwrap_or_default())
                    .unwrap_or_default(),
                declared_digest: response
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get(aex_content_aws::object_key::METADATA_DIGEST))
                    .and_then(|hex| {
                        aex_wire::ids::ContentHash::parse(&format!("sha256:{hex}")).ok()
                    }),
                evidence: CompletionEvidence {
                    etag: response.e_tag.unwrap_or_default(),
                    checksum_sha256: response.checksum_sha256,
                    checksum_crc64_nvme: response.checksum_crc64_nvme,
                    part_count: response
                        .parts_count
                        .and_then(|count| u64::try_from(count).ok()),
                },
            },
            Err(error) => {
                let code = aws_sdk_s3::error::ProvideErrorMetadata::code(&error);
                match code {
                    // An absent object is evidence that the completion did not
                    // land. Anything else is evidence of nothing at all.
                    Some("NoSuchKey" | "NotFound") => HeadOracle::Absent,
                    _ => HeadOracle::Unavailable,
                }
            }
        }
    }

    async fn abort_multipart(
        &self,
        upload: &aex_workspace_domain::upload::Upload,
    ) -> Result<(), String> {
        let outcome = self
            .client
            .abort_multipart_upload()
            .bucket(&self.bucket)
            .key(&upload.object_key)
            .upload_id(&upload.provider_upload_id)
            .expected_bucket_owner(&self.expected_owner)
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(()),
            Err(error) => match aws_sdk_s3::error::ProvideErrorMetadata::code(&error) {
                // An already-aborted upload is an idempotent success.
                Some("NoSuchUpload") => Ok(()),
                _ => Err(error.to_string()),
            },
        }
    }
}

/// The `regional-registry` half of the upload-expiry role.
#[derive(Clone)]
struct RowAdapter {
    registry: aex_registry_dynamodb::store::RegistryDynamoStore,
}

#[async_trait::async_trait]
impl UploadRows for RowAdapter {
    async fn load(
        &self,
        workspace: aex_wire::ids::WorkspaceId,
        upload: aex_wire::ids::UploadId,
    ) -> Result<Option<aex_workspace_domain::upload::Upload>, aex_session_dynamodb::error::StoreError>
    {
        aex_registry_dynamodb::store::RegistryStore::load_upload(&self.registry, workspace, upload)
            .await
    }

    async fn transition_fenced(
        &self,
        upload: &aex_workspace_domain::upload::Upload,
        from: aex_workspace_domain::upload::UploadState,
        to: aex_workspace_domain::upload::UploadState,
    ) -> Result<(), aex_session_dynamodb::error::StoreError> {
        aex_registry_dynamodb::store::RegistryStore::transition_upload_fenced(
            &self.registry,
            upload.id,
            from,
            to,
            &upload.provider_upload_id,
        )
        .await
    }

    async fn settle_ready(
        &self,
        upload: &aex_workspace_domain::upload::Upload,
    ) -> Result<(), aex_session_dynamodb::error::StoreError> {
        aex_registry_dynamodb::store::RegistryStore::settle_ready(&self.registry, upload).await
    }

    async fn delete_row(
        &self,
        upload: &aex_workspace_domain::upload::Upload,
    ) -> Result<(), aex_session_dynamodb::error::StoreError> {
        aex_registry_dynamodb::store::RegistryStore::delete_upload(&self.registry, upload).await
    }
}

/// Reads the workspace and bucket set one scheduled sweep covers.
fn sweep_request(payload: serde_json::Value) -> Result<SweepRequest, LambdaError> {
    serde_json::from_value(payload).map_err(|error| {
        LambdaError::from(format!(
            "a scheduled sweep names the workspaces and buckets it covers: {error}"
        ))
    })
}

fn page_bound(configured: Option<u64>) -> u32 {
    configured
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(0)
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
            .field("work_table", &self.work_table)
            .field("holds_object_client", &self.objects.is_some())
            .finish_non_exhaustive()
    }
}
