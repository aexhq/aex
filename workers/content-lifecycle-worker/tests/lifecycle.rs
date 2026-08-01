//! Orphan grace, mark/sweep and exact fenced deletion requirements.

use content_lifecycle_worker::{
    ContentItem, ContentState, DeleteOutcome, DeletionDenial, GRACE_MILLIS, Mode, ModeError,
    ObjectDeleteResult, ReconcileOutcome, admit_mode, apply_delete_result, plan_delete,
    reconcile_staged,
};

fn staged() -> ContentItem {
    ContentItem::staged(
        "content-1",
        "workspace/content/content-1",
        "etag-1",
        1_000,
        7,
    )
    .expect("item")
}

#[test]
fn staged_orphan_grace_is_exact_and_every_pin_blocks_deletion() {
    assert_eq!(
        reconcile_staged(&staged(), 1_000 + GRACE_MILLIS - 1),
        ReconcileOutcome::KeepStaged
    );
    assert_eq!(
        reconcile_staged(&staged(), 1_000 + GRACE_MILLIS),
        ReconcileOutcome::ConfirmOrphan
    );
    let mut pinned = staged();
    pinned.grant_pins = 1;
    assert_eq!(
        reconcile_staged(&pinned, 1_000 + GRACE_MILLIS),
        ReconcileOutcome::RestoreLive
    );
    let mut owned = staged();
    owned.owner_edges = 1;
    assert_eq!(
        reconcile_staged(&owned, 1_000 + GRACE_MILLIS),
        ReconcileOutcome::RestoreLive
    );
}

#[test]
fn delete_rechecks_denial_and_produces_the_exact_conditional_request() {
    let mut item = staged();
    item.state = ContentState::OrphanConfirmed;
    assert_eq!(
        plan_delete(
            &item,
            11,
            DeletionDenial::PendingProjection,
            1_000 + GRACE_MILLIS,
            "522921482290"
        ),
        Ok(DeleteOutcome::Deferred)
    );
    assert_eq!(
        plan_delete(
            &item,
            11,
            DeletionDenial::LiveClosure,
            1_000 + GRACE_MILLIS,
            "522921482290"
        ),
        Ok(DeleteOutcome::RestoreLive)
    );
    let DeleteOutcome::Delete(intent) = plan_delete(
        &item,
        11,
        DeletionDenial::PurgedClosure,
        1_000 + GRACE_MILLIS,
        "522921482290",
    )
    .expect("decision") else {
        panic!("expected delete intent")
    };
    assert_eq!(intent.key, "workspace/content/content-1");
    assert_eq!(intent.if_match, "etag-1");
    assert_eq!(intent.fence, 11);
    assert!(!intent.expected_bucket_owner.is_empty());
}

#[test]
fn etag_mismatch_never_deletes_and_missing_object_is_success() {
    let mut item = staged();
    item.state = ContentState::Deleting { fence: 12 };
    assert_eq!(
        apply_delete_result(&mut item, 12, ObjectDeleteResult::PreconditionFailed),
        Ok(DeleteOutcome::Deferred)
    );
    assert_ne!(item.state, ContentState::Deleted { fence: 12 });
    assert_eq!(
        apply_delete_result(&mut item, 12, ObjectDeleteResult::Missing),
        Ok(DeleteOutcome::Deleted)
    );
    assert_eq!(item.state, ContentState::Deleted { fence: 12 });
}

#[test]
fn mode_admission_gives_delete_capability_only_to_delete_mode() {
    assert_eq!(
        admit_mode(Mode::Delete, false),
        Err(ModeError::DeleteCapabilityMissing)
    );
    assert!(
        admit_mode(Mode::Delete, true)
            .expect("delete")
            .may_delete_objects
    );
    assert!(
        !admit_mode(Mode::Reconcile, false)
            .expect("reconcile")
            .may_delete_objects
    );
    assert_eq!(
        admit_mode(Mode::Reconcile, true),
        Err(ModeError::UnexpectedDeleteCapability)
    );
}
