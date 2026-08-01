//! Minimal stand-ins for types owned by peer streams that have not landed yet.
//!
//! Every item here is the smallest shape the Brain fold actually needs. At merge each is
//! deleted and replaced by the peer's definition; nothing in this module is a second
//! authority, and nothing outside it may define one of these concepts again.

use serde::{Deserialize, Serialize};

use crate::ids::{
    AgentId, CatalogPin, ContentHash, JoinId, JournalSeq, ModelSlug, Timestamp, ToolCallId,
    ToolName,
};

/// A journal envelope.
///
/// `TODO(cross-stream): replaced by aex_session_domain::journal::JournalEnvelope at merge.`
/// `aex-session-domain` owns the envelope, the ordering algebra and the authority fold;
/// this crate owns payload interpretation and effect receipts and consumes those types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEnvelope {
    /// Contiguous position in the owning agent's journal.
    pub seq: JournalSeq,
    /// `blake3` over the canonical body.
    pub content_hash: ContentHash,
    /// When the authority accepted the record.
    pub recorded_at: Timestamp,
}

/// The six admitted `BYOK` providers.
///
/// `TODO(cross-stream): replaced by aex_model_catalog::ProviderId at merge.` A closed set
/// is the point: `A11-PROVIDERS` allows no gateway, no `OpenRouter` and no arbitrary base
/// URL, so an open string would represent a provider that cannot exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderId {
    /// `api.anthropic.com`.
    Anthropic,
    /// `api.deepseek.com`.
    Deepseek,
    /// Google Generative Language.
    Google,
    /// `api.moonshot.ai`.
    Moonshotai,
    /// `api.openai.com`.
    Openai,
    /// `api.z.ai`.
    Zai,
}

/// A large immutable body held by the regional content authority.
///
/// `TODO(cross-stream): replaced by aex_content_domain::ContentRef at merge.`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentRef {
    /// `blake3` over the plaintext bytes.
    pub hash: ContentHash,
    /// Plaintext length in bytes.
    pub len: u64,
    /// IANA media type.
    pub media_type: String,
    /// Immutable content-addressed key inside the content authority.
    pub key: String,
    /// The encryption context the object was written under.
    pub encryption: String,
}

/// A block of model-visible content.
///
/// `TODO(cross-stream): replaced by aex_model_catalog::canonical::CanonicalBlock at merge.`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CanonicalBlock {
    /// Plain assistant or user text.
    Text {
        /// The text.
        text: String,
    },
    /// A reasoning block, with the provider signature when one was supplied.
    Thinking {
        /// The reasoning text.
        text: String,
        /// Provider signature over the reasoning block, when the provider signs it.
        signature: Option<String>,
    },
    /// A tool the model asked to run.
    ToolUse {
        /// The call identity the provider minted.
        id: ToolCallId,
        /// Tool name from the pinned manifest.
        name: ToolName,
        /// Canonical tool input.
        input: serde_json::Value,
    },
    /// The result of a tool the model asked to run.
    ToolResult {
        /// The call this result answers.
        call: ToolCallId,
        /// Result content.
        content: Vec<ResultContent>,
        /// Whether the tool reported failure.
        is_error: bool,
    },
    /// An image referenced by content hash.
    Image {
        /// IANA media type.
        media_type: String,
        /// Where the bytes live.
        content: ContentRef,
    },
}

/// Content inside a tool result. Deliberately not recursive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResultContent {
    /// Result text.
    Text {
        /// The text.
        text: String,
    },
    /// A result image referenced by content hash.
    Image {
        /// IANA media type.
        media_type: String,
        /// Where the bytes live.
        content: ContentRef,
    },
}

/// A content block that may live inline or in the content authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "placement", rename_all = "snake_case")]
pub enum ContentBlockRef {
    /// The block is small enough to live in the journal item.
    Inline {
        /// The block.
        block: CanonicalBlock,
    },
    /// The block exceeded the inline boundary and was placed first.
    Placed {
        /// Where the canonical block bytes live.
        body: ContentRef,
    },
}

/// Provider-reported token usage, normalized across dialects.
///
/// `TODO(cross-stream): replaced by aex_model_catalog::canonical::NormalizedUsage at merge.`
/// Under `BYOK` these are zero-dollar observability facts (`A11`), which is why nothing
/// here is money.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NormalizedUsage {
    /// Input tokens billed by the provider.
    pub input_tokens: u64,
    /// Output tokens billed by the provider.
    pub output_tokens: u64,
    /// Tokens written into the provider's prompt cache.
    pub cache_creation_tokens: u64,
    /// Tokens served from the provider's prompt cache.
    pub cache_read_tokens: u64,
    /// Reasoning tokens, where the provider reports them separately.
    pub reasoning_tokens: u64,
}

impl NormalizedUsage {
    /// The total prompt size the context policy triggers on.
    ///
    /// Cache creation and cache read both occupy the window, so a policy that triggers on
    /// input tokens alone under-counts a cached prompt and never compacts it.
    #[must_use]
    pub const fn prompt_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.cache_creation_tokens)
            .saturating_add(self.cache_read_tokens)
    }

    /// Accumulates `other` into this total.
    pub const fn add(&mut self, other: Self) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cache_creation_tokens = self
            .cache_creation_tokens
            .saturating_add(other.cache_creation_tokens);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(other.cache_read_tokens);
        self.reasoning_tokens = self.reasoning_tokens.saturating_add(other.reasoning_tokens);
    }
}

/// Why the provider stopped generating.
///
/// `TODO(cross-stream): replaced by aex_model_catalog::canonical::StopReason at merge.`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The model finished its turn.
    EndTurn,
    /// The model asked for one or more tools.
    ToolUse,
    /// A stop sequence matched.
    StopSequence,
    /// Generation was cut off by the output-token limit.
    MaxTokens,
    /// The provider refused.
    Refusal,
}

impl StopReason {
    /// Whether this stop reason means the assistant message is whole.
    ///
    /// `MaxTokens` is not terminal: a cut-off deliverable reported as success is a
    /// correctness bug, which is why the planner finishes such a turn `Failed`.
    #[must_use]
    pub const fn is_complete(self) -> bool {
        matches!(
            self,
            Self::EndTurn | Self::ToolUse | Self::StopSequence | Self::Refusal
        )
    }

    /// Whether the provider truncated the message.
    #[must_use]
    pub const fn is_truncating(self) -> bool {
        matches!(self, Self::MaxTokens)
    }
}

/// Proof that an assistant message is whole.
///
/// Only complete messages enter model-visible history. The proof can be minted only from
/// a terminal stop reason over a validated block set, and there is no `From`/`Into` from a
/// [`PreviewFrame`] into anything that carries one, so a partial stream is structurally
/// unable to become history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompleteProof {
    stop_reason: StopReason,
    block_count: u32,
    blocks_hash: ContentHash,
}

/// Why an assistant message could not be proved complete.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IncompleteMessage {
    /// The provider stopped without a terminal reason.
    #[error("stop reason {reason:?} is not terminal, so the message is not complete")]
    NotTerminal {
        /// The non-terminal reason observed.
        reason: StopReason,
    },
    /// The block set was empty.
    #[error("a complete assistant message carries at least one block")]
    NoBlocks,
    /// A block could not be canonicalized.
    #[error("blocks are not canonicalizable: {reason}")]
    NotCanonical {
        /// Why canonicalization failed.
        reason: String,
    },
}

impl CompleteProof {
    /// Mints a proof over `blocks`.
    ///
    /// # Errors
    ///
    /// Returns [`IncompleteMessage`] when the stop reason is not terminal, the block set
    /// is empty, or the blocks cannot be canonicalized.
    pub fn mint(
        stop_reason: StopReason,
        blocks: &[CanonicalBlock],
    ) -> Result<Self, IncompleteMessage> {
        if !stop_reason.is_complete() {
            return Err(IncompleteMessage::NotTerminal {
                reason: stop_reason,
            });
        }
        if blocks.is_empty() {
            return Err(IncompleteMessage::NoBlocks);
        }
        let bytes = crate::canonical::canonicalize_value(&blocks).map_err(|error| {
            IncompleteMessage::NotCanonical {
                reason: error.to_string(),
            }
        })?;
        Ok(Self {
            stop_reason,
            block_count: u32::try_from(blocks.len()).unwrap_or(u32::MAX),
            blocks_hash: ContentHash::of(&bytes),
        })
    }

    /// Whether this proof describes exactly `blocks` under `stop_reason`.
    #[must_use]
    pub fn covers(&self, stop_reason: StopReason, blocks: &[CanonicalBlock]) -> bool {
        Self::mint(stop_reason, blocks).is_ok_and(|minted| minted == *self)
    }

    /// The terminal stop reason this proof was minted under.
    #[must_use]
    pub const fn stop_reason(&self) -> StopReason {
        self.stop_reason
    }

    /// How many blocks the proved message carries.
    #[must_use]
    pub const fn block_count(&self) -> u32 {
        self.block_count
    }
}

/// A provider-neutral complete assistant message.
///
/// `TODO(cross-stream): replaced by aex_model_catalog::canonical::CompleteAssistantMessage
/// at merge.`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompleteAssistantMessage {
    /// The whole block set.
    pub blocks: Vec<CanonicalBlock>,
    /// Why generation stopped.
    pub stop_reason: StopReason,
    /// Proof the message is whole.
    pub complete: CompleteProof,
}

/// A provider-neutral model request.
///
/// `TODO(cross-stream): replaced by aex_model_catalog::canonical::CanonicalModelRequest at
/// merge.`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalModelRequest {
    /// The provider the request is bound to.
    pub provider: ProviderId,
    /// The exact model slug within that provider.
    pub model: ModelSlug,
    /// System instruction reference.
    pub system: Option<ContentRef>,
    /// Ordered model-visible turns.
    pub turns: Vec<Turn>,
    /// Tool names admitted for this request.
    pub tools: Vec<ToolName>,
    /// Output-token ceiling for this request.
    pub max_output_tokens: u32,
}

/// One model-visible turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Turn {
    /// A user turn, including reconstructed tool-result turns.
    User {
        /// The blocks of the turn.
        blocks: Vec<CanonicalBlock>,
    },
    /// A complete assistant turn.
    Assistant {
        /// The blocks of the turn.
        blocks: Vec<CanonicalBlock>,
        /// The provider that actually served it.
        provider: ProviderId,
        /// The model that actually served it.
        model: ModelSlug,
        /// Why generation stopped.
        stop_reason: StopReason,
    },
}

/// A non-authoritative client preview frame.
///
/// There is deliberately no conversion from this type into any journal variant: the fold
/// takes journal records only, so a partial delta cannot become model-visible history by
/// any code path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewFrame {
    /// Which journal record the frame belongs to.
    pub journal_seq: JournalSeq,
    /// Ordered slot inside that record's 1 024-wide sub-slot space.
    pub sub_slot: u16,
    /// The frame payload as the adapter produced it.
    pub payload: String,
}

/// What the provider returned about the request itself.
///
/// `TODO(cross-stream): replaced by aex_model_catalog::canonical::ProviderReceipt at merge.`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderReceipt {
    /// The provider that actually served the request, which may differ from the requested
    /// one when the catalog declared the two equivalent and the effect was still
    /// `Prepared`.
    pub provider: ProviderId,
    /// The model that actually served it.
    pub model: ModelSlug,
    /// The provider's own request id, preserved verbatim.
    pub request_id: Option<crate::ids::ProviderRequestId>,
    /// The catalog revision the route was resolved under.
    pub route_revision: u64,
}

/// What the catalog says about resuming a dispatched effect.
///
/// `TODO(cross-stream): replaced by aex_model_catalog::DurableOperationSupport at merge.`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DurableOperationSupport {
    /// No durable operation or result lookup exists. Every launch model is here.
    None,
    /// A durable operation exists and has a live conformance receipt.
    Proven,
}

/// What the catalog says about one model.
///
/// `TODO(cross-stream): replaced by aex_model_catalog::ModelCapability at merge.`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCapability {
    /// The provider the model belongs to.
    pub provider: ProviderId,
    /// The exact slug.
    pub model: ModelSlug,
    /// Maximum total context window in tokens.
    pub context_window_tokens: u64,
    /// Maximum output tokens per request.
    pub max_output_tokens: u32,
    /// The smallest stable prefix the provider will cache, in tokens. `None` when the
    /// provider does not cache prompts.
    pub min_cacheable_prefix_tokens: Option<u64>,
    /// Whether the model accepts tools.
    pub supports_tools: bool,
    /// Whether a live conformance receipt admitted this entry.
    pub admitted: bool,
}

/// What the catalog says about one tool.
///
/// `TODO(cross-stream): replaced by aex_brain_tool_catalog::ToolManifestEntry at merge.`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolManifestEntry {
    /// The tool name.
    pub name: ToolName,
    /// Which recovery contract the tool declares.
    pub class: crate::effect::EffectClass,
    /// The wall-clock ceiling for one invocation, in milliseconds.
    pub timeout_ms: u32,
    /// Whether the tool runs inside the session's Hands generation.
    pub runs_on_hands: bool,
}

/// The resolved, pinned configuration one agent runs under for its whole life.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedAgentConfig {
    /// The signed catalog artifact every capability lookup resolves against.
    pub catalog_pin: CatalogPin,
    /// The provider the agent's model calls are bound to.
    pub provider: ProviderId,
    /// The exact model slug.
    pub model: ModelSlug,
    /// System instruction reference, when one is configured.
    pub system: Option<ContentRef>,
    /// Digests of the admitted tool manifests.
    pub tool_manifest_digests: Vec<ContentHash>,
    /// The Hands generation every agent in the session shares.
    pub hands_generation: HandsGenerationRef,
    /// Per-agent run limits.
    pub limits: AgentLimits,
}

/// The Hands generation an agent inherits, and whether one exists yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandsGenerationRef {
    /// No Hands `MicroVM` has been required yet.
    Unbound,
    /// The exact generation this agent's Hands work is fenced to.
    Bound(crate::ids::HandsGeneration),
}

/// Per-agent structural limits carried by the pinned config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentLimits {
    /// Maximum assistant turns before the planner finishes `MaxTurns`.
    pub max_turns: u32,
    /// Maximum planner steps inside one turn before it finishes `MaxSteps`.
    pub max_steps_per_turn: u32,
    /// Wall-clock ceiling for one turn, in milliseconds.
    pub turn_deadline_ms: u32,
}

/// Membership and mode of one join group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinGroup {
    /// The join identity.
    pub join: JoinId,
    /// Whether the parent resumes on the first terminal child or on all of them.
    pub mode: JoinMode,
    /// Every child the parent is waiting on.
    pub members: Vec<AgentId>,
    /// Children observed terminal so far.
    pub done: Vec<AgentId>,
    /// The shard count chosen at join creation.
    pub shards: u16,
}

/// Whether a join releases on the first terminal child or on all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinMode {
    /// Release on the first terminal member.
    Any,
    /// Release when every member is terminal.
    All,
}

/// The shard count for a join over `members` children.
///
/// Measured basis: 1 000 leaves on one counter produced 885 first-pass conflicts and 64
/// shards produced 191. Small joins must not pay a fixed 64-shard tax.
#[must_use]
pub fn join_shards(members: usize) -> u16 {
    let target = (members / 8).max(1);
    let power = target.next_power_of_two();
    u16::try_from(power.clamp(1, 64)).unwrap_or(64)
}

#[cfg(test)]
mod tests {
    use super::{
        CanonicalBlock, CompleteProof, IncompleteMessage, NormalizedUsage, StopReason, join_shards,
    };

    fn blocks() -> Vec<CanonicalBlock> {
        vec![CanonicalBlock::Text {
            text: "done".to_owned(),
        }]
    }

    #[test]
    fn a_truncating_stop_reason_cannot_mint_a_completeness_proof() {
        let error = CompleteProof::mint(StopReason::MaxTokens, &blocks())
            .expect_err("max_tokens is not terminal");
        assert_eq!(
            error,
            IncompleteMessage::NotTerminal {
                reason: StopReason::MaxTokens
            }
        );
    }

    #[test]
    fn an_empty_block_set_cannot_mint_a_completeness_proof() {
        assert_eq!(
            CompleteProof::mint(StopReason::EndTurn, &[]).expect_err("no blocks"),
            IncompleteMessage::NoBlocks
        );
    }

    #[test]
    fn a_proof_only_covers_the_blocks_it_was_minted_over() {
        let proof = CompleteProof::mint(StopReason::EndTurn, &blocks()).expect("a whole message");
        assert!(proof.covers(StopReason::EndTurn, &blocks()));
        assert!(!proof.covers(StopReason::ToolUse, &blocks()));
        assert!(!proof.covers(
            StopReason::EndTurn,
            &[CanonicalBlock::Text {
                text: "tampered".to_owned()
            }]
        ));
    }

    #[test]
    fn prompt_tokens_count_the_whole_window_including_cache() {
        let usage = NormalizedUsage {
            input_tokens: 10,
            output_tokens: 100,
            cache_creation_tokens: 5,
            cache_read_tokens: 7,
            reasoning_tokens: 3,
        };
        assert_eq!(usage.prompt_tokens(), 22);
    }

    #[test]
    fn join_shards_follow_the_measured_conflict_curve() {
        assert_eq!(join_shards(0), 1);
        assert_eq!(join_shards(1), 1);
        assert_eq!(join_shards(8), 1);
        assert_eq!(join_shards(16), 2);
        assert_eq!(join_shards(100), 16);
        assert_eq!(join_shards(1_000), 64);
        assert_eq!(join_shards(100_000), 64);
    }
}
