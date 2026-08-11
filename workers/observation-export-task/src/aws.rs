//! The production `DynamoDB` and `S3` adapters behind the two export surfaces.
//!
//! Two properties are structural rather than conventional:
//!
//! - **Nothing customer-supplied reaches an expression string.** Every condition
//!   is built through [`ExpressionBuilder`], which emits generated `#n0` / `:v0`
//!   placeholders.
//! - **`ListParts` is followed to exhaustion.** A truncated listing whose marker
//!   is absent is reported as such, never folded into an empty tail: a short
//!   list looks exactly like a completed upload and would publish a truncated
//!   artifact.
//!
//! This role holds `s3:DeleteObject` nowhere, so no code path here deletes an
//! object. The only cleanup it performs is `AbortMultipartUpload`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use aex_observation_domain::keys::{self, ScopeKey};
use aex_observation_export::{
    Completeness, ExportCheckpoint, ExportMember, Format, PartRecord, Publication, ResumeError,
};
use aex_observation_store_dynamodb::expressions::{ExpressionBuilder, PK, SK};
use aex_observation_store_dynamodb::{
    ExportPairStore, ExportPublish, ExportSettlement, ExportStart,
};
use aex_operation_domain::OperationResult;
use aex_wire::CanonicalJson;
use aex_wire::error::ErrorCode;
use aex_wire::ids::{ContentHash, ExportId, WorkspaceId};
use aex_wire::models::{ExportFormat, TelemetryExportResult};
use aex_wire::types::Timestamp;
use aws_sdk_dynamodb::types::{AttributeValue, ConditionCheck, Put, TransactWriteItem};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart};

use crate::config::Config;
use crate::task::{
    ExportAuthority, ExportLease, ExportObjects, ExportProgress, LeaseOutcome, ObjectHead, Page,
    PartListing, PublishRequest, TaskError,
};

/// The sort key of the export state item.
// The sort key of an export state row is the export identity itself; see
// `aex_observation_domain::keys::export_sk`. The former `STATE` constant put
// every workspace's exports in their own partition, which made a per-workspace
// listing impossible without a scan.
/// The item name used in decode failures for the export state row.
const EXPORT_STATE: &str = "export state";
/// The item name used in decode failures for the checkpoint row.
const EXPORT_CHECKPOINT: &str = "export checkpoint";
/// The item name used in decode failures for one observation.
const OBSERVATION: &str = "observation";

/// The `DynamoDB` side of one export: the lease, the checkpoint, the walk and
/// the publishing update.
pub struct DynamoExportAuthority {
    dynamodb: aws_sdk_dynamodb::Client,
    table: String,
    session_table: String,
    workspace: WorkspaceId,
    export: ExportId,
    lease: Duration,
    reader: regional_observation_api::ObservationReader,
}

impl DynamoExportAuthority {
    /// Binds the adapter to the configured table, bucket and export.
    #[must_use]
    pub fn new(
        dynamodb: aws_sdk_dynamodb::Client,
        s3: &aws_sdk_s3::Client,
        config: &Config,
    ) -> Self {
        let reader = regional_observation_api::ObservationReader::new(
            dynamodb.clone(),
            s3.clone(),
            config.observation_table.clone(),
            config.session_table.clone(),
            config.observation_bucket.clone(),
            0,
            Arc::new(regional_observation_api::counters::ReadCounters::default()),
        );
        Self {
            dynamodb,
            table: config.observation_table.clone(),
            session_table: config.session_table.clone(),
            workspace: config.workspace_id,
            export: config.export_id,
            lease: config.lease,
            reader,
        }
    }

    /// Proves the observation authority is reachable.
    ///
    /// # Errors
    ///
    /// Returns [`TaskError::Provider`] when the table cannot be described.
    pub async fn probe(&self) -> Result<(), TaskError> {
        self.dynamodb
            .describe_table()
            .table_name(&self.table)
            .send()
            .await
            .map_err(|error| TaskError::provider("DescribeTable", error))?;
        Ok(())
    }

    /// Proves the canonical operation authority is reachable.
    pub async fn probe_session(&self) -> Result<(), TaskError> {
        self.dynamodb
            .describe_table()
            .table_name(&self.session_table)
            .send()
            .await
            .map_err(|error| TaskError::provider("DescribeSessionTable", error))?;
        Ok(())
    }

    fn pairs(&self) -> ExportPairStore {
        ExportPairStore::new(
            self.dynamodb.clone(),
            self.table.clone(),
            self.session_table.clone(),
        )
    }

    /// The partition key of this export's items: one partition per workspace.
    fn export_pk(&self) -> String {
        keys::export_pk(self.workspace)
    }

    /// The per-export checkpoint key inside the workspace export partition.
    fn checkpoint_sk(&self) -> String {
        keys::export_checkpoint_sk(self.export)
    }

    /// Reads one item by its exact key.
    async fn get(
        &self,
        pk: &str,
        sk: &str,
    ) -> Result<Option<HashMap<String, AttributeValue>>, TaskError> {
        let response = self
            .dynamodb
            .get_item()
            .table_name(&self.table)
            .key(PK, AttributeValue::S(pk.to_owned()))
            .key(SK, AttributeValue::S(sk.to_owned()))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| TaskError::provider("GetItem", error))?;
        Ok(response.item)
    }

    fn export_query(
        &self,
        lease: &ExportLease,
        limit: usize,
    ) -> Result<
        (
            aex_observation_query::NormalizedQuery,
            aex_observation_query::Plan,
        ),
        TaskError,
    > {
        let query: aex_wire::models::ExportQuery = serde_json::from_str(&lease.normalized_query)
            .map_err(|_| TaskError::Malformed {
                item: EXPORT_STATE,
                attribute: "normalizedQuery",
            })?;
        let synthetic = aex_wire::models::ObservationQuery {
            consistency: query.consistency,
            cursor: None,
            filter: query.filter,
            limit: Some(u32::try_from(limit).unwrap_or(u32::MAX)),
            order: Some(aex_wire::models::ObservationOrder::Ascending),
            signal: query.signal,
            time_range: query.time_range,
        };
        let signals = regional_observation_api::query::signal_for(query.signal, query.signal)
            .map_err(|_| TaskError::Malformed {
                item: EXPORT_STATE,
                attribute: "normalizedQuery",
            })?;
        let normalized = regional_observation_api::query::with_scope(
            regional_observation_api::query::normalize(
                &synthetic,
                signals,
                u16::try_from(limit).unwrap_or(aex_observation_domain::limits::QUERY_MAX_LIMIT),
            )
            .map_err(|_| TaskError::Malformed {
                item: EXPORT_STATE,
                attribute: "normalizedQuery",
            })?,
            &lease.scope,
        );
        let planned = aex_observation_query::plan(
            &normalized,
            aex_observation_query::Budget {
                max_returned: normalized.limit,
                max_items_scanned: u32::MAX,
                max_segments: u16::MAX,
                max_bytes_read: u64::MAX,
            },
        )
        .map_err(|_| TaskError::Malformed {
            item: EXPORT_STATE,
            attribute: "normalizedQuery",
        })?;
        let reconstructed = regional_observation_api::ObservationReader::export_partitions(
            &lease.scope,
            self.workspace,
            &normalized,
            &planned,
        );
        if reconstructed != lease.partitions {
            return Err(TaskError::Malformed {
                item: EXPORT_STATE,
                attribute: "partitions",
            });
        }
        Ok((normalized, planned))
    }
}

#[async_trait::async_trait]
impl ExportAuthority for DynamoExportAuthority {
    async fn take_lease(&self, now: Timestamp) -> Result<LeaseOutcome, TaskError> {
        let deadline = now
            .unix_millis()
            .saturating_add(i64::try_from(self.lease.as_millis()).unwrap_or(i64::MAX));
        match self
            .pairs()
            .start(self.workspace, self.export, now, deadline)
            .await
            .map_err(|error| TaskError::provider("TransactStartExport", error))?
        {
            ExportStart::Taken(item) => Ok(LeaseOutcome::Taken(Box::new(decode_lease(&item)?))),
            ExportStart::Lost { reason } => Ok(LeaseOutcome::Lost { reason }),
        }
    }

    async fn load_progress(&self, fence: u64) -> Result<Option<ExportProgress>, TaskError> {
        let Some(item) = self.get(&self.export_pk(), &self.checkpoint_sk()).await? else {
            return Ok(None);
        };
        let progress = decode_progress(&item)?;
        // A checkpoint written under a *newer* fence means a replacement task
        // already took over; resuming on top of it would corrupt its upload.
        if progress.checkpoint.fence > fence {
            return Err(TaskError::Resume(ResumeError::LeaseLost {
                held: fence,
                observed: progress.checkpoint.fence,
            }));
        }
        Ok(Some(progress))
    }

    async fn record_progress(&self, progress: &ExportProgress) -> Result<(), TaskError> {
        let mut builder = ExpressionBuilder::new();
        let fence = builder.name("fence");
        let held = builder.number(progress.checkpoint.fence);
        let condition = format!("attribute_not_exists({fence}) OR {fence} <= {held}");
        let checkpoint = Put::builder()
            .table_name(&self.table)
            .set_item(Some(encode_progress(
                &self.export_pk(),
                &self.checkpoint_sk(),
                progress,
            )))
            .condition_expression(condition)
            .set_expression_attribute_names(Some(builder.names()))
            .set_expression_attribute_values(Some(builder.values()))
            .build()
            .map_err(|error| TaskError::provider("TransactWriteItems", error))?;

        let mut live = ExpressionBuilder::new();
        let state = live.name("state");
        let generating = live.string("generating");
        let fence = live.name("fence");
        let held = live.number(progress.checkpoint.fence);
        let cancelled = live.name("cancelRequested");
        let no = live.value(AttributeValue::Bool(false));
        let export = ConditionCheck::builder()
            .table_name(&self.table)
            .key(PK, AttributeValue::S(self.export_pk()))
            .key(SK, AttributeValue::S(keys::export_sk(self.export)))
            .condition_expression(format!(
                "{state} = {generating} AND {fence} = {held} AND {cancelled} = {no}"
            ))
            .set_expression_attribute_names(Some(live.names()))
            .set_expression_attribute_values(Some(live.values()))
            .build()
            .map_err(|error| TaskError::provider("TransactWriteItems", error))?;
        self.dynamodb
            .transact_write_items()
            .transact_items(TransactWriteItem::builder().condition_check(export).build())
            .transact_items(TransactWriteItem::builder().put(checkpoint).build())
            .send()
            .await
            .map_err(|error| TaskError::provider("TransactWriteItems", error))?;
        Ok(())
    }

    async fn next_page(
        &self,
        lease: &ExportLease,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Page, TaskError> {
        let (normalized, planned) = self.export_query(lease, limit)?;
        let resume = cursor
            .map(serde_json::from_str::<aex_observation_query::ObservationResume>)
            .transpose()
            .map_err(|_| TaskError::Malformed {
                item: EXPORT_CHECKPOINT,
                attribute: "cursor",
            })?;
        let snapshot = i64::try_from(lease.snapshot)
            .ok()
            .and_then(|millis| Timestamp::from_unix_millis(millis).ok())
            .map(aex_observation_query::Snapshot::at)
            .ok_or(TaskError::Malformed {
                item: EXPORT_STATE,
                attribute: "snapshot",
            })?;
        let page = self
            .reader
            .read_page(
                &lease.scope,
                self.workspace,
                &normalized,
                &planned,
                snapshot,
                None,
                resume.as_ref(),
                false,
            )
            .await
            .map_err(|error| TaskError::provider("ReadExportPage", error))?;
        let next_cursor = if page.more {
            page.resume
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|_| TaskError::Malformed {
                    item: EXPORT_CHECKPOINT,
                    attribute: "cursor",
                })?
                .map(Into::into)
        } else {
            None
        };
        let records = page
            .items
            .iter()
            .map(|item| {
                aex_wire::to_jcs_bytes(item).map_err(|_| TaskError::Malformed {
                    item: OBSERVATION,
                    attribute: "canonical",
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Page {
            records,
            next_cursor,
        })
    }

    async fn publish(&self, request: &PublishRequest) -> Result<Publication, TaskError> {
        let format = match request.format {
            Format::Ndjson => ExportFormat::Ndjson,
            Format::OtlpJson => ExportFormat::OtlpJson,
            Format::Parquet => {
                return Err(TaskError::Malformed {
                    item: EXPORT_STATE,
                    attribute: "format",
                });
            }
        };
        let digest = if request.manifest_hash.starts_with("sha256:") {
            request.manifest_hash.to_string()
        } else {
            format!("sha256:{}", request.manifest_hash)
        };
        let result = TelemetryExportResult {
            expires_at: request.expires_at,
            export_id: self.export,
            format,
            manifest_hash: ContentHash::parse(&digest).map_err(|_| TaskError::Malformed {
                item: EXPORT_STATE,
                attribute: "manifestHash",
            })?,
        };
        let canonical = aex_wire::to_jcs_bytes(&result).map_err(|_| TaskError::Malformed {
            item: EXPORT_STATE,
            attribute: "operationResult",
        })?;
        let content = CanonicalJson::parse(std::str::from_utf8(&canonical).map_err(|_| {
            TaskError::Malformed {
                item: EXPORT_STATE,
                attribute: "operationResult",
            }
        })?)
        .map_err(|_| TaskError::Malformed {
            item: EXPORT_STATE,
            attribute: "operationResult",
        })?;
        match self
            .pairs()
            .publish(
                self.workspace,
                self.export,
                &ExportPublish {
                    fence: request.fence,
                    object_key: request.object_key.clone(),
                    manifest_hash: digest.into(),
                    object_bytes: request.object_bytes,
                    ready_at: request.ready_at,
                    result: OperationResult {
                        measurement: None,
                        content: Some(content),
                    },
                },
            )
            .await
            .map_err(|error| TaskError::provider("TransactPublishExport", error))?
        {
            ExportSettlement::Settled => Ok(Publication::Ready),
            ExportSettlement::Superseded { reason } => Ok(Publication::Superseded { reason }),
        }
    }

    async fn fail(
        &self,
        lease: &ExportLease,
        error: &TaskError,
        now: Timestamp,
    ) -> Result<ExportSettlement, TaskError> {
        let class = if matches!(error, TaskError::Provider { .. }) {
            aex_operation_domain::FailureClass::Retryable
        } else {
            aex_operation_domain::FailureClass::Terminal
        };
        let code = match error {
            TaskError::Manifest(aex_observation_export::ManifestError::Incomplete { .. }) => {
                ErrorCode::TelemetryIncomplete
            }
            TaskError::Manifest(aex_observation_export::ManifestError::UnsupportedSignal {
                ..
            })
            | TaskError::Encode(aex_observation_export::EncodeError::Unsupported { .. }) => {
                ErrorCode::UnsupportedExportSignal
            }
            _ => error.code().unwrap_or(ErrorCode::InternalError),
        };
        self.pairs()
            .fail(
                self.workspace,
                self.export,
                lease.fence,
                aex_operation_domain::OperationFailure::bare(code, class),
                now,
            )
            .await
            .map_err(|failure| TaskError::provider("TransactFailExport", failure))
    }
}

/// The `S3` side of one export: the multipart upload and its verification.
pub struct S3ExportObjects {
    client: aws_sdk_s3::Client,
    bucket: String,
}

impl S3ExportObjects {
    /// Binds the adapter to the configured bucket.
    #[must_use]
    pub fn new(client: aws_sdk_s3::Client, bucket: String) -> Self {
        Self { client, bucket }
    }

    /// Proves the observation bucket is reachable.
    ///
    /// # Errors
    ///
    /// Returns [`TaskError::Provider`] when the bucket cannot be headed.
    pub async fn probe(&self) -> Result<(), TaskError> {
        self.client
            .head_bucket()
            .bucket(&self.bucket)
            .send()
            .await
            .map_err(|error| TaskError::provider("HeadBucket", error))?;
        Ok(())
    }

    /// Reads one page of the part listing.
    async fn list_page(
        &self,
        key: &str,
        upload_id: &str,
        marker: Option<&str>,
    ) -> Result<aws_sdk_s3::operation::list_parts::ListPartsOutput, TaskError> {
        let mut request = self
            .client
            .list_parts()
            .bucket(&self.bucket)
            .key(key)
            .upload_id(upload_id);
        if let Some(marker) = marker {
            request = request.part_number_marker(marker.to_owned());
        }
        request
            .send()
            .await
            .map_err(|error| TaskError::provider("ListParts", error))
    }
}

#[async_trait::async_trait]
impl ExportObjects for S3ExportObjects {
    async fn create_upload(&self, key: &str) -> Result<Box<str>, TaskError> {
        let response = self
            .client
            .create_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|error| TaskError::provider("CreateMultipartUpload", error))?;
        response
            .upload_id
            .map(Into::into)
            .ok_or(TaskError::Malformed {
                item: "multipart upload",
                attribute: "uploadId",
            })
    }

    async fn upload_part(
        &self,
        key: &str,
        upload_id: &str,
        number: u32,
        body: Vec<u8>,
    ) -> Result<Box<str>, TaskError> {
        let response = self
            .client
            .upload_part()
            .bucket(&self.bucket)
            .key(key)
            .upload_id(upload_id)
            .part_number(part_number(number)?)
            .body(ByteStream::from(body))
            .send()
            .await
            .map_err(|error| TaskError::provider("UploadPart", error))?;
        response
            .e_tag
            .map(|etag| etag_of(&etag))
            .ok_or(TaskError::Malformed {
                item: "uploaded part",
                attribute: "eTag",
            })
    }

    async fn list_parts(&self, key: &str, upload_id: &str) -> Result<PartListing, TaskError> {
        let mut marker: Option<String> = None;
        let mut parts = Vec::new();
        loop {
            let response = self.list_page(key, upload_id, marker.as_deref()).await?;
            for part in response.parts() {
                parts.push(PartRecord {
                    number: u32::try_from(part.part_number.unwrap_or_default()).unwrap_or_default(),
                    etag: part.e_tag.as_deref().map(etag_of).unwrap_or_default(),
                    // The provider does not carry our digest; the checkpoint is
                    // the authority for it and the comparison is over the part
                    // number, size and `ETag`.
                    sha256: Box::from(""),
                    bytes: u64::try_from(part.size.unwrap_or_default()).unwrap_or_default(),
                });
            }
            if !response.is_truncated.unwrap_or(false) {
                return Ok(PartListing {
                    parts,
                    truncated_without_marker: false,
                });
            }
            match response.next_part_number_marker.as_deref() {
                Some(next) if !next.is_empty() => marker = Some(next.to_owned()),
                // Truncated with no way to continue: a short list would look
                // exactly like a completed upload, so it is refused instead.
                _ => {
                    return Ok(PartListing {
                        parts,
                        truncated_without_marker: true,
                    });
                }
            }
        }
    }

    async fn complete_upload(
        &self,
        key: &str,
        upload_id: &str,
        parts: &[PartRecord],
    ) -> Result<Box<str>, TaskError> {
        let mut completed = Vec::with_capacity(parts.len());
        for part in parts {
            completed.push(
                CompletedPart::builder()
                    .part_number(part_number(part.number)?)
                    .e_tag(part.etag.as_ref())
                    .build(),
            );
        }
        let response = self
            .client
            .complete_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .upload_id(upload_id)
            .multipart_upload(
                CompletedMultipartUpload::builder()
                    .set_parts(Some(completed))
                    .build(),
            )
            .send()
            .await
            .map_err(|error| TaskError::provider("CompleteMultipartUpload", error))?;
        response
            .e_tag
            .map(|etag| etag_of(&etag))
            .ok_or(TaskError::Malformed {
                item: "completed upload",
                attribute: "eTag",
            })
    }

    async fn abort_upload(&self, key: &str, upload_id: &str) -> Result<(), TaskError> {
        self.client
            .abort_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .upload_id(upload_id)
            .send()
            .await
            .map_err(|error| TaskError::provider("AbortMultipartUpload", error))?;
        Ok(())
    }

    async fn head_object(&self, key: &str) -> Result<ObjectHead, TaskError> {
        let response = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|error| TaskError::provider("HeadObject", error))?;
        Ok(ObjectHead {
            bytes: u64::try_from(response.content_length.unwrap_or_default()).unwrap_or_default(),
            checksum: response
                .e_tag
                .as_deref()
                .map_or_else(|| Box::from(""), etag_of),
        })
    }
}

/// The provider `ETag`, without the quoting the wire adds.
fn etag_of(raw: &str) -> Box<str> {
    Box::from(raw.trim_matches('"'))
}

/// One part number, as the provider spells it.
fn part_number(number: u32) -> Result<i32, TaskError> {
    i32::try_from(number).map_err(|_| TaskError::PartLimit {
        limit: crate::task::PART_NUMBER_MAX,
    })
}

/// Reads a string attribute.
fn string<'a>(item: &'a HashMap<String, AttributeValue>, name: &str) -> Option<&'a str> {
    match item.get(name) {
        Some(AttributeValue::S(value)) => Some(value),
        _ => None,
    }
}

/// Reads a numeric attribute.
fn number(item: &HashMap<String, AttributeValue>, name: &str) -> Option<u64> {
    match item.get(name) {
        Some(AttributeValue::N(value)) => value.parse().ok(),
        _ => None,
    }
}

/// Reads a list-of-strings attribute.
fn strings(item: &HashMap<String, AttributeValue>, name: &str) -> Vec<String> {
    match item.get(name) {
        Some(AttributeValue::L(values)) => values
            .iter()
            .filter_map(|value| match value {
                AttributeValue::S(text) => Some(text.clone()),
                _ => None,
            })
            .collect(),
        Some(AttributeValue::Ss(values)) => values.clone(),
        _ => Vec::new(),
    }
}

/// Reads a required string attribute.
fn require_string<'a>(
    item: &'a HashMap<String, AttributeValue>,
    which: &'static str,
    name: &'static str,
) -> Result<&'a str, TaskError> {
    string(item, name).ok_or(TaskError::Malformed {
        item: which,
        attribute: name,
    })
}

/// Reads a required numeric attribute.
fn require_number(
    item: &HashMap<String, AttributeValue>,
    which: &'static str,
    name: &'static str,
) -> Result<u64, TaskError> {
    number(item, name).ok_or(TaskError::Malformed {
        item: which,
        attribute: name,
    })
}

/// Decodes the export row the lease returned.
fn decode_lease(item: &HashMap<String, AttributeValue>) -> Result<ExportLease, TaskError> {
    let scope = ScopeKey::parse(require_string(item, EXPORT_STATE, "scopeKey")?).map_err(|_| {
        TaskError::Malformed {
            item: EXPORT_STATE,
            attribute: "scopeKey",
        }
    })?;
    let format = decode_format(require_string(item, EXPORT_STATE, "format")?)?;
    let completeness = decode_completeness(require_string(item, EXPORT_STATE, "completeness")?)?;
    let expires_at = decode_timestamp(require_string(item, EXPORT_STATE, "expiresAt")?)?;
    Ok(ExportLease {
        fence: require_number(item, EXPORT_STATE, "fence")?,
        scope,
        snapshot: u128::from(require_number(item, EXPORT_STATE, "snapshot")?),
        format,
        completeness,
        normalized_query: require_string(item, EXPORT_STATE, "normalizedQuery")?.to_owned(),
        gaps: strings(item, "gaps"),
        pinned_deletion_epoch: require_number(item, EXPORT_STATE, "deletionEpochPinned")?,
        expires_at,
        partitions: strings(item, "partitions"),
    })
}

/// Decodes the requested format.
fn decode_format(raw: &str) -> Result<Format, TaskError> {
    Format::ALL
        .iter()
        .copied()
        .find(|format| format.as_str() == raw)
        .ok_or(TaskError::Malformed {
            item: EXPORT_STATE,
            attribute: "format",
        })
}

/// Decodes the completeness rule.
fn decode_completeness(raw: &str) -> Result<Completeness, TaskError> {
    [Completeness::Require, Completeness::AllowGaps]
        .into_iter()
        .find(|rule| rule.as_str() == raw)
        .ok_or(TaskError::Malformed {
            item: EXPORT_STATE,
            attribute: "completeness",
        })
}

/// Decodes a wire timestamp.
fn decode_timestamp(raw: &str) -> Result<Timestamp, TaskError> {
    Timestamp::parse(raw).map_err(|_| TaskError::Malformed {
        item: EXPORT_STATE,
        attribute: "expiresAt",
    })
}

/// Writes the checkpoint item.
fn encode_progress(
    pk: &str,
    sk: &str,
    progress: &ExportProgress,
) -> HashMap<String, AttributeValue> {
    let checkpoint = &progress.checkpoint;
    HashMap::from([
        (PK.to_owned(), AttributeValue::S(pk.to_owned())),
        (SK.to_owned(), AttributeValue::S(sk.to_owned())),
        (
            "itemType".to_owned(),
            AttributeValue::S("export_checkpoint".to_owned()),
        ),
        (
            "fence".to_owned(),
            AttributeValue::N(checkpoint.fence.to_string()),
        ),
        (
            "uploadId".to_owned(),
            AttributeValue::S(checkpoint.upload_id.to_string()),
        ),
        (
            "parts".to_owned(),
            AttributeValue::L(checkpoint.parts.iter().map(encode_part).collect()),
        ),
        (
            "members".to_owned(),
            AttributeValue::L(progress.members.iter().map(encode_member).collect()),
        ),
        (
            "cursor".to_owned(),
            AttributeValue::S(checkpoint.cursor.to_string()),
        ),
        (
            "memberOrdinal".to_owned(),
            AttributeValue::N(checkpoint.member_ordinal.to_string()),
        ),
        (
            "memberCheckpoint".to_owned(),
            AttributeValue::N(checkpoint.member_checkpoint.to_string()),
        ),
        (
            "bytesWritten".to_owned(),
            AttributeValue::N(checkpoint.bytes_written.to_string()),
        ),
    ])
}

/// Writes one part record.
fn encode_part(part: &PartRecord) -> AttributeValue {
    AttributeValue::M(HashMap::from([
        (
            "number".to_owned(),
            AttributeValue::N(part.number.to_string()),
        ),
        ("etag".to_owned(), AttributeValue::S(part.etag.to_string())),
        (
            "sha256".to_owned(),
            AttributeValue::S(part.sha256.to_string()),
        ),
        (
            "bytes".to_owned(),
            AttributeValue::N(part.bytes.to_string()),
        ),
    ]))
}

/// Writes one member record.
fn encode_member(member: &ExportMember) -> AttributeValue {
    AttributeValue::M(HashMap::from([
        (
            "ordinal".to_owned(),
            AttributeValue::N(member.ordinal.to_string()),
        ),
        (
            "name".to_owned(),
            AttributeValue::S(member.name.to_string()),
        ),
        (
            "bytes".to_owned(),
            AttributeValue::N(member.bytes.to_string()),
        ),
        (
            "sha256".to_owned(),
            AttributeValue::S(member.sha256.to_string()),
        ),
        (
            "records".to_owned(),
            AttributeValue::N(member.records.to_string()),
        ),
    ]))
}

/// Reads the checkpoint item back.
fn decode_progress(item: &HashMap<String, AttributeValue>) -> Result<ExportProgress, TaskError> {
    let parts = match item.get("parts") {
        Some(AttributeValue::L(values)) => values
            .iter()
            .map(decode_part)
            .collect::<Result<Vec<_>, TaskError>>()?,
        _ => Vec::new(),
    };
    let members = match item.get("members") {
        Some(AttributeValue::L(values)) => values
            .iter()
            .map(decode_member)
            .collect::<Result<Vec<_>, TaskError>>()?,
        _ => Vec::new(),
    };
    Ok(ExportProgress {
        checkpoint: ExportCheckpoint {
            fence: require_number(item, EXPORT_CHECKPOINT, "fence")?,
            upload_id: Box::from(require_string(item, EXPORT_CHECKPOINT, "uploadId")?),
            parts,
            cursor: Box::from(string(item, "cursor").unwrap_or_default()),
            member_ordinal: u16::try_from(require_number(
                item,
                EXPORT_CHECKPOINT,
                "memberOrdinal",
            )?)
            .map_err(|_| TaskError::Malformed {
                item: EXPORT_CHECKPOINT,
                attribute: "memberOrdinal",
            })?,
            member_checkpoint: require_number(item, EXPORT_CHECKPOINT, "memberCheckpoint")?,
            bytes_written: require_number(item, EXPORT_CHECKPOINT, "bytesWritten")?,
        },
        members,
    })
}

/// Reads one part record back.
fn decode_part(value: &AttributeValue) -> Result<PartRecord, TaskError> {
    let AttributeValue::M(item) = value else {
        return Err(TaskError::Malformed {
            item: EXPORT_CHECKPOINT,
            attribute: "parts",
        });
    };
    Ok(PartRecord {
        number: u32::try_from(require_number(item, EXPORT_CHECKPOINT, "number")?).map_err(
            |_| TaskError::Malformed {
                item: EXPORT_CHECKPOINT,
                attribute: "number",
            },
        )?,
        etag: Box::from(require_string(item, EXPORT_CHECKPOINT, "etag")?),
        sha256: Box::from(string(item, "sha256").unwrap_or_default()),
        bytes: require_number(item, EXPORT_CHECKPOINT, "bytes")?,
    })
}

/// Reads one member record back.
fn decode_member(value: &AttributeValue) -> Result<ExportMember, TaskError> {
    let AttributeValue::M(item) = value else {
        return Err(TaskError::Malformed {
            item: EXPORT_CHECKPOINT,
            attribute: "members",
        });
    };
    Ok(ExportMember {
        ordinal: u16::try_from(require_number(item, EXPORT_CHECKPOINT, "ordinal")?).map_err(
            |_| TaskError::Malformed {
                item: EXPORT_CHECKPOINT,
                attribute: "ordinal",
            },
        )?,
        name: Box::from(require_string(item, EXPORT_CHECKPOINT, "name")?),
        bytes: require_number(item, EXPORT_CHECKPOINT, "bytes")?,
        sha256: Box::from(require_string(item, EXPORT_CHECKPOINT, "sha256")?),
        records: require_number(item, EXPORT_CHECKPOINT, "records")?,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use aex_observation_export::{
        Completeness, ExportCheckpoint, ExportMember, Format, PartRecord,
    };
    use aex_observation_store_dynamodb::expressions::{PK, SK};
    use aws_sdk_dynamodb::types::AttributeValue;

    use super::{
        ExportProgress, decode_completeness, decode_format, decode_progress, decode_timestamp,
        encode_progress, etag_of, part_number,
    };
    use crate::task::TaskError;

    fn progress() -> ExportProgress {
        ExportProgress {
            checkpoint: ExportCheckpoint {
                fence: 4,
                upload_id: "upload-9".into(),
                parts: vec![PartRecord {
                    number: 1,
                    etag: "etag-1".into(),
                    sha256: "a".repeat(64).into(),
                    bytes: 5 << 20,
                }],
                cursor: "0|2026-08-01T00:00:00.000Z#abcd1234#7\u{1}".into(),
                member_ordinal: 1,
                member_checkpoint: 12,
                bytes_written: 5 << 20,
            },
            members: vec![ExportMember {
                ordinal: 0,
                name: "0000.jsonl".into(),
                bytes: 5 << 20,
                sha256: "b".repeat(64).into(),
                records: 12,
            }],
        }
    }

    #[test]
    fn the_checkpoint_round_trips_through_the_item_encoding() {
        let item = encode_progress("EXPORT#wsp", "CKPT#exp", &progress());
        assert_eq!(
            item.get(PK),
            Some(&AttributeValue::S("EXPORT#wsp".to_owned()))
        );
        assert_eq!(
            item.get(SK),
            Some(&AttributeValue::S("CKPT#exp".to_owned()))
        );
        let decoded = decode_progress(&item).expect("the checkpoint decodes");
        assert_eq!(decoded, progress());
    }

    #[test]
    fn a_checkpoint_missing_its_upload_identity_is_malformed_rather_than_guessed() {
        let mut item = encode_progress("EXPORT#wsp", "CKPT#exp", &progress());
        item.remove("uploadId");
        assert!(matches!(
            decode_progress(&item),
            Err(TaskError::Malformed {
                attribute: "uploadId",
                ..
            })
        ));
    }

    #[test]
    fn the_provider_etag_is_stored_without_its_quoting() {
        assert_eq!(etag_of("\"abc123\"").as_ref(), "abc123");
        assert_eq!(etag_of("abc123").as_ref(), "abc123");
        assert_eq!(part_number(1).expect("a part number"), 1);
    }

    #[test]
    fn a_format_or_completeness_outside_the_vocabulary_is_refused() {
        assert_eq!(decode_format("ndjson").expect("ndjson"), Format::Ndjson);
        assert_eq!(decode_format("parquet").expect("parquet"), Format::Parquet);
        assert!(matches!(
            decode_format("csv"),
            Err(TaskError::Malformed {
                attribute: "format",
                ..
            })
        ));
        assert_eq!(
            decode_completeness("require").expect("require"),
            Completeness::Require
        );
        assert!(matches!(
            decode_completeness("best_effort"),
            Err(TaskError::Malformed { .. })
        ));
    }

    #[test]
    fn a_malformed_expiry_is_refused_rather_than_defaulted() {
        assert!(decode_timestamp("2026-08-01T00:00:00.000Z").is_ok());
        assert!(matches!(
            decode_timestamp("tomorrow"),
            Err(TaskError::Malformed { .. })
        ));
    }

    #[test]
    fn nothing_here_reads_an_attribute_that_does_not_exist() {
        let empty: HashMap<String, AttributeValue> = HashMap::new();
        assert!(matches!(
            decode_progress(&empty),
            Err(TaskError::Malformed { .. })
        ));
    }
}
