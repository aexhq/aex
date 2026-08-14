//! The fold: `FoldState`, `apply` and `fold`.
//!
//! `fold` is a loop over `apply`, so the hot incremental path and the cold rehydration
//! path are the same code and cannot drift. Refold-per-append is forbidden: it was
//! measured quadratic, ~5.1 s cumulative at 10 000 entries.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::budget::{BudgetError, BudgetNode, Dimension, StructuralLimits};
use crate::child::{ChildRecord, ChildState, JoinError};
use crate::effect::{EffectState, SettledOutcome};
use crate::ids::{
    AgentId, ContentHash, EffectId, JoinId, JournalSeq, Timestamp, ToolCallId, WaitId,
};
use crate::journal::{
    ExecutorRoute, FinishReason, JournalEntry, JournalRecord, ParkReason, TypedFailure,
    WaitResolution,
};
use crate::wire_pending::{
    CanonicalBlock, CanonicalMessage, ContentBlockRef, JoinGroup, JoinMode, NormalizedUsage,
    ResolvedAgentConfig, Role, StopReason, ToolResultPart,
};
use aex_internal_contracts::RunId;
use aex_wire::ids::MessageId;

/// Where the agent is in its cycle.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum Phase {
    /// No input yet.
    #[default]
    AwaitingInput,
    /// Input is present and a model call is owed.
    AwaitingModel,
    /// The model asked for tools that have not all resolved.
    AwaitingTools,
    /// An effect is open.
    Effecting {
        /// Which effect.
        effect: EffectId,
    },
    /// A durable wait is open and nothing is held.
    Parked {
        /// Why.
        reason: Box<ParkReason>,
    },
    /// Every tool has resolved and the turn may close.
    AwaitingFinish,
    /// Absorbing.
    Finished,
}

/// One tool call the model asked for and that has not resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingCall {
    /// The call identity.
    pub call: ToolCallId,
    /// The tool name.
    pub name: crate::ids::ToolName,
    /// The call's position in the assistant message's tool-use order.
    pub order: u32,
    /// The canonical input.
    pub input: aex_wire::CanonicalJson,
}

/// A tool result waiting to be placed in tool-use order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedCall {
    /// The call's position in the assistant message's tool-use order.
    pub order: u32,
    /// The result blocks.
    pub content: Vec<ToolResultPart>,
    /// Whether the tool reported failure.
    pub is_error: bool,
    /// Which executor ran it.
    pub executed_on: ExecutorRoute,
    /// How long it took.
    pub duration_ms: u32,
}

/// A committed assistant usage observation awaiting the regional FIFO handoff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingModelUsage {
    /// The complete provider message carries provider and model attribution.
    pub message: crate::wire_pending::CompleteAssistantMessage,
    /// Provider-reported usage.
    pub usage: NormalizedUsage,
    /// Time of the authoritative assistant commit.
    pub observed_at: Timestamp,
}

/// Everything the fold derives from an agent's journal.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FoldState {
    /// The pinned configuration, once `AgentStarted` has been applied.
    pub config: Option<Box<ResolvedAgentConfig>>,
    /// The parent, when this agent is a child.
    pub parent: Option<AgentId>,
    /// Ordered model-visible turns. Only complete messages ever enter it.
    pub model_history: Vec<CanonicalMessage>,
    /// The stop reason on the most recently committed assistant message.
    pub last_stop_reason: Option<StopReason>,
    /// The user turn currently accumulating.
    pub open_user: Vec<ContentBlockRef>,
    /// The root session run currently executing, when one was admitted.
    pub active_run: Option<RunId>,
    /// The public message that admitted [`FoldState::active_run`].
    pub active_message: Option<MessageId>,
    /// Run-local spend ceiling for the current root run.
    pub active_max_spend_cents: Option<u64>,
    /// Absolute fence for the current root run.
    pub active_deadline: Option<Timestamp>,
    /// Sealed public output messages produced by the current root run, in journal order.
    pub active_output_messages: Vec<MessageId>,
    /// The effect whose outcome became ambiguous in the current root run.
    pub ambiguous_effect: Option<EffectId>,
    /// Tool calls asked for and not yet resolved, keyed by call id.
    #[serde(with = "ordered_map_entries")]
    pub pending_calls: BTreeMap<ToolCallId, PendingCall>,
    /// Tool results resolved but not yet emitted into a turn.
    #[serde(with = "ordered_map_entries")]
    pub resolved_calls: BTreeMap<ToolCallId, ResolvedCall>,
    /// Calls already resolved in this turn, so a duplicate result is refused.
    pub retired_calls: BTreeSet<ToolCallId>,
    /// Canonical arguments of the last successful `todo_write`.
    ///
    /// This is derived from the durable assistant tool call plus its matching
    /// successful result. It is process-independent fold state, not an
    /// executor cache, and is reconstructed from the durable journal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub todo_state: Option<aex_wire::CanonicalJson>,
    /// Effects opened and not yet settled.
    #[serde(with = "ordered_map_entries")]
    pub open_effects: BTreeMap<EffectId, EffectState>,
    /// Durable call-to-effect links for open members of the current tool batch.
    #[serde(with = "ordered_map_entries")]
    pub tool_effects: BTreeMap<ToolCallId, EffectId>,
    /// Effects that have settled, so a settlement without a preparation is refused.
    pub settled_effects: BTreeSet<EffectId>,
    /// Cumulative provider usage.
    pub usage: NormalizedUsage,
    /// Assistant observations without a matching durable publication marker.
    #[serde(with = "ordered_map_entries")]
    pub pending_model_usage: BTreeMap<EffectId, PendingModelUsage>,
    /// This agent's budget node.
    pub budget: BudgetNode,
    /// Children, keyed by identity.
    #[serde(with = "ordered_map_entries")]
    pub children: BTreeMap<AgentId, ChildRecord>,
    /// Join groups this agent opened.
    #[serde(with = "ordered_map_entries")]
    pub joins: BTreeMap<JoinId, JoinGroup>,
    /// Open durable waits.
    #[serde(with = "ordered_map_entries")]
    pub waits: BTreeMap<WaitId, ParkReason>,
    /// Where the agent is.
    pub phase: Phase,
    /// The last applied sequence, `None` before the first record.
    pub tail: Option<JournalSeq>,
    /// The first sequence still represented in `hashes`, moved forward by a verified
    /// checkpoint baseline.
    pub base_seq: JournalSeq,
    /// The content hash of every applied entry from `base_seq` onward, so a duplicate is a
    /// no-op and a fork is detected rather than folded.
    pub hashes: Vec<ContentHash>,
    /// When the current turn started, for the turn deadline.
    pub turn_started_at: Option<Timestamp>,
    /// How many assistant turns have committed.
    pub assistant_turns: u32,
    /// The parent's monotonic spawn counter.
    pub spawn_ordinal: u32,
    /// The terminal reason, once `AgentFinished` has been applied.
    pub finish: Option<FinishReason>,
    /// The typed failure, when the terminal carried one.
    pub failure: Option<TypedFailure>,
    /// The structural limits every spawn is checked against.
    pub structural: StructuralLimits,
}

/// Stable JSON representation for typed-key maps.
///
/// JSON object keys can only be strings. Encoding a typed identifier as a string-keyed
/// object would make its display spelling part of the persisted JSON contract and would bypass
/// the identifier's normal serde validation. A sorted array of `(key, value)` pairs keeps
/// the typed codec, inherits `BTreeMap` ordering, and permits duplicate rejection on read.
mod ordered_map_entries {
    use std::collections::BTreeMap;

    use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

    pub fn serialize<K, V, S>(map: &BTreeMap<K, V>, serializer: S) -> Result<S::Ok, S::Error>
    where
        K: Serialize + Ord,
        V: Serialize,
        S: Serializer,
    {
        map.iter().collect::<Vec<_>>().serialize(serializer)
    }

    pub fn deserialize<'de, K, V, D>(deserializer: D) -> Result<BTreeMap<K, V>, D::Error>
    where
        K: Deserialize<'de> + Ord,
        V: Deserialize<'de>,
        D: Deserializer<'de>,
    {
        let entries = Vec::<(K, V)>::deserialize(deserializer)?;
        let expected = entries.len();
        let map = entries.into_iter().collect::<BTreeMap<_, _>>();
        if map.len() != expected {
            return Err(D::Error::custom("duplicate typed map key"));
        }
        Ok(map)
    }
}

/// Why a record could not be folded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FoldError {
    /// The journal is not contiguous. The agent does not fold and does not act.
    #[error("journal gap: expected sequence {expected}, found {found}")]
    JournalGap {
        /// The sequence the fold required.
        expected: JournalSeq,
        /// The sequence that arrived.
        found: JournalSeq,
    },
    /// The same sequence arrived with a different hash. The agent quarantines.
    #[error("journal fork at sequence {seq}: {stored} was already applied, {arriving} arrived")]
    JournalForked {
        /// Where the fork is.
        seq: JournalSeq,
        /// The hash already folded.
        stored: ContentHash,
        /// The hash that arrived.
        arriving: ContentHash,
    },
    /// A record arrived after the agent finished.
    #[error("agent finished {reason:?}; `{kind}` at sequence {seq} is rejected")]
    TerminalAbsorbing {
        /// Why the agent finished.
        reason: FinishReason,
        /// The record kind that arrived.
        kind: &'static str,
        /// Where it arrived.
        seq: JournalSeq,
    },
    /// A record other than `AgentStarted` arrived first.
    #[error("`{kind}` arrived before `agent_started`")]
    NotStarted {
        /// The record kind that arrived.
        kind: &'static str,
    },
    /// A second `AgentStarted` arrived.
    #[error("agent is already started")]
    AlreadyStarted,
    /// A tool result answered a call the model never made.
    #[error("tool result for unknown call `{call}`")]
    UnknownCall {
        /// The call.
        call: String,
    },
    /// A tool result answered a call that was already resolved.
    #[error("duplicate tool result for call `{call}`")]
    DuplicateToolResult {
        /// The call.
        call: String,
    },
    /// An assistant message arrived whose completeness proof does not cover its blocks.
    #[error("assistant message at sequence {seq} is not covered by its completeness proof")]
    UnprovenAssistantMessage {
        /// Where it arrived.
        seq: JournalSeq,
    },
    /// A publication marker named no committed assistant usage observation.
    #[error("model usage publication for unknown effect {effect}")]
    UnknownModelUsagePublication {
        /// The effect named by the malformed marker.
        effect: EffectId,
    },
    /// The provider receipt does not name or commit to the assistant outcome.
    #[error("provider receipt at sequence {seq} does not identify the assistant outcome")]
    ReceiptMismatch {
        /// Where it arrived.
        seq: JournalSeq,
    },
    /// A content reference reached the pure fold before the store hydrated it.
    #[error("placed content at sequence {seq} was not hydrated before folding")]
    UnhydratedContent {
        /// Where it arrived.
        seq: JournalSeq,
    },
    /// The same effect was prepared twice.
    #[error("effect {effect} is already open")]
    DuplicateEffect {
        /// The effect.
        effect: EffectId,
    },
    /// An effect settled that was never prepared.
    #[error("effect {effect} settled without being prepared")]
    UnknownEffect {
        /// The effect.
        effect: EffectId,
    },
    /// A tool effect did not name the immutable batch call it executes.
    #[error("tool effect {effect} does not name a tool call")]
    MissingToolEffectCall {
        /// The malformed tool effect.
        effect: EffectId,
    },
    /// A non-tool effect attempted to carry a tool-batch call link.
    #[error("non-tool effect {effect} unexpectedly names a tool call")]
    UnexpectedToolEffectCall {
        /// The malformed non-tool effect.
        effect: EffectId,
    },
    /// Two open effects attempted to execute the same batch call.
    #[error("tool call `{call}` already has an open effect")]
    DuplicateToolEffectCall {
        /// Provider-minted duplicate call identity.
        call: String,
    },
    /// A run boundary did not name the root run currently owned by this fold.
    #[error("run boundary for {presented} does not match active run {active:?}")]
    RunBoundaryMismatch {
        /// The active run, when one exists.
        active: Option<RunId>,
        /// What the record presented.
        presented: RunId,
    },
    /// A run boundary did not carry the exact public output-message sequence.
    #[error("run boundary output messages disagree with the folded public messages")]
    RunOutputMismatch,
    /// A budget operation was refused.
    #[error(transparent)]
    Budget(#[from] BudgetError),
    /// A join operation was refused.
    #[error(transparent)]
    Join(#[from] JoinError),
    /// A child record referred to an agent that was never spawned here.
    #[error("child {child:?} is unknown to this agent")]
    UnknownChild {
        /// The child.
        child: AgentId,
    },
    /// A second `ChildSpawned` for the same identity.
    #[error("child {child:?} is already spawned")]
    DuplicateChild {
        /// The child.
        child: AgentId,
    },
    /// A `WaitResolved` for a wait that is not open.
    #[error("wait {wait:?} is not open")]
    UnknownWait {
        /// The wait.
        wait: WaitId,
    },
    /// The envelope hash does not cover the record it carries.
    #[error("envelope hash at sequence {seq} does not cover its record")]
    EnvelopeHashMismatch {
        /// Where.
        seq: JournalSeq,
    },
    /// The record could not be canonicalized to check its hash.
    #[error(transparent)]
    Canonical(#[from] crate::canonical::CanonicalizeError),
}

impl FoldState {
    /// The state of an agent with no journal at all.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            phase: Phase::AwaitingInput,
            ..Self::default()
        }
    }

    /// The sequence the next record must carry.
    #[must_use]
    pub fn expected_seq(&self) -> JournalSeq {
        self.tail.map_or(JournalSeq::ZERO, JournalSeq::next)
    }

    /// Whether the agent has reached its absorbing terminal.
    #[must_use]
    pub const fn is_finished(&self) -> bool {
        self.finish.is_some()
    }

    fn stored_hash(&self, seq: JournalSeq) -> Option<ContentHash> {
        let offset = usize::try_from(seq.get().checked_sub(self.base_seq.get())?).ok()?;
        self.hashes.get(offset).copied()
    }
}

#[cfg(test)]
mod control_projection_tests {
    use super::{FoldState, PendingCall, Phase, apply_record, project_control_state};
    use crate::ids::{JournalSeq, Timestamp, ToolCallId, ToolName};
    use crate::journal::{ExecutorRoute, JournalEntry, JournalRecord};
    use aex_wire::CanonicalJson;
    use aex_wire::ids::{MessageId, PrefixedId as _, Uuid7};

    fn todo_write(input: &str) -> PendingCall {
        PendingCall {
            call: ToolCallId::truncating("todo-call"),
            name: ToolName::parse("todo_write").expect("tool name"),
            order: 0,
            input: CanonicalJson::parse(input).expect("canonical input"),
        }
    }

    #[test]
    fn only_a_successful_todo_write_replaces_the_folded_control_state() {
        let first = todo_write(
            r#"{"todos":[{"activeForm":"Doing","content":"First","status":"in_progress"}]}"#,
        );
        let failed = todo_write(
            r#"{"todos":[{"activeForm":"Doing","content":"Failed","status":"completed"}]}"#,
        );
        let mut state = FoldState::empty();

        project_control_state(&mut state, &first, false, ExecutorRoute::BrainInline);
        assert_eq!(state.todo_state.as_ref(), Some(&first.input));
        project_control_state(&mut state, &failed, true, ExecutorRoute::BrainInline);
        assert_eq!(
            state.todo_state.as_ref(),
            Some(&first.input),
            "a tool-level failure cannot mutate the folded replacement"
        );
        project_control_state(&mut state, &failed, false, ExecutorRoute::ToolMux);
        assert_eq!(
            state.todo_state.as_ref(),
            Some(&first.input),
            "a result attributed to another executor cannot mutate Brain control state"
        );
    }

    #[test]
    fn run_admission_pins_internal_execution_and_enters_model_work() {
        let run = aex_internal_contracts::RunId::from_uuid7(Uuid7::compose(1, [1; 10]));
        let message = MessageId::from_uuid7(Uuid7::compose(2, [2; 10]));
        let deadline = Timestamp::from_millis(90_000);
        let entry = JournalEntry::seal(
            JournalSeq::ZERO,
            Timestamp::from_millis(1_000),
            JournalRecord::RunAdmitted {
                run,
                message,
                content: Vec::new(),
                max_spend_cents: 1_000,
                deadline,
                limits_revision: 7,
            },
        )
        .expect("the admission record seals");
        let mut state = FoldState::empty();

        apply_record(&mut state, &entry).expect("run admission folds");

        assert_eq!(state.active_run, Some(run));
        assert_eq!(state.active_message, Some(message));
        assert_eq!(state.active_max_spend_cents, Some(1_000));
        assert_eq!(state.active_deadline, Some(deadline));
        assert_eq!(state.turn_started_at, Some(Timestamp::from_millis(1_000)));
        assert_eq!(state.phase, Phase::AwaitingModel);
    }

    #[test]
    fn a_root_run_boundary_returns_to_input_without_finishing_the_agent() {
        let run = aex_internal_contracts::RunId::from_uuid7(Uuid7::compose(1, [1; 10]));
        let input = MessageId::from_uuid7(Uuid7::compose(2, [2; 10]));
        let output = MessageId::from_uuid7(Uuid7::compose(3, [3; 10]));
        let mut state = FoldState::empty();
        let admitted = JournalEntry::seal(
            JournalSeq::ZERO,
            Timestamp::from_millis(1_000),
            JournalRecord::RunAdmitted {
                run,
                message: input,
                content: Vec::new(),
                max_spend_cents: 1_000,
                deadline: Timestamp::from_millis(90_000),
                limits_revision: 7,
            },
        )
        .expect("admission seals");
        apply_record(&mut state, &admitted).expect("admission folds");
        state.active_output_messages.push(output);
        let finished = JournalEntry::seal(
            JournalSeq(1),
            Timestamp::from_millis(2_000),
            JournalRecord::RunFinished {
                run,
                reason: crate::journal::FinishReason::Completed,
                failure: None,
                output_messages: vec![output],
                ambiguous_effect: None,
            },
        )
        .expect("boundary seals");

        apply_record(&mut state, &finished).expect("boundary folds");

        assert_eq!(state.phase, Phase::AwaitingInput);
        assert_eq!(state.active_run, None);
        assert_eq!(state.active_message, None);
        assert!(state.active_output_messages.is_empty());
        assert!(
            !state.is_finished(),
            "the retained root accepts another message"
        );
    }
}

/// Folds `entries` into a state.
///
/// # Errors
///
/// Returns the first [`FoldError`] any entry produces. A journal that does not fold is
/// never partially applied by the caller: this function owns its own state.
pub fn fold(entries: &[JournalEntry]) -> Result<FoldState, FoldError> {
    let mut state = FoldState::empty();
    for entry in entries {
        apply(&mut state, entry)?;
    }
    Ok(state)
}

/// Applies one entry.
///
/// Duplicate delivery of an identical `(seq, content_hash)` is a no-op. The same sequence
/// with a different hash is [`FoldError::JournalForked`] and quarantines the agent.
///
/// # Errors
///
/// Returns [`FoldError`] when the entry is out of sequence, forked, arrives after the
/// terminal, or violates a fold invariant. On error the state is left unchanged in every
/// field the failed record would have touched.
pub fn apply(state: &mut FoldState, entry: &JournalEntry) -> Result<(), FoldError> {
    let seq = entry.envelope.seq;
    let arriving = entry.envelope.content_hash;

    if !entry.hash_is_intact()? {
        return Err(FoldError::EnvelopeHashMismatch { seq });
    }

    let expected = state.expected_seq();
    if seq < expected {
        return match state.stored_hash(seq) {
            Some(stored) if stored == arriving => Ok(()),
            Some(stored) => Err(FoldError::JournalForked {
                seq,
                stored,
                arriving,
            }),
            // The prefix was compacted away, so the hash is no longer retained. A record
            // before the compaction boundary can only be a redelivery of something the
            // compaction already absorbed.
            None => Ok(()),
        };
    }
    if seq > expected {
        return Err(FoldError::JournalGap {
            expected,
            found: seq,
        });
    }

    if let Some(reason) = state.finish {
        return Err(FoldError::TerminalAbsorbing {
            reason,
            kind: entry.record.kind_name(),
            seq,
        });
    }
    if state.config.is_none() && !matches!(entry.record, JournalRecord::AgentStarted { .. }) {
        return Err(FoldError::NotStarted {
            kind: entry.record.kind_name(),
        });
    }

    apply_record(state, entry)?;

    state.tail = Some(seq);
    state.hashes.push(arriving);
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn apply_record(state: &mut FoldState, entry: &JournalEntry) -> Result<(), FoldError> {
    let seq = entry.envelope.seq;
    match &entry.record {
        JournalRecord::AgentStarted {
            config,
            parent,
            join,
            depth,
            budget,
        } => {
            if state.config.is_some() {
                return Err(FoldError::AlreadyStarted);
            }
            state.config = Some(config.clone());
            state.parent = *parent;
            state.budget = BudgetNode {
                limit: *budget,
                depth: *depth,
                ..BudgetNode::default()
            };
            state.structural = StructuralLimits {
                max_depth: config.limits.max_depth,
                max_fanout: config.limits.max_fanout,
            };
            if let Some(join) = join {
                state.joins.entry(*join).or_insert_with(|| JoinGroup {
                    join: *join,
                    mode: JoinMode::All,
                    members: Vec::new(),
                    done: Vec::new(),
                    shards: 1,
                });
            }
            state.phase = Phase::AwaitingInput;
        }

        JournalRecord::RunAdmitted {
            run,
            message,
            content,
            max_spend_cents,
            deadline,
            limits_revision: _,
        } => {
            // Model history and cumulative usage survive run boundaries. Assistant-turn
            // count remains diagnostic/control context; it is not a semantic stop limit.
            state.assistant_turns = 0;
            state.active_run = Some(*run);
            state.active_message = Some(*message);
            state.active_max_spend_cents = Some(*max_spend_cents);
            state.active_deadline = Some(*deadline);
            state.open_user.extend(content.iter().cloned());
            state.turn_started_at = Some(entry.envelope.recorded_at);
            state.phase = Phase::AwaitingModel;
        }

        JournalRecord::UserMessage { content, origin } => {
            state.open_user.extend(content.iter().cloned());
            if matches!(state.phase, Phase::AwaitingFinish | Phase::AwaitingInput) {
                // A new user message re-anchors the turn deadline: the customer just gave
                // the agent fresh work, and charging that work against the previous turn's
                // clock would time out a healthy conversation.
                state.turn_started_at = Some(entry.envelope.recorded_at);
            }
            state.phase = Phase::AwaitingModel;
            let _ = origin;
        }

        JournalRecord::AssistantMessage {
            public_message,
            message,
            usage,
            receipt,
            effect,
        } => {
            if !message.proof_covers(usage) {
                return Err(FoldError::UnprovenAssistantMessage { seq });
            }
            if !receipt.matches_outcome(message, usage) {
                return Err(FoldError::ReceiptMismatch { seq });
            }
            close_user_turn(state, seq)?;
            state.model_history.push(message.as_message());
            state.last_stop_reason = Some(message.stop_reason);
            state.usage.add(*usage);
            state.pending_model_usage.insert(
                *effect,
                PendingModelUsage {
                    message: message.clone(),
                    usage: *usage,
                    observed_at: entry.envelope.recorded_at,
                },
            );
            state.assistant_turns = state.assistant_turns.saturating_add(1);
            state.retired_calls.clear();
            state.resolved_calls.clear();
            state.pending_calls.clear();
            let mut order = 0_u32;
            for block in &message.blocks {
                if let CanonicalBlock::ToolUse { id, name, input } = block {
                    state.pending_calls.insert(
                        id.clone(),
                        PendingCall {
                            call: id.clone(),
                            name: name.clone(),
                            order,
                            input: input.clone(),
                        },
                    );
                    order = order.saturating_add(1);
                }
            }
            state.phase = if state.pending_calls.is_empty() {
                Phase::AwaitingFinish
            } else {
                Phase::AwaitingTools
            };
            if let Some(message) = public_message {
                state.active_output_messages.push(*message);
            }
        }

        JournalRecord::ModelUsagePublished { effect } => {
            if state.pending_model_usage.remove(effect).is_none() {
                return Err(FoldError::UnknownModelUsagePublication { effect: *effect });
            }
        }

        JournalRecord::ToolResult {
            public_message,
            call,
            content,
            is_error,
            executed_on,
            duration_ms,
            ..
        } => {
            if state.retired_calls.contains(call) || state.resolved_calls.contains_key(call) {
                return Err(FoldError::DuplicateToolResult {
                    call: call.as_str().to_owned(),
                });
            }
            let pending =
                state
                    .pending_calls
                    .remove(call)
                    .ok_or_else(|| FoldError::UnknownCall {
                        call: call.as_str().to_owned(),
                    })?;
            project_control_state(state, &pending, *is_error, *executed_on);
            state.resolved_calls.insert(
                call.clone(),
                ResolvedCall {
                    order: pending.order,
                    content: content.clone(),
                    is_error: *is_error,
                    executed_on: *executed_on,
                    duration_ms: *duration_ms,
                },
            );
            if state.pending_calls.is_empty() {
                emit_tool_result_turn(state);
                state.phase = Phase::AwaitingModel;
            } else {
                state.phase = Phase::AwaitingTools;
            }
            if let Some(message) = public_message {
                state.active_output_messages.push(*message);
            }
        }

        JournalRecord::EffectPrepared {
            effect,
            tool_call,
            kind,
            attempt,
            reservation,
            ..
        } => {
            if state.open_effects.contains_key(effect) {
                return Err(FoldError::DuplicateEffect { effect: *effect });
            }
            for delta in reservation {
                state.budget.consume(delta.dimension, delta.quantity)?;
            }
            state
                .open_effects
                .insert(*effect, EffectState::Prepared { attempt: *attempt });
            if *kind == crate::effect::EffectKind::ToolCall {
                let call = tool_call
                    .as_ref()
                    .ok_or(FoldError::MissingToolEffectCall { effect: *effect })?;
                if state.tool_effects.insert(call.clone(), *effect).is_some() {
                    return Err(FoldError::DuplicateToolEffectCall {
                        call: call.as_str().to_owned(),
                    });
                }
            } else if tool_call.is_some() {
                return Err(FoldError::UnexpectedToolEffectCall { effect: *effect });
            }
            state.phase = Phase::Effecting { effect: *effect };
        }

        JournalRecord::EffectSettled {
            effect, outcome, ..
        } => {
            if state.open_effects.remove(effect).is_none() {
                return Err(FoldError::UnknownEffect { effect: *effect });
            }
            let tool_effect = state
                .tool_effects
                .iter()
                .find_map(|(call, linked)| (*linked == *effect).then(|| call.clone()));
            if let Some(call) = &tool_effect {
                state.tool_effects.remove(call);
            }
            state.settled_effects.insert(*effect);
            if let SettledOutcome::OutcomeUnknown { .. } = outcome {
                if tool_effect.is_some() {
                    state.phase = if state.pending_calls.is_empty() {
                        Phase::AwaitingFinish
                    } else {
                        Phase::AwaitingTools
                    };
                } else {
                    // A child remains terminal for life. The root agent instead owes a
                    // `RunFinished` boundary that settles the current message and returns
                    // the retained session to input-ready state.
                    if state.parent.is_none() && state.active_run.is_some() {
                        state.ambiguous_effect = Some(*effect);
                        state.phase = Phase::AwaitingFinish;
                    } else {
                        state.finish = Some(FinishReason::Interrupted);
                        state.phase = Phase::Finished;
                    }
                }
            } else if matches!(state.phase, Phase::Effecting { effect: open } if open == *effect) {
                state.phase = if state.pending_calls.is_empty() {
                    Phase::AwaitingFinish
                } else {
                    Phase::AwaitingTools
                };
            }
        }

        JournalRecord::ChildSpawned {
            child,
            ordinal,
            grant,
            join,
            queued_reason,
        } => {
            if state.children.contains_key(child) {
                return Err(FoldError::DuplicateChild { child: *child });
            }
            state.budget.spawn(*grant, state.structural)?;
            state.budget.consume(Dimension::TotalChildrenCreated, 1)?;
            state.children.insert(
                *child,
                ChildRecord {
                    child: *child,
                    ordinal: *ordinal,
                    grant: *grant,
                    join: *join,
                    state: queued_reason
                        .map_or(ChildState::Starting, |reason| ChildState::Queued { reason }),
                },
            );
            state.spawn_ordinal = state.spawn_ordinal.max(ordinal.saturating_add(1));
            if let Some(group) = state.joins.get_mut(join)
                && !group.members.contains(child)
            {
                group.members.push(*child);
            }
        }

        JournalRecord::ChildTerminal {
            child,
            outcome,
            rolled_up,
            ..
        } => {
            let record = state
                .children
                .get_mut(child)
                .ok_or(FoldError::UnknownChild { child: *child })?;
            if record.state.is_terminal() {
                // A duplicate terminal fact is idempotent by `(child_id, join_id)`.
                return Ok(());
            }
            record.state = outcome.state();
            let join = record.join;
            let grant = record.grant;
            let mut settled = BudgetNode {
                limit: grant,
                ..BudgetNode::default()
            };
            for delta in rolled_up {
                settled.consume(delta.dimension, delta.quantity)?;
            }
            state.budget.release_child(&settled)?;
            if let Some(group) = state.joins.get_mut(&join)
                && !group.done.contains(child)
            {
                group.done.push(*child);
            }
        }

        JournalRecord::ChildStateChanged { child, state: next } => {
            let record = state
                .children
                .get_mut(child)
                .ok_or(FoldError::UnknownChild { child: *child })?;
            record.state = *next;
        }

        JournalRecord::JoinOpened {
            join,
            mode,
            members,
            shards,
        } => {
            let group = state.joins.entry(*join).or_insert_with(|| JoinGroup {
                join: *join,
                mode: *mode,
                members: Vec::new(),
                done: Vec::new(),
                shards: *shards,
            });
            group.mode = *mode;
            group.shards = *shards;
            for member in members {
                if !group.members.contains(member) {
                    group.members.push(*member);
                }
            }
        }

        JournalRecord::WaitOpened { wait, reason, .. } => {
            state.waits.insert(*wait, reason.clone());
            state.phase = Phase::Parked {
                reason: Box::new(reason.clone()),
            };
        }

        JournalRecord::WaitResolved { wait, resolution } => {
            if state.waits.remove(wait).is_none() {
                return Err(FoldError::UnknownWait { wait: *wait });
            }
            state.phase = if matches!(resolution, WaitResolution::Cancelled) {
                Phase::AwaitingFinish
            } else if !state.pending_calls.is_empty() {
                Phase::AwaitingTools
            } else if state.open_user.is_empty() && state.resolved_calls.is_empty() {
                Phase::AwaitingInput
            } else {
                Phase::AwaitingModel
            };
        }

        JournalRecord::RunFinished {
            run,
            reason: _,
            failure: _,
            output_messages,
            ambiguous_effect: _,
        } => {
            if state.parent.is_some() || state.active_run != Some(*run) {
                return Err(FoldError::RunBoundaryMismatch {
                    active: state.active_run,
                    presented: *run,
                });
            }
            if state.active_output_messages != *output_messages {
                return Err(FoldError::RunOutputMismatch);
            }
            state.active_run = None;
            state.active_message = None;
            state.active_max_spend_cents = None;
            state.active_deadline = None;
            state.active_output_messages.clear();
            state.ambiguous_effect = None;
            state.open_user.clear();
            state.pending_calls.clear();
            state.resolved_calls.clear();
            state.retired_calls.clear();
            state.open_effects.clear();
            state.turn_started_at = None;
            state.assistant_turns = 0;
            state.failure = None;
            state.finish = None;
            state.phase = Phase::AwaitingInput;
        }

        JournalRecord::AgentFinished { reason, failure } => {
            state.finish = Some(*reason);
            state.failure.clone_from(failure);
            state.phase = Phase::Finished;
        }
    }
    Ok(())
}

/// Closes the accumulating user turn into model-visible history, if there is one.
fn close_user_turn(state: &mut FoldState, seq: JournalSeq) -> Result<(), FoldError> {
    if state.open_user.is_empty() {
        return Ok(());
    }
    if state
        .open_user
        .iter()
        .any(|reference| matches!(reference, ContentBlockRef::Placed { .. }))
    {
        return Err(FoldError::UnhydratedContent { seq });
    }
    let blocks = state
        .open_user
        .drain(..)
        .filter_map(|reference| match reference {
            ContentBlockRef::Inline { block } => Some(block),
            ContentBlockRef::Placed { .. } => None,
        })
        .collect();
    state.model_history.push(CanonicalMessage {
        role: Role::User,
        blocks,
    });
    Ok(())
}

/// Emits the resolved tool results as one user turn, in tool-use order.
///
/// Order comes from the assistant message that asked for the calls, not from the order the
/// results happened to arrive in, so a slow tool cannot reorder a turn.
fn emit_tool_result_turn(state: &mut FoldState) {
    let mut resolved: Vec<(ToolCallId, ResolvedCall)> = state
        .resolved_calls
        .iter()
        .map(|(call, result)| (call.clone(), result.clone()))
        .collect();
    resolved.sort_by_key(|(_, result)| result.order);
    let blocks: Vec<CanonicalBlock> = resolved
        .iter()
        .map(|(call, result)| CanonicalBlock::ToolResult {
            call: call.clone(),
            content: result.content.clone(),
            is_error: result.is_error,
        })
        .collect();
    for (call, _) in resolved {
        state.retired_calls.insert(call);
    }
    state.resolved_calls.clear();
    state.model_history.push(CanonicalMessage {
        role: Role::User,
        blocks,
    });
}

fn project_control_state(
    state: &mut FoldState,
    pending: &PendingCall,
    is_error: bool,
    executed_on: ExecutorRoute,
) {
    if pending.name.as_str() == "todo_write"
        && !is_error
        && executed_on == ExecutorRoute::BrainInline
    {
        state.todo_state = Some(pending.input.clone());
    }
}

/// A turn-neutral summary of the fold, for diagnostics and admission decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoldSummary {
    /// The last applied sequence.
    pub tail: Option<JournalSeq>,
    /// How many model-visible turns exist.
    pub turns: usize,
    /// How many tool calls are outstanding.
    pub pending_calls: usize,
    /// How many effects are open.
    pub open_effects: usize,
    /// How many children exist.
    pub children: usize,
    /// The terminal reason, when the agent has one.
    pub finish: Option<FinishReason>,
}

impl FoldState {
    /// A turn-neutral summary of this fold.
    #[must_use]
    pub fn summary(&self) -> FoldSummary {
        FoldSummary {
            tail: self.tail,
            turns: self.model_history.len(),
            pending_calls: self.pending_calls.len(),
            open_effects: self.open_effects.len(),
            children: self.children.len(),
            finish: self.finish,
        }
    }

    /// Whether every open effect carries evidence that permits a retry.
    ///
    /// Used by the activation orchestrator to decide whether an owner that has just
    /// claimed may plan a new step at all.
    #[must_use]
    pub fn has_ambiguous_effect(&self) -> bool {
        self.open_effects
            .values()
            .any(|state| !matches!(state, EffectState::Prepared { .. }))
    }
}

/// Convenience constructors used by the planner and by test histories.
impl FoldState {
    /// The unresolved calls in tool-use order.
    #[must_use]
    pub fn pending_in_order(&self) -> Vec<PendingCall> {
        let mut calls: Vec<PendingCall> = self.pending_calls.values().cloned().collect();
        calls.sort_by_key(|call| call.order);
        calls
    }
}
