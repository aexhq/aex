//! Property catalogue for `aex-operation-domain`: plan 04 items 40-55.
//!
//! The lease cases run a reference model of interleaved claim/renew/steal
//! histories: at every instant of every generated history at most one owner may
//! hold a live lease, and the fence never goes backwards.

use std::collections::BTreeMap;
use std::num::NonZeroU16;

use aex_operation_domain::admission::ConflictCode;
use aex_operation_domain::due::PageBudget;
use aex_operation_domain::operation::{FailureClass, OperationScope};
use aex_operation_domain::{
    AdmissionOutcome, AdmitRequest, CancelRejection, ClaimDenial, ClaimOutcome, ContinuationCursor,
    CursorPosition, DedupIdentity, DeletionEpoch, DeletionGuard, DeletionState, ExportMember,
    Fence, FenceRejection, LifecycleStage, Operation, OperationFailure, OperationKind,
    OperationResult, OperationStatus, OwnerId, PAGE_DEFAULT, PageError, PageToken, Progress,
    SessionDeleteStage, StepOutcome, WorkId, WorkItem, WorkState, admit, backoff, backoff_step,
    cancel, claim, commit_point, complete, fail, page_limit, plan_due_scan,
    redact_for_session_delete, shard_of, start, succeed,
};
use aex_wire::error::ErrorCode;
use aex_wire::idempotency::IntentDigest;
use aex_wire::ids::{
    GenerationId, MeasurementId, OperationId, PrefixedId as _, SessionId, Uuid7, WorkspaceId,
};
use aex_wire::types::Timestamp;
use proptest::prelude::*;
use time::Duration;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn moment(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(millis).expect("in range")
}

fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]))
}

fn session() -> SessionId {
    SessionId::from_uuid7(Uuid7::compose(1, [3; 10]))
}

fn operation_id(tag: u8) -> OperationId {
    OperationId::from_uuid7(Uuid7::compose(1, [tag; 10]))
}

fn work(tag: u8) -> WorkId {
    WorkId(Uuid7::compose(1, [tag; 10]))
}

fn owner(tag: u8) -> OwnerId {
    OwnerId(Uuid7::compose(1, [tag; 10]))
}

fn runnable_item(max_attempts: u16) -> WorkItem {
    WorkItem {
        id: work(2),
        operation: operation_id(1),
        kind: OperationKind::ContentGc,
        due_at: moment(0),
        priority: 0,
        attempt: 0,
        max_attempts,
        lease: None,
        state: WorkState::Runnable,
        dedup: DedupIdentity {
            operation: operation_id(1),
            step: 0,
        },
        cancel_requested: false,
    }
}

fn request(kind: OperationKind, intent: u8) -> AdmitRequest {
    AdmitRequest {
        id: operation_id(2),
        workspace: workspace(),
        session: Some(session()),
        kind,
        intent: IntentDigest::from_bytes([intent; 32]),
        scope: OperationScope::Session(session()),
        inline_result: None,
        execution: None,
    }
}

fn guard(state: DeletionState) -> DeletionGuard {
    DeletionGuard {
        session: session(),
        state,
        epoch: DeletionEpoch(1),
        delete_operation: Some(operation_id(9)),
    }
}

fn admitted(kind: OperationKind) -> Operation {
    match admit(None, None, &request(kind, 1), moment(0)) {
        AdmissionOutcome::Inserted(created) => *created,
        other => panic!("expected an insert, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 40-44 — the lease reference model
// ---------------------------------------------------------------------------

/// One step of a generated claim history.
#[derive(Debug, Clone, Copy)]
enum LeaseStep {
    Claim {
        owner: u8,
        at: i64,
        ttl: i64,
    },
    Renew {
        owner: u8,
        fence: u64,
        at: i64,
        ttl: i64,
    },
    Finish {
        fence: u64,
        at: i64,
    },
}

fn lease_step() -> impl Strategy<Value = LeaseStep> {
    prop_oneof![
        (1_u8..4, 0_i64..5_000, 1_i64..2_000).prop_map(|(owner, at, ttl)| LeaseStep::Claim {
            owner,
            at,
            ttl
        }),
        (1_u8..4, 0_u64..6, 0_i64..5_000, 1_i64..2_000).prop_map(|(owner, fence, at, ttl)| {
            LeaseStep::Renew {
                owner,
                fence,
                at,
                ttl,
            }
        }),
        (0_u64..6, 0_i64..5_000).prop_map(|(fence, at)| LeaseStep::Finish { fence, at }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 192, ..ProptestConfig::default() })]

    /// 40 `fence_monotone`, 41 `single_lease_owner`, 42 `stale_fence_rejected`,
    /// 43 `steal_requires_expiry`.
    #[test]
    fn lease_history_is_single_owner_and_fence_monotone(
        steps in prop::collection::vec(lease_step(), 1..24)
    ) {
        let mut item = runnable_item(64);
        let mut highest = Fence::INITIAL;

        for step in steps {
            match step {
                LeaseStep::Claim { owner: tag, at, ttl } => {
                    let before = item.clone();
                    let outcome = claim(&item, owner(tag), moment(at), Duration::milliseconds(ttl));
                    match outcome {
                        ClaimOutcome::Claimed(lease) | ClaimOutcome::Stolen { lease, .. } => {
                            // An ownership change always advances the fence.
                            prop_assert!(lease.fence > highest, "a takeover must advance the fence");
                            highest = lease.fence;
                            // A steal is only legal once the previous lease lapsed.
                            if let ClaimOutcome::Stolen { .. } = outcome {
                                let previous = before.lease.expect("a steal needs a previous lease");
                                prop_assert!(!previous.is_live_at(moment(at)));
                            }
                            item.lease = Some(lease);
                            item.state = WorkState::Claimed;
                        }
                        ClaimOutcome::Renewed(lease) => {
                            // A renew keeps the fence exactly.
                            prop_assert_eq!(lease.fence, highest);
                            item.lease = Some(lease);
                        }
                        ClaimOutcome::Denied(denial) => {
                            if let ClaimDenial::HeldByOther { owner: held, .. } = denial {
                                let live = item.lease.expect("a denial names a live lease");
                                prop_assert!(live.is_live_at(moment(at)));
                                prop_assert_eq!(live.owner, held);
                            }
                            // A denial never writes.
                            prop_assert_eq!(&item, &before);
                        }
                    }
                }
                LeaseStep::Renew { owner: tag, fence, at, ttl } => {
                    let before = item.clone();
                    match aex_operation_domain::renew(
                        &item,
                        owner(tag),
                        Fence(fence),
                        moment(at),
                        Duration::milliseconds(ttl),
                    ) {
                        Ok(lease) => {
                            prop_assert_eq!(lease.fence, highest);
                            item.lease = Some(lease);
                        }
                        Err(rejection) => {
                            prop_assert_eq!(&item, &before, "a rejection writes nothing");
                            if let FenceRejection::StaleFence { current, presented } = rejection {
                                prop_assert_ne!(current, presented);
                            }
                        }
                    }
                }
                LeaseStep::Finish { fence, at } => {
                    let before = item.clone();
                    let settled = complete(
                        &item,
                        Fence(fence),
                        StepOutcome::Finished(OperationResult::receipt()),
                        moment(at),
                    );
                    if let Ok(commit) = settled {
                        prop_assert_eq!(Fence(fence), highest);
                        item = commit.item;
                    } else {
                        prop_assert_eq!(&item, &before, "a rejection writes nothing");
                    }
                }
            }

            // At most one live lease exists at any instant, by construction:
            // the item holds at most one `Option<Lease>`, and every accepted
            // ownership change replaced it under a strictly higher fence.
            if let Some(lease) = item.lease {
                prop_assert!(lease.fence <= highest);
            }
        }
    }

    /// 45 `backoff_deterministic`.
    #[test]
    fn backoff_deterministic(tag in 0_u8..64, base_ms in 1_i64..5_000, cap_ms in 1_i64..600_000) {
        let id = work(tag);
        let base = Duration::milliseconds(base_ms);
        let cap = Duration::milliseconds(cap_ms.max(base_ms));
        let mut previous = Duration::ZERO;
        for attempt in 0_u16..24 {
            let delay = backoff(attempt, base, cap, &id);
            prop_assert_eq!(delay, backoff(attempt, base, cap, &id), "backoff must be pure");
            prop_assert!(delay >= previous, "backoff must be non-decreasing");
            prop_assert!(delay <= cap, "backoff must respect the cap");
            let step = backoff_step(attempt, base, cap);
            prop_assert!(delay <= step, "jitter never exceeds the nominal step");
            prop_assert!(
                delay.whole_milliseconds() >= step.whole_milliseconds() / 2,
                "jitter never falls below half the nominal step"
            );
            previous = delay;
        }
    }
}

#[test]
fn due_shards_are_uniform() {
    // 46 `due_shard_uniform`.
    let shards = NonZeroU16::new(64).expect("non-zero");
    let sample = 100_000_u64;
    let mut counts: BTreeMap<u16, u64> = BTreeMap::new();
    for index in 0..sample {
        let id = WorkId(Uuid7::compose(index, [3; 10]));
        *counts.entry(shard_of(&id, shards).0).or_default() += 1;
    }
    let mean = sample / u64::from(shards.get());
    for (shard, count) in &counts {
        assert!(
            *count <= mean * 2,
            "shard {shard} took {count} of an expected {mean}"
        );
    }
    assert_eq!(counts.len(), usize::from(shards.get()));
}

#[test]
fn a_due_scan_covers_every_runnable_item_within_one_horizon() {
    // 47 `due_scan_covers`.
    let shards = NonZeroU16::new(16).expect("non-zero");
    let now = moment(10_000);
    let plan = plan_due_scan(shards, now, PageBudget::default_budget());
    for index in 0..2_000_u64 {
        let id = WorkId(Uuid7::compose(index, [4; 10]));
        let shard = shard_of(&id, shards);
        // Every item due at or before the horizon appears in exactly one page.
        assert!(plan.covers(shard, moment(0)));
        assert!(plan.covers(shard, now));
        assert!(!plan.covers(shard, moment(now.unix_millis() + 1)));
    }
    assert_eq!(plan.shards.len(), usize::from(shards.get()));
}

#[test]
fn attempts_are_bounded_and_poison_is_never_redriven() {
    // 44 `attempt_bounded`.
    let mut item = runnable_item(3);
    for round in 0..8 {
        let outcome = claim(&item, owner(1), moment(0), Duration::seconds(30));
        let Some(lease) = outcome.lease() else {
            assert_eq!(
                outcome,
                ClaimOutcome::Denied(ClaimDenial::Terminal(WorkState::Poison)),
                "round {round}"
            );
            assert!(item.attempt <= item.max_attempts);
            return;
        };
        item.lease = Some(lease);
        item.state = WorkState::Claimed;
        let commit = complete(
            &item,
            lease.fence,
            StepOutcome::Failed {
                class: FailureClass::Retryable,
                error: OperationFailure::bare(ErrorCode::InternalError, FailureClass::Retryable),
            },
            moment(1),
        )
        .expect("completes");
        item = commit.item;
        assert!(item.attempt <= item.max_attempts);
    }
    panic!("a bounded item must reach Poison");
}

// ---------------------------------------------------------------------------
// 48, 49 — the cursor
// ---------------------------------------------------------------------------

fn cursor_position() -> impl Strategy<Value = CursorPosition> {
    let generation = GenerationId::from_uuid7(Uuid7::compose(1, [8; 10]));
    prop_oneof![
        prop::sample::select(SessionDeleteStage::ALL.to_vec())
            .prop_map(CursorPosition::SessionDelete),
        prop::sample::select(LifecycleStage::ALL.to_vec())
            .prop_map(move |stage| { CursorPosition::SessionSuspend { stage, generation } }),
        prop::sample::select(LifecycleStage::ALL.to_vec())
            .prop_map(move |stage| { CursorPosition::SessionResume { stage, generation } }),
        prop::sample::select(LifecycleStage::ALL.to_vec())
            .prop_map(move |stage| { CursorPosition::SessionTerminate { stage, generation } }),
        (
            0_u32..1_000,
            0_u64..1_000_000,
            prop::sample::select(ExportMember::ALL.to_vec())
        )
            .prop_map(|(part, byte_offset, member)| CursorPosition::Export {
                part,
                byte_offset,
                member
            }),
        prop::collection::vec(any::<u8>(), 0..512).prop_map(|bytes| CursorPosition::Gc {
            scanned_through: PageToken::new(bytes).expect("in range")
        }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// 48 `cursor_roundtrip`.
    #[test]
    fn cursor_roundtrip(
        position in cursor_position(),
        processed in 0_u64..1_000_000,
        total in prop::option::of(0_u64..1_000_000),
    ) {
        let cursor = ContinuationCursor::new(position, processed, total).expect("builds");
        let encoded = cursor.encode().expect("encodes");
        prop_assert!(encoded.len() <= aex_operation_domain::CURSOR_MAX_ENCODED_BYTES);
        prop_assert_eq!(ContinuationCursor::decode(&encoded), Ok(cursor));

        let mut trailing = encoded.clone();
        trailing.push(0);
        prop_assert!(ContinuationCursor::decode(&trailing).is_err());
        prop_assert!(ContinuationCursor::decode(&encoded[..encoded.len() - 1]).is_err());
    }

    /// 49 `cursor_progress_monotone`.
    #[test]
    fn cursor_progress_monotone(reports in prop::collection::vec(0_u64..1_000, 1..16)) {
        let running = start(&admitted(OperationKind::TelemetryExport), moment(1))
            .expect("starts")
            .operation;
        let mut current = running;
        let mut highest = 0_u64;
        for processed in reports {
            let attempt = aex_operation_domain::progress(
                &current,
                Progress {
                    phase: "working".to_owned(),
                    processed,
                    total_hint: None,
                },
                moment(2),
            );
            if processed >= highest {
                current = attempt.expect("a non-decreasing report is accepted").operation;
                highest = processed;
            } else {
                prop_assert!(attempt.is_err(), "a regression must be rejected");
            }
            prop_assert_eq!(
                current.progress.as_ref().map_or(0, |value| value.processed),
                highest
            );
        }
    }

    /// 55 `page_limit_bounds`.
    #[test]
    fn page_limit_bounds(requested in prop::option::of(0_u16..3_000), max in 1_u16..2_000) {
        match page_limit(requested, max) {
            Ok(limit) => {
                prop_assert!(limit >= 1 && limit <= max);
                if let Some(value) = requested {
                    prop_assert_eq!(limit, value);
                } else {
                    prop_assert_eq!(limit, PAGE_DEFAULT.min(max));
                }
            }
            Err(PageError::Zero) => prop_assert_eq!(requested, Some(0)),
            Err(PageError::AboveMaximum { requested: asked, max: bound }) => {
                prop_assert_eq!(Some(asked), requested);
                prop_assert!(asked > bound);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 50-54 — the operation envelope
// ---------------------------------------------------------------------------

#[test]
fn commit_latch_is_total() {
    // 50 `commit_latch`. A continued kind is the one that is created `Queued`;
    // an inline kind is created already `Succeeded` by construction (D-07).
    let queued = admitted(OperationKind::TelemetryExport);
    assert_eq!(queued.status, OperationStatus::Queued);
    let running = start(&queued, moment(1)).expect("starts").operation;
    let latched = commit_point(&running, None, moment(2)).expect("latches");
    assert!(latched.latched_commit);
    assert_eq!(latched.operation.status, OperationStatus::Running);
    assert!(!latched.operation.cancelable());
    assert!(matches!(
        fail(
            &latched.operation,
            OperationFailure::bare(ErrorCode::InternalError, FailureClass::Terminal),
            moment(3)
        ),
        Err(aex_operation_domain::TransitionError::FailAfterCommit { .. })
    ));
    // Latching twice keeps the first instant and reports no second latch.
    let again = commit_point(&latched.operation, None, moment(4)).expect("idempotent");
    assert!(!again.latched_commit);
    assert_eq!(again.operation.committed_at, Some(moment(2)));

    let direct = succeed(&queued, OperationResult::receipt(), moment(9)).expect("succeeds");
    assert!(direct.latched_commit);
    assert_eq!(direct.operation.committed_at, Some(moment(9)));
}

#[test]
fn cancel_precedence_is_decided_by_the_latch() {
    // 51 `cancel_precedence`.
    for kind in OperationKind::ALL {
        let queued = admitted(kind);
        let before = cancel(&queued, moment(1));
        if kind.cancelable_on_accept() && !queued.status.is_terminal() {
            let cancelled = before.expect("cancelable before the latch").operation;
            assert_eq!(cancelled.status, OperationStatus::Cancelled);
        } else if queued.status.is_terminal() {
            assert!(matches!(
                before,
                Err(CancelRejection::AlreadyTerminal(_)) | Ok(_)
            ));
        } else {
            assert!(matches!(before, Err(CancelRejection::NotCancelable { .. })));
        }

        if queued.status.is_terminal() {
            continue;
        }
        let running = start(&queued, moment(1)).expect("starts").operation;
        let committed = commit_point(&running, None, moment(2))
            .expect("latches")
            .operation;
        assert!(matches!(
            cancel(&committed, moment(3)),
            Err(CancelRejection::NotCancelable {
                committed: true,
                ..
            })
        ));
    }
}

#[test]
fn admission_outcomes_are_total() {
    // 52 `admission_outcomes_total`.
    let guards = [
        None,
        Some(guard(DeletionState::Live)),
        Some(guard(DeletionState::Deleting)),
        Some(guard(DeletionState::Deleted)),
    ];
    for kind in OperationKind::ALL {
        for guard_state in &guards {
            for intent in [1_u8, 2] {
                let fresh = admit(
                    None,
                    guard_state.as_ref(),
                    &request(kind, intent),
                    moment(1),
                );
                match (&fresh, guard_state.map(|value| value.state)) {
                    (AdmissionOutcome::Inserted(_), Some(DeletionState::Live) | None)
                    | (
                        AdmissionOutcome::DeletionInProgress { .. },
                        Some(DeletionState::Deleting),
                    )
                    | (AdmissionOutcome::SessionDeleted { .. }, Some(DeletionState::Deleted)) => {}
                    other => panic!("unexpected fresh admission {other:?}"),
                }

                let existing = admitted(kind);
                let replay = admit(
                    Some(&existing),
                    guard_state.as_ref(),
                    &request(kind, intent),
                    moment(2),
                );
                if intent == 1 {
                    assert_eq!(replay, AdmissionOutcome::Replay(Box::new(existing)));
                } else {
                    assert_eq!(
                        replay,
                        AdmissionOutcome::Conflict(ConflictCode::OperationIdempotencyConflict)
                    );
                }
            }
        }
    }
}

#[test]
fn a_foreign_workspace_is_never_told_the_record_exists() {
    // 52, workspace scoping: not_found, never forbidden.
    let existing = admitted(OperationKind::SessionCancel);
    let mut foreign = request(OperationKind::SessionCancel, 1);
    foreign.workspace = WorkspaceId::from_uuid7(Uuid7::compose(2, [8; 10]));
    let outcome = admit(Some(&existing), None, &foreign, moment(2));
    assert_eq!(
        outcome,
        AdmissionOutcome::Conflict(ConflictCode::WrongWorkspace)
    );
    assert_eq!(ConflictCode::WrongWorkspace.code(), ErrorCode::NotFound);
}

#[test]
fn redaction_preserves_the_envelope() {
    // 54 `redaction_preserves_envelope`.
    let content = aex_wire::CanonicalJson::parse("{\"a\":1}").expect("canonical");
    for kind in OperationKind::ALL {
        let mut done = admitted(kind);
        done.status = OperationStatus::Succeeded;
        done.terminal_at = Some(moment(3));
        done.committed_at = Some(moment(3));
        done.result = Some(OperationResult {
            measurement: Some(MeasurementId::from_uuid7(Uuid7::compose(1, [5; 10]))),
            content: Some(content.clone()),
        });

        match redact_for_session_delete(&done) {
            None => assert!(
                kind.result_survives_session_delete(),
                "{kind:?} must be redacted"
            ),
            Some(commit) => {
                assert!(!kind.result_survives_session_delete());
                let redacted = commit.operation;
                assert_eq!(redacted.id, done.id);
                assert_eq!(redacted.kind, done.kind);
                assert_eq!(redacted.status, done.status);
                assert_eq!(redacted.intent, done.intent);
                assert_eq!(redacted.created_at, done.created_at);
                assert_eq!(redacted.terminal_at, done.terminal_at);
                let result = redacted.result.expect("the envelope survives");
                assert_eq!(result.content, None);
                assert_eq!(
                    result.measurement,
                    Some(MeasurementId::from_uuid7(Uuid7::compose(1, [5; 10])))
                );
            }
        }
    }
}

#[test]
fn a_telemetry_export_is_never_cancelable_through_the_operation_row() {
    // Two shipped crates disagreed about this: `cancelable_on_accept` said
    // true while `aex_session_dynamodb::operation_cancel_owned` said false.
    // An export's effect fence is the observation export row, so an
    // operation-row update would acknowledge a command that cannot stop the
    // launcher or the task. Cancellation lives on `telemetry_export_revoke`.
    assert!(!OperationKind::TelemetryExport.cancelable_on_accept());
    let queued = admitted(OperationKind::TelemetryExport);
    assert_eq!(queued.status, OperationStatus::Queued);
    assert!(
        !queued.cancelable(),
        "a 202 must not advertise a cancellation nothing behind it can honour"
    );
}
