//! The worker's continuation vocabulary is the domain's, and only the domain's.
//!
//! This file used to exercise a second, entirely in-memory continuation kernel
//! that lived in `src/lib.rs` and had no production call site: its own
//! `WorkKind` beside `OperationKind`, its own `WorkRecord` beside `WorkItem`,
//! its own `claim`, `plan_step`, `commit_step` and `record_failure`. Its
//! vocabulary was also stale — `SessionDelete` and `SessionForkPage` were
//! renamed to trash, purge and clone by R-DELETE and deliberately not aliased —
//! so the tests here were pinning verbs the product no longer has.
//!
//! The kernel is gone (D-16). What remains is the assertion that matters: the
//! worker owns exactly one model of "may this step commit", and it is
//! `aex_operation_domain::lease`. Every property below is written against the
//! re-exports, so a future second kernel cannot slip in beside them without
//! this file failing to compile.

use std::num::NonZeroU16;

use aex_operation_domain::operation::{FailureClass, OperationFailure, OperationKind, Progress};
use aex_operation_domain::{DueShard, cursor::ContinuationCursor, cursor::CursorPosition};
use aex_wire::error::ErrorCode;
use aex_wire::ids::{OperationId, PrefixedId as _, Uuid7};
use aex_wire::types::Timestamp;
use session_operation_worker::{
    BatchItem, Fence, MAX_ATTEMPTS, StepOutcome, WorkItem, WorkState, batch_response, claim,
    complete, due_shard, renew,
};
use time::Duration;

use aex_operation_domain::lease::{ClaimDenial, ClaimOutcome, DedupIdentity, OwnerId, WorkId};

fn moment(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(millis).expect("in range")
}

fn owner(tag: u8) -> OwnerId {
    OwnerId(Uuid7::compose(1, [tag; 10]))
}

fn operation(tag: u8) -> OperationId {
    OperationId::from_uuid7(Uuid7::compose(1, [tag; 10]))
}

fn item(kind: OperationKind) -> WorkItem {
    let id = WorkId(Uuid7::compose(1, [9; 10]));
    WorkItem {
        id,
        operation: operation(2),
        kind,
        shard: DueShard(0),
        due_at: moment(0),
        priority: 0,
        attempt: 0,
        max_attempts: u16::try_from(MAX_ATTEMPTS).expect("the pinned ceiling fits a u16"),
        lease: None,
        state: WorkState::Runnable,
        dedup: DedupIdentity {
            operation: operation(2),
            step: 0,
        },
        cancel_requested: false,
    }
}

fn progressed(processed: u64) -> StepOutcome {
    StepOutcome::Progressed {
        cursor: ContinuationCursor::new(
            CursorPosition::Purge(aex_operation_domain::cursor::PurgeStage::Generations),
            processed,
            None,
        )
        .expect("builds"),
        next_due: moment(10_000),
        progress: Progress {
            phase: "purging".to_owned(),
            processed,
            total_hint: None,
        },
    }
}

#[test]
fn a_claim_takes_the_next_fence_and_a_renewal_never_advances_it() {
    let work = item(OperationKind::SessionPurge);
    let ClaimOutcome::Claimed(lease) = claim(&work, owner(1), moment(0), Duration::seconds(30))
    else {
        panic!("a runnable, due, uncancelled item is claimable");
    };
    assert_eq!(lease.fence, Fence::INITIAL.next());

    let mut held = work.clone();
    held.lease = Some(lease);
    held.state = WorkState::Claimed;

    let renewed = renew(&held, owner(1), lease.fence, moment(1), Duration::seconds(30))
        .expect("the holder may extend its own lease");
    assert_eq!(
        renewed.fence, lease.fence,
        "renewal churn must never invalidate the holder's own in-flight writes"
    );
    assert!(renewed.expires_at > lease.expires_at);
}

#[test]
fn a_live_lease_denies_another_owner_and_an_expired_one_is_a_visible_takeover() {
    let mut work = item(OperationKind::SessionPurge);
    let ClaimOutcome::Claimed(first) = claim(&work, owner(1), moment(0), Duration::seconds(30))
    else {
        panic!("claimable");
    };
    work.lease = Some(first);
    work.state = WorkState::Claimed;

    assert!(matches!(
        claim(&work, owner(2), moment(1), Duration::seconds(30)),
        ClaimOutcome::Denied(ClaimDenial::HeldByOther { .. })
    ));

    // Past the lease. A takeover is reported as itself rather than as a plain
    // claim, so it is always visible.
    let ClaimOutcome::Stolen { previous, lease } =
        claim(&work, owner(2), moment(60_000), Duration::seconds(30))
    else {
        panic!("an expired lease is takeable");
    };
    assert_eq!(previous, owner(1));
    assert_eq!(
        lease.fence,
        first.fence.next(),
        "the fence advances on every ownership change, or a paused worker could still commit"
    );
}

#[test]
fn a_cancelled_or_terminal_item_is_never_claimed() {
    let mut cancelled = item(OperationKind::SessionPurge);
    cancelled.cancel_requested = true;
    assert!(matches!(
        claim(&cancelled, owner(1), moment(0), Duration::seconds(30)),
        ClaimOutcome::Denied(ClaimDenial::CancelRequested)
    ));

    for state in [WorkState::Completed, WorkState::Poison] {
        let mut terminal = item(OperationKind::SessionPurge);
        terminal.state = state;
        assert!(
            matches!(
                claim(&terminal, owner(1), moment(0), Duration::seconds(30)),
                ClaimOutcome::Denied(ClaimDenial::Terminal(_))
            ),
            "{state:?} is absorbing"
        );
    }
}

#[test]
fn a_progressed_step_returns_the_item_to_the_queue_and_resets_its_attempts() {
    let mut work = item(OperationKind::SessionPurge);
    let ClaimOutcome::Claimed(lease) = claim(&work, owner(1), moment(0), Duration::seconds(30))
    else {
        panic!("claimable");
    };
    work.lease = Some(lease);
    work.state = WorkState::Claimed;
    work.attempt = 3;

    let commit = complete(&work, lease.fence, progressed(50), moment(1)).expect("the holder may commit");
    assert_eq!(commit.item.state, WorkState::Runnable);
    assert_eq!(commit.item.due_at, moment(10_000));
    assert_eq!(commit.item.lease, None, "a committed step releases its claim");
    assert_eq!(
        commit.item.attempt, 0,
        "progress is evidence the step is not poison, so its attempt budget resets"
    );
}

#[test]
fn a_stale_fence_cannot_commit_even_while_its_holder_still_believes_it_owns_the_item() {
    let mut work = item(OperationKind::SessionPurge);
    let ClaimOutcome::Claimed(first) = claim(&work, owner(1), moment(0), Duration::seconds(30))
    else {
        panic!("claimable");
    };
    work.lease = Some(first);
    work.state = WorkState::Claimed;
    let ClaimOutcome::Stolen { lease: second, .. } =
        claim(&work, owner(2), moment(60_000), Duration::seconds(30))
    else {
        panic!("takeable");
    };
    work.lease = Some(second);

    assert!(
        complete(&work, first.fence, progressed(1), moment(60_001)).is_err(),
        "the fence is a transaction condition precisely so a paused worker loses"
    );
    complete(&work, second.fence, progressed(1), moment(60_001))
        .expect("the current owner commits");
}

#[test]
fn a_retryable_failure_poisons_at_exactly_the_pinned_ceiling_and_is_never_redriven() {
    let mut work = item(OperationKind::SessionPurge);
    work.attempt = u16::try_from(MAX_ATTEMPTS).expect("fits") - 1;
    let ClaimOutcome::Claimed(lease) = claim(&work, owner(1), moment(0), Duration::seconds(30))
    else {
        panic!("claimable");
    };
    work.lease = Some(lease);
    work.state = WorkState::Claimed;

    let commit = complete(
        &work,
        lease.fence,
        StepOutcome::Failed {
            class: FailureClass::Retryable,
            error: OperationFailure::bare(ErrorCode::InternalError, FailureClass::Retryable),
        },
        moment(1),
    )
    .expect("the holder may record its failure");
    assert_eq!(commit.item.state, WorkState::Poison);
    assert!(
        matches!(
            commit.outcome,
            StepOutcome::Failed {
                class: FailureClass::PoisonManualReview,
                ..
            }
        ),
        "the last attempt reclassifies itself, so nothing redrives it"
    );
    assert!(
        matches!(
            claim(&commit.item, owner(1), moment(2), Duration::seconds(30)),
            ClaimOutcome::Denied(ClaimDenial::Terminal(WorkState::Poison))
        ),
        "poison is absorbing"
    );
}

#[test]
fn a_finished_or_cancelled_step_completes_the_item_outright() {
    for outcome in [
        StepOutcome::Finished(aex_operation_domain::operation::OperationResult::receipt()),
        StepOutcome::Cancelled,
    ] {
        let mut work = item(OperationKind::SessionPurge);
        let ClaimOutcome::Claimed(lease) = claim(&work, owner(1), moment(0), Duration::seconds(30))
        else {
            panic!("claimable");
        };
        work.lease = Some(lease);
        work.state = WorkState::Claimed;
        let commit = complete(&work, lease.fence, outcome, moment(1)).expect("commits");
        assert_eq!(commit.item.state, WorkState::Completed);
        assert_eq!(commit.item.lease, None);
    }
}

#[test]
fn due_shards_are_deterministic_and_a_zero_shard_count_is_refused() {
    assert_eq!(
        due_shard("work-1", 16).expect("shard"),
        due_shard("work-1", 16).expect("shard")
    );
    assert!(due_shard("work-1", 0).is_err());

    // The domain's own sharding agrees on the same invariant, over the identity
    // the durable item actually carries.
    let shards = NonZeroU16::new(64).expect("positive");
    let id = WorkId(Uuid7::compose(1, [9; 10]));
    assert_eq!(
        aex_operation_domain::due::shard_of(&id, shards),
        aex_operation_domain::due::shard_of(&id, shards)
    );
}

#[test]
fn a_partial_batch_response_never_throws_a_successful_item_away() {
    let response = batch_response(&[
        BatchItem::succeeded("a"),
        BatchItem::failed("b"),
        BatchItem::succeeded("c"),
    ]);
    assert_eq!(response.batch_item_failures, vec!["b".to_owned()]);
}
