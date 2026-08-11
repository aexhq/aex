//! Exact-generation live-file transport exposed to the regional session API.

use aex_brain_app::ports::{BoxFuture, HandsError};
use aex_hands_protocol::files::{FileRequest, FileResponse};
use aex_hands_protocol::rpc::HandsOperationId;
use aex_wire::ids::{GenerationId, SessionId};
use aex_wire::types::Timestamp;

/// One authenticated guest answer and the lifecycle work needed to obtain it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveFileReply {
    /// Typed lstat or binary-transfer answers from Hands, in request order.
    pub responses: Vec<FileResponse>,
    /// Exact generation that answered.
    pub generation: GenerationId,
    /// Whether this call successfully requested resume of a suspended generation.
    pub resumed: bool,
    /// Lifecycle fence observed after any same-generation resume. A list cursor
    /// binds this value so a suspend/resume transition invalidates the walk.
    pub lifecycle_fence: u64,
    /// Provider hard-stop instant for this generation.
    pub expires_at: Timestamp,
}

/// Provider-authoritative readiness for one exact retained generation.
///
/// Session creation uses this before it publishes a public head. Reaching this
/// value proves both that the provider settled the elected generation as
/// running and that the authenticated Hands endpoint answered for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveGenerationReady {
    /// Exact generation that became reachable.
    pub generation: GenerationId,
    /// Provider-authoritative launch instant.
    pub launched_at: Timestamp,
    /// Provider hard-stop instant for the generation.
    pub expires_at: Timestamp,
    /// When the authenticated endpoint readiness was observed.
    pub observed_at: Timestamp,
    /// Lifecycle fence of the reachable generation.
    pub lifecycle_fence: u64,
}

/// The narrow live-file capability consumed by the public regional edge.
///
/// The port cannot launch a successor generation. Every call names the retained
/// session and exact generation; a suspended generation is resumed in place,
/// while a terminated, lost or superseded one is refused.
pub trait LiveFileBackend: Send + Sync + 'static {
    /// Launches or recovers one elected generation and proves its authenticated
    /// guest endpoint is reachable without admitting a customer activity.
    fn ensure_ready(
        &self,
        session: SessionId,
        generation: GenerationId,
    ) -> BoxFuture<'_, Result<LiveGenerationReady, HandsError>>;

    /// Sends one bounded batch under one durable activity admission.
    fn call<'a>(
        &'a self,
        session: SessionId,
        generation: GenerationId,
        activity: HandsOperationId,
        requests: &'a [FileRequest],
    ) -> BoxFuture<'a, Result<LiveFileReply, HandsError>>;

    /// Compensates a create that launched but could not publish.
    ///
    /// The call is exact-generation and idempotent. It reconciles an open
    /// launch/terminate intent before returning and never allocates a successor.
    fn abort_unpublished(
        &self,
        session: SessionId,
        generation: GenerationId,
    ) -> BoxFuture<'_, Result<(), HandsError>>;
}
