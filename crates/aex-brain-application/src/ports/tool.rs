//! `ToolPort` — implemented by the composed router over `aex-brain-tool-catalog`,
//! `aex-brain-managed-web` and `aex-brain-mcp`.

use super::BoxFuture;
use super::proof::{CancelToken, DispatchTicket};
use aex_brain_domain::effect::{DetachedOperationRef, DispatchProof, DispatchStage, EffectClass};
use aex_brain_domain::ids::{
    CatalogPin, ContentHash, DetachedOperationId, Fence, ToolCallId, ToolName,
};
use aex_brain_domain::journal::ExecutorRoute;
use aex_model_catalog::canonical::ToolResultPart;
use aex_wire::CanonicalJson;
use aex_wire::ids::GenerationId;

/// One tool invocation, whichever executor actually runs it.
pub trait ToolPort: Send + Sync + 'static {
    /// Resolves `name` against the pinned catalog.
    ///
    /// Synchronous because the catalog is a signed immutable artifact already in memory:
    /// routing must not be able to reach the network, or a routing decision would become a
    /// failure mode.
    ///
    /// # Errors
    ///
    /// Returns [`ToolRoutingError`] when the tool is absent from the pin or not admitted.
    fn route(&self, pin: &CatalogPin, name: &ToolName) -> Result<ToolRoute, ToolRoutingError>;

    /// Invokes `call`.
    ///
    /// The ticket proves the durable `dispatch_started` write already committed.
    fn invoke<'a>(
        &'a self,
        ticket: &'a DispatchTicket,
        call: &'a PreparedToolCall,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<ToolOutcome, ToolDispatchError>>;

    /// Asks about a detached operation. Never creates a second one.
    ///
    /// [`DetachedStatus::Unknown`] is reserved for an authoritative durable-absence
    /// response from the selected executor. Read-after-accept propagation lag must be a
    /// retryable [`ToolDispatchError`], so the activation can retry until the persisted
    /// effect deadline instead of prematurely settling an unknown outcome.
    fn query<'a>(
        &'a self,
        operation: &'a DetachedOperationRef,
    ) -> BoxFuture<'a, Result<DetachedStatus, ToolDispatchError>>;

    /// Best-effort cancellation of a detached operation, fenced by the caller's ownership
    /// generation so a stale owner cannot cancel the new owner's work.
    fn cancel<'a>(
        &'a self,
        operation: &'a DetachedOperationRef,
        fence: Fence,
    ) -> BoxFuture<'a, Result<(), ToolDispatchError>>;
}

/// Where a tool runs and under what recovery contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRoute {
    /// The tool.
    pub name: ToolName,
    /// Which executor runs it.
    pub executor: ExecutorRoute,
    /// The recovery contract the manifest declares. The effect driver reads this and
    /// nothing else when deciding whether an ambiguous dispatch may be retried.
    pub class: EffectClass,
    /// The wall-clock ceiling for one invocation.
    pub timeout_ms: u32,
    /// The manifest digest the route was resolved from, so a receipt can name it.
    pub manifest_digest: ContentHash,
}

/// Why a tool could not be routed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ToolRoutingError {
    /// The pinned catalog does not contain the tool.
    #[error("tool `{name}` is not in the pinned catalog")]
    Unknown {
        /// The tool that was asked for.
        name: String,
    },
    /// The tool exists but has no live conformance receipt admitting it.
    #[error("tool `{name}` is staged, not admitted")]
    NotAdmitted {
        /// The tool that was asked for.
        name: String,
    },
    /// The pin does not resolve to a catalog this process holds.
    #[error("catalog pin {pin} is not loaded")]
    UnknownPin {
        /// The pin.
        pin: CatalogPin,
    },
}

/// A tool call validated against its manifest and ready to invoke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedToolCall {
    /// The call identity the provider minted.
    pub call: ToolCallId,
    /// Where it runs.
    pub route: ToolRoute,
    /// The canonical input, already validated against the manifest schema.
    pub input: CanonicalJson,
    /// The most bytes the result may carry.
    pub max_result_bytes: usize,
    /// The immutable session Hands generation this call must use.
    ///
    /// Non-Hands executors ignore this value. Carrying it on the prepared call keeps the
    /// tenant-scoped generation out of the process-global router and gives detached Hands
    /// recovery an exact generation to persist in its operation reference.
    pub hands_generation: GenerationId,
    /// A read-only view of the agent's control state.
    ///
    /// Carried on the call so a control-reading tool such as `todo_read` stays a pure
    /// function of its inputs instead of reaching back into the activation, which would
    /// make it unorderable against the journal.
    pub control: ControlStateView,
}

/// The read-only control state a tool may see.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ControlStateView {
    /// Canonical arguments of the last successful `todo_write`, when present.
    ///
    /// The executor reads this fold projection rather than reaching back into
    /// the activation or a second storage authority.
    pub todo_state: Option<CanonicalJson>,
    /// How many assistant turns have committed.
    pub assistant_turns: u32,
    /// Lineage depth, root at zero.
    pub depth: u16,
}

/// What an invocation produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolOutcome {
    /// The tool ran to completion inside the call.
    Completed(ToolResultBody),
    /// The tool accepted the work and returned an operation to query later. The connection
    /// is released and the activation parks: a long tool must not hold a socket or a lease.
    Detached {
        /// What to query.
        operation: DetachedOperationId,
        /// How long to wait before the first query.
        poll_after: core::time::Duration,
    },
}

/// A tool result, bounded and checksummed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResultBody {
    /// The canonical non-recursive result content.
    pub content: Vec<ToolResultPart>,
    /// Whether the tool reported failure. A tool that failed is a *result*, not a dispatch
    /// error: the model decides what to do about it.
    pub is_error: bool,
    /// How long it took.
    pub duration_ms: u32,
    /// Which executor ran it.
    pub executed_on: ExecutorRoute,
    /// A checksum over the canonical result content, so a detached result can
    /// be verified before it enters the journal.
    pub checksum: ContentHash,
}

/// Where a detached operation stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetachedStatus {
    /// Still running. Query again after this long.
    Running {
        /// How long to wait before the next query.
        poll_after: core::time::Duration,
    },
    /// Finished with a result.
    Completed(Box<ToolResultBody>),
    /// Finished with a proved failure.
    Failed {
        /// A redacted reason.
        reason: String,
    },
    /// The selected executor authoritatively reports that the accepted durable operation
    /// does not exist. The effect settles `OutcomeUnknown`.
    ///
    /// This arm exists on the status response rather than in a shared enum because these
    /// are the responses that can settle a dropped connection, and a caller must be forced
    /// to handle it rather than being able to reach for a default. Eventual-consistency lag
    /// is not `Unknown`; it is a retryable [`ToolDispatchError`].
    Unknown,
}

/// Why a tool dispatch failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("tool dispatch failed at {stage:?} ({proof:?}): {detail}")]
pub struct ToolDispatchError {
    /// How far the attempt got.
    pub stage: DispatchStage,
    /// What the adapter can prove about whether the call reached its executor.
    pub proof: DispatchProof,
    /// Whether retrying could help.
    pub retryable: bool,
    /// A redacted description.
    pub detail: super::provider::RedactedDetail,
}
