//! Crash, duplicate, fence, due-scan and poison requirements for continuations.

use session_operation_worker::{
    BatchItem, ClaimError, CommitError, DeletionDenial, MAX_ATTEMPTS, WorkKind, WorkRecord,
    WorkStatus, batch_response, claim, commit_step, due_shard, may_commit_tombstone, plan_step,
    record_failure,
};

fn record(kind: WorkKind) -> WorkRecord {
    WorkRecord::new("work-1", "op-1", kind, 3).expect("record")
}

#[test]
fn only_purge_and_paged_persist_fork_are_continuations() {
    for accepted in [
        "session_delete",
        "workspace_delete",
        "session_persist_page",
        "session_fork_page",
    ] {
        assert!(WorkKind::parse(accepted).is_ok(), "{accepted}");
    }
    for inline_or_peer_owned in [
        "session_stop",
        "session_persist",
        "session_fork",
        "workspace_discard",
        "credential_rebind",
    ] {
        assert!(
            WorkKind::parse(inline_or_peer_owned).is_err(),
            "{inline_or_peer_owned}"
        );
    }
}

#[test]
fn effect_identity_is_stable_and_a_stale_fence_never_commits() {
    let mut work = record(WorkKind::SessionPersistPage);
    let first = claim(&mut work, "worker-a", 1_000, 60_000).expect("claim");
    let plan = plan_step(&work, &first).expect("plan");
    assert_eq!(plan, plan_step(&work, &first).expect("same plan"));
    let mut taken_over = work.clone();
    taken_over.lease_expires_at_ms = 999;
    let second = claim(&mut taken_over, "worker-b", 1_000, 60_000).expect("takeover");
    assert!(second.fence > first.fence);
    assert_eq!(
        commit_step(&mut taken_over, &first, &plan.effect_id),
        Err(CommitError::StaleFence)
    );
    let takeover_plan = plan_step(&taken_over, &second).expect("plan");
    commit_step(&mut taken_over, &second, &takeover_plan.effect_id).expect("current owner commits");
    assert_eq!(taken_over.cursor, 1);
}

#[test]
fn a_duplicate_hint_after_commit_observes_the_advanced_cursor() {
    let mut work = record(WorkKind::SessionForkPage);
    let claim = claim(&mut work, "worker", 0, 60_000).expect("claim");
    let plan = plan_step(&work, &claim).expect("plan");
    commit_step(&mut work, &claim, &plan.effect_id).expect("commit");
    assert_eq!(
        commit_step(&mut work, &claim, &plan.effect_id),
        Err(CommitError::AlreadyAdvanced)
    );
}

#[test]
fn poison_terminalizes_at_exactly_max_attempts_and_partial_batch_is_per_item() {
    let mut work = record(WorkKind::SessionDelete);
    for attempt in 1..MAX_ATTEMPTS {
        record_failure(&mut work, "typed_kind").expect("retryable before ceiling");
        assert_eq!(work.attempts, attempt);
        assert_eq!(work.status, WorkStatus::Due);
    }
    record_failure(&mut work, "typed_kind").expect("terminal at ceiling");
    assert_eq!(work.attempts, MAX_ATTEMPTS);
    assert_eq!(work.status, WorkStatus::ManualReview);
    assert_eq!(work.error_kind.as_deref(), Some("operation_manual_review"));
    let response = batch_response(&[
        BatchItem::Succeeded("message-a".into()),
        BatchItem::Failed("message-b".into()),
        BatchItem::Succeeded("message-c".into()),
    ]);
    assert_eq!(response.batch_item_failures, vec!["message-b"]);
}

#[test]
fn due_shards_are_deterministic_and_claims_obey_live_leases() {
    assert_eq!(
        due_shard("work-1", 16).expect("shard"),
        due_shard("work-1", 16).expect("shard")
    );
    assert!(due_shard("work-1", 0).is_err());
    let mut work = record(WorkKind::SessionPersistPage);
    claim(&mut work, "worker-a", 1_000, 60_000).expect("claim");
    assert_eq!(
        claim(&mut work, "worker-b", 1_001, 60_000),
        Err(ClaimError::LeaseHeld)
    );
}

#[test]
fn tombstone_requires_the_durable_projected_deletion_denial() {
    assert!(!may_commit_tombstone(DeletionDenial::Missing));
    assert!(!may_commit_tombstone(DeletionDenial::DurableNotProjected));
    assert!(may_commit_tombstone(DeletionDenial::DurableAndProjected));
}
