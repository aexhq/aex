//! Verified rolling context checkpoint authority.

use super::{BoxFuture, FenceGuard, SessionAuthority};
use aex_brain_domain::checkpoint::{CheckpointMetadata, ContextCheckpoint};
use aex_brain_domain::ids::AgentKey;

/// Closed failures for immutable object or fenced pointer operations.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CheckpointError {
    /// The committed object is absent.
    #[error("the committed context checkpoint object is missing")]
    Missing,
    /// Body, metadata, authority, or content hash disagreed.
    #[error("the context checkpoint failed integrity validation")]
    Corrupt,
    /// Declared or received body exceeded the hard bound.
    #[error("the context checkpoint exceeds the hard size bound")]
    TooLarge,
    /// Another fenced owner or checkpoint won the metadata condition.
    #[error("the context checkpoint metadata condition was refused")]
    Conflict,
    /// AWS or the configured binding was unavailable.
    #[error("the context checkpoint authority is unavailable")]
    Unavailable,
}

/// Immutable `S3` object plus fenced `DynamoDB` head pointer.
pub trait ContextCheckpointStore: Send + Sync + 'static {
    /// Loads and verifies the exact object named by committed metadata.
    fn load<'a>(
        &'a self,
        key: AgentKey,
        metadata: &'a CheckpointMetadata,
    ) -> BoxFuture<'a, Result<ContextCheckpoint, CheckpointError>>;

    /// Writes the immutable object first, then conditionally advances the
    /// control-row pointer under the current session and agent fence.
    fn save<'a>(
        &'a self,
        guard: &'a FenceGuard,
        authority: &'a SessionAuthority,
        checkpoint: &'a ContextCheckpoint,
    ) -> BoxFuture<'a, Result<CheckpointMetadata, CheckpointError>>;
}
