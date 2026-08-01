//! `HandsPort` — implemented by `aex-brain-hands`.
//!
//! Every method names an exact [`HandsGeneration`]. A generation is a specific `MicroVM`
//! incarnation, and an operation started against one must never be answered by another: a
//! new generation has a different filesystem, so impersonating the old one's result would
//! hand the model a fabricated answer.

use super::BoxFuture;
use super::proof::DispatchTicket;
use super::provider::RedactedDetail;
use aex_brain_domain::effect::{DispatchProof, DispatchStage};
use aex_brain_domain::ids::{
    ContentHash, Fence, HandsGeneration, HandsOperationId, SessionId, Timestamp,
};
use aex_brain_domain::wire_pending::ContentRef;

/// Lifecycle and operations against one session's Hands `MicroVM`.
pub trait HandsPort: Send + Sync + 'static {
    /// Ensures the exact generation `g` is running and reachable.
    ///
    /// "Exact" is the contract: an implementation that silently starts a replacement has
    /// broken the generation fence, whatever it returns.
    fn ensure_generation<'a>(
        &'a self,
        session: &'a SessionId,
        generation: HandsGeneration,
    ) -> BoxFuture<'a, Result<HandsEndpoint, HandsError>>;

    /// Starts an operation.
    ///
    /// The identity in `start` is deterministic, so repeating an identical `start` after a
    /// dropped connection returns the existing operation rather than creating a second one.
    fn start<'a>(
        &'a self,
        ticket: &'a DispatchTicket,
        generation: HandsGeneration,
        start: &'a HandsOperationStart,
    ) -> BoxFuture<'a, Result<HandsAccepted, HandsError>>;

    /// Asks where an operation stands.
    fn status<'a>(
        &'a self,
        generation: HandsGeneration,
        operation: &'a HandsOperationId,
    ) -> BoxFuture<'a, Result<HandsOperationStatus, HandsError>>;

    /// Best-effort cancellation, fenced by the caller's ownership generation.
    fn cancel<'a>(
        &'a self,
        generation: HandsGeneration,
        operation: &'a HandsOperationId,
        fence: Fence,
    ) -> BoxFuture<'a, Result<(), HandsError>>;

    /// Reads a completed operation's result within `bounds`.
    fn result<'a>(
        &'a self,
        generation: HandsGeneration,
        operation: &'a HandsOperationId,
        bounds: &'a ResultBounds,
    ) -> BoxFuture<'a, Result<HandsResult, HandsError>>;
}

/// A reachable guest for one generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandsEndpoint {
    /// The generation this endpoint serves. Compared against the requested one before any
    /// operation is sent.
    pub generation: HandsGeneration,
    /// An opaque address the adapter understands.
    pub address: String,
    /// When the endpoint's keepalive lease expires.
    pub lease_expires_at: Timestamp,
}

/// A request to start one operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandsOperationStart {
    /// The deterministic operation identity, so a repeated start is idempotent.
    pub operation: HandsOperationId,
    /// A hash over the canonical call, so a repeated start carrying *different* arguments
    /// is a conflict rather than a silently different operation.
    pub call_hash: ContentHash,
    /// The canonical operation request the guest understands.
    pub request: serde_json::Value,
    /// The bounds the guest enforces.
    pub bounds: ResultBounds,
    /// When this attempt must have settled by.
    pub deadline: Timestamp,
}

/// The guest accepted an operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandsAccepted {
    /// The operation the guest is running.
    pub operation: HandsOperationId,
    /// The generation that accepted it.
    pub generation: HandsGeneration,
    /// Whether this call created the operation or found it already running. A repeated
    /// start reports `false`, which is how the caller knows its retry was idempotent
    /// rather than a second execution.
    pub created: bool,
    /// How long to wait before the first status query.
    pub poll_after: core::time::Duration,
}

/// Where a Hands operation stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandsOperationStatus {
    /// Still running.
    Running {
        /// How long to wait before the next query.
        poll_after: core::time::Duration,
    },
    /// Finished; the result is readable.
    Completed {
        /// The exit status the guest reported.
        exit_code: i32,
    },
    /// Finished with a proved failure.
    Failed {
        /// A redacted reason.
        reason: RedactedDetail,
    },
    /// Cancelled.
    Cancelled,
    /// The generation that ran it is gone, so the result cannot be produced and must not
    /// be reconstructed from a successor.
    GenerationLost,
}

/// The bounds a result is read under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResultBounds {
    /// The most bytes the result may carry.
    pub max_bytes: usize,
    /// The most bytes of each output stream that are retained.
    pub max_stream_bytes: usize,
    /// The wall-clock ceiling for the operation.
    pub timeout_ms: u32,
}

/// A completed operation's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandsResult {
    /// The operation.
    pub operation: HandsOperationId,
    /// The generation that produced it.
    pub generation: HandsGeneration,
    /// The exit status.
    pub exit_code: i32,
    /// Inline output, when it fitted the bounds.
    pub inline: Option<String>,
    /// A pointer to the placed output, when it did not.
    pub placed: Option<ContentRef>,
    /// Whether the output was cut short by the bounds. Never silently: a truncated
    /// deliverable reported as complete is the failure this flag exists to prevent.
    pub truncated: bool,
    /// A checksum over the result, verified before it enters the journal.
    pub checksum: ContentHash,
}

/// Why a Hands call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HandsError {
    /// The endpoint served a different generation than the one requested.
    #[error("expected generation {expected:?}, endpoint serves {found:?}")]
    GenerationMismatch {
        /// What the caller required.
        expected: HandsGeneration,
        /// What the endpoint offered.
        found: HandsGeneration,
    },
    /// The exact generation no longer exists. An uncommitted operation interrupts; a new
    /// generation never impersonates the old result.
    #[error("generation {generation:?} is gone")]
    GenerationLost {
        /// The generation that is gone.
        generation: HandsGeneration,
    },
    /// The same operation identity arrived carrying a different call.
    #[error("operation {operation:?} already exists under a different call hash")]
    CallHashConflict {
        /// The operation.
        operation: HandsOperationId,
    },
    /// The result failed its checksum or exceeded its bounds, so no bytes enter the
    /// journal. The diagnostic pointer is retained; the corrupt payload is not.
    #[error("result for {operation:?} was rejected: {reason}")]
    ResultRejected {
        /// The operation.
        operation: HandsOperationId,
        /// A redacted reason.
        reason: RedactedDetail,
    },
    /// The transport failed.
    #[error("hands transport failed at {stage:?} ({proof:?}): {detail}")]
    Transport {
        /// How far the attempt got.
        stage: DispatchStage,
        /// What the adapter can prove about whether the call reached the guest.
        proof: DispatchProof,
        /// A redacted description.
        detail: RedactedDetail,
    },
}
