//! The S3 object adapter above `regional-content`.
//!
//! Every create is conditional (`If-None-Match: *`) and every delete is fenced
//! (`If-Match: {etag}`). Both are also denied unconditionally by the bucket
//! policy (see [`crate::policy`]), so an overwrite or a blind delete fails
//! closed at the service rather than only in this code — which is the difference
//! between a rule and a habit.

use aex_session_dynamodb::error::Resolution;
use aex_wire::ids::{ContentHash, WorkspaceId};
use async_trait::async_trait;
use aws_sdk_s3::Client;
use aws_sdk_s3::presigning::PresigningConfig;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{ChecksumAlgorithm, ServerSideEncryption};

use crate::errors::{
    ContentObjectError, ObjectIdempotence, PRECONDITION_FAILED, classify, code_of,
};
use crate::multipart::{CompletionManifest, MultipartHandle, PresignedPart, ProviderPart};
use crate::object_key::{
    self, METADATA_DIGEST, METADATA_PLAINTEXT_BYTES, METADATA_WORKSPACE, ObjectKey, PRESIGN_EXPIRY,
};
use crate::redacted::RedactedUrl;

/// Where the adapter writes, and under which key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketBinding {
    /// The content bucket.
    pub bucket: String,
    /// The account that must own it, so a bucket moved between accounts fails
    /// closed instead of silently succeeding.
    pub expected_owner: String,
    /// The content CMK.
    pub kms_key_id: String,
}

/// One immutable create.
#[derive(Debug, Clone)]
pub struct PutImmutable<'a> {
    /// The owning workspace.
    pub workspace: WorkspaceId,
    /// The customer-visible body digest.
    pub digest: &'a ContentHash,
    /// The declared plaintext size.
    pub plaintext_bytes: u64,
    /// The bytes to store.
    pub body: Vec<u8>,
    /// The canonical encryption context, which S3 binds to the SSE-KMS
    /// operation and `CloudTrail` records.
    pub encryption_context: &'a [u8],
}

/// What a committed object looks like afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectCommit {
    /// The key it landed on.
    pub key: ObjectKey,
    /// The `ETag` a fenced delete will condition on.
    pub etag: String,
    /// The SHA-256 checksum S3 stored: `COMPOSITE` for a multipart object.
    pub checksum_sha256: Option<String>,
    /// The full-object CRC64NVME checksum.
    pub checksum_crc64_nvme: Option<String>,
    /// How many parts the object has, when it was multipart.
    pub part_count: Option<u64>,
}

/// What a `HeadObject` established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectHead {
    /// The stored length.
    pub content_length: u64,
    /// The `ETag`.
    pub etag: String,
    /// The digest metadata the create wrote.
    pub declared_digest: Option<String>,
}

/// A presigned read.
#[derive(Debug, Clone)]
pub struct PresignedGet {
    /// The URL, which cannot print itself.
    pub url: RedactedUrl,
    /// Headers the caller must replay, including the signed `Range`.
    pub headers: Vec<(String, String)>,
    /// How long the URL lives.
    pub expires_in_seconds: u64,
}

/// One fenced delete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FencedDelete<'a> {
    /// The key the sweep decided on.
    pub key: &'a ObjectKey,
    /// The `ETag` recorded on the descriptor when it was marked.
    pub etag: &'a str,
}

/// What a fenced delete established.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FencedDeleteOutcome {
    /// The exact object the sweep marked is gone.
    Deleted,
    /// It was already gone, which is an idempotent success.
    AlreadyAbsent,
    /// The object changed since it was marked, so the body survives.
    Changed,
}

// TODO(cross-stream): replaced by aex_content_domain::ports::ContentObjectStore
/// The object side of content storage.
#[async_trait]
pub trait ContentObjectStore: Send + Sync + 'static {
    /// Creates an object, conditionally and exactly once.
    ///
    /// # Errors
    ///
    /// [`ContentObjectError::DigestCollision`] when the key already holds a
    /// different body, which under SHA-256 can only be a bug or a substitution.
    async fn put_immutable(
        &self,
        request: PutImmutable<'_>,
    ) -> Result<ObjectCommit, ContentObjectError>;

    /// Reads an object's metadata.
    ///
    /// # Errors
    ///
    /// [`ContentObjectError::ContentMissing`] when it is absent.
    async fn head(&self, key: &ObjectKey) -> Result<ObjectHead, ContentObjectError>;

    /// Presigns a bounded read.
    ///
    /// # Errors
    ///
    /// [`ContentObjectError::InvalidRange`] above the single-read ceiling.
    async fn presign_get(
        &self,
        key: &ObjectKey,
        range: Option<(u64, u64)>,
    ) -> Result<PresignedGet, ContentObjectError>;

    /// Deletes exactly the object a sweep marked.
    ///
    /// # Errors
    ///
    /// Never for a changed or absent object: both are outcomes, because "the
    /// object changed" must keep the body rather than fail the worker.
    async fn delete_fenced(
        &self,
        request: FencedDelete<'_>,
    ) -> Result<FencedDeleteOutcome, ContentObjectError>;

    /// Opens a multipart upload.
    ///
    /// # Errors
    ///
    /// [`ContentObjectError`] for any transport or service failure.
    async fn begin_multipart(
        &self,
        workspace: WorkspaceId,
        digest: &ContentHash,
        encryption_context: &[u8],
    ) -> Result<MultipartHandle, ContentObjectError>;

    /// Presigns one part upload.
    ///
    /// # Errors
    ///
    /// As [`ContentObjectStore::begin_multipart`].
    async fn presign_part(
        &self,
        handle: &MultipartHandle,
        part_number: i32,
        part_sha256_base64: &str,
        content_length: u64,
    ) -> Result<PresignedPart, ContentObjectError>;

    /// Lists every uploaded part, to exhaustion.
    ///
    /// # Errors
    ///
    /// [`ContentObjectError::IntegrityMismatch`] when the provider truncates a
    /// page without a marker, because a partial list would silently complete an
    /// object with missing parts.
    async fn list_parts(
        &self,
        handle: &MultipartHandle,
    ) -> Result<Vec<ProviderPart>, ContentObjectError>;

    /// Completes a multipart upload against a manifest.
    ///
    /// # Errors
    ///
    /// [`ContentObjectError::IntegrityMismatch`] when the provider's parts
    /// disagree with the manifest, and
    /// [`ContentObjectError::CommitAmbiguous`] — resolved by `HeadObject`, never
    /// by re-issuing — when the completion times out.
    async fn complete_multipart(
        &self,
        handle: &MultipartHandle,
        manifest: &CompletionManifest,
    ) -> Result<ObjectCommit, ContentObjectError>;

    /// Aborts a multipart upload.
    ///
    /// Only ever called for an upload whose transaction outcome is **known**. An
    /// ambiguous completion is left to the 24-hour expiry sweeper, because
    /// aborting an upload that may already have completed destroys a committed
    /// body.
    ///
    /// # Errors
    ///
    /// As [`ContentObjectStore::begin_multipart`]; an already-aborted upload is
    /// an idempotent success.
    async fn abort_multipart(&self, handle: &MultipartHandle) -> Result<(), ContentObjectError>;
}

/// The adapter.
#[derive(Debug, Clone)]
pub struct S3ContentObjects {
    client: Client,
    binding: BucketBinding,
}

impl S3ContentObjects {
    /// Binds an adapter to a client and a bucket.
    #[must_use]
    pub const fn new(client: Client, binding: BucketBinding) -> Self {
        Self { client, binding }
    }

    /// The bucket binding.
    #[must_use]
    pub const fn binding(&self) -> &BucketBinding {
        &self.binding
    }

    fn presigning() -> Result<PresigningConfig, ContentObjectError> {
        PresigningConfig::expires_in(PRESIGN_EXPIRY).map_err(|error| ContentObjectError::Invalid {
            detail: error.to_string(),
        })
    }
}

#[async_trait]
impl ContentObjectStore for S3ContentObjects {
    async fn put_immutable(
        &self,
        request: PutImmutable<'_>,
    ) -> Result<ObjectCommit, ContentObjectError> {
        let key = ObjectKey::new(request.workspace, request.digest);
        let digest_hex = hex::encode(request.digest.as_bytes());
        let length =
            i64::try_from(request.body.len()).map_err(|_| ContentObjectError::Invalid {
                detail: "a body longer than i64::MAX cannot be described".to_owned(),
            })?;
        let outcome = self
            .client
            .put_object()
            .bucket(&self.binding.bucket)
            .key(key.as_str())
            .if_none_match("*")
            .checksum_algorithm(ChecksumAlgorithm::Sha256)
            .checksum_sha256(object_key::checksum_base64(request.digest))
            .content_length(length)
            .server_side_encryption(ServerSideEncryption::AwsKms)
            .ssekms_key_id(&self.binding.kms_key_id)
            .ssekms_encryption_context(object_key::encryption_context_base64(
                request.encryption_context,
            ))
            .bucket_key_enabled(true)
            .expected_bucket_owner(&self.binding.expected_owner)
            .metadata(METADATA_WORKSPACE, request.workspace.to_string())
            .metadata(METADATA_DIGEST, &digest_hex)
            .metadata(
                METADATA_PLAINTEXT_BYTES,
                request.plaintext_bytes.to_string(),
            )
            .body(ByteStream::from(request.body.clone()))
            .send()
            .await;

        match outcome {
            Ok(response) => Ok(ObjectCommit {
                key,
                etag: response.e_tag.unwrap_or_default(),
                checksum_sha256: response.checksum_sha256,
                checksum_crc64_nvme: response.checksum_crc64_nvme,
                part_count: None,
            }),
            Err(error) => {
                if code_of(&error).as_deref() != Some(PRECONDITION_FAILED) {
                    return Err(classify(
                        &error,
                        ObjectIdempotence::Write(Resolution::ObjectHead),
                    ));
                }
                // The key exists. Under a content-addressed key that is either
                // the identical body — an idempotent success — or evidence of a
                // key-derivation bug or a substituted object, which is hard.
                let head = self.head(&key).await?;
                let stored = u64::try_from(request.body.len()).unwrap_or(u64::MAX);
                let same = head.content_length == stored
                    && head.declared_digest.as_deref() == Some(digest_hex.as_str());
                if same {
                    Ok(ObjectCommit {
                        key,
                        etag: head.etag,
                        checksum_sha256: None,
                        checksum_crc64_nvme: None,
                        part_count: None,
                    })
                } else {
                    Err(ContentObjectError::DigestCollision {
                        key: key.as_str().to_owned(),
                    })
                }
            }
        }
    }

    async fn head(&self, key: &ObjectKey) -> Result<ObjectHead, ContentObjectError> {
        let response = self
            .client
            .head_object()
            .bucket(&self.binding.bucket)
            .key(key.as_str())
            .expected_bucket_owner(&self.binding.expected_owner)
            .send()
            .await
            .map_err(|error| classify(&error, ObjectIdempotence::Read))?;
        Ok(ObjectHead {
            content_length: u64::try_from(response.content_length.unwrap_or_default())
                .unwrap_or_default(),
            etag: response.e_tag.unwrap_or_default(),
            declared_digest: response
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get(METADATA_DIGEST).cloned()),
        })
    }

    async fn presign_get(
        &self,
        key: &ObjectKey,
        range: Option<(u64, u64)>,
    ) -> Result<PresignedGet, ContentObjectError> {
        let mut request = self
            .client
            .get_object()
            .bucket(&self.binding.bucket)
            .key(key.as_str())
            .expected_bucket_owner(&self.binding.expected_owner);
        if let Some((start, end_inclusive)) = range {
            let requested = end_inclusive.saturating_sub(start).saturating_add(1);
            if requested > object_key::MAX_RANGE_BYTES {
                return Err(ContentObjectError::InvalidRange {
                    requested,
                    ceiling: object_key::MAX_RANGE_BYTES,
                });
            }
            request = request.range(format!("bytes={start}-{end_inclusive}"));
        }
        let presigned = request
            .presigned(Self::presigning()?)
            .await
            .map_err(|error| classify(&error, ObjectIdempotence::Read))?;
        Ok(PresignedGet {
            url: RedactedUrl::new(presigned.uri()),
            headers: presigned
                .headers()
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect(),
            expires_in_seconds: PRESIGN_EXPIRY.as_secs(),
        })
    }

    async fn delete_fenced(
        &self,
        request: FencedDelete<'_>,
    ) -> Result<FencedDeleteOutcome, ContentObjectError> {
        let outcome = self
            .client
            .delete_object()
            .bucket(&self.binding.bucket)
            .key(request.key.as_str())
            .if_match(request.etag)
            .expected_bucket_owner(&self.binding.expected_owner)
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(FencedDeleteOutcome::Deleted),
            Err(error) => match code_of(&error).as_deref() {
                // The object changed since it was marked: the candidate is
                // dropped and the body survives.
                Some(PRECONDITION_FAILED) => Ok(FencedDeleteOutcome::Changed),
                Some("NoSuchKey" | "NotFound") => Ok(FencedDeleteOutcome::AlreadyAbsent),
                _ => Err(classify(
                    &error,
                    ObjectIdempotence::Write(Resolution::ObjectHead),
                )),
            },
        }
    }

    async fn begin_multipart(
        &self,
        workspace: WorkspaceId,
        digest: &ContentHash,
        encryption_context: &[u8],
    ) -> Result<MultipartHandle, ContentObjectError> {
        let key = ObjectKey::new(workspace, digest);
        let response = self
            .client
            .create_multipart_upload()
            .bucket(&self.binding.bucket)
            .key(key.as_str())
            .checksum_algorithm(ChecksumAlgorithm::Sha256)
            .server_side_encryption(ServerSideEncryption::AwsKms)
            .ssekms_key_id(&self.binding.kms_key_id)
            .ssekms_encryption_context(object_key::encryption_context_base64(encryption_context))
            .bucket_key_enabled(true)
            .expected_bucket_owner(&self.binding.expected_owner)
            .metadata(METADATA_WORKSPACE, workspace.to_string())
            .metadata(METADATA_DIGEST, hex::encode(digest.as_bytes()))
            .send()
            .await
            .map_err(|error| classify(&error, ObjectIdempotence::Write(Resolution::ObjectHead)))?;
        Ok(MultipartHandle {
            key,
            upload_id: response.upload_id.unwrap_or_default(),
        })
    }

    async fn presign_part(
        &self,
        handle: &MultipartHandle,
        part_number: i32,
        part_sha256_base64: &str,
        content_length: u64,
    ) -> Result<PresignedPart, ContentObjectError> {
        let length = i64::try_from(content_length).map_err(|_| ContentObjectError::Invalid {
            detail: "a part longer than i64::MAX cannot be described".to_owned(),
        })?;
        let presigned = self
            .client
            .upload_part()
            .bucket(&self.binding.bucket)
            .key(handle.key.as_str())
            .upload_id(&handle.upload_id)
            .part_number(part_number)
            .checksum_sha256(part_sha256_base64)
            .content_length(length)
            .expected_bucket_owner(&self.binding.expected_owner)
            .presigned(Self::presigning()?)
            .await
            .map_err(|error| classify(&error, ObjectIdempotence::Read))?;
        Ok(PresignedPart {
            part_number,
            url: RedactedUrl::new(presigned.uri()),
            headers: presigned
                .headers()
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect(),
            expires_in_seconds: PRESIGN_EXPIRY.as_secs(),
        })
    }

    async fn list_parts(
        &self,
        handle: &MultipartHandle,
    ) -> Result<Vec<ProviderPart>, ContentObjectError> {
        let mut parts = Vec::new();
        let mut marker: Option<String> = None;
        loop {
            let response = self
                .client
                .list_parts()
                .bucket(&self.binding.bucket)
                .key(handle.key.as_str())
                .upload_id(&handle.upload_id)
                .expected_bucket_owner(&self.binding.expected_owner)
                .set_part_number_marker(marker.clone())
                .send()
                .await
                .map_err(|error| classify(&error, ObjectIdempotence::Read))?;
            for part in response.parts.unwrap_or_default() {
                parts.push(ProviderPart {
                    part_number: part.part_number.unwrap_or_default(),
                    size: u64::try_from(part.size.unwrap_or_default()).unwrap_or_default(),
                    etag: part.e_tag.unwrap_or_default(),
                    checksum_sha256: part.checksum_sha256,
                });
            }
            if response.is_truncated.unwrap_or(false) {
                // A truncated page with no marker cannot be continued, and
                // completing from a partial list would silently drop parts.
                let next = response.next_part_number_marker.clone().ok_or_else(|| {
                    ContentObjectError::IntegrityMismatch {
                        detail: "the part listing was truncated without a continuation marker"
                            .to_owned(),
                    }
                })?;
                marker = Some(next);
            } else {
                break;
            }
        }
        parts.sort_by_key(|part| part.part_number);
        Ok(parts)
    }

    async fn complete_multipart(
        &self,
        handle: &MultipartHandle,
        manifest: &CompletionManifest,
    ) -> Result<ObjectCommit, ContentObjectError> {
        let provider = self.list_parts(handle).await?;
        manifest.agrees_with(&provider)?;

        let outcome = self
            .client
            .complete_multipart_upload()
            .bucket(&self.binding.bucket)
            .key(handle.key.as_str())
            .upload_id(&handle.upload_id)
            .multipart_upload(manifest.to_completed_upload())
            .mpu_object_size(i64::try_from(manifest.total_bytes).map_err(|_| {
                ContentObjectError::Invalid {
                    detail: "an object larger than i64::MAX cannot be described".to_owned(),
                }
            })?)
            .if_none_match("*")
            .expected_bucket_owner(&self.binding.expected_owner)
            .send()
            .await;

        match outcome {
            Ok(response) => Ok(ObjectCommit {
                key: handle.key.clone(),
                etag: response.e_tag.unwrap_or_default(),
                checksum_sha256: response.checksum_sha256,
                checksum_crc64_nvme: response.checksum_crc64_nvme,
                part_count: Some(u64::try_from(manifest.parts.len()).unwrap_or_default()),
            }),
            Err(error) => Err(classify(
                &error,
                // Never re-issue a completion: resolve it by reading the final
                // key instead.
                ObjectIdempotence::Write(Resolution::ObjectHead),
            )),
        }
    }

    async fn abort_multipart(&self, handle: &MultipartHandle) -> Result<(), ContentObjectError> {
        let outcome = self
            .client
            .abort_multipart_upload()
            .bucket(&self.binding.bucket)
            .key(handle.key.as_str())
            .upload_id(&handle.upload_id)
            .expected_bucket_owner(&self.binding.expected_owner)
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(()),
            Err(error) => match code_of(&error).as_deref() {
                Some("NoSuchUpload") => Ok(()),
                _ => Err(classify(
                    &error,
                    ObjectIdempotence::Write(Resolution::ObjectHead),
                )),
            },
        }
    }
}
