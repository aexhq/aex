//! Plan compilation for the `session-authority` transactions.
//!
//! Each function here compiles one accepted command into exactly the
//! participants, in exactly the order, with exactly the conditions that plan 05
//! §3 declares. No function makes an external call, reads a clock, or consults a
//! second store: everything a transaction commits is already in its plan.
//!
//! Participants that live in `regional-work`, `regional-content` and
//! `regional-authz-projection` are supplied by the caller as pre-built actions,
//! because those row shapes belong to their own adapter crates and those crates
//! depend on this one for the compiler. The order they are spliced in is fixed
//! here, so a caller cannot reorder a transaction by accident.

use aws_sdk_dynamodb::types::builders::{
    ConditionCheckBuilder, DeleteBuilder, PutBuilder, UpdateBuilder,
};

use crate::attr::{boolean, n, s, stamp};
use crate::codec;
use crate::error::StoreError;
use crate::keys;
use crate::plan::{IMMUTABLE, Participant, RegionalTables, TransactionPlan, key};
use crate::replay::{Receipt, ReceiptBody};
use crate::wire_pending::{
    AdmissionPlan, AgentDecisionPlan, EffectStage, FanoutPagePlan, LifecyclePlan,
    LifecycleTransition, SessionLifecycle, TerminalPlan,
};

/// One action whose row shape belongs to another adapter crate.
#[derive(Debug, Clone)]
pub enum ForeignAction {
    /// A read-only guard.
    ConditionCheck(Box<ConditionCheckBuilder>),
    /// A conditional put.
    Put(Box<PutBuilder>),
    /// A conditional update.
    Update(Box<UpdateBuilder>),
    /// A conditional delete.
    Delete(Box<DeleteBuilder>),
}

/// A foreign action with the participant it commits as.
#[derive(Debug, Clone)]
pub struct Foreign {
    /// Which named participant.
    pub participant: Participant,
    /// What it does.
    pub action: ForeignAction,
}

impl Foreign {
    /// Names a foreign action.
    #[must_use]
    pub fn new(participant: Participant, action: ForeignAction) -> Self {
        Self {
            participant,
            action,
        }
    }

    fn push(self, plan: &mut TransactionPlan) -> Result<(), StoreError> {
        match self.action {
            ForeignAction::ConditionCheck(builder) => {
                plan.condition_check(self.participant, *builder)?;
            }
            ForeignAction::Put(builder) => {
                plan.put(self.participant, *builder)?;
            }
            ForeignAction::Update(builder) => {
                plan.update(self.participant, *builder)?;
            }
            ForeignAction::Delete(builder) => {
                plan.delete(self.participant, *builder)?;
            }
        }
        Ok(())
    }
}

/// Authority fields required to terminalize a running operation whose cancel
/// request was observed by its next fenced worker step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationStepCancel {
    /// Tenant bound into the authority-row condition.
    pub workspace: aex_wire::ids::WorkspaceId,
    /// Operation bound into the authority-row key and condition.
    pub operation: aex_wire::ids::OperationId,
    /// Exact optimistic version observed before the work claim.
    pub version: u64,
    /// Commit instant shared with the fenced work retirement.
    pub now: aex_wire::types::Timestamp,
}

/// Builds the operation half of an atomic cancelled-step commit.
///
/// The work adapter supplies the other half. Keeping both updates in one
/// [`TransactionPlan`] prevents a crash from leaving a cancelled operation
/// runnable or a retired work row whose operation is still running.
///
/// # Errors
///
/// [`StoreError::Invalid`] when the version cannot advance.
pub fn operation_cancelled(
    table: &str,
    request: &OperationStepCancel,
) -> Result<UpdateBuilder, StoreError> {
    let next_version = request
        .version
        .checked_add(1)
        .ok_or_else(|| StoreError::Invalid {
            detail: "an operation version cannot advance past u64::MAX".to_owned(),
        })?;
    let operation_key = keys::operation(request.operation);
    Ok(aws_sdk_dynamodb::types::Update::builder()
        .table_name(table)
        .set_key(Some(key(&operation_key.pk, &operation_key.sk)))
        .condition_expression(
            "attribute_exists(pk) AND workspaceId = :workspaceId AND operationId = :operationId \
             AND version = :version AND #status = :running AND cancelRequested = :true \
             AND attribute_not_exists(committedAt)",
        )
        .update_expression(
            "SET #status = :cancelled, version = :nextVersion, updatedAt = :now, terminalAt = :now",
        )
        .expression_attribute_names("#status", "status")
        .expression_attribute_values(":workspaceId", s(request.workspace.to_string()))
        .expression_attribute_values(":operationId", s(request.operation.to_string()))
        .expression_attribute_values(":version", n(request.version))
        .expression_attribute_values(":running", s("running"))
        .expression_attribute_values(":true", boolean(true))
        .expression_attribute_values(":cancelled", s("cancelled"))
        .expression_attribute_values(":nextVersion", n(next_version))
        .expression_attribute_values(":now", stamp(request.now)))
}

/// Authority fields observed by the public cancellation command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationCancelRequest {
    /// Tenant bound into the authority-row condition.
    pub workspace: aex_wire::ids::WorkspaceId,
    /// Operation bound into both the key and the condition.
    pub operation: aex_wire::ids::OperationId,
    /// Immutable operation kind observed by the strong read.
    pub kind: aex_operation_domain::OperationKind,
    /// Exact nonterminal status observed by the strong read.
    pub status: aex_operation_domain::OperationStatus,
    /// Exact optimistic version observed by the strong read.
    pub version: u64,
    /// Acceptance instant.
    pub now: aex_wire::types::Timestamp,
}

/// Whether cancellation for this kind is owned by session authority.
///
/// Telemetry exports are public and cancelable in the product model, but their
/// effect fence is the observation export row. Treating an operation-row
/// update as their cancellation would acknowledge a command that cannot stop
/// the export launcher or task.
#[must_use]
pub const fn operation_cancel_owned(kind: aex_operation_domain::OperationKind) -> bool {
    use aex_operation_domain::OperationKind;

    match kind {
        OperationKind::SessionPersist
        | OperationKind::SessionClone
        | OperationKind::WorkspaceDiscard
        | OperationKind::CredentialRebind
        | OperationKind::SessionRestore => true,
        OperationKind::SessionStop
        | OperationKind::SessionTrash
        | OperationKind::SessionPurge
        | OperationKind::WorkspaceDelete
        | OperationKind::TelemetryExport
        | OperationKind::ContentGc => false,
    }
}

/// Builds the one-row transaction that accepts a public cancellation.
///
/// A queued operation terminalizes here. A running operation only latches the
/// request; its next fenced worker step atomically terminalizes the operation
/// and retires the exact work claim through [`operation_cancelled`]. Both
/// shapes condition on the complete observed identity, version, kind, status,
/// open commit latch and previously-unset cancel flag, so a worker crossing the
/// commit point wins the race rather than being overwritten.
///
/// # Errors
///
/// [`StoreError::Invalid`] for a terminal input, an internal, non-cancelable or
/// separately-owned kind, or version exhaustion.
pub fn operation_cancel_requested(
    table: &str,
    request: &OperationCancelRequest,
) -> Result<UpdateBuilder, StoreError> {
    use aex_operation_domain::{OperationKind, OperationStatus};

    if !request.kind.is_public()
        || !request.kind.cancelable_on_accept()
        || !operation_cancel_owned(request.kind)
    {
        return Err(StoreError::Invalid {
            detail: "a session-authority cancellation requires an owned customer-visible cancelable kind"
                .to_owned(),
        });
    }
    if request.status.is_terminal() {
        return Err(StoreError::Invalid {
            detail: "a public cancellation cannot update a terminal operation".to_owned(),
        });
    }
    let next_version = request
        .version
        .checked_add(1)
        .ok_or_else(|| StoreError::Invalid {
            detail: "an operation version cannot advance past u64::MAX".to_owned(),
        })?;
    let operation_key = keys::operation(request.operation);
    let mut builder = aws_sdk_dynamodb::types::Update::builder()
        .table_name(table)
        .set_key(Some(key(&operation_key.pk, &operation_key.sk)))
        .condition_expression(
            "attribute_exists(pk) AND workspaceId = :workspaceId AND operationId = :operationId \
             AND kind = :kind AND version = :version AND #status = :status \
             AND cancelRequested = :false AND attribute_not_exists(committedAt)",
        )
        .expression_attribute_names("#status", "status")
        .expression_attribute_values(":workspaceId", s(request.workspace.to_string()))
        .expression_attribute_values(":operationId", s(request.operation.to_string()))
        .expression_attribute_values(":kind", s(request.kind.as_str()))
        .expression_attribute_values(":version", n(request.version))
        .expression_attribute_values(":status", s(request.status.as_str()))
        .expression_attribute_values(":false", boolean(false))
        .expression_attribute_values(":true", boolean(true))
        .expression_attribute_values(":nextVersion", n(next_version))
        .expression_attribute_values(":now", stamp(request.now));
    builder = match request.status {
        OperationStatus::Queued => builder
            .update_expression(
                "SET #status = :cancelled, cancelRequested = :true, version = :nextVersion, \
                 updatedAt = :now, terminalAt = :now",
            )
            .expression_attribute_values(":cancelled", s("cancelled")),
        OperationStatus::Running => builder.update_expression(
            "SET cancelRequested = :true, version = :nextVersion, updatedAt = :now",
        ),
        OperationStatus::Succeeded | OperationStatus::Failed | OperationStatus::Cancelled => {
            return Err(StoreError::Invalid {
                detail: "a public cancellation cannot update a terminal operation".to_owned(),
            });
        }
    };
    // Keep this match exhaustive if a new internal kind is introduced: the
    // validation above owns the policy, while the match makes the dependency
    // on the closed vocabulary visible to the compiler.
    match request.kind {
        OperationKind::SessionPersist
        | OperationKind::SessionClone
        | OperationKind::WorkspaceDiscard
        | OperationKind::CredentialRebind
        | OperationKind::SessionRestore => Ok(builder),
        OperationKind::SessionStop
        | OperationKind::SessionTrash
        | OperationKind::SessionPurge
        | OperationKind::WorkspaceDelete
        | OperationKind::TelemetryExport
        | OperationKind::ContentGc => Err(StoreError::Invalid {
            detail: "a session-authority cancellation requires an owned customer-visible cancelable kind"
                .to_owned(),
        }),
    }
}

/// The foreign participants of the admission transaction.
#[derive(Debug, Clone, Default)]
pub struct AdmissionForeign {
    /// The projected placement guard, from `regional-authz-projection`.
    pub placement: Option<Foreign>,
    /// The staged-body commit and its pin, from `regional-content`.
    ///
    /// Empty when the message body fitted inline.
    pub content: Vec<Foreign>,
    /// The root wake and its dedupe claim, from `regional-work`.
    pub work: Vec<Foreign>,
}

/// The canonical participant order of the admission transaction.
///
/// Published so a peer can assert the order it must supply its actions in
/// without reading this module's source.
pub const ADMISSION_ORDER: &[Participant] = &[
    Participant::AUTHZ_PLACEMENT,
    Participant::CONTENT_COMMIT,
    Participant::CONTENT_MESSAGE_PIN,
    Participant::SESSION_HEAD,
    Participant::SESSION_MESSAGE,
    Participant::SESSION_RUN,
    Participant::SESSION_RESERVATION,
    Participant::AGENT_ROOT_CONTROL,
    Participant::SESSION_ADMITTED_EVENT,
    Participant::WORK_ROOT_WAKE,
    Participant::WORK_DEDUPE,
    Participant::SESSION_IDEMPOTENCY,
];

/// Compiles `session.admit_message_and_run`.
///
/// The API returns only after this transaction succeeds. It calls no Brain
/// socket, no Hands provider, no queue and no external provider before
/// responding, which is what makes an admitted run durable the instant the
/// caller is told about it.
///
/// # Errors
///
/// [`StoreError::Invalid`] when a participant is unconditional or incomplete,
/// [`StoreError::ItemTooLarge`] when a row exceeds the application ceiling, and
/// [`StoreError::Key`] when a component cannot enter a key.
#[allow(
    clippy::too_many_lines,
    reason = "one contiguous function per transaction keeps participant order readable in one place; splitting it would hide the order it exists to fix"
)]
pub fn compile_admission(
    tables: &RegionalTables,
    request: &AdmissionPlan,
    foreign: AdmissionForeign,
) -> Result<TransactionPlan, StoreError> {
    let table = tables.session_authority.as_str();
    let head = &request.head;
    let mut plan = TransactionPlan::new(request.replay.intent.to_string());

    if let Some(placement) = foreign.placement {
        placement.push(&mut plan)?;
    }
    for action in foreign.content {
        action.push(&mut plan)?;
    }

    // 4. The head. Every fence the admission depends on is a top-level
    // attribute, so one condition covers competing runs, cancellation,
    // deletion, content re-staging and a changed resolved config.
    let head_key = keys::head(head.session);
    plan.update(
        Participant::SESSION_HEAD,
        aws_sdk_dynamodb::types::Update::builder()
            .table_name(table)
            .set_key(Some(key(&head_key.pk, &head_key.sk)))
            .condition_expression(
                "attribute_exists(pk) AND revision = :revision AND #status = :idle \
                 AND lifecycle = :active AND deletionEpoch = :deletionEpoch \
                 AND cancelEpoch = :cancelEpoch AND contentAdmissionEpoch = :contentEpoch \
                 AND resolvedConfigDigest = :configDigest AND attribute_not_exists(activeRunId)",
            )
            .update_expression(
                "SET revision = :nextRevision, #status = :running, activeRunId = :runId, \
                 updatedAt = :now, agentBudget = :agentBudget",
            )
            .expression_attribute_names("#status", "status")
            .expression_attribute_values(":revision", n(head.revision))
            .expression_attribute_values(":idle", s("idle"))
            .expression_attribute_values(":active", s("active"))
            .expression_attribute_values(":deletionEpoch", n(head.deletion_epoch))
            .expression_attribute_values(":cancelEpoch", n(head.cancel_epoch))
            .expression_attribute_values(":contentEpoch", n(head.content_admission_epoch))
            .expression_attribute_values(":configDigest", s(head.resolved_config_digest.clone()))
            .expression_attribute_values(":nextRevision", n(head.revision + 1))
            .expression_attribute_values(":running", s("running"))
            .expression_attribute_values(":runId", s(request.run.run.to_string()))
            .expression_attribute_values(":now", stamp(request.now))
            .expression_attribute_values(":agentBudget", n(head.agent_budget)),
    )?;

    // 5-7. Three immutable rows. A collision under UUIDv7 means a duplicated
    // request, which the replay combinator resolves against the receipt.
    plan.put(
        Participant::SESSION_MESSAGE,
        immutable_put(
            table,
            codec::encode_message(&request.message, head.workspace, head.organization),
        ),
    )?;
    plan.put(
        Participant::SESSION_RUN,
        immutable_put(
            table,
            codec::encode_run(&request.run, head.workspace, head.organization),
        ),
    )?;
    let reservation_key = keys::reservation(head.session, &request.run.reservation)?;
    plan.put(
        Participant::SESSION_RESERVATION,
        immutable_put(
            table,
            crate::attr::ItemBuilder::new(codec::SPEND_RESERVATION)
                .set(crate::attr::PK, s(reservation_key.pk))
                .set(crate::attr::SK, s(reservation_key.sk))
                .set("reservationId", s(request.run.reservation.clone()))
                .set("runId", s(request.run.run.to_string()))
                .set("reservedCents", n(request.run.max_spend_cents))
                .set("state", s("held"))
                .set("heldAt", stamp(request.now))
                .build(),
        ),
    )?;

    // 8. The root agent receives its hierarchical child budget here, which is
    // why no shared agent counter exists anywhere (D-04).
    let control_key = keys::agent_control(head.session, head.root_agent);
    plan.update(
        Participant::AGENT_ROOT_CONTROL,
        aws_sdk_dynamodb::types::Update::builder()
            .table_name(table)
            .set_key(Some(key(&control_key.pk, &control_key.sk)))
            .condition_expression(
                "attribute_exists(pk) AND revision = :agentRevision AND fence = :fence",
            )
            .update_expression(
                "SET #status = :queued, revision = :nextAgentRevision, \
                 childBudgetRemaining = :childBudget, childBudgetGranted = :childBudget, \
                 updatedAt = :now",
            )
            .expression_attribute_names("#status", "status")
            .expression_attribute_values(":agentRevision", n(request.root_control.revision))
            .expression_attribute_values(":fence", n(request.root_control.fence))
            .expression_attribute_values(":queued", s("queued"))
            .expression_attribute_values(":nextAgentRevision", n(request.root_control.revision + 1))
            .expression_attribute_values(":childBudget", n(request.child_budget))
            .expression_attribute_values(":now", stamp(request.now)),
    )?;

    plan.put(
        Participant::SESSION_ADMITTED_EVENT,
        immutable_put(table, codec::encode_event(head.session, &request.event)),
    )?;

    for action in foreign.work {
        action.push(&mut plan)?;
    }

    plan.put(
        Participant::SESSION_IDEMPOTENCY,
        immutable_put(table, receipt_item(request)?),
    )?;

    Ok(plan)
}

fn receipt_item(request: &AdmissionPlan) -> Result<crate::attr::Item, StoreError> {
    let receipt = Receipt {
        scope: request.replay.scope.clone(),
        key_sha256: crate::replay::key_digest(&request.replay.key),
        intent: request.replay.intent,
        response_kind: request.replay.response_kind.clone(),
        response: ReceiptBody::Inline(request.replay.response.clone()),
        committed_at: request.now,
        expires_at: request.replay.expires_at,
    };
    Ok(codec::encode_receipt(request.head.workspace, &receipt)?)
}

fn immutable_put(table: &str, item: crate::attr::Item) -> PutBuilder {
    aws_sdk_dynamodb::types::Put::builder()
        .table_name(table)
        .set_item(Some(item))
        .condition_expression(IMMUTABLE)
}

/// The canonical participant order of the run terminal barrier.
pub const TERMINAL_ORDER: &[Participant] = &[
    Participant::SESSION_RUN,
    Participant::AGENT_ROOT_CONTROL,
    Participant::SESSION_HEAD,
    Participant::SESSION_TERMINAL_EVENT,
    Participant::SESSION_RESERVATION,
    Participant::WORK_USAGE_CLOSURE,
    Participant::WORK_WAKE_DONE,
];

/// The foreign participants of the terminal barrier.
#[derive(Debug, Clone, Default)]
pub struct TerminalForeign {
    /// The compute-closure usage fact, from `regional-work`.
    pub usage_closure: Option<Foreign>,
    /// The fenced retirement of the wake, from `regional-work`.
    pub wake_done: Option<Foreign>,
}

/// Compiles `session.commit_run_terminal`.
///
/// A `ConditionalCheckFailed` on the first participant means a competing
/// terminal already won: the loser reads the run, returns the existing terminal
/// outcome and publishes nothing. There is exactly one terminal truth per run
/// and no second outcome can ever be written.
///
/// # Errors
///
/// As [`compile_admission`].
pub fn compile_terminal(
    tables: &RegionalTables,
    request: &TerminalPlan,
    foreign: TerminalForeign,
) -> Result<TransactionPlan, StoreError> {
    let table = tables.session_authority.as_str();
    let mut plan = TransactionPlan::new(format!("terminal:{}", request.run));

    let run_key = keys::run(request.session, request.run);
    plan.update(
        Participant::SESSION_RUN,
        aws_sdk_dynamodb::types::Update::builder()
            .table_name(table)
            .set_key(Some(key(&run_key.pk, &run_key.sk)))
            .condition_expression("attribute_exists(pk) AND #status IN (:queued, :running)")
            .update_expression(
                "SET #status = :terminal, terminalAt = :now, resultDigest = :result, \
                 usageClosureId = :closure",
            )
            .expression_attribute_names("#status", "status")
            .expression_attribute_values(":queued", s("queued"))
            .expression_attribute_values(":running", s("running"))
            .expression_attribute_values(":terminal", s(request.terminal_status))
            .expression_attribute_values(":now", stamp(request.now))
            .expression_attribute_values(
                ":result",
                request
                    .result_digest
                    .clone()
                    .map_or(aws_sdk_dynamodb::types::AttributeValue::Null(true), s),
            )
            .expression_attribute_values(":closure", s(request.usage_closure_id.clone())),
    )?;

    let control_key = keys::agent_control(request.session, request.root_agent);
    plan.update(
        Participant::AGENT_ROOT_CONTROL,
        aws_sdk_dynamodb::types::Update::builder()
            .table_name(table)
            .set_key(Some(key(&control_key.pk, &control_key.sk)))
            .condition_expression("revision = :agentRevision AND fence = :fence")
            .update_expression(
                "SET #status = :terminalAgentStatus, journalTail = :tail, revision = :nextRevision",
            )
            .expression_attribute_names("#status", "status")
            .expression_attribute_values(":agentRevision", n(request.agent_revision))
            .expression_attribute_values(":fence", n(request.agent_fence))
            .expression_attribute_values(
                ":terminalAgentStatus",
                s(request.terminal_agent_status.clone()),
            )
            .expression_attribute_values(":tail", n(request.journal_tail))
            .expression_attribute_values(":nextRevision", n(request.agent_revision + 1)),
    )?;

    let head_key = keys::head(request.session);
    plan.update(
        Participant::SESSION_HEAD,
        aws_sdk_dynamodb::types::Update::builder()
            .table_name(table)
            .set_key(Some(key(&head_key.pk, &head_key.sk)))
            .condition_expression(
                "revision = :revision AND activeRunId = :runId AND deletionEpoch = :deletionEpoch",
            )
            .update_expression(
                "SET revision = :nextRevision, #status = :idle, updatedAt = :now \
                 REMOVE activeRunId",
            )
            .expression_attribute_names("#status", "status")
            .expression_attribute_values(":revision", n(request.head_revision))
            .expression_attribute_values(":runId", s(request.run.to_string()))
            .expression_attribute_values(":deletionEpoch", n(request.deletion_epoch))
            .expression_attribute_values(":nextRevision", n(request.head_revision + 1))
            .expression_attribute_values(":idle", s("idle"))
            .expression_attribute_values(":now", stamp(request.now)),
    )?;

    plan.put(
        Participant::SESSION_TERMINAL_EVENT,
        immutable_put(table, codec::encode_event(request.session, &request.event)),
    )?;

    let reservation_key = keys::reservation(request.session, &request.reservation)?;
    plan.update(
        Participant::SESSION_RESERVATION,
        aws_sdk_dynamodb::types::Update::builder()
            .table_name(table)
            .set_key(Some(key(&reservation_key.pk, &reservation_key.sk)))
            .condition_expression("#state = :held")
            .update_expression("SET #state = :released, releasedAt = :now")
            .expression_attribute_names("#state", "state")
            .expression_attribute_values(":held", s("held"))
            .expression_attribute_values(":released", s("released"))
            .expression_attribute_values(":now", stamp(request.now)),
    )?;

    if let Some(action) = foreign.usage_closure {
        action.push(&mut plan)?;
    }
    if let Some(action) = foreign.wake_done {
        action.push(&mut plan)?;
    }
    Ok(plan)
}

/// The canonical participant order of an agent decision.
pub const DECISION_ORDER: &[Participant] = &[
    Participant::SESSION_HEAD_GUARD,
    Participant::AGENT_CONTROL,
    Participant::AGENT_JOURNAL,
    Participant::AGENT_EFFECT,
    Participant::SESSION_PREVIEW_EVENT,
    Participant::WORK_NEXT_WAKE,
];

/// Compiles the agent decision transaction Brain plans.
///
/// No external call occurs inside it and no second store participates, so a
/// lost condition makes the activation discard its computed result and reload.
/// It can never publish stale state.
///
/// # Errors
///
/// As [`compile_admission`].
#[allow(
    clippy::too_many_lines,
    reason = "one contiguous function per transaction keeps participant order readable in one place; splitting it would hide the order it exists to fix"
)]
pub fn compile_decision(
    tables: &RegionalTables,
    request: &AgentDecisionPlan,
    next_wake: Option<Foreign>,
) -> Result<TransactionPlan, StoreError> {
    let table = tables.session_authority.as_str();
    let mut plan =
        TransactionPlan::new(format!("decision:{}:{}", request.agent, request.entry.seq));

    let head_key = keys::head(request.session);
    plan.condition_check(
        Participant::SESSION_HEAD_GUARD,
        aws_sdk_dynamodb::types::ConditionCheck::builder()
            .table_name(table)
            .set_key(Some(key(&head_key.pk, &head_key.sk)))
            .condition_expression(
                "cancelEpoch = :cancelEpoch AND deletionEpoch = :deletionEpoch \
                 AND lifecycle = :active",
            )
            .expression_attribute_values(":cancelEpoch", n(request.head_guard.cancel_epoch))
            .expression_attribute_values(":deletionEpoch", n(request.head_guard.deletion_epoch))
            .expression_attribute_values(":active", s("active")),
    )?;

    let control = &request.control;
    let control_key = keys::agent_control(request.session, request.agent);
    plan.update(
        Participant::AGENT_CONTROL,
        aws_sdk_dynamodb::types::Update::builder()
            .table_name(table)
            .set_key(Some(key(&control_key.pk, &control_key.sk)))
            .condition_expression(
                "revision = :revision AND journalTail = :tail AND claimOwner = :owner \
                 AND fence = :fence",
            )
            .update_expression(
                "SET revision = :nextRevision, journalTail = :nextTail, #status = :status, \
                 leaseExpiresAt = :lease, updatedAt = :now",
            )
            .expression_attribute_names("#status", "status")
            .expression_attribute_values(":revision", n(control.revision))
            .expression_attribute_values(":tail", n(control.journal_tail))
            .expression_attribute_values(
                ":owner",
                s(control.claim_owner.clone().ok_or(StoreError::Invalid {
                    detail: "a decision needs a claimed control item".to_owned(),
                })?),
            )
            .expression_attribute_values(":fence", n(control.fence))
            .expression_attribute_values(":nextRevision", n(control.revision + 1))
            .expression_attribute_values(":nextTail", n(request.entry.seq))
            .expression_attribute_values(":status", s(request.next_status.clone()))
            .expression_attribute_values(":lease", stamp(request.lease_expires_at))
            .expression_attribute_values(":now", stamp(request.now)),
    )?;

    plan.put(
        Participant::AGENT_JOURNAL,
        immutable_put(
            table,
            codec::encode_journal(request.session, request.agent, &request.entry),
        ),
    )?;

    if let Some(effect) = &request.effect {
        let effect_key = keys::effect(request.session, request.agent, &effect.effect_id)?;
        match effect.stage {
            EffectStage::Prepare => {
                plan.put(
                    Participant::AGENT_EFFECT,
                    immutable_put(
                        table,
                        crate::attr::ItemBuilder::new(codec::AGENT_EFFECT)
                            .set(crate::attr::PK, s(effect_key.pk))
                            .set(crate::attr::SK, s(effect_key.sk))
                            .set("effectId", s(effect.effect_id.clone()))
                            .set("kind", s(effect.kind.clone()))
                            .set("requestHash", s(effect.request_hash.clone()))
                            .set("attempt", n(effect.attempt))
                            .set("state", s("prepared"))
                            .set("preparedAt", stamp(request.now))
                            .build(),
                    ),
                )?;
            }
            EffectStage::Settle { state } => {
                plan.update(
                    Participant::AGENT_EFFECT,
                    aws_sdk_dynamodb::types::Update::builder()
                        .table_name(table)
                        .set_key(Some(key(&effect_key.pk, &effect_key.sk)))
                        .condition_expression("#state = :prepared AND requestHash = :hash")
                        .update_expression("SET #state = :settled, settledAt = :now")
                        .expression_attribute_names("#state", "state")
                        .expression_attribute_values(":prepared", s("prepared"))
                        .expression_attribute_values(":hash", s(effect.request_hash.clone()))
                        .expression_attribute_values(":settled", s(state))
                        .expression_attribute_values(":now", stamp(request.now)),
                )?;
            }
        }
    }

    if let Some(event) = &request.event {
        plan.put(
            Participant::SESSION_PREVIEW_EVENT,
            immutable_put(table, codec::encode_event(request.session, event)),
        )?;
    }

    if let Some(action) = next_wake {
        action.push(&mut plan)?;
    }
    Ok(plan)
}

/// The largest number of actions a fanout page may commit.
///
/// A page is sized so the transaction stays inside this bound; a larger swarm
/// commits one durable intent first and then deterministic idempotent pages, so
/// no child is runnable before its page is committed.
pub const MAX_FANOUT_ACTIONS: usize = 90;

/// Compiles one bounded fanout page.
///
/// The budget condition is the whole point: `childBudgetRemaining >= :childCount`
/// on the parent's own partition. There is no shared counter, so 500 children
/// never contend on one item, and a failed condition is `limit_exceeded` with
/// the effective value rather than a silent clamp.
///
/// # Errors
///
/// [`StoreError::Invalid`] when the page would exceed [`MAX_FANOUT_ACTIONS`],
/// plus everything [`compile_admission`] can return.
#[allow(
    clippy::too_many_lines,
    reason = "one contiguous function per transaction keeps participant order readable in one place; splitting it would hide the order it exists to fix"
)]
pub fn compile_fanout_page(
    tables: &RegionalTables,
    request: &FanoutPagePlan,
    wakes: Vec<Foreign>,
) -> Result<TransactionPlan, StoreError> {
    let table = tables.session_authority.as_str();
    let children = u64::try_from(request.children.len()).map_err(|_| StoreError::Invalid {
        detail: "a fanout page cannot hold that many children".to_owned(),
    })?;
    // one budget update + one page row + two rows per child + one wake per child
    let actions = 2 + request.children.len() * 3;
    if actions > MAX_FANOUT_ACTIONS {
        return Err(StoreError::Invalid {
            detail: format!(
                "a fanout page of {} children compiles to {actions} actions; the page ceiling is \
                 {MAX_FANOUT_ACTIONS}",
                request.children.len()
            ),
        });
    }

    let mut plan = TransactionPlan::new(format!("fanout:{}:{}", request.intent_id, request.page));
    let parent_key = keys::agent_control(request.session, request.parent);
    plan.update(
        Participant::AGENT_CONTROL,
        aws_sdk_dynamodb::types::Update::builder()
            .table_name(table)
            .set_key(Some(key(&parent_key.pk, &parent_key.sk)))
            .condition_expression("revision = :revision AND childBudgetRemaining >= :childCount")
            .update_expression(
                "SET childBudgetRemaining = childBudgetRemaining - :childCount, \
                 revision = :nextRevision, updatedAt = :now",
            )
            .expression_attribute_values(":revision", n(request.parent_revision))
            .expression_attribute_values(":childCount", n(children))
            .expression_attribute_values(":nextRevision", n(request.parent_revision + 1))
            .expression_attribute_values(":now", stamp(request.now)),
    )?;

    let page_key = keys::fanout_page(
        request.session,
        request.parent,
        &request.intent_id,
        request.page,
    )?;
    plan.put(
        Participant::AGENT_FANOUT_PAGE,
        immutable_put(
            table,
            crate::attr::ItemBuilder::new(codec::FANOUT_PAGE)
                .set(crate::attr::PK, s(page_key.pk))
                .set(crate::attr::SK, s(page_key.sk))
                .set("intentId", s(request.intent_id.clone()))
                .set("page", n(u64::from(request.page)))
                .set(
                    "childIds",
                    crate::attr::string_list(
                        request.children.iter().map(|child| child.agent.to_string()),
                    ),
                )
                .set("state", s("committed"))
                .set("createdAt", stamp(request.now))
                .build(),
        ),
    )?;

    for child in &request.children {
        let control_key = keys::agent_control(request.session, child.agent);
        plan.put(
            Participant::AGENT_CONTROL,
            immutable_put(
                table,
                crate::attr::ItemBuilder::new(codec::AGENT_CONTROL)
                    .set(crate::attr::PK, s(control_key.pk))
                    .set(crate::attr::SK, s(control_key.sk))
                    .set("agentId", s(child.agent.to_string()))
                    .set("sessionId", s(request.session.to_string()))
                    .set("generationId", s(child.generation.to_string()))
                    .set("status", s("queued"))
                    .set("revision", n(0))
                    .set("journalTail", n(0))
                    .set("fence", n(0))
                    .set("childBudgetRemaining", n(child.child_budget))
                    .set("childBudgetGranted", n(child.child_budget))
                    .set("createdAt", stamp(request.now))
                    .set("updatedAt", stamp(request.now))
                    .build(),
            ),
        )?;
        let index_key = keys::agent_index(request.session, child.agent);
        plan.put(
            Participant::AGENT_CONTROL,
            immutable_put(
                table,
                crate::attr::ItemBuilder::new(codec::AGENT_INDEX)
                    .set(crate::attr::PK, s(index_key.pk))
                    .set(crate::attr::SK, s(index_key.sk))
                    .set("agentId", s(child.agent.to_string()))
                    .set("parentAgentId", s(request.parent.to_string()))
                    .set("kind", s("subagent"))
                    .set("status", s("queued"))
                    .set("createdAt", stamp(request.now))
                    .build(),
            ),
        )?;
    }
    for wake in wakes {
        wake.push(&mut plan)?;
    }
    Ok(plan)
}

/// Compiles a trash, restore, purge admission or purge completion.
///
/// Every non-purge commit elsewhere conditions on `deletionEpoch`, so an
/// operation that raced the purge admission cannot commit afterwards. A purged
/// head keeps its item — stripped, `lifecycle = "purged"`, index attributes
/// removed — so one point read serves `200`, `410 session_deleting` and
/// `410 session_deleted`, and the deletion epoch stays conditionable forever.
///
/// # Errors
///
/// As [`compile_admission`].
#[allow(
    clippy::too_many_lines,
    reason = "one contiguous function per transaction keeps participant order readable in one place; splitting it would hide the order it exists to fix"
)]
pub fn compile_lifecycle(
    tables: &RegionalTables,
    request: &LifecyclePlan,
    foreign: Vec<Foreign>,
) -> Result<TransactionPlan, StoreError> {
    let table = tables.session_authority.as_str();
    let head_key = keys::head(request.session);
    let mut plan = TransactionPlan::new(format!(
        "lifecycle:{}:{:?}:{}",
        request.session, request.transition, request.revision
    ));
    let next = request.revision + 1;
    let base = || {
        aws_sdk_dynamodb::types::Update::builder()
            .table_name(table)
            .set_key(Some(key(&head_key.pk, &head_key.sk)))
    };

    match request.transition {
        LifecycleTransition::Trash => {
            plan.update(
                Participant::SESSION_HEAD,
                base()
                    .condition_expression(
                        "revision = :revision AND lifecycle = :active \
                         AND attribute_not_exists(activeRunId)",
                    )
                    .update_expression(
                        "SET lifecycle = :trashed, revision = :nextRevision, \
                         deletionEpoch = deletionEpoch + :one, cancelEpoch = cancelEpoch + :one, \
                         contentAdmissionEpoch = contentAdmissionEpoch + :one, \
                         trashedAt = :now, deletionOperationId = :operationId, \
                         wsIndexPk = :trashedIndexPk",
                    )
                    .expression_attribute_values(":revision", n(request.revision))
                    .expression_attribute_values(":active", s("active"))
                    .expression_attribute_values(":trashed", s("trashed"))
                    .expression_attribute_values(":nextRevision", n(next))
                    .expression_attribute_values(":one", n(1))
                    .expression_attribute_values(":now", stamp(request.now))
                    .expression_attribute_values(
                        ":operationId",
                        s(operation_id(request)?.to_string()),
                    )
                    .expression_attribute_values(
                        ":trashedIndexPk",
                        s(keys::workspace_index::session_partition(
                            request.workspace,
                            SessionLifecycle::Trashed.as_str(),
                        )),
                    ),
            )?;
        }
        LifecycleTransition::Restore => {
            plan.update(
                Participant::SESSION_HEAD,
                base()
                    .condition_expression("lifecycle = :trashed AND revision = :revision")
                    .update_expression(
                        "SET lifecycle = :active, revision = :nextRevision, \
                         wsIndexPk = :activeIndexPk REMOVE trashedAt",
                    )
                    .expression_attribute_values(":trashed", s("trashed"))
                    .expression_attribute_values(":revision", n(request.revision))
                    .expression_attribute_values(":active", s("active"))
                    .expression_attribute_values(":nextRevision", n(next))
                    .expression_attribute_values(
                        ":activeIndexPk",
                        s(keys::workspace_index::session_partition(
                            request.workspace,
                            SessionLifecycle::Active.as_str(),
                        )),
                    ),
            )?;
        }
        LifecycleTransition::PurgeAdmit => {
            plan.update(
                Participant::SESSION_HEAD,
                base()
                    .condition_expression(
                        "lifecycle IN (:active, :trashed) AND revision = :revision",
                    )
                    .update_expression(
                        "SET lifecycle = :purging, revision = :nextRevision, \
                         deletionEpoch = deletionEpoch + :one, cancelEpoch = cancelEpoch + :one, \
                         contentAdmissionEpoch = contentAdmissionEpoch + :one, \
                         deletionOperationId = :operationId",
                    )
                    .expression_attribute_values(":active", s("active"))
                    .expression_attribute_values(":trashed", s("trashed"))
                    .expression_attribute_values(":revision", n(request.revision))
                    .expression_attribute_values(":purging", s("purging"))
                    .expression_attribute_values(":nextRevision", n(next))
                    .expression_attribute_values(":one", n(1))
                    .expression_attribute_values(
                        ":operationId",
                        s(operation_id(request)?.to_string()),
                    ),
            )?;
        }
        LifecycleTransition::PurgeComplete => {
            plan.update(
                Participant::SESSION_HEAD,
                base()
                    .condition_expression(
                        "lifecycle = :purging AND deletionOperationId = :operationId",
                    )
                    .update_expression(
                        "SET lifecycle = :purged, purgedAt = :now, revision = :nextRevision \
                         REMOVE wsIndexPk, wsIndexSk, resolvedConfig, initialRootDigest, \
                         persistedRootDigest, continuity, activeRunId",
                    )
                    .expression_attribute_values(":purging", s("purging"))
                    .expression_attribute_values(
                        ":operationId",
                        s(operation_id(request)?.to_string()),
                    )
                    .expression_attribute_values(":purged", s("purged"))
                    .expression_attribute_values(":now", stamp(request.now))
                    .expression_attribute_values(":nextRevision", n(next)),
            )?;
        }
    }

    for action in foreign {
        action.push(&mut plan)?;
    }
    Ok(plan)
}

fn operation_id(request: &LifecyclePlan) -> Result<aex_wire::ids::OperationId, StoreError> {
    request.operation.ok_or(StoreError::Invalid {
        detail: "this lifecycle transition must name the operation that owns it".to_owned(),
    })
}
