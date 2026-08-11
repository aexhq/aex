//! Authoritative terminal-operation reconciliation and crash recovery.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use aex_observation_store_dynamodb::{
    SessionObservationDeletion, SessionObservationDeletionError, SessionObservationDeletionOutcome,
    SessionObservationDeletionRequest, SessionObservationDeletionStatus,
};
use aex_operation_domain::{OperationKind, OperationStatus};
use aex_session_dynamodb::StoreError;
use aex_session_dynamodb::plan::Participant;
use aex_wire::ids::{OperationId, PrefixedId, SessionId, Uuid7, WorkspaceId};
use aex_wire::types::Timestamp;
use aex_work_dynamodb::WorkClaim;
use aex_work_dynamodb::codec::{DeliveryEvidence, Payload, WorkRecord};
use aex_work_dynamodb::store::{DueEntry, DuePage};
use session_operation_worker::{
    LifecyclePort, LifecycleReadiness, LifecycleStep, OperationPort, OperationReconciler,
    OperationSnapshot, ReconcileDisposition, WorkHint, WorkPort, compile_cancelled_step,
    next_due_cursor,
};

fn timestamp(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(millis).expect("fixture instant")
}

fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [1; 10]))
}

fn session() -> SessionId {
    SessionId::from_uuid7(Uuid7::compose(1_754_051_696_789, [2; 10]))
}

fn operation() -> OperationId {
    OperationId::from_uuid7(Uuid7::compose(1_754_051_696_789, [3; 10]))
}

fn pending_work() -> WorkRecord {
    WorkRecord {
        work_id: "wrk_01j0000000000000000000000".to_owned(),
        workspace: workspace(),
        organization: aex_wire::ids::OrganizationId::from_uuid7(Uuid7::compose(
            1_754_051_696_789,
            [4; 10],
        )),
        session: Some(session()),
        agent: None,
        kind: "operation.step".to_owned(),
        priority: 0,
        due_at: timestamp(1_000),
        state: "pending".to_owned(),
        attempt: 0,
        max_attempts: 8,
        fence: 0,
        claim_owner: None,
        lease_expires_at: None,
        dedupe_key: "a".repeat(64),
        payload: Payload::new()
            .set("operationId", operation().to_string())
            .set("sessionId", session().to_string())
            .set("version", "4"),
        delivery: DeliveryEvidence::default(),
        created_at: timestamp(0),
        updated_at: timestamp(0),
    }
}

fn hint() -> WorkHint {
    WorkHint {
        work_id: pending_work().work_id,
        workspace: workspace(),
    }
}

struct MemoryWork {
    record: Arc<Mutex<Option<WorkRecord>>>,
    complete_faults: Mutex<VecDeque<StoreError>>,
}

impl MemoryWork {
    fn new(record: WorkRecord) -> Self {
        Self {
            record: Arc::new(Mutex::new(Some(record))),
            complete_faults: Mutex::new(VecDeque::new()),
        }
    }

    fn fail_complete_after_commit(&self) {
        self.complete_faults
            .lock()
            .expect("fault lock")
            .push_back(StoreError::CommitAmbiguous {
                resolve_by: aex_session_dynamodb::Resolution::TargetItem,
            });
    }

    fn state(&self) -> Option<String> {
        self.record
            .lock()
            .expect("record lock")
            .as_ref()
            .map(|record| record.state.clone())
    }
}

#[async_trait::async_trait]
impl WorkPort for MemoryWork {
    async fn load(
        &self,
        workspace: WorkspaceId,
        work_id: &str,
    ) -> Result<Option<WorkRecord>, StoreError> {
        Ok(self
            .record
            .lock()
            .expect("record lock")
            .clone()
            .filter(|record| record.workspace == workspace && record.work_id == work_id))
    }

    async fn claim(
        &self,
        work_id: &str,
        owner: &str,
        _now: Timestamp,
        lease_until: Timestamp,
    ) -> Result<WorkClaim, StoreError> {
        let mut record = self.record.lock().expect("record lock");
        let current = record.as_mut().ok_or_else(precondition)?;
        if current.work_id != work_id || current.state != "pending" {
            return Err(precondition());
        }
        "claimed".clone_into(&mut current.state);
        current.fence += 1;
        current.attempt += 1;
        current.claim_owner = Some(owner.to_owned());
        current.lease_expires_at = Some(lease_until);
        Ok(WorkClaim {
            work_id: current.work_id.clone(),
            fence: current.fence,
            owner: owner.to_owned(),
            attempt: current.attempt,
            lease_expires_at: lease_until,
        })
    }

    async fn complete(&self, hold: &WorkClaim, now: Timestamp) -> Result<(), StoreError> {
        let mut record = self.record.lock().expect("record lock");
        let current = record.as_mut().ok_or_else(precondition)?;
        if current.fence != hold.fence
            || current.claim_owner.as_deref() != Some(hold.owner.as_str())
            || current.state != "claimed"
        {
            return Err(precondition());
        }
        "done".clone_into(&mut current.state);
        current.claim_owner = None;
        current.lease_expires_at = None;
        current.updated_at = now;
        if let Some(error) = self.complete_faults.lock().expect("fault lock").pop_front() {
            return Err(error);
        }
        Ok(())
    }

    async fn scan_due(
        &self,
        _shard: u16,
        _now: Timestamp,
        _budget: aex_session_dynamodb::paging::PageBudget,
    ) -> Result<DuePage, StoreError> {
        unreachable!("these cases exercise a projected hint")
    }

    async fn load_cursor(
        &self,
        _shard: u16,
    ) -> Result<Option<aex_work_dynamodb::codec::ReconciliationCursor>, StoreError> {
        unreachable!("these cases exercise a projected hint")
    }

    async fn scan_due_after(
        &self,
        _shard: u16,
        _now: Timestamp,
        _budget: aex_session_dynamodb::paging::PageBudget,
        _after: Option<&aex_work_dynamodb::codec::ReconciliationCursor>,
    ) -> Result<DuePage, StoreError> {
        unreachable!("these cases exercise a projected hint")
    }

    async fn advance_cursor(
        &self,
        _cursor: &aex_work_dynamodb::codec::ReconciliationCursor,
    ) -> Result<(), StoreError> {
        unreachable!("these cases exercise a projected hint")
    }
}

struct MemoryLifecycle {
    work: Arc<Mutex<Option<WorkRecord>>>,
    prepares: Arc<Mutex<u64>>,
    settlements: Arc<Mutex<u64>>,
}

#[async_trait::async_trait]
impl LifecyclePort for MemoryLifecycle {
    async fn prepare(
        &self,
        _step: &LifecycleStep,
        _now: Timestamp,
    ) -> Result<LifecycleReadiness, StoreError> {
        *self.prepares.lock().expect("prepare lock") += 1;
        Ok(LifecycleReadiness::Ready)
    }

    async fn settle(
        &self,
        _step: &LifecycleStep,
        hold: &WorkClaim,
        _now: Timestamp,
    ) -> Result<(), StoreError> {
        let mut work = self.work.lock().expect("work lock");
        let row = work.as_mut().ok_or_else(precondition)?;
        if row.state != "claimed"
            || row.fence != hold.fence
            || row.claim_owner.as_deref() != Some(hold.owner.as_str())
        {
            return Err(precondition());
        }
        row.state = "done".to_owned();
        row.claim_owner = None;
        row.lease_expires_at = None;
        *self.settlements.lock().expect("settlement lock") += 1;
        Ok(())
    }
}

fn precondition() -> StoreError {
    StoreError::PreconditionFailed {
        participant: Participant::WORK_ROOT_WAKE,
        observed: None,
    }
}

struct MemoryOperations {
    snapshot: Mutex<OperationSnapshot>,
    work: Arc<Mutex<Option<WorkRecord>>>,
    cancellations: Mutex<u64>,
    cancel_faults: Mutex<VecDeque<StoreError>>,
}

#[derive(Default)]
struct MemoryObservationDeletion {
    requests: Mutex<Vec<SessionObservationDeletionRequest>>,
    complete: bool,
}

#[async_trait::async_trait]
impl SessionObservationDeletion for MemoryObservationDeletion {
    async fn request(
        &self,
        request: SessionObservationDeletionRequest,
    ) -> Result<SessionObservationDeletionOutcome, SessionObservationDeletionError> {
        self.requests.lock().expect("request lock").push(request);
        Ok(SessionObservationDeletionOutcome::Started)
    }

    async fn status(
        &self,
        _workspace: WorkspaceId,
        _session: SessionId,
        _operation: OperationId,
    ) -> Result<SessionObservationDeletionStatus, SessionObservationDeletionError> {
        Ok(if self.complete {
            SessionObservationDeletionStatus::Complete
        } else {
            SessionObservationDeletionStatus::Deleting
        })
    }
}

impl MemoryOperations {
    fn fail_cancel_after_commit(&self) {
        self.cancel_faults
            .lock()
            .expect("cancel fault lock")
            .push_back(StoreError::CommitAmbiguous {
                resolve_by: aex_session_dynamodb::Resolution::TargetItem,
            });
    }
}

#[async_trait::async_trait]
impl OperationPort for MemoryOperations {
    async fn load(
        &self,
        workspace: WorkspaceId,
        operation: OperationId,
    ) -> Result<Option<OperationSnapshot>, StoreError> {
        let snapshot = *self.snapshot.lock().expect("snapshot lock");
        Ok((snapshot.workspace == workspace && snapshot.id == operation).then_some(snapshot))
    }

    async fn cancel_step(
        &self,
        operation: &OperationSnapshot,
        hold: &WorkClaim,
        _now: Timestamp,
    ) -> Result<(), StoreError> {
        let mut snapshot = self.snapshot.lock().expect("snapshot lock");
        if operation != &*snapshot {
            return Err(precondition());
        }
        snapshot.status = OperationStatus::Cancelled;
        snapshot.version += 1;
        let mut work = self.work.lock().expect("work record lock");
        let record = work.as_mut().ok_or_else(precondition)?;
        if record.state != "claimed"
            || record.fence != hold.fence
            || record.claim_owner.as_deref() != Some(hold.owner.as_str())
        {
            return Err(precondition());
        }
        "done".clone_into(&mut record.state);
        record.claim_owner = None;
        record.lease_expires_at = None;
        *self.cancellations.lock().expect("cancellation lock") += 1;
        match self
            .cancel_faults
            .lock()
            .expect("cancel fault lock")
            .pop_front()
        {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

fn reconciler_with_cancel(
    status: OperationStatus,
    cancel_requested: bool,
) -> OperationReconciler<MemoryWork, MemoryOperations, MemoryObservationDeletion> {
    let work = MemoryWork::new(pending_work());
    OperationReconciler::new(
        MemoryWork {
            record: Arc::clone(&work.record),
            complete_faults: work.complete_faults,
        },
        MemoryOperations {
            snapshot: Mutex::new(OperationSnapshot {
                id: operation(),
                workspace: workspace(),
                session: Some(session()),
                kind: OperationKind::ContentGc,
                status,
                version: 4,
                cancel_requested,
                committed_at: None,
            }),
            work: Arc::clone(&work.record),
            cancellations: Mutex::new(0),
            cancel_faults: Mutex::new(VecDeque::new()),
        },
        MemoryObservationDeletion::default(),
        "worker-1",
        60_000,
    )
    .expect("valid worker settings")
}

fn reconciler(
    status: OperationStatus,
) -> OperationReconciler<MemoryWork, MemoryOperations, MemoryObservationDeletion> {
    reconciler_with_cancel(status, false)
}

fn deletion_reconciler(
    status: OperationStatus,
    observations_complete: bool,
) -> OperationReconciler<MemoryWork, MemoryOperations, MemoryObservationDeletion> {
    let work = MemoryWork::new(pending_work());
    let shared_work = Arc::clone(&work.record);
    let prepares = Arc::new(Mutex::new(0));
    let settlements = Arc::new(Mutex::new(0));
    OperationReconciler::new(
        MemoryWork {
            record: Arc::clone(&shared_work),
            complete_faults: work.complete_faults,
        },
        MemoryOperations {
            snapshot: Mutex::new(OperationSnapshot {
                id: operation(),
                workspace: workspace(),
                session: Some(session()),
                kind: OperationKind::SessionDelete,
                status,
                version: 4,
                cancel_requested: false,
                committed_at: None,
            }),
            work: Arc::clone(&shared_work),
            cancellations: Mutex::new(0),
            cancel_faults: Mutex::new(VecDeque::new()),
        },
        MemoryObservationDeletion {
            requests: Mutex::new(Vec::new()),
            complete: observations_complete,
        },
        "worker-1",
        60_000,
    )
    .expect("valid worker settings")
    .with_lifecycle(MemoryLifecycle {
        work: shared_work,
        prepares,
        settlements,
    })
}

#[tokio::test]
async fn irreversible_session_delete_schedules_the_exact_observation_participant() {
    let reconciler = deletion_reconciler(OperationStatus::Queued, false);
    assert_eq!(
        reconciler
            .reconcile(&hint(), timestamp(2_000))
            .await
            .expect("observation deletion request"),
        ReconcileDisposition::EffectScheduled
    );
    assert_eq!(reconciler.work().state().as_deref(), Some("pending"));
    assert_eq!(
        *reconciler
            .observation_deletions()
            .requests
            .lock()
            .expect("request lock"),
        vec![SessionObservationDeletionRequest {
            workspace: workspace(),
            session: session(),
            operation: operation(),
            now: timestamp(2_000),
        }]
    );
}

#[tokio::test]
async fn session_delete_settles_only_after_observation_deletion_is_complete() {
    let reconciler = deletion_reconciler(OperationStatus::Queued, true);
    assert_eq!(
        reconciler
            .reconcile(&hint(), timestamp(2_000))
            .await
            .expect("complete deletion cascade"),
        ReconcileDisposition::Retired
    );
    assert_eq!(reconciler.work().state().as_deref(), Some("done"));
    assert_eq!(
        reconciler
            .observation_deletions()
            .requests
            .lock()
            .expect("request lock")
            .len(),
        1
    );
}

#[tokio::test]
async fn a_nonterminal_lifecycle_step_is_prepared_then_settled_under_the_exact_claim() {
    let work = MemoryWork::new(pending_work());
    let shared_work = Arc::clone(&work.record);
    let prepares = Arc::new(Mutex::new(0));
    let settlements = Arc::new(Mutex::new(0));
    let reconciler = OperationReconciler::new(
        MemoryWork {
            record: Arc::clone(&shared_work),
            complete_faults: work.complete_faults,
        },
        MemoryOperations {
            snapshot: Mutex::new(OperationSnapshot {
                id: operation(),
                workspace: workspace(),
                session: Some(session()),
                kind: OperationKind::SessionSuspend,
                status: OperationStatus::Queued,
                version: 4,
                cancel_requested: false,
                committed_at: None,
            }),
            work: Arc::clone(&shared_work),
            cancellations: Mutex::new(0),
            cancel_faults: Mutex::new(VecDeque::new()),
        },
        MemoryObservationDeletion::default(),
        "worker-1",
        60_000,
    )
    .expect("valid worker settings")
    .with_lifecycle(MemoryLifecycle {
        work: Arc::clone(&shared_work),
        prepares: Arc::clone(&prepares),
        settlements: Arc::clone(&settlements),
    });

    assert_eq!(
        reconciler
            .reconcile(&hint(), timestamp(2_000))
            .await
            .expect("lifecycle reconciliation"),
        ReconcileDisposition::Retired
    );
    assert_eq!(*prepares.lock().expect("prepare lock"), 1);
    assert_eq!(*settlements.lock().expect("settlement lock"), 1);
    assert_eq!(
        shared_work
            .lock()
            .expect("work lock")
            .as_ref()
            .map(|row| row.state.as_str()),
        Some("done")
    );
}

#[tokio::test]
async fn a_succeeded_or_cancelled_operation_retires_its_exact_work_under_a_fence() {
    for status in [OperationStatus::Succeeded, OperationStatus::Cancelled] {
        let reconciler = reconciler(status);
        assert_eq!(
            reconciler
                .reconcile(&hint(), timestamp(2_000))
                .await
                .expect("terminal reconciliation"),
            ReconcileDisposition::Retired
        );
        assert_eq!(reconciler.work().state().as_deref(), Some("done"));
    }
}

#[tokio::test]
async fn nonterminal_work_is_never_claimed_by_the_terminal_reconciler() {
    let reconciler = reconciler(OperationStatus::Running);
    assert_eq!(
        reconciler
            .reconcile(&hint(), timestamp(2_000))
            .await
            .expect("deferred"),
        ReconcileDisposition::Deferred
    );
    assert_eq!(reconciler.work().state().as_deref(), Some("pending"));
}

#[tokio::test]
async fn a_requested_running_cancellation_commits_with_the_fenced_retirement() {
    let reconciler = reconciler_with_cancel(OperationStatus::Running, true);
    assert_eq!(
        reconciler
            .reconcile(&hint(), timestamp(2_000))
            .await
            .expect("cancellation reconciliation"),
        ReconcileDisposition::Retired
    );
    assert_eq!(
        *reconciler
            .operations()
            .cancellations
            .lock()
            .expect("cancellation lock"),
        1
    );
    assert_eq!(reconciler.work().state().as_deref(), Some("done"));
}

#[tokio::test]
async fn a_cancel_commit_crash_is_resolved_from_both_atomic_targets() {
    let reconciler = reconciler_with_cancel(OperationStatus::Running, true);
    reconciler.operations().fail_cancel_after_commit();
    assert_eq!(
        reconciler
            .reconcile(&hint(), timestamp(2_000))
            .await
            .expect("ambiguous cancelled step is resolved"),
        ReconcileDisposition::AlreadyRetired
    );
    assert_eq!(
        reconciler
            .reconcile(&hint(), timestamp(2_001))
            .await
            .expect("the duplicate is a no-op"),
        ReconcileDisposition::AlreadyRetired
    );
}

#[test]
fn a_cancelled_step_is_one_stably_identified_two_authority_transaction() {
    let operation = OperationSnapshot {
        id: operation(),
        workspace: workspace(),
        session: Some(session()),
        kind: OperationKind::ContentGc,
        status: OperationStatus::Running,
        version: 4,
        cancel_requested: true,
        committed_at: None,
    };
    let hold = WorkClaim {
        work_id: pending_work().work_id,
        fence: 3,
        owner: "worker-1".to_owned(),
        attempt: 1,
        lease_expires_at: timestamp(60_000),
    };
    let plan = compile_cancelled_step(
        "dev-eu-west-1-session-authority",
        "dev-eu-west-1-regional-work",
        &operation,
        &hold,
        timestamp(2_000),
    )
    .expect("the cancelled step compiles");

    assert_eq!(
        plan.participants(),
        &[Participant::SESSION_OPERATION, Participant::WORK_WAKE_DONE]
    );
    assert_eq!(plan.len(), 2);
    assert_eq!(
        plan.client_request_token(),
        format!("cancel:{}", operation.id)
    );
    assert!(plan.client_request_token().len() <= 36);
}

#[test]
fn a_full_due_page_advances_and_the_last_page_wraps_without_starvation() {
    let due = DueEntry {
        work_id: pending_work().work_id,
        workspace: workspace(),
        kind: "operation.step".to_owned(),
        state: "pending".to_owned(),
        attempt: 0,
        max_attempts: 8,
        fence: 0,
        lease_expires_at: None,
        effective_due_at: timestamp(1_000),
    };
    let full = DuePage {
        items: vec![due.clone()],
        scanned_through: Some(due.effective_due_at),
        has_more: true,
    };
    let advanced = next_due_cursor(7, None, &full, timestamp(2_000))
        .expect("the full page is valid")
        .expect("the page advances");
    assert_eq!(advanced.last_work_id.as_deref(), Some(due.work_id.as_str()));
    assert_eq!(advanced.revision, 1);

    let last = DuePage {
        items: vec![due],
        scanned_through: Some(timestamp(1_000)),
        has_more: false,
    };
    let wrapped = next_due_cursor(7, Some(&advanced), &last, timestamp(2_001))
        .expect("the last page is valid")
        .expect("the position wraps");
    assert_eq!(wrapped.last_work_id, None);
    assert_eq!(wrapped.revision, 2);
}

#[tokio::test]
async fn a_crash_after_retirement_is_resolved_by_read_and_the_duplicate_is_acknowledged() {
    let reconciler = reconciler(OperationStatus::Succeeded);
    reconciler.work().fail_complete_after_commit();
    assert_eq!(
        reconciler
            .reconcile(&hint(), timestamp(2_000))
            .await
            .expect("ambiguous commit is resolved"),
        ReconcileDisposition::AlreadyRetired
    );
    assert_eq!(
        reconciler
            .reconcile(&hint(), timestamp(2_001))
            .await
            .expect("duplicate is a no-op"),
        ReconcileDisposition::AlreadyRetired
    );
}

#[tokio::test]
async fn the_projection_body_is_only_a_hint_and_the_authoritative_row_must_match_it() {
    let reconciler = reconciler(OperationStatus::Succeeded);
    let wrong_workspace = WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [9; 10]));
    let error = reconciler
        .reconcile(
            &WorkHint {
                work_id: hint().work_id,
                workspace: wrong_workspace,
            },
            timestamp(2_000),
        )
        .await
        .expect_err("a forged tenant hint is not acknowledged");
    assert!(
        error.to_string().contains("authoritative work row"),
        "{error}"
    );
}
