//! Typed identifiers.
//!
//! Every identifier in the Brain is a newtype. A bag of `Uuid`s and `u64`s would let a
//! fence be compared against a revision, or a child agent id be passed where a parent is
//! expected, and neither mistake would be visible at a call site.

use core::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A regional session. Every agent, budget item and Hands generation hangs off one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub Uuid);

/// One agent: the durable serialization unit that owns exactly one journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentId(pub Uuid);

/// The pair that names one journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AgentKey {
    /// Owning session.
    pub session: SessionId,
    /// Agent within that session.
    pub agent: AgentId,
}

impl AgentKey {
    /// The key for `agent` inside `session`.
    #[must_use]
    pub const fn new(session: SessionId, agent: AgentId) -> Self {
        Self { session, agent }
    }
}

/// Position in one agent's contiguous journal.
///
/// Contiguity is the point: a gap is a typed error, never a fold. `seq > last_seq` — the
/// rule the TypeScript kernel used — lets a torn read fold silently.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct JournalSeq(pub u64);

impl JournalSeq {
    /// The first sequence number of every journal.
    pub const ZERO: Self = Self(0);

    /// The next position.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }

    /// The raw position.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for JournalSeq {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// Monotonic ownership token on the agent control item.
///
/// Increments **only** when a new owner claims. A renewal must not perturb it, because
/// the fence is what rejects a stale writer and a renewal proves nothing changed hands.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Fence(pub u64);

impl Fence {
    /// The fence of an agent nobody has ever claimed.
    pub const ZERO: Self = Self(0);

    /// The fence a new owner takes.
    #[must_use]
    pub const fn advance(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

/// Monotonic commit counter on the agent control item. Increments on every commit.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct AgentRevision(pub u64);

impl AgentRevision {
    /// The revision of an agent that has never committed.
    pub const ZERO: Self = Self(0);

    /// The revision the next commit writes.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

/// A fresh token minted per claim attempt.
///
/// The activation-pool spike found that reusing one owner token across attempts is a
/// correctness bug: two attempts by the same task become indistinguishable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OwnerToken(pub Uuid);

/// Session-wide cancellation generation. Advancing it fences every future effect.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct CancelEpoch(pub u64);

impl CancelEpoch {
    /// The epoch of a session nobody has cancelled.
    pub const ZERO: Self = Self(0);

    /// The epoch a cancellation takes.
    #[must_use]
    pub const fn advance(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

/// One `wait_subagents` group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JoinId(pub Uuid);

/// One durable wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WaitId(pub Uuid);

/// One durable wake item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WakeId(pub Uuid);

/// A paged fanout intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FanoutIntentId(pub Uuid);

/// Epoch milliseconds, wall clock. Never read inside this crate; always a parameter.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Timestamp(pub i64);

impl Timestamp {
    /// The instant `millis` milliseconds after the Unix epoch.
    #[must_use]
    pub const fn from_millis(millis: i64) -> Self {
        Self(millis)
    }

    /// Milliseconds since the Unix epoch.
    #[must_use]
    pub const fn millis(self) -> i64 {
        self.0
    }

    /// This instant moved forward by `millis`.
    #[must_use]
    pub const fn plus_millis(self, millis: i64) -> Self {
        Self(self.0.saturating_add(millis))
    }
}

/// A `blake3` digest over a canonical encoding.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContentHash(pub [u8; 32]);

impl ContentHash {
    /// The digest of `bytes`.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    /// Lowercase hexadecimal, for sort keys and diagnostics.
    #[must_use]
    pub fn to_hex(self) -> String {
        hex::encode(self.0)
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ContentHash({})", self.to_hex())
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

/// A tool call identifier as the provider emitted it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolCallId(pub String);

impl ToolCallId {
    /// Borrows the underlying string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A tool name from the catalog manifest.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolName(pub String);

/// A model slug within one provider.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ModelSlug(pub String);

/// The signed catalog artifact an agent is pinned to for its whole life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CatalogPin(pub ContentHash);

/// A durable operation identity a detached tool or MCP Task returned.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DetachedOperationId(pub String);

/// One Hands operation inside a generation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HandsOperationId(pub String);

/// A provider request id, preserved verbatim for support and reconciliation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderRequestId(pub String);

/// A caller-supplied idempotency key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IdempotencyKey(pub String);

/// One `regional-work` due shard.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct WorkShard(pub u16);

/// Deterministic effect identity: `blake3(agent || seq || kind_tag)[..16]`.
///
/// A redelivered wake that replans the same owed step derives the same id, so the durable
/// record collapses the duplicate without any extra state.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EffectId(pub [u8; 16]);

impl EffectId {
    /// Derives the identity of the effect `kind` prepared by `agent` at `seq`.
    #[must_use]
    pub fn derive(agent: AgentId, seq: JournalSeq, kind_tag: u8) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(agent.0.as_bytes());
        hasher.update(&seq.get().to_be_bytes());
        hasher.update(&[kind_tag]);
        let digest = hasher.finalize();
        let mut bytes = [0_u8; 16];
        bytes.copy_from_slice(&digest.as_bytes()[..16]);
        Self(bytes)
    }

    /// Lowercase hexadecimal, for sort keys and diagnostics.
    #[must_use]
    pub fn to_hex(self) -> String {
        hex::encode(self.0)
    }
}

impl fmt::Debug for EffectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "EffectId({})", self.to_hex())
    }
}

impl fmt::Display for EffectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

/// Derives the identity of the `ordinal`-th child of `parent`.
///
/// Deterministic so a retried fanout page creates the same ids and is therefore
/// idempotent without a second durable record.
#[must_use]
pub fn child_agent_id(parent: AgentId, ordinal: u32) -> AgentId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(parent.0.as_bytes());
    hasher.update(&ordinal.to_be_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    // A version-7 layout keeps child ids sortable next to every other agent id; the
    // timestamp field is derived, not read from a clock, because this crate has none.
    bytes[6] = (bytes[6] & 0x0F) | 0x70;
    bytes[8] = (bytes[8] & 0x3F) | 0x80;
    AgentId(Uuid::from_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::{AgentId, ContentHash, EffectId, Fence, JournalSeq, child_agent_id};
    use uuid::Uuid;

    fn agent(seed: u128) -> AgentId {
        AgentId(Uuid::from_u128(seed))
    }

    #[test]
    fn effect_identity_is_a_function_of_agent_sequence_and_kind() {
        let a = EffectId::derive(agent(1), JournalSeq(7), 3);
        assert_eq!(a, EffectId::derive(agent(1), JournalSeq(7), 3));
        assert_ne!(a, EffectId::derive(agent(2), JournalSeq(7), 3));
        assert_ne!(a, EffectId::derive(agent(1), JournalSeq(8), 3));
        assert_ne!(a, EffectId::derive(agent(1), JournalSeq(7), 4));
    }

    #[test]
    fn child_identity_is_a_function_of_parent_and_ordinal() {
        let first = child_agent_id(agent(9), 0);
        assert_eq!(first, child_agent_id(agent(9), 0));
        assert_ne!(first, child_agent_id(agent(9), 1));
        assert_ne!(first, child_agent_id(agent(8), 0));
        assert_eq!(first.0.get_version_num(), 7, "{first:?}");
    }

    #[test]
    fn a_fence_advances_and_never_wraps() {
        assert_eq!(Fence::ZERO.advance(), Fence(1));
        assert_eq!(Fence(u64::MAX).advance(), Fence(u64::MAX));
    }

    #[test]
    fn a_content_hash_round_trips_through_hex() {
        let hash = ContentHash::of(b"brain");
        assert_eq!(hash.to_hex().len(), 64);
        assert_eq!(hash, ContentHash::of(b"brain"));
        assert_ne!(hash, ContentHash::of(b"brainy"));
    }
}
