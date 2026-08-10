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

use aex_operation_domain::cursor::{ContinuationCursor, CursorPosition};
use aex_operation_domain::operation::{Execution, OperationResult, OperationScope, Progress};
use aex_operation_domain::{AdmissionOutcome, AdmitRequest, DeletionState, OperationKind};
use aex_secret_domain::{CustodyRejection, OwnerKeyEdgeId, SecretName, admit_custody, rebind};
use aex_secret_domain::{SessionCustody, WorkspaceSecret};
use aex_session_domain::{
    CancelCause, CommandClass, DeletionRejection, Message, MessageRole, MessageState, PurgeCascade,
    QueueRun, Run, Session, SessionDomainRunError, SessionRevision, SessionStatus, TerminalAttempt,
    WorkAdmission, acquire_mutation_guard, cancel_session_fence, claim_terminal, pause_gate, purge,
    queue, restore, start as start_run_domain, trash,
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
    let materialized = context
        .sessions
        .load_snapshot(command.workspace, command.session)
        .await?;
    gate(
        context,
        &materialized.session,
        CommandClass::PausableMutation,
    )
    .await?;

    let grant = context
        .reservations
        .prepare(ReservationRequest {
            organization: materialized.session.organization,
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
        &materialized.session,
        now,
    )?;

    let message = Message {
        id: command.message,
        session: command.session,
        run: Some(command.run),
        agent: materialized.root_agent.id,
        role: MessageRole::User,
        // A user message is born sealed.
        state: MessageState::Sealed,
        parts: command.parts.clone(),
        created_at: now,
        sealed_at: Some(now),
    };

    let mut head = materialized.session.clone();
    head.status = SessionStatus::Running;
    head.active_run = Some(command.run);
    head.revision = materialized.session.revision.next();

    let mut conditions = live_conditions(&materialized.session);
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
        expected: materialized.session.cancellation,
    });
    conditions.push(Condition::ReservationOpen {
        reservation: grant.reservation,
    });

    let plan = SessionTransaction {
        intent: TransactionIntent::AdmitMessage,
        conditions,
        writes: vec![
            Write::PutMessage(Box::new(message.clone())),
            Write::PutRun(Box::new(commit.run.clone())),
            Write::PutSessionHead(Box::new(head)),
        ],
        after_commit: vec![Hint::WakeAgent {
            agent: materialized.root_agent.id,
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
    gate(context, &snapshot, CommandClass::PausableMutation).await?;

    let stored = context
        .sessions
        .load_run(command.session, command.run)
        .await?;
    if snapshot.active_run != Some(command.run) {
        return Err(AppError::Run(SessionDomainRunError::SessionBusy {
            active: snapshot.active_run.unwrap_or(command.run),
        }));
    }

    let commit = start_run_domain(&stored, &snapshot, context.clock.now())?;

    let mut conditions = live_conditions(&snapshot);
    conditions.push(Condition::RunNonTerminal {
        session: command.session,
        run: command.run,
    });
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
        &snapshot,
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
            .map(|message| Write::PutMessage(Box::new(message.clone()))),
    );

    let plan = SessionTransaction {
        intent: TransactionIntent::CommitTerminal,
        conditions: vec![
            Condition::RunNonTerminal {
                session: command.session,
                run: command.attempt.run,
            },
            Condition::SessionActiveRun {
                session: command.session,
                expected: Some(command.attempt.run),
            },
            Condition::SessionRevision {
                session: command.session,
                expected: snapshot.revision,
            },
            Condition::CancellationEpoch {
                session: command.session,
                expected: snapshot.cancellation,
            },
            Condition::DeletionState {
                session: command.session,
                expected: snapshot.deletion.state,
                epoch: snapshot.deletion.epoch,
            },
            Condition::AgentFence {
                session: command.session,
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

/// The largest number of agent rows one stop step examines.
///
/// Derived from the transaction budget rather than chosen (D-4): a step also
/// writes the operation row and the session head, so its agent batch may never
/// exceed `MAX_ACTIONS - 2`. The landed use case read a flat 100 and could
/// therefore build a 101-action plan that `validate` rejected as an internal
/// fault.
#[allow(
    clippy::cast_possible_truncation,
    reason = "`MAX_ACTIONS` is `DynamoDB`'s hard ceiling of 100 and the static assertion below               refuses any value a `u16` could not hold"
)]
pub const STOP_BATCH_AGENTS: u16 = (crate::plan::MAX_ACTIONS as u16) - 2;

const _: () = assert!(
    crate::plan::MAX_ACTIONS > 2 && crate::plan::MAX_ACTIONS <= u16::MAX as usize,
    "the stop batch is derived from the action budget and must fit a bounded page"
);

/// The customer-visible phase a paged stop reports while it settles agents.
const STOP_PHASE: &str = "stopping";

/// Stops a session's work.
///
/// Pause exempt: stopping is exactly what a paused customer needs to be able to
/// do.
///
/// # Paged, never truncated
///
/// The landed implementation read one 100-agent page, ignored `more`, and set
/// the head to `Idle` regardless — so a session with more agents than one page
/// kept spending after the customer was told it had stopped. This reads a
/// bounded batch, and when more rows remain it admits the operation as
/// [`Execution::Continued`] with a [`CursorPosition::Stop`] cursor. The fence
/// that actually stops work — the closed `WorkAdmission` and the bumped
/// `CancellationEpoch` — closes on this first step; only the **final** step
/// moves the head to `Idle` and terminalizes the operation (D-2).
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
    gate(context, &snapshot, CommandClass::PauseExempt).await?;

    let now = context.clock.now();
    let existing = context
        .sessions
        .load_operation(command.workspace, command.operation)
        .await?;
    if let Some(stored) = existing.as_ref() {
        // An exact replay returns the stored envelope and writes nothing. It
        // must not re-read agents, re-bump the epoch or re-latch: the first
        // admission already closed the fence.
        let operation = admitted(
            Some(&stored.operation),
            Some(&snapshot.deletion),
            &stop_request(command, None, None),
            now,
        )?;
        let plan = empty_plan(TransactionIntent::StopSession);
        plan.validate()?;
        return Ok(Planned {
            plan,
            projected: operation,
        });
    }

    let page = context
        .sessions
        .list_agent_cancel_targets(
            command.session,
            None,
            crate::ports::PageBudget {
                limit: STOP_BATCH_AGENTS,
            },
        )
        .await?;
    let step = StopBatch::of(&snapshot, &page);

    let admit_request = stop_request(
        command,
        Some(step.execution),
        step.inline_result(command.session)?,
    );
    let admitted_operation = admitted(None, Some(&snapshot.deletion), &admit_request, now)?;
    let operation = step.shape_operation(&admitted_operation, now)?;

    Ok(Planned {
        plan: step.plan(command.session, &operation, Resume::Admission)?,
        projected: operation,
    })
}

/// Whether a step commit writes a new operation row or advances an existing one.
///
/// Modelled as one value rather than two independent `Option`s because the two
/// facts a resumed step needs — the cursor it advances **from** and the row
/// version it observed — are only ever known together, and a plan that carried
/// one without the other could not be compiled: the adapter refuses a resumed
/// operation write that cannot name its version, and refuses an admission that
/// does name one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resume {
    /// The row does not exist yet; the write is an insert.
    Admission,
    /// The row exists and this step advances it.
    Step {
        /// The cursor the row must still carry.
        from: Option<ContinuationCursor>,
        /// The version the step read.
        version: aex_operation_domain::operation::OperationVersion,
    },
}

/// Advances a continued stop by exactly one bounded batch.
///
/// The step is idempotent by construction: its plan names the cursor it
/// advances **from** ([`Condition::OperationCursorAt`]), so a duplicate
/// delivery fails its condition and writes nothing. `Succeeded` is written only
/// by the step that consumes the final page — there is no partial-success
/// status and no "completed with residue" flag (D-4).
///
/// # Errors
///
/// Returns [`AppError`] when a port read fails, the operation is absent, is not
/// a paged stop, or has already terminalized.
pub async fn continue_stop(
    context: &AppContext<'_>,
    command: &SessionCommand,
) -> Result<Planned<aex_operation_domain::Operation>, AppError> {
    let snapshot = context
        .sessions
        .load_session(command.workspace, command.session)
        .await?;
    let versioned = context
        .sessions
        .load_operation(command.workspace, command.operation)
        .await?
        .ok_or(crate::ports::PortError::NotFound { kind: "operation" })?;
    let stored = versioned.operation;
    if stored.kind != OperationKind::SessionStop || stored.status.is_terminal() {
        return Err(AppError::Transition(
            aex_operation_domain::TransitionError::WrongStatus {
                from: stored.status,
                to: aex_operation_domain::operation::OperationStatus::Running,
            },
        ));
    }
    let from = stored.cursor.clone();
    let next_agent = match from.as_ref().map(|cursor| &cursor.position) {
        Some(CursorPosition::Stop { next_agent }) => *next_agent,
        _ => {
            return Err(AppError::Port(crate::ports::PortError::Corrupt {
                kind: "operation",
                reason: "a continued stop carries no stop cursor",
            }));
        }
    };

    let now = context.clock.now();
    let page = context
        .sessions
        .list_agent_cancel_targets(
            command.session,
            Some(next_agent),
            crate::ports::PageBudget {
                limit: STOP_BATCH_AGENTS,
            },
        )
        .await?;
    let step = StopBatch::of(&snapshot, &page);
    let operation = step.advance(&stored, command.session, now)?;

    Ok(Planned {
        plan: step.plan(
            command.session,
            &operation,
            Resume::Step {
                from,
                version: versioned.version,
            },
        )?,
        projected: operation,
    })
}

/// What one stop step observed on the head it read.
///
/// Held rather than recomputed: every one of these values is re-asserted as a
/// condition (D-11), and deriving them back out of the head the step *writes*
/// would silently assert the value the step is establishing instead of the one
/// it saw.
#[derive(Debug, Clone, Copy)]
struct ObservedHead {
    revision: SessionRevision,
    cancellation: aex_session_domain::CancellationEpoch,
    deletion_state: DeletionState,
    deletion_epoch: aex_operation_domain::DeletionEpoch,
}

/// One bounded stop batch, resolved against the session it read.
struct StopBatch {
    /// The agents this step settles, already filtered to the active ones.
    settling: Vec<crate::ports::AgentCancelTarget>,
    /// Where the next step resumes, when one is needed.
    next: Option<AgentId>,
    /// How many agent rows this step examined.
    examined: u64,
    /// What the step read.
    observed: ObservedHead,
    /// The head this step writes.
    head: Session,
    /// What the whole operation is, given what this step found.
    execution: Execution,
}

impl StopBatch {
    fn of(session: &Session, page: &crate::ports::AgentCancelPage) -> Self {
        let settling: Vec<crate::ports::AgentCancelTarget> = page
            .targets
            .iter()
            .filter(|target| target.active)
            .copied()
            .collect();
        let fence = cancel_session_fence(session, CancelCause::StopRequested, !settling.is_empty());
        let last = page.next.is_none();

        let mut head = session.clone();
        head.revision = session.revision.next();
        head.cancellation = fence.cancellation;
        head.work_admission = fence.admission;
        if last {
            head.active_run = None;
            // A trashed or purging session keeps its lifecycle status: stopping
            // its work must never resurrect it into the live projection. The
            // landed implementation set `Idle` unconditionally.
            if session.deletion.state == DeletionState::Live {
                head.status = SessionStatus::Idle;
            }
        }

        Self {
            settling,
            next: page.next,
            examined: page.targets.len() as u64,
            observed: ObservedHead {
                revision: session.revision,
                cancellation: session.cancellation,
                deletion_state: session.deletion.state,
                deletion_epoch: session.deletion.epoch,
            },
            head,
            execution: if last {
                Execution::Inline
            } else {
                Execution::Continued
            },
        }
    }

    /// The terminal result, present only when this step consumes the final page.
    fn inline_result(&self, session: SessionId) -> Result<Option<OperationResult>, AppError> {
        if self.execution != Execution::Inline {
            return Ok(None);
        }
        Ok(Some(self.result(session, !self.settling.is_empty())?))
    }

    fn result(&self, session: SessionId, changed: bool) -> Result<OperationResult, AppError> {
        let public = aex_wire::models::SessionStopResult {
            changed,
            session_id: session,
            session_revision: self.head.revision.0,
        };
        Ok(OperationResult {
            measurement: None,
            content: Some(CanonicalJson::parse(&to_jcs_string(&public)?)?),
        })
    }

    /// Shapes a freshly admitted operation for this step.
    ///
    /// An inline stop is already `Succeeded` and latched by `admit`. A continued
    /// one is born `Queued`; this step starts it, **latches** it — the first
    /// step that bumps the cancellation epoch and closes `WorkAdmission` is the
    /// point of no return (D-2) — and parks it at its resume cursor.
    fn shape_operation(
        &self,
        admitted_operation: &aex_operation_domain::Operation,
        now: Timestamp,
    ) -> Result<aex_operation_domain::Operation, AppError> {
        if self.execution == Execution::Inline {
            return Ok(admitted_operation.clone());
        }
        let started = aex_operation_domain::operation::start(admitted_operation, now)?.operation;
        let mut latched =
            aex_operation_domain::operation::commit_point(&started, None, now)?.operation;
        self.park(&mut latched)?;
        Ok(latched)
    }

    /// Advances an already-running continued stop.
    fn advance(
        &self,
        stored: &aex_operation_domain::Operation,
        session: SessionId,
        now: Timestamp,
    ) -> Result<aex_operation_domain::Operation, AppError> {
        let carried = stored
            .progress
            .as_ref()
            .map_or(0, |progress| progress.processed);
        if self.execution == Execution::Inline {
            let mut done =
                aex_operation_domain::operation::succeed(stored, self.result(session, true)?, now)?
                    .operation;
            // The final step consumes the last page: the cursor is retired and
            // progress is closed at the total it actually reached.
            done.cursor = None;
            done.progress = Some(Progress {
                phase: STOP_PHASE.to_owned(),
                processed: carried.saturating_add(self.examined),
                total_hint: Some(carried.saturating_add(self.examined)),
            });
            return Ok(done);
        }
        let mut next = stored.clone();
        next.updated_at = now;
        self.park_from(&mut next, carried)?;
        Ok(next)
    }

    fn park(&self, operation: &mut aex_operation_domain::Operation) -> Result<(), AppError> {
        self.park_from(operation, 0)
    }

    fn park_from(
        &self,
        operation: &mut aex_operation_domain::Operation,
        carried: u64,
    ) -> Result<(), AppError> {
        let next_agent = self
            .next
            .ok_or(AppError::Port(crate::ports::PortError::Corrupt {
                kind: "agent page",
                reason: "a continued stop step reported no resume position",
            }))?;
        let processed = carried.saturating_add(self.examined);
        operation.cursor = Some(ContinuationCursor::new(
            CursorPosition::Stop { next_agent },
            processed,
            None,
        )?);
        let reported = Progress {
            phase: STOP_PHASE.to_owned(),
            processed,
            total_hint: None,
        };
        if let Some(current) = operation.progress.as_ref() {
            current
                .check_successor(&reported)
                .map_err(aex_operation_domain::TransitionError::from)?;
        } else {
            reported
                .validate()
                .map_err(aex_operation_domain::TransitionError::from)?;
        }
        operation.progress = Some(reported);
        Ok(())
    }

    /// The one transaction this step commits.
    fn plan(
        &self,
        session: SessionId,
        operation: &aex_operation_domain::Operation,
        resume: Resume,
    ) -> Result<SessionTransaction, AppError> {
        let mut conditions = vec![
            Condition::SessionRevision {
                session,
                expected: self.observed.revision,
            },
            Condition::CancellationEpoch {
                session,
                expected: self.observed.cancellation,
            },
            // D-12: every non-purge step commit carries the deletion state, so
            // a continued stop whose session enters `Purging` fails its next
            // step instead of writing against a session being destroyed.
            Condition::DeletionState {
                session,
                expected: self.observed.deletion_state,
                epoch: self.observed.deletion_epoch,
            },
        ];
        match &resume {
            // An admission is an insert, and `OperationCursorAt { expected:
            // None }` is how the plan says so: the row must not exist yet.
            Resume::Admission => {
                if operation.cursor.is_some() {
                    conditions.push(Condition::OperationCursorAt {
                        operation: operation.id,
                        expected: None,
                    });
                }
            }
            // A resumed step names both facts it observed. The cursor makes the
            // step idempotent against a duplicate delivery; the version keeps
            // the public cancellation's optimistic loop over the same row
            // intact. Both guards target the operation item, so together with
            // the operation write they merge into one physical action.
            Resume::Step { from, version } => {
                conditions.push(Condition::OperationCursorAt {
                    operation: operation.id,
                    expected: from.clone().map(Box::new),
                });
                conditions.push(Condition::OperationVersion {
                    operation: operation.id,
                    expected: *version,
                });
            }
        }
        conditions.extend(self.settling.iter().map(|target| Condition::AgentRevision {
            session,
            agent: target.agent,
            expected: target.revision,
        }));

        let mut writes = vec![
            Write::PutOperation(Box::new(operation.clone())),
            Write::PutSessionHead(Box::new(self.head.clone())),
        ];
        writes.extend(self.settling.iter().map(|target| Write::CancelAgent {
            session,
            agent: target.agent,
            from_revision: target.revision,
            to_revision: target.revision.next(),
            at: operation.updated_at,
        }));

        let after_commit = if self.execution == Execution::Continued {
            vec![Hint::OperationDue {
                operation: operation.id,
                due_at: operation.updated_at,
            }]
        } else {
            Vec::new()
        };

        let plan = SessionTransaction {
            intent: TransactionIntent::StopSession,
            conditions,
            writes,
            after_commit,
        };
        plan.validate()?;
        Ok(plan)
    }
}

const fn stop_request(
    command: &SessionCommand,
    execution: Option<Execution>,
    inline_result: Option<OperationResult>,
) -> AdmitRequest {
    AdmitRequest {
        id: command.operation,
        workspace: command.workspace,
        session: Some(command.session),
        kind: OperationKind::SessionStop,
        intent: command.intent,
        scope: OperationScope::Session(command.session),
        inline_result,
        execution,
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
    gate(context, &snapshot, CommandClass::PausableMutation).await?;

    let now = context.clock.now();
    let existing = context
        .sessions
        .load_operation(command.command.workspace, command.command.operation)
        .await?;
    if existing.is_some() {
        let operation = admitted(
            existing.as_ref().map(|stored| &stored.operation),
            Some(&snapshot.deletion),
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

    let prepared = prepare_rebind(context, command, &snapshot, now).await?;
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
        Some(&snapshot.deletion),
        &rebind_request(
            command,
            Some(OperationResult {
                measurement: None,
                content: Some(result_json),
            }),
        ),
        now,
    )?;

    let plan = build_rebind_plan(&snapshot, command, operation.clone(), prepared);
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
        execution: None,
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
) -> SessionTransaction {
    let mut head = session.clone();
    head.revision = session.revision.next();
    head.custody_revision = prepared.custody.revision;

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
    gate(context, &snapshot, CommandClass::PauseExempt).await?;

    let now = context.clock.now();
    let commit = trash(&snapshot.deletion, command.operation, now, RECOVERY_WINDOW)?;
    let existing = context
        .sessions
        .load_operation(command.workspace, command.operation)
        .await?;
    let operation = admitted(
        existing.as_ref().map(|stored| &stored.operation),
        Some(&snapshot.deletion),
        &AdmitRequest {
            id: command.operation,
            workspace: command.workspace,
            session: Some(command.session),
            kind: OperationKind::SessionTrash,
            intent: command.intent,
            scope: OperationScope::Session(command.session),
            inline_result: None,
            execution: None,
        },
        now,
    )?;

    let mut head = snapshot.clone();
    head.revision = snapshot.revision.next();
    head.deletion = commit.guard;
    head.work_admission = commit.admission;
    head.status = SessionStatus::Trashed;

    let plan = SessionTransaction {
        intent: TransactionIntent::TrashSession,
        conditions: vec![
            Condition::SessionRevision {
                session: command.session,
                expected: snapshot.revision,
            },
            Condition::DeletionState {
                session: command.session,
                expected: snapshot.deletion.state,
                epoch: snapshot.deletion.epoch,
            },
        ],
        writes: vec![
            Write::PutOperation(Box::new(operation.clone())),
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
    gate(context, &snapshot, CommandClass::PausableMutation).await?;

    let now = context.clock.now();
    let commit = restore(&snapshot.deletion, command.operation, now)?;
    let existing = context
        .sessions
        .load_operation(command.workspace, command.operation)
        .await?;
    let operation = admitted(
        existing.as_ref().map(|stored| &stored.operation),
        None,
        &AdmitRequest {
            id: command.operation,
            workspace: command.workspace,
            session: Some(command.session),
            kind: OperationKind::SessionRestore,
            intent: command.intent,
            scope: OperationScope::Session(command.session),
            inline_result: None,
            execution: None,
        },
        now,
    )?;

    let mut head = snapshot.clone();
    head.revision = snapshot.revision.next();
    head.deletion = commit.guard;
    head.work_admission = commit.admission;
    head.status = SessionStatus::Idle;

    let plan = SessionTransaction {
        intent: TransactionIntent::RestoreSession,
        conditions: vec![
            Condition::SessionRevision {
                session: command.session,
                expected: snapshot.revision,
            },
            Condition::DeletionState {
                session: command.session,
                expected: DeletionState::Trashed,
                epoch: snapshot.deletion.epoch,
            },
        ],
        writes: vec![
            Write::PutOperation(Box::new(operation.clone())),
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
    gate(context, &snapshot, CommandClass::PauseExempt).await?;

    let now = context.clock.now();
    let commit = purge(
        &snapshot.deletion,
        command.command.operation,
        command.cascade,
        &command.closure,
        now,
    )?;
    let existing = context
        .sessions
        .load_operation(command.command.workspace, command.command.operation)
        .await?;
    let operation = admitted(
        existing.as_ref().map(|stored| &stored.operation),
        Some(&snapshot.deletion),
        &AdmitRequest {
            id: command.command.operation,
            workspace: command.command.workspace,
            session: Some(command.command.session),
            kind: OperationKind::SessionPurge,
            intent: command.command.intent,
            scope: OperationScope::Session(command.command.session),
            inline_result: None,
            execution: None,
        },
        now,
    )?;

    let mut head = snapshot.clone();
    head.revision = snapshot.revision.next();
    head.deletion = commit.guard;
    head.work_admission = commit.admission;
    head.status = SessionStatus::Purging;

    let plan = SessionTransaction {
        intent: TransactionIntent::PurgeSession,
        conditions: vec![
            Condition::SessionRevision {
                session: command.command.session,
                expected: snapshot.revision,
            },
            Condition::DeletionState {
                session: command.command.session,
                expected: snapshot.deletion.state,
                epoch: snapshot.deletion.epoch,
            },
        ],
        writes: vec![
            Write::PutOperation(Box::new(operation.clone())),
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
