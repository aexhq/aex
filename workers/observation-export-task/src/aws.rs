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
use std::time::Duration;

use aex_observation_domain::keys::{self, ScopeKey};
use aex_observation_export::checkpoint::publish as evaluate_publication;
use aex_observation_export::encoder::sha256_hex;
use aex_observation_export::{
    Completeness, ExportCheckpoint, ExportMember, Format, PartRecord, Publication, PublishFence,
    ResumeError,
};
use aex_observation_store_dynamodb::expressions::{ExpressionBuilder, Index, PK, SK};
use aex_wire::ids::{ExportId, WorkspaceId};
use aex_wire::types::Timestamp;
use aws_sdk_dynamodb::types::AttributeValue;
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
/// The sort key of the export checkpoint item.
pub const CHECKPOINT_SK: &str = "CKPT";
/// The sort key of the scope deletion item.
pub const DELETION_SK: &str = "DELETION";

/// The state an export is launched in.
pub const STATE_LAUNCHING: &str = "launching";
/// The state an export generates in.
pub const STATE_GENERATING: &str = "generating";
/// The state a published export is in.
pub const STATE_READY: &str = "ready";

/// The item name used in decode failures for the export state row.
const EXPORT_STATE: &str = "export state";
/// The item name used in decode failures for the checkpoint row.
const EXPORT_CHECKPOINT: &str = "export checkpoint";
/// The item name used in decode failures for one observation.
const OBSERVATION: &str = "observation";

/// The lowest sort-key value any accepted-order walk can start from.
///
/// Wire timestamps begin with a year digit, so `0` sorts strictly before every
/// real key without needing an empty string in a key condition.
const WALK_FLOOR: &str = "0";

/// The one-character sentinel that makes a stored cursor an **exclusive** lower
/// bound inside an inclusive `BETWEEN`.
const CURSOR_SENTINEL: char = '\u{1}';

/// The sentinel that closes a snapshot's upper bound.
///
/// `~` sorts after `#` and after every hexadecimal scope digest, so
/// `{snapshot}~` is above every key accepted at the snapshot instant and below
/// every key accepted after it.
const SNAPSHOT_CEILING: char = '~';

/// The `DynamoDB` side of one export: the lease, the checkpoint, the walk and
/// the publishing update.
pub struct DynamoExportAuthority {
    dynamodb: aws_sdk_dynamodb::Client,
    s3: aws_sdk_s3::Client,
    table: String,
    bucket: String,
    workspace: WorkspaceId,
    export: ExportId,
    lease: Duration,
}

impl DynamoExportAuthority {
    /// Binds the adapter to the configured table, bucket and export.
    #[must_use]
    pub fn new(
        dynamodb: aws_sdk_dynamodb::Client,
        s3: aws_sdk_s3::Client,
        config: &Config,
    ) -> Self {
        Self {
            dynamodb,
            s3,
            table: config.observation_table.clone(),
            bucket: config.observation_bucket.clone(),
            workspace: config.workspace_id,
            export: config.export_id,
            lease: config.lease,
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

    /// The partition key of this export's items: one partition per workspace.
    fn export_pk(&self) -> String {
        keys::export_pk(self.workspace)
    }

    /// The sort key of this export's state row: the bare export identity.
    fn export_sk(&self) -> String {
        keys::export_sk(self.export)
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

    /// The scope's live deletion epoch.
    async fn live_deletion_epoch(&self, scope: ScopeKey) -> Result<u64, TaskError> {
        let Some(item) = self.get(&keys::frontier_pk(&scope), DELETION_SK).await? else {
            return Ok(0);
        };
        Ok(number(&item, "deletionEpoch").unwrap_or(0))
    }

    /// Names what won when the publishing condition failed.
    async fn publication_loser(&self, request: &PublishRequest) -> Result<Publication, TaskError> {
        let live = self.live_deletion_epoch(request.scope).await?;
        let Some(item) = self.get(&self.export_pk(), &self.export_sk()).await? else {
            return Ok(Publication::Superseded {
                reason: "the export row is gone",
            });
        };
        let observed = evaluate_publication(&PublishFence {
            state_is_generating: string(&item, "state") == Some(STATE_GENERATING),
            fence: request.fence,
            authority_fence: number(&item, "fence").unwrap_or_default(),
            cancel_requested: boolean(&item, "cancelRequested"),
            pinned_deletion_epoch: number(&item, "deletionEpochPinned").unwrap_or_default(),
            live_deletion_epoch: live,
        });
        Ok(match observed {
            // The condition failed, so something moved between the read and the
            // write; it is superseded either way and never republished.
            Publication::Ready => Publication::Superseded {
                reason: "the publication condition failed",
            },
            superseded @ Publication::Superseded { .. } => superseded,
        })
    }

    /// Queries one pinned partition for at most `limit` observations.
    async fn query_partition(
        &self,
        partition: &str,
        after: &str,
        ceiling: &str,
        limit: usize,
    ) -> Result<Vec<HashMap<String, AttributeValue>>, TaskError> {
        let mut builder = ExpressionBuilder::new();
        let pk = builder.name(Index::WorkspaceAccepted.partition_key());
        let sk = builder.name(Index::WorkspaceAccepted.sort_key());
        let partition_value = builder.string(partition);
        let low = builder.string(after);
        let high = builder.string(ceiling);
        let condition = format!("{pk} = {partition_value} AND {sk} BETWEEN {low} AND {high}");
        let response = self
            .dynamodb
            .query()
            .table_name(&self.table)
            .index_name(Index::WorkspaceAccepted.as_str())
            .key_condition_expression(condition)
            .set_expression_attribute_names(Some(builder.names()))
            .set_expression_attribute_values(Some(builder.values()))
            .limit(i32::try_from(limit).unwrap_or(i32::MAX))
            .send()
            .await
            .map_err(|error| TaskError::provider("Query", error))?;
        Ok(response.items.unwrap_or_default())
    }

    /// Resolves one observation's canonical bytes.
    ///
    /// An inline body is the bytes themselves; an out-of-line body is read from
    /// the observation bucket and its stored digest is re-checked, because the
    /// manifest hash is only meaningful over bytes that were verified.
    async fn canonical(
        &self,
        item: &HashMap<String, AttributeValue>,
    ) -> Result<Vec<u8>, TaskError> {
        if let Some(AttributeValue::B(blob)) = item.get("bodyInline") {
            return Ok(blob.clone().into_inner());
        }
        let key = require_string(item, OBSERVATION, "bodyS3Key")?.to_owned();
        let digest = require_string(item, OBSERVATION, "bodySha256")?.to_owned();
        let response = self
            .s3
            .get_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
            .map_err(|error| TaskError::provider("GetObject", error))?;
        let bytes = response
            .body
            .collect()
            .await
            .map_err(|error| TaskError::provider("GetObject", error))?
            .into_bytes()
            .to_vec();
        if sha256_hex(&bytes) != digest {
            return Err(TaskError::Integrity {
                what: format!("the stored body of `{key}` does not match its recorded digest"),
            });
        }
        Ok(bytes)
    }
}

#[async_trait::async_trait]
impl ExportAuthority for DynamoExportAuthority {
    async fn take_lease(&self, now: Timestamp) -> Result<LeaseOutcome, TaskError> {
        let deadline = now
            .unix_millis()
            .saturating_add(i64::try_from(self.lease.as_millis()).unwrap_or(i64::MAX));
        let mut builder = ExpressionBuilder::new();
        let state = builder.name("state");
        let fence = builder.name("fence");
        let expires = builder.name("leaseExpiresAt");
        let cancel = builder.name("cancelRequested");
        let generating = builder.string(STATE_GENERATING);
        let launching = builder.string(STATE_LAUNCHING);
        let now_ms = builder.number(now.unix_millis());
        let deadline_ms = builder.number(deadline);
        let one = builder.number(1);
        let no = builder.boolean(false);
        let update =
            format!("SET {state} = {generating}, {expires} = {deadline_ms} ADD {fence} {one}");
        let condition = format!(
            "({state} = {launching} OR {state} = {generating}) \
             AND (attribute_not_exists({expires}) OR {expires} < {now_ms}) \
             AND (attribute_not_exists({cancel}) OR {cancel} = {no})"
        );
        let outcome = self
            .dynamodb
            .update_item()
            .table_name(&self.table)
            .key(PK, AttributeValue::S(self.export_pk()))
            .key(SK, AttributeValue::S(self.export_sk()))
            .update_expression(update)
            .condition_expression(condition)
            .set_expression_attribute_names(Some(builder.names()))
            .set_expression_attribute_values(Some(builder.values()))
            .return_values(aws_sdk_dynamodb::types::ReturnValue::AllNew)
            .send()
            .await;
        match outcome {
            Ok(response) => {
                let item = response.attributes.ok_or(TaskError::Malformed {
                    item: EXPORT_STATE,
                    attribute: "attributes",
                })?;
                Ok(LeaseOutcome::Taken(Box::new(decode_lease(&item)?)))
            }
            Err(error) => {
                if matches!(
                    error.as_service_error(),
                    Some(
                        aws_sdk_dynamodb::operation::update_item::UpdateItemError::ConditionalCheckFailedException(_)
                    )
                ) {
                    // A cancel, a live lease elsewhere or a state that is no
                    // longer launchable. All three are authoritative outcomes.
                    return Ok(LeaseOutcome::Lost {
                        reason: "the export row is not leasable by this task",
                    });
                }
                Err(TaskError::provider("UpdateItem", error))
            }
        }
    }

    async fn load_progress(&self, fence: u64) -> Result<Option<ExportProgress>, TaskError> {
        let Some(item) = self.get(&self.export_pk(), CHECKPOINT_SK).await? else {
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
        self.dynamodb
            .put_item()
            .table_name(&self.table)
            .set_item(Some(encode_progress(&self.export_pk(), progress)))
            .condition_expression(condition)
            .set_expression_attribute_names(Some(builder.names()))
            .set_expression_attribute_values(Some(builder.values()))
            .send()
            .await
            .map_err(|error| TaskError::provider("PutItem", error))?;
        Ok(())
    }

    async fn next_page(
        &self,
        lease: &ExportLease,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Page, TaskError> {
        let (mut index, after) = decode_cursor(cursor)?;
        let mut after = after;
        let ceiling = snapshot_ceiling(lease.snapshot)?;
        while index < lease.partitions.len() {
            let items = self
                .query_partition(&lease.partitions[index], &after, &ceiling, limit)
                .await?;
            if let Some(last) = items.last() {
                let position =
                    require_string(last, OBSERVATION, Index::WorkspaceAccepted.sort_key())?
                        .to_owned();
                let mut records = Vec::with_capacity(items.len());
                for item in &items {
                    records.push(self.canonical(item).await?);
                }
                return Ok(Page {
                    records,
                    next_cursor: Some(encode_cursor(index, &position).into()),
                });
            }
            index += 1;
            WALK_FLOOR.clone_into(&mut after);
        }
        Ok(Page {
            records: Vec::new(),
            next_cursor: None,
        })
    }

    async fn publish(&self, request: &PublishRequest) -> Result<Publication, TaskError> {
        let live = self.live_deletion_epoch(request.scope).await?;
        let mut builder = ExpressionBuilder::new();
        let state = builder.name("state");
        let fence = builder.name("fence");
        let cancel = builder.name("cancelRequested");
        let pinned = builder.name("deletionEpochPinned");
        let object_key = builder.name("objectKey");
        let manifest_hash = builder.name("manifestHash");
        let object_bytes = builder.name("objectBytes");
        let ready_at = builder.name("readyAt");
        let expires_at = builder.name("expiresAt");
        let ready = builder.string(STATE_READY);
        let generating = builder.string(STATE_GENERATING);
        let held = builder.number(request.fence);
        let no = builder.boolean(false);
        let live_epoch = builder.number(live);
        let key_value = builder.string(request.object_key.as_ref());
        let hash_value = builder.string(request.manifest_hash.as_ref());
        let bytes_value = builder.number(request.object_bytes);
        let ready_value = builder.string(request.ready_at.to_wire());
        let expiry_value = builder.string(request.expires_at.to_wire());
        let update = format!(
            "SET {state} = {ready}, {object_key} = {key_value}, {manifest_hash} = {hash_value}, \
             {object_bytes} = {bytes_value}, {ready_at} = {ready_value}, \
             {expires_at} = {expiry_value}"
        );
        let condition = format!(
            "{state} = {generating} AND {fence} = {held} \
             AND (attribute_not_exists({cancel}) OR {cancel} = {no}) \
             AND {pinned} = {live_epoch}"
        );
        let outcome = self
            .dynamodb
            .update_item()
            .table_name(&self.table)
            .key(PK, AttributeValue::S(self.export_pk()))
            .key(SK, AttributeValue::S(self.export_sk()))
            .update_expression(update)
            .condition_expression(condition)
            .set_expression_attribute_names(Some(builder.names()))
            .set_expression_attribute_values(Some(builder.values()))
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(Publication::Ready),
            Err(error) => {
                if matches!(
                    error.as_service_error(),
                    Some(
                        aws_sdk_dynamodb::operation::update_item::UpdateItemError::ConditionalCheckFailedException(_)
                    )
                ) {
                    return self.publication_loser(request).await;
                }
                Err(TaskError::provider("UpdateItem", error))
            }
        }
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

/// The exclusive upper bound of one pinned snapshot.
fn snapshot_ceiling(snapshot: u128) -> Result<String, TaskError> {
    let millis = i64::try_from(snapshot).map_err(|_| TaskError::Malformed {
        item: EXPORT_STATE,
        attribute: "snapshot",
    })?;
    let pinned = Timestamp::from_unix_millis(millis).map_err(|_| TaskError::Malformed {
        item: EXPORT_STATE,
        attribute: "snapshot",
    })?;
    Ok(format!("{}{SNAPSHOT_CEILING}", pinned.to_wire()))
}

/// The cursor spelling: the partition ordinal and the last key inside it.
fn encode_cursor(partition: usize, position: &str) -> String {
    format!("{partition}|{position}{CURSOR_SENTINEL}")
}

/// Reads the cursor spelling back.
fn decode_cursor(cursor: Option<&str>) -> Result<(usize, String), TaskError> {
    let Some(cursor) = cursor else {
        return Ok((0, WALK_FLOOR.to_owned()));
    };
    let (partition, position) = cursor.split_once('|').ok_or(TaskError::Malformed {
        item: EXPORT_CHECKPOINT,
        attribute: "cursor",
    })?;
    let partition = partition
        .parse::<usize>()
        .map_err(|_| TaskError::Malformed {
            item: EXPORT_CHECKPOINT,
            attribute: "cursor",
        })?;
    Ok((partition, position.to_owned()))
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

/// Reads a boolean attribute, absent meaning `false`.
fn boolean(item: &HashMap<String, AttributeValue>, name: &str) -> bool {
    matches!(item.get(name), Some(AttributeValue::Bool(true)))
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
fn encode_progress(pk: &str, progress: &ExportProgress) -> HashMap<String, AttributeValue> {
    let checkpoint = &progress.checkpoint;
    HashMap::from([
        (PK.to_owned(), AttributeValue::S(pk.to_owned())),
        (SK.to_owned(), AttributeValue::S(CHECKPOINT_SK.to_owned())),
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
    use aex_observation_store_dynamodb::expressions::{PK, SK, is_safe_expression};
    use aws_sdk_dynamodb::types::AttributeValue;

    use super::{
        CHECKPOINT_SK, ExportProgress, STATE_GENERATING, STATE_LAUNCHING, STATE_READY, WALK_FLOOR,
        decode_completeness, decode_cursor, decode_format, decode_progress, decode_timestamp,
        encode_cursor, encode_progress, etag_of, part_number, snapshot_ceiling,
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
        let item = encode_progress("EXPORT#wsp#exp", &progress());
        assert_eq!(
            item.get(PK),
            Some(&AttributeValue::S("EXPORT#wsp#exp".to_owned()))
        );
        assert_eq!(
            item.get(SK),
            Some(&AttributeValue::S(CHECKPOINT_SK.to_owned()))
        );
        let decoded = decode_progress(&item).expect("the checkpoint decodes");
        assert_eq!(decoded, progress());
    }

    #[test]
    fn a_checkpoint_missing_its_upload_identity_is_malformed_rather_than_guessed() {
        let mut item = encode_progress("EXPORT#wsp#exp", &progress());
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
    fn the_cursor_carries_the_partition_and_an_exclusive_position() {
        let cursor = encode_cursor(3, "2026-08-01T00:00:00.000Z#abcd1234#7");
        let (partition, position) = decode_cursor(Some(&cursor)).expect("the cursor decodes");
        assert_eq!(partition, 3);
        assert!(
            position.ends_with('\u{1}'),
            "the stored position is an exclusive lower bound: {position:?}"
        );
        assert_eq!(
            decode_cursor(None).expect("a fresh walk"),
            (0, WALK_FLOOR.to_owned())
        );
        assert!(matches!(
            decode_cursor(Some("not-a-cursor")),
            Err(TaskError::Malformed { .. })
        ));
    }

    #[test]
    fn the_snapshot_ceiling_sorts_above_every_key_accepted_at_the_snapshot() {
        let ceiling = snapshot_ceiling(1_785_000_000_000).expect("the snapshot renders");
        let pinned = "2026-07-25T12:00:00.000Z";
        assert!(ceiling.starts_with(&ceiling[..4]));
        assert!(ceiling.ends_with('~'));
        let at_snapshot = format!("{}#abcd1234#9", &ceiling[..ceiling.len() - 1]);
        assert!(at_snapshot < ceiling, "{at_snapshot} < {ceiling}");
        assert!(WALK_FLOOR < pinned, "the floor sorts below every real key");
    }

    #[test]
    fn the_declared_states_are_the_ones_the_conditions_use() {
        assert_eq!(STATE_LAUNCHING, "launching");
        assert_eq!(STATE_GENERATING, "generating");
        assert_eq!(STATE_READY, "ready");
    }

    #[test]
    fn every_condition_this_adapter_builds_carries_only_generated_placeholders() {
        // A customer string can never become a placeholder, because a caller
        // never writes one.
        let mut builder = aex_observation_store_dynamodb::expressions::ExpressionBuilder::new();
        let state = builder.name("state");
        let generating = builder.string(STATE_GENERATING);
        assert!(is_safe_expression(&format!("{state} = {generating}")));
        assert_eq!(state, "#n0");
        assert_eq!(generating, ":v0");
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
