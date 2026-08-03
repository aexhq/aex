//! The closed journal record set and its bounded canonical decoder.
//!
//! One agent owns one contiguous append-only journal. [`JournalRecord`] is a **closed**
//! enum: a new variant is a new schema revision, never an open string kind. An unknown
//! wire kind is a typed [`JournalDecodeError`], not a silent passthrough — the TypeScript
//! forward-compatibility passthrough is deleted because one generated internal contract
//! now covers both sides of the wire, and passthrough only ever hid corruption.

use serde::{Deserialize, Serialize};

use crate::budget::{BudgetDelta, BudgetGrant};
use crate::child::{ChildOutcome, ChildState, QueuedReason};
use crate::effect::{DetachedOperationRef, EffectClass, EffectKind, SettledOutcome};
use crate::ids::{
    AgentId, ContentHash, EffectId, JoinId, JournalSeq, Timestamp, ToolCallId, WaitId,
};
use crate::wire_pending::{
    CanonicalBlock, CompleteAssistantMessage, ContentBlockRef, ContentRef, JoinMode,
    JournalEnvelope, NormalizedUsage, ResolvedAgentConfig, ToolResultPart,
};
use aex_model_catalog::canonical::ProviderReceipt;

/// The inline body boundary.
///
/// A canonical body at or below this stays in the journal item; a larger one is written to
/// the content authority first and referenced.
pub const INLINE_BODY_BYTES: usize = 32_768;

/// Why an agent stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    /// The model finished the work.
    Completed,
    /// The per-agent turn limit was reached.
    MaxTurns,
    /// The per-turn step limit was reached.
    MaxSteps,
    /// A budget dimension was exhausted.
    Budget,
    /// The turn deadline passed.
    Timeout,
    /// Cancelled by the caller or by an epoch advance.
    Cancelled,
    /// A typed failure, including a truncated provider response.
    Failed,
    /// The projection of an `OutcomeUnknown` effect. Nothing else may produce it.
    Interrupted,
}

impl FinishReason {
    /// Whether this reason reports success to the customer.
    #[must_use]
    pub const fn is_success(self) -> bool {
        matches!(self, Self::Completed)
    }
}

/// A typed failure carried by a terminal record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypedFailure {
    /// A stable machine-readable code.
    pub code: String,
    /// A redacted human-readable message. Never carries a credential.
    pub message: String,
    /// A pointer to the diagnostic detail in the content authority, when one exists.
    pub detail: Option<ContentRef>,
}

/// Where a user message came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageOrigin {
    /// The run submission that created the agent.
    Submission,
    /// A later public API message.
    Api,
    /// A parent's `send_message` into a child's mailbox.
    ParentMessage,
}

/// Why an agent is parked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "park", rename_all = "snake_case", deny_unknown_fields)]
pub enum ParkReason {
    /// Waiting for the next user message.
    AwaitingUserMessage,
    /// Waiting for a detached tool or MCP Task.
    AwaitingToolResult {
        /// The call the agent is waiting on.
        call: ToolCallId,
        /// The executor-bound operation being polled.
        operation: DetachedOperationRef,
    },
    /// Waiting for a Hands operation.
    AwaitingHandsOperation {
        /// The operation the agent is waiting on.
        operation: crate::ids::HandsOperationId,
    },
    /// Waiting on a join group.
    AwaitingChildren {
        /// The join group.
        join: JoinId,
    },
    /// Waiting for a durable timer.
    AwaitingTimer {
        /// When the timer is due.
        due: Timestamp,
    },
    /// Waiting for an approval decision.
    AwaitingApproval {
        /// The approval request.
        id: String,
    },
    /// Waiting for scheduler capacity.
    AwaitingCapacity {
        /// Which limit is binding.
        reason: QueuedReason,
    },
    /// Re-armed by drain, waiting to be picked up elsewhere.
    AwaitingDrainReroute,
}

/// How a durable wait ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "resolution", rename_all = "snake_case")]
pub enum WaitResolution {
    /// The awaited input arrived.
    Delivered,
    /// The wait's due time passed.
    Expired,
    /// The wait was cancelled.
    Cancelled,
}

/// Counters a compaction must carry forward exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PreservedCounters {
    /// Cumulative usage across the whole replaced prefix.
    pub usage: NormalizedUsage,
    /// How many assistant turns the replaced prefix contained.
    pub assistant_turns: u32,
    /// The parent's spawn counter at the compaction point.
    pub spawn_ordinal: u32,
}

/// Which executor actually ran a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutorRoute {
    /// Ran inside the mux process.
    BrainInline,
    /// Ran through the managed web adapter.
    ManagedWeb,
    /// Ran through an MCP server.
    Mcp,
    /// Ran inside the session's Hands `MicroVM`.
    Hands,
}

/// One semantic fact in an agent's journal.
///
/// The enum is closed on purpose. `serde`'s default enum handling rejects an unknown tag,
/// which is exactly the behaviour required: a kind this build does not know is a decode
/// error and the agent does not fold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "record", rename_all = "snake_case", deny_unknown_fields)]
pub enum JournalRecord {
    /// The agent exists and is pinned to its configuration.
    AgentStarted {
        /// The pinned configuration for the agent's whole life.
        config: Box<ResolvedAgentConfig>,
        /// The parent, when this is a child.
        parent: Option<AgentId>,
        /// The join group this agent belongs to, when it is a child.
        join: Option<JoinId>,
        /// Lineage depth, root at zero.
        depth: u16,
        /// What the parent (or the session) granted.
        budget: BudgetGrant,
    },
    /// Input from the customer, the API, or a parent's mailbox.
    UserMessage {
        /// The blocks of the message.
        content: Vec<ContentBlockRef>,
        /// Where it came from.
        origin: MessageOrigin,
    },
    /// A **complete** assistant message. Partial deltas can never reach here.
    AssistantMessage {
        /// The whole provider/model/catalog-bound message and completeness proof.
        message: CompleteAssistantMessage,
        /// Provider-reported usage.
        usage: NormalizedUsage,
        /// Dispatch identity, byte counts, attempts and binding identity.
        receipt: Box<ProviderReceipt>,
        /// The effect that produced it.
        effect: EffectId,
    },
    /// The result of one tool call.
    ToolResult {
        /// Which call this answers.
        call: ToolCallId,
        /// Result content in the canonical non-recursive tool-result vocabulary.
        content: Vec<ToolResultPart>,
        /// Whether the tool reported failure.
        is_error: bool,
        /// Which executor ran it.
        executed_on: ExecutorRoute,
        /// How long it took.
        duration_ms: u32,
        /// The effect that produced it.
        effect: EffectId,
    },
    /// Intent to perform external work. Committed before any byte leaves.
    EffectPrepared {
        /// Deterministic identity.
        effect: EffectId,
        /// What kind of work.
        kind: EffectKind,
        /// Its recovery contract.
        class: EffectClass,
        /// `blake3` over the canonical request.
        request_hash: ContentHash,
        /// When this attempt must have settled by.
        deadline: Timestamp,
        /// The attempt number, from one.
        attempt: u16,
        /// The budget this effect reserves.
        reservation: Vec<BudgetDelta>,
    },
    /// How the external work settled. Atomic with the record it produced.
    EffectSettled {
        /// Which effect.
        effect: EffectId,
        /// How it settled.
        outcome: SettledOutcome,
        /// What the reservation became.
        charged: Vec<BudgetDelta>,
    },
    /// A child agent was durably created.
    ChildSpawned {
        /// The child's deterministic identity.
        child: AgentId,
        /// The parent's spawn counter value that derived it.
        ordinal: u32,
        /// What the parent reserved.
        grant: BudgetGrant,
        /// The join group it belongs to.
        join: JoinId,
        /// Which limit is binding, when it was created queued.
        queued_reason: Option<QueuedReason>,
    },
    /// A child reached a terminal state.
    ChildTerminal {
        /// Which child.
        child: AgentId,
        /// How it ended.
        outcome: ChildOutcome,
        /// The conserved budget it actually used, rolled up to the parent.
        rolled_up: Vec<BudgetDelta>,
        /// A pointer to its committed result, when it produced one.
        result: Option<ContentRef>,
    },
    /// A durable wait opened. The activation releases everything and ends.
    WaitOpened {
        /// The wait identity.
        wait: WaitId,
        /// Why the agent is parked.
        reason: ParkReason,
        /// When the wait is due, for the reasons that have one.
        due: Option<Timestamp>,
    },
    /// A durable wait ended.
    WaitResolved {
        /// Which wait.
        wait: WaitId,
        /// How it ended.
        resolution: WaitResolution,
    },
    /// The model-visible prefix was replaced by a summary.
    Compaction {
        /// The last sequence the summary replaces, inclusive.
        replaces_through: JournalSeq,
        /// The summary that stands in for the replaced prefix.
        summary: Vec<CanonicalBlock>,
        /// Counters carried forward exactly.
        preserved: PreservedCounters,
    },
    /// The absorbing terminal.
    AgentFinished {
        /// Why the agent stopped.
        reason: FinishReason,
        /// The typed failure, when the reason carries one.
        failure: Option<TypedFailure>,
    },
    /// A join group was opened over a set of children.
    JoinOpened {
        /// The join identity.
        join: JoinId,
        /// Whether the parent releases on the first terminal member or on all of them.
        mode: JoinMode,
        /// Every member.
        members: Vec<AgentId>,
        /// The shard count chosen at creation.
        shards: u16,
    },
    /// A child's durable state changed without ending.
    ChildStateChanged {
        /// Which child.
        child: AgentId,
        /// Its new state.
        state: ChildState,
    },
}

impl JournalRecord {
    /// A stable discriminant for diagnostics and item attributes.
    #[must_use]
    pub const fn kind_name(&self) -> &'static str {
        match self {
            Self::AgentStarted { .. } => "agent_started",
            Self::UserMessage { .. } => "user_message",
            Self::AssistantMessage { .. } => "assistant_message",
            Self::ToolResult { .. } => "tool_result",
            Self::EffectPrepared { .. } => "effect_prepared",
            Self::EffectSettled { .. } => "effect_settled",
            Self::ChildSpawned { .. } => "child_spawned",
            Self::ChildTerminal { .. } => "child_terminal",
            Self::WaitOpened { .. } => "wait_opened",
            Self::WaitResolved { .. } => "wait_resolved",
            Self::Compaction { .. } => "compaction",
            Self::AgentFinished { .. } => "agent_finished",
            Self::JoinOpened { .. } => "join_opened",
            Self::ChildStateChanged { .. } => "child_state_changed",
        }
    }

    /// The canonical bytes of this record.
    ///
    /// # Errors
    ///
    /// Returns [`crate::canonical::CanonicalizeError`] when the record carries a
    /// non-integer number, nests too deeply, or encodes too large.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, crate::canonical::CanonicalizeError> {
        crate::canonical::canonicalize_value(self)
    }

    /// The `blake3` digest over this record's canonical bytes.
    ///
    /// # Errors
    ///
    /// Identical to [`JournalRecord::canonical_bytes`].
    pub fn content_hash(&self) -> Result<ContentHash, crate::canonical::CanonicalizeError> {
        Ok(ContentHash::of(&self.canonical_bytes()?))
    }

    /// Whether this record's canonical body fits inside a journal item.
    ///
    /// # Errors
    ///
    /// Identical to [`JournalRecord::canonical_bytes`].
    pub fn fits_inline(&self) -> Result<bool, crate::canonical::CanonicalizeError> {
        Ok(self.canonical_bytes()?.len() <= INLINE_BODY_BYTES)
    }
}

/// One journal record with the envelope the authority sequenced it under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    /// Sequence, hash and record time.
    pub envelope: JournalEnvelope,
    /// The semantic fact.
    pub record: JournalRecord,
}

impl JournalEntry {
    /// Seals `record` at `seq` and `recorded_at`, computing the envelope hash.
    ///
    /// # Errors
    ///
    /// Returns [`crate::canonical::CanonicalizeError`] when the record cannot be
    /// canonicalized.
    pub fn seal(
        seq: JournalSeq,
        recorded_at: Timestamp,
        record: JournalRecord,
    ) -> Result<Self, crate::canonical::CanonicalizeError> {
        let content_hash = record.content_hash()?;
        Ok(Self {
            envelope: JournalEnvelope {
                seq,
                content_hash,
                recorded_at,
            },
            record,
        })
    }

    /// Whether the envelope hash actually covers the record.
    ///
    /// # Errors
    ///
    /// Returns [`crate::canonical::CanonicalizeError`] when the record cannot be
    /// canonicalized.
    pub fn hash_is_intact(&self) -> Result<bool, crate::canonical::CanonicalizeError> {
        Ok(self.record.content_hash()? == self.envelope.content_hash)
    }
}

/// Why a journal record could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JournalDecodeError {
    /// The bytes were not valid JSON, or not a shape this closed enum admits.
    #[error("journal body is not a known record: {reason}")]
    Malformed {
        /// The decoder's reason, redacted of any payload.
        reason: String,
    },
    /// The body was larger than a journal item may carry.
    #[error("journal body of {len} bytes exceeds the {limit} permitted inline")]
    TooLarge {
        /// The observed length.
        len: usize,
        /// The inline boundary.
        limit: usize,
    },
    /// The body decoded, but its re-encoding differs, so it was not canonical.
    #[error("journal body is not in canonical form")]
    NonCanonical,
}

/// Decodes one journal body from its stored bytes.
///
/// Bounded on purpose: the length is checked before any parse, so an oversized body costs
/// a comparison rather than an allocation. Re-canonicalizing and comparing rejects a body
/// that decodes but was not written in canonical form, which is how a second encoder would
/// be caught.
///
/// # Errors
///
/// Returns [`JournalDecodeError`] when the bytes are oversized, malformed, or non-canonical.
pub fn decode(bytes: &[u8]) -> Result<JournalRecord, JournalDecodeError> {
    if bytes.len() > INLINE_BODY_BYTES {
        return Err(JournalDecodeError::TooLarge {
            len: bytes.len(),
            limit: INLINE_BODY_BYTES,
        });
    }
    let record: JournalRecord =
        serde_json::from_slice(bytes).map_err(|error| JournalDecodeError::Malformed {
            reason: format!("{} at line {}", error.classify_text(), error.line()),
        })?;
    let recanonicalized =
        record
            .canonical_bytes()
            .map_err(|error| JournalDecodeError::Malformed {
                reason: error.to_string(),
            })?;
    if recanonicalized != bytes {
        return Err(JournalDecodeError::NonCanonical);
    }
    Ok(record)
}

trait ClassifyText {
    fn classify_text(&self) -> &'static str;
}

impl ClassifyText for serde_json::Error {
    /// Names the failure class without echoing any payload back into a log.
    fn classify_text(&self) -> &'static str {
        match self.classify() {
            serde_json::error::Category::Io => "io",
            serde_json::error::Category::Syntax => "syntax",
            serde_json::error::Category::Data => "unknown or mistyped field",
            serde_json::error::Category::Eof => "truncated",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FinishReason, INLINE_BODY_BYTES, JournalDecodeError, JournalEntry, JournalRecord, decode,
    };
    use crate::ids::{JournalSeq, Timestamp};

    fn finished() -> JournalRecord {
        JournalRecord::AgentFinished {
            reason: FinishReason::Completed,
            failure: None,
        }
    }

    #[test]
    fn a_record_round_trips_through_its_canonical_bytes() {
        let bytes = finished()
            .canonical_bytes()
            .expect("a record canonicalizes");
        assert_eq!(decode(&bytes).expect("canonical bytes decode"), finished());
    }

    #[test]
    fn an_unknown_kind_is_a_decode_error_not_a_passthrough() {
        let error =
            decode(br#"{"record":"telepathy_v2"}"#).expect_err("an unknown kind is rejected");
        assert!(
            matches!(error, JournalDecodeError::Malformed { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn an_unknown_field_inside_a_known_kind_is_rejected() {
        let error =
            decode(br#"{"extra":1,"failure":null,"reason":"completed","record":"agent_finished"}"#)
                .expect_err("an unknown field is rejected");
        assert!(
            matches!(error, JournalDecodeError::Malformed { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn a_non_canonical_encoding_is_rejected_even_when_it_parses() {
        let reordered = br#"{"reason":"completed","record":"agent_finished","failure":null}"#;
        assert_eq!(
            decode(reordered).expect_err("member order is part of the canonical form"),
            JournalDecodeError::NonCanonical
        );
    }

    #[test]
    fn an_oversized_body_is_refused_before_it_is_parsed() {
        let oversized = vec![b'{'; INLINE_BODY_BYTES + 1];
        assert_eq!(
            decode(&oversized).expect_err("an oversized body is refused"),
            JournalDecodeError::TooLarge {
                len: INLINE_BODY_BYTES + 1,
                limit: INLINE_BODY_BYTES,
            }
        );
    }

    #[test]
    fn a_sealed_entry_carries_a_hash_that_covers_its_record() {
        let entry = JournalEntry::seal(JournalSeq(3), Timestamp::from_millis(9), finished())
            .expect("a record seals");
        assert!(entry.hash_is_intact().expect("the hash recomputes"));
        let tampered = JournalEntry {
            record: JournalRecord::AgentFinished {
                reason: FinishReason::Failed,
                failure: None,
            },
            ..entry
        };
        assert!(!tampered.hash_is_intact().expect("the hash recomputes"));
    }

    #[test]
    fn every_variant_reports_a_distinct_kind_name() {
        // Deliberately exhaustive over the tag strings the store writes; a new variant
        // that forgets a name collides here rather than in a `DynamoDB` item.
        let names = [
            "agent_started",
            "user_message",
            "assistant_message",
            "tool_result",
            "effect_prepared",
            "effect_settled",
            "child_spawned",
            "child_terminal",
            "wait_opened",
            "wait_resolved",
            "compaction",
            "agent_finished",
            "join_opened",
            "child_state_changed",
        ];
        let mut sorted = names.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len());
        assert_eq!(finished().kind_name(), "agent_finished");
    }

    #[test]
    fn only_completed_reports_success() {
        assert!(FinishReason::Completed.is_success());
        for reason in [
            FinishReason::MaxTurns,
            FinishReason::MaxSteps,
            FinishReason::Budget,
            FinishReason::Timeout,
            FinishReason::Cancelled,
            FinishReason::Failed,
            FinishReason::Interrupted,
        ] {
            assert!(!reason.is_success(), "{reason:?}");
        }
    }
}
