//! `ProviderPort` — implemented by `aex-brain-provider-gateway`.

use super::BoxFuture;
use super::proof::{CancelToken, DispatchTicket, PreviewSink, StreamBudget};
use aex_brain_domain::effect::{DispatchEvidence, DispatchProof, DispatchStage};
use aex_brain_domain::ids::ProviderRequestId;
use aex_brain_domain::wire_pending::{
    CanonicalModelRequest, CompleteAssistantMessage, NormalizedUsage, ProviderReceipt,
};

/// One model dispatch over the six admitted `BYOK` providers.
///
/// There is exactly one implementation: a gateway that routes to `openai`, `anthropic`,
/// `deepseek`, `zai`, `moonshotai` or `google`. No aggregator, no arbitrary base URL and no
/// cross-provider fallback, because a request served by a provider the customer did not
/// name is a request they cannot reconcile against their own bill.
pub trait ProviderPort: Send + Sync + 'static {
    /// Dispatches `request` and streams it to completion.
    ///
    /// The ticket is the proof that the durable `dispatch_started` write already committed.
    /// An implementation must not send a byte before it holds one, and must not send a
    /// second request under the same ticket.
    fn dispatch<'a>(
        &'a self,
        ticket: &'a DispatchTicket,
        request: &'a CanonicalModelRequest,
        budget: &'a StreamBudget,
        preview: &'a dyn PreviewSink,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderDispatchError>>;

    /// Asks the upstream what became of an effect whose outcome is not provable locally.
    ///
    /// At launch every implementation returns [`UnknownResolution::NoDurableOperation`]:
    /// no `BYOK` provider exposes a proven generation-resume or result-lookup operation.
    /// The method exists so that when one does, adopting it is a catalog change rather
    /// than a redesign — and so the absence is stated rather than assumed.
    fn resolve_unknown<'a>(
        &'a self,
        identity: &'a aex_brain_domain::effect::DurableEffect,
        evidence: &'a DispatchEvidence,
    ) -> BoxFuture<'a, Result<UnknownResolution, ProviderDispatchError>>;
}

/// What a completed dispatch produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderOutcome {
    /// The whole message, carrying the proof that makes it admissible to the fold.
    pub message: CompleteAssistantMessage,
    /// Provider-reported usage, normalized across dialects.
    pub usage: NormalizedUsage,
    /// Which provider and model actually served it, and under which route revision.
    pub receipt: ProviderReceipt,
}

/// Why a dispatch did not produce an outcome.
///
/// `proof` is the load-bearing field. It is the adapter's assertion about whether the
/// request reached the upstream, and the recovery matrix consumes nothing else:
/// [`DispatchProof::NotSent`] is the only value that permits an automatic retry.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("provider dispatch failed at {stage:?} ({class:?}, {proof:?})")]
pub struct ProviderDispatchError {
    /// How far the attempt got.
    pub stage: DispatchStage,
    /// What the adapter can prove about whether bytes left the process.
    pub proof: DispatchProof,
    /// How the failure should be treated by a retry policy.
    pub class: ProviderFailureClass,
    /// The provider's own request id, when one was observed.
    pub provider_request_id: Option<ProviderRequestId>,
    /// How long the provider asked the caller to wait.
    pub retry_after: Option<core::time::Duration>,
    /// A redacted description. Never carries a credential.
    pub detail: RedactedDetail,
}

/// How a provider failure should be treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderFailureClass {
    /// Retryable within the effect deadline.
    Transient,
    /// Retrying will not help: a bad request, an unknown model, a rejected credential.
    Permanent,
    /// The provider is at capacity. Retryable, but back off.
    Overloaded,
    /// The caller cancelled.
    Cancelled,
}

/// A description safe to log.
///
/// A newtype rather than a `String` so that "this text has been through redaction" is a
/// fact the type system carries. Constructing one is where an adapter must have already
/// removed credentials, prompts and customer content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactedDetail(String);

impl RedactedDetail {
    /// Records `text` as already redacted.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    /// The redacted text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Display for RedactedDetail {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// What an upstream said about an effect whose outcome could not be proved locally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnknownResolution {
    /// The upstream produced the outcome after all.
    Resolved(Box<ProviderOutcome>),
    /// The upstream has no durable operation to ask about, so the effect stays unknown and
    /// the run terminalizes `interrupted`. This is the honest answer at launch, and
    /// returning it is not a failure to try.
    NoDurableOperation,
}
