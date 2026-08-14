//! Immutable S3 checkpoint bodies and fenced DynamoDB head pointers.

use aex_brain_app::ports::{
    BoxFuture, CheckpointError, ContextCheckpointStore, FenceGuard, SessionAuthority,
};
use aex_brain_domain::checkpoint::{CheckpointMetadata, ContextCheckpoint, MAX_CHECKPOINT_BYTES};
use aex_brain_domain::ids::{AgentKey, ContentHash};
use aex_session_dynamodb::attr::{n, s, stamp};
use aex_session_dynamodb::plan::key;
use aws_sdk_dynamodb::types::{TransactWriteItem, Update};
use aws_sdk_s3::error::ProvideErrorMetadata as _;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::ServerSideEncryption;

use crate::{BrainStore, control, plan, translate};

const METADATA_HASH: &str = "aex-checkpoint-hash";
const METADATA_BYTES: &str = "aex-checkpoint-bytes";

/// Immutable checkpoint bucket and encryption authority.
#[derive(Debug, Clone)]
pub struct CheckpointBinding {
    /// S3 client from the runtime's one regional SDK configuration.
    pub client: aws_sdk_s3::Client,
    /// Existing regional customer-content bucket.
    pub bucket: String,
    /// Account that must still own the bucket.
    pub expected_owner: String,
    /// Existing regional customer-data CMK.
    pub kms_key_id: String,
}

/// Renders the only admitted checkpoint object prefix.
#[must_use]
pub fn object_key(key: AgentKey, checkpoint: ContentHash, body: ContentHash) -> String {
    format!(
        "session-content/v1/session={}/agent={}/checkpoint={}/{}.json",
        key.session.0.as_hyphenated(),
        key.agent.0.as_hyphenated(),
        checkpoint.to_hex(),
        body.to_hex(),
    )
}

impl ContextCheckpointStore for BrainStore {
    fn load<'a>(
        &'a self,
        key: AgentKey,
        metadata: &'a CheckpointMetadata,
    ) -> BoxFuture<'a, Result<ContextCheckpoint, CheckpointError>> {
        Box::pin(async move {
            let binding = self
                .checkpoint_binding()
                .ok_or(CheckpointError::Unavailable)?;
            if metadata.object_bytes > MAX_CHECKPOINT_BYTES {
                return Err(CheckpointError::TooLarge);
            }
            let expected_key = object_key(key, metadata.id, metadata.object_hash);
            let response = binding
                .client
                .get_object()
                .bucket(&binding.bucket)
                .key(&expected_key)
                .expected_bucket_owner(&binding.expected_owner)
                .send()
                .await
                .map_err(
                    |error| match error.as_service_error().and_then(|it| it.code()) {
                        Some("NoSuchKey" | "NotFound") => CheckpointError::Missing,
                        _ => CheckpointError::Unavailable,
                    },
                )?;
            let declared = u64::try_from(response.content_length.unwrap_or_default())
                .map_err(|_| CheckpointError::Corrupt)?;
            if declared != metadata.object_bytes || declared > MAX_CHECKPOINT_BYTES {
                return Err(if declared > MAX_CHECKPOINT_BYTES {
                    CheckpointError::TooLarge
                } else {
                    CheckpointError::Corrupt
                });
            }
            let expected_hash = metadata.object_hash.to_hex();
            let expected_bytes = metadata.object_bytes.to_string();
            if response
                .metadata
                .as_ref()
                .and_then(|values| values.get(METADATA_HASH))
                .map(String::as_str)
                != Some(expected_hash.as_str())
                || response
                    .metadata
                    .as_ref()
                    .and_then(|values| values.get(METADATA_BYTES))
                    .map(String::as_str)
                    != Some(expected_bytes.as_str())
                || response.server_side_encryption != Some(ServerSideEncryption::AwsKms)
                || response.ssekms_key_id.as_deref() != Some(binding.kms_key_id.as_str())
            {
                return Err(CheckpointError::Corrupt);
            }
            let bytes = response
                .body
                .collect()
                .await
                .map_err(|_| CheckpointError::Unavailable)?
                .into_bytes();
            if u64::try_from(bytes.len()).ok() != Some(declared)
                || ContentHash::of(&bytes) != metadata.object_hash
            {
                return Err(CheckpointError::Corrupt);
            }
            let checkpoint = serde_json::from_slice::<ContextCheckpoint>(&bytes)
                .map_err(|_| CheckpointError::Corrupt)?;
            if !checkpoint.matches(key, metadata) {
                return Err(CheckpointError::Corrupt);
            }
            Ok(checkpoint)
        })
    }

    fn save<'a>(
        &'a self,
        guard: &'a FenceGuard,
        authority: &'a SessionAuthority,
        checkpoint: &'a ContextCheckpoint,
    ) -> BoxFuture<'a, Result<CheckpointMetadata, CheckpointError>> {
        Box::pin(async move {
            let binding = self
                .checkpoint_binding()
                .ok_or(CheckpointError::Unavailable)?;
            if checkpoint.key != guard.key()
                || checkpoint.covers_through != guard.tail().ok_or(CheckpointError::Conflict)?
            {
                return Err(CheckpointError::Conflict);
            }
            let bytes = serde_json::to_vec(checkpoint).map_err(|_| CheckpointError::Corrupt)?;
            let object_bytes = u64::try_from(bytes.len()).map_err(|_| CheckpointError::TooLarge)?;
            if object_bytes > MAX_CHECKPOINT_BYTES {
                return Err(CheckpointError::TooLarge);
            }
            let object_hash = ContentHash::of(&bytes);
            let metadata = CheckpointMetadata {
                schema_version: checkpoint.schema_version,
                id: checkpoint.id,
                previous: checkpoint.previous,
                covers_through: checkpoint.covers_through,
                covers_hash: checkpoint.covers_hash,
                object_hash,
                object_bytes,
                source_hash: checkpoint.source_hash,
                approximate_tokens: checkpoint.approximate_tokens,
                compactor: checkpoint.compactor.clone(),
                created_at: checkpoint.created_at,
            };
            let object_key = object_key(guard.key(), checkpoint.id, object_hash);
            let put = binding
                .client
                .put_object()
                .bucket(&binding.bucket)
                .key(&object_key)
                .if_none_match("*")
                .content_length(i64::try_from(object_bytes).map_err(|_| CheckpointError::TooLarge)?)
                .server_side_encryption(ServerSideEncryption::AwsKms)
                .ssekms_key_id(&binding.kms_key_id)
                .bucket_key_enabled(true)
                .expected_bucket_owner(&binding.expected_owner)
                .metadata(METADATA_HASH, object_hash.to_hex())
                .metadata(METADATA_BYTES, object_bytes.to_string())
                .body(ByteStream::from(bytes.clone()))
                .send()
                .await;
            if let Err(error) = put {
                if error.as_service_error().and_then(|it| it.code()) != Some("PreconditionFailed") {
                    return Err(CheckpointError::Unavailable);
                }
                let existing = binding
                    .client
                    .get_object()
                    .bucket(&binding.bucket)
                    .key(&object_key)
                    .expected_bucket_owner(&binding.expected_owner)
                    .send()
                    .await
                    .map_err(|_| CheckpointError::Unavailable)?;
                if u64::try_from(existing.content_length.unwrap_or_default()).ok()
                    != Some(object_bytes)
                    || existing.server_side_encryption != Some(ServerSideEncryption::AwsKms)
                    || existing.ssekms_key_id.as_deref() != Some(binding.kms_key_id.as_str())
                {
                    return Err(CheckpointError::Corrupt);
                }
                let existing = existing
                    .body
                    .collect()
                    .await
                    .map_err(|_| CheckpointError::Unavailable)?
                    .into_bytes();
                if existing.as_ref() != bytes.as_slice() {
                    return Err(CheckpointError::Corrupt);
                }
            }
            self.commit_checkpoint_pointer(guard, authority, &metadata)
                .await?;
            Ok(metadata)
        })
    }
}

impl BrainStore {
    async fn commit_checkpoint_pointer(
        &self,
        guard: &FenceGuard,
        authority: &SessionAuthority,
        metadata: &CheckpointMetadata,
    ) -> Result<(), CheckpointError> {
        let (session, agent) =
            translate::agent_key(&guard.key()).map_err(|_| CheckpointError::Conflict)?;
        let control_key = aex_session_dynamodb::keys::agent_control(session, agent);
        let tail = guard.tail().ok_or(CheckpointError::Conflict)?;
        if tail != metadata.covers_through {
            return Err(CheckpointError::Conflict);
        }
        let previous_condition = if metadata.previous.is_some() {
            "checkpointId = :previous"
        } else {
            "attribute_not_exists(checkpointId)"
        };
        let checkpoint_previous_assignment = metadata
            .previous
            .map(|_| ", checkpointPreviousId = :previousValue")
            .unwrap_or_default();
        let checkpoint_previous_removal = metadata
            .previous
            .is_none()
            .then_some(" REMOVE checkpointPreviousId")
            .unwrap_or_default();
        let mut update = Update::builder()
            .table_name(self.table())
            .set_key(Some(key(&control_key.pk, &control_key.sk)))
            .condition_expression(format!(
                "revision = :revision AND fence = :fence AND claimOwner = :owner \
                 AND cancelEpoch = :cancelEpoch AND journalTail = :tail \
                 AND journalTailHash = :coversHash AND {previous_condition}"
            ))
            .update_expression(format!(
                "SET checkpointSchemaVersion = :schema, checkpointId = :id{checkpoint_previous_assignment}, \
                 checkpointCoversThrough = :tail, checkpointCoversHash = :coversHash, \
                 checkpointObjectHash = :objectHash, checkpointObjectBytes = :objectBytes, \
                 checkpointSourceHash = :sourceHash, checkpointApproximateTokens = :tokens, \
                 checkpointCompactor = :compactor, checkpointCreatedAt = :createdAt{checkpoint_previous_removal}"
            ))
            .expression_attribute_values(":revision", n(guard.revision().0))
            .expression_attribute_values(":fence", n(guard.fence().0))
            .expression_attribute_values(
                ":owner",
                s(guard.as_ref().owner.0.as_hyphenated().to_string()),
            )
            .expression_attribute_values(":cancelEpoch", n(guard.cancel_epoch().0))
            .expression_attribute_values(":tail", n(tail.get()))
            .expression_attribute_values(":coversHash", s(metadata.covers_hash.to_hex()))
            .expression_attribute_values(":schema", n(u64::from(metadata.schema_version)))
            .expression_attribute_values(":id", s(metadata.id.to_hex()))
            .expression_attribute_values(":objectHash", s(metadata.object_hash.to_hex()))
            .expression_attribute_values(":objectBytes", n(metadata.object_bytes))
            .expression_attribute_values(":sourceHash", s(metadata.source_hash.to_hex()))
            .expression_attribute_values(":tokens", n(metadata.approximate_tokens))
            .expression_attribute_values(":compactor", s(metadata.compactor.clone()))
            .expression_attribute_values(
                ":createdAt",
                stamp(
                    translate::at(metadata.created_at, "checkpoint created at")
                        .map_err(|_| CheckpointError::Conflict)?,
                ),
            );
        if let Some(previous) = metadata.previous {
            update = update
                .expression_attribute_values(":previous", s(previous.to_hex()))
                .expression_attribute_values(":previousValue", s(previous.to_hex()));
        }
        let head_guard =
            plan::session_head_guard(self.table(), session, guard.cancel_epoch(), authority)
                .build()
                .map_err(|_| CheckpointError::Unavailable)?;
        let update = update.build().map_err(|_| CheckpointError::Unavailable)?;
        let result = self
            .client()
            .transact_write_items()
            .transact_items(
                TransactWriteItem::builder()
                    .condition_check(head_guard)
                    .build(),
            )
            .transact_items(TransactWriteItem::builder().update(update).build())
            .send()
            .await;
        if result.is_ok() {
            return Ok(());
        }
        // Resolve an ambiguous/conditional response by strongly reading the
        // pointer. Never issue another S3 write or advance from an unknown head.
        let Some(item) = self
            .get_control(&guard.key())
            .await
            .map_err(|_| CheckpointError::Unavailable)?
        else {
            return Err(CheckpointError::Conflict);
        };
        let current = control::decode(&item, guard.key(), Vec::new())
            .map_err(|_| CheckpointError::Corrupt)?;
        if current.checkpoint.as_ref() == Some(metadata) {
            Ok(())
        } else {
            Err(CheckpointError::Conflict)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::object_key;
    use aex_brain_domain::ids::{AgentId, AgentKey, ContentHash, SessionId};
    use uuid::Uuid;

    #[test]
    fn object_keys_are_session_and_agent_isolated_and_never_overwrite() {
        let a = AgentKey::new(SessionId(Uuid::from_u128(1)), AgentId(Uuid::from_u128(2)));
        let b = AgentKey::new(SessionId(Uuid::from_u128(1)), AgentId(Uuid::from_u128(3)));
        let checkpoint = ContentHash::of(b"checkpoint");
        let body = ContentHash::of(b"body");
        let first = object_key(a, checkpoint, body);
        assert!(first.starts_with("session-content/v1/session="));
        assert_ne!(first, object_key(b, checkpoint, body));
        assert_ne!(first, object_key(a, checkpoint, ContentHash::of(b"other")));
        assert!(first.ends_with(".json"));
    }
}
