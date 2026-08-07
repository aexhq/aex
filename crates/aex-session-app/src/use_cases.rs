//! The use cases.
//!
//! Every one of them reads through ports, decides in the domain, and returns
//! exactly one [`SessionTransaction`] plus the projection the caller is told.
//! None of them commits: there is no committer in [`AppContext`], so "one
//! command = one transaction, no external call inside it" is enforced by the
//! type of the context rather than by review.
//!
//! Authorization and the pause gate run **before** idempotent replay, so a
//! paused caller is never re-shown a grant it is no longer entitled to.

use std::collections::BTreeSet;
use std::num::NonZeroU64;

use aex_operation_domain::operation::{OperationResult, OperationScope};
use aex_operation_domain::{AdmissionOutcome, AdmitRequest, DeletionState, OperationKind};
use aex_secret_domain::{CustodyRejection, OwnerKeyEdgeId, SecretName, admit_custody, rebind};
use aex_secret_domain::{SessionCustody, WorkspaceSecret};
use aex_session_domain::{
    CancelCause, CommandClass, DeletionRejection, Message, MessageRole, MessageState, PurgeCascade,
    QueueRun, Run, RunError, Session, SessionStatus, TerminalAttempt, WorkAdmission,
    acquire_mutation_guard, cancel_session_work, claim_terminal, pause_gate, purge, queue, restore,
    start as start_run_domain, trash,
};
use aex_wire::canonical::{CanonicalJson, to_jcs_string};
use aex_wire::error::ErrorCode;
use aex_wire::idempotency::IntentDigest;
use aex_wire::ids::{AgentId, MessageId, OperationId, RunId, SessionId, WorkspaceId};
use aex_wire::models::{CredentialRebindResult, SecretRef};
use aex_wire::types::Timestamp;
use time::Duration;

use crate::error::AppError;
use crate::plan::{Condition, Hint, Planned, SessionTransaction, TransactionIntent, Write};
use crate::ports::{AppContext, ReservationRequest};

/// How long a trashed session may be restored.
pub const RECOVERY_WINDOW: Duration = Duration::days(7);

/// Admit a message and the run it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendMessage {
    /// Which workspace.
    pub workspace: WorkspaceId,
    /// Which session.
    pub session: SessionId,
    /// The message identity the caller minted.
    pub message: MessageId,
    /// The run identity the caller minted.
    pub run: RunId,
    /// The message parts.
    pub parts: Vec<aex_session_domain::MessagePart>,
    /// The spend ceiling.
    pub max_spend_cents: NonZeroU64,
    /// The run deadline.
    pub deadline: Timestamp,
    /// What was asked for.
    pub intent: IntentDigest,
}

/// Start an admitted run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartRun {
    /// Which workspace.
    pub workspace: WorkspaceId,
    /// Which session.
    pub session: SessionId,
    /// Which run.
    pub run: RunId,
}

/// Settle a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitTerminal {
    /// Which workspace.
    pub workspace: WorkspaceId,
    /// Which session.
    pub session: SessionId,
    /// The run and its outcome.
    pub attempt: TerminalAttempt,
    /// The agent that produced it.
    pub agent: AgentId,
    /// The open messages of that run.
    pub open_messages: Vec<Message>,
}

/// A whole-session command that names only its session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionCommand {
    /// Which workspace.
    pub workspace: WorkspaceId,
    /// Which session.
    pub session: SessionId,
    /// The operation identity the caller minted.
    pub operation: OperationId,
    /// What was asked for.
    pub intent: IntentDigest,
}

/// Purge a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Purge {
    /// The command envelope.
    pub command: SessionCommand,
    /// How descendants are treated.
    pub cascade: PurgeCascade,
    /// The immutable closure the fence supplied.
    pub closure: Vec<SessionId>,
}

/// Replace the session's credential custody set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rebind {
    /// The durable operation envelope.
    pub command: SessionCommand,
    /// The exact workspace secret names to bind.
    pub secrets: Vec<SecretName>,
}

struct PreparedRebind {
    custody: SessionCustody,
    selected: Vec<WorkspaceSecret>,
    destroy_key_edges: Vec<OwnerKeyEdgeId>,
}

async fn gate(
    context: &AppContext<'_>,
    session: &Session,
    class: CommandClass,
) -> Result<(), AppError> {
    // Authorization and the pause gate run before any replay lookup.
    let projection = context.accounts.projection(session.organization).await?;
    pause_gate(class, &projection)?;
    Ok(())
}

fn live_conditions(session: &Session) -> Vec<Condition> {
    vec![
        Condition::SessionRevision {
            session: session.id,
            expected: session.revision,
        },
        Condition::DeletionState {
            session: session.id,
            expected: DeletionState::Live,
            epoch: session.deletion.epoch,
        },
    ]
}

/// Admits a message and the run it starts.
///
/// The plan carries the message, the run, the session advance, the root wake,
/// the idempotency receipt and the outbox event **together**: no history yields
/// a durable message without a wake.
///
/// # Errors
///
/// Returns [`AppError`] when the account is paused, a port read fails, or the
/// domain refuses the admission.
pub async fn admit_message(
    context: &AppContext<'_>,
    command: &SendMessage,
) -> Result<Planned<(Message, Run)>, AppError> {
    let snapshot = context
        .sessions
        .load_session(command.workspace, command.session)
        .await?;
    gate(context, &snapshot.session, CommandClass::PausableMutation).await?;

    let grant = context
        .reservations
        .prepare(ReservationRequest {
            organization: snapshot.session.organization,
            workspace: command.workspace,
            max_spend_cents: command.max_spend_cents.get(),
        })
        .await?;

    let now = context.clock.now();
    let commit = queue(
        command.run,
        &QueueRun {
            message: command.message,
            max_spend_cents: command.max_spend_cents,
            reservation: grant.reservation,
            deadline: command.deadline,
        },
        &snapshot.session,
        now,
    )?;

    let message = Message {
        id: command.message,
        session: command.session,
        run: Some(command.run),
        agent: snapshot.root_agent.id,
        role: MessageRole::User,
        // A user message is born sealed.
        state: MessageState::Sealed,
        parts: command.parts.clone(),
        created_at: now,
        sealed_at: Some(now),
    };

    let mut head = snapshot.session.clone();
    head.status = SessionStatus::Running;
    head.active_run = Some(command.run);
    head.revision = snapshot.session.revision.next();
    head.updated_at = now;

    let mut conditions = live_conditions(&snapshot.session);
    conditions.push(Condition::SessionActiveRun {
        session: command.session,
        expected: None,
    });
    conditions.push(Condition::WorkAdmission {
        session: command.session,
        expected: WorkAdmission::Open,
    });
    conditions.push(Condition::MutationGuardFree {
        session: command.session,
    });
    conditions.push(Condition::CancellationEpoch {
        session: command.session,
        expected: snapshot.session.cancellation,
    });
    conditions.push(Condition::ReservationOpen {
        reservation: grant.reservation,
    });

    let plan = SessionTransaction {
        intent: TransactionIntent::AdmitMessage,
        conditions,
        writes: vec![
            Write::AppendMessage(Box::new(message.clone())),
            Write::PutRun(Box::new(commit.run.clone())),
            Write::PutSessionHead(Box::new(head)),
        ],
        after_commit: vec![Hint::WakeAgent {
            agent: snapshot.root_agent.id,
            reason: aex_internal_contracts::wake::WakeHint::SessionWork {
                session: command.session,
            },
        }],
    };
    plan.validate()?;

    Ok(Planned {
        plan,
        projected: (message, commit.run),
    })
}

/// Starts an admitted run.
///
/// # Errors
///
/// Returns [`AppError`] when the account is paused, a port read fails, or the
/// run cannot start.
pub async fn start_run(
    context: &AppContext<'_>,
    command: &StartRun,
) -> Result<Planned<Run>, AppError> {
    let snapshot = context
        .sessions
        .load_session(command.workspace, command.session)
        .await?;
    gate(context, &snapshot.session, CommandClass::PausableMutation).await?;

    let stored = context
        .sessions
        .load_run(command.session, command.run)
        .await?;
    if snapshot.session.active_run != Some(command.run) {
        return Err(AppError::Run(RunError::SessionBusy {
            active: snapshot.session.active_run.unwrap_or(command.run),
        }));
    }

    let commit = start_run_domain(&stored, &snapshot.session, context.clock.now())?;

    let mut conditions = live_conditions(&snapshot.session);
    conditions.push(Condition::RunNonTerminal { run: command.run });
    conditions.push(Condition::SessionActiveRun {
        session: command.session,
        expected: Some(command.run),
    });

    let plan = SessionTransaction {
        intent: TransactionIntent::StartRun,
        conditions,
        // An idempotent start writes nothing and emits no `run.started` fact.
        writes: if commit.changed {
            vec![Write::PutRun(Box::new(commit.run.clone()))]
        } else {
            Vec::new()
        },
        after_commit: Vec::new(),
    };
    plan.validate()?;

    Ok(Planned {
        plan,
        projected: commit.run,
    })
}

/// Settles a run through the terminal barrier.
///
/// # Errors
///
/// Returns [`AppError`] when a port read fails or this attempt is not the
/// winner. A losing attempt produces no plan at all.
pub async fn commit_terminal(
    context: &AppContext<'_>,
    command: &CommitTerminal,
) -> Result<Planned<Run>, AppError> {
    let snapshot = context
        .sessions
        .load_session(command.workspace, command.session)
        .await?;
    let agent = context
        .sessions
        .load_agent(command.session, command.agent)
        .await?;
    let stored = context
        .sessions
        .load_run(command.session, command.attempt.run)
        .await?;

    let commit = claim_terminal(
        &stored,
        &snapshot.session,
        agent.id,
        agent.fence(),
        &command.open_messages,
        &command.attempt,
    )?;

    let mut writes = vec![
        Write::PutRun(Box::new(commit.run.clone())),
        Write::PutAgentControl(Box::new(agent.clone())),
        Write::PutSessionHead(Box::new(commit.session.clone())),
        Write::PutOutboxEvent(Box::new(commit.outbox.clone())),
    ];
    writes.extend(
        commit
            .sealed_messages
            .iter()
            .map(|message| Write::SealMessage(message.id)),
    );

    let plan = SessionTransaction {
        intent: TransactionIntent::CommitTerminal,
        conditions: vec![
            Condition::RunNonTerminal {
                run: command.attempt.run,
            },
            Condition::SessionActiveRun {
                session: command.session,
                expected: Some(command.attempt.run),
            },
            Condition::SessionRevision {
                session: command.session,
                expected: snapshot.session.revision,
            },
            Condition::CancellationEpoch {
                session: command.session,
                expected: snapshot.session.cancellation,
            },
            Condition::DeletionState {
                session: command.session,
                expected: snapshot.session.deletion.state,
                epoch: snapshot.session.deletion.epoch,
            },
            Condition::AgentFence {
                agent: agent.id,
                at_least: agent.fence(),
            },
        ],
        writes,
        after_commit: Vec::new(),
    };
    plan.validate()?;

    Ok(Planned {
        plan,
        projected: commit.run,
    })
}

/// Stops a session's work.
///
/// Pause exempt: stopping is exactly what a paused customer needs to be able to
/// do.
///
/// # Errors
///
/// Returns [`AppError`] when a port read fails or the operation cannot be
/// admitted.
pub async fn stop_session(
    context: &AppContext<'_>,
    command: &SessionCommand,
) -> Result<Planned<aex_operation_domain::Operation>, AppError> {
    let snapshot = context
        .sessions
        .load_session(command.workspace, command.session)
        .await?;
    gate(context, &snapshot.session, CommandClass::PauseExempt).await?;

    let now = context.clock.now();
    let existing = context
        .sessions
        .load_operation(command.workspace, command.operation)
        .await?;
    let request = AdmitRequest {
        id: command.operation,
        workspace: command.workspace,
        session: Some(command.session),
        kind: OperationKind::SessionStop,
        intent: command.intent,
        scope: OperationScope::Session(command.session),
        inline_result: None,
    };
    let operation = admitted(
        existing.as_ref(),
        Some(&snapshot.session.deletion),
        &request,
        now,
    )?;
    if existing.is_some() {
        return Ok(Planned {
            plan: empty_plan(TransactionIntent::StopSession),
            projected: operation,
        });
    }

    // Ninety-five non-root rows plus the root leave room for two conditions,
    // the operation, and the session head under the provider action ceiling.
    const STOP_AGENT_BUDGET: u16 = 95;
    let page = context
        .sessions
        .list_agents(
            command.session,
            crate::ports::PageBudget {
                limit: STOP_AGENT_BUDGET,
            },
        )
        .await?;
    if page.more {
        return Err(crate::plan::PlanError::TooManyActions {
            actions: crate::plan::MAX_ACTIONS + 1,
            max: crate::plan::MAX_ACTIONS,
        }
        .into());
    }
    let mut agents = Vec::with_capacity(page.agents.len() + 1);
    agents.push(snapshot.root_agent.clone());
    agents.extend(page.agents);
    let cancellation = cancel_session_work(
        &snapshot.session,
        &agents,
        CancelCause::StopRequested,
        None,
        now,
    );

    let mut head = snapshot.session.clone();
    head.revision = snapshot.session.revision.next();
    head.cancellation = cancellation.cancellation;
    head.work_admission = cancellation.admission;
    // Cancellation requests ownership to stop; only the terminal barrier may
    // settle the run and clear `active_run`. Clearing it here would orphan a
    // non-terminal run and admit a successor before the winner sealed output.
    if head.active_run.is_none() {
        head.status = SessionStatus::Idle;
    }
    head.updated_at = now;

    let mut writes = vec![
        Write::PutOperation(Box::new(operation.clone())),
        Write::PutSessionHead(Box::new(head)),
    ];
    writes.extend(
        cancellation
            .agents
            .iter()
            .map(|agent| Write::PutAgentControl(Box::new(agent.clone()))),
    );

    let plan = SessionTransaction {
        intent: TransactionIntent::StopSession,
        conditions: vec![
            Condition::SessionRevision {
                session: command.session,
                expected: snapshot.session.revision,
            },
            Condition::CancellationEpoch {
                session: command.session,
                expected: snapshot.session.cancellation,
            },
        ],
        writes,
        after_commit: Vec::new(),
    };
    plan.validate()?;

    Ok(Planned {
        plan,
        projected: operation,
    })
}

/// Rebinds a true-idle session to the current generations of an exact secret set.
///
/// The prior owner-key edge is returned only as an after-commit hint. An exact
/// operation replay returns the stored envelope without reading custody again,
/// so it cannot advance the custody revision twice.
///
/// # Errors
///
/// Returns [`AppError`] when the account is paused, the runtime is not truly
/// idle, a selected secret is unavailable, or the domain refuses the rebind.
pub async fn rebind_credentials(
    context: &AppContext<'_>,
    command: &Rebind,
) -> Result<Planned<aex_operation_domain::Operation>, AppError> {
    let snapshot = context
        .sessions
        .load_session(command.command.workspace, command.command.session)
        .await?;
    gate(context, &snapshot.session, CommandClass::PausableMutation).await?;

    let now = context.clock.now();
    let existing = context
        .sessions
        .load_operation(command.command.workspace, command.command.operation)
        .await?;
    if existing.is_some() {
        let operation = admitted(
            existing.as_ref(),
            Some(&snapshot.session.deletion),
            &rebind_request(command, None),
            now,
        )?;
        let plan = empty_rebind_plan();
        plan.validate()?;
        return Ok(Planned {
            plan,
            projected: operation,
        });
    }

    let prepared = prepare_rebind(context, command, &snapshot.session, now).await?;
    let public_result = CredentialRebindResult {
        session_id: command.command.session,
        custody_revision: prepared.custody.revision.0,
        secrets: prepared
            .custody
            .entries
            .iter()
            .map(|entry| SecretRef {
                name: entry.name.clone(),
            })
            .collect(),
    };
    let result_json = CanonicalJson::parse(&to_jcs_string(&public_result)?)?;
    let operation = admitted(
        None,
        Some(&snapshot.session.deletion),
        &rebind_request(
            command,
            Some(OperationResult {
                measurement: None,
                content: Some(result_json),
            }),
        ),
        now,
    )?;

    let plan = build_rebind_plan(&snapshot.session, command, operation.clone(), prepared, now);
    plan.validate()?;

    Ok(Planned {
        plan,
        projected: operation,
    })
}

const fn rebind_request(command: &Rebind, inline_result: Option<OperationResult>) -> AdmitRequest {
    AdmitRequest {
        id: command.command.operation,
        workspace: command.command.workspace,
        session: Some(command.command.session),
        kind: OperationKind::CredentialRebind,
        intent: command.command.intent,
        scope: OperationScope::Session(command.command.session),
        inline_result,
    }
}

fn empty_rebind_plan() -> SessionTransaction {
    SessionTransaction {
        intent: TransactionIntent::RebindCredentials,
        conditions: Vec::new(),
        writes: Vec::new(),
        after_commit: Vec::new(),
    }
}

async fn prepare_rebind(
    context: &AppContext<'_>,
    command: &Rebind,
    session: &Session,
    now: Timestamp,
) -> Result<PreparedRebind, AppError> {
    acquire_mutation_guard(
        session,
        command.command.operation,
        OperationKind::CredentialRebind,
        now,
    )?;
    let idle = context
        .continuity
        .true_idle(command.command.session)
        .await?;
    if let Some(violation) = idle.violation {
        return Err(CustodyRejection::NotTrueIdle(violation).into());
    }

    for (index, name) in command.secrets.iter().enumerate() {
        if command.secrets[..index].contains(name) {
            return Err(CustodyRejection::DuplicateName(name.clone()).into());
        }
    }
    let selected = context
        .secrets
        .read_secrets(command.command.workspace, &command.secrets)
        .await?;
    if command
        .secrets
        .iter()
        .any(|name| !selected.iter().any(|secret| secret.name == *name))
    {
        return Err(crate::ports::PortError::NotFound { kind: "secret" }.into());
    }

    let current = context
        .secrets
        .read_custody(command.command.session)
        .await?;
    let edge = OwnerKeyEdgeId(context.ids.next_uuid_v7());
    let (custody, destroy_key_edges) = if let Some(current) = &current {
        let commit = rebind(current, &selected, &idle, edge, now)?;
        (commit.custody, commit.destroy_key_edges)
    } else {
        (
            admit_custody(
                command.command.session,
                command.command.workspace,
                None,
                &selected,
                edge,
                now,
            )?,
            Vec::new(),
        )
    };
    Ok(PreparedRebind {
        custody,
        selected,
        destroy_key_edges,
    })
}

fn build_rebind_plan(
    session: &Session,
    command: &Rebind,
    operation: aex_operation_domain::Operation,
    prepared: PreparedRebind,
    now: Timestamp,
) -> SessionTransaction {
    let mut head = session.clone();
    head.revision = session.revision.next();
    head.custody_revision = prepared.custody.revision;
    head.updated_at = now;

    let mut conditions = live_conditions(session);
    conditions.extend([
        Condition::SessionActiveRun {
            session: command.command.session,
            expected: None,
        },
        Condition::MutationGuardFree {
            session: command.command.session,
        },
        Condition::CustodyRevision {
            session: command.command.session,
            expected: session.custody_revision,
        },
    ]);
    conditions.extend(
        prepared
            .selected
            .iter()
            .map(|secret| Condition::SecretRevocationEpoch {
                workspace: command.command.workspace,
                name: secret.name.clone(),
                expected: secret.revocation_epoch,
            }),
    );

    SessionTransaction {
        intent: TransactionIntent::RebindCredentials,
        conditions,
        writes: vec![
            Write::PutOperation(Box::new(operation)),
            Write::PutCustody(Box::new(prepared.custody)),
            Write::PutSessionHead(Box::new(head)),
        ],
        after_commit: prepared
            .destroy_key_edges
            .into_iter()
            .map(|edge| Hint::DestroyKeyEdge { edge })
            .collect(),
    }
}

/// Moves a session into the recovery window.
///
/// # Errors
///
/// Returns [`AppError`] when a port read fails or the session cannot be trashed.
pub async fn trash_session(
    context: &AppContext<'_>,
    command: &SessionCommand,
) -> Result<Planned<aex_operation_domain::Operation>, AppError> {
    let snapshot = context
        .sessions
        .load_session(command.workspace, command.session)
        .await?;
    gate(context, &snapshot.session, CommandClass::PauseExempt).await?;

    let now = context.clock.now();
    let existing = context
        .sessions
        .load_operation(command.workspace, command.operation)
        .await?;
    let operation = admitted(
        existing.as_ref(),
        Some(&snapshot.session.deletion),
        &AdmitRequest {
            id: command.operation,
            workspace: command.workspace,
            session: Some(command.session),
            kind: OperationKind::SessionTrash,
            intent: command.intent,
            scope: OperationScope::Session(command.session),
            inline_result: None,
        },
        now,
    )?;
    if existing.is_some() {
        return Ok(Planned {
            plan: empty_plan(TransactionIntent::TrashSession),
            projected: operation,
        });
    }
    let commit = trash(
        &snapshot.session.deletion,
        command.operation,
        now,
        RECOVERY_WINDOW,
    )?;

    let mut head = snapshot.session.clone();
    head.revision = snapshot.session.revision.next();
    head.deletion = commit.guard;
    head.work_admission = commit.admission;
    head.status = SessionStatus::Trashed;
    head.updated_at = now;

    let plan = SessionTransaction {
        intent: TransactionIntent::TrashSession,
        conditions: vec![
            Condition::SessionRevision {
                session: command.session,
                expected: snapshot.session.revision,
            },
            Condition::DeletionState {
                session: command.session,
                expected: snapshot.session.deletion.state,
                epoch: snapshot.session.deletion.epoch,
            },
        ],
        writes: vec![
            Write::PutOperation(Box::new(operation.clone())),
            Write::PutDeletionGuard(Box::new(commit.guard)),
            Write::PutSessionHead(Box::new(head)),
        ],
        after_commit: Vec::new(),
    };
    plan.validate()?;

    Ok(Planned {
        plan,
        projected: operation,
    })
}

/// Brings a session back out of the recovery window.
///
/// # Errors
///
/// Returns [`AppError`] when a port read fails or the window has closed.
pub async fn restore_session(
    context: &AppContext<'_>,
    command: &SessionCommand,
) -> Result<Planned<aex_operation_domain::Operation>, AppError> {
    let snapshot = context
        .sessions
        .load_session(command.workspace, command.session)
        .await?;
    gate(context, &snapshot.session, CommandClass::PausableMutation).await?;

    let now = context.clock.now();
    let existing = context
        .sessions
        .load_operation(command.workspace, command.operation)
        .await?;
    let operation = admitted(
        existing.as_ref(),
        None,
        &AdmitRequest {
            id: command.operation,
            workspace: command.workspace,
            session: Some(command.session),
            kind: OperationKind::SessionRestore,
            intent: command.intent,
            scope: OperationScope::Session(command.session),
            inline_result: None,
        },
        now,
    )?;
    if existing.is_some() {
        return Ok(Planned {
            plan: empty_plan(TransactionIntent::RestoreSession),
            projected: operation,
        });
    }
    let commit = restore(&snapshot.session.deletion, command.operation, now)?;

    let mut head = snapshot.session.clone();
    head.revision = snapshot.session.revision.next();
    head.deletion = commit.guard;
    head.work_admission = commit.admission;
    head.status = SessionStatus::Idle;
    head.updated_at = now;

    let plan = SessionTransaction {
        intent: TransactionIntent::RestoreSession,
        conditions: vec![
            Condition::SessionRevision {
                session: command.session,
                expected: snapshot.session.revision,
            },
            Condition::DeletionState {
                session: command.session,
                expected: DeletionState::Trashed,
                epoch: snapshot.session.deletion.epoch,
            },
        ],
        writes: vec![
            Write::PutOperation(Box::new(operation.clone())),
            Write::PutDeletionGuard(Box::new(commit.guard)),
            Write::PutSessionHead(Box::new(head)),
        ],
        after_commit: Vec::new(),
    };
    plan.validate()?;

    Ok(Planned {
        plan,
        projected: operation,
    })
}

/// Claims the destructive purge fence.
///
/// # Errors
///
/// Returns [`AppError`] when a port read fails or a purge already holds the
/// session.
pub async fn purge_session(
    context: &AppContext<'_>,
    command: &Purge,
) -> Result<Planned<aex_operation_domain::Operation>, AppError> {
    let snapshot = context
        .sessions
        .load_session(command.command.workspace, command.command.session)
        .await?;
    gate(context, &snapshot.session, CommandClass::PauseExempt).await?;

    let now = context.clock.now();
    let existing = context
        .sessions
        .load_operation(command.command.workspace, command.command.operation)
        .await?;
    let operation = admitted(
        existing.as_ref(),
        Some(&snapshot.session.deletion),
        &AdmitRequest {
            id: command.command.operation,
            workspace: command.command.workspace,
            session: Some(command.command.session),
            kind: OperationKind::SessionPurge,
            intent: command.command.intent,
            scope: OperationScope::Session(command.command.session),
            inline_result: None,
        },
        now,
    )?;
    if existing.is_some() {
        return Ok(Planned {
            plan: empty_plan(TransactionIntent::PurgeSession),
            projected: operation,
        });
    }
    let commit = purge(
        &snapshot.session.deletion,
        command.command.operation,
        command.cascade,
        &command.closure,
        now,
    )?;

    let mut head = snapshot.session.clone();
    head.revision = snapshot.session.revision.next();
    head.deletion = commit.guard;
    head.work_admission = commit.admission;
    head.status = SessionStatus::Purging;
    head.updated_at = now;

    let plan = SessionTransaction {
        intent: TransactionIntent::PurgeSession,
        conditions: vec![
            Condition::SessionRevision {
                session: command.command.session,
                expected: snapshot.session.revision,
            },
            Condition::DeletionState {
                session: command.command.session,
                expected: snapshot.session.deletion.state,
                epoch: snapshot.session.deletion.epoch,
            },
        ],
        writes: vec![
            Write::PutOperation(Box::new(operation.clone())),
            Write::PutDeletionGuard(Box::new(commit.guard)),
            Write::PutSessionHead(Box::new(head)),
        ],
        after_commit: Vec::new(),
    };
    plan.validate()?;

    Ok(Planned {
        plan,
        projected: operation,
    })
}

fn admitted(
    existing: Option<&aex_operation_domain::Operation>,
    guard: Option<&aex_session_domain::DeletionGuard>,
    request: &AdmitRequest,
    now: Timestamp,
) -> Result<aex_operation_domain::Operation, AppError> {
    match aex_operation_domain::admit(existing, guard, request, now) {
        AdmissionOutcome::Inserted(operation) | AdmissionOutcome::Replay(operation) => {
            Ok(*operation)
        }
        AdmissionOutcome::Conflict(code) => Err(AppError::Conflict(code.code())),
        AdmissionOutcome::DeletionInProgress { operation } => Err(AppError::Deletion(
            DeletionRejection::PurgeInProgress(operation),
        )),
        AdmissionOutcome::SessionPurged { operation } => {
            Err(AppError::Deletion(DeletionRejection::Purged(operation)))
        }
    }
}

const fn empty_plan(intent: TransactionIntent) -> SessionTransaction {
    SessionTransaction {
        intent,
        conditions: Vec::new(),
        writes: Vec::new(),
        after_commit: Vec::new(),
    }
}

/// The statuses a read command accepts. Present so a condition table can name
/// one value instead of building a set inline at each call site.
#[must_use]
pub fn live_statuses() -> BTreeSet<SessionStatus> {
    [
        SessionStatus::Idle,
        SessionStatus::Running,
        SessionStatus::AwaitingApproval,
    ]
    .into_iter()
    .collect()
}

/// The stable public code a rejection maps onto.
#[must_use]
pub const fn rejection_code(error: &AppError) -> ErrorCode {
    error.code()
}
