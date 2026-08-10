//! The named registry's read, write and delete paths.
//!
//! One mechanism over five kinds. The complete non-payload value of a registered
//! resource is a canonical JSON document stored **on the pointer row**, so a
//! point read is one `GetItem` and zero content reads, and no registry response
//! ever publishes payload bytes (D-2, D-3). The payload — file bytes, a skill or
//! tool bundle — is admitted as an SSE-KMS object and referenced by digest.
//!
//! `sha256` and `sizeBytes` on a registered resource are the value **document**'s
//! own, for all five kinds (D-4). That is what makes the domain's `Unchanged`
//! rule correct: a metadata-only edit over identical payload bytes changes the
//! document, so it reports `Replaced` and is written, rather than comparing equal
//! and being silently discarded.

use aex_content_aws::object_store::PutImmutable;
use aex_content_domain::identity::RegistryKind;
use aex_content_dynamodb::codec::{ContentDescriptor, ContentPin, ObjectLocation};
use aex_content_dynamodb::wire_pending::PinOwner;
use aex_regional_http::projection::{ProjectionError, authority_failure};
use aex_registry_dynamodb::store::{SET_RESPONSE_KIND, SetCommit, SetCommitted, SetReceipt};
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::replay::{IdempotencyScope, RECEIPT_RETENTION, Receipt};
use aex_wire::error::{ErrorCode, WireError, WireResult};
use aex_wire::ids::{ContentHash, ResourceName};
use aex_wire::models;
use aex_wire::server::{NoContent, WithETag};
use aex_wire::types::{DecimalU128, Timestamp};
use aex_workspace_domain::registry::RegistryRejection;
use aex_workspace_domain::registry::{
    ProposedValue, RegisteredValueRef, RegistryPointer, SetOutcome, ValueDocument, set,
};
use aex_workspace_domain::upload::UploadState;
use serde::Serialize;

use super::handlers::Routes;

/// A payload the request named, resolved to a durable content reference.
///
/// The reference is what the *pointer* records; the digest inside `content` is
/// what the value document records. A pointer never stores an upload id (D-15):
/// an upload is consumed at admission time and resolved to its digest, because a
/// pointer that named an upload would outlive its own referent.
pub struct AdmittedPayload {
    /// The payload source, for the transaction that consumes it.
    pub source: RegisteredValueRef,
    /// What the value document publishes.
    pub reference: models::ContentRef,
}

/// The media type every registry payload is stored under.
///
/// The registry never interprets payload bytes, and `RegisteredFileValue`'s own
/// `mediaType` is customer metadata that belongs in the value document, not in
/// the content descriptor.
const PAYLOAD_MEDIA_TYPE: &str = "application/octet-stream";

impl Routes {
    /// Reads one registered resource, value and all.
    ///
    /// One eventually consistent `GetItem` and no content read at all (D-12).
    pub(crate) async fn registry_get<T>(
        &self,
        kind: RegistryKind,
        name: &ResourceName,
        project: fn(&RegistryPointer) -> Result<T, ProjectionError>,
    ) -> WireResult<WithETag<T>> {
        let pointer = self
            .shared
            .registry
            .load_pointer(self.cx.auth.workspace_id, kind, name.as_str())
            .await
            .map_err(|error| authority_failure(&error))?
            .ok_or_else(|| WireError::new(ErrorCode::NotFound))?;
        let etag = pointer.row.etag.clone();
        let value = project(&pointer).map_err(WireError::from)?;
        Ok(WithETag { value, etag })
    }

    /// Creates or replaces one registered resource.
    ///
    /// The order is fixed by D-7 and every step before the last is individually
    /// idempotent, so a crash leaves an unreferenced-but-pinned body that GC
    /// reclaims and never a pointer naming an unpinned body: admit the payload,
    /// pin it, then commit the pointer, the upload consume, the entry claim and
    /// the durable receipt as one transaction over `regional-registry`.
    pub(crate) async fn registry_put<V, T>(
        &self,
        kind: RegistryKind,
        name: &ResourceName,
        read: &V,
        payload: Option<RegisteredValueRef>,
        project: fn(&RegistryPointer) -> Result<T, ProjectionError>,
    ) -> WireResult<WithETag<T>>
    where
        V: Serialize,
    {
        let workspace = self.cx.auth.workspace_id;
        let now = self.cx.now().map_err(|error| {
            WireError::new(ErrorCode::InternalError).with_message(error.to_string())
        })?;
        let identity = self.cx.idempotency.as_ref().ok_or_else(|| {
            // The route declares `idempotency: idempotency_key`. A request that
            // reached a handler without one is a boundary defect, and answering
            // it would write without a replay fence.
            WireError::new(ErrorCode::InvalidRequest)
                .with_message("this route requires an `Idempotency-Key`".to_owned())
        })?;

        let value_doc = value_document(read)?;
        if value_doc.size_bytes() > self.shared.registry_value_bytes {
            return Err(
                WireError::new(ErrorCode::LimitExceeded).with_message(format!(
                    "the canonical value document measures {} bytes; the limit is {}",
                    value_doc.size_bytes(),
                    self.shared.registry_value_bytes
                )),
            );
        }

        let upload_state = match &payload {
            Some(RegisteredValueRef::Upload { upload }) => self
                .shared
                .registry
                .load_upload(workspace, *upload)
                .await
                .map_err(|error| authority_failure(&error))?
                .map(|staged| staged.state),
            _ => None,
        };
        let proposed = ProposedValue {
            workspace,
            kind,
            name: name.clone(),
            value_doc,
            payload,
            upload_state,
        };

        let current = self
            .shared
            .registry
            .load_pointer(workspace, kind, name.as_str())
            .await
            .map_err(|error| authority_failure(&error))?;
        let (outcome, commit) =
            set(current.as_ref(), &proposed, self.cx.if_match.as_ref(), now).map_err(rejection)?;

        // `Unchanged` writes nothing at all, so a concurrent editor's `If-Match`
        // survives an idempotent retry (D-08). There is nothing to make
        // idempotent either: the answer is the row that is already there.
        if !commit.wrote {
            let etag = commit.pointer.row.etag.clone();
            let value = project(&commit.pointer).map_err(WireError::from)?;
            return Ok(WithETag { value, etag });
        }

        let scope =
            IdempotencyScope::new("registry.set", Some(kind.as_str())).map_err(|error| {
                WireError::new(ErrorCode::InternalError).with_message(error.to_string())
            })?;
        let answer = SetReceipt::of(outcome, &commit.pointer);
        let receipt = Receipt {
            scope: scope.render(),
            key_sha256: aex_session_dynamodb::replay::key_digest(&identity.key),
            intent: aex_wire::idempotency::IntentDigest::from_bytes(identity.intent),
            response_kind: SET_RESPONSE_KIND.to_owned(),
            response: answer
                .to_body()
                .map_err(|error| authority_failure(&error))?,
            committed_at: now,
            expires_at: retention_end(now)?,
        };
        let committed = self
            .shared
            .registry
            .commit_set(
                workspace,
                SetCommit {
                    commit: &commit,
                    from_revision: current.as_ref().map(|pointer| pointer.row.revision),
                    entries_cap: self.shared.registry_entries,
                    creates: outcome == SetOutcome::Created,
                    receipt: &receipt,
                },
            )
            .await
            .map_err(|error| commit_failure(&error))?;

        let pointer = match committed {
            SetCommitted::Committed => commit.pointer,
            SetCommitted::Replayed(stored) => stored.pointer(workspace),
        };
        let etag = pointer.row.etag.clone();
        let value = project(&pointer).map_err(WireError::from)?;
        Ok(WithETag { value, etag })
    }

    /// Removes one registered resource.
    ///
    /// `204` always, including for a name that was never there: the route
    /// declares no `not_found`, and delete is idempotent (D-9). The bytes stay —
    /// a running session holds its own root pin, so removing the registry pin is
    /// left to `content-lifecycle-worker` (D-10) and never happens here.
    pub(crate) async fn registry_delete(
        &self,
        kind: RegistryKind,
        name: &ResourceName,
    ) -> WireResult<NoContent> {
        self.shared
            .registry
            .commit_delete(
                self.cx.auth.workspace_id,
                kind,
                name.as_str(),
                self.cx.if_match.as_ref(),
            )
            .await
            .map_err(|error| authority_failure(&error))?;
        Ok(NoContent)
    }

    /// Resolves one request payload to a durable, pinned content reference.
    ///
    /// Registry payloads are **always object-placed**, however short (D-11): the
    /// API never reads a registry payload, so an inline copy would buy nothing
    /// and would leave the download route with no object to sign.
    pub(crate) async fn admit_payload(
        &self,
        kind: RegistryKind,
        name: &ResourceName,
        input: &models::BlobInput,
    ) -> WireResult<AdmittedPayload> {
        match input {
            models::BlobInput::Inline(inline) => self.admit_inline(kind, name, inline).await,
            models::BlobInput::Upload(staged) => self.admit_staged(kind, name, staged).await,
        }
    }

    /// Admits a payload the request carried, as an object.
    async fn admit_inline(
        &self,
        kind: RegistryKind,
        name: &ResourceName,
        inline: &models::BlobInline,
    ) -> WireResult<AdmittedPayload> {
        let workspace = self.cx.auth.workspace_id;
        let now = self.cx.now().map_err(|error| {
            WireError::new(ErrorCode::InternalError).with_message(error.to_string())
        })?;
        {
            {
                let bytes = decode_inline(inline)?;
                let observed = ContentHash::of(&bytes);
                if observed != inline.sha256 {
                    return Err(WireError::new(ErrorCode::InvalidRequest).with_message(
                        "the declared `sha256` is not the digest of the supplied bytes".to_owned(),
                    ));
                }
                let size = bytes.len() as u64;
                let commit = self
                    .shared
                    .content_objects
                    .put_immutable(PutImmutable {
                        workspace,
                        digest: &observed,
                        plaintext_bytes: size,
                        body: bytes,
                        encryption_context: self.shared.content_encryption_context.as_slice(),
                    })
                    .await
                    .map_err(|error| {
                        WireError::new(ErrorCode::InternalError).with_message(error.to_string())
                    })?;
                self.shared
                    .content
                    .admit_body(
                        &ContentDescriptor {
                            workspace,
                            organization: self.cx.auth.organization_id,
                            digest: observed,
                            size_bytes: size,
                            media_type: PAYLOAD_MEDIA_TYPE.to_owned(),
                            placement: aex_session_dynamodb::measure::Placement::ObjectStore,
                            state: "staged".to_owned(),
                            gc_epoch: 0,
                            created_at: now,
                            verified_at: None,
                            object: Some(ObjectLocation {
                                key: commit.key.as_str().to_owned(),
                                etag: commit.etag,
                                checksum_sha256: commit.checksum_sha256.unwrap_or_default(),
                                checksum_crc64_nvme: commit.checksum_crc64_nvme.unwrap_or_default(),
                                part_count: commit.part_count,
                                kms_key_id: self.shared.content_kms_key_id.clone(),
                            }),
                        },
                        &ContentPin {
                            workspace,
                            owner: PinOwner::Registry {
                                kind: kind.as_str().to_owned(),
                                name: name.as_str().to_owned(),
                            },
                            created_at: now,
                        },
                        now,
                    )
                    .await
                    .map_err(|error| authority_failure(&error))?;
                Ok(AdmittedPayload {
                    source: RegisteredValueRef::Content { digest: observed },
                    reference: models::ContentRef {
                        sha256: observed,
                        size_bytes: DecimalU128::new(u128::from(size)),
                    },
                })
            }
        }
    }

    /// Adopts a payload the upload surface already committed.
    async fn admit_staged(
        &self,
        kind: RegistryKind,
        name: &ResourceName,
        staged: &models::BlobUpload,
    ) -> WireResult<AdmittedPayload> {
        let workspace = self.cx.auth.workspace_id;
        let now = self.cx.now().map_err(|error| {
            WireError::new(ErrorCode::InternalError).with_message(error.to_string())
        })?;
        {
            {
                // The upload surface verified the bytes against
                // `declaredSha256` when it completed; this never re-verifies
                // them. What it does require is the evidence that completion
                // happened: a `Ready` row and a committed descriptor. Either
                // missing is `content_missing`, loudly, never a pointer that
                // names bytes nobody wrote.
                let upload = self
                    .shared
                    .registry
                    .load_upload(workspace, staged.upload_id)
                    .await
                    .map_err(|error| authority_failure(&error))?
                    .ok_or_else(|| WireError::new(ErrorCode::ContentMissing))?;
                if upload.state != UploadState::Ready || upload.declared_sha256 != staged.sha256 {
                    return Err(WireError::new(ErrorCode::ContentMissing));
                }
                let descriptor = self
                    .shared
                    .content
                    .load_descriptor(workspace, &staged.sha256)
                    .await
                    .map_err(|error| authority_failure(&error))?
                    .ok_or_else(|| WireError::new(ErrorCode::ContentMissing))?;
                self.shared
                    .content
                    .admit_body(
                        &descriptor,
                        &ContentPin {
                            workspace,
                            owner: PinOwner::Registry {
                                kind: kind.as_str().to_owned(),
                                name: name.as_str().to_owned(),
                            },
                            created_at: now,
                        },
                        now,
                    )
                    .await
                    .map_err(|error| authority_failure(&error))?;
                Ok(AdmittedPayload {
                    source: RegisteredValueRef::Upload {
                        upload: staged.upload_id,
                    },
                    reference: models::ContentRef {
                        sha256: staged.sha256,
                        size_bytes: staged.size_bytes,
                    },
                })
            }
        }
    }
}

/// Canonicalizes one read model into the document the pointer row stores.
fn value_document<V: Serialize>(read: &V) -> WireResult<ValueDocument> {
    let value = serde_json::to_value(read).map_err(|error| {
        WireError::new(ErrorCode::InternalError).with_message(error.to_string())
    })?;
    Ok(ValueDocument::new(
        aex_wire::CanonicalJson::from_value(&value).map_err(|error| {
            WireError::new(ErrorCode::InternalError).with_message(error.to_string())
        })?,
    ))
}

/// Decodes one inline blob according to its declared encoding.
fn decode_inline(inline: &models::BlobInline) -> WireResult<Vec<u8>> {
    match inline.encoding {
        models::BlobEncoding::Utf8 => Ok(inline.data.clone().into_bytes()),
        models::BlobEncoding::Base64 => {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD
                .decode(inline.data.as_bytes())
                .map_err(|error| {
                    WireError::new(ErrorCode::InvalidRequest).with_message(error.to_string())
                })
        }
    }
}

/// When a receipt written now stops being readable (OD-16).
fn retention_end(now: Timestamp) -> WireResult<Timestamp> {
    let millis = i64::try_from(RECEIPT_RETENTION.as_millis()).unwrap_or(i64::MAX);
    Timestamp::from_unix_millis(now.unix_millis().saturating_add(millis))
        .map_err(|error| WireError::new(ErrorCode::InternalError).with_message(error.to_string()))
}

/// Renders one domain refusal as its declared code.
fn rejection(error: RegistryRejection) -> WireError {
    match error {
        RegistryRejection::PreconditionFailed { current } => {
            let refusal = WireError::new(ErrorCode::PreconditionFailed);
            match current {
                None => refusal.with_message("the name does not exist".to_owned()),
                Some(tag) => refusal.with_message(format!("the current entity tag is `{tag}`")),
            }
        }
        RegistryRejection::InvalidValue(cause) => {
            WireError::new(ErrorCode::InvalidRequest).with_message(cause.to_string())
        }
        // A staged upload that is not `Ready` has no bytes to point at. It is
        // the same refusal as an absent one, and for the same reason.
        RegistryRejection::UploadNotReady(_) => WireError::new(ErrorCode::ContentMissing),
    }
}

/// Renders one commit failure, separating the entry cap from every other lost
/// condition.
fn commit_failure(error: &StoreError) -> WireError {
    if let StoreError::PreconditionFailed { participant, .. } = error
        && *participant == aex_session_dynamodb::plan::Participant::REGISTRY_COUNT
    {
        return WireError::new(ErrorCode::LimitExceeded)
            .with_message("this registry is at its `registry.entries` limit".to_owned());
    }
    authority_failure(error)
}
