//! Compiling one [`DecisionCommit`] into one `TransactWriteItems` plan.
//!
//! The whole decision is one transaction, and it is built with the workspace's single
//! [`TransactionPlan`] compiler. That compiler refuses an unconditional authority write, so
//! a Brain action that forgot its precondition cannot reach the service; it also names
//! every participant, so a cancellation comes back as "the effect was not prepared" rather
//! than as "action 4 failed".
//!
//! `aex_session_dynamodb::transactions::compile_decision` compiles the single-append form
//! of this transaction. Brain's decision carries *vectors* — several appends, two effect
//! writes, a spawn page, join shards — so the compiler here builds the same participants in
//! the same order over the same key templates and the same row codecs, and a test asserts
//! that the one-append case produces exactly [`DECISION_ORDER`]. Sharing the item shapes
//! matters; sharing one function that cannot express the decision does not.

use aex_brain_app::ports::{DecisionContext, SessionAuthority};
use aex_brain_domain::budget::{BudgetDelta, Dimension};
use aex_brain_domain::child::{ChildOutcome, ChildState};
use aex_brain_domain::commit::{
    ChildWrite, DecisionCommit, EffectWrite, JoinWrite, PublicMessagePart, PublicMessageRole,
    RunTransition, WakeCreate,
};
use aex_brain_domain::ids::{AgentKey, CancelEpoch};
use aex_brain_domain::journal::FinishReason;
use aex_session_domain::{
    AgentFence, DomainError, InterruptReason, Message, MessagePart, MessageRole, MessageState,
    RunOutcome, TerminalAttempt, UsageClosureId, claim_terminal,
};
use aex_session_dynamodb::attr::{ItemBuilder, n, s, stamp};
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::plan::{IMMUTABLE, Participant, TransactionPlan, key};
use aex_session_dynamodb::transactions::DECISION_ORDER;
use aex_session_dynamodb::{codec, keys as shared};
use aex_wire::error::ErrorCode;
use aex_wire::ids::{PrefixedId as _, Uuid7};
use aex_work_dynamodb::codec::{DeliveryEvidence, Payload, WorkRecord};

use crate::keys::{self, BrainKeyError};
use crate::translate;

/// The `itemType` of Brain's session-level budget row.
pub const BRAIN_BUDGET: &str = "brain_budget";
/// The `itemType` of one queued child in the scheduler index.
pub const BRAIN_QUEUED: &str = "brain_queued";
/// The `itemType` of one child index entry.
pub const BRAIN_CHILD: &str = "brain_child";
/// The `itemType` of one join group.
pub const BRAIN_JOIN: &str = "brain_join";
/// The `itemType` of one mailbox entry.
pub const BRAIN_MAILBOX: &str = "brain_mailbox";

/// The Brain-owned participants, named here because Brain owns the rows they touch.
pub mod participant {
    use aex_session_dynamodb::plan::Participant;

    /// The session-level Brain budget row.
    pub const SESSION_BUDGET: Participant = Participant::new("brain.session_budget");
    /// One agent's own budget node, carried on its control row.
    pub const AGENT_BUDGET: Participant = Participant::new("brain.agent_budget");
    /// One child index entry under its parent.
    pub const CHILD_INDEX: Participant = Participant::new("brain.child_index");
    /// One queued child in the scheduler index.
    pub const QUEUED_INDEX: Participant = Participant::new("brain.queued_index");
    /// One join group.
    pub const JOIN_GROUP: Participant = Participant::new("brain.join_group");
    /// One join shard counter.
    pub const JOIN_SHARD: Participant = Participant::new("brain.join_shard");
    /// One mailbox entry.
    pub const MAILBOX: Participant = Participant::new("brain.mailbox");
    /// One paged fanout intent.
    pub const FANOUT_INTENT: Participant = Participant::new("brain.fanout_intent");
}

/// The only physical tables Brain's durable store addresses.
///
/// These names are process configuration. Session ownership is intentionally absent: it is
/// read from each session head and arrives separately in [`DecisionContext`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrainTables {
    /// The `session-authority` table containing heads, controls, journals and effects.
    pub session_authority: String,
    /// The `regional-work` table containing durable continuation wakes.
    pub regional_work: String,
}

/// Why a decision could not be compiled.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PlanError {
    /// The decision would not fit one transaction.
    #[error(transparent)]
    Envelope(#[from] aex_brain_domain::commit::EnvelopeViolation),
    /// A key could not be rendered.
    #[error(transparent)]
    Key(#[from] BrainKeyError),
    /// The shared compiler refused an action.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// A root run boundary was incomplete or contradicted its strongly-read authority.
    #[error("invalid root run boundary: {0}")]
    Boundary(String),
}

/// The lowest-priority band a Brain wake may carry.
///
/// `regional-work` gives each band a lead on the due time, so a band is a scheduling
/// statement rather than a queue. Five bands exist; Brain uses band 1 for an ordinary
/// continuation and leaves band 0 for admission.
pub const CONTINUATION_PRIORITY: u8 = 1;

/// Builds the canonical session-head condition used by both decisions and pre-dispatch.
///
/// The typed claim facts remain the authority: callers supply the cancellation epoch from
/// their fence guard and the tenant/deletion facts returned by the session-head read.
/// Keeping the expression here prevents ticket minting and decision commits from growing
/// subtly different lifecycle guards.
pub(crate) fn session_head_guard(
    table: &str,
    session: aex_wire::ids::SessionId,
    cancel_epoch: CancelEpoch,
    authority: &SessionAuthority,
) -> aws_sdk_dynamodb::types::builders::ConditionCheckBuilder {
    let head_key = shared::head(session);
    aws_sdk_dynamodb::types::ConditionCheck::builder()
        .table_name(table)
        .set_key(Some(key(&head_key.pk, &head_key.sk)))
        .condition_expression(
            "cancelEpoch = :cancelEpoch AND deletionEpoch = :deletionEpoch \
             AND workspaceId = :workspaceId AND organizationId = :organizationId \
             AND lifecycle = :active",
        )
        .expression_attribute_values(":cancelEpoch", n(cancel_epoch.0))
        .expression_attribute_values(":deletionEpoch", n(authority.deletion_epoch))
        .expression_attribute_values(":workspaceId", s(authority.workspace.to_string()))
        .expression_attribute_values(":organizationId", s(authority.organization.to_string()))
        .expression_attribute_values(":active", s("active"))
}

/// Compiles one decision into one transaction plan.
///
/// # Errors
///
/// [`PlanError::Envelope`] when the decision exceeds the transaction envelope — checked
/// here, before the round trip, because `DynamoDB` rejects an over-large transaction with
/// no usable diagnosis. [`PlanError::Key`] when an identifier has no wire form, and
/// [`PlanError::Store`] when an action would be unconditional or over-large.
#[allow(
    clippy::too_many_lines,
    reason = "one contiguous function per transaction keeps participant order readable in one place; splitting it would hide the order it exists to fix"
)]
pub fn compile(
    tables: &BrainTables,
    context: &DecisionContext,
    commit: &DecisionCommit,
) -> Result<TransactionPlan, PlanError> {
    commit.validate()?;

    let agent_key = commit.guard.key;
    let table = tables.session_authority.as_str();
    let work_table = tables.regional_work.as_str();
    let (session, agent) = translate::agent_key(&agent_key).map_err(BrainKeyError::from)?;
    let now = translate::at(context.now, "now").map_err(BrainKeyError::from)?;
    let lease = translate::at(context.lease_expires_at, "lease").map_err(BrainKeyError::from)?;

    let mut plan = TransactionPlan::new(client_request_token(commit));

    let terminal = terminal_commit(context, commit, agent)?;

    // 1. Ordinary decisions use a read-only session guard. A run boundary replaces it
    //    with the canonical session-head write below: DynamoDB forbids checking and
    //    writing the same item in one transaction, and that put carries the identical
    //    tenant/deletion/cancellation guards plus revision and active-run fences.
    if terminal.is_none() {
        plan.condition_check(
            Participant::SESSION_HEAD_GUARD,
            session_head_guard(
                table,
                session,
                commit.guard.cancel_epoch,
                &context.authority,
            ),
        )
        .map_err(PlanError::Store)?;
    }

    // 2. The control item, under the whole precondition set. All five, always: each
    //    rejects a different way of being stale, and dropping one lets that writer publish.
    let control_key = shared::agent_control(session, agent);
    if commit.is_retirement_only() {
        let tail_condition = if commit.guard.tail.is_some() {
            "journalTail = :tail"
        } else {
            "attribute_not_exists(journalTail)"
        };
        let mut guard = aws_sdk_dynamodb::types::ConditionCheck::builder()
            .table_name(table)
            .set_key(Some(key(&control_key.pk, &control_key.sk)))
            .condition_expression(format!(
                "revision = :revision AND fence = :fence AND claimOwner = :owner AND {tail_condition}"
            ))
            .expression_attribute_values(":revision", n(commit.guard.revision.0))
            .expression_attribute_values(":fence", n(commit.guard.fence.0))
            .expression_attribute_values(
                ":owner",
                s(commit.guard.owner.0.as_hyphenated().to_string()),
            );
        if let Some(tail) = commit.guard.tail {
            guard = guard.expression_attribute_values(":tail", n(tail.get()));
        }
        plan.condition_check(Participant::AGENT_CONTROL, guard)
            .map_err(PlanError::Store)?;
    } else {
        let next_tail_hash = commit
            .appends
            .last()
            .map(aex_brain_domain::journal::JournalRecord::content_hash)
            .transpose()
            .map_err(aex_brain_domain::commit::EnvelopeViolation::from)?;
        let mut control = aws_sdk_dynamodb::types::Update::builder()
            .table_name(table)
            .set_key(Some(key(&control_key.pk, &control_key.sk)))
            .condition_expression(
                "revision = :revision AND fence = :fence AND claimOwner = :owner \
                 AND journalTail = :tail",
            )
            .expression_attribute_names("#status", "status")
            .expression_attribute_values(":revision", n(commit.guard.revision.0))
            .expression_attribute_values(":fence", n(commit.guard.fence.0))
            .expression_attribute_values(
                ":owner",
                s(commit.guard.owner.0.as_hyphenated().to_string()),
            )
            .expression_attribute_values(
                ":tail",
                n(commit
                    .guard
                    .tail
                    .map_or(0, aex_brain_domain::ids::JournalSeq::get)),
            )
            .expression_attribute_values(":nextRevision", n(commit.control.next_revision.0))
            .expression_attribute_values(":nextTail", n(commit.control.next_tail.get()))
            .expression_attribute_values(":status", s(commit.control.phase.clone()))
            .expression_attribute_values(":lease", stamp(lease))
            .expression_attribute_values(":now", stamp(now));
        let mut set_clause = "revision = :nextRevision, journalTail = :nextTail, \
                              #status = :status, leaseExpiresAt = :lease, updatedAt = :now"
            .to_owned();
        if let Some(hash) = next_tail_hash {
            set_clause.push_str(", journalTailHash = :nextTailHash, hasJournal = :hasJournal");
            control = control
                .expression_attribute_values(":nextTailHash", s(hash.to_hex()))
                .expression_attribute_values(
                    ":hasJournal",
                    aex_session_dynamodb::attr::boolean(true),
                );
        }
        if let Some(finish) = commit.control.finish {
            set_clause.push_str(", finishReason = :finish");
            control = control.expression_attribute_values(":finish", s(finish_name(finish)));
        }
        // The agent's own budget node lives on its control row, so its movements ride inside
        // this one update rather than as a second action. Two actions on one item are illegal
        // in a `DynamoDB` transaction, and giving the node its own row would give it a second
        // fence to keep in step with this one.
        let mut adds = Vec::new();
        for (index, delta) in commit.budget.iter().enumerate() {
            let placeholder = format!(":b{index}");
            adds.push(format!("{} {placeholder}", used_attribute(delta.dimension)));
            control = control.expression_attribute_values(placeholder, n(delta.quantity));
        }
        let clear_stop_latch = terminal.as_ref().is_some_and(|terminal| {
            matches!(
                terminal.run.outcome.as_ref(),
                Some(aex_session_domain::RunOutcome::Cancelled { .. })
            )
        });
        let remove_clause = if clear_stop_latch {
            " REMOVE stopRequested"
        } else {
            ""
        };
        let expression = if adds.is_empty() {
            format!("SET {set_clause}{remove_clause}")
        } else {
            format!("SET {set_clause}{remove_clause} ADD {}", adds.join(", "))
        };
        plan.update(
            Participant::AGENT_CONTROL,
            control.update_expression(expression),
        )
        .map_err(PlanError::Store)?;
    }

    // 3. Every append, immutable. `attribute_not_exists` is what makes a redelivered
    //    decision idempotent: the second attempt loses the journal put instead of
    //    appending a duplicate record.
    let first_seq = commit.guard.tail.map_or(
        aex_brain_domain::ids::JournalSeq::ZERO,
        aex_brain_domain::ids::JournalSeq::next,
    );
    for (offset, record) in commit.appends.iter().enumerate() {
        let seq = first_seq.get().saturating_add(offset as u64);
        let body = record
            .canonical_bytes()
            .map_err(aex_brain_domain::commit::EnvelopeViolation::from)?;
        let hash = aex_brain_domain::ids::ContentHash::of(&body);
        let stored = codec::encode_journal(
            session,
            agent,
            &aex_session_dynamodb::wire_pending::JournalEntry {
                seq,
                entry_id: hash.to_hex(),
                kind: record.kind_name().to_owned(),
                body: aex_session_dynamodb::wire_pending::Body::Inline(body.clone()),
                body_bytes: body.len() as u64,
                occurred_at: now,
            },
        );
        plan.put(
            Participant::AGENT_JOURNAL,
            aws_sdk_dynamodb::types::Put::builder()
                .table_name(table)
                .set_item(Some(stored))
                .condition_expression(IMMUTABLE),
        )
        .map_err(PlanError::Store)?;
    }

    // Complete public output messages are born sealed in the same decision as the
    // complete assistant/tool journal record. They are never reconstructed from preview
    // deltas, and the immutable seal-order row makes list pagination stable.
    for append in &commit.messages {
        let active = context.authority.active.as_deref().ok_or_else(|| {
            PlanError::Boundary("a public output message has no active run authority".to_owned())
        })?;
        if append.run != active.run.id || active.session.id != session {
            return Err(PlanError::Boundary(
                "a public output message crosses its active session or run".to_owned(),
            ));
        }
        let message = Message {
            id: append.id,
            session,
            run: Some(append.run),
            agent,
            role: match append.role {
                PublicMessageRole::Assistant => MessageRole::Assistant,
                PublicMessageRole::Tool => MessageRole::Tool,
            },
            state: MessageState::Sealed,
            parts: append
                .parts
                .iter()
                .map(|part| match part {
                    PublicMessagePart::Text { text } => MessagePart::Text { text: text.clone() },
                    PublicMessagePart::ToolCall { id, arguments } => MessagePart::ToolCall {
                        id: *id,
                        arguments: *arguments,
                    },
                    PublicMessagePart::ToolResult { id, result } => MessagePart::ToolResult {
                        id: *id,
                        result: *result,
                    },
                })
                .collect(),
            created_at: translate::at(append.at, "public message instant")
                .map_err(BrainKeyError::from)?,
            sealed_at: Some(
                translate::at(append.at, "public message seal instant")
                    .map_err(BrainKeyError::from)?,
            ),
        };
        let base = aex_session_dynamodb::authority_codec::encode_domain_message(
            &message,
            context.authority.workspace,
            context.authority.organization,
        )
        .map_err(StoreError::from)?;
        plan.put(
            Participant::SESSION_MESSAGE,
            aws_sdk_dynamodb::types::Put::builder()
                .table_name(table)
                .set_item(Some(base))
                .condition_expression(IMMUTABLE),
        )
        .map_err(PlanError::Store)?;
        let projection = aex_session_dynamodb::authority_codec::encode_sealed_message(
            &message,
            context.authority.workspace,
            context.authority.organization,
        )
        .map_err(StoreError::from)?;
        plan.put(
            Participant::SESSION_SEALED_MESSAGE,
            aws_sdk_dynamodb::types::Put::builder()
                .table_name(table)
                .set_item(Some(projection))
                .condition_expression(IMMUTABLE),
        )
        .map_err(PlanError::Store)?;
    }

    // 4. Effect writes. A preparation is immutable; a settlement conditions on the request
    //    hash it was prepared under, so the same effect id with a different request is a
    //    refusal rather than a silent second generation.
    for write in &commit.effects {
        match write {
            EffectWrite::Prepare {
                id,
                kind,
                generation,
                class,
                request_hash,
                deadline,
                attempt,
            } => {
                let effect_key = keys::effect(&agent_key, *id)?;
                plan.put(
                    Participant::AGENT_EFFECT,
                    aws_sdk_dynamodb::types::Put::builder()
                        .table_name(table)
                        .set_item(Some(
                            ItemBuilder::new(codec::AGENT_EFFECT)
                                .set(aex_session_dynamodb::attr::PK, s(effect_key.pk))
                                .set(aex_session_dynamodb::attr::SK, s(effect_key.sk))
                                .set("effectId", s(id.to_hex()))
                                .set("kind", s(format!("{kind:?}")))
                                .set_opt(
                                    "generationId",
                                    generation.map(|value| s(value.to_string())),
                                )
                                .set("effectClass", s(format!("{class:?}")))
                                .set("requestHash", s(request_hash.to_hex()))
                                .set("attempt", n(u64::from(*attempt)))
                                .set("state", s("prepared"))
                                .set("agentFence", n(commit.guard.fence.0))
                                .set(
                                    "deadline",
                                    stamp(
                                        translate::at(*deadline, "deadline")
                                            .map_err(BrainKeyError::from)?,
                                    ),
                                )
                                .set("preparedAt", stamp(now))
                                .build(),
                        ))
                        .condition_expression(IMMUTABLE),
                )
                .map_err(PlanError::Store)?;
            }
            EffectWrite::Settle { id, outcome } => {
                let effect_key = keys::effect(&agent_key, *id)?;
                plan.update(
                    Participant::AGENT_EFFECT,
                    aws_sdk_dynamodb::types::Update::builder()
                        .table_name(table)
                        .set_key(Some(key(&effect_key.pk, &effect_key.sk)))
                        .condition_expression("#state <> :settled AND effectId = :id")
                        .update_expression("SET #state = :next, settledAt = :now")
                        .expression_attribute_names("#state", "state")
                        .expression_attribute_values(":settled", s("settled"))
                        .expression_attribute_values(":id", s(id.to_hex()))
                        .expression_attribute_values(":next", s(settled_name(outcome)))
                        .expression_attribute_values(":now", stamp(now)),
                )
                .map_err(PlanError::Store)?;
            }
        }
    }

    // 5. The session's shared budget node is its own row, so a spawn checks and consumes
    //    the active limit in one action and two claimants can never both conclude there is
    //    room. The agent's own node was folded into the control update above.
    if !commit.session_budget.is_empty() {
        let budget_key = keys::session_budget(agent_key.session)?;
        plan.update(
            participant::SESSION_BUDGET,
            budget_update(table, &budget_key, &commit.session_budget, now)
                .condition_expression("cancelEpoch = :cancelEpoch")
                .expression_attribute_values(":cancelEpoch", n(commit.guard.cancel_epoch.0)),
        )
        .map_err(PlanError::Store)?;
    }

    // 6. Children. A spawn costs three actions — the child index entry, the queued index
    //    entry and the child's own control row — which is exactly where the 32-child page
    //    size comes from.
    for write in &commit.children {
        match write {
            ChildWrite::Spawn {
                child,
                ordinal,
                grant,
                join,
                queued_reason,
            } => {
                let child_key = AgentKey::new(agent_key.session, *child);
                let index = keys::child_index(&agent_key, *ordinal, *child)?;
                plan.put(
                    participant::CHILD_INDEX,
                    aws_sdk_dynamodb::types::Put::builder()
                        .table_name(table)
                        .set_item(Some(
                            ItemBuilder::new(BRAIN_CHILD)
                                .set(aex_session_dynamodb::attr::PK, s(index.pk))
                                .set(aex_session_dynamodb::attr::SK, s(index.sk))
                                .set("workspaceId", s(context.authority.workspace.to_string()))
                                .set("childAgentId", s(child.0.as_hyphenated().to_string()))
                                .set("ordinal", n(u64::from(*ordinal)))
                                .set("joinId", s(join.0.as_hyphenated().to_string()))
                                .set("state", s("queued"))
                                .set_opt(
                                    "queuedReason",
                                    queued_reason.map(|reason| s(queued_reason_name(reason))),
                                )
                                .set("grant", grant_attribute(*grant))
                                .set("createdAt", stamp(now))
                                .build(),
                        ))
                        .condition_expression(IMMUTABLE),
                )
                .map_err(PlanError::Store)?;

                let queued = keys::queued_index(
                    agent_key.session,
                    CONTINUATION_PRIORITY,
                    context.now.millis(),
                    *child,
                )?;
                plan.put(
                    participant::QUEUED_INDEX,
                    aws_sdk_dynamodb::types::Put::builder()
                        .table_name(table)
                        .set_item(Some(
                            ItemBuilder::new(BRAIN_QUEUED)
                                .set(aex_session_dynamodb::attr::PK, s(queued.pk))
                                .set(aex_session_dynamodb::attr::SK, s(queued.sk))
                                .set("workspaceId", s(context.authority.workspace.to_string()))
                                .set("parentAgentId", s(agent.to_string()))
                                .set("childAgentId", s(child.0.as_hyphenated().to_string()))
                                .set("grant", grant_attribute(*grant))
                                .set_opt(
                                    "queuedReason",
                                    queued_reason.map(|reason| s(queued_reason_name(reason))),
                                )
                                .set("enqueuedAt", stamp(now))
                                .build(),
                        ))
                        .condition_expression(IMMUTABLE),
                )
                .map_err(PlanError::Store)?;

                let child_control = keys::control(&child_key)?;
                plan.put(
                    Participant::AGENT_CONTROL,
                    aws_sdk_dynamodb::types::Put::builder()
                        .table_name(table)
                        .set_item(Some(
                            ItemBuilder::new(codec::AGENT_CONTROL)
                                .set(aex_session_dynamodb::attr::PK, s(child_control.pk))
                                .set(aex_session_dynamodb::attr::SK, s(child_control.sk))
                                .set("workspaceId", s(context.authority.workspace.to_string()))
                                .set("agentId", s(child.0.as_hyphenated().to_string()))
                                .set("sessionId", s(session.to_string()))
                                .set("parentAgentId", s(agent.to_string()))
                                .set("status", s("queued"))
                                .set("revision", n(0))
                                .set("journalTail", n(0))
                                .set("fence", n(0))
                                .set("cancelEpoch", n(commit.guard.cancel_epoch.0))
                                .set("createdAt", stamp(now))
                                .set("updatedAt", stamp(now))
                                .build(),
                        ))
                        .condition_expression(IMMUTABLE),
                )
                .map_err(PlanError::Store)?;
            }
            ChildWrite::Transition {
                child,
                state,
                observed_fence,
            } => {
                let child_key = AgentKey::new(agent_key.session, *child);
                let child_control = keys::control(&child_key)?;
                plan.update(
                    Participant::AGENT_CONTROL,
                    aws_sdk_dynamodb::types::Update::builder()
                        .table_name(table)
                        .set_key(Some(key(&child_control.pk, &child_control.sk)))
                        .condition_expression("fence = :fence")
                        .update_expression("SET #status = :status, updatedAt = :now")
                        .expression_attribute_names("#status", "status")
                        .expression_attribute_values(":fence", n(observed_fence.0))
                        .expression_attribute_values(":status", s(child_state_name(*state)))
                        .expression_attribute_values(":now", stamp(now)),
                )
                .map_err(PlanError::Store)?;
            }
            ChildWrite::Terminal { child, outcome } => {
                let child_key = AgentKey::new(agent_key.session, *child);
                let child_control = keys::control(&child_key)?;
                plan.update(
                    Participant::AGENT_CONTROL,
                    aws_sdk_dynamodb::types::Update::builder()
                        .table_name(table)
                        .set_key(Some(key(&child_control.pk, &child_control.sk)))
                        .condition_expression("attribute_exists(pk)")
                        .update_expression("SET #status = :status, updatedAt = :now")
                        .expression_attribute_names("#status", "status")
                        .expression_attribute_values(
                            ":status",
                            s(child_state_name(outcome.state())),
                        )
                        .expression_attribute_values(":now", stamp(now)),
                )
                .map_err(PlanError::Store)?;

                let ret = keys::budget_return(&agent_key, *child)?;
                plan.put(
                    participant::CHILD_INDEX,
                    aws_sdk_dynamodb::types::Put::builder()
                        .table_name(table)
                        .set_item(Some(
                            ItemBuilder::new(codec::AGENT_INDEX)
                                .set(aex_session_dynamodb::attr::PK, s(ret.pk))
                                .set(aex_session_dynamodb::attr::SK, s(ret.sk))
                                .set("workspaceId", s(context.authority.workspace.to_string()))
                                .set("agentId", s(child.0.as_hyphenated().to_string()))
                                .set("outcome", s(child_outcome_name(*outcome)))
                                .set("createdAt", stamp(now))
                                .build(),
                        ))
                        .condition_expression(IMMUTABLE),
                )
                .map_err(PlanError::Store)?;
            }
            ChildWrite::FanoutIntent {
                intent,
                total,
                grant_template,
            } => {
                let intent_key = keys::fanout_intent(agent_key.session, *intent)?;
                plan.put(
                    participant::FANOUT_INTENT,
                    aws_sdk_dynamodb::types::Put::builder()
                        .table_name(table)
                        .set_item(Some(
                            ItemBuilder::new(codec::FANOUT_PAGE)
                                .set(aex_session_dynamodb::attr::PK, s(intent_key.pk))
                                .set(aex_session_dynamodb::attr::SK, s(intent_key.sk))
                                .set("workspaceId", s(context.authority.workspace.to_string()))
                                .set("intentId", s(intent.0.as_hyphenated().to_string()))
                                .set("parentAgentId", s(agent.to_string()))
                                .set("total", n(u64::from(*total)))
                                .set("pagesCommitted", n(0))
                                .set("grantTemplate", grant_attribute(*grant_template))
                                .set("createdAt", stamp(now))
                                .build(),
                        ))
                        .condition_expression(IMMUTABLE),
                )
                .map_err(PlanError::Store)?;
            }
        }
    }

    // 7. Joins. The shard counter is a projection: it is incremented here, and a
    //    disagreement with membership makes the parent page the immutable child terminal
    //    rows instead of believing the counter.
    for write in &commit.joins {
        match write {
            JoinWrite::Open {
                join,
                mode,
                members,
                shards,
            } => {
                let group = keys::join_group(&agent_key, *join)?;
                plan.put(
                    participant::JOIN_GROUP,
                    aws_sdk_dynamodb::types::Put::builder()
                        .table_name(table)
                        .set_item(Some(
                            ItemBuilder::new(BRAIN_JOIN)
                                .set(aex_session_dynamodb::attr::PK, s(group.pk))
                                .set(aex_session_dynamodb::attr::SK, s(group.sk))
                                .set("workspaceId", s(context.authority.workspace.to_string()))
                                .set("joinId", s(join.0.as_hyphenated().to_string()))
                                .set("mode", s(format!("{mode:?}").to_lowercase()))
                                .set("shards", n(u64::from(*shards)))
                                .set(
                                    "members",
                                    aex_session_dynamodb::attr::string_list(
                                        members.iter().map(|m| m.0.as_hyphenated().to_string()),
                                    ),
                                )
                                .set("createdAt", stamp(now))
                                .build(),
                        ))
                        .condition_expression(IMMUTABLE),
                )
                .map_err(PlanError::Store)?;
            }
            JoinWrite::IncrementShard { join, shard } => {
                let shard_key = keys::join_shard(&agent_key, *join, *shard)?;
                plan.update(
                    participant::JOIN_SHARD,
                    aws_sdk_dynamodb::types::Update::builder()
                        .table_name(table)
                        .set_key(Some(key(&shard_key.pk, &shard_key.sk)))
                        .condition_expression("attribute_not_exists(sealedAt)")
                        .update_expression(
                            "SET itemType = :itemType, updatedAt = :now ADD doneCount :one",
                        )
                        .expression_attribute_values(":itemType", s("join_shard"))
                        .expression_attribute_values(":one", n(1))
                        .expression_attribute_values(":now", stamp(now)),
                )
                .map_err(PlanError::Store)?;
            }
        }
    }

    // 8. Preview events, through the session event row the observation stream reads.
    for event in &commit.events {
        let event_key = shared::event(session, event.event_seq);
        plan.put(
            Participant::SESSION_PREVIEW_EVENT,
            aws_sdk_dynamodb::types::Put::builder()
                .table_name(table)
                .set_item(Some(
                    ItemBuilder::new(codec::SESSION_EVENT)
                        .set(aex_session_dynamodb::attr::PK, s(event_key.pk))
                        .set(aex_session_dynamodb::attr::SK, s(event_key.sk))
                        .set("eventSeq", n(event.event_seq))
                        .set("eventType", s(event.kind.clone()))
                        .set("agentId", s(agent.to_string()))
                        .set(
                            "contentInline",
                            aex_session_dynamodb::attr::b(
                                serde_json::to_vec(&event.body).unwrap_or_default(),
                            ),
                        )
                        .set("occurredAt", stamp(now))
                        .set("outboxState", s("pending"))
                        .build(),
                ))
                .condition_expression(IMMUTABLE),
        )
        .map_err(PlanError::Store)?;
    }

    for event in &commit.session_events {
        let occurred_at =
            translate::at(event.at, "public session event instant").map_err(BrainKeyError::from)?;
        let body = serde_json::to_vec(&serde_json::json!({
            "messageId": event.message,
            "outcome": event.outcome,
        }))
        .map_err(|error| PlanError::Boundary(format!("session event body: {error}")))?;
        let stored = aex_session_dynamodb::wire_pending::SessionEvent {
            workspace: context.authority.workspace,
            event_seq: event.event_seq,
            event_id: aex_wire::ids::ObservationId::from_uuid7(event.message.uuid7()),
            event_type: "session.message_completed".to_owned(),
            run: None,
            agent: None,
            body: aex_session_dynamodb::wire_pending::Body::Inline(body),
            occurred_at,
            outbox_state: "pending",
        };
        plan.put(
            Participant::SESSION_COMPLETED_EVENT,
            aws_sdk_dynamodb::types::Put::builder()
                .table_name(table)
                .set_item(Some(aex_session_dynamodb::codec::encode_event(
                    session, &stored,
                )))
                .condition_expression(IMMUTABLE),
        )
        .map_err(PlanError::Store)?;
    }

    // 9. Wakes, through the work adapter's own builders. Brain never forks that row shape,
    //    and the queue never originates a wake: the durable item is the fact.
    for wake in &commit.wakes {
        let record = wake_record(context, wake)?;
        plan.put(
            Participant::WORK_NEXT_WAKE,
            aex_work_dynamodb::claim::enqueue(work_table, &record).map_err(PlanError::Store)?,
        )
        .map_err(PlanError::Store)?;
        plan.put(
            aex_work_dynamodb::claim::DEDUPE,
            aex_work_dynamodb::claim::enqueue_dedupe(work_table, &record)
                .map_err(PlanError::Store)?,
        )
        .map_err(PlanError::Store)?;
    }

    // 10. The source wake is satisfied only inside this guarded transaction. The work
    //     adapter owns the row shape and requires the exact pending, unclaimed agent wake;
    //     completion removes both sparse due-index keys, so a successful activation cannot
    //     be resurrected by the backstop.
    if let Some(retired) = &commit.retired_wake {
        let expected = aex_work_dynamodb::claim::PendingAgentWake {
            work_id: retired.work_id.clone(),
            workspace: context.authority.workspace,
            session,
            agent,
        };
        plan.update(
            aex_work_dynamodb::claim::WAKE_DONE,
            aex_work_dynamodb::claim::complete_pending_agent_wake(work_table, &expected, now)
                .map_err(PlanError::Store)?,
        )
        .map_err(PlanError::Store)?;
    }

    // The internal run, retained session head and accounting outbox fact cross their
    // terminal barrier together. The outbox row is an internal billing authority; public
    // session events are projected separately and never expose this run identity.
    if let Some(terminal) = terminal {
        let run_item = aex_session_dynamodb::authority_codec::encode_domain_run(
            &terminal.run,
            context.authority.workspace,
            context.authority.organization,
        )
        .map_err(StoreError::from)?;
        plan.put(
            Participant::SESSION_RUN,
            aws_sdk_dynamodb::types::Put::builder()
                .table_name(table)
                .set_item(Some(run_item))
                .condition_expression("runId = :run AND (#status = :queued OR #status = :running)")
                .expression_attribute_names("#status", "status")
                .expression_attribute_values(":run", s(terminal.run.id.to_string()))
                .expression_attribute_values(":queued", s("queued"))
                .expression_attribute_values(":running", s("running")),
        )
        .map_err(PlanError::Store)?;

        let head_item = aex_session_dynamodb::authority_codec::encode_session(&terminal.session)
            .map_err(StoreError::from)?;
        plan.put(
            Participant::SESSION_HEAD,
            aws_sdk_dynamodb::types::Put::builder()
                .table_name(table)
                .set_item(Some(head_item))
                .condition_expression(
                    "revision = :revision AND activeRunId = :run \
                     AND cancelEpoch = :cancelEpoch AND deletionEpoch = :deletionEpoch \
                     AND workspaceId = :workspaceId AND organizationId = :organizationId \
                     AND lifecycle = :active",
                )
                .expression_attribute_values(
                    ":revision",
                    n(terminal.session.revision.0.saturating_sub(1)),
                )
                .expression_attribute_values(":run", s(terminal.run.id.to_string()))
                .expression_attribute_values(":cancelEpoch", n(commit.guard.cancel_epoch.0))
                .expression_attribute_values(":deletionEpoch", n(context.authority.deletion_epoch))
                .expression_attribute_values(
                    ":workspaceId",
                    s(context.authority.workspace.to_string()),
                )
                .expression_attribute_values(
                    ":organizationId",
                    s(context.authority.organization.to_string()),
                )
                .expression_attribute_values(":active", s("active")),
        )
        .map_err(PlanError::Store)?;

        plan.put(
            Participant::SESSION_TERMINAL_EVENT,
            aws_sdk_dynamodb::types::Put::builder()
                .table_name(table)
                .set_item(Some(aex_session_dynamodb::codec::encode_outbox_event(
                    context.authority.workspace,
                    &terminal.outbox,
                )))
                .condition_expression(IMMUTABLE),
        )
        .map_err(PlanError::Store)?;
    }

    // The domain validator includes the head guard (or its boundary replacement); this
    // final assertion checks the actual compiled provider plan as a second line of defence.
    if plan.len() > aex_session_dynamodb::plan::MAX_ACTIONS {
        return Err(PlanError::Envelope(
            aex_brain_domain::commit::EnvelopeViolation::TooManyActions {
                actions: plan.len(),
            },
        ));
    }
    Ok(plan)
}

fn terminal_commit(
    context: &DecisionContext,
    commit: &DecisionCommit,
    agent: aex_wire::ids::AgentId,
) -> Result<Option<aex_session_domain::TerminalCommit>, PlanError> {
    let (Some(run), Some(session)) = (&commit.run, &commit.session) else {
        if commit.run.is_some() || commit.session.is_some() {
            return Err(PlanError::Boundary(
                "run and session transitions must be present together".to_owned(),
            ));
        }
        if !commit.session_events.is_empty()
            || commit.appends.iter().any(|record| {
                matches!(
                    record,
                    aex_brain_domain::journal::JournalRecord::RunFinished { .. }
                )
            })
        {
            return Err(PlanError::Boundary(
                "a run-finished record or completion event requires the terminal transition"
                    .to_owned(),
            ));
        }
        return Ok(None);
    };
    if session.status != "idle" {
        return Err(PlanError::Boundary(
            "a root run boundary must return the session to idle".to_owned(),
        ));
    }
    let active =
        context.authority.active.as_deref().ok_or_else(|| {
            PlanError::Boundary("the claimant did not bind an active run".to_owned())
        })?;
    if active.run.id != run.run
        || active.session.active_run != Some(run.run)
        || active.session.revision.0 != session.revision
    {
        return Err(PlanError::Boundary(
            "the boundary disagrees with its strongly-read session/run authority".to_owned(),
        ));
    }
    let finished = commit.appends.last().ok_or_else(|| {
        PlanError::Boundary("a root run boundary has no final journal record".to_owned())
    })?;
    let aex_brain_domain::journal::JournalRecord::RunFinished {
        run: journal_run,
        reason,
        failure,
        output_messages,
        ambiguous_effect,
    } = finished
    else {
        return Err(PlanError::Boundary(
            "the terminal transition is not closed by RunFinished".to_owned(),
        ));
    };
    if commit
        .appends
        .iter()
        .filter(|record| {
            matches!(
                record,
                aex_brain_domain::journal::JournalRecord::RunFinished { .. }
            )
        })
        .count()
        != 1
        || *journal_run != run.run
        || *reason != run.finish
        || failure != &run.failure
        || output_messages != &run.output_messages
        || ambiguous_effect != &run.ambiguous_effect
    {
        return Err(PlanError::Boundary(
            "RunFinished disagrees with the terminal transition".to_owned(),
        ));
    }
    let [event] = commit.session_events.as_slice() else {
        return Err(PlanError::Boundary(
            "a root run boundary requires exactly one public completion event".to_owned(),
        ));
    };
    let first_seq = commit.guard.tail.map_or(
        aex_brain_domain::ids::JournalSeq::ZERO,
        aex_brain_domain::ids::JournalSeq::next,
    );
    let boundary_seq =
        aex_brain_domain::ids::JournalSeq(first_seq.get().saturating_add(
            u64::try_from(commit.appends.len().saturating_sub(1)).unwrap_or(u64::MAX),
        ));
    if event.message != active.run.message
        || event.outcome != finish_outcome(run.finish)
        || event.event_seq != aex_brain_domain::commit::event_seq(boundary_seq, 0)
        || event.at != context.now
    {
        return Err(PlanError::Boundary(
            "the public completion event disagrees with the run boundary".to_owned(),
        ));
    }
    let attempt = TerminalAttempt {
        run: run.run,
        outcome: terminal_outcome(run, &active.run)?,
        at: translate::at(context.now, "run terminal instant").map_err(BrainKeyError::from)?,
        session_revision_seen: active.session.revision,
        cancellation_seen: active.session.cancellation,
        agent_fence: AgentFence(commit.guard.fence.0),
        usage_closure: UsageClosureId(active.run.id.uuid7()),
    };
    claim_terminal(
        &active.run,
        &active.session,
        agent,
        AgentFence(commit.guard.fence.0),
        &[],
        &attempt,
    )
    .map(Some)
    .map_err(|error| PlanError::Boundary(error.to_string()))
}

const fn finish_outcome(reason: FinishReason) -> &'static str {
    match reason {
        FinishReason::Completed => "succeeded",
        FinishReason::Timeout => "timed_out",
        FinishReason::Cancelled => "cancelled",
        FinishReason::Interrupted | FinishReason::Budget => "interrupted",
        FinishReason::MaxTurns | FinishReason::MaxSteps | FinishReason::Failed => "failed",
    }
}

fn terminal_outcome(
    transition: &RunTransition,
    run: &aex_session_domain::Run,
) -> Result<RunOutcome, PlanError> {
    Ok(match transition.finish {
        FinishReason::Completed => RunOutcome::Succeeded {
            output_messages: transition.output_messages.clone(),
        },
        FinishReason::Timeout => RunOutcome::TimedOut {
            deadline: run.deadline,
        },
        FinishReason::Budget => RunOutcome::Interrupted(InterruptReason::SpendCapExhausted),
        FinishReason::Cancelled => RunOutcome::Cancelled {
            by: transition.cancellation.ok_or_else(|| {
                PlanError::Boundary("a cancelled run has no cancellation operation".to_owned())
            })?,
        },
        FinishReason::Interrupted => {
            let effect = transition.ambiguous_effect.ok_or_else(|| {
                PlanError::Boundary("an interrupted run has no ambiguous effect".to_owned())
            })?;
            let mut entropy = [0_u8; 10];
            entropy.copy_from_slice(&effect.0[..10]);
            RunOutcome::Interrupted(InterruptReason::AmbiguousEffect {
                effect: aex_session_domain::EffectId(Uuid7::compose(
                    transition.run.uuid7().unix_millis(),
                    entropy,
                )),
            })
        }
        FinishReason::MaxTurns | FinishReason::MaxSteps => RunOutcome::Failed {
            error: DomainError {
                code: ErrorCode::LimitExceeded,
                message: "the current message reached its execution limit".to_owned(),
                detail: None,
                retryable: false,
            },
        },
        FinishReason::Failed => {
            let failure = transition.failure.as_ref();
            RunOutcome::Failed {
                error: DomainError {
                    code: failure
                        .and_then(|failure| ErrorCode::parse(&failure.code))
                        .unwrap_or(ErrorCode::UpstreamError),
                    message: failure.map_or_else(
                        || "the current message failed".to_owned(),
                        |failure| failure.message.clone(),
                    ),
                    detail: None,
                    retryable: false,
                },
            }
        }
    })
}

/// The transport deduplication identity of one decision.
///
/// Derived from the agent, revision, tail and optional source wake. The source identity is
/// load-bearing: a retirement-only transaction follows the final journal decision without
/// advancing its tail, and reusing that earlier token for a different transaction would be
/// rejected as an idempotent-parameter mismatch.
#[must_use]
pub fn client_request_token(commit: &DecisionCommit) -> String {
    let material = format!(
        "{}:{}:{}:{}",
        commit.guard.key.agent.0.as_hyphenated(),
        commit.control.next_revision.0,
        commit.control.next_tail.get(),
        commit
            .retired_wake
            .as_ref()
            .map_or("-", |wake| wake.work_id.as_str())
    );
    let encoded = blake3::hash(material.as_bytes()).to_hex().to_string();
    format!("brain-{}", &encoded[..30])
}

fn budget_update(
    table: &str,
    item: &shared::Key,
    deltas: &[BudgetDelta],
    now: aex_wire::types::Timestamp,
) -> aws_sdk_dynamodb::types::builders::UpdateBuilder {
    let mut adds = Vec::new();
    let mut builder = aws_sdk_dynamodb::types::Update::builder()
        .table_name(table)
        .set_key(Some(key(&item.pk, &item.sk)));
    for (index, delta) in deltas.iter().enumerate() {
        let placeholder = format!(":d{index}");
        adds.push(format!("{} {placeholder}", used_attribute(delta.dimension)));
        builder = builder.expression_attribute_values(placeholder, n(delta.quantity));
    }
    builder
        .update_expression(format!("SET updatedAt = :now ADD {}", adds.join(", ")))
        .expression_attribute_values(":now", stamp(now))
}

fn wake_record(context: &DecisionContext, wake: &WakeCreate) -> Result<WorkRecord, PlanError> {
    let (session, agent) = translate::agent_key(&wake.key).map_err(BrainKeyError::from)?;
    let due = wake.due.unwrap_or(context.now);
    Ok(WorkRecord {
        work_id: format!("wrk_{}", wake.id.0.as_simple()),
        workspace: context.authority.workspace,
        organization: context.authority.organization,
        session: Some(session),
        agent: Some(agent),
        kind: "agent.wake".to_owned(),
        priority: wake.priority,
        due_at: translate::at(due, "due").map_err(BrainKeyError::from)?,
        state: "pending".to_owned(),
        attempt: 0,
        max_attempts: 8,
        fence: 0,
        claim_owner: None,
        lease_expires_at: None,
        dedupe_key: hex_digest(&wake.dedup_key),
        payload: Payload::new()
            .set("sessionId", session.to_string())
            .set("agentId", agent.to_string())
            .set("fromSeq", "0")
            .set("cancelEpoch", "0"),
        delivery: DeliveryEvidence::default(),
        created_at: translate::at(context.now, "now").map_err(BrainKeyError::from)?,
        updated_at: translate::at(context.now, "now").map_err(BrainKeyError::from)?,
    })
}

/// The hashed dedupe key `regional-work` stores.
///
/// The durable "one wake outstanding" claim is keyed by this digest rather than by the
/// caller's string, so a dedupe key can carry an arbitrary reason without ever entering a
/// key template.
fn hex_digest(value: &str) -> String {
    use core::fmt::Write as _;
    let digest = blake3::hash(value.as_bytes());
    let mut rendered = String::with_capacity(64);
    for byte in digest.as_bytes() {
        let _ = write!(rendered, "{byte:02x}");
    }
    rendered
}

fn grant_attribute(
    grant: aex_brain_domain::budget::DimensionVector,
) -> aws_sdk_dynamodb::types::AttributeValue {
    aws_sdk_dynamodb::types::AttributeValue::M(
        aex_brain_domain::budget::DIMENSIONS
            .iter()
            .map(|dimension| {
                (
                    dimension_stem(*dimension).to_owned(),
                    n(grant.get(*dimension)),
                )
            })
            .collect(),
    )
}

/// The attribute holding what a node has consumed in `dimension`.
///
/// Three top-level attributes per dimension rather than one nested document: a condition
/// expression can fence a top-level attribute and `ADD` can increment one, while a nested
/// map forces a whole-document rewrite for every consumption. They are literals rather than
/// a formatted stem so an attribute name is a compile-time constant everywhere it is used.
#[must_use]
pub const fn used_attribute(dimension: Dimension) -> &'static str {
    match dimension {
        Dimension::TotalChildrenCreated => "usedTotalChildrenCreated",
        Dimension::ProviderCalls => "usedProviderCalls",
        Dimension::HandsCalls => "usedHandsCalls",
        Dimension::CostMicroUsd => "usedCostMicroUsd",
        Dimension::ActiveChildren => "usedActiveChildren",
        Dimension::QueuedChildren => "usedQueuedChildren",
        Dimension::RetainedResultBytes => "usedRetainedResultBytes",
    }
}

/// The attribute holding what a node may ever use in `dimension`.
#[must_use]
pub const fn limit_attribute(dimension: Dimension) -> &'static str {
    match dimension {
        Dimension::TotalChildrenCreated => "limitTotalChildrenCreated",
        Dimension::ProviderCalls => "limitProviderCalls",
        Dimension::HandsCalls => "limitHandsCalls",
        Dimension::CostMicroUsd => "limitCostMicroUsd",
        Dimension::ActiveChildren => "limitActiveChildren",
        Dimension::QueuedChildren => "limitQueuedChildren",
        Dimension::RetainedResultBytes => "limitRetainedResultBytes",
    }
}

/// The attribute holding what a node has promised to children in `dimension`.
#[must_use]
pub const fn reserved_attribute(dimension: Dimension) -> &'static str {
    match dimension {
        Dimension::TotalChildrenCreated => "reservedTotalChildrenCreated",
        Dimension::ProviderCalls => "reservedProviderCalls",
        Dimension::HandsCalls => "reservedHandsCalls",
        Dimension::CostMicroUsd => "reservedCostMicroUsd",
        Dimension::ActiveChildren => "reservedActiveChildren",
        Dimension::QueuedChildren => "reservedQueuedChildren",
        Dimension::RetainedResultBytes => "reservedRetainedResultBytes",
    }
}

/// The short name of one dimension, used inside a stored grant document.
#[must_use]
pub const fn dimension_stem(dimension: Dimension) -> &'static str {
    match dimension {
        Dimension::TotalChildrenCreated => "TotalChildrenCreated",
        Dimension::ProviderCalls => "ProviderCalls",
        Dimension::HandsCalls => "HandsCalls",
        Dimension::CostMicroUsd => "CostMicroUsd",
        Dimension::ActiveChildren => "ActiveChildren",
        Dimension::QueuedChildren => "QueuedChildren",
        Dimension::RetainedResultBytes => "RetainedResultBytes",
    }
}

const fn child_state_name(state: ChildState) -> &'static str {
    match state {
        ChildState::Queued { .. } => "queued",
        ChildState::Starting => "starting",
        ChildState::Running => "running",
        ChildState::Stopping => "stopping",
        ChildState::Completed => "completed",
        ChildState::Failed => "failed",
        ChildState::Cancelled => "cancelled",
    }
}

const fn child_outcome_name(outcome: ChildOutcome) -> &'static str {
    match outcome {
        ChildOutcome::Completed => "completed",
        ChildOutcome::Failed => "failed",
        ChildOutcome::Cancelled { .. } => "cancelled",
    }
}

const fn queued_reason_name(reason: aex_brain_domain::child::QueuedReason) -> &'static str {
    use aex_brain_domain::child::QueuedReason as Reason;
    match reason {
        Reason::ActiveBudget => "active_budget",
        Reason::SessionActiveLimit => "session_active_limit",
        Reason::ProviderPermits => "provider_permits",
        Reason::HandsPermits => "hands_permits",
        Reason::TenantFairness => "tenant_fairness",
        Reason::RegionalCapacity => "regional_capacity",
        Reason::DepthDeferred => "depth_deferred",
    }
}

const fn finish_name(reason: aex_brain_domain::journal::FinishReason) -> &'static str {
    use aex_brain_domain::journal::FinishReason as Finish;
    match reason {
        Finish::Completed => "completed",
        Finish::MaxTurns => "max_turns",
        Finish::MaxSteps => "max_steps",
        Finish::Budget => "budget",
        Finish::Timeout => "timeout",
        Finish::Cancelled => "cancelled",
        Finish::Failed => "failed",
        Finish::Interrupted => "interrupted",
    }
}

const fn settled_name(outcome: &aex_brain_domain::effect::SettledOutcome) -> &'static str {
    use aex_brain_domain::effect::SettledOutcome as Outcome;
    match outcome {
        // A proved outcome, good or bad, is `settled`; only an unproved one is `unknown`.
        // The specific outcome lives in the journal record this settlement is atomic with.
        Outcome::Complete { .. } | Outcome::KnownFailure { .. } => "settled",
        Outcome::OutcomeUnknown { .. } => "unknown",
    }
}

/// The participant order the single-append form of a decision produces.
///
/// Published so the shared compiler's [`DECISION_ORDER`] and this one can be asserted
/// equal rather than compared by eye.
#[must_use]
pub fn decision_order() -> &'static [Participant] {
    DECISION_ORDER
}
