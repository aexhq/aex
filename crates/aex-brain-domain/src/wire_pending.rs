//! Cross-stream vocabulary still awaiting a lower owning crate.
//!
//! Provider/model concepts are intentionally re-exported from their canonical
//! lower authorities. This module retains only Brain journal/configuration
//! shapes that do not yet have an importable owner.

use aex_wire::ids::{GenerationId, ProviderCredentialId};
use core::num::NonZeroU64;
use serde::{Deserialize, Serialize};

use crate::effect::EffectClass;
use crate::ids::{
    AgentId, CatalogPin, ContentHash, JoinId, JournalSeq, ModelSlug, Timestamp, ToolName,
};

pub use aex_model_catalog::QualifiedModel;
pub use aex_model_catalog::canonical::{
    CanonicalBlock, CanonicalMessage, CanonicalModelRequest, CompleteAssistantMessage,
    CompleteProof, NormalizedUsage, PreviewFrame, Role, StopReason, ToolResultPart,
};
pub use aex_model_catalog::document::DurableOperationSupport;
pub use aex_wire::provider::ProviderId;

/// A Brain journal envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEnvelope {
    /// Contiguous position in the owning agent's journal.
    pub seq: JournalSeq,
    /// Digest over the canonical body.
    pub content_hash: ContentHash,
    /// When the authority accepted the record.
    pub recorded_at: Timestamp,
}

/// A large immutable body held by the regional content authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentRef {
    /// Digest over the plaintext bytes.
    pub hash: ContentHash,
    /// Plaintext length in bytes.
    pub len: u64,
    /// IANA media type.
    pub media_type: String,
    /// Immutable content-addressed key.
    pub key: String,
    /// The encryption context the object was written under.
    pub encryption: String,
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

/// What the signed tool catalog says about one tool.
///
/// This remains here because `aex-brain-tool-catalog` already depends on the
/// domain crate; importing its manifest entry would create a cycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolManifestEntry {
    /// The tool name.
    pub name: ToolName,
    /// Which recovery contract the tool declares.
    pub class: EffectClass,
    /// The wall-clock ceiling for one invocation, in milliseconds.
    pub timeout_ms: u32,
    /// Whether the tool runs inside the session's Hands generation.
    pub runs_on_hands: bool,
}

/// The resolved, pinned configuration one agent runs under for its whole life.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedAgentConfig {
    /// The signed model catalog revision every model lookup resolves against.
    pub catalog_pin: CatalogPin,
    /// The provider the agent's model calls are bound to.
    pub provider: ProviderId,
    /// The exact provider credential admitted with the session.
    ///
    /// This is immutable journal state. Runtime dispatch must never resolve a
    /// mutable workspace default in its place.
    pub credential: SessionCredentialPin,
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

/// The immutable provider-credential authority admitted with a session.
///
/// The four scalar fields are normalized here rather than importing secret
/// storage types into Brain. The gateway converts them at its custody boundary.
/// Keeping this value `Copy` also means dispatch adds no allocation or secret
/// material to the hot path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionCredentialPin {
    /// The stable `pcr_` binding identity.
    pub binding: ProviderCredentialId,
    /// The immutable binding revision.
    pub revision: NonZeroU64,
    /// The workspace-secret source generation.
    pub generation: NonZeroU64,
    /// The secret revocation epoch observed at session admission.
    pub revocation_epoch: u64,
}

impl SessionCredentialPin {
    /// Builds a pin, refusing zero revision or source generation.
    #[must_use]
    pub const fn new(
        binding: ProviderCredentialId,
        revision: u64,
        generation: u64,
        revocation_epoch: u64,
    ) -> Option<Self> {
        let Some(revision) = NonZeroU64::new(revision) else {
            return None;
        };
        let Some(generation) = NonZeroU64::new(generation) else {
            return None;
        };
        Some(Self {
            binding,
            revision,
            generation,
            revocation_epoch,
        })
    }
}

/// Per-agent structural limits carried by the pinned config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentLimits {
    /// Maximum assistant turns before the planner finishes `MaxTurns`.
    pub max_turns: u32,
    /// Maximum planner steps inside one turn before the planner finishes.
    pub max_steps_per_turn: u32,
    /// Wall-clock ceiling for one turn, in milliseconds.
    pub turn_deadline_ms: u32,
}

/// Membership and mode of one join group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinGroup {
    /// The join identity.
    pub join: JoinId,
    /// Whether the parent resumes on the first terminal child or all children.
    pub mode: JoinMode,
    /// Every child the parent is waiting for.
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
#[must_use]
pub fn join_shards(members: usize) -> u16 {
    let target = (members / 8).max(1);
    let power = target.next_power_of_two();
    u16::try_from(power.clamp(1, 64)).unwrap_or(64)
}

#[cfg(test)]
mod tests {
    use super::join_shards;

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
