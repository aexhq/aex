//! Content-addressed, tail-bound snapshots of [`FoldState`](crate::fold::FoldState).
//!
//! A snapshot is an acceleration structure, never a second journal. Its body binds the
//! exact agent and absorbed journal point, and its pointer repeats those facts beside a
//! SHA-256 digest and exact byte length. Restore verifies both representations before the
//! state is allowed to reach the planner.

use serde::{Deserialize, Serialize};

use crate::fold::FoldState;
use crate::ids::{AgentKey, ContentHash as JournalHash, JournalSeq};
use crate::wire_pending::ResolvedAgentConfig;
use aex_wire::ids::ContentHash as BodyDigest;

/// The only snapshot schema this build can read or write.
pub const FOLD_SNAPSHOT_SCHEMA: &str = "aex.brain.fold.v1";

/// A hard ceiling for one uncompressed canonical snapshot body.
///
/// Compression is deliberately absent. A compressed body would need a second bound over
/// decompressed bytes and CPU before it could be safe under hostile input.
pub const MAX_FOLD_SNAPSHOT_BYTES: usize = 16 * 1_024 * 1_024;

/// One exact point in an agent's append-only journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalPoint {
    /// Last sequence absorbed by the snapshot.
    pub seq: JournalSeq,
    /// BLAKE3 content hash stored on that immutable journal row.
    pub hash: JournalHash,
}

/// Durable pointer selected under the owning agent's authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoldSnapshotPointer {
    /// Closed codec/schema identity.
    pub schema: String,
    /// The exact journal owner.
    pub agent: AgentKey,
    /// Last journal row absorbed.
    pub absorbed: JournalPoint,
    /// SHA-256 over canonical [`ResolvedAgentConfig`] bytes.
    pub config_digest: BodyDigest,
    /// SHA-256 over the whole canonical snapshot body.
    pub body_digest: BodyDigest,
    /// Exact uncompressed body length.
    pub body_bytes: u64,
}

/// A verified snapshot ready to seed suffix replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedFoldSnapshot {
    /// The pointer that selected the body.
    pub pointer: FoldSnapshotPointer,
    /// The validated state at `pointer.absorbed`.
    pub state: FoldState,
}

/// Canonical immutable bytes and the pointer that names them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoldSnapshotArtifact {
    /// Metadata written to the monotonic durable pointer.
    pub pointer: FoldSnapshotPointer,
    /// Uncompressed JCS bytes written through the regional content authority.
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FoldSnapshotDocument {
    schema: String,
    agent: AgentKey,
    absorbed: JournalPoint,
    config_digest: BodyDigest,
    state: FoldState,
}

/// Why snapshot bytes cannot seed a fold.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FoldSnapshotError {
    /// A snapshot can only be cut after at least one journal row was applied.
    #[error("a fold snapshot requires a non-empty journal tail")]
    EmptyJournal,
    /// The fold must carry the configuration introduced by `AgentStarted`.
    #[error("a fold snapshot requires the pinned agent configuration")]
    MissingConfig,
    /// The pointer's schema is not understood by this build.
    #[error("unsupported fold snapshot schema `{found}`")]
    Schema {
        /// The supplied schema.
        found: String,
    },
    /// A pointer selected another agent's state.
    #[error("fold snapshot agent does not match the requested journal")]
    AgentMismatch,
    /// The declared body length is outside the caller's bound.
    #[error("fold snapshot body length {declared} exceeds the {max} byte bound")]
    BodyTooLarge {
        /// Pointer-declared bytes.
        declared: u64,
        /// Caller-selected ceiling.
        max: usize,
    },
    /// The fetched body length differs from the pointer.
    #[error("fold snapshot body length mismatch: pointer {declared}, body {actual}")]
    BodyLengthMismatch {
        /// Pointer-declared bytes.
        declared: u64,
        /// Fetched bytes.
        actual: usize,
    },
    /// SHA-256 over the fetched body differs from the pointer.
    #[error("fold snapshot body digest mismatch")]
    BodyDigestMismatch,
    /// The body was not valid JSON for the closed snapshot document.
    #[error("fold snapshot body is not the closed document: {reason}")]
    Decode {
        /// Redacted parser detail.
        reason: String,
    },
    /// The body was valid JSON but was not the workspace's exact JCS spelling.
    #[error("fold snapshot body is not canonical JCS")]
    NonCanonical,
    /// A repeated pointer field disagreed with the immutable body.
    #[error("fold snapshot pointer and body disagree on `{field}`")]
    PointerBodyMismatch {
        /// The disagreeing closed field name.
        field: &'static str,
    },
    /// The state does not end at the absorbed journal sequence.
    #[error("fold state tail does not match the absorbed journal sequence")]
    StateTailMismatch,
    /// The state retained an impossible hash span.
    #[error("fold state retained hash span is inconsistent with its base and tail")]
    StateHashSpanMismatch,
    /// The last retained journal hash differs from the absorbed row.
    #[error("fold state tail hash does not match the absorbed journal row")]
    StateTailHashMismatch,
    /// The pinned configuration digest differs from the body header/pointer.
    #[error("fold state pinned configuration digest mismatch")]
    ConfigDigestMismatch,
    /// The workspace canonicalizer refused the document.
    #[error("fold snapshot canonicalization failed: {reason}")]
    Canonical {
        /// Canonicalizer detail.
        reason: String,
    },
}

impl FoldSnapshotArtifact {
    /// Captures a validated fold using the workspace's sole JCS serializer and the content
    /// authority's SHA-256 body identity.
    ///
    /// # Errors
    ///
    /// Refuses an empty/unconfigured/internally inconsistent fold or a body above
    /// [`MAX_FOLD_SNAPSHOT_BYTES`].
    pub fn capture(agent: AgentKey, state: &FoldState) -> Result<Self, FoldSnapshotError> {
        let absorbed = point_of(state)?;
        let config_digest = config_digest(state)?;
        validate_state(state, absorbed, config_digest)?;
        let document = FoldSnapshotDocument {
            schema: FOLD_SNAPSHOT_SCHEMA.to_owned(),
            agent,
            absorbed,
            config_digest,
            state: state.clone(),
        };
        let body = canonical_bytes(&document)?;
        if body.len() > MAX_FOLD_SNAPSHOT_BYTES {
            return Err(FoldSnapshotError::BodyTooLarge {
                declared: u64::try_from(body.len()).unwrap_or(u64::MAX),
                max: MAX_FOLD_SNAPSHOT_BYTES,
            });
        }
        let pointer = FoldSnapshotPointer {
            schema: FOLD_SNAPSHOT_SCHEMA.to_owned(),
            agent,
            absorbed,
            config_digest,
            body_digest: BodyDigest::of(&body),
            body_bytes: u64::try_from(body.len()).unwrap_or(u64::MAX),
        };
        Ok(Self { pointer, body })
    }
}

impl FoldSnapshotPointer {
    /// Verifies and decodes the exact body selected by this pointer.
    ///
    /// The byte ceiling is checked before JSON parsing. Canonical bytes are recomputed with
    /// `aex_wire::to_jcs_bytes`, so an AWS dependency enabling `serde_json/preserve_order`
    /// cannot change key order or digest identity.
    ///
    /// # Errors
    ///
    /// Refuses every pointer/body/schema/key/digest/size/tail/hash/config mismatch.
    pub fn verify(
        &self,
        requested: AgentKey,
        body: &[u8],
        max_bytes: usize,
    ) -> Result<VerifiedFoldSnapshot, FoldSnapshotError> {
        let max_bytes = max_bytes.min(MAX_FOLD_SNAPSHOT_BYTES);
        if self.schema != FOLD_SNAPSHOT_SCHEMA {
            return Err(FoldSnapshotError::Schema {
                found: self.schema.clone(),
            });
        }
        if self.agent != requested {
            return Err(FoldSnapshotError::AgentMismatch);
        }
        if self.body_bytes > u64::try_from(max_bytes).unwrap_or(u64::MAX) {
            return Err(FoldSnapshotError::BodyTooLarge {
                declared: self.body_bytes,
                max: max_bytes,
            });
        }
        if usize::try_from(self.body_bytes).ok() != Some(body.len()) {
            return Err(FoldSnapshotError::BodyLengthMismatch {
                declared: self.body_bytes,
                actual: body.len(),
            });
        }
        if BodyDigest::of(body) != self.body_digest {
            return Err(FoldSnapshotError::BodyDigestMismatch);
        }
        let document: FoldSnapshotDocument =
            serde_json::from_slice(body).map_err(|error| FoldSnapshotError::Decode {
                reason: error.to_string(),
            })?;
        let canonical = canonical_bytes(&document)?;
        if canonical != body {
            return Err(FoldSnapshotError::NonCanonical);
        }
        if document.schema != self.schema {
            return Err(FoldSnapshotError::PointerBodyMismatch { field: "schema" });
        }
        if document.agent != self.agent {
            return Err(FoldSnapshotError::PointerBodyMismatch { field: "agent" });
        }
        if document.absorbed != self.absorbed {
            return Err(FoldSnapshotError::PointerBodyMismatch { field: "absorbed" });
        }
        if document.config_digest != self.config_digest {
            return Err(FoldSnapshotError::PointerBodyMismatch {
                field: "config_digest",
            });
        }
        validate_state(&document.state, document.absorbed, document.config_digest)?;
        Ok(VerifiedFoldSnapshot {
            pointer: self.clone(),
            state: document.state,
        })
    }
}

fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, FoldSnapshotError> {
    aex_wire::to_jcs_bytes(value).map_err(|error| FoldSnapshotError::Canonical {
        reason: error.to_string(),
    })
}

fn config_digest(state: &FoldState) -> Result<BodyDigest, FoldSnapshotError> {
    let config = state
        .config
        .as_deref()
        .ok_or(FoldSnapshotError::MissingConfig)?;
    config_digest_of(config)
}

/// Derives the snapshot's configuration identity from the exact typed configuration and
/// the workspace's canonical bytes.
///
/// # Errors
///
/// Returns [`FoldSnapshotError::Canonical`] if the typed configuration cannot be encoded as
/// the exact canonical bytes the snapshot contract hashes.
pub fn config_digest_of(config: &ResolvedAgentConfig) -> Result<BodyDigest, FoldSnapshotError> {
    canonical_bytes(config).map(|bytes| BodyDigest::of(&bytes))
}

fn point_of(state: &FoldState) -> Result<JournalPoint, FoldSnapshotError> {
    let seq = state.tail.ok_or(FoldSnapshotError::EmptyJournal)?;
    let hash = state
        .hashes
        .last()
        .copied()
        .ok_or(FoldSnapshotError::StateTailHashMismatch)?;
    Ok(JournalPoint { seq, hash })
}

fn validate_state(
    state: &FoldState,
    absorbed: JournalPoint,
    expected_config: BodyDigest,
) -> Result<(), FoldSnapshotError> {
    if state.tail != Some(absorbed.seq) {
        return Err(FoldSnapshotError::StateTailMismatch);
    }
    if state.base_seq > absorbed.seq {
        return Err(FoldSnapshotError::StateHashSpanMismatch);
    }
    let expected_hashes = absorbed
        .seq
        .get()
        .checked_sub(state.base_seq.get())
        .and_then(|delta| delta.checked_add(1))
        .and_then(|count| usize::try_from(count).ok())
        .ok_or(FoldSnapshotError::StateHashSpanMismatch)?;
    if state.hashes.len() != expected_hashes {
        return Err(FoldSnapshotError::StateHashSpanMismatch);
    }
    if state.hashes.last().copied() != Some(absorbed.hash) {
        return Err(FoldSnapshotError::StateTailHashMismatch);
    }
    if config_digest(state)? != expected_config {
        return Err(FoldSnapshotError::ConfigDigestMismatch);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{FoldSnapshotArtifact, FoldSnapshotError};
    use crate::fold::FoldState;
    use crate::ids::{AgentId, AgentKey, SessionId};
    use uuid::Uuid;

    #[test]
    fn an_empty_fold_cannot_become_snapshot_authority() {
        let key = AgentKey::new(SessionId(Uuid::from_u128(1)), AgentId(Uuid::from_u128(2)));
        assert_eq!(
            FoldSnapshotArtifact::capture(key, &FoldState::empty()),
            Err(FoldSnapshotError::EmptyJournal)
        );
    }
}
