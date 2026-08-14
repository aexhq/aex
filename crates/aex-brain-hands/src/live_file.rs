//! Exact-generation live-file transport exposed to the regional session API.

use aex_brain_app::ports::{BoxFuture, HandsError};
use aex_hands_protocol::files::{FileRequest, FileResponse};
use aex_hands_protocol::operation::SandboxMcpQualification;
use aex_hands_protocol::rpc::Fence;
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

    /// Durably admits the exact tool operation as a runtime waiter and proves
    /// the guest is reachable. The waiter remains open until
    /// [`Self::settle_tool_waiter`], preventing eager suspension in the
    /// readiness-to-dispatch gap.
    fn hold_tool_waiter(
        &self,
        _session: SessionId,
        _generation: GenerationId,
        _operation: HandsOperationId,
    ) -> BoxFuture<'_, Result<Fence, HandsError>> {
        Box::pin(async {
            Err(HandsError::Transport {
                stage: aex_brain_domain::effect::DispatchStage::PreDispatch,
                proof: aex_brain_domain::effect::DispatchProof::NotSent,
                detail: aex_brain_app::ports::RedactedDetail::internal(
                    aex_brain_app::ports::ProviderFailureKind::ServerError,
                    "durable Tool Mux waiter admission is not composed",
                ),
            })
        })
    }

    /// Settles one previously admitted exact tool waiter idempotently.
    fn settle_tool_waiter(
        &self,
        _generation: GenerationId,
        _operation: HandsOperationId,
    ) -> BoxFuture<'_, Result<(), HandsError>> {
        Box::pin(async {
            Err(HandsError::Transport {
                stage: aex_brain_domain::effect::DispatchStage::PreDispatch,
                proof: aex_brain_domain::effect::DispatchProof::NotSent,
                detail: aex_brain_app::ports::RedactedDetail::internal(
                    aex_brain_app::ports::ProviderFailureKind::ServerError,
                    "durable Tool Mux waiter settlement is not composed",
                ),
            })
        })
    }

    /// Suspends a fully prepared exact generation when no durable tool waiter
    /// exists. Session admission invokes this only after workspace setup and
    /// MCP qualification have completed.
    fn suspend_ready(
        &self,
        _session: SessionId,
        _generation: GenerationId,
    ) -> BoxFuture<'_, Result<(), HandsError>> {
        Box::pin(async {
            Err(HandsError::Transport {
                stage: aex_brain_domain::effect::DispatchStage::PreDispatch,
                proof: aex_brain_domain::effect::DispatchProof::NotSent,
                detail: aex_brain_app::ports::RedactedDetail::internal(
                    aex_brain_app::ports::ProviderFailureKind::ServerError,
                    "eager sandbox suspension is not composed",
                ),
            })
        })
    }

    /// Starts one sandbox-process MCP server inside the exact generation,
    /// completes the pinned handshake, and returns its bounded tool names.
    fn qualify_sandbox_mcp<'a>(
        &'a self,
        _session: SessionId,
        _generation: GenerationId,
        _activity: HandsOperationId,
        _request: &'a SandboxMcpQualification,
    ) -> BoxFuture<'a, Result<Vec<String>, HandsError>> {
        Box::pin(async {
            Err(HandsError::Transport {
                stage: aex_brain_domain::effect::DispatchStage::PreDispatch,
                proof: aex_brain_domain::effect::DispatchProof::NotSent,
                detail: aex_brain_app::ports::RedactedDetail::internal(
                    aex_brain_app::ports::ProviderFailureKind::ServerError,
                    "sandbox MCP qualification is not composed",
                ),
            })
        })
    }

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
