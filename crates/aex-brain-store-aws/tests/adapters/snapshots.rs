//! Production fold-snapshot adapter boundaries.

use std::sync::{Arc, Mutex};

use aex_brain_app::ports::{FoldSnapshotStore, SnapshotPublishOutcome, StoreError};
use aex_brain_domain::snapshot::{FoldSnapshotArtifact, JournalPoint, SnapshotReplay};
use aex_brain_store_aws::{AwsFoldSnapshotStore, SnapshotBodyStore, SnapshotContentContext};
use aex_content_aws::ContentObjectError;
use aex_wire::ids::{ContentHash, WorkspaceId};
use async_trait::async_trait;

type RecordedWrite = (WorkspaceId, ContentHash, Vec<u8>, Vec<u8>);

#[derive(Debug, Default)]
struct RecordingBodies {
    reads: Mutex<Vec<(WorkspaceId, ContentHash, u64)>>,
    writes: Mutex<Vec<RecordedWrite>>,
    body: Mutex<Vec<u8>>,
}

impl RecordingBodies {
    fn returning(body: Vec<u8>) -> Self {
        Self {
            body: Mutex::new(body),
            ..Self::default()
        }
    }
}

#[async_trait]
impl SnapshotBodyStore for RecordingBodies {
    async fn publish_immutable(
        &self,
        workspace: WorkspaceId,
        digest: ContentHash,
        body: Vec<u8>,
        encryption_context: Vec<u8>,
    ) -> Result<(), ContentObjectError> {
        self.writes.lock().expect("writes lock").push((
            workspace,
            digest,
            body,
            encryption_context,
        ));
        Ok(())
    }

    async fn read_bounded(
        &self,
        workspace: WorkspaceId,
        digest: ContentHash,
        max_bytes: u64,
    ) -> Result<Vec<u8>, ContentObjectError> {
        self.reads
            .lock()
            .expect("reads lock")
            .push((workspace, digest, max_bytes));
        Ok(self.body.lock().expect("body lock").clone())
    }
}

fn artifact() -> FoldSnapshotArtifact {
    let history = aex_brain_test_support::journal_gen::HistoryBuilder::new()
        .push(aex_brain_test_support::journal_gen::started(
            aex_brain_test_support::journal_gen::grant(100),
        ))
        .push(aex_brain_test_support::journal_gen::user_text(
            "snapshot me",
        ))
        .build();
    let mut replay = SnapshotReplay::from_sequence_zero();
    for entry in &history {
        replay.apply(entry).expect("fixture folds");
    }
    let tail = history.last().expect("history has a tail");
    replay
        .finish_exact(
            super::key(),
            JournalPoint {
                seq: tail.envelope.seq,
                hash: tail.envelope.content_hash,
            },
        )
        .expect("fixture snapshots")
}

fn context() -> SnapshotContentContext {
    SnapshotContentContext::new("dev", "eu-west-1").expect("valid deployment")
}

#[tokio::test]
async fn selected_pointer_is_read_strongly_and_decoded_exactly() {
    let artifact = artifact();
    let item =
        aex_brain_store_aws::snapshot::encode_pointer(artifact.pointer()).expect("pointer encodes");
    let (brain, replay) = super::replaying(vec![serde_json::json!({
        "Item": super::dynamo_item(&item),
    })]);
    let bodies = Arc::new(RecordingBodies::default());
    let snapshots = AwsFoldSnapshotStore::with_body_store(
        brain.client().clone(),
        brain.tables().clone(),
        bodies,
        context(),
    );

    let selected = snapshots
        .load_latest(&super::key())
        .await
        .expect("strong pointer read succeeds");
    assert_eq!(selected.as_ref(), Some(artifact.pointer()));
    let requests = super::captured_requests(&replay);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["ConsistentRead"], true);
}

#[tokio::test]
async fn body_bound_is_refused_before_the_content_adapter_can_allocate() {
    let artifact = artifact();
    let mut pointer = artifact.pointer().clone();
    pointer.body_bytes = 1_025;
    let (brain, _replay) = super::replaying(Vec::new());
    let bodies = Arc::new(RecordingBodies::default());
    let snapshots = AwsFoldSnapshotStore::with_body_store(
        brain.client().clone(),
        brain.tables().clone(),
        bodies.clone(),
        context(),
    );

    assert_eq!(
        snapshots
            .load_body(super::authority().workspace, &pointer, 1_024)
            .await,
        Err(StoreError::SnapshotBodyTooLarge {
            declared: 1_025,
            max: 1_024,
        })
    );
    assert!(
        bodies.reads.lock().expect("reads lock").is_empty(),
        "the declared pointer length is checked before any provider read"
    );
}

#[tokio::test]
async fn body_read_uses_only_the_request_workspace_and_exact_pointer_bound() {
    let artifact = artifact();
    let (brain, _replay) = super::replaying(Vec::new());
    let bodies = Arc::new(RecordingBodies::returning(artifact.body().to_vec()));
    let snapshots = AwsFoldSnapshotStore::with_body_store(
        brain.client().clone(),
        brain.tables().clone(),
        bodies.clone(),
        context(),
    );
    let workspace = super::authority().workspace;

    let body = snapshots
        .load_body(workspace, artifact.pointer(), artifact.body().len())
        .await
        .expect("bounded content read succeeds");
    assert_eq!(body, artifact.body());
    assert_eq!(
        *bodies.reads.lock().expect("reads lock"),
        vec![(
            workspace,
            artifact.pointer().body_digest,
            artifact.pointer().body_bytes,
        )]
    );
}

#[tokio::test]
async fn immutable_body_is_durable_before_the_monotonic_pointer_transaction() {
    let artifact = artifact();
    let (brain, replay) = super::replaying(vec![serde_json::json!({}), serde_json::json!({})]);
    let bodies = Arc::new(RecordingBodies::default());
    let snapshots = AwsFoldSnapshotStore::with_body_store(
        brain.client().clone(),
        brain.tables().clone(),
        bodies.clone(),
        context(),
    );
    let workspace = super::authority().workspace;

    assert_eq!(
        snapshots.publish(workspace, &artifact).await,
        Ok(SnapshotPublishOutcome::Published)
    );
    let writes = bodies.writes.lock().expect("writes lock");
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].0, workspace);
    assert_eq!(writes[0].1, artifact.pointer().body_digest);
    assert_eq!(writes[0].2, artifact.body());
    let context: serde_json::Value =
        serde_json::from_slice(&writes[0].3).expect("context is canonical JSON");
    assert_eq!(context["aex:workspace"], workspace.to_string());
    assert_eq!(context["aex:domain"], "brain-fold-snapshot");
    drop(writes);

    let requests = super::captured_requests(&replay);
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["ConsistentRead"], true);
    let actions = requests[1]["TransactItems"]
        .as_array()
        .expect("pointer publication is a transaction");
    assert_eq!(actions.len(), 3);
    assert_eq!(
        actions[2]["Update"]["ConditionExpression"],
        "attribute_not_exists(pk)"
    );
}

#[tokio::test]
async fn a_late_publisher_neither_uploads_or_rolls_back_a_newer_pointer() {
    let artifact = artifact();
    let mut newer = artifact.pointer().clone();
    newer.absorbed.seq = newer.absorbed.seq.next();
    let item = aex_brain_store_aws::snapshot::encode_pointer(&newer).expect("pointer encodes");
    let (brain, replay) = super::replaying(vec![serde_json::json!({
        "Item": super::dynamo_item(&item),
    })]);
    let bodies = Arc::new(RecordingBodies::default());
    let snapshots = AwsFoldSnapshotStore::with_body_store(
        brain.client().clone(),
        brain.tables().clone(),
        bodies.clone(),
        context(),
    );

    assert_eq!(
        snapshots
            .publish(super::authority().workspace, &artifact)
            .await,
        Ok(SnapshotPublishOutcome::Superseded {
            current: newer.absorbed,
        })
    );
    assert!(bodies.writes.lock().expect("writes lock").is_empty());
    assert_eq!(
        super::captured_requests(&replay).len(),
        1,
        "a strongly observed newer pointer ends the attempt before S3 or a transaction"
    );
}

#[tokio::test]
async fn two_distinct_bodies_at_one_snapshot_cut_are_a_hard_conflict() {
    let artifact = artifact();
    let mut conflicting = artifact.pointer().clone();
    conflicting.body_digest = ContentHash::of(b"different derived state");
    let item = aex_brain_store_aws::snapshot::encode_pointer(&conflicting)
        .expect("conflicting pointer encodes");
    let (brain, _replay) = super::replaying(vec![serde_json::json!({
        "Item": super::dynamo_item(&item),
    })]);
    let bodies = Arc::new(RecordingBodies::default());
    let snapshots = AwsFoldSnapshotStore::with_body_store(
        brain.client().clone(),
        brain.tables().clone(),
        bodies.clone(),
        context(),
    );

    assert_eq!(
        snapshots
            .publish(super::authority().workspace, &artifact)
            .await,
        Err(StoreError::SnapshotPointerConflict {
            seq: artifact.pointer().absorbed.seq,
        })
    );
    assert!(bodies.writes.lock().expect("writes lock").is_empty());
}
