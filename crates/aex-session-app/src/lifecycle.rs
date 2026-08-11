//! Admission plans for the five public session lifecycle operations.
//!
//! Admission never performs a provider effect. It elects the caller's operation,
//! takes the session mutation guard, records the truthful transitional state and
//! inserts one deterministic runnable work row in the same transaction. A worker
//! must observe the exact retained generation before it publishes a terminal
//! operation result.

use aex_operation_domain::{
    AdmissionOutcome, AdmitRequest, DedupIdentity, Operation, OperationKind, OperationResult,
    OperationScope, OperationVersion, WorkId, WorkItem, WorkState, admit, succeed,
};
use aex_session_domain::{
    CancelCause, CommandClass, Session, TerminationReason, acquire_mutation_guard, begin_delete,
    cancel_session_fence, pause_gate,
};
use aex_wire::idempotency::IntentDigest;
use aex_wire::ids::{OperationId, PrefixedId as _, SessionId, WorkspaceId};
use aex_wire::types::Timestamp;
use aex_wire::{CanonicalJson, canonical::to_jcs_string, models};

use crate::error::AppError;
use crate::plan::{
    Condition, Hint, Planned, SessionTransaction, TransactionIntent, WorkCompletion, Write,
};
use crate::ports::{AppContext, PortError};

// `regional-work` has five bounded priority bands, zero highest. Customer
// lifecycle control must outrank background cleanup and reconciliation.
const LIFECYCLE_WORK_PRIORITY: u16 = 0;
const LIFECYCLE_MAX_ATTEMPTS: u16 = 5;

/// One caller-minted lifecycle operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LifecycleCommand {
    /// Owning workspace.
    pub workspace: WorkspaceId,
    /// Target session.
    pub session: SessionId,
    /// Caller-minted operation identity.
    pub operation: OperationId,
    /// Exact canonical request intent.
    pub intent: IntentDigest,
    /// Which session lifecycle transition to execute.
    pub kind: OperationKind,
}

/// New atomic admission or exact durable operation replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleAdmissionOutcome {
    /// The operation, head transition and work item must commit together.
    Planned(Planned<Operation>),
    /// The identity was already elected with the same scope, kind and intent.
    Replayed(Operation),
}

/// The exact regional-work claim held while a lifecycle effect is settled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleWorkClaim {
    /// Canonical work identity.
    pub work_id: String,
    /// Monotonic claim fence.
    pub fence: u64,
    /// Exact claim owner.
    pub owner: String,
}

/// Atomically publishes one completed lifecycle effect.
///
/// The provider/runtime authority is observed before this planner runs. This
/// transaction is the session-facing consistency barrier: it advances the
/// public head, terminalizes the operation, releases the mutation guard and
/// retires the exact fenced work row together.
pub fn settle_lifecycle_operation(
    session: &Session,
    operation: &Operation,
    operation_version: OperationVersion,
    claim: &LifecycleWorkClaim,
    now: Timestamp,
) -> Result<Planned<Operation>, AppError> {
    if operation.workspace != session.workspace
        || operation.session != Some(session.id)
        || operation.scope != OperationScope::Session(session.id)
        || claim.work_id != WorkId(operation.id.uuid7()).to_string()
    {
        return Err(AppError::Port(PortError::Corrupt {
            kind: "lifecycle operation settlement",
            reason: "session, operation and work binding disagree",
        }));
    }
    aex_session_domain::release_mutation_guard(session, operation.id)?;

    let mut head = session.clone();
    let result = match operation.kind {
        OperationKind::SessionCancel => {
            let changed = head.lifecycle.cancel_current(now)?;
            head.active_run = None;
            head.work_admission = aex_session_domain::WorkAdmission::Open;
            canonical_result(&models::SessionCancelResult {
                changed,
                session_id: session.id,
                session_revision: session.revision.next().0,
            })?
        }
        OperationKind::SessionSuspend => {
            let changed = head.lifecycle.status == aex_session_domain::LifecycleStatus::Suspending;
            if changed {
                head.lifecycle.complete_suspend(now)?;
            }
            canonical_result(&models::SessionSuspendResult {
                changed,
                session_id: session.id,
                session_revision: session.revision.next().0,
                suspended_at: head.lifecycle.suspended_at.unwrap_or(now),
            })?
        }
        OperationKind::SessionResume => {
            let changed = head.lifecycle.status == aex_session_domain::LifecycleStatus::Resuming;
            if changed {
                head.lifecycle.complete_resume(now)?;
            }
            canonical_result(&models::SessionResumeResult {
                changed,
                resumed_at: now,
                session_id: session.id,
                session_revision: session.revision.next().0,
                status: crate::projection::public_status(head.lifecycle.status),
            })?
        }
        OperationKind::SessionTerminate => {
            let changed = head.lifecycle.status == aex_session_domain::LifecycleStatus::Terminating;
            if changed {
                head.lifecycle.complete_terminate(now)?;
            }
            canonical_result(&models::SessionTerminateResult {
                changed,
                session_id: session.id,
                session_revision: session.revision.next().0,
                terminated_at: head.lifecycle.terminated_at.unwrap_or(now),
            })?
        }
        OperationKind::SessionDelete => {
            return Err(AppError::Port(PortError::Unowned {
                kind: "lifecycle operation settlement",
                seam: "session deletion requires its bounded multi-owner evidence cascade",
            }));
        }
        OperationKind::WorkspaceDelete
        | OperationKind::TelemetryExport
        | OperationKind::ContentGc => return Err(not_session_kind()),
    };

    head.status = head.lifecycle.status;
    head.revision = session.revision.next();
    head.mutation_guard = None;
    head.updated_at = now;
    let succeeded = succeed(operation, result, now)?.operation;
    let plan = SessionTransaction {
        intent: TransactionIntent::SettleLifecycle,
        conditions: vec![
            Condition::SessionRevision {
                session: session.id,
                expected: session.revision,
            },
            Condition::MutationGuardHeldBy {
                session: session.id,
                holder: operation.id,
            },
            Condition::OperationVersion {
                operation: operation.id,
                expected: operation_version,
            },
        ],
        writes: vec![
            Write::PutSessionHead(Box::new(head)),
            Write::PutOperation(Box::new(succeeded.clone())),
            Write::CompleteWorkItem(Box::new(WorkCompletion {
                work_id: claim.work_id.clone(),
                fence: claim.fence,
                owner: claim.owner.clone(),
                at: now,
            })),
        ],
        after_commit: Vec::new(),
    };
    plan.validate()?;
    Ok(Planned {
        plan,
        projected: succeeded,
    })
}

fn canonical_result(value: &impl serde::Serialize) -> Result<OperationResult, AppError> {
    Ok(OperationResult {
        measurement: None,
        content: Some(CanonicalJson::parse(&to_jcs_string(value)?)?),
    })
}

/// Resolves replay first, then plans one lifecycle admission.
///
/// # Errors
///
/// Returns [`AppError`] for a conflicting operation identity, account pause,
/// illegal lifecycle transition or authority read failure.
pub async fn admit_lifecycle_operation(
    context: &AppContext<'_>,
    command: &LifecycleCommand,
) -> Result<LifecycleAdmissionOutcome, AppError> {
    ensure_session_kind(command.kind)?;
    if let Some(stored) = context
        .sessions
        .load_operation(command.workspace, command.operation)
        .await?
    {
        return match admit(
            Some(&stored.operation),
            None,
            &admit_request(command),
            context.clock.now(),
        ) {
            AdmissionOutcome::Replay(operation) => {
                Ok(LifecycleAdmissionOutcome::Replayed(*operation))
            }
            AdmissionOutcome::Conflict(conflict) => Err(AppError::Conflict(conflict.code())),
            other => Err(AppError::Port(PortError::Corrupt {
                kind: "lifecycle operation replay",
                reason: match other {
                    AdmissionOutcome::Inserted(_) => "an existing operation replay inserted",
                    AdmissionOutcome::DeletionInProgress { .. } => {
                        "receipt-first replay consulted deletion state"
                    }
                    AdmissionOutcome::SessionDeleted { .. } => {
                        "receipt-first replay consulted a tombstone"
                    }
                    AdmissionOutcome::Replay(_) | AdmissionOutcome::Conflict(_) => unreachable!(),
                },
            })),
        };
    }

    let session = context
        .sessions
        .load_session(command.workspace, command.session)
        .await?;
    let account = context.accounts.projection(session.organization).await?;
    pause_gate(command_class(command.kind), &account)?;
    plan_lifecycle_admission(command, &session, account.revision, context.clock.now())
        .map(LifecycleAdmissionOutcome::Planned)
}

fn plan_lifecycle_admission(
    command: &LifecycleCommand,
    session: &Session,
    account_revision: aex_session_domain::AccountRevision,
    now: Timestamp,
) -> Result<Planned<Operation>, AppError> {
    let operation = match admit(None, Some(&session.deletion), &admit_request(command), now) {
        AdmissionOutcome::Inserted(operation) => *operation,
        AdmissionOutcome::Conflict(conflict) => return Err(AppError::Conflict(conflict.code())),
        AdmissionOutcome::DeletionInProgress { operation } => {
            return Err(AppError::Deletion(
                aex_session_domain::DeletionRejection::InProgress(operation),
            ));
        }
        AdmissionOutcome::SessionDeleted { operation } => {
            return Err(AppError::Deletion(
                aex_session_domain::DeletionRejection::Deleted(operation),
            ));
        }
        AdmissionOutcome::Replay(_) => {
            return Err(AppError::Port(PortError::Corrupt {
                kind: "lifecycle operation admission",
                reason: "an absent operation replayed",
            }));
        }
    };

    let mut head = session.clone();
    head.mutation_guard = Some(acquire_mutation_guard(
        session,
        command.operation,
        command.kind,
        now,
    )?);
    match command.kind {
        OperationKind::SessionCancel => {
            let active = session.active_run.is_some() || session.lifecycle.active.is_some();
            let fence = cancel_session_fence(session, CancelCause::SessionCancel, active);
            head.cancellation = fence.cancellation;
            head.work_admission = fence.admission;
        }
        OperationKind::SessionSuspend => {
            head.lifecycle.begin_suspend()?;
        }
        OperationKind::SessionResume => {
            head.lifecycle.begin_resume()?;
        }
        OperationKind::SessionTerminate => {
            let active = session.active_run.is_some() || session.lifecycle.active.is_some();
            let fence = cancel_session_fence(session, CancelCause::SessionCancel, active);
            head.cancellation = fence.cancellation;
            head.work_admission = fence.admission;
            head.lifecycle.begin_terminate(TerminationReason::User)?;
            head.active_run = None;
        }
        OperationKind::SessionDelete => {
            let deletion = begin_delete(&session.deletion, command.operation)?;
            head.deletion = deletion.guard;
            head.work_admission = deletion.admission;
            let active = session.active_run.is_some() || session.lifecycle.active.is_some();
            let fence = cancel_session_fence(session, CancelCause::SessionDeleting, active);
            head.cancellation = fence.cancellation;
            if session.lifecycle.status == aex_session_domain::LifecycleStatus::Terminated {
                head.lifecycle.begin_delete()?;
            } else {
                head.lifecycle.begin_terminate(TerminationReason::User)?;
                head.active_run = None;
            }
        }
        OperationKind::WorkspaceDelete
        | OperationKind::TelemetryExport
        | OperationKind::ContentGc => return Err(not_session_kind()),
    }
    head.status = head.lifecycle.status;
    head.revision = session.revision.next();
    head.updated_at = now;

    let work_id = WorkId(command.operation.uuid7());
    let work = WorkItem {
        id: work_id,
        operation: command.operation,
        kind: command.kind,
        due_at: now,
        priority: LIFECYCLE_WORK_PRIORITY,
        attempt: 0,
        max_attempts: LIFECYCLE_MAX_ATTEMPTS,
        lease: None,
        state: WorkState::Runnable,
        dedup: DedupIdentity {
            operation: command.operation,
            step: 0,
        },
        cancel_requested: false,
    };

    let operation_write = Write::PutOperation(Box::new(operation.clone()));
    let work_write = Write::PutWorkItem(Box::new(work));
    let mut conditions = vec![
        Condition::SessionRevision {
            session: session.id,
            expected: session.revision,
        },
        Condition::DeletionState {
            session: session.id,
            expected: session.deletion.state,
            epoch: session.deletion.epoch,
        },
        Condition::CancellationEpoch {
            session: session.id,
            expected: session.cancellation,
        },
        Condition::AccountRevisionAtLeast {
            organization: session.organization,
            at_least: account_revision,
        },
        Condition::ItemAbsent(operation_write.target()),
        Condition::ItemAbsent(work_write.target()),
    ];
    conditions.push(match session.mutation_guard {
        Some(guard) => Condition::MutationGuardHeldBy {
            session: session.id,
            holder: guard.holder,
        },
        None => Condition::MutationGuardFree {
            session: session.id,
        },
    });
    let plan = SessionTransaction {
        intent: transaction_intent(command.kind)?,
        conditions,
        writes: vec![
            operation_write,
            Write::PutSessionHead(Box::new(head)),
            work_write,
        ],
        after_commit: vec![Hint::OperationDue {
            operation: command.operation,
            due_at: now,
        }],
    };
    plan.validate()?;
    Ok(Planned {
        plan,
        projected: operation,
    })
}

fn admit_request(command: &LifecycleCommand) -> AdmitRequest {
    AdmitRequest {
        id: command.operation,
        workspace: command.workspace,
        session: Some(command.session),
        kind: command.kind,
        intent: command.intent,
        scope: OperationScope::Session(command.session),
        inline_result: None,
        execution: None,
    }
}

const fn command_class(kind: OperationKind) -> CommandClass {
    match kind {
        OperationKind::SessionResume => CommandClass::PausableMutation,
        OperationKind::SessionCancel
        | OperationKind::SessionSuspend
        | OperationKind::SessionTerminate
        | OperationKind::SessionDelete => CommandClass::PauseExempt,
        OperationKind::WorkspaceDelete
        | OperationKind::TelemetryExport
        | OperationKind::ContentGc => CommandClass::PausableMutation,
    }
}

const fn transaction_intent(kind: OperationKind) -> Result<TransactionIntent, AppError> {
    Ok(match kind {
        OperationKind::SessionCancel => TransactionIntent::CancelSession,
        OperationKind::SessionSuspend => TransactionIntent::SuspendSession,
        OperationKind::SessionResume => TransactionIntent::ResumeSession,
        OperationKind::SessionTerminate => TransactionIntent::TerminateSession,
        OperationKind::SessionDelete => TransactionIntent::DeleteSession,
        OperationKind::WorkspaceDelete
        | OperationKind::TelemetryExport
        | OperationKind::ContentGc => return Err(not_session_kind()),
    })
}

fn ensure_session_kind(kind: OperationKind) -> Result<(), AppError> {
    transaction_intent(kind).map(|_| ())
}

const fn not_session_kind() -> AppError {
    AppError::Port(PortError::Corrupt {
        kind: "session lifecycle operation",
        reason: "the command kind is not a session lifecycle operation",
    })
}

#[cfg(test)]
mod tests {
    use aex_operation_domain::OperationKind;
    use aex_session_domain::{LifecycleStatus, WorkAdmission};
    use aex_wire::idempotency::IntentDigest;
    use aex_wire::ids::{OperationId, PrefixedId as _, Uuid7};

    use super::*;

    fn command(kind: OperationKind) -> LifecycleCommand {
        let session = aex_session_domain::testing::session_fixture();
        LifecycleCommand {
            workspace: session.workspace,
            session: session.id,
            operation: OperationId::from_uuid7(Uuid7::compose(2, [4; 10])),
            intent: IntentDigest::from_bytes([7; 32]),
            kind,
        }
    }

    fn now() -> Timestamp {
        aex_session_domain::testing::moment(10)
    }

    fn planned(kind: OperationKind, session: &Session) -> Planned<Operation> {
        plan_lifecycle_admission(
            &command(kind),
            session,
            aex_session_domain::AccountRevision(7),
            now(),
        )
        .expect("plans")
    }

    fn planned_head(plan: &SessionTransaction) -> &Session {
        plan.writes
            .iter()
            .find_map(|write| match write {
                Write::PutSessionHead(head) => Some(head.as_ref()),
                _ => None,
            })
            .expect("head write")
    }

    #[test]
    fn suspend_and_resume_publish_only_truthful_transitional_states() {
        let idle = aex_session_domain::testing::session_fixture();
        let suspend = planned(OperationKind::SessionSuspend, &idle);
        assert_eq!(
            planned_head(&suspend.plan).lifecycle.status,
            LifecycleStatus::Suspending
        );

        let mut suspended = idle;
        suspended.lifecycle.begin_suspend().expect("begins");
        suspended
            .lifecycle
            .complete_suspend(now())
            .expect("suspends");
        suspended.status = suspended.lifecycle.status;
        let resume = planned(OperationKind::SessionResume, &suspended);
        assert_eq!(
            planned_head(&resume.plan).lifecycle.status,
            LifecycleStatus::Resuming
        );
    }

    #[test]
    fn cancel_and_terminate_can_fence_active_work() {
        let (running, _, _, _) = aex_session_domain::testing::running_session();
        let cancel = planned(OperationKind::SessionCancel, &running);
        let cancelling = planned_head(&cancel.plan);
        assert_eq!(cancelling.lifecycle.status, LifecycleStatus::Running);
        assert!(cancelling.cancellation > running.cancellation);

        let terminate = planned(OperationKind::SessionTerminate, &running);
        let terminating = planned_head(&terminate.plan);
        assert_eq!(terminating.lifecycle.status, LifecycleStatus::Terminating);
        assert_eq!(terminating.active_run, None);
    }

    #[test]
    fn delete_closes_admission_before_provider_cleanup() {
        let live = aex_session_domain::testing::session_fixture();
        let delete = planned(OperationKind::SessionDelete, &live);
        let deleting = planned_head(&delete.plan);
        assert_eq!(deleting.work_admission, WorkAdmission::Deleting);
        assert_eq!(
            deleting.deletion.state,
            aex_operation_domain::DeletionState::Deleting
        );
        assert_eq!(deleting.lifecycle.status, LifecycleStatus::Terminating);
    }

    #[test]
    fn one_operation_elects_one_deterministic_work_item() {
        let session = aex_session_domain::testing::session_fixture();
        let command = command(OperationKind::SessionSuspend);
        let planned = planned(OperationKind::SessionSuspend, &session);
        let work = planned
            .plan
            .writes
            .iter()
            .find_map(|write| match write {
                Write::PutWorkItem(work) => Some(work.as_ref()),
                _ => None,
            })
            .expect("work write");
        assert_eq!(work.id, WorkId(command.operation.uuid7()));
        assert_eq!(work.dedup.operation, command.operation);
        assert_eq!(work.dedup.step, 0);
        assert_eq!(work.state, WorkState::Runnable);
        assert!(planned.plan.writes.iter().any(|write| {
            matches!(write, Write::PutOperation(_))
                && write.family() == crate::plan::TableFamily::OperationAuthority
        }));
        assert_eq!(
            planned
                .plan
                .writes
                .iter()
                .find(|write| matches!(write, Write::PutWorkItem(_)))
                .map(Write::family),
            Some(crate::plan::TableFamily::WorkAuthority)
        );
    }

    #[test]
    fn lifecycle_success_releases_the_guard_and_retires_the_same_work_claim() {
        let session = aex_session_domain::testing::session_fixture();
        let admitted = planned(OperationKind::SessionSuspend, &session);
        let transitional = planned_head(&admitted.plan).clone();
        let claim = LifecycleWorkClaim {
            work_id: WorkId(admitted.projected.id.uuid7()).to_string(),
            fence: 3,
            owner: "session-operation-worker:test".to_owned(),
        };
        let settled = settle_lifecycle_operation(
            &transitional,
            &admitted.projected,
            OperationVersion::FIRST,
            &claim,
            aex_session_domain::testing::moment(20),
        )
        .expect("settles");
        let head = planned_head(&settled.plan);
        assert_eq!(head.lifecycle.status, LifecycleStatus::Suspended);
        assert_eq!(head.mutation_guard, None);
        assert_eq!(
            settled.projected.status,
            aex_operation_domain::OperationStatus::Succeeded
        );
        assert!(settled.plan.writes.iter().any(|write| {
            matches!(write, Write::CompleteWorkItem(completion) if completion.work_id == claim.work_id && completion.fence == claim.fence)
        }));
        assert_eq!(settled.plan.validate().expect("valid").actions, 3);
    }

    #[test]
    fn cancellation_returns_the_same_session_to_idle() {
        let (running, _, _, _) = aex_session_domain::testing::running_session();
        let admitted = planned(OperationKind::SessionCancel, &running);
        let transitional = planned_head(&admitted.plan).clone();
        let claim = LifecycleWorkClaim {
            work_id: WorkId(admitted.projected.id.uuid7()).to_string(),
            fence: 1,
            owner: "session-operation-worker:test".to_owned(),
        };
        let settled = settle_lifecycle_operation(
            &transitional,
            &admitted.projected,
            OperationVersion::FIRST,
            &claim,
            aex_session_domain::testing::moment(20),
        )
        .expect("settles");
        let head = planned_head(&settled.plan);
        assert_eq!(head.lifecycle.status, LifecycleStatus::Idle);
        assert_eq!(head.active_run, None);
        assert_eq!(head.work_admission, WorkAdmission::Open);
    }
}
