//! Non-destructive rolling context checkpoints.
//!
//! A checkpoint is an immutable, verified fold baseline. It may replace the
//! model-visible history in the baseline with a compact summary, but it never
//! rewrites or deletes the journal it covers. A successor applies only entries
//! after `covers_through` to this baseline.

use serde::{Deserialize, Serialize};

use crate::fold::FoldState;
use crate::ids::{AgentKey, ContentHash, JournalSeq, Timestamp};

/// Current closed checkpoint object schema.
pub const CHECKPOINT_SCHEMA_VERSION: u16 = 1;
/// Hard maximum for one decoded checkpoint object.
pub const MAX_CHECKPOINT_BYTES: u64 = 8 * 1_024 * 1_024;

/// Small committed pointer stored on the agent control item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CheckpointMetadata {
    /// Closed object schema.
    pub schema_version: u16,
    /// Deterministic checkpoint identity.
    pub id: ContentHash,
    /// Prior committed checkpoint, retained for audit/deletion only.
    pub previous: Option<ContentHash>,
    /// Last journal entry represented by the baseline.
    pub covers_through: JournalSeq,
    /// Exact hash of `covers_through`.
    pub covers_hash: ContentHash,
    /// Immutable S3 body identity.
    pub object_hash: ContentHash,
    /// Exact immutable S3 body length.
    pub object_bytes: u64,
    /// Hash of the uncompacted source fold.
    pub source_hash: ContentHash,
    /// Approximate prompt tokens in the compacted baseline.
    pub approximate_tokens: u64,
    /// Closed compactor implementation identity.
    pub compactor: String,
    /// When the object was produced.
    pub created_at: Timestamp,
}

/// Closed encrypted S3 body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContextCheckpoint {
    /// Closed object schema.
    pub schema_version: u16,
    /// Agent authority this object belongs to.
    pub key: AgentKey,
    /// Deterministic checkpoint identity.
    pub id: ContentHash,
    /// Prior committed checkpoint.
    pub previous: Option<ContentHash>,
    /// Last represented journal sequence.
    pub covers_through: JournalSeq,
    /// Exact source journal tail hash.
    pub covers_hash: ContentHash,
    /// Hash of the uncompacted source fold.
    pub source_hash: ContentHash,
    /// Approximate tokens in `state.model_history`.
    pub approximate_tokens: u64,
    /// Closed compactor implementation identity.
    pub compactor: String,
    /// Object creation time.
    pub created_at: Timestamp,
    /// Complete execution baseline with a compacted model-visible history.
    pub state: FoldState,
}

impl ContextCheckpoint {
    /// Validates the body against its committed pointer and requested agent.
    #[must_use]
    pub fn matches(&self, key: AgentKey, metadata: &CheckpointMetadata) -> bool {
        self.schema_version == CHECKPOINT_SCHEMA_VERSION
            && metadata.schema_version == CHECKPOINT_SCHEMA_VERSION
            && self.key == key
            && self.id == metadata.id
            && self.previous == metadata.previous
            && self.covers_through == metadata.covers_through
            && self.covers_hash == metadata.covers_hash
            && self.source_hash == metadata.source_hash
            && self.approximate_tokens == metadata.approximate_tokens
            && self.compactor == metadata.compactor
            && self.created_at == metadata.created_at
            && self.state.tail == Some(metadata.covers_through)
            && self.state.base_seq == metadata.covers_through.next()
            && self.state.hashes.is_empty()
    }
}
