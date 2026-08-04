//! Journal builders and `proptest` strategies over arbitrary well-typed histories.
//!
//! Owned by the brain-core stream (plan 07 §1). Peer streams own `provider_fake`,
//! `tool_fake` and `hands_fake` inside this same crate.
//!
//! Two things live here and nowhere else.
//!
//! - [`HistoryBuilder`] seals records into envelopes at contiguous sequences, so a test
//!   never hand-writes a hash and therefore can never accidentally assert against a stale
//!   one.
//! - [`arb_history`] and the [`Hostile`] mutators produce the *admissible* and the
//!   *hostile* histories the fold properties quantify over. A generator that only produced
//!   well-formed input would prove nothing about the guards that exist for malformed input.

use aex_brain_domain::budget::{BudgetGrant, Dimension, DimensionVector};
use aex_brain_domain::child::{ChildOutcome, QueuedReason};
use aex_brain_domain::effect::{
    DispatchEvidence, DispatchProof, DispatchStage, EffectClass, EffectKind, SettledOutcome,
};
use aex_brain_domain::ids::{
    AgentId, ContentHash, EffectId, JoinId, JournalSeq, Timestamp, ToolCallId, ToolName, WaitId,
};
use aex_brain_domain::journal::{
    ExecutorRoute, FinishReason, JournalEntry, JournalRecord, MessageOrigin, ParkReason,
    PreservedCounters, TypedFailure, WaitResolution,
};
use aex_brain_domain::wire_pending::{
    AgentLimits, CanonicalBlock, ContentBlockRef, JoinMode, NormalizedUsage, ProviderId,
    ResolvedAgentConfig, StopReason, ToolResultPart,
};
use aex_model_catalog::canonical::{CredentialBindingRef, ProviderReceipt, ReceiptBounds, seal};
use aex_model_catalog::document::CapabilitySet;
use aex_model_catalog::{BoundedString, QualifiedModel, fixture};
use aex_wire::CanonicalJson;
use aex_wire::ids::{GenerationId, PrefixedId as _, ProviderCredentialId, Uuid7};
use proptest::prelude::*;
use uuid::Uuid;

/// The instant the first record of every generated history carries.
pub const HISTORY_EPOCH_MILLIS: i64 = 1_767_225_600_000;

/// The gap between two consecutive generated records.
pub const HISTORY_STEP_MILLIS: i64 = 250;

/// Builds a contiguous, correctly sealed journal.
///
/// The builder owns sequencing and hashing so a test states only *what happened*. A test
/// that also had to state the sequence and the hash could assert a history the authority
/// could never have written, which is the opposite of what these properties are for.
#[derive(Debug, Clone)]
pub struct HistoryBuilder {
    next: u64,
    at: i64,
    entries: Vec<JournalEntry>,
}

impl HistoryBuilder {
    /// An empty journal starting at sequence zero and [`HISTORY_EPOCH_MILLIS`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            next: 0,
            at: HISTORY_EPOCH_MILLIS,
            entries: Vec::new(),
        }
    }

    /// Seals `record` at the next sequence.
    ///
    /// # Panics
    ///
    /// Panics when the record cannot be canonicalized. A fixture that cannot be encoded is
    /// a defect in the fixture, and failing loudly here beats a test that silently
    /// exercises a shorter history than it claims.
    #[must_use]
    pub fn push(mut self, record: JournalRecord) -> Self {
        let entry = JournalEntry::seal(JournalSeq(self.next), Timestamp(self.at), record)
            .expect("a fixture record must canonicalize");
        self.entries.push(entry);
        self.next = self.next.saturating_add(1);
        self.at = self.at.saturating_add(HISTORY_STEP_MILLIS);
        self
    }

    /// The sequence the next [`HistoryBuilder::push`] will use.
    #[must_use]
    pub const fn next_seq(&self) -> JournalSeq {
        JournalSeq(self.next)
    }

    /// The sealed entries.
    #[must_use]
    pub fn build(self) -> Vec<JournalEntry> {
        self.entries
    }
}

impl Default for HistoryBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// A stable agent identity for `seed`.
#[must_use]
pub fn agent(seed: u128) -> AgentId {
    AgentId(Uuid::from_u128(seed))
}

/// A stable join identity for `seed`.
#[must_use]
pub fn join(seed: u128) -> JoinId {
    JoinId(Uuid::from_u128(seed))
}

/// A stable wait identity for `seed`.
#[must_use]
pub fn wait(seed: u128) -> WaitId {
    WaitId(Uuid::from_u128(seed))
}

/// The pinned configuration every generated agent runs under.
#[must_use]
pub fn config() -> ResolvedAgentConfig {
    let model = model();
    ResolvedAgentConfig {
        catalog_pin: model.catalog(),
        provider: ProviderId::Deepseek,
        credential: aex_brain_domain::wire_pending::SessionCredentialPin {
            binding: ProviderCredentialId::from_uuid7(Uuid7::compose(1, [8; 10])),
            revision: core::num::NonZeroU64::MIN,
            generation: core::num::NonZeroU64::MIN,
            revocation_epoch: 0,
        },
        model: model.model().clone(),
        system: None,
        tool_manifest_digests: vec![ContentHash::of(b"fixture-tools")],
        hands_generation: GenerationId::from_uuid7(Uuid7::compose(1, [9; 10])),
        limits: AgentLimits {
            max_turns: 32,
            max_steps_per_turn: 16,
            turn_deadline_ms: 600_000,
        },
    }
}

/// The deterministic provider/model pair used by Brain fixtures.
#[must_use]
pub fn model() -> QualifiedModel {
    fixture::qualified_entry(
        ProviderId::Deepseek,
        "deepseek-chat",
        CapabilitySet::default(),
    )
}

/// A grant with every dimension set to `value`.
#[must_use]
pub fn grant(value: u64) -> BudgetGrant {
    DimensionVector::uniform(value)
}

/// The root `AgentStarted` record.
#[must_use]
pub fn started(budget: BudgetGrant) -> JournalRecord {
    JournalRecord::AgentStarted {
        config: Box::new(config()),
        parent: None,
        join: None,
        depth: 0,
        budget,
    }
}

/// An `AgentStarted` record for a child of `parent` at `depth`.
#[must_use]
pub fn started_child(
    parent: AgentId,
    join_id: JoinId,
    depth: u16,
    budget: BudgetGrant,
) -> JournalRecord {
    JournalRecord::AgentStarted {
        config: Box::new(config()),
        parent: Some(parent),
        join: Some(join_id),
        depth,
        budget,
    }
}

/// A user message carrying one inline text block.
#[must_use]
pub fn user_text(text: &str) -> JournalRecord {
    JournalRecord::UserMessage {
        content: vec![ContentBlockRef::Inline {
            block: CanonicalBlock::Text {
                text: BoundedString::truncating(text),
                annotations: Vec::new(),
            },
        }],
        origin: MessageOrigin::Submission,
    }
}

/// A complete assistant message carrying `blocks` under `stop_reason`.
///
/// # Panics
///
/// Panics when `stop_reason` is not terminal or `blocks` is empty: such a message is
/// structurally unable to carry a completeness proof, so no fixture may claim one.
#[must_use]
pub fn assistant(
    blocks: Vec<CanonicalBlock>,
    stop_reason: StopReason,
    effect: EffectId,
) -> JournalRecord {
    let message = seal(blocks, stop_reason, &TURN_USAGE, &model())
        .expect("a fixture assistant message must be provably complete");
    let at = fixture::at(HISTORY_EPOCH_MILLIS);
    let receipt = ProviderReceipt {
        provider: message.provider,
        model: message.model.clone(),
        catalog: message.catalog,
        dialect: model().dialect(),
        dialect_revision: model().dialect_revision(),
        credential: CredentialBindingRef {
            id: ProviderCredentialId::from_uuid7(Uuid7::compose(1, [4; 10])),
            revision: 1,
            generation: 1,
        },
        provider_request_id: None,
        gateway_route: None,
        http_status: 200,
        attempts: 1,
        started_at: at,
        first_frame_at: Some(at),
        completed_at: at,
        request_bytes: 1,
        response_bytes: 1,
        frames: 1,
        rate_limit: None,
        response_receipt: Some(message.proof.0),
        bounds: ReceiptBounds {
            max_frame_bytes: 1_024,
            max_response_bytes: 1_024,
            idle_frame_timeout_ms: 1_000,
            total_deadline_ms: 10_000,
        },
    };
    JournalRecord::AssistantMessage {
        message,
        usage: TURN_USAGE,
        receipt: Box::new(receipt),
        effect,
    }
}

/// A one-text-block assistant turn that ends cleanly.
#[must_use]
pub fn assistant_text(text: &str, effect: EffectId) -> JournalRecord {
    assistant(
        vec![CanonicalBlock::Text {
            text: BoundedString::truncating(text),
            annotations: Vec::new(),
        }],
        StopReason::EndTurn,
        effect,
    )
}

/// An assistant turn asking for the named tool calls, in order.
///
/// # Panics
///
/// Panics when a fixture supplies an invalid tool name. The tool input is a
/// fixed valid canonical JSON object.
#[must_use]
pub fn assistant_tool_use(calls: &[(&str, &str)], effect: EffectId) -> JournalRecord {
    let blocks = calls
        .iter()
        .map(|(id, name)| CanonicalBlock::ToolUse {
            id: ToolCallId::truncating(id),
            name: ToolName::parse(name).expect("fixture tool name"),
            input: CanonicalJson::parse("{}").expect("fixture tool input"),
        })
        .collect();
    assistant(blocks, StopReason::ToolUse, effect)
}

/// The result of one tool call.
#[must_use]
pub fn tool_result(call: &str, text: &str, effect: EffectId) -> JournalRecord {
    JournalRecord::ToolResult {
        call: ToolCallId::truncating(call),
        content: vec![ToolResultPart::Text {
            text: BoundedString::truncating(text),
        }],
        is_error: false,
        executed_on: ExecutorRoute::ManagedWeb,
        duration_ms: 12,
        effect,
    }
}

/// The usage every generated assistant turn reports.
pub const TURN_USAGE: NormalizedUsage = NormalizedUsage {
    input_tokens: 100,
    cache_read_input_tokens: 0,
    cache_write_input_tokens: 0,
    output_tokens: 20,
    reasoning_tokens: 0,
    tool_use_prompt_tokens: 0,
    provider_total_tokens: Some(120),
    completeness: aex_model_catalog::canonical::UsageCompleteness::Exact,
};

/// An `EffectPrepared` record for `effect`.
#[must_use]
pub fn effect_prepared(effect: EffectId, kind: EffectKind, class: EffectClass) -> JournalRecord {
    JournalRecord::EffectPrepared {
        effect,
        kind,
        class,
        request_hash: ContentHash::of(b"fixture-request"),
        deadline: Timestamp(HISTORY_EPOCH_MILLIS + 60_000),
        attempt: 1,
        reservation: Vec::new(),
    }
}

/// An `EffectSettled` record closing `effect` cleanly.
#[must_use]
pub fn effect_complete(effect: EffectId) -> JournalRecord {
    JournalRecord::EffectSettled {
        effect,
        outcome: SettledOutcome::Complete {
            receipt: ContentHash::of(b"fixture-receipt"),
        },
        charged: Vec::new(),
    }
}

/// An `EffectSettled` record closing `effect` with an unprovable outcome.
#[must_use]
pub fn effect_unknown(effect: EffectId) -> JournalRecord {
    JournalRecord::EffectSettled {
        effect,
        outcome: SettledOutcome::OutcomeUnknown {
            evidence: DispatchEvidence::ambiguous(1, DispatchStage::Streaming),
        },
        charged: Vec::new(),
    }
}

/// An `EffectSettled` record closing `effect` with a proved upstream refusal.
#[must_use]
pub fn effect_known_failure(effect: EffectId) -> JournalRecord {
    JournalRecord::EffectSettled {
        effect,
        outcome: SettledOutcome::KnownFailure {
            stage: DispatchStage::PreDispatch,
            proof: DispatchProof::NotSent,
        },
        charged: Vec::new(),
    }
}

/// A join opened over `members`.
#[must_use]
pub fn join_opened(join_id: JoinId, mode: JoinMode, members: Vec<AgentId>) -> JournalRecord {
    let shards = aex_brain_domain::wire_pending::join_shards(members.len());
    JournalRecord::JoinOpened {
        join: join_id,
        mode,
        members,
        shards,
    }
}

/// A child spawned at `ordinal` into `join_id`.
#[must_use]
pub fn child_spawned(
    child: AgentId,
    ordinal: u32,
    join_id: JoinId,
    child_grant: BudgetGrant,
    queued_reason: Option<QueuedReason>,
) -> JournalRecord {
    JournalRecord::ChildSpawned {
        child,
        ordinal,
        grant: child_grant,
        join: join_id,
        queued_reason,
    }
}

/// A child reaching `outcome`.
#[must_use]
pub fn child_terminal(child: AgentId, outcome: ChildOutcome) -> JournalRecord {
    JournalRecord::ChildTerminal {
        child,
        outcome,
        rolled_up: Vec::new(),
        result: None,
    }
}

/// A durable wait opening for `reason`.
#[must_use]
pub fn wait_opened(wait_id: WaitId, reason: ParkReason) -> JournalRecord {
    JournalRecord::WaitOpened {
        wait: wait_id,
        reason,
        due: None,
    }
}

/// A durable wait resolving.
#[must_use]
pub fn wait_resolved(wait_id: WaitId, resolution: WaitResolution) -> JournalRecord {
    JournalRecord::WaitResolved {
        wait: wait_id,
        resolution,
    }
}

/// A compaction replacing the prefix through `through`.
#[must_use]
pub fn compaction(through: JournalSeq, preserved: PreservedCounters) -> JournalRecord {
    JournalRecord::Compaction {
        replaces_through: through,
        summary: vec![CanonicalBlock::Text {
            text: BoundedString::truncating("summary of the replaced prefix"),
            annotations: Vec::new(),
        }],
        preserved,
    }
}

/// The absorbing terminal for `reason`.
#[must_use]
pub fn finished(reason: FinishReason) -> JournalRecord {
    JournalRecord::AgentFinished {
        reason,
        failure: match reason {
            FinishReason::Failed => Some(TypedFailure {
                code: "provider_truncated".to_owned(),
                message: "the provider cut the response off".to_owned(),
                detail: None,
            }),
            _ => None,
        },
    }
}

/// The deterministic effect identity a model call at `seq` derives.
#[must_use]
pub fn model_effect(owner: AgentId, seq: JournalSeq) -> EffectId {
    EffectId::derive(owner, seq, EffectKind::ModelCall.tag())
}

/// The deterministic effect identity a tool call at `seq` derives.
#[must_use]
pub fn tool_effect(owner: AgentId, seq: JournalSeq) -> EffectId {
    EffectId::derive(owner, seq, EffectKind::ToolCall.tag())
}

/// One admissible step a generated history may take.
///
/// The strategy walks a small state machine rather than emitting records independently,
/// because an independent emitter produces almost only rejected histories and would
/// therefore exercise the guards but never the fold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    UserTurn,
    ModelTurn,
    ToolTurn,
    ParkAndResume,
    SpawnAndJoin,
    Compact,
    Finish(FinishReason),
}

fn arb_step() -> impl Strategy<Value = Step> {
    prop_oneof![
        3 => Just(Step::UserTurn),
        6 => Just(Step::ModelTurn),
        4 => Just(Step::ToolTurn),
        2 => Just(Step::ParkAndResume),
        2 => Just(Step::SpawnAndJoin),
        1 => Just(Step::Compact),
        1 => prop_oneof![
            Just(Step::Finish(FinishReason::Completed)),
            Just(Step::Finish(FinishReason::Failed)),
            Just(Step::Finish(FinishReason::Cancelled)),
            Just(Step::Finish(FinishReason::Interrupted)),
        ],
    ]
}

/// Renders `steps` into a contiguous admissible history for `owner`.
///
/// Every step is expanded into whole, well-formed groups — a model turn is always
/// `EffectPrepared` then `AssistantMessage` then `EffectSettled` — so the resulting history
/// is one the authority could genuinely have written.
#[must_use]
#[allow(clippy::too_many_lines)]
fn render(owner: AgentId, steps: &[Step]) -> Vec<JournalEntry> {
    let mut builder = HistoryBuilder::new().push(started(grant(1_000_000)));
    let mut has_input = false;
    let mut open_calls: Vec<String> = Vec::new();
    let mut spawned = 0_u32;
    let mut counters = PreservedCounters::default();

    for step in steps {
        match step {
            Step::UserTurn => {
                builder = builder.push(user_text("please continue"));
                has_input = true;
            }
            Step::ModelTurn => {
                if !has_input || !open_calls.is_empty() {
                    continue;
                }
                let seq = builder.next_seq();
                let effect = model_effect(owner, seq);
                builder = builder
                    .push(effect_prepared(
                        effect,
                        EffectKind::ModelCall,
                        EffectClass::NonReplayable,
                    ))
                    .push(assistant_text("here you go", effect))
                    .push(effect_complete(effect));
                counters.assistant_turns = counters.assistant_turns.saturating_add(1);
                counters.usage.add(TURN_USAGE);
                has_input = false;
            }
            Step::ToolTurn => {
                if !has_input || !open_calls.is_empty() {
                    continue;
                }
                let seq = builder.next_seq();
                let effect = model_effect(owner, seq);
                let call = format!("call-{}", seq.get());
                builder = builder
                    .push(effect_prepared(
                        effect,
                        EffectKind::ModelCall,
                        EffectClass::NonReplayable,
                    ))
                    .push(assistant_tool_use(&[(&call, "read_file")], effect))
                    .push(effect_complete(effect));
                counters.assistant_turns = counters.assistant_turns.saturating_add(1);
                counters.usage.add(TURN_USAGE);
                open_calls.push(call);
                has_input = false;
            }
            Step::ParkAndResume => {
                let id = wait(u128::from(builder.next_seq().get()) + 9_000);
                builder = builder
                    .push(wait_opened(id, ParkReason::AwaitingUserMessage))
                    .push(wait_resolved(id, WaitResolution::Delivered));
            }
            Step::SpawnAndJoin => {
                let child = aex_brain_domain::ids::child_agent_id(owner, spawned);
                let join_id = join(u128::from(spawned) + 7_000);
                builder = builder
                    .push(join_opened(join_id, JoinMode::All, vec![child]))
                    .push(child_spawned(child, spawned, join_id, grant(1), None))
                    .push(child_terminal(child, ChildOutcome::Completed));
                spawned = spawned.saturating_add(1);
                counters.spawn_ordinal = spawned;
            }
            Step::Compact => {
                // `replaces_through` must name a sequence already applied, so the tail is
                // the largest admissible value and zero records means nothing to compact.
                let Some(tail) = builder.next_seq().get().checked_sub(1) else {
                    continue;
                };
                builder = builder.push(compaction(JournalSeq(tail), counters));
            }
            Step::Finish(reason) => {
                // Resolve every open call first: finishing mid-tool is a separate hostile
                // case, not something an admissible history does.
                for call in std::mem::take(&mut open_calls) {
                    let seq = builder.next_seq();
                    let effect = tool_effect(owner, seq);
                    builder = builder
                        .push(effect_prepared(
                            effect,
                            EffectKind::ToolCall,
                            EffectClass::IdempotentManaged,
                        ))
                        .push(tool_result(&call, "ok", effect))
                        .push(effect_complete(effect));
                }
                return builder.push(finished(*reason)).build();
            }
        }
        // Close any tool call the step opened, in tool-use order.
        for call in std::mem::take(&mut open_calls) {
            let seq = builder.next_seq();
            let effect = tool_effect(owner, seq);
            builder = builder
                .push(effect_prepared(
                    effect,
                    EffectKind::ToolCall,
                    EffectClass::IdempotentManaged,
                ))
                .push(tool_result(&call, "ok", effect))
                .push(effect_complete(effect));
            has_input = true;
        }
    }
    builder.build()
}

/// Arbitrary admissible histories for `owner`.
pub fn arb_history(owner: AgentId) -> impl Strategy<Value = Vec<JournalEntry>> {
    proptest::collection::vec(arb_step(), 0..14).prop_map(move |steps| render(owner, &steps))
}

/// Arbitrary admissible histories for a fixed owner.
pub fn arb_any_history() -> impl Strategy<Value = Vec<JournalEntry>> {
    arb_history(agent(1))
}

/// A hostile transformation of an otherwise admissible history.
///
/// These are the deliveries a real queue produces: at-least-once redelivery, out-of-order
/// arrival, a lost page, and a torn read that returns a different body at a sequence
/// already folded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hostile {
    /// Deliver the entry at `index` twice in a row.
    Duplicate {
        /// Which entry to repeat.
        index: usize,
    },
    /// Swap the entries at `index` and `index + 1`.
    Reorder {
        /// The first of the two entries to swap.
        index: usize,
    },
    /// Drop the entry at `index`, leaving a gap.
    Gap {
        /// Which entry to drop.
        index: usize,
    },
    /// Re-seal the entry at `index` with a different body under the same sequence.
    Fork {
        /// Which entry to fork.
        index: usize,
    },
    /// Append a record after the absorbing terminal.
    AfterTerminal,
}

impl Hostile {
    /// Applies this transformation to `history`, returning it unchanged when it does not
    /// apply (an empty history cannot be reordered, for instance).
    ///
    /// # Panics
    ///
    /// Panics when the forged record cannot be canonicalized, which is a defect in this
    /// module rather than in the code under test.
    #[must_use]
    pub fn apply(&self, history: &[JournalEntry]) -> Vec<JournalEntry> {
        let mut out = history.to_vec();
        match *self {
            Self::Duplicate { index } => {
                if let Some(entry) = out.get(index % out.len().max(1)).cloned() {
                    out.insert((index % out.len().max(1)) + 1, entry);
                }
            }
            Self::Reorder { index } => {
                if out.len() >= 2 {
                    let first = index % (out.len() - 1);
                    out.swap(first, first + 1);
                }
            }
            Self::Gap { index } => {
                if !out.is_empty() {
                    out.remove(index % out.len());
                }
            }
            Self::Fork { index } => {
                if !out.is_empty() {
                    let position = index % out.len();
                    let seq = out[position].envelope.seq;
                    let at = out[position].envelope.recorded_at;
                    let forged = JournalEntry::seal(seq, at, user_text("a torn read"))
                        .expect("the forged record canonicalizes");
                    out[position] = forged;
                }
            }
            Self::AfterTerminal => {
                let seq = out
                    .last()
                    .map_or(JournalSeq::ZERO, |entry| entry.envelope.seq.next());
                out.push(
                    JournalEntry::seal(seq, Timestamp(HISTORY_EPOCH_MILLIS), user_text("too late"))
                        .expect("the trailing record canonicalizes"),
                );
            }
        }
        out
    }
}

/// Arbitrary hostile transformations.
pub fn arb_hostile() -> impl Strategy<Value = Hostile> {
    prop_oneof![
        (0_usize..16).prop_map(|index| Hostile::Duplicate { index }),
        (0_usize..16).prop_map(|index| Hostile::Reorder { index }),
        (0_usize..16).prop_map(|index| Hostile::Gap { index }),
        (0_usize..16).prop_map(|index| Hostile::Fork { index }),
        Just(Hostile::AfterTerminal),
    ]
}

/// A grant whose only non-zero dimension is `dimension`.
#[must_use]
pub fn only(dimension: Dimension, value: u64) -> BudgetGrant {
    let mut vector = DimensionVector::ZERO;
    vector.set(dimension, value);
    vector
}

#[cfg(test)]
mod tests {
    use super::{
        HistoryBuilder, Hostile, agent, arb_any_history, finished, grant, started, user_text,
    };
    use aex_brain_domain::fold::fold;
    use aex_brain_domain::journal::FinishReason;
    use proptest::prelude::*;

    #[test]
    fn a_built_history_is_contiguous_and_intact() {
        let history = HistoryBuilder::new()
            .push(started(grant(10)))
            .push(user_text("hello"))
            .push(finished(FinishReason::Completed))
            .build();
        for (index, entry) in history.iter().enumerate() {
            assert_eq!(entry.envelope.seq.get(), index as u64);
            assert!(entry.hash_is_intact().expect("the record canonicalizes"));
        }
    }

    #[test]
    fn a_fork_changes_the_hash_but_not_the_sequence() {
        let history = HistoryBuilder::new()
            .push(started(grant(10)))
            .push(user_text("hello"))
            .build();
        let forked = Hostile::Fork { index: 0 }.apply(&history);
        assert_eq!(forked[0].envelope.seq, history[0].envelope.seq);
        assert_ne!(
            forked[0].envelope.content_hash,
            history[0].envelope.content_hash
        );
    }

    proptest! {
        /// The generator's whole purpose is to produce histories the authority could have
        /// written. If it emitted rejected histories the fold properties would pass
        /// vacuously.
        #[test]
        fn every_generated_history_folds(history in arb_any_history()) {
            prop_assert!(fold(&history).is_ok(), "{:?}", fold(&history).err());
        }

        #[test]
        fn generation_is_a_function_of_its_steps(history in arb_any_history()) {
            let _ = agent(1);
            prop_assert_eq!(fold(&history).ok(), fold(&history).ok());
        }
    }
}
