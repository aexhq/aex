//! Pending-upload expiry: the `HeadObject` oracle and the S3-first ordering.
//!
//! This is the first consumer of the `regional-work` due kind
//! `"registry.upload_expiry"`, which was declared and unused. Before it existed
//! there was no pending-upload expiry at all, because the durable row named
//! neither the provider multipart upload nor the object key.
//!
//! Two rules shape every branch here.
//!
//! **`HeadObject` on the durable object key is the sole oracle** (E D-3). It is
//! total, not probabilistic: the key is content-addressed and every write to it is
//! conditional, so "an object exists at this key with this digest and this length"
//! is exactly equivalent to "these bytes are committed for this workspace".
//!
//! **The irreversible provider effect goes first, and the write that records it is
//! conditional on the exact state and handle the decision was made under**
//! (E D-6). A committed `Aborted` row followed by a failed abort would leave a
//! multipart upload nothing but the bucket lifecycle rule could ever reclaim, and
//! would leave the durable state lying about the provider.
//!
//! This role holds **no object-delete verb** (E D-4). The worst outcome of any bug
//! here is an orphan object the content GC reclaims after its grace, at storage
//! cost — not a destroyed committed body.

use aex_session_dynamodb::error::StoreError;
use aex_wire::ids::{UploadId, WorkspaceId};
use aex_wire::types::Timestamp;
use aex_workspace_domain::upload::{
    AmbiguityResolution, ExpiryOutcome, HeadOracle, Upload, UploadState, expire, resolve_completing,
};
use async_trait::async_trait;

/// The narrow object operations this role owns.
///
/// There is no delete verb, and there is nowhere in this trait to add one without
/// the reviewer seeing it.
#[async_trait]
pub trait UploadObjects: Send + Sync {
    /// Heads the upload's exact durable object key.
    ///
    /// Never fails: a failure is [`HeadOracle::Unavailable`], which is evidence of
    /// nothing and re-arms the due item rather than deciding anything.
    async fn head(&self, upload: &Upload) -> HeadOracle;

    /// Aborts the exact multipart upload the row names.
    ///
    /// # Errors
    ///
    /// Any provider failure, as text. `NoSuchUpload` is an idempotent success at
    /// the adapter and never reaches here.
    async fn abort_multipart(&self, upload: &Upload) -> Result<(), String>;
}

/// The narrow registry operations this role owns.
#[async_trait]
pub trait UploadRows: Send + Sync {
    /// Reads one upload with every spilled part block.
    async fn load(
        &self,
        workspace: WorkspaceId,
        upload: UploadId,
    ) -> Result<Option<Upload>, StoreError>;

    /// Applies a transition fenced on the state and the provider handle.
    async fn transition_fenced(
        &self,
        upload: &Upload,
        from: UploadState,
        to: UploadState,
    ) -> Result<(), StoreError>;

    /// Settles an upload as `Ready` with the evidence that proves its object.
    async fn settle_ready(&self, upload: &Upload) -> Result<(), StoreError>;

    /// Removes a row that has no remaining consumer.
    async fn delete_row(&self, upload: &Upload) -> Result<(), StoreError>;
}

/// What one due upload's sweep did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SweepOutcome {
    /// The row was gone before the sweep reached it.
    Absent,
    /// The row is not due, or nothing would change.
    Unchanged,
    /// The object turned out to exist, so the upload was settled `Ready`.
    ///
    /// Never aborted and never deleted: the bytes the caller asked for are
    /// committed, and that is the upload's entire purpose.
    Committed,
    /// The provider upload was aborted and the row lapsed.
    Expired,
    /// The row had no remaining consumer and was removed.
    Deleted,
    /// An object exists whose digest or length disagrees with the declaration.
    ///
    /// A permanent stall by design. The row stays visible and unswept rather than
    /// being cleaned up wrongly; this is impossible under content addressing and
    /// must be investigated.
    Integrity {
        /// What disagreed.
        detail: String,
    },
    /// Nothing could be established. Re-arm with backoff.
    Retry {
        /// Why.
        detail: String,
    },
}

impl SweepOutcome {
    /// Whether the due item should be re-armed rather than retired.
    #[must_use]
    pub const fn re_arms(&self) -> bool {
        matches!(self, Self::Integrity { .. } | Self::Retry { .. })
    }
}

/// Sweeps one due upload.
///
/// # Errors
///
/// [`StoreError`] from the durable reads and writes. A provider failure is an
/// outcome, not an error: it re-arms.
pub async fn sweep_one<R: UploadRows + ?Sized, O: UploadObjects + ?Sized>(
    rows: &R,
    objects: &O,
    workspace: WorkspaceId,
    upload_id: UploadId,
    now: Timestamp,
) -> Result<SweepOutcome, StoreError> {
    let Some(upload) = rows.load(workspace, upload_id).await? else {
        return Ok(SweepOutcome::Absent);
    };

    match expire(&upload, now) {
        ExpiryOutcome::Unchanged => Ok(SweepOutcome::Unchanged),
        ExpiryOutcome::DeleteRow => {
            rows.delete_row(&upload).await?;
            Ok(SweepOutcome::Deleted)
        }
        ExpiryOutcome::Expire(commit) => {
            // The row is due and pre-completion. Nothing is decided until the
            // oracle answers.
            let oracle = objects.head(&upload).await;
            match resolve_completing(&upload, &oracle, now) {
                AmbiguityResolution::AlreadySettled => Ok(SweepOutcome::Unchanged),
                AmbiguityResolution::Retry => Ok(SweepOutcome::Retry {
                    detail: "the head could not be taken".to_owned(),
                }),
                AmbiguityResolution::Integrity { detail } => Ok(SweepOutcome::Integrity { detail }),
                AmbiguityResolution::Committed(settled) => {
                    // The bytes are committed. Never abort, never delete.
                    rows.settle_ready(&settled.upload).await?;
                    Ok(SweepOutcome::Committed)
                }
                AmbiguityResolution::NotCommitted => {
                    // Only now is the abort provably safe. S3 first, then the
                    // conditional write that records it.
                    if let Err(detail) = objects.abort_multipart(&upload).await {
                        return Ok(SweepOutcome::Retry { detail });
                    }
                    rows.transition_fenced(&upload, upload.state, commit.upload.state)
                        .await?;
                    Ok(SweepOutcome::Expired)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use aex_wire::ids::ContentHash as ContentDigest;
    use aex_wire::ids::{PrefixedId as _, UploadId, Uuid7, WorkspaceId};
    use aex_wire::types::Timestamp;
    use aex_workspace_domain::upload::{
        CompletionEvidence, PartGrantRequest, PartPlan, PlannedPart, Upload, UploadState,
        grant_parts,
    };

    use super::{HeadOracle, StoreError, SweepOutcome, UploadObjects, UploadRows, sweep_one};

    fn moment(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("in range")
    }

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1, [2; 10]))
    }

    fn upload_id() -> UploadId {
        UploadId::from_uuid7(Uuid7::compose(1, [1; 10]))
    }

    fn evidence() -> CompletionEvidence {
        CompletionEvidence {
            etag: "\"assembled\"".to_owned(),
            checksum_sha256: None,
            checksum_crc64_nvme: None,
            part_count: Some(1),
        }
    }

    fn upload(state: UploadState) -> Upload {
        let staged = Upload {
            id: upload_id(),
            workspace: workspace(),
            state: UploadState::Created,
            provider_upload_id: "provider-mpu-1".to_owned(),
            object_key: "wks/ab/cd/abcd".to_owned(),
            declared_size: 1_024,
            declared_sha256: ContentDigest::of(b"body"),
            content_type: None,
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
            created_at: moment(0),
            expires_at: moment(86_400_000),
        };
        let mut granted = grant_parts(
            &staged,
            &[PartGrantRequest {
                number: 1,
                sha256: ContentDigest::of(b"part one"),
                size_bytes: 1_024,
            }],
            moment(1),
        )
        .expect("grants")
        .upload;
        granted.state = state;
        if state == UploadState::Ready {
            granted.completion = Some(evidence());
        }
        granted
    }

    #[derive(Default)]
    struct Recorder {
        row: Option<Upload>,
        writes: Vec<String>,
    }

    struct Rows(Mutex<Recorder>);

    #[async_trait::async_trait]
    impl UploadRows for Rows {
        async fn load(
            &self,
            _workspace: WorkspaceId,
            _upload: UploadId,
        ) -> Result<Option<Upload>, StoreError> {
            Ok(self.0.lock().expect("not poisoned").row.clone())
        }

        async fn transition_fenced(
            &self,
            _upload: &Upload,
            from: UploadState,
            to: UploadState,
        ) -> Result<(), StoreError> {
            self.0
                .lock()
                .expect("not poisoned")
                .writes
                .push(format!("transition {from:?}->{to:?}"));
            Ok(())
        }

        async fn settle_ready(&self, _upload: &Upload) -> Result<(), StoreError> {
            self.0
                .lock()
                .expect("not poisoned")
                .writes
                .push("settle_ready".to_owned());
            Ok(())
        }

        async fn delete_row(&self, _upload: &Upload) -> Result<(), StoreError> {
            self.0
                .lock()
                .expect("not poisoned")
                .writes
                .push("delete_row".to_owned());
            Ok(())
        }
    }

    struct Objects {
        oracle: HeadOracle,
        aborts: Mutex<Vec<String>>,
        abort_fails: bool,
    }

    #[async_trait::async_trait]
    impl UploadObjects for Objects {
        async fn head(&self, _upload: &Upload) -> HeadOracle {
            self.oracle.clone()
        }

        async fn abort_multipart(&self, upload: &Upload) -> Result<(), String> {
            self.aborts
                .lock()
                .expect("not poisoned")
                .push(upload.provider_upload_id.clone());
            if self.abort_fails {
                return Err("the provider refused".to_owned());
            }
            Ok(())
        }
    }

    fn objects(oracle: HeadOracle) -> Objects {
        Objects {
            oracle,
            aborts: Mutex::new(Vec::new()),
            abort_fails: false,
        }
    }

    async fn sweep(row: Option<Upload>, objects: &Objects, at: i64) -> (SweepOutcome, Recorder) {
        let rows = Rows(Mutex::new(Recorder {
            row,
            writes: Vec::new(),
        }));
        let outcome = sweep_one(&rows, objects, workspace(), upload_id(), moment(at))
            .await
            .expect("sweeps");
        let recorded = std::mem::take(&mut *rows.0.lock().expect("not poisoned"));
        (outcome, recorded)
    }

    #[tokio::test]
    async fn an_object_that_exists_settles_ready_and_is_never_aborted() {
        let store = objects(HeadOracle::Present {
            content_length: 1_024,
            declared_digest: Some(ContentDigest::of(b"body")),
            evidence: evidence(),
        });
        let (outcome, recorded) = sweep(
            Some(upload(UploadState::Completing)),
            &store,
            86_400_000,
        )
        .await;
        assert_eq!(outcome, SweepOutcome::Committed);
        assert_eq!(recorded.writes, vec!["settle_ready".to_owned()]);
        assert!(
            store.aborts.lock().expect("not poisoned").is_empty(),
            "aborting an upload that completed destroys a committed body"
        );
    }

    #[tokio::test]
    async fn an_absent_object_is_aborted_at_the_provider_before_the_row_moves() {
        let store = objects(HeadOracle::Absent);
        let (outcome, recorded) = sweep(
            Some(upload(UploadState::Completing)),
            &store,
            86_400_000,
        )
        .await;
        assert_eq!(outcome, SweepOutcome::Expired);
        assert_eq!(
            store.aborts.lock().expect("not poisoned").as_slice(),
            ["provider-mpu-1".to_owned()]
        );
        assert_eq!(
            recorded.writes,
            vec!["transition Completing->Expired".to_owned()]
        );
    }

    #[tokio::test]
    async fn a_failed_abort_writes_nothing_and_re_arms() {
        let mut store = objects(HeadOracle::Absent);
        store.abort_fails = true;
        let (outcome, recorded) = sweep(
            Some(upload(UploadState::PartsGranted)),
            &store,
            86_400_000,
        )
        .await;
        assert!(matches!(outcome, SweepOutcome::Retry { .. }));
        assert!(outcome.re_arms());
        assert!(
            recorded.writes.is_empty(),
            "a committed Aborted row after a failed abort leaves the state lying \
             about the provider"
        );
    }

    #[tokio::test]
    async fn a_disagreeing_object_stalls_loudly_rather_than_being_swept() {
        let store = objects(HeadOracle::Present {
            content_length: 999,
            declared_digest: Some(ContentDigest::of(b"body")),
            evidence: evidence(),
        });
        let (outcome, recorded) = sweep(
            Some(upload(UploadState::Completing)),
            &store,
            86_400_000,
        )
        .await;
        assert!(matches!(outcome, SweepOutcome::Integrity { .. }));
        assert!(outcome.re_arms());
        assert!(recorded.writes.is_empty());
        assert!(store.aborts.lock().expect("not poisoned").is_empty());
    }

    #[tokio::test]
    async fn an_unavailable_head_decides_nothing() {
        let store = objects(HeadOracle::Unavailable);
        let (outcome, recorded) = sweep(
            Some(upload(UploadState::Completing)),
            &store,
            86_400_000,
        )
        .await;
        assert!(matches!(outcome, SweepOutcome::Retry { .. }));
        assert!(recorded.writes.is_empty());
        assert!(store.aborts.lock().expect("not poisoned").is_empty());
    }

    #[tokio::test]
    async fn a_ready_row_past_its_grace_is_reclaimed_without_touching_the_provider() {
        let store = objects(HeadOracle::Absent);
        let (outcome, recorded) =
            sweep(Some(upload(UploadState::Ready)), &store, 86_400_000).await;
        assert_eq!(outcome, SweepOutcome::Deleted);
        assert_eq!(recorded.writes, vec!["delete_row".to_owned()]);
        assert!(store.aborts.lock().expect("not poisoned").is_empty());
    }

    #[tokio::test]
    async fn a_row_that_is_not_due_yet_writes_nothing() {
        let store = objects(HeadOracle::Absent);
        let (outcome, recorded) = sweep(
            Some(upload(UploadState::PartsGranted)),
            &store,
            86_399_999,
        )
        .await;
        assert_eq!(outcome, SweepOutcome::Unchanged);
        assert!(recorded.writes.is_empty());
    }

    #[tokio::test]
    async fn a_row_that_is_already_gone_is_an_idempotent_success() {
        let store = objects(HeadOracle::Absent);
        let (outcome, _) = sweep(None, &store, 86_400_000).await;
        assert_eq!(outcome, SweepOutcome::Absent);
    }
}
