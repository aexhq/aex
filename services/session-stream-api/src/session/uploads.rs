//! The staged-upload authority: the four `uploads` routes.
//!
//! This is the composition the cluster was missing. Every primitive underneath it
//! was already written and tested — the presigner, the durable multipart handle,
//! the conditional expressions, the shared replay combinator, the content
//! collector — and none of it had a caller.
//!
//! Four rules run through every handler here.
//!
//! **S3 first, `DynamoDB` second** (E D-2, D-6). The irreversible provider effect
//! is issued before the durable write that records it, and that write is
//! conditional on the exact state *and the exact handle* the decision was made
//! under. The reverse order produces a row that lies about the provider and a
//! multipart upload nothing but the bucket lifecycle rule can reclaim.
//!
//! **`HeadObject` is the sole oracle** (E D-3). Whenever a completion's outcome
//! is unknown, the answer comes from heading the durable object key — never from
//! re-issuing the completion, never from `ListParts`, and never from a timeout
//! heuristic. Guessing wrong here deletes customer data.
//!
//! **Nothing here deletes an object** (E D-4). There is no delete verb on the
//! path and the deployable's role holds none. The worst outcome of any bug in
//! this file is an orphan object the collector reclaims after its grace.
//!
//! **A replay never re-opens a multipart upload** (E D-19). The receipt is read
//! before any provider call and is written in the same transaction as the row it
//! describes.

use std::sync::Arc;

use aex_content_aws::multipart::{
    CompletedPartPlan, CompletionManifest, MultipartHandle, PartPlan as AwsPartPlan,
};
use aex_content_aws::object_key::{ObjectKey, PRESIGN_EXPIRY, checksum_base64};
use aex_content_aws::object_store::ContentObjectStore;
use aex_content_dynamodb::codec::{ContentDescriptor, ObjectLocation};
use aex_content_dynamodb::store::ContentMetadataStore;
use aex_registry_dynamodb::store::RegistryStore;
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::plan::{Participant, TransactionPlan};
use aex_session_dynamodb::replay::{
    DecodeReceipt, IdempotencyScope, RECEIPT_RETENTION, Receipt, ReceiptBody, ReceiptStore,
    ReplayRequest, commit_or_replay, encode_receipt_row, key_digest,
};
use aex_wire::error::{ErrorCode, WireError, WireResult};
use aex_wire::idempotency::IntentDigest;
use aex_wire::ids::{ContentHash, PrefixedId as _, UploadId, WorkspaceId};
use aex_wire::models;
use aex_wire::types::{DecimalU128, HttpsUrl, Timestamp};
use aex_workspace_domain::upload::{
    self, AmbiguityResolution, CompletionEvidence, HeadOracle, PART_GRANT_MAX_PER_CALL, PartPlan,
    PartReceipt, UPLOAD_GRACE, Upload, UploadError, UploadState,
};

use crate::session::handlers::{Routes, Shared};

/// The content-object domain an upload's encryption context names.
const ENCRYPTION_DOMAIN: &str = "content-object";

/// A fresh time-ordered identity.
///
/// Deliberately random rather than derived from the replay key: a derived id
/// would leak the key into a public identifier, and a derived-from-body id would
/// make two legitimate uploads of identical bytes collide.
fn new_uuid7() -> aex_wire::ids::Uuid7 {
    aex_wire::ids::Uuid7::from_bytes(*uuid::Uuid::now_v7().as_bytes())
        .unwrap_or_else(|_| unreachable!("`now_v7` always produces a v7 payload"))
}

/// The canonical encryption context a content object is sealed under.
///
/// Canonical JSON with sorted keys, because S3 records it verbatim in
/// `CloudTrail` and a KMS key policy condition matches it literally.
fn encryption_context(workspace: WorkspaceId) -> Vec<u8> {
    format!(r#"{{"aex:domain":"{ENCRYPTION_DOMAIN}","aex:workspace":"{workspace}"}}"#).into_bytes()
}

/// A `Backoff` that never sleeps.
///
/// The combinator's retry ladder exists for provider contention on a durable
/// transaction. On a request path the honest wait is zero: the route is
/// synchronous, the caller is holding a connection, and a bounded three-attempt
/// ladder with no sleep still removes the double-commit window, which is the
/// property that actually matters.
struct Immediate;

#[async_trait::async_trait]
impl aex_session_dynamodb::replay::Backoff for Immediate {
    async fn wait(&self, _policy: aex_session_dynamodb::error::RetryPolicy, _attempt: u32) {}
}

/// The canonical `Upload` response, rebuildable from a receipt.
///
/// A receipt stores the canonical response body so a replay is byte-identical to
/// the answer the winning request gave.
#[derive(Debug, Clone, PartialEq)]
pub struct UploadResponse(pub models::Upload);

impl DecodeReceipt for UploadResponse {
    fn decode_receipt(receipt: &Receipt) -> Result<Self, StoreError> {
        let ReceiptBody::Inline(bytes) = &receipt.response else {
            return Err(StoreError::Invalid {
                detail: "an upload receipt stores its response inline".to_owned(),
            });
        };
        serde_json::from_slice(bytes)
            .map(Self)
            .map_err(|error| StoreError::Invalid {
                detail: format!("the stored upload response is not an `Upload`: {error}"),
            })
    }
}

/// Everything the upload routes reach.
///
/// Written as a view over [`Shared`] rather than a second adapter set, so the
/// deployable still has exactly one statement of what it may touch.
struct Ports<'a> {
    registry: &'a Arc<dyn RegistryStore>,
    content: &'a Arc<dyn ContentMetadataStore>,
    objects: &'a Arc<dyn ContentObjectStore>,
    receipts: &'a Arc<dyn ReceiptStore>,
    registry_table: &'a str,
    work_table: &'a str,
}

impl Routes {
    fn upload_ports(&self) -> Ports<'_> {
        let shared: &Shared = self.shared();
        Ports {
            registry: &shared.registry,
            content: &shared.content,
            objects: &shared.content_objects,
            receipts: &shared.receipts,
            registry_table: &shared.registry_table,
            work_table: &shared.work_table,
        }
    }

    fn workspace(&self) -> WorkspaceId {
        self.context().auth.workspace_id
    }

    fn upload_now(&self) -> WireResult<Timestamp> {
        self.context()
            .now()
            .map_err(|_| WireError::new(ErrorCode::InternalError).with_message("clock"))
    }
}

// ---------------------------------------------------------------------------
// The wire projection
// ---------------------------------------------------------------------------

/// Projects a stored upload onto the two-value wire enum (E D-5).
///
/// Every terminal state is absent from the wire, because every route answers
/// `gone` for one: an upload the client can no longer act on has no lifecycle
/// position to report.
fn wire_state(state: UploadState) -> Option<models::UploadState> {
    if state.is_staging() {
        return Some(models::UploadState::Staging);
    }
    if state == UploadState::Ready {
        return Some(models::UploadState::Ready);
    }
    None
}

fn wire_upload(upload: &Upload) -> WireResult<models::Upload> {
    let state = wire_state(upload.state).ok_or_else(|| WireError::new(ErrorCode::Gone))?;
    let part_size = upload
        .parts
        .parts
        .first()
        .map_or(upload.declared_size, |part| part.bytes);
    Ok(models::Upload {
        id: upload.id,
        state,
        size_bytes: DecimalU128::new(u128::from(upload.declared_size)),
        sha256: upload.declared_sha256,
        content_type: upload
            .content_type
            .clone()
            .unwrap_or_else(|| "application/octet-stream".to_owned()),
        part_size_bytes: DecimalU128::new(u128::from(part_size)),
        part_count: u32::try_from(upload.parts.parts.len()).unwrap_or(u32::MAX),
        created_at: upload.created_at,
        expires_at: upload.expires_at,
    })
}

/// Maps a domain refusal onto a code the route declares.
///
/// A handler never invents a code: everything here is in the declared error set
/// of every route that can produce it, and the boundary refuses the rest.
fn refuse(error: &UploadError) -> WireError {
    match error {
        UploadError::WrongState { from } if from.is_terminal() => WireError::new(ErrorCode::Gone),
        UploadError::AlreadyConsumed(_) => WireError::new(ErrorCode::Gone),
        other => WireError::new(ErrorCode::InvalidRequest).with_message(other.to_string()),
    }
}

fn store_failure(error: &StoreError) -> WireError {
    match error {
        StoreError::IdempotencyConflict => WireError::new(ErrorCode::IdempotencyConflict),
        StoreError::PreconditionFailed { .. } => {
            WireError::new(ErrorCode::InvalidRequest).with_message("the upload moved")
        }
        other => WireError::new(ErrorCode::InternalError).with_message(other.to_string()),
    }
}

// ---------------------------------------------------------------------------
// The handlers
// ---------------------------------------------------------------------------

impl Routes {
    /// Loads an upload for this workspace, or answers the route's absence code.
    async fn load_upload(&self, id: UploadId) -> WireResult<Upload> {
        self.upload_ports()
            .registry
            .load_upload(self.workspace(), id)
            .await
            .map_err(|error| store_failure(&error))?
            .ok_or_else(|| WireError::new(ErrorCode::NotFound))
    }

    /// The D-3 oracle, taken against the durable object key.
    async fn head_oracle(&self, upload: &Upload) -> HeadOracle {
        let Ok(key) = ObjectKey::parse(&upload.object_key) else {
            // A key that no longer parses is corruption, not evidence.
            return HeadOracle::Unavailable;
        };
        match self.upload_ports().objects.head(&key).await {
            Ok(head) => HeadOracle::Present {
                content_length: head.content_length,
                declared_digest: head
                    .declared_digest
                    .as_deref()
                    .and_then(|hex| ContentHash::parse(&format!("sha256:{hex}")).ok()),
                evidence: CompletionEvidence {
                    etag: head.etag,
                    checksum_sha256: None,
                    checksum_crc64_nvme: None,
                    part_count: None,
                },
            },
            Err(aex_content_aws::ContentObjectError::ContentMissing { .. }) => HeadOracle::Absent,
            Err(_) => HeadOracle::Unavailable,
        }
    }

    /// Writes the committed descriptor, then settles the row (E D-18).
    ///
    /// The order is fixed even though the write is not transactional, because
    /// only this order has a safe failure mode: a descriptor without its row is
    /// an unreferenced body the collector reclaims, while a `ready` row without
    /// its descriptor is a registry PUT that answers `content_missing` with no
    /// way to repair it.
    async fn settle_ready(&self, settled: &Upload) -> WireResult<()> {
        let ports = self.upload_ports();
        let evidence = settled
            .completion
            .as_ref()
            .ok_or_else(|| WireError::new(ErrorCode::InternalError).with_message("no evidence"))?;
        let now = self.upload_now()?;
        ports
            .content
            .put_descriptor(&ContentDescriptor {
                workspace: settled.workspace,
                organization: self.context().auth.organization_id,
                digest: settled.declared_sha256,
                size_bytes: settled.declared_size,
                media_type: settled
                    .content_type
                    .clone()
                    .unwrap_or_else(|| "application/octet-stream".to_owned()),
                placement: aex_session_dynamodb::measure::Placement::ObjectStore,
                state: "committed".to_owned(),
                gc_epoch: 0,
                created_at: now,
                verified_at: Some(now),
                object: Some(ObjectLocation {
                    key: settled.object_key.clone(),
                    etag: evidence.etag.clone(),
                    checksum_sha256: evidence.checksum_sha256.clone().unwrap_or_default(),
                    checksum_crc64_nvme: evidence.checksum_crc64_nvme.clone().unwrap_or_default(),
                    part_count: evidence.part_count,
                    kms_key_id: self.shared().content_kms_key_id.clone(),
                }),
            })
            .await
            .map_err(|error| store_failure(&error))?;
        ports
            .registry
            .settle_ready(settled)
            .await
            .map_err(|error| store_failure(&error))
    }

    /// `POST /api/workspace/uploads`
    async fn stage_upload(&self, body: models::UploadCreateRequest) -> WireResult<models::Upload> {
        let workspace = self.workspace();
        let now = self.upload_now()?;
        let size = u64::try_from(body.size_bytes.get()).map_err(|_| {
            WireError::new(ErrorCode::InvalidRequest).with_message("size is out of range")
        })?;
        let plan = upload::plan_parts(size).map_err(|error| refuse(&error))?;
        let ports = self.upload_ports();

        let identity = self
            .context()
            .idempotency
            .as_ref()
            .ok_or_else(|| WireError::new(ErrorCode::InvalidRequest).with_message("no replay key"))?
            .clone();
        let scope = IdempotencyScope::new("registry.upload", Some("create")).map_err(|error| {
            WireError::new(ErrorCode::InternalError).with_message(error.to_string())
        })?;
        let intent = IntentDigest::from_bytes(identity.intent);

        let outcome = commit_or_replay::<UploadResponse, _, _>(
            ports.receipts.as_ref(),
            &Immediate,
            ReplayRequest {
                workspace,
                scope,
                key: &identity.key,
                intent,
                receipt_participant: Participant::SESSION_IDEMPOTENCY,
                policy: aex_session_dynamodb::error::RetryPolicy::PINNED,
                now,
            },
            || {
                let plan = plan.clone();
                let body = body.clone();
                let key = identity.key.clone();
                let scope_rendered = scope.render();
                async move {
                    self.admit_upload(
                        workspace,
                        size,
                        &body,
                        plan,
                        &scope_rendered,
                        &key,
                        intent,
                        now,
                    )
                    .await
                }
            },
        )
        .await
        .map_err(|error| store_failure(&error))?;

        Ok(outcome.into_inner().0)
    }

    /// The admission itself: S3 first, then one transaction (E D-2, D-19).
    #[allow(clippy::too_many_arguments, reason = "one argument per durable fact")]
    async fn admit_upload(
        &self,
        workspace: WorkspaceId,
        size: u64,
        body: &models::UploadCreateRequest,
        plan: PartPlan,
        scope: &str,
        key: &aex_wire::idempotency::IdempotencyKey,
        intent: IntentDigest,
        now: Timestamp,
    ) -> Result<UploadResponse, StoreError> {
        let ports = self.upload_ports();

        // S3 first. A crash between here and the transaction leaks an orphan
        // multipart upload with no row naming it, which the bucket's
        // `AbortIncompleteMultipartUpload` rule reclaims. Row-first would instead
        // make "a durable row that names no handle" a steady state.
        let handle = ports
            .objects
            .begin_multipart(workspace, &body.sha256, &encryption_context(workspace))
            .await
            .map_err(|error| StoreError::Invalid {
                detail: error.to_string(),
            })?;

        let expires_at = Timestamp::from_unix_millis(
            now.unix_millis()
                .saturating_add(i64::try_from(UPLOAD_GRACE.whole_milliseconds()).unwrap_or(0)),
        )
        .map_err(|error| StoreError::Invalid {
            detail: error.to_string(),
        })?;

        let staged = Upload {
            id: UploadId::from_uuid7(new_uuid7()),
            workspace,
            state: UploadState::Created,
            provider_upload_id: handle.upload_id.clone(),
            object_key: handle.key.as_str().to_owned(),
            declared_size: size,
            declared_sha256: body.sha256,
            content_type: Some(body.content_type.clone()),
            parts: plan,
            completion_manifest: Vec::new(),
            completion: None,
            consumed_by: None,
            created_at: now,
            expires_at,
        };

        let rendered = wire_upload(&staged).map_err(|_| StoreError::Invalid {
            detail: "a freshly staged upload always has a wire state".to_owned(),
        })?;
        let canonical = serde_json::to_vec(&rendered).map_err(|error| StoreError::Invalid {
            detail: error.to_string(),
        })?;

        // Part blocks are written before the transaction, so the head row is
        // never visible without the parts it names. They are keyed by the upload
        // id and immutable, so a retry re-writes identical bytes.
        ports.registry.stage_part_blocks(&staged).await?;

        let receipt = Receipt {
            scope: scope.to_owned(),
            key_sha256: key_digest(key),
            intent,
            response_kind: "Upload".to_owned(),
            response: ReceiptBody::Inline(canonical),
            committed_at: now,
            expires_at: Timestamp::from_unix_millis(
                now.unix_millis()
                    .saturating_add(i64::try_from(RECEIPT_RETENTION.as_millis()).unwrap_or(0)),
            )
            .map_err(|error| StoreError::Invalid {
                detail: error.to_string(),
            })?,
        };
        let receipt_item =
            encode_receipt_row(workspace, &receipt).map_err(|error| StoreError::Invalid {
                detail: error.to_string(),
            })?;

        let due = due_item(&staged, self.context().auth.organization_id, now)?;

        // Three items, two tables, one transaction: the upload row, the
        // `registry.upload_expiry` due item and the receipt. The receipt has to
        // land with the row, or a replay could be answered after the row exists
        // but before the receipt does — and would open a second multipart upload.
        let mut transaction = TransactionPlan::new(format!("upload-create:{}", staged.id));
        transaction.put(
            Participant::REGISTRY_UPLOAD,
            immutable_put(
                ports.registry_table,
                aex_registry_dynamodb::codec::encode_upload(&staged).head,
            ),
        )?;
        transaction.put(
            Participant::WORK_NEXT_WAKE,
            immutable_put(ports.work_table, due),
        )?;
        transaction.put(
            Participant::SESSION_IDEMPOTENCY,
            immutable_put(ports.registry_table, receipt_item),
        )?;
        ports.registry.commit_admission(&transaction).await?;

        Ok(UploadResponse(rendered))
    }

    /// `POST /api/workspace/uploads/{uploadId}/parts`
    async fn grant_parts(
        &self,
        id: UploadId,
        body: models::UploadPartsRequest,
    ) -> WireResult<models::UploadPartGrants> {
        if body.parts.len() > PART_GRANT_MAX_PER_CALL {
            return Err(WireError::new(ErrorCode::LimitExceeded)
                .with_message("a grant call covers at most 1000 parts"));
        }
        let now = self.upload_now()?;
        let stored = self.load_upload(id).await?;
        if stored.state.is_terminal() || stored.state == UploadState::Ready {
            return Err(WireError::new(ErrorCode::Gone));
        }

        let requests: Vec<upload::PartGrantRequest> = body
            .parts
            .iter()
            .map(|part| {
                Ok(upload::PartGrantRequest {
                    number: part.part_number,
                    sha256: part.sha256,
                    size_bytes: u64::try_from(part.size_bytes.get()).map_err(|_| {
                        WireError::new(ErrorCode::InvalidRequest).with_message("part size")
                    })?,
                })
            })
            .collect::<WireResult<Vec<_>>>()?;

        let commit =
            upload::grant_parts(&stored, &requests, now).map_err(|error| refuse(&error))?;
        let ports = self.upload_ports();
        ports
            .registry
            .record_part_declarations(&commit.upload)
            .await
            .map_err(|error| store_failure(&error))?;

        let handle = MultipartHandle {
            key: ObjectKey::parse(&commit.upload.object_key).map_err(|_| {
                WireError::new(ErrorCode::InternalError).with_message("stored object key")
            })?,
            upload_id: commit.upload.provider_upload_id.clone(),
        };
        let expires_at = Timestamp::from_unix_millis(
            now.unix_millis()
                .saturating_add(i64::try_from(PRESIGN_EXPIRY.as_millis()).unwrap_or(0)),
        )
        .map_err(|_| WireError::new(ErrorCode::InternalError).with_message("expiry"))?;

        let mut grants = Vec::with_capacity(commit.granted.len());
        for number in &commit.granted {
            let planned = commit
                .upload
                .parts
                .part(*number)
                .ok_or_else(|| WireError::new(ErrorCode::InvalidRequest))?;
            let digest = planned.sha256.ok_or_else(|| {
                WireError::new(ErrorCode::InvalidRequest).with_message("no part digest")
            })?;
            let signed = ports
                .objects
                .presign_part(
                    &handle,
                    i32::try_from(*number).unwrap_or(i32::MAX),
                    &checksum_base64(&digest),
                    planned.bytes,
                )
                .await
                .map_err(|error| {
                    WireError::new(ErrorCode::InternalError).with_message(error.to_string())
                })?;
            grants.push(models::UploadPartGrant {
                part_number: *number,
                url: HttpsUrl::parse(signed.url.expose()).map_err(|_| {
                    WireError::new(ErrorCode::InternalError).with_message("signed url")
                })?,
                // The headers are the difference between a grant and a URL that
                // S3 rejects: they carry the checksum and the length the
                // signature covers.
                headers: signed
                    .headers
                    .into_iter()
                    .map(|(name, value)| models::HttpHeader { name, value })
                    .collect(),
                expires_at,
            });
        }

        Ok(models::UploadPartGrants {
            upload_id: commit.upload.id,
            grants,
        })
    }

    /// `POST /api/workspace/uploads/{uploadId}/completion`
    async fn complete_upload(
        &self,
        id: UploadId,
        body: models::UploadCompleteRequest,
    ) -> WireResult<models::Upload> {
        let now = self.upload_now()?;
        let stored = self.load_upload(id).await?;
        // `Ready` replays: the bytes the caller asked for are committed, which is
        // the upload's entire purpose.
        if stored.state == UploadState::Ready {
            return wire_upload(&stored);
        }
        if stored.state.is_terminal() {
            return Err(WireError::new(ErrorCode::Gone));
        }

        let receipts: Vec<PartReceipt> = body
            .parts
            .iter()
            .map(|part| PartReceipt {
                number: part.part_number,
                etag: part.etag.as_str().to_owned(),
            })
            .collect();
        let begun =
            upload::begin_complete(&stored, &receipts, now).map_err(|error| refuse(&error))?;
        let intent_hash = completion_intent_hash(&begun.upload);
        let ports = self.upload_ports();
        ports
            .registry
            .begin_completion(&begun.upload, &intent_hash)
            .await
            .map_err(|error| store_failure(&error))?;

        let manifest = completion_manifest(&begun.upload)?;
        let handle = MultipartHandle {
            key: ObjectKey::parse(&begun.upload.object_key).map_err(|_| {
                WireError::new(ErrorCode::InternalError).with_message("stored object key")
            })?,
            upload_id: begun.upload.provider_upload_id.clone(),
        };

        let evidence = match ports.objects.complete_multipart(&handle, &manifest).await {
            Ok(commit) => CompletionEvidence {
                etag: commit.etag,
                checksum_sha256: commit.checksum_sha256,
                checksum_crc64_nvme: commit.checksum_crc64_nvme,
                part_count: commit.part_count,
            },
            Err(error) => return self.resolve_completion(&begun.upload, &error, now).await,
        };

        let settled = upload::finish_complete(
            &begun.upload,
            &upload::VerifiedObject {
                size_bytes: begun.upload.declared_size,
                sha256: begun.upload.declared_sha256,
                evidence,
            },
            now,
        )
        .map_err(|error| refuse(&error))?;
        self.settle_ready(&settled.upload).await?;
        wire_upload(&settled.upload)
    }

    /// Resolves a completion whose outcome the provider left unknown (E D-3).
    async fn resolve_completion(
        &self,
        stored: &Upload,
        error: &aex_content_aws::ContentObjectError,
        now: Timestamp,
    ) -> WireResult<models::Upload> {
        // Only an ambiguous commit is resolvable by heading the key. An integrity
        // refusal is the provider telling us the manifest is wrong, and that is
        // the client's answer, not a reason to look for the object.
        if let aex_content_aws::ContentObjectError::IntegrityMismatch { detail } = error {
            return Err(WireError::new(ErrorCode::InvalidRequest).with_message(detail.clone()));
        }
        let oracle = self.head_oracle(stored).await;
        match upload::resolve_completing(stored, &oracle, now) {
            AmbiguityResolution::Committed(settled) => {
                self.settle_ready(&settled.upload).await?;
                wire_upload(&settled.upload)
            }
            // The object is not there. The row stays `Completing` and the sweep
            // owns the abort; the client retries.
            AmbiguityResolution::NotCommitted => Err(WireError::new(ErrorCode::ContentMissing)),
            // Impossible under content addressing. Refuse loudly and leave the
            // row visible rather than cleaning it up wrongly.
            AmbiguityResolution::Integrity { detail } => {
                Err(WireError::new(ErrorCode::InternalError).with_message(detail))
            }
            AmbiguityResolution::Retry => Err(WireError::new(ErrorCode::InternalError)
                .with_message("the completion outcome is not yet established")),
            AmbiguityResolution::AlreadySettled => wire_upload(stored),
        }
    }

    /// `DELETE /api/workspace/uploads/{uploadId}`
    async fn abort_upload(&self, id: UploadId) -> WireResult<()> {
        let now = self.upload_now()?;
        let stored = self.load_upload(id).await?;
        let ports = self.upload_ports();

        if stored.state.can_abort() {
            let handle = MultipartHandle {
                key: ObjectKey::parse(&stored.object_key).map_err(|_| {
                    WireError::new(ErrorCode::InternalError).with_message("stored object key")
                })?,
                upload_id: stored.provider_upload_id.clone(),
            };
            // S3 first, then the conditional write that records it.
            ports
                .objects
                .abort_multipart(&handle)
                .await
                .map_err(|error| {
                    WireError::new(ErrorCode::InternalError).with_message(error.to_string())
                })?;
            let aborted = upload::abort(&stored, now).map_err(|error| refuse(&error))?;
            return ports
                .registry
                .transition_upload_fenced(
                    stored.id,
                    stored.state,
                    aborted.upload.state,
                    &stored.provider_upload_id,
                )
                .await
                .map_err(|error| store_failure(&error));
        }

        if stored.state == UploadState::Completing {
            // An upload that may already be assembled is never aborted on a
            // guess. The oracle decides, and only "the object is absent" makes
            // the abort safe.
            let oracle = self.head_oracle(&stored).await;
            return match upload::resolve_completing(&stored, &oracle, now) {
                AmbiguityResolution::Committed(settled) => {
                    self.settle_ready(&settled.upload).await?;
                    Err(WireError::new(ErrorCode::Gone))
                }
                AmbiguityResolution::NotCommitted => {
                    let handle = MultipartHandle {
                        key: ObjectKey::parse(&stored.object_key).map_err(|_| {
                            WireError::new(ErrorCode::InternalError).with_message("object key")
                        })?,
                        upload_id: stored.provider_upload_id.clone(),
                    };
                    ports
                        .objects
                        .abort_multipart(&handle)
                        .await
                        .map_err(|error| {
                            WireError::new(ErrorCode::InternalError).with_message(error.to_string())
                        })?;
                    ports
                        .registry
                        .transition_upload_fenced(
                            stored.id,
                            UploadState::Completing,
                            UploadState::Expired,
                            &stored.provider_upload_id,
                        )
                        .await
                        .map_err(|error| store_failure(&error))
                }
                AmbiguityResolution::Integrity { detail } => {
                    Err(WireError::new(ErrorCode::InternalError).with_message(detail))
                }
                AmbiguityResolution::Retry => Err(WireError::new(ErrorCode::InternalError)
                    .with_message("the completion outcome is not yet established")),
                AmbiguityResolution::AlreadySettled => Err(WireError::new(ErrorCode::Gone)),
            };
        }

        // `Ready` and every terminal state.
        Err(WireError::new(ErrorCode::Gone))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The identity of one completion intent.
///
/// A retry carrying a different manifest fails the stored hash and is an
/// idempotency conflict; a retry carrying the same manifest re-enters
/// `Completing` and is safe.
fn completion_intent_hash(upload: &Upload) -> String {
    use sha2::Digest as _;
    let mut hasher = sha2::Sha256::new();
    hasher.update(upload.id.to_string().as_bytes());
    for part in &upload.completion_manifest {
        hasher.update(part.number.to_be_bytes());
        hasher.update(b":");
        hasher.update(part.etag.as_bytes());
        hasher.update(b";");
    }
    hex::encode(hasher.finalize())
}

/// Builds the provider manifest from the stored plan and the stored declarations.
///
/// Sizes come from the plan and digests from the declarations, which is why the
/// completion request carries only `{partNumber, etag}` (E D-9).
fn completion_manifest(upload: &Upload) -> WireResult<CompletionManifest> {
    let declared = upload.declared_parts().map_err(|error| {
        WireError::new(ErrorCode::InvalidRequest).with_message(error.to_string())
    })?;
    let mut parts = Vec::with_capacity(declared.len());
    for (number, bytes, digest) in declared {
        let etag = upload
            .completion_manifest
            .iter()
            .find(|part| part.number == number)
            .map(|part| part.etag.clone())
            .ok_or_else(|| {
                WireError::new(ErrorCode::InvalidRequest)
                    .with_message(format!("part {number} has no submitted ETag"))
            })?;
        parts.push(CompletedPartPlan {
            plan: AwsPartPlan {
                part_number: i32::try_from(number).unwrap_or(i32::MAX),
                size: bytes,
                sha256_base64: checksum_base64(&digest),
            },
            etag,
        });
    }
    Ok(CompletionManifest {
        parts,
        total_bytes: upload.declared_size,
        whole_object_sha256: upload.declared_sha256,
    })
}

/// One transaction action that must not overwrite an existing row.
fn immutable_put(
    table: &str,
    item: aex_session_dynamodb::attr::Item,
) -> aws_sdk_dynamodb::types::builders::PutBuilder {
    aws_sdk_dynamodb::types::Put::builder()
        .table_name(table)
        .set_item(Some(item))
        .condition_expression(aex_session_dynamodb::plan::IMMUTABLE)
}

/// The `registry.upload_expiry` due item, written in the admission transaction.
///
/// The kind and its payload keys were declared and unused; this is their first
/// consumer (E D-8).
fn due_item(
    upload: &Upload,
    organization: aex_wire::ids::OrganizationId,
    now: Timestamp,
) -> Result<aex_session_dynamodb::attr::Item, StoreError> {
    let payload = aex_work_dynamodb::codec::Payload::new()
        .set("uploadId", upload.id.to_string())
        .set("workspaceId", upload.workspace.to_string());
    let record = aex_work_dynamodb::codec::WorkRecord {
        work_id: format!("upload-expiry-{}", upload.id),
        workspace: upload.workspace,
        organization,
        session: None,
        agent: None,
        kind: "registry.upload_expiry".to_owned(),
        priority: 5,
        due_at: upload.expires_at,
        state: "pending".to_owned(),
        attempt: 0,
        max_attempts: 8,
        fence: 0,
        claim_owner: None,
        lease_expires_at: None,
        dedupe_key: upload.id.to_string(),
        payload,
        delivery: aex_work_dynamodb::codec::DeliveryEvidence::default(),
        created_at: now,
        updated_at: now,
    };
    aex_work_dynamodb::codec::encode_work(&record).map_err(|error| StoreError::Invalid {
        detail: error.to_string(),
    })
}

// ---------------------------------------------------------------------------
// The generated trait
// ---------------------------------------------------------------------------

impl aex_wire::server::UploadsApi for Routes {
    async fn upload_create(
        &self,
        _cx: &aex_wire::server::RequestContext,
        body: models::UploadCreateRequest,
    ) -> WireResult<aex_wire::server::Created<models::Upload>> {
        self.stage_upload(body).await.map(aex_wire::server::Created)
    }

    async fn upload_parts_grant(
        &self,
        _cx: &aex_wire::server::RequestContext,
        upload_id: UploadId,
        body: models::UploadPartsRequest,
    ) -> WireResult<models::UploadPartGrants> {
        self.grant_parts(upload_id, body).await
    }

    async fn upload_complete(
        &self,
        _cx: &aex_wire::server::RequestContext,
        upload_id: UploadId,
        body: models::UploadCompleteRequest,
    ) -> WireResult<models::Upload> {
        self.complete_upload(upload_id, body).await
    }

    async fn upload_abort(
        &self,
        _cx: &aex_wire::server::RequestContext,
        upload_id: UploadId,
    ) -> WireResult<aex_wire::server::NoContent> {
        self.abort_upload(upload_id)
            .await
            .map(|()| aex_wire::server::NoContent)
    }
}

#[cfg(test)]
mod tests {
    use aex_wire::ids::{PrefixedId as _, UploadId, Uuid7, WorkspaceId};
    use aex_wire::types::Timestamp;
    use aex_workspace_domain::upload::{PartPlan, PlannedPart, Upload, UploadState};

    use super::{completion_intent_hash, encryption_context, wire_state, wire_upload};

    fn upload(state: UploadState) -> Upload {
        Upload {
            id: UploadId::from_uuid7(Uuid7::compose(1, [1; 10])),
            workspace: WorkspaceId::from_uuid7(Uuid7::compose(1, [2; 10])),
            state,
            provider_upload_id: "provider-mpu-1".to_owned(),
            object_key: "wks/ab/cd/abcd".to_owned(),
            declared_size: 1_024,
            declared_sha256: aex_wire::ids::ContentHash::from_bytes([2; 32]),
            content_type: Some("application/zip".to_owned()),
            parts: PartPlan {
                parts: vec![PlannedPart {
                    number: 1,
                    bytes: 1_024,
                    sha256: None,
                }],
            },
            completion_manifest: Vec::new(),
            completion: None,
            consumed_by: None,
            created_at: Timestamp::from_unix_millis(0).expect("in range"),
            expires_at: Timestamp::from_unix_millis(86_400_000).expect("in range"),
        }
    }

    #[test]
    fn every_terminal_state_is_absent_from_the_wire() {
        for state in UploadState::ALL {
            let rendered = wire_state(state);
            assert_eq!(
                rendered.is_none(),
                state.is_terminal(),
                "{state:?} must be `gone` on the wire, not a lifecycle position"
            );
        }
    }

    #[test]
    fn a_terminal_upload_can_never_be_projected_onto_a_wire_upload() {
        for state in [
            UploadState::Consumed,
            UploadState::Aborted,
            UploadState::Expired,
        ] {
            assert!(wire_upload(&upload(state)).is_err(), "{state:?}");
        }
        assert!(wire_upload(&upload(UploadState::Created)).is_ok());
        assert!(wire_upload(&upload(UploadState::Ready)).is_ok());
    }

    #[test]
    fn the_completion_intent_changes_with_the_manifest_it_pins() {
        let mut first = upload(UploadState::PartsGranted);
        first.completion_manifest = vec![aex_workspace_domain::upload::SubmittedPart {
            number: 1,
            etag: "\"one\"".to_owned(),
        }];
        let mut second = first.clone();
        second.completion_manifest[0].etag = "\"other\"".to_owned();
        assert_ne!(
            completion_intent_hash(&first),
            completion_intent_hash(&second),
            "a retry carrying a different manifest must lose the stored intent"
        );
    }

    #[test]
    fn the_encryption_context_is_canonical_json_naming_the_workspace() {
        let workspace = WorkspaceId::from_uuid7(Uuid7::compose(1, [2; 10]));
        let rendered = String::from_utf8(encryption_context(workspace)).expect("utf-8");
        assert!(rendered.starts_with(r#"{"aex:domain":"content-object","aex:workspace":"#));
        assert!(rendered.contains(&workspace.to_string()));
    }
}
