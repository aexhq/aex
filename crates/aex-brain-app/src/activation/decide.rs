//! Turning one owed step into one [`DecisionCommit`].
//!
//! Everything here is pure. The asynchronous half calls a port, gets an outcome, and hands
//! it to a function in this module; nothing in this file can reach a store, a clock or a
//! provider, which is what makes every decision assertable without one.
//!
//! Two rules are carried by the shapes rather than by review.
//!
//! - **A settlement and the record it produced are one commit.** [`Draft::settle_complete`]
//!   appends `EffectSettled` and the record together, so there is no window where an effect
//!   is settled and the journal does not say what it produced.
//! - **A wake is created only here.** `WakeCreate` rides inside the decision, so the queue
//!   never becomes a second authority.

use aex_brain_domain::budget::{BudgetDelta, Dimension};
use aex_brain_domain::commit::{
    ChildWrite, ControlUpdate, DecisionCommit, EffectWrite, FenceGuardRef, JoinWrite,
    PublicMessageAppend, PublicMessagePart, PublicMessageRole, PublicSessionEvent, RunTransition,
    SessionHeadTransition, WakeCreate, WakeRetirement, event_seq,
};
use aex_brain_domain::effect::{
    DispatchEvidence, DispatchProof, DispatchStage, EffectClass, EffectKind, SettledOutcome,
};
use aex_brain_domain::fold::{FoldState, Phase};
use aex_brain_domain::ids::{
    AgentKey, ContentHash, EffectId, JournalSeq, Timestamp, ToolCallId, WakeId, WorkShard,
};
use aex_brain_domain::journal::{
    ExecutorRoute, FinishReason, JournalRecord, ParkReason, TypedFailure,
};
use aex_brain_domain::wire_pending::{CanonicalBlock, ToolResultPart};
use aex_internal_contracts::RunId;
use aex_wire::ids::{
    ContentHash as WireContentHash, MessageId, OperationId, PrefixedId as _,
    ToolCallId as PublicToolCallId, Uuid7,
};

use crate::ports::{FenceGuard, ProviderDispatchError, ProviderOutcome, ToolDispatchError};

/// The stable phase tag one control update writes.
///
/// A function rather than a `Display` impl on `Phase`: the tag is a durable value another
/// process reads back, so it belongs where its stability is documented rather than in a
/// formatting trait somebody could reasonably change.
#[must_use]
pub const fn phase_tag(phase: &Phase) -> &'static str {
    match phase {
        Phase::AwaitingInput => "awaiting_input",
        Phase::AwaitingModel => "awaiting_model",
        Phase::AwaitingTools => "awaiting_tools",
        Phase::Effecting { .. } => "effecting",
        Phase::Parked { .. } => "parked",
        Phase::AwaitingFinish => "awaiting_finish",
        Phase::Finished => "finished",
    }
}

/// One decision under construction.
///
/// The guard fixes the whole precondition set, so a draft cannot be committed against an
/// agent the holder does not own — there is no way to name the agent except through it.
#[derive(Debug, Clone)]
pub struct Draft {
    guard: FenceGuardRef,
    now: Timestamp,
    next_seq: JournalSeq,
    appends: Vec<JournalRecord>,
    effects: Vec<EffectWrite>,
    budget: Vec<BudgetDelta>,
    session_budget: Vec<BudgetDelta>,
    children: Vec<ChildWrite>,
    joins: Vec<JoinWrite>,
    wakes: Vec<WakeCreate>,
    retired_wake: Option<WakeRetirement>,
    messages: Vec<PublicMessageAppend>,
    session_events: Vec<PublicSessionEvent>,
    run: Option<RunTransition>,
    session: Option<SessionHeadTransition>,
    phase: &'static str,
    finish: Option<FinishReason>,
}

impl Draft {
    /// Opens a draft under `guard` at `now`.
    #[must_use]
    pub fn new(guard: &FenceGuard, now: Timestamp, phase: &'static str) -> Self {
        let reference = guard.as_ref();
        Self {
            next_seq: reference.tail.map_or(JournalSeq::ZERO, JournalSeq::next),
            guard: reference,
            now,
            appends: Vec::new(),
            effects: Vec::new(),
            budget: Vec::new(),
            session_budget: Vec::new(),
            children: Vec::new(),
            joins: Vec::new(),
            wakes: Vec::new(),
            retired_wake: None,
            messages: Vec::new(),
            session_events: Vec::new(),
            run: None,
            session: None,
            phase,
            finish: None,
        }
    }

    /// The sequence the next append will carry.
    #[must_use]
    pub const fn next_seq(&self) -> JournalSeq {
        self.next_seq
    }

    /// When this decision is being taken.
    #[must_use]
    pub const fn now(&self) -> Timestamp {
        self.now
    }

    /// Appends one record.
    pub fn append(&mut self, record: JournalRecord) {
        self.appends.push(record);
        self.next_seq = self.next_seq.next();
    }

    /// Adds one effect write.
    pub fn effect(&mut self, write: EffectWrite) {
        self.effects.push(write);
    }

    /// Charges `quantity` of `dimension` to this agent's node.
    pub fn charge(&mut self, dimension: Dimension, quantity: u64) {
        if quantity > 0 {
            self.budget.push(BudgetDelta::new(dimension, quantity));
        }
    }

    /// Charges one shared session budget dimension in the same authority change.
    pub fn charge_session(&mut self, dimension: Dimension, quantity: u64) {
        if quantity > 0 {
            self.session_budget
                .push(BudgetDelta::new(dimension, quantity));
        }
    }

    /// Adds one child authority mutation.
    pub fn child(&mut self, write: ChildWrite) {
        self.children.push(write);
    }

    /// Adds one join-ledger mutation.
    pub fn join(&mut self, write: JoinWrite) {
        self.joins.push(write);
    }

    /// Sets the phase the control update writes.
    pub const fn phase(&mut self, phase: &'static str) {
        self.phase = phase;
    }

    /// Terminalizes the agent.
    pub const fn finish(&mut self, reason: FinishReason) {
        self.finish = Some(reason);
        self.phase = "finished";
    }

    /// Creates the wake that brings the agent back.
    ///
    /// This is the only place a wake comes from. A queue that could originate one would be a
    /// second authority for what runs next, and two authorities is how work is invented.
    pub fn wake(
        &mut self,
        id: WakeId,
        reason: ParkReason,
        due: Timestamp,
        tenant: String,
        shard: WorkShard,
    ) {
        self.wake_agent(
            self.guard.key,
            id,
            reason,
            due,
            tenant,
            shard,
            continuation_key(self.guard.key, self.next_seq),
        );
    }

    /// Creates a wake for a child or parent agent inside this same decision.
    #[allow(clippy::too_many_arguments)]
    pub fn wake_agent(
        &mut self,
        key: AgentKey,
        id: WakeId,
        reason: ParkReason,
        due: Timestamp,
        tenant: String,
        shard: WorkShard,
        dedup_key: String,
    ) {
        self.wakes.push(WakeCreate {
            id,
            key,
            dedup_key,
            reason,
            due: Some(due),
            priority: 1,
            tenant,
            shard,
        });
    }

    /// Retires the durable wake that admitted this activation.
    ///
    /// The store binds this identity back to the decision's workspace, session and agent
    /// and removes the sparse due-index keys in the same fenced transaction.
    pub fn retire_wake(&mut self, work_id: String) {
        self.retired_wake = Some(WakeRetirement { work_id });
    }

    /// Whether this draft would change anything.
    ///
    /// A decision that carries no action is a defect rather than an optimization, and
    /// [`DecisionCommit::validate`] refuses one; asking first lets the caller report "nothing
    /// owed" instead of a validation error.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.appends.is_empty()
            && self.effects.is_empty()
            && self.budget.is_empty()
            && self.session_budget.is_empty()
            && self.children.is_empty()
            && self.joins.is_empty()
            && self.wakes.is_empty()
            && self.retired_wake.is_none()
            && self.messages.is_empty()
            && self.session_events.is_empty()
            && self.run.is_none()
            && self.session.is_none()
    }

    /// The records this draft appends, in order.
    #[must_use]
    pub fn records(&self) -> &[JournalRecord] {
        &self.appends
    }

    /// Seals the draft into the transaction the store executes.
    #[must_use]
    pub fn into_commit(self) -> DecisionCommit {
        let next_tail = JournalSeq(self.next_seq.get().saturating_sub(1));
        let retirement_only = self.appends.is_empty()
            && self.effects.is_empty()
            && self.budget.is_empty()
            && self.session_budget.is_empty()
            && self.children.is_empty()
            && self.joins.is_empty()
            && self.wakes.is_empty()
            && self.retired_wake.is_some()
            && self.finish.is_none();
        DecisionCommit {
            control: ControlUpdate {
                next_revision: if retirement_only {
                    self.guard.revision
                } else {
                    self.guard.revision.next()
                },
                next_tail,
                phase: self.phase.to_owned(),
                finish: self.finish,
            },
            guard: self.guard,
            appends: self.appends,
            effects: self.effects,
            budget: self.budget,
            session_budget: self.session_budget,
            children: self.children,
            joins: self.joins,
            wakes: self.wakes,
            retired_wake: self.retired_wake,
            events: Vec::new(),
            messages: self.messages,
            session_events: self.session_events,
            run: self.run,
            session: self.session,
            idempotency: None,
        }
    }
}

/// The dedup key a continuation wake carries.
///
/// Derived from the agent and the sequence the decision wrote, so two deliveries of one
/// continuation collapse before either reaches admission and a redelivered decision produces
/// the identical key rather than a second wake.
#[must_use]
pub fn continuation_key(key: AgentKey, seq: JournalSeq) -> String {
    format!(
        "{}:{}:{}",
        key.session.0.as_hyphenated(),
        key.agent.0.as_hyphenated(),
        seq.get()
    )
}

/// Whether the model-call effect may reserve a provider call against this node.
///
/// A zero limit means the dimension is unset, exactly as the planner reads it. Charging
/// against an unset dimension would exhaust a budget nobody configured on the first call.
#[must_use]
pub fn provider_call_reservation(state: &FoldState) -> Vec<BudgetDelta> {
    if state.budget.limit.get(Dimension::ProviderCalls) > 0 {
        vec![BudgetDelta::new(Dimension::ProviderCalls, 1)]
    } else {
        Vec::new()
    }
}

/// Appends the intent to perform one external effect.
///
/// Committed **before** any byte leaves. Without it a crash before the socket write is
/// indistinguishable from a crash after it, and every prepared effect becomes ambiguous.
#[allow(
    clippy::too_many_arguments,
    reason = "these are exactly the fields of `EffectPrepared`; a parameter struct would have one construction site and one reader, and would put the record's shape in two places"
)]
pub fn prepare(
    draft: &mut Draft,
    effect: EffectId,
    tool_call: Option<ToolCallId>,
    kind: EffectKind,
    class: EffectClass,
    request_hash: ContentHash,
    attempt: u16,
    deadline: Timestamp,
    reservation: Vec<BudgetDelta>,
) {
    for delta in &reservation {
        draft.charge(delta.dimension, delta.quantity);
    }
    draft.append(JournalRecord::EffectPrepared {
        effect,
        tool_call,
        kind,
        class,
        request_hash,
        deadline,
        attempt,
        reservation,
    });
    draft.effect(EffectWrite::Prepare {
        id: effect,
        kind,
        generation: None,
        class,
        request_hash,
        deadline,
        attempt,
    });
    draft.phase("effecting");
}

/// Settles a model call that produced a whole message, atomically with the message itself.
///
pub fn settle_model_call(draft: &mut Draft, effect: EffectId, outcome: &ProviderOutcome) {
    // `CompleteProof` is the wire authority's SHA-256 type; Brain's durable
    // `ContentHash` is explicitly blake3. Hash the proof bytes rather than
    // relabelling one algorithm's digest as the other.
    let receipt = ContentHash::of(outcome.message.proof.0.as_bytes());
    draft.append(JournalRecord::EffectSettled {
        effect,
        outcome: SettledOutcome::Complete { receipt },
        charged: Vec::new(),
    });
    draft.effect(EffectWrite::Settle {
        id: effect,
        outcome: SettledOutcome::Complete { receipt },
    });
    draft.append(JournalRecord::AssistantMessage {
        public_message: None,
        message: outcome.message.clone(),
        usage: outcome.usage,
        receipt: Box::new(outcome.receipt.clone()),
        effect,
    });
    draft.phase(if outcome.message.blocks.iter().any(is_tool_use) {
        "awaiting_tools"
    } else {
        "awaiting_finish"
    });
}

/// Settles a root model call and writes its complete born-sealed public message.
pub fn settle_root_model_call(
    draft: &mut Draft,
    run: RunId,
    effect: EffectId,
    outcome: &ProviderOutcome,
) {
    let receipt = ContentHash::of(outcome.message.proof.0.as_bytes());
    let public_message = public_message_id(run, effect, 0x01);
    draft.append(JournalRecord::EffectSettled {
        effect,
        outcome: SettledOutcome::Complete { receipt },
        charged: Vec::new(),
    });
    draft.effect(EffectWrite::Settle {
        id: effect,
        outcome: SettledOutcome::Complete { receipt },
    });
    draft.append(JournalRecord::AssistantMessage {
        public_message: Some(public_message),
        message: outcome.message.clone(),
        usage: outcome.usage,
        receipt: Box::new(outcome.receipt.clone()),
        effect,
    });
    draft.messages.push(PublicMessageAppend {
        id: public_message,
        run,
        role: PublicMessageRole::Assistant,
        parts: public_assistant_parts(run, &outcome.message.blocks),
        at: draft.now,
    });
    draft.phase(if outcome.message.blocks.iter().any(is_tool_use) {
        "awaiting_tools"
    } else {
        "awaiting_finish"
    });
}

fn is_tool_use(block: &CanonicalBlock) -> bool {
    matches!(block, CanonicalBlock::ToolUse { .. })
}

/// Settles a tool call that produced a result, atomically with the result itself.
#[allow(
    clippy::too_many_arguments,
    reason = "these are exactly the fields of `ToolResult` plus the receipt the settlement is atomic with; bundling them would hide which of the two records each value belongs to"
)]
pub fn settle_tool_call(
    draft: &mut Draft,
    effect: EffectId,
    call: aex_brain_domain::ids::ToolCallId,
    content: Vec<ToolResultPart>,
    is_error: bool,
    executed_on: ExecutorRoute,
    duration_ms: u32,
    checksum: ContentHash,
) {
    draft.append(JournalRecord::EffectSettled {
        effect,
        outcome: SettledOutcome::Complete { receipt: checksum },
        charged: Vec::new(),
    });
    draft.effect(EffectWrite::Settle {
        id: effect,
        outcome: SettledOutcome::Complete { receipt: checksum },
    });
    draft.append(JournalRecord::ToolResult {
        public_message: None,
        call,
        content,
        is_error,
        executed_on,
        duration_ms,
        effect,
    });
}

/// Settles one root Bash result and writes its complete born-sealed public message.
#[allow(
    clippy::too_many_arguments,
    reason = "the arguments are the closed ToolResult journal shape plus its root-run binding"
)]
pub fn settle_root_tool_call(
    draft: &mut Draft,
    run: RunId,
    effect: EffectId,
    call: aex_brain_domain::ids::ToolCallId,
    content: Vec<ToolResultPart>,
    is_error: bool,
    executed_on: ExecutorRoute,
    duration_ms: u32,
    checksum: ContentHash,
) {
    draft.append(JournalRecord::EffectSettled {
        effect,
        outcome: SettledOutcome::Complete { receipt: checksum },
        charged: Vec::new(),
    });
    draft.effect(EffectWrite::Settle {
        id: effect,
        outcome: SettledOutcome::Complete { receipt: checksum },
    });
    append_root_tool_result(
        draft,
        run,
        effect,
        call,
        content,
        is_error,
        executed_on,
        duration_ms,
    );
}

/// Appends a root Bash result after the caller has written its effect settlement.
///
/// # Panics
///
/// Panics if the closed [`ToolResultPart`] vocabulary cannot be serialized to an
/// in-memory JSON buffer.
#[allow(
    clippy::too_many_arguments,
    reason = "the arguments are the closed ToolResult journal shape plus its root-run binding"
)]
pub fn append_root_tool_result(
    draft: &mut Draft,
    run: RunId,
    effect: EffectId,
    call: aex_brain_domain::ids::ToolCallId,
    content: Vec<ToolResultPart>,
    is_error: bool,
    executed_on: ExecutorRoute,
    duration_ms: u32,
) {
    let public_message = public_message_id(run, effect, 0x02);
    let public_call = public_tool_call_id(run, &call);
    let result_bytes = serde_json::to_vec(&content)
        .expect("the closed canonical tool-result vocabulary always serializes");
    let mut parts = content
        .iter()
        .map(|part| match part {
            ToolResultPart::Text { text } => PublicMessagePart::Text {
                text: text.as_str().to_owned(),
            },
            ToolResultPart::Json { value } => PublicMessagePart::Text {
                text: value.as_str().to_owned(),
            },
        })
        .collect::<Vec<_>>();
    parts.push(PublicMessagePart::ToolResult {
        id: public_call,
        result: WireContentHash::of(&result_bytes),
    });
    draft.append(JournalRecord::ToolResult {
        public_message: Some(public_message),
        call,
        content,
        is_error,
        executed_on,
        duration_ms,
        effect,
    });
    draft.messages.push(PublicMessageAppend {
        id: public_message,
        run,
        role: PublicMessageRole::Tool,
        parts,
        at: draft.now,
    });
}

/// Closes one root run without terminalizing the retained root agent.
#[allow(
    clippy::too_many_arguments,
    reason = "the boundary deliberately carries every run/session fence and outcome fact"
)]
pub fn finish_run(
    draft: &mut Draft,
    run: RunId,
    message: MessageId,
    session_revision: u64,
    reason: FinishReason,
    failure: Option<TypedFailure>,
    output_messages: Vec<MessageId>,
    cancellation: Option<OperationId>,
    ambiguous_effect: Option<EffectId>,
) {
    let boundary_seq = draft.next_seq();
    draft.append(JournalRecord::RunFinished {
        run,
        reason,
        failure: failure.clone(),
        output_messages: output_messages.clone(),
        ambiguous_effect,
    });
    draft.run = Some(RunTransition {
        run,
        finish: reason,
        failure,
        output_messages,
        cancellation,
        ambiguous_effect,
    });
    draft.session = Some(SessionHeadTransition {
        status: "idle".to_owned(),
        revision: session_revision,
    });
    draft.session_events.push(PublicSessionEvent {
        event_seq: event_seq(boundary_seq, 0),
        message,
        outcome: finish_outcome(reason).to_owned(),
        at: draft.now,
    });
    draft.phase("awaiting_input");
}

const fn finish_outcome(reason: FinishReason) -> &'static str {
    match reason {
        FinishReason::Completed => "succeeded",
        FinishReason::Timeout => "timed_out",
        FinishReason::Cancelled => "cancelled",
        FinishReason::Interrupted | FinishReason::Budget | FinishReason::AccountPaused => {
            "interrupted"
        }
        FinishReason::Failed => "failed",
    }
}

pub(super) fn public_message_id(run: RunId, effect: EffectId, tag: u8) -> MessageId {
    MessageId::from_uuid7(derived_uuid(run, &[tag], &effect.0))
}

fn public_tool_call_id(run: RunId, call: &aex_brain_domain::ids::ToolCallId) -> PublicToolCallId {
    PublicToolCallId::from_uuid7(derived_uuid(run, &[0x03], call.as_str().as_bytes()))
}

fn derived_uuid(run: RunId, tag: &[u8], material: &[u8]) -> Uuid7 {
    let mut bytes = Vec::with_capacity(tag.len().saturating_add(material.len()));
    bytes.extend_from_slice(tag);
    bytes.extend_from_slice(material);
    let digest = ContentHash::of(&bytes);
    let mut entropy = [0_u8; 10];
    entropy.copy_from_slice(&digest.0[..10]);
    Uuid7::compose(run.uuid7().unix_millis(), entropy)
}

fn public_assistant_parts(run: RunId, blocks: &[CanonicalBlock]) -> Vec<PublicMessagePart> {
    blocks
        .iter()
        .filter_map(|block| match block {
            CanonicalBlock::Text { text, .. } => Some(PublicMessagePart::Text {
                text: text.as_str().to_owned(),
            }),
            CanonicalBlock::Refusal { text } => Some(PublicMessagePart::Text {
                text: text.as_str().to_owned(),
            }),
            CanonicalBlock::ToolUse { id, input, .. } => Some(PublicMessagePart::ToolCall {
                id: public_tool_call_id(run, id),
                arguments: WireContentHash::of(input.as_bytes()),
            }),
            // Private chain-of-thought is never projected into the public message surface.
            CanonicalBlock::Reasoning(_) | CanonicalBlock::ToolResult { .. } => None,
        })
        .collect()
}

/// Settles an effect nothing can be proved about, and terminalizes the run.
///
/// `Interrupted` exists only as the projection of an `OutcomeUnknown` effect. Reporting a
/// clean cancellation for a request that may have been served would be a lie the customer
/// pays for twice: once to the provider and once in a rerun.
pub fn settle_unknown(draft: &mut Draft, effect: EffectId, evidence: DispatchEvidence) {
    let outcome = SettledOutcome::OutcomeUnknown { evidence };
    draft.append(JournalRecord::EffectSettled {
        effect,
        outcome: outcome.clone(),
        charged: Vec::new(),
    });
    draft.effect(EffectWrite::Settle {
        id: effect,
        outcome,
    });
    // The fold terminalizes on `OutcomeUnknown` by itself; the control update has to agree
    // with it, or the row and the journal would disagree about whether the agent is done.
    draft.finish(FinishReason::Interrupted);
}

/// Settles an outcome-unknown root effect without absorbing the retained root agent.
/// The caller appends [`JournalRecord::RunFinished`] in the same draft.
pub fn settle_root_unknown(draft: &mut Draft, effect: EffectId, evidence: DispatchEvidence) {
    let outcome = SettledOutcome::OutcomeUnknown { evidence };
    draft.append(JournalRecord::EffectSettled {
        effect,
        outcome: outcome.clone(),
        charged: Vec::new(),
    });
    draft.effect(EffectWrite::Settle {
        id: effect,
        outcome,
    });
    draft.phase("awaiting_finish");
}

/// Settles an ambiguous tool-batch member without terminalizing its siblings.
///
/// The matching error result is appended by the caller in the same decision.
/// The fold recognizes the durable call-to-effect link and keeps the batch
/// pending until every member is terminal.
pub fn settle_tool_unknown(draft: &mut Draft, effect: EffectId, evidence: DispatchEvidence) {
    let outcome = SettledOutcome::OutcomeUnknown { evidence };
    draft.append(JournalRecord::EffectSettled {
        effect,
        outcome: outcome.clone(),
        charged: Vec::new(),
    });
    draft.effect(EffectWrite::Settle {
        id: effect,
        outcome,
    });
    draft.phase("awaiting_tools");
}

/// Settles an effect the upstream definitively refused.
pub fn settle_known_failure(
    draft: &mut Draft,
    effect: EffectId,
    stage: DispatchStage,
    proof: DispatchProof,
) {
    draft.append(JournalRecord::EffectSettled {
        effect,
        outcome: SettledOutcome::KnownFailure { stage, proof },
        charged: Vec::new(),
    });
    draft.effect(EffectWrite::Settle {
        id: effect,
        outcome: SettledOutcome::KnownFailure { stage, proof },
    });
}

/// Appends the absorbing terminal.
pub fn finish(draft: &mut Draft, reason: FinishReason, failure: Option<TypedFailure>) {
    draft.append(JournalRecord::AgentFinished { reason, failure });
    draft.finish(reason);
}

/// What a failed provider dispatch settles as.
///
/// [`DispatchProof::NotSent`] is the only value that permits a further attempt,
/// because it is the only one that says the upstream cannot have seen the
/// request. An explicitly observed [`DispatchStage::Terminal`] refusal is a
/// known failure but never retryable. Every other sent failure is
/// `OutcomeUnknown`: an over-generous reading there is exactly how a
/// possibly-served request becomes a second generation the customer is billed
/// for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailureSettlement {
    /// Nothing left the process. The effect closes and the agent may plan again.
    NotSent {
        /// How far the attempt got.
        stage: DispatchStage,
        /// Whether the planner may open a new effect, or must terminalize.
        retryable: bool,
    },
    /// The upstream returned a definitive terminal refusal. It must not be
    /// retried, but it is also not an ambiguous outcome.
    KnownFailure {
        /// How far the attempt got.
        stage: DispatchStage,
        /// What the adapter proved about the observed response.
        proof: DispatchProof,
    },
    /// The upstream may have served it. The effect settles unknown and the run interrupts.
    Unknown(Box<DispatchEvidence>),
}

/// Classifies a provider dispatch failure.
#[must_use]
pub fn classify_provider_failure(error: &ProviderDispatchError, attempt: u16) -> FailureSettlement {
    use crate::ports::ProviderFailureClass;
    if error.proof == DispatchProof::NotSent {
        return FailureSettlement::NotSent {
            stage: error.stage,
            retryable: matches!(
                error.kind.class(),
                ProviderFailureClass::Transient | ProviderFailureClass::Overloaded
            ),
        };
    }
    if error.stage == DispatchStage::Terminal {
        return FailureSettlement::KnownFailure {
            stage: error.stage,
            proof: error.proof,
        };
    }
    FailureSettlement::Unknown(Box::new(DispatchEvidence {
        stage: error.stage,
        proof: error.proof,
        attempt,
        provider_request_id: error.provider_request_id.clone(),
        external_operation: None,
        detached_tool: None,
        receipt: None,
        detail: Some(error.detail.as_str().to_owned()),
    }))
}

/// Classifies a tool dispatch failure by the same rule.
#[must_use]
pub fn classify_tool_failure(error: &ToolDispatchError, attempt: u16) -> FailureSettlement {
    if error.proof == DispatchProof::NotSent {
        return FailureSettlement::NotSent {
            stage: error.stage,
            retryable: error.retryable,
        };
    }
    if error.stage == DispatchStage::Terminal {
        return FailureSettlement::KnownFailure {
            stage: error.stage,
            proof: error.proof,
        };
    }
    FailureSettlement::Unknown(Box::new(DispatchEvidence {
        stage: error.stage,
        proof: error.proof,
        attempt,
        provider_request_id: None,
        external_operation: None,
        detached_tool: None,
        receipt: None,
        detail: Some(error.detail.as_str().to_owned()),
    }))
}

/// The typed failure a definitively refused dispatch records.
#[must_use]
pub fn refusal(code: &str, message: &str) -> TypedFailure {
    TypedFailure {
        code: code.to_owned(),
        message: message.to_owned(),
        detail: None,
    }
}
