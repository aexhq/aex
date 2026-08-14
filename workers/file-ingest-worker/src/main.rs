//! DynamoDB-stream composition for latest-only URL file ingestion.

use std::process::ExitCode;

use aex_content_aws::object_store::{
    BucketBinding, ContentObjectStore as _, PutImmutable, S3ContentObjects,
};
use aex_content_dynamodb::codec::{ContentDescriptor, ContentPin, ObjectLocation};
use aex_content_dynamodb::store::{ContentMetadataStore as _, ContentStore};
use aex_content_dynamodb::wire_pending::PinOwner;
use aex_registry_dynamodb::store::{RegistryDynamoStore, RegistryStore as _};
use aex_session_dynamodb::error::StoreError;
use aex_wire::CanonicalJson;
use aex_wire::ids::ContentHash;
use aex_wire::types::Timestamp;
use aex_workspace_domain::registry::{
    ProposedValue, RegisteredValueRef, RegistryState, ValueDocument, set,
};
use aws_lambda_events::event::dynamodb::Event;
use aws_lambda_events::event::streams::DynamoDbBatchItemFailure;
use file_ingest_worker::{PendingError, fetch_url, pending_url, ready_document, stream_candidate};
use lambda_runtime::{Error as LambdaError, LambdaEvent, service_fn};
use serde::Serialize;

const DEPLOYABLE: &str = "file-ingest-worker";

#[derive(Debug, Clone)]
struct Config {
    file_authority_table: String,
    content_bucket: String,
    content_bucket_owner: String,
    content_kms_key: String,
}

impl Config {
    fn from_env() -> Result<Self, String> {
        let required = |name: &str| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| format!("required environment variable `{name}` is absent"))
        };
        let config = Self {
            file_authority_table: required("AEX_FILE_AUTHORITY_TABLE")?,
            content_bucket: required("AEX_CONTENT_BUCKET")?,
            content_bucket_owner: required("AEX_CONTENT_BUCKET_OWNER")?,
            content_kms_key: required("AEX_CONTENT_KMS_KEY_ARN")?,
        };
        if config.content_bucket_owner.len() != 12
            || !config
                .content_bucket_owner
                .bytes()
                .all(|byte| byte.is_ascii_digit())
        {
            return Err("`AEX_CONTENT_BUCKET_OWNER` must be a 12-digit AWS account id".to_owned());
        }
        Ok(config)
    }
}

#[derive(Clone)]
struct Processor {
    registry: RegistryDynamoStore,
    content: ContentStore,
    objects: S3ContentObjects,
    kms_key_id: String,
}

impl Processor {
    async fn process(&self, locator: file_ingest_worker::StreamLocator) -> Result<(), String> {
        let Some(current) = self
            .registry
            .load_pointer(
                locator.workspace_id,
                aex_content_domain::identity::RegistryKind::File,
                locator.name.as_str(),
            )
            .await
            .map_err(|error| error.to_string())?
        else {
            return Ok(());
        };
        let pending = match pending_url(&current) {
            Ok(pending) => pending,
            Err(PendingError::NotPendingUrl) => return Ok(()),
            Err(PendingError::Malformed) => {
                return self.publish_failed(&current, "invalid_source").await;
            }
        };
        let fetched = match fetch_url(&pending.source.url).await {
            Ok(fetched) => fetched,
            Err(error) => return self.publish_failed(&current, error.code()).await,
        };
        let digest = ContentHash::of(&fetched.bytes);
        let size_bytes = u64::try_from(fetched.bytes.len())
            .map_err(|_| "fetched file size is outside the durable range".to_owned())?;
        let encryption_context = format!(
            r#"{{"aex:domain":"content-object","aex:workspace":"{}"}}"#,
            current.row.workspace
        );
        let object = self
            .objects
            .put_immutable(PutImmutable {
                workspace: current.row.workspace,
                digest: &digest,
                plaintext_bytes: size_bytes,
                body: fetched.bytes,
                encryption_context: encryption_context.as_bytes(),
            })
            .await
            .map_err(|error| error.to_string())?;
        let now = now()?;
        let descriptor = ContentDescriptor {
            workspace: current.row.workspace,
            organization: pending.organization_id,
            digest,
            size_bytes,
            media_type: pending.media_type.clone(),
            placement: aex_session_dynamodb::measure::Placement::ObjectStore,
            state: "committed".to_owned(),
            gc_epoch: 0,
            created_at: now,
            verified_at: Some(now),
            object: Some(ObjectLocation {
                key: object.key.as_str().to_owned(),
                etag: object.etag,
                checksum_sha256: object.checksum_sha256.unwrap_or_default(),
                checksum_crc64_nvme: object.checksum_crc64_nvme.unwrap_or_default(),
                part_count: object.part_count,
                kms_key_id: self.kms_key_id.clone(),
            }),
        };
        // A stale worker may leave an extra immutable body/pin. It may not move
        // the current pointer; reference-aware lifecycle cleanup reclaims it.
        self.content
            .admit_body(
                &descriptor,
                &ContentPin {
                    workspace: current.row.workspace,
                    owner: PinOwner::Registry {
                        kind: "file".to_owned(),
                        name: current.row.name.as_str().to_owned(),
                    },
                    created_at: now,
                },
                now,
            )
            .await
            .map_err(|error| error.to_string())?;
        let ready = ready_document(&pending, digest, size_bytes);
        let value_doc = value_document(&ready)?;
        let proposal = ProposedValue {
            workspace: current.row.workspace,
            kind: current.row.kind,
            name: current.row.name.clone(),
            value_doc,
            payload: Some(RegisteredValueRef::Content { digest }),
            upload_state: None,
            state: RegistryState::Ready,
            failure_code: None,
        };
        let (_, commit) = set(Some(&current), &proposal, Some(&current.row.etag), now)
            .map_err(|error| error.to_string())?;
        self.replace_or_ignore_stale(&current, &commit.pointer)
            .await
    }

    async fn publish_failed(
        &self,
        current: &aex_workspace_domain::registry::RegistryPointer,
        code: &str,
    ) -> Result<(), String> {
        let now = now()?;
        let proposal = ProposedValue {
            workspace: current.row.workspace,
            kind: current.row.kind,
            name: current.row.name.clone(),
            value_doc: value_document(&serde_json::json!({ "failureCode": code }))?,
            payload: None,
            upload_state: None,
            state: RegistryState::Failed,
            failure_code: Some(code.to_owned()),
        };
        let (_, commit) = set(Some(current), &proposal, Some(&current.row.etag), now)
            .map_err(|error| error.to_string())?;
        self.replace_or_ignore_stale(current, &commit.pointer).await
    }

    async fn replace_or_ignore_stale(
        &self,
        current: &aex_workspace_domain::registry::RegistryPointer,
        next: &aex_workspace_domain::registry::RegistryPointer,
    ) -> Result<(), String> {
        match self.registry.replace_current(current, next).await {
            Ok(()) | Err(StoreError::PreconditionFailed { .. }) => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }
}

fn value_document(value: &impl Serialize) -> Result<ValueDocument, String> {
    let bytes = aex_wire::canonical::to_jcs_bytes(value).map_err(|error| error.to_string())?;
    let text = String::from_utf8(bytes).map_err(|error| error.to_string())?;
    CanonicalJson::parse(&text)
        .map(ValueDocument::new)
        .map_err(|error| error.to_string())
}

fn now() -> Result<Timestamp, String> {
    let millis = time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000;
    let millis =
        i64::try_from(millis).map_err(|_| "clock is outside timestamp range".to_owned())?;
    Timestamp::from_unix_millis(millis).map_err(|error| error.to_string())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BatchResponse {
    batch_item_failures: Vec<DynamoDbBatchItemFailure>,
}

async fn handle(processor: &Processor, event: Event) -> BatchResponse {
    let mut failures = Vec::new();
    for record in event.records {
        let Some(locator) = stream_candidate(record.change.new_image) else {
            continue;
        };
        if let Err(error) = processor.process(locator).await {
            tracing::error!(
                target: "aex::diagnostics",
                event_name = "file_ingest.retryable_failure",
                error = %error,
                "file ingest record will be retried"
            );
            let mut failure = DynamoDbBatchItemFailure::default();
            failure.item_identifier = Some(record.event_id);
            failures.push(failure);
        }
    }
    BatchResponse {
        batch_item_failures: failures,
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    if let Err(error) = aex_platform_diagnostics::install_json() {
        eprintln!("{DEPLOYABLE}: diagnostics installation failed: {error}");
        return ExitCode::FAILURE;
    }
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{DEPLOYABLE}: refusing to start: {error}");
            return ExitCode::FAILURE;
        }
    };
    let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let dynamodb = aws_sdk_dynamodb::Client::new(&aws);
    let processor = Processor {
        registry: RegistryDynamoStore::new(dynamodb.clone(), config.file_authority_table.clone()),
        content: ContentStore::new(dynamodb, config.file_authority_table),
        objects: S3ContentObjects::new(
            aws_sdk_s3::Client::new(&aws),
            BucketBinding {
                bucket: config.content_bucket,
                expected_owner: config.content_bucket_owner,
                kms_key_id: config.content_kms_key.clone(),
            },
        ),
        kms_key_id: config.content_kms_key,
    };
    let result = lambda_runtime::run(service_fn(move |event: LambdaEvent<Event>| {
        let processor = processor.clone();
        async move { Ok::<_, LambdaError>(handle(&processor, event.payload).await) }
    }))
    .await;
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{DEPLOYABLE}: runtime stopped: {error}");
            ExitCode::FAILURE
        }
    }
}
