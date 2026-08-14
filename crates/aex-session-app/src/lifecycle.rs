//! Admission plans for the five public session lifecycle operations.
//!
//! Admission never performs a provider effect. It elects the caller's operation,
//! takes the session mutation guard, records the truthful transitional state and
//! inserts one deterministic runnable work row in the same transaction. A worker
//! must observe the exact retained generation before it publishes a terminal
//! operation result.

use aex_operation_domain::operation::OperationVersion;
use aex_operation_domain::{
    AdmissionOutcome, AdmitRequest, DedupIdentity, FailureClass, Operation, OperationFailure,
    OperationKind, OperationResult, OperationScope, Progress, WorkId, WorkItem, WorkState, admit,
    fail, succeed,
};
use aex_session_domain::{
    CancelCause, CommandClass, DeleteEvidence, Session, TerminationReason, acquire_mutation_guard,
    begin_delete, cancel_session_fence, complete_delete, pause_gate,
};
use aex_wire::error::ErrorCode;
use aex_wire::idempotency::IntentDigest;
use aex_wire::ids::{OperationId, PrefixedId as _, SessionId, WorkspaceId};
use aex_wire::types::Timestamp;
use aex_wire::{CanonicalJson, canonical::to_jcs_string, models};

use crate::error::AppError;
use crate::plan::{
    Condition, Hint, Planned, RootStopReason, SessionTransaction, TransactionIntent,
    WorkCompletion, Write,
};
use crate::ports::{AppContext, PortError, RootAdmissionState};

// `regional-work` has five bounded priority bands, zero highest. Customer
// lifecycle control must outrank background cleanup and reconciliation.
const LIFECYCLE_WORK_PRIORITY: u16 = 0;
const LIFECYCLE_MAX_ATTEMPTS: u16 = 5;

fn lowercase_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

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
///
/// # Errors
///
/// Returns [`AppError`] when the session, operation and work claim disagree,
/// the lifecycle transition is invalid, or the atomic plan is not submittable.
#[expect(
    clippy::too_many_lines,
    reason = "the closed lifecycle result mapping and its one atomic barrier remain auditable together"
)]
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
            if head.active_run.is_some() || head.lifecycle.active.is_some() {
                return Err(AppError::Port(PortError::Corrupt {
                    kind: "session cancellation settlement",
                    reason: "the Brain RunFinished barrier has not cleared current work",
                }));
            }
            head.lifecycle.cancel_current(now)?;
            head.active_run = None;
            head.work_admission = aex_session_domain::WorkAdmission::Open;
            canonical_result(&models::SessionCommandReceipt {
                accepted_at: now,
                operation_id: operation.id,
                session_id: session.id,
            })?
        }
        OperationKind::SessionSuspend => {
            let changed = head.lifecycle.status == aex_session_domain::LifecycleStatus::Suspending;
            if changed {
                head.lifecycle.complete_suspend(now)?;
            }
            canonical_result(&models::SessionCommandReceipt {
                accepted_at: now,
                operation_id: operation.id,
                session_id: session.id,
            })?
        }
        OperationKind::SessionResume => {
            let changed = head.lifecycle.status == aex_session_domain::LifecycleStatus::Resuming;
            if changed {
                head.lifecycle.complete_resume(now)?;
            }
            canonical_result(&models::SessionCommandReceipt {
                accepted_at: now,
                operation_id: operation.id,
                session_id: session.id,
            })?
        }
        OperationKind::SessionTerminate => {
            let changed = head.lifecycle.status == aex_session_domain::LifecycleStatus::Terminating;
            if changed {
                head.lifecycle.complete_terminate(now)?;
            }
            canonical_result(&models::SessionCommandReceipt {
                accepted_at: now,
                operation_id: operation.id,
                session_id: session.id,
            })?
        }
        OperationKind::SessionDelete => {
            return Err(AppError::Port(PortError::Unowned {
                kind: "lifecycle operation settlement",
                seam: "session deletion requires its bounded multi-owner evidence cascade",
            }));
        }
        OperationKind::WorkspaceDelete | OperationKind::ContentGc => return Err(not_session_kind()),
    };

    head.status = head.lifecycle.status;
    head.revision = session.revision.next();
    head.mutation_guard = None;
    head.updated_at = now;
    let mut completed_operation = operation.clone();
    completed_operation.progress = None;
    let succeeded = succeed(&completed_operation, result, now)?.operation;
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

/// Plans the canonical public result for a fully evidenced irreversible delete.
///
/// This is intentionally only the application projection. The production
/// deletion committer must compile the head replacement through
/// `aex-session-dynamodb::deletion::tombstone_put` and append the exact eight
/// evidence/progress removals to the same operation/work transaction. The
/// generic application-plan compiler is not a deletion authority.
///
/// # Errors
///
/// Returns [`AppError`] unless the operation owns the exact deletion guard and
/// every independent owner fact is present.
pub fn settle_session_delete(
    head: &aex_session_domain::SessionDeletionHead,
    operation: &Operation,
    operation_version: OperationVersion,
    claim: &LifecycleWorkClaim,
    evidence: &DeleteEvidence,
    now: Timestamp,
) -> Result<Planned<Operation>, AppError> {
    if operation.workspace != head.workspace
        || operation.session != Some(head.session)
        || operation.scope != OperationScope::Session(head.session)
        || operation.kind != OperationKind::SessionDelete
        || operation.id != head.operation
        || claim.work_id != WorkId(operation.id.uuid7()).to_string()
    {
        return Err(AppError::Port(PortError::Corrupt {
            kind: "session deletion settlement",
            reason: "session, operation and work binding disagree",
        }));
    }
    let guard = aex_session_domain::DeletionGuard {
        session: head.session,
        state: aex_session_domain::DeletionState::Deleting,
        epoch: head.epoch,
        delete_operation: Some(head.operation),
    };
    let tombstone = complete_delete(&guard, head.workspace, evidence, now)?;
    let result = canonical_result(&models::SessionTombstone {
        deleted_at: tombstone.deleted_at,
        id: tombstone.session,
    })?;
    let succeeded = succeed(operation, result, now)?.operation;
    let plan = SessionTransaction {
        intent: TransactionIntent::SettleLifecycle,
        conditions: vec![
            Condition::SessionRevision {
                session: head.session,
                expected: head.revision,
            },
            Condition::MutationGuardHeldBy {
                session: head.session,
                holder: operation.id,
            },
            Condition::OperationVersion {
                operation: operation.id,
                expected: operation_version,
            },
        ],
        writes: vec![
            Write::PutTombstone(Box::new(tombstone)),
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

/// Atomically terminalizes a session whose exact generation was observed lost.
///
/// A manual suspend or resume cannot succeed after the retained generation has
/// become absorbing. This barrier publishes the truthful session termination,
/// fails the requested operation, releases its mutation guard and retires the
/// exact work claim together. Explicit terminate uses
/// [`settle_lifecycle_operation`] instead, because a terminal provider is its
/// requested successful effect.
///
/// # Errors
///
/// Returns [`AppError`] when the bound authorities disagree, termination cannot
/// be represented, or the atomic loss-settlement plan is not submittable.
pub fn settle_lifecycle_loss(
    session: &Session,
    operation: &Operation,
    operation_version: OperationVersion,
    claim: &LifecycleWorkClaim,
    reason: TerminationReason,
    at: Timestamp,
) -> Result<Planned<Operation>, AppError> {
    if operation.workspace != session.workspace
        || operation.session != Some(session.id)
        || operation.scope != OperationScope::Session(session.id)
        || claim.work_id != WorkId(operation.id.uuid7()).to_string()
        || !matches!(
            operation.kind,
            OperationKind::SessionSuspend | OperationKind::SessionResume
        )
    {
        return Err(AppError::Port(PortError::Corrupt {
            kind: "lifecycle loss settlement",
            reason: "session, operation, work or operation kind binding disagree",
        }));
    }
    aex_session_domain::release_mutation_guard(session, operation.id)?;

    let mut head = session.clone();
    head.lifecycle.begin_terminate(reason)?;
    head.lifecycle.complete_terminate(at)?;
    head.status = head.lifecycle.status;
    head.active_run = None;
    head.work_admission = aex_session_domain::WorkAdmission::ContinuityLost;
    head.revision = session.revision.next();
    head.mutation_guard = None;
    head.updated_at = at;
    let failed = fail(
        operation,
        OperationFailure::bare(ErrorCode::SessionTerminated, FailureClass::Terminal),
        at,
    )?
    .operation;
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
            Write::PutOperation(Box::new(failed.clone())),
            Write::CompleteWorkItem(Box::new(WorkCompletion {
                work_id: claim.work_id.clone(),
                fence: claim.fence,
                owner: claim.owner.clone(),
                at,
            })),
        ],
        after_commit: Vec::new(),
    };
    plan.validate()?;
    Ok(Planned {
        plan,
        projected: failed,
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

    let (session, root) = if command.kind == OperationKind::SessionCancel {
        let snapshot = context
            .sessions
            .load_message_snapshot(command.workspace, command.session)
            .await?;
        (snapshot.session, Some(snapshot.root))
    } else {
        let loaded = context
            .sessions
            .load_session(command.workspace, command.session)
            .await;
        let session = match loaded {
            Err(PortError::Deleting { operation, .. })
                if command.kind == OperationKind::SessionDelete =>
            {
                return Err(AppError::Deletion(
                    aex_session_domain::DeletionRejection::InProgress(operation),
                ));
            }
            result => result?,
        };
        (session, None)
    };
    let account = context.accounts.projection(session.organization).await?;
    pause_gate(command_class(command.kind), &account)?;
    plan_lifecycle_admission(
        command,
        &session,
        root.as_ref(),
        account.revision,
        context.clock.now(),
    )
    .map(LifecycleAdmissionOutcome::Planned)
}

#[expect(
    clippy::too_many_lines,
    reason = "the exhaustive admission outcome and fixed transaction membership are one closed authority"
)]
fn plan_lifecycle_admission(
    command: &LifecycleCommand,
    session: &Session,
    root: Option<&RootAdmissionState>,
    account_revision: aex_session_domain::AccountRevision,
    now: Timestamp,
) -> Result<Planned<Operation>, AppError> {
    let mut operation = match admit(None, Some(&session.deletion), &admit_request(command), now) {
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
    let mut cancellation_root = None;
    match command.kind {
        OperationKind::SessionCancel => {
            let active = session.active_run.is_some();
            if session.lifecycle.active.map(|activity| activity.run) != session.active_run {
                return Err(AppError::Port(PortError::Corrupt {
                    kind: "session cancellation",
                    reason: "the session head and lifecycle disagree on the current run",
                }));
            }
            let fence = cancel_session_fence(session, CancelCause::SessionCancel, active);
            head.cancellation = fence.cancellation;
            head.work_admission = fence.admission;
            if active {
                operation.progress = Some(Progress {
                    phase: "cancelling_current".to_owned(),
                    processed: 0,
                    total_hint: Some(1),
                });
                let root = root.ok_or(AppError::Port(PortError::Corrupt {
                    kind: "session cancellation",
                    reason: "active work has no strongly bound Brain root",
                }))?;
                if root.agent != session.root_agent || root.idle {
                    return Err(AppError::Port(PortError::Corrupt {
                        kind: "session cancellation",
                        reason: "the active session and Brain root disagree",
                    }));
                }
                cancellation_root = Some((root, fence.cancellation));
            }
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
        OperationKind::WorkspaceDelete | OperationKind::ContentGc => return Err(not_session_kind()),
    }
    head.status = head.lifecycle.status;
    head.revision = session.revision.next();
    head.updated_at = now;

    let head_write = if command.kind == OperationKind::SessionDelete {
        Write::PutSessionDeletionHead(Box::new(aex_session_domain::SessionDeletionHead {
            session: session.id,
            workspace: session.workspace,
            organization: session.organization,
            operation: command.operation,
            epoch: head.deletion.epoch,
            revision: head.revision,
            generation: session.lifecycle.generation,
            started_at: now,
        }))
    } else {
        Write::PutSessionHead(Box::new(head))
    };

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
    let operation_edge_write = Write::PutSessionOperationEdge {
        session: session.id,
        operation: command.operation,
    };
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
        match command_class(command.kind) {
            CommandClass::PauseExempt => Condition::AccountRevisionAtLeast {
                workspace: session.workspace,
                organization: session.organization,
                at_least: account_revision,
            },
            CommandClass::PausableMutation | CommandClass::PausableRead => {
                Condition::AccountActiveAtLeast {
                    workspace: session.workspace,
                    organization: session.organization,
                    at_least: account_revision,
                }
            }
        },
        Condition::ItemAbsent(operation_write.target()),
        Condition::ItemAbsent(operation_edge_write.target()),
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
    let mut writes = vec![
        operation_write,
        operation_edge_write,
        head_write,
        work_write,
    ];
    if matches!(
        command.kind,
        OperationKind::SessionTerminate | OperationKind::SessionDelete
    ) && let Some(run) = session.active_run
    {
        writes.push(Write::DeleteActiveSession {
            workspace: session.workspace,
            session: session.id,
            run,
        });
    }
    if let Some((root, cancellation)) = cancellation_root {
        let wake = cancellation_wake(command, root, cancellation, now)?;
        let root_write = Write::RequestRootCancellation {
            session: session.id,
            agent: root.agent,
            from_revision: root.revision,
            to_revision: root.revision.next(),
            from_cancellation: session.cancellation,
            to_cancellation: cancellation,
            reason: RootStopReason::SessionCancelled,
            at: now,
        };
        conditions.push(Condition::AgentRevision {
            session: session.id,
            agent: root.agent,
            expected: root.revision,
        });
        conditions.push(Condition::ItemAbsent(
            Write::PutAgentWake(Box::new(wake.clone())).target(),
        ));
        conditions.push(Condition::ItemAbsent(
            Write::PutAgentWakeDedupe(Box::new(wake.clone())).target(),
        ));
        writes.extend([
            root_write,
            Write::PutAgentWake(Box::new(wake.clone())),
            Write::PutAgentWakeDedupe(Box::new(wake)),
        ]);
    }
    let plan = SessionTransaction {
        intent: transaction_intent(command.kind)?,
        conditions,
        writes,
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

fn cancellation_wake(
    command: &LifecycleCommand,
    root: &RootAdmissionState,
    cancellation: aex_session_domain::CancellationEpoch,
    now: Timestamp,
) -> Result<crate::plan::AgentWake, AppError> {
    use sha2::Digest as _;

    let suffix = command.operation.uuid7().encode_suffix();
    let suffix = std::str::from_utf8(&suffix).map_err(|_| {
        AppError::Port(PortError::Corrupt {
            kind: "session cancellation wake",
            reason: "an operation UUIDv7 did not render as Crockford ASCII",
        })
    })?;
    let mut digest = sha2::Sha256::new();
    digest.update(b"aex.agent.cancel.wake.v1\0");
    digest.update(command.session.to_string().as_bytes());
    digest.update(b"\0");
    digest.update(root.agent.to_string().as_bytes());
    digest.update(b"\0");
    digest.update(command.operation.to_string().as_bytes());
    Ok(crate::plan::AgentWake {
        work_id: format!("wrk_cancel{suffix}"),
        dedupe_key: lowercase_hex(&digest.finalize()),
        session: command.session,
        agent: root.agent,
        from: root.journal_tail,
        cancellation,
        at: now,
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
        OperationKind::SessionCancel
        | OperationKind::SessionSuspend
        | OperationKind::SessionTerminate
        | OperationKind::SessionDelete => CommandClass::PauseExempt,
        OperationKind::SessionResume
        | OperationKind::WorkspaceDelete
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
        OperationKind::WorkspaceDelete | OperationKind::ContentGc => return Err(not_session_kind()),
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
    use aex_session_domain::{DeleteEvidence, LifecycleStatus, WorkAdmission};
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
        let root = (kind == OperationKind::SessionCancel && session.active_run.is_some())
            .then_some({
                RootAdmissionState {
                    agent: session.root_agent,
                    revision: aex_session_domain::AgentRevision(3),
                    journal_tail: aex_session_domain::JournalSeq(7),
                    limits_revision: 4,
                    max_run_duration_ms: 28_800_000,
                    idle: false,
                }
            });
        plan_lifecycle_admission(
            &command(kind),
            session,
            root.as_ref(),
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

    fn planned_deletion_head(
        plan: &SessionTransaction,
    ) -> &aex_session_domain::SessionDeletionHead {
        plan.writes
            .iter()
            .find_map(|write| match write {
                Write::PutSessionDeletionHead(head) => Some(head.as_ref()),
                _ => None,
            })
            .expect("deletion head write")
    }

    fn complete_delete_evidence() -> DeleteEvidence {
        DeleteEvidence {
            generation_terminated: true,
            session_content_removed: true,
            messages_removed: true,
            brain_user_content_removed: true,
            billing_aggregate_retained: true,
            audit_fact_retained: true,
        }
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
    fn pause_exempt_cleanup_uses_an_epoch_fence_while_resume_requires_active() {
        let session = aex_session_domain::testing::session_fixture();
        for kind in [
            OperationKind::SessionCancel,
            OperationKind::SessionSuspend,
            OperationKind::SessionTerminate,
            OperationKind::SessionDelete,
        ] {
            let planned = planned(kind, &session);
            assert!(
                planned
                    .plan
                    .conditions
                    .iter()
                    .any(|condition| matches!(condition, Condition::AccountRevisionAtLeast { .. }))
            );
            assert!(
                !planned
                    .plan
                    .conditions
                    .iter()
                    .any(|condition| matches!(condition, Condition::AccountActiveAtLeast { .. }))
            );
        }

        let mut suspended = session;
        suspended.lifecycle.begin_suspend().expect("begins");
        suspended
            .lifecycle
            .complete_suspend(now())
            .expect("suspends");
        suspended.status = suspended.lifecycle.status;
        let resume = planned(OperationKind::SessionResume, &suspended);
        assert!(
            resume
                .plan
                .conditions
                .iter()
                .any(|condition| matches!(condition, Condition::AccountActiveAtLeast { .. }))
        );
    }

    #[test]
    fn cancel_and_terminate_can_fence_active_work() {
        let (running, _, _, _) = aex_session_domain::testing::running_session();
        let cancel = planned(OperationKind::SessionCancel, &running);
        let cancelling = planned_head(&cancel.plan);
        assert_eq!(cancelling.lifecycle.status, LifecycleStatus::Running);
        assert!(cancelling.cancellation > running.cancellation);
        assert!(cancel.plan.writes.iter().any(|write| matches!(
            write,
            Write::RequestRootCancellation {
                from_cancellation,
                to_cancellation,
                ..
            } if *from_cancellation == running.cancellation
                && *to_cancellation == running.cancellation.next()
        )));
        assert_eq!(
            cancel
                .plan
                .writes
                .iter()
                .filter(|write| matches!(
                    write,
                    Write::PutAgentWake(_) | Write::PutAgentWakeDedupe(_)
                ))
                .count(),
            2
        );
        assert_eq!(cancel.plan.validate().expect("valid").actions, 8);

        let terminate = planned(OperationKind::SessionTerminate, &running);
        let terminating = planned_head(&terminate.plan);
        assert_eq!(terminating.lifecycle.status, LifecycleStatus::Terminating);
        assert_eq!(terminating.active_run, None);
    }

    #[test]
    fn delete_closes_admission_before_provider_cleanup() {
        let live = aex_session_domain::testing::session_fixture();
        let delete = planned(OperationKind::SessionDelete, &live);
        let deleting = planned_deletion_head(&delete.plan);
        assert_eq!(deleting.session, live.id);
        assert_eq!(deleting.generation, live.lifecycle.generation);
        assert_eq!(deleting.operation, delete.projected.id);
        assert_eq!(deleting.epoch, live.deletion.epoch.next());
        assert!(
            !delete
                .plan
                .writes
                .iter()
                .any(|write| matches!(write, Write::PutSessionHead(_)))
        );
    }

    #[test]
    fn deletion_completion_requires_all_eight_owner_facts_and_targets_head() {
        let live = aex_session_domain::testing::session_fixture();
        let admitted = planned(OperationKind::SessionDelete, &live);
        let deleting = *planned_deletion_head(&admitted.plan);
        let claim = LifecycleWorkClaim {
            work_id: WorkId(admitted.projected.id.uuid7()).to_string(),
            fence: 9,
            owner: "session-operation-worker:test".to_owned(),
        };
        let settled = settle_session_delete(
            &deleting,
            &admitted.projected,
            OperationVersion::FIRST,
            &claim,
            &complete_delete_evidence(),
            aex_session_domain::testing::moment(20),
        )
        .expect("settles complete evidence");
        let tombstone = settled
            .plan
            .writes
            .iter()
            .find_map(|write| match write {
                Write::PutTombstone(tombstone) => Some(tombstone.as_ref()),
                _ => None,
            })
            .expect("minimal tombstone");
        assert_eq!(tombstone.session, deleting.session);
        assert_eq!(
            Write::PutTombstone(Box::new(tombstone.clone()))
                .target()
                .sort,
            "HEAD"
        );
        assert_eq!(settled.plan.validate().expect("valid").actions, 3);

        let mut incomplete = complete_delete_evidence();
        incomplete.session_content_removed = false;
        assert!(
            settle_session_delete(
                &deleting,
                &admitted.projected,
                OperationVersion::FIRST,
                &claim,
                &incomplete,
                aex_session_domain::testing::moment(20),
            )
            .is_err()
        );
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
    fn deletion_settlement_writes_only_the_minimal_tombstone_and_terminal_authorities() {
        let session = aex_session_domain::testing::session_fixture();
        let admitted = planned(OperationKind::SessionDelete, &session);
        let deleting = *planned_deletion_head(&admitted.plan);
        let claim = LifecycleWorkClaim {
            work_id: WorkId(admitted.projected.id.uuid7()).to_string(),
            fence: 9,
            owner: "session-operation-worker:test".to_owned(),
        };
        let at = aex_session_domain::testing::moment(20);
        let settled = settle_session_delete(
            &deleting,
            &admitted.projected,
            OperationVersion::FIRST,
            &claim,
            &complete_delete_evidence(),
            at,
        )
        .expect("settles complete deletion evidence");
        let tombstone = settled
            .plan
            .writes
            .iter()
            .find_map(|write| match write {
                Write::PutTombstone(tombstone) => Some(tombstone.as_ref()),
                _ => None,
            })
            .expect("minimal tombstone");
        assert_eq!(tombstone.session, deleting.session);
        assert_eq!(tombstone.workspace, deleting.workspace);
        assert_eq!(tombstone.deleted_by, admitted.projected.id);
        assert_eq!(tombstone.deleted_at, at);
        assert!(
            !settled
                .plan
                .writes
                .iter()
                .any(|write| matches!(write, Write::PutSessionHead(_)))
        );
        assert_eq!(settled.plan.validate().expect("valid").actions, 3);
        assert_eq!(
            settled.projected.status,
            aex_operation_domain::OperationStatus::Succeeded
        );
    }

    #[test]
    fn deletion_settlement_refuses_one_missing_owner_evidence() {
        let session = aex_session_domain::testing::session_fixture();
        let admitted = planned(OperationKind::SessionDelete, &session);
        let deleting = *planned_deletion_head(&admitted.plan);
        let claim = LifecycleWorkClaim {
            work_id: WorkId(admitted.projected.id.uuid7()).to_string(),
            fence: 9,
            owner: "session-operation-worker:test".to_owned(),
        };
        let mut evidence = complete_delete_evidence();
        evidence.session_content_removed = false;
        assert!(
            settle_session_delete(
                &deleting,
                &admitted.projected,
                OperationVersion::FIRST,
                &claim,
                &evidence,
                aex_session_domain::testing::moment(20),
            )
            .is_err()
        );
    }

    #[test]
    fn cancellation_returns_the_same_session_to_idle() {
        let (running, run, _, _) = aex_session_domain::testing::running_session();
        let admitted = planned(OperationKind::SessionCancel, &running);
        let transitional = planned_head(&admitted.plan).clone();
        let mut after_brain = transitional.clone();
        after_brain
            .lifecycle
            .complete_message(run.id, aex_session_domain::testing::moment(15))
            .expect("Brain terminal barrier returns the lifecycle to idle");
        after_brain.status = after_brain.lifecycle.status;
        after_brain.active_run = None;
        after_brain.revision = transitional.revision.next();
        after_brain.updated_at = aex_session_domain::testing::moment(15);
        let claim = LifecycleWorkClaim {
            work_id: WorkId(admitted.projected.id.uuid7()).to_string(),
            fence: 1,
            owner: "session-operation-worker:test".to_owned(),
        };
        let settled = settle_lifecycle_operation(
            &after_brain,
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
        assert!(
            settled
                .projected
                .result
                .as_ref()
                .and_then(|result| result.content.as_ref())
                .is_some_and(|content| {
                    content.as_str().contains("\"operationId\"")
                        && content.as_str().contains("\"sessionId\"")
                })
        );
    }

    #[test]
    fn cancellation_cannot_publish_success_before_brain_clears_current_work() {
        let (running, _, _, _) = aex_session_domain::testing::running_session();
        let admitted = planned(OperationKind::SessionCancel, &running);
        let transitional = planned_head(&admitted.plan).clone();
        let claim = LifecycleWorkClaim {
            work_id: WorkId(admitted.projected.id.uuid7()).to_string(),
            fence: 1,
            owner: "session-operation-worker:test".to_owned(),
        };
        let error = settle_lifecycle_operation(
            &transitional,
            &admitted.projected,
            OperationVersion::FIRST,
            &claim,
            aex_session_domain::testing::moment(20),
        )
        .expect_err("current work still belongs to Brain");
        assert_eq!(error.code(), ErrorCode::InternalError);
    }

    #[test]
    fn a_lost_generation_fails_manual_lifecycle_and_closes_the_session_atomically() {
        let session = aex_session_domain::testing::session_fixture();
        let admitted = planned(OperationKind::SessionSuspend, &session);
        let transitional = planned_head(&admitted.plan).clone();
        let claim = LifecycleWorkClaim {
            work_id: WorkId(admitted.projected.id.uuid7()).to_string(),
            fence: 7,
            owner: "session-operation-worker:test".to_owned(),
        };
        let at = aex_session_domain::testing::moment(20);
        let settled = settle_lifecycle_loss(
            &transitional,
            &admitted.projected,
            OperationVersion::FIRST,
            &claim,
            TerminationReason::RuntimeLost,
            at,
        )
        .expect("settles the observed loss");
        let head = planned_head(&settled.plan);
        assert_eq!(head.lifecycle.status, LifecycleStatus::Terminated);
        assert_eq!(
            head.lifecycle.termination_reason,
            Some(TerminationReason::RuntimeLost)
        );
        assert_eq!(head.lifecycle.terminated_at, Some(at));
        assert_eq!(head.work_admission, WorkAdmission::ContinuityLost);
        assert_eq!(head.mutation_guard, None);
        assert_eq!(
            settled.projected.status,
            aex_operation_domain::OperationStatus::Failed
        );
        assert_eq!(
            settled.projected.error.as_ref().map(|error| error.code),
            Some(ErrorCode::SessionTerminated)
        );
        assert!(settled.plan.writes.iter().any(|write| {
            matches!(write, Write::CompleteWorkItem(completion) if completion.work_id == claim.work_id && completion.fence == claim.fence)
        }));
        assert_eq!(settled.plan.validate().expect("valid").actions, 3);
    }
}
