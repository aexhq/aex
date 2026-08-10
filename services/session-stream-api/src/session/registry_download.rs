//! Registry-file download grant composition.
//!
//! A presigned URL is derived only after the replay reader has elected this
//! request to attempt a commit. Its exact wire bytes then land in the same
//! transaction as the grant and pin, and no URL leaves this module unless that
//! transaction commits. An exact replay returns those stored bytes and never
//! asks S3 to sign again.

use aex_content_aws::errors::ContentObjectError;
use aex_content_aws::object_key::{ObjectKey, PRESIGN_EXPIRY};
use aex_content_domain::{ContentDigest, ContentObjectKey, GrantId, MediaType};
use aex_regional_http::projection::{self, authority_failure};
use aex_session_app::plan::{Condition, SessionTransaction, TableFamily, TransactionIntent, Write};
use aex_session_domain::{
    IdempotencyIdentity, IdempotencyReceipt, ReceiptKey, ReceiptOutcome, ResourceId, ResourceKind,
    ResponseBody,
};
use aex_session_dynamodb::app_authority::{ApiHintSink, DynamoAuthorityCommitter};
use aex_session_dynamodb::application_plan::{
    FamilyCompilers, IdempotencyCompiler, WorkspaceBinding,
};
use aex_session_dynamodb::error::{RetryPolicy, StoreError};
use aex_session_dynamodb::plan::Participant;
use aex_session_dynamodb::replay::{
    DecodeReceipt, IdempotencyScope, Receipt, ReceiptBody, ReplayRequest, commit_or_replay,
};
use aex_wire::error::{ErrorCode, WireError, WireResult};
use aex_wire::idempotency::{IntentDigest, ReplayIdentity};
use aex_wire::ids::{MeasurementId, PrefixedId as _, ResourceName};
use aex_wire::models;
use aex_wire::types::{DecimalU128, HttpsUrl};
use aex_workspace_domain::{
    ByteRange, ContentObjectLocation, GrantContentDescriptor, GrantRejection, GrantSubject,
    ObjectChecksum, RegistrySelector, mint_grant,
};

use super::handlers::Routes;

const FILE_SUBJECT: &str = "file";

/// No waiting on a synchronous public request; the replay loop remains bounded
/// and always re-reads before retrying a write.
struct Immediate;

#[async_trait::async_trait]
impl aex_session_dynamodb::replay::Backoff for Immediate {
    async fn wait(&self, _policy: RetryPolicy, _attempt: u32) {}
}

#[derive(Debug, Clone)]
struct GrantContent {
    digest: ContentDigest,
    size_bytes: u64,
    media_type: Option<MediaType>,
}

impl GrantContentDescriptor for GrantContent {
    fn digest(&self) -> ContentDigest {
        self.digest
    }

    fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    fn media_type(&self) -> Option<MediaType> {
        self.media_type.clone()
    }
}

/// The canonical public response stored on the receipt.
#[derive(Debug, Clone, PartialEq)]
struct DownloadResponse(models::DownloadGrant);

impl DecodeReceipt for DownloadResponse {
    fn decode_receipt(receipt: &Receipt) -> Result<Self, StoreError> {
        if receipt.response_kind != ResourceKind::Grant.as_str() {
            return Err(StoreError::Invalid {
                detail: format!(
                    "a registry download receipt has response kind `{}`",
                    receipt.response_kind
                ),
            });
        }
        let ReceiptBody::Inline(bytes) = &receipt.response else {
            return Err(StoreError::Invalid {
                detail: "a download-grant response must be stored inline".to_owned(),
            });
        };
        serde_json::from_slice(bytes)
            .map(Self)
            .map_err(|error| StoreError::Invalid {
                detail: format!("a stored download grant is malformed: {error}"),
            })
    }
}

impl Routes {
    /// Mints one five-minute direct S3 grant for a registered file.
    pub(crate) async fn registry_file_download_create(
        &self,
        name: &ResourceName,
        request: &models::RegistryDownloadRequest,
    ) -> WireResult<models::DownloadGrant> {
        let workspace = self.context().auth.workspace_id;
        let organization = self.context().auth.organization_id;
        let now = self.now()?;
        let replay = self.context().idempotency.as_ref().ok_or_else(|| {
            WireError::new(ErrorCode::InvalidRequest)
                .with_message("this route requires an `Idempotency-Key`")
        })?;
        let requested = request.range.map(domain_range).transpose()?;
        let wire_range = request.range;
        // D-23 has one request digest. Registry state is intentionally not
        // folded into a second identity: a live receipt remains the winner even
        // if the mutable name has since changed or disappeared.
        let intent = IntentDigest::from_bytes(replay.intent);
        let scope =
            IdempotencyScope::new("registry.download", Some(FILE_SUBJECT)).map_err(|error| {
                WireError::new(ErrorCode::InternalError).with_message(error.to_string())
            })?;

        let outcome = commit_or_replay::<DownloadResponse, _, _>(
            self.shared().receipts.as_ref(),
            &Immediate,
            ReplayRequest {
                workspace,
                scope,
                key: &replay.key,
                intent,
                receipt_participant: Participant::REGISTRY_IDEMPOTENCY,
                policy: RetryPolicy::PINNED,
                now,
            },
            || {
                let identity = replay_identity(self, replay, intent);
                let scope = scope.render();
                async move {
                    // Mutable authority is consulted only after the receipt
                    // reader elects this request to try the transaction. A
                    // replay therefore neither resolves a current name nor
                    // presigns a replacement URL.
                    let pointer = self
                        .shared()
                        .registry
                        .load_pointer(
                            workspace,
                            aex_content_domain::RegistryKind::File,
                            name.as_str(),
                        )
                        .await?
                        .ok_or_else(|| missing(Participant::REGISTRY_POINTER))?;
                    let registered = projection::registered_file(&pointer).map_err(|error| {
                        StoreError::Invalid {
                            detail: format!("a registered file pointer is malformed: {error}"),
                        }
                    })?;
                    let digest = registered.value.content.sha256;
                    let size = u64::try_from(registered.value.content.size_bytes.get())
                        .map_err(|_| missing(Participant::CONTENT_DESCRIPTOR))?;
                    let descriptor = self
                        .shared()
                        .content
                        .load_descriptor(workspace, &digest)
                        .await?
                        .ok_or_else(|| missing(Participant::CONTENT_DESCRIPTOR))?;
                    if descriptor.workspace != workspace
                        || descriptor.organization != organization
                        || descriptor.digest != digest
                        || descriptor.size_bytes != size
                        || descriptor.state != "committed"
                    {
                        return Err(missing(Participant::CONTENT_DESCRIPTOR));
                    }
                    let object = descriptor
                        .object
                        .as_ref()
                        .ok_or_else(|| missing(Participant::CONTENT_DESCRIPTOR))?;
                    let object_key = ObjectKey::parse(&object.key)
                        .map_err(|_| missing(Participant::CONTENT_DESCRIPTOR))?;
                    if object_key != ObjectKey::new(workspace, &digest) {
                        return Err(missing(Participant::CONTENT_DESCRIPTOR));
                    }
                    let domain_key = ContentObjectKey::parse(&object.key)
                        .map_err(|_| missing(Participant::CONTENT_DESCRIPTOR))?;
                    let checksum = if !object.checksum_crc64_nvme.is_empty() {
                        ObjectChecksum::Crc64Nvme(object.checksum_crc64_nvme.clone())
                    } else if !object.checksum_sha256.is_empty() {
                        ObjectChecksum::Sha256(object.checksum_sha256.clone())
                    } else {
                        return Err(missing(Participant::CONTENT_DESCRIPTOR));
                    };
                    let content = GrantContent {
                        digest,
                        size_bytes: size,
                        media_type: MediaType::parse(&descriptor.media_type).ok(),
                    };
                    let (grant, pin) = mint_grant(
                        GrantSubject {
                            workspace,
                            session: None,
                        },
                        &content,
                        &ContentObjectLocation {
                            key: domain_key,
                            checksum,
                        },
                        requested,
                        GrantId(new_uuid7()),
                        MeasurementId::from_uuid7(new_uuid7()),
                        now,
                    )
                    .map_err(invalid_grant)?;
                    let signed_range = presign_range(grant.range, size)?;
                    let presigned = self
                        .shared()
                        .content_objects
                        .presign_get(&object_key, signed_range)
                        .await
                        .map_err(object_failure)?;
                    validate_presign(&presigned, signed_range)?;
                    let response = models::DownloadGrant {
                        authorized_bytes: DecimalU128::new(u128::from(grant.authorized_bytes)),
                        expires_at: grant.expires_at,
                        measurement_id: grant.measurement,
                        range: wire_range,
                        sha256: grant.whole_sha256,
                        size_bytes: DecimalU128::new(u128::from(size)),
                        url: HttpsUrl::parse(presigned.url.expose()).map_err(|error| {
                            StoreError::Invalid {
                                detail: format!("a presigned URL is not a wire URL: {error}"),
                            }
                        })?,
                    };
                    let canonical =
                        serde_json::to_vec(&response).map_err(|error| StoreError::Invalid {
                            detail: format!("a download grant could not be serialized: {error}"),
                        })?;
                    let receipt = IdempotencyReceipt {
                        key: ReceiptKey::of(&scope, &identity).map_err(|error| {
                            StoreError::Invalid {
                                detail: format!("a download receipt key is invalid: {error}"),
                            }
                        })?,
                        identity,
                        intent,
                        outcome: ReceiptOutcome::Resource {
                            kind: ResourceKind::Grant,
                            id: ResourceId(grant.id.0.to_string()),
                            response: ResponseBody::of(&canonical),
                        },
                        created_at: now,
                        // The replay row and URL share one explicit fence. The
                        // compiler may replace the stale row after this instant
                        // even while DynamoDB TTL reclamation is still pending.
                        expires_at: Some(grant.expires_at),
                    };
                    let plan = SessionTransaction {
                        intent: TransactionIntent::RegistryDownload,
                        conditions: vec![
                            Condition::RegistryEtag {
                                selector: RegistrySelector {
                                    workspace,
                                    kind: aex_content_domain::RegistryKind::File,
                                    name: name.clone(),
                                },
                                expected: pointer.row.etag,
                            },
                            Condition::ContentOwned { workspace, digest },
                        ],
                        writes: vec![
                            Write::PutGrant(Box::new(grant)),
                            Write::PutPin(Box::new(pin)),
                            Write::PutIdempotencyReceipt(Box::new(receipt)),
                        ],
                        after_commit: Vec::new(),
                    };
                    let content = aex_content_dynamodb::application_plan::ContentAuthorityCompiler;
                    let registry =
                        aex_registry_dynamodb::application_plan::RegistryAuthorityCompiler;
                    let registry_receipts = IdempotencyCompiler::registry();
                    let compilers = FamilyCompilers::new()
                        .with(TableFamily::ContentAuthority, &content)
                        .with(TableFamily::Registry, &registry)
                        .with(TableFamily::Idempotency, &registry_receipts);
                    let committer = DynamoAuthorityCommitter::new_workspace(
                        self.shared().authority.clone(),
                        self.shared().tables.clone(),
                        WorkspaceBinding {
                            workspace,
                            organization,
                        },
                        now,
                        ApiHintSink,
                    );
                    retry_stale_authority(committer.commit_replayable(&plan, &compilers).await)?;
                    Ok(DownloadResponse(response))
                }
            },
        )
        .await
        .map_err(download_failure)?;
        Ok(outcome.into_inner().0)
    }
}

fn replay_identity(
    routes: &Routes,
    source: &aex_regional_http::idempotency::IdempotencyIdentity,
    intent: IntentDigest,
) -> IdempotencyIdentity {
    IdempotencyIdentity::Key(Box::new(ReplayIdentity {
        principal: routes.context().auth.principal,
        route: aex_wire::routes::RouteId::RegistryFilesDownloadCreate,
        key: source.key.clone(),
        intent,
    }))
}

fn domain_range(range: aex_wire::types::ByteRange) -> WireResult<ByteRange> {
    let start =
        u64::try_from(range.start()).map_err(|_| WireError::new(ErrorCode::InvalidRange))?;
    let end_inclusive = u64::try_from(range.end_inclusive())
        .map_err(|_| WireError::new(ErrorCode::InvalidRange))?;
    Ok(ByteRange {
        start,
        end_exclusive: end_inclusive
            .checked_add(1)
            .ok_or_else(|| WireError::new(ErrorCode::InvalidRange))?,
    })
}

fn presign_range(range: ByteRange, size: u64) -> Result<Option<(u64, u64)>, StoreError> {
    if size == 0 {
        return Ok(None);
    }
    let end = range
        .end_exclusive
        .checked_sub(1)
        .ok_or_else(|| StoreError::Invalid {
            detail: "a non-empty object grant has an empty range".to_owned(),
        })?;
    Ok(Some((range.start, end)))
}

fn validate_presign(
    signed: &aex_content_aws::object_store::PresignedGet,
    range: Option<(u64, u64)>,
) -> Result<(), StoreError> {
    if signed.expires_in_seconds != PRESIGN_EXPIRY.as_secs() {
        return Err(StoreError::Invalid {
            detail: "a presigned URL and its domain grant have different lifetimes".to_owned(),
        });
    }
    if let Some((start, end)) = range {
        let expected = format!("bytes={start}-{end}");
        if !signed
            .headers
            .iter()
            .any(|(name, value)| name.eq_ignore_ascii_case("range") && value == &expected)
        {
            return Err(StoreError::Invalid {
                detail: "the object presigner did not bind the authorized range".to_owned(),
            });
        }
    }
    Ok(())
}

fn new_uuid7() -> aex_wire::ids::Uuid7 {
    aex_wire::ids::Uuid7::from_bytes(*uuid::Uuid::now_v7().as_bytes())
        .unwrap_or_else(|_| unreachable!("`now_v7` produces a v7 payload"))
}

fn invalid_grant(_error: GrantRejection) -> StoreError {
    // The participant is a typed local handoff to `download_failure`; no
    // transaction was attempted when domain range validation rejects.
    missing(Participant::CONTENT_GRANT)
}

fn missing(participant: Participant) -> StoreError {
    StoreError::PreconditionFailed {
        participant,
        observed: None,
    }
}

fn retry_stale_authority(result: Result<(), StoreError>) -> Result<(), StoreError> {
    match result {
        // The pointer/descriptor changed after resolution, or an opaque grant
        // id collided with an immutable row. No URL has escaped; elect again
        // under the bounded policy and derive against fresh authority/ids.
        Err(StoreError::PreconditionFailed { participant, .. })
            if matches!(
                participant,
                Participant::REGISTRY_POINTER
                    | Participant::CONTENT_DESCRIPTOR
                    | Participant::CONTENT_GRANT
                    | Participant::CONTENT_GRANT_PIN
            ) =>
        {
            Err(StoreError::Contended)
        }
        other => other,
    }
}

fn object_failure(error: ContentObjectError) -> StoreError {
    StoreError::Invalid {
        detail: format!("the content object could not be presigned: {error}"),
    }
}

fn download_failure(error: StoreError) -> WireError {
    match error {
        StoreError::IdempotencyConflict => WireError::new(ErrorCode::IdempotencyConflict),
        StoreError::PreconditionFailed {
            participant: Participant::REGISTRY_POINTER,
            ..
        } => WireError::new(ErrorCode::NotFound),
        StoreError::PreconditionFailed {
            participant: Participant::CONTENT_DESCRIPTOR,
            ..
        } => WireError::new(ErrorCode::ContentMissing),
        StoreError::PreconditionFailed {
            participant: Participant::CONTENT_GRANT,
            ..
        } => WireError::new(ErrorCode::InvalidRange),
        other => authority_failure(&other),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use aex_session_dynamodb::error::StoreError;
    use aex_session_dynamodb::replay::{
        IdempotencyScope, Receipt, ReceiptBody, ReceiptStore, ReplayRequest, Replayable,
        commit_or_replay,
    };
    use aex_wire::idempotency::{IdempotencyKey, IntentDigest};
    use aex_wire::ids::{MeasurementId, Uuid7, WorkspaceId};
    use aex_wire::models;
    use aex_wire::types::{DecimalU128, HttpsUrl, Timestamp};

    use super::{DownloadResponse, Immediate, retry_stale_authority};

    #[derive(Clone)]
    struct OneReceipt(Option<Receipt>);

    #[async_trait::async_trait]
    impl ReceiptStore for OneReceipt {
        async fn read_receipt(
            &self,
            _workspace: WorkspaceId,
            _scope: &IdempotencyScope<'_>,
            _key: &IdempotencyKey,
            now: Timestamp,
        ) -> Result<Option<Receipt>, StoreError> {
            Ok(self
                .0
                .clone()
                .filter(|receipt| aex_session_dynamodb::replay::receipt_is_live(receipt, now)))
        }
    }

    fn id<T: aex_wire::ids::PrefixedId>(byte: u8) -> T {
        T::from_uuid7(Uuid7::compose(1_754_051_696_789, [byte; 10]))
    }

    fn now() -> Timestamp {
        Timestamp::from_unix_millis(0).expect("a timestamp")
    }

    fn response() -> DownloadResponse {
        DownloadResponse(models::DownloadGrant {
            authorized_bytes: DecimalU128::new(4),
            expires_at: Timestamp::from_unix_millis(300_000).expect("a timestamp"),
            measurement_id: id::<MeasurementId>(2),
            range: Some(aex_wire::types::ByteRange::new(2, 5).expect("a range")),
            sha256: aex_wire::ids::ContentHash::of(b"body"),
            size_bytes: DecimalU128::new(10),
            url: HttpsUrl::parse("https://signed.example/body?sig=one").expect("a URL"),
        })
    }

    fn receipt(intent: IntentDigest) -> Receipt {
        Receipt {
            scope: "registry.download:file".to_owned(),
            key_sha256: "a".repeat(64),
            intent,
            response_kind: "grant".to_owned(),
            response: ReceiptBody::Inline(
                serde_json::to_vec(&response().0).expect("the response serializes"),
            ),
            committed_at: now(),
            expires_at: Timestamp::from_unix_millis(300_000).expect("a timestamp"),
        }
    }

    fn request<'a>(
        workspace: WorkspaceId,
        key: &'a IdempotencyKey,
        intent: IntentDigest,
    ) -> ReplayRequest<'a> {
        ReplayRequest {
            workspace,
            scope: IdempotencyScope::new("registry.download", Some("file")).expect("a scope"),
            key,
            intent,
            receipt_participant: aex_session_dynamodb::plan::Participant::REGISTRY_IDEMPOTENCY,
            policy: aex_session_dynamodb::RetryPolicy::PINNED,
            now: now(),
        }
    }

    #[tokio::test]
    async fn a_live_replay_survives_pointer_mutation_or_deletion_without_resolution_or_presign() {
        let workspace = id::<WorkspaceId>(1);
        let key = IdempotencyKey::parse("download-1").expect("a key");
        let intent = IntentDigest::from_bytes([3; 32]);
        for current_pointer in ["mutated", "deleted"] {
            let resolutions = AtomicUsize::new(0);
            let signing = AtomicUsize::new(0);
            let replayed = commit_or_replay::<DownloadResponse, _, _>(
                &OneReceipt(Some(receipt(intent))),
                &Immediate,
                request(workspace, &key, intent),
                || async {
                    // The entire mutable pointer/descriptor resolver lives in
                    // this elected closure in production. Reaching it would
                    // observe the simulated current mutation/deletion and
                    // incorrectly replace the still-live winner.
                    resolutions.fetch_add(1, Ordering::SeqCst);
                    signing.fetch_add(1, Ordering::SeqCst);
                    Ok(response())
                },
            )
            .await
            .expect("the stored winner replays");

            assert!(matches!(replayed, Replayable::Replayed(_)));
            assert_eq!(resolutions.load(Ordering::SeqCst), 0, "{current_pointer}");
            assert_eq!(signing.load(Ordering::SeqCst), 0, "{current_pointer}");
            assert_eq!(
                serde_json::to_vec(&replayed.into_inner().0).expect("serializes"),
                serde_json::to_vec(&response().0).expect("serializes"),
                "{current_pointer}"
            );
        }
    }

    #[tokio::test]
    async fn a_body_bound_intent_mismatch_conflicts_before_presigning() {
        let workspace = id::<WorkspaceId>(1);
        let key = IdempotencyKey::parse("download-1").expect("a key");
        let signing = AtomicUsize::new(0);
        let error = commit_or_replay::<DownloadResponse, _, _>(
            &OneReceipt(Some(receipt(IntentDigest::from_bytes([3; 32])))),
            &Immediate,
            request(workspace, &key, IntentDigest::from_bytes([4; 32])),
            || async {
                signing.fetch_add(1, Ordering::SeqCst);
                Ok(response())
            },
        )
        .await
        .expect_err("a changed resolved body conflicts");

        assert!(matches!(error, StoreError::IdempotencyConflict));
        assert_eq!(signing.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_rejected_atomic_commit_exposes_no_presigned_response() {
        let workspace = id::<WorkspaceId>(1);
        let key = IdempotencyKey::parse("download-1").expect("a key");
        let signing = AtomicUsize::new(0);
        let result = commit_or_replay::<DownloadResponse, _, _>(
            &OneReceipt(None),
            &Immediate,
            request(workspace, &key, IntentDigest::from_bytes([3; 32])),
            || async {
                // This represents the local presign performed inside the
                // elected closure. The response is constructed but cannot
                // escape after any non-receipt participant rejects the atomic
                // transaction.
                signing.fetch_add(1, Ordering::SeqCst);
                let _not_exposed = response();
                Err(StoreError::PreconditionFailed {
                    participant: aex_session_dynamodb::plan::Participant::CONTENT_GRANT,
                    observed: None,
                })
            },
        )
        .await;

        assert!(matches!(
            result,
            Err(StoreError::PreconditionFailed {
                participant: aex_session_dynamodb::plan::Participant::CONTENT_GRANT,
                ..
            })
        ));
        assert_eq!(signing.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn an_expired_url_receipt_is_not_replayed() {
        let workspace = id::<WorkspaceId>(1);
        let key = IdempotencyKey::parse("download-1").expect("a key");
        let intent = IntentDigest::from_bytes([3; 32]);
        let mut expired = receipt(intent);
        expired.expires_at = Timestamp::from_unix_millis(0).expect("a timestamp");
        let signing = AtomicUsize::new(0);
        let committed = commit_or_replay::<DownloadResponse, _, _>(
            &OneReceipt(Some(expired)),
            &Immediate,
            request(workspace, &key, intent),
            || async {
                signing.fetch_add(1, Ordering::SeqCst);
                Ok(response())
            },
        )
        .await
        .expect("an expired receipt elects a fresh attempt");

        assert!(matches!(committed, Replayable::Committed(_)));
        assert_eq!(signing.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn an_immutable_grant_or_pin_collision_reenters_the_bounded_election() {
        for participant in [
            aex_session_dynamodb::plan::Participant::CONTENT_GRANT,
            aex_session_dynamodb::plan::Participant::CONTENT_GRANT_PIN,
        ] {
            let classified = retry_stale_authority(Err(StoreError::PreconditionFailed {
                participant,
                observed: Some(Box::new(std::collections::HashMap::new())),
            }));
            assert_eq!(classified, Err(StoreError::Contended));
        }
    }
}
