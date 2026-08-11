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
use crate::wire_pending::{AgentDecisionPlan, EffectStage, FanoutPagePlan};

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

/// Whether generic operation cancellation for this kind is owned here.
///
/// The released session operations have dedicated verbs, telemetry owns its
/// own effect fence, and internal maintenance operations are not public.
#[must_use]
pub const fn operation_cancel_owned(kind: aex_operation_domain::OperationKind) -> bool {
    use aex_operation_domain::OperationKind;

    match kind {
        OperationKind::SessionCancel
        | OperationKind::SessionSuspend
        | OperationKind::SessionResume
        | OperationKind::SessionTerminate
        | OperationKind::SessionDelete
        | OperationKind::WorkspaceDelete
        | OperationKind::TelemetryExport
        | OperationKind::ContentGc => false,
    }
}

/// Refuses generic operation cancellation for the current session vocabulary.
///
/// Current-work cancellation is the explicit `session_cancel` lifecycle
/// operation. None of the current durable operation kinds is cancelable through
/// the generic operation endpoint.
///
/// # Errors
///
/// [`StoreError::Invalid`] for every currently released kind.
pub fn operation_cancel_requested(
    _table: &str,
    request: &OperationCancelRequest,
) -> Result<UpdateBuilder, StoreError> {
    if !request.kind.is_public()
        || !request.kind.cancelable_on_accept()
        || !operation_cancel_owned(request.kind)
    {
        return Err(StoreError::Invalid {
            detail: "a session-authority cancellation requires an owned customer-visible cancelable kind"
                .to_owned(),
        });
    }
    Err(StoreError::Invalid {
        detail: "the current operation vocabulary has no generic-cancelable kind".to_owned(),
    })
}

fn immutable_put(table: &str, item: crate::attr::Item) -> PutBuilder {
    aws_sdk_dynamodb::types::Put::builder()
        .table_name(table)
        .set_item(Some(item))
        .condition_expression(IMMUTABLE)
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
/// As for every conditional transaction compiler in this module.
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
                "SET revision = :nextRevision, journalTail = :nextTail, \
                 journalTailHash = :nextTailHash, hasJournal = :hasJournal, #status = :status, \
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
            .expression_attribute_values(":nextTailHash", s(request.entry.entry_id.clone()))
            .expression_attribute_values(":hasJournal", crate::attr::boolean(true))
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
/// plus the ordinary transaction-plan validation errors.
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
                    .set("hasJournal", crate::attr::boolean(false))
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
