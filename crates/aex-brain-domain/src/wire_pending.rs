//! Minimal stand-ins for types owned by peer streams.
//!
//! Every item here is the smallest shape the Brain fold actually needs. Nothing in this
//! module is a second authority, and nothing outside it may define one of these concepts
//! again.
//!
//! # State of the peers
//!
//! The peer streams have landed, and none of them landed the shape this module guessed at.
//! Every marker below therefore records one of two things, and no marker names a path that
//! does not resolve:
//!
//! - the peer published a *different* type under the concept's name, in which case the
//!   marker names the real path and says what diverged. Adopting it is a change to the
//!   fold, the journal encoding or both — not a rename; or
//! - the peer published no equivalent at all, in which case the marker names the owning
//!   crate in its dashed spelling and describes what is owed in prose. A dashed crate name
//!   is deliberately not a Rust path: it cannot be mistaken for something importable.
//!
//! `aex-brain-domain` also cannot depend on `aex-brain-tool-catalog`, which already depends
//! on this crate, so the one marker naming that direction is a cycle rather than a pending
//! delete.

use aex_wire::ids::GenerationId;
use serde::{Deserialize, Serialize};

use crate::ids::{
    AgentId, CatalogPin, ContentHash, JoinId, JournalSeq, ModelSlug, Timestamp, ToolCallId,
    ToolName,
};

/// A journal envelope.
///
/// `TODO(cross-stream)`: `aex-session-domain` publishes no journal *envelope*. It publishes
/// `aex_session_domain::journal::JournalEntry`, which carries the agent, the kind, a
/// content-derived `EntryIdentity`, a `JournalBody` and an `AuthorityFact` alongside the
/// sequence and instant this envelope keeps. Adopting it means the Brain fold reads the
/// peer's body/fact split rather than a bare content hash, so it is a fold change.
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
/// `TODO(cross-stream)`: `aex-model-catalog` does not define a provider id; it consumes
/// `aex_wire::provider::ProviderId`, which is the workspace's single closed set. This copy
/// exists only because `aex-brain-domain` does not yet depend on `aex-wire`. A closed set is
/// the point either way: `A11-PROVIDERS` allows no gateway, no `OpenRouter` and no arbitrary
/// base URL, so an open string would represent a provider that cannot exist.
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
/// `TODO(cross-stream)`: `aex-content-domain` publishes no `ContentRef`. Its nearest type is
/// `aex_content_domain::descriptor::ContentDescriptor`, which is workspace-scoped and carries
/// a `Placement` and a `CiphertextIdentity` rather than an opaque key plus an encryption
/// label. Adopting it makes every reference workspace-bound, which is a journal-encoding
/// change.
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
/// `TODO(cross-stream)`: `aex_model_catalog::canonical::CanonicalBlock` exists and is a
/// different set. It has `Reasoning(ReasoningBlock)` in place of `Thinking`, a `Refusal`
/// arm, `BoundedString`/`CanonicalJson` payloads in place of `String`/`serde_json::Value`,
/// and **no** `Image` arm. Adopting it changes what the fold can represent, so it is not a
/// substitution.
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
/// `TODO(cross-stream)`: `aex_model_catalog::canonical::NormalizedUsage` exists with
/// different fields — `cache_read_input_tokens`/`cache_write_input_tokens` rather than
/// `cache_read_tokens`/`cache_creation_tokens`, plus `tool_use_prompt_tokens`,
/// `provider_total_tokens` and a `UsageCompleteness`. Its `reasoning_tokens` is a subset of
/// `output_tokens`; here it is a sibling. Adopting it changes what `prompt_tokens` sums.
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
/// `TODO(cross-stream)`: `aex_model_catalog::canonical::StopReason` exists and spells the
/// truncating arm `MaxOutputTokens`. Adopting it is a wire rename in every serialized
/// journal record that carries a stop reason.
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
/// `TODO(cross-stream)`: `aex_model_catalog::canonical::CompleteAssistantMessage` exists and
/// additionally binds the provider, the model and the catalog revision into the message, and
/// its proof is a `CompleteProof(ContentHash)` minted only by `canonical::seal`. Adopting it
/// moves proof minting out of this crate.
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
/// `TODO(cross-stream)`: `aex_model_catalog::canonical::CanonicalModelRequest` exists and is
/// far wider — a `QualifiedModel` selection, `CanonicalMessage` history, `CanonicalToolDef`
/// declarations, tool choice, sampling in integer milli-units, reasoning and structured-output
/// requests, cache breakpoints and a correlation handle. Adopting it is a planner change.
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
/// `TODO(cross-stream)`: `aex_model_catalog::canonical::ProviderReceipt` exists and carries
/// the dialect, dialect revision, credential binding, HTTP status, attempt count and timing
/// besides the provider, model and request id kept here, and it identifies the catalog by
/// `CatalogRevision` rather than by a `u64`. Adopting it is a receipt-encoding change.
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
/// `TODO(cross-stream)`: `aex_model_catalog::document::DurableOperationSupport` exists with
/// three arms — `None`, `ResultLookup { ttl_ms }` and `ResumableStream { .. }` — where this
/// has two. The `Proven` arm here does not exist there; the peer states *which* durable
/// operation exists rather than that one does.
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
/// `TODO(cross-stream)`: `aex-model-catalog` publishes no `ModelCapability`. The catalog
/// describes a model with `document::ModelEntry`, whose limits live in `document::ModelLimits`
/// and whose capability bits live in `document::CapabilitySet`, and whose admissibility is an
/// `document::EntryState` plus a live `ConformanceReceipt` rather than a `bool`.
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
/// `TODO(cross-stream)`: `aex-brain-tool-catalog` publishes a `ToolManifestEntry`, but it
/// already depends on this crate, so importing it here would be a dependency cycle. The
/// concept has to move down into `aex-brain-domain` or up into a third crate; it cannot be
/// deleted in place.
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
    /// The canonical Hands generation every agent in the session shares.
    pub hands_generation: GenerationId,
    /// Per-agent run limits.
    pub limits: AgentLimits,
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
