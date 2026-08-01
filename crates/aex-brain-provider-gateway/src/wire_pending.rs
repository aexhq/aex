//! The Brain and secret-custody vocabulary this crate implements but does not
//! own.
//!
//! `plans/07-brain-core.md` §7 declares `ProviderPort`, `CatalogPort`,
//! `DispatchTicket`, `CancelToken`, `PreviewSink`, `DispatchProof`,
//! `DispatchStage`, `ProviderFailureClass`, `ProviderOutcome`,
//! `ProviderDispatchError`, `UnknownResolution`, `DispatchEvidence`,
//! `EffectIdentity`, `BoxFuture`, `MemoryReservation` and `ReservationClass`.
//! `aex-brain-application` is being implemented concurrently, so the signatures
//! are restated here **verbatim** and this crate implements them against these
//! definitions.
//!
//! `plans/04-regional-domains.md` owns `SourceGeneration`, `RevocationEpoch`,
//! `CiphertextRef` and `EncryptionContext`; the same applies.
//!
//! `TODO(cross-stream): every item in this module is replaced by
//! aex_brain_application::ports::<Path> or aex_secret_domain::<Path> at merge.
//! Nothing here re-specifies a rule; the shapes are copies.`

use core::future::Future;
use core::pin::Pin;
use core::time::Duration;

use aex_model_catalog::ProviderFailureClass;
use aex_model_catalog::canonical::{
    CanonicalModelRequest, CompleteAssistantMessage, NormalizedUsage, PreviewFrame, ProviderReceipt,
};
use aex_model_catalog::primitives::{BoundedString, ProviderRequestId};
use aex_wire::ids::{ProviderCredentialId, WorkspaceId};
use aex_wire::{CanonicalJson, ContentHash};
use serde::{Deserialize, Serialize};

use crate::error::RedactedDetail;

/// `TODO(cross-stream): replaced by aex_brain_application::ports::BoxFuture at merge.`
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

// ---------------------------------------------------------------------------
// effect identity and dispatch evidence
// ---------------------------------------------------------------------------

/// `TODO(cross-stream): replaced by aex_brain_domain::effect::EffectId at merge.`
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EffectId(pub [u8; 16]);

/// `TODO(cross-stream): replaced by aex_brain_domain::effect::EffectIdentity at merge.`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectIdentity {
    /// The deterministic effect id.
    pub effect: EffectId,
    /// The hash of the canonical request the effect prepared.
    pub request_hash: ContentHash,
    /// How many times the effect has been attempted.
    pub attempt: u16,
}

/// How far a dispatch got. The adapter's only durable output on the failure
/// path.
///
/// `TODO(cross-stream): replaced by aex_brain_domain::effect::DispatchStage at merge.`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchStage {
    /// Nothing left the process.
    PreDispatch,
    /// The request was handed to the transport.
    Dispatched,
    /// At least one validated dialect frame was decoded.
    Streaming,
    /// A terminal frame arrived.
    Terminal,
}

/// Whether the provider can have seen the request.
///
/// `NotSent` is producible **only** before [`crate::transport::SendGate`] is
/// consumed. Everything downstream is `PossiblySent`, including `reqwest`
/// connect errors: a pooled HTTP/2 connection may already have carried the
/// request head, and no provider in this set offers a way to ask (D-13).
///
/// `TODO(cross-stream): replaced by aex_brain_domain::effect::DispatchProof at merge.`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchProof {
    /// Provably never left this process.
    NotSent,
    /// May or may not have reached the provider.
    PossiblySent,
    /// The provider was observed generating.
    ResponseStarted,
}

/// What the effect driver writes durably about a dispatch.
///
/// `TODO(cross-stream): replaced by aex_brain_domain::effect::DispatchEvidence at merge.`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchEvidence {
    /// How far it got.
    pub stage: DispatchStage,
    /// Whether the provider can have seen it.
    pub proof: DispatchProof,
    /// The provider's own request id, where it publishes one.
    pub provider_request_id: Option<ProviderRequestId>,
    /// Frames decoded before the stream ended.
    pub frames: u32,
    /// Response bytes observed.
    pub response_bytes: u64,
    /// The committed response receipt, where one was written.
    pub response_receipt: Option<ContentHash>,
}

// ---------------------------------------------------------------------------
// tickets, cancellation and preview
// ---------------------------------------------------------------------------

/// Proof that `EffectStore::mark_dispatch_started` committed.
///
/// Minted only by the effect store and consumed by
/// [`ProviderPort::dispatch`], so dispatching without a durable record does not
/// compile.
///
/// `TODO(cross-stream): replaced by aex_brain_application::ports::DispatchTicket at merge.`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchTicket {
    /// Which effect the ticket authorises.
    pub identity: EffectIdentity,
    /// The owning workspace, for pool isolation.
    pub workspace: WorkspaceId,
}

/// A cooperative cancellation flag.
///
/// `TODO(cross-stream): replaced by aex_brain_application::ports::CancelToken at merge.`
#[derive(Debug, Default)]
pub struct CancelToken {
    flag: std::sync::atomic::AtomicBool,
    notify: tokio::sync::Notify,
}

impl CancelToken {
    /// A token that has not been cancelled.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation. Idempotent.
    pub fn cancel(&self) {
        self.flag.store(true, std::sync::atomic::Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    /// Whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Resolves as soon as cancellation is requested, and immediately if it
    /// already has been.
    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        let waiter = self.notify.notified();
        if self.is_cancelled() {
            return;
        }
        waiter.await;
    }
}

/// Where non-authoritative deltas go.
///
/// `TODO(cross-stream): replaced by aex_brain_application::ports::PreviewSink at merge.`
pub trait PreviewSink: Send + Sync {
    /// Offers one preview frame. Never blocks and never fails the dispatch: a
    /// dropped preview is a lost pixel, not a lost turn.
    fn offer(&self, frame: PreviewFrame);
}

/// A sink that discards every frame, for callers that do not stream.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullPreviewSink;

impl PreviewSink for NullPreviewSink {
    fn offer(&self, _frame: PreviewFrame) {}
}

// ---------------------------------------------------------------------------
// the port itself
// ---------------------------------------------------------------------------

/// What a successful dispatch produced.
///
/// `TODO(cross-stream): replaced by aex_brain_application::ports::ProviderOutcome at merge.`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderOutcome {
    /// The sealed assistant turn, carrying its own completeness proof.
    pub message: CompleteAssistantMessage,
    /// Token accounting. A zero-dollar `BYOK` observability fact.
    pub usage: NormalizedUsage,
    /// The dispatch receipt.
    pub receipt: ProviderReceipt,
}

/// What a failed dispatch produced. Never carries a credential.
///
/// `TODO(cross-stream): replaced by aex_brain_application::ports::ProviderDispatchError at merge.`
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("provider dispatch failed at {stage:?} ({proof:?}, {class:?}): {detail}")]
pub struct ProviderDispatchError {
    /// How far it got.
    pub stage: DispatchStage,
    /// Whether the provider can have seen it.
    pub proof: DispatchProof,
    /// The port-facing class.
    pub class: ProviderFailureClass,
    /// The provider's own request id, where it publishes one.
    pub provider_request_id: Option<ProviderRequestId>,
    /// How long the provider asked the caller to wait.
    pub retry_after: Option<Duration>,
    /// Bounded, redacted detail.
    pub detail: RedactedDetail,
}

/// What a durable-operation lookup found.
///
/// `TODO(cross-stream): replaced by aex_brain_application::ports::UnknownResolution at merge.`
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnknownResolution {
    /// The provider still had the result.
    Resolved(Box<ProviderOutcome>),
    /// The provider offers no way to ask. The launch answer for all six.
    NoDurableOperation,
}

/// The provider dispatch port.
///
/// `TODO(cross-stream): replaced by aex_brain_application::ports::ProviderPort at merge.`
pub trait ProviderPort: Send + Sync + 'static {
    /// Runs exactly one generation, or fails with exactly one typed error.
    fn dispatch<'a>(
        &'a self,
        ticket: &'a DispatchTicket,
        request: &'a CanonicalModelRequest,
        budget: &'a crate::budget::StreamBudget,
        preview: &'a dyn PreviewSink,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderDispatchError>>;

    /// Asks the provider whether an ambiguous dispatch produced a result.
    fn resolve_unknown<'a>(
        &'a self,
        identity: &'a EffectIdentity,
        evidence: &'a DispatchEvidence,
    ) -> BoxFuture<'a, Result<UnknownResolution, ProviderDispatchError>>;
}

// ---------------------------------------------------------------------------
// memory reservations
// ---------------------------------------------------------------------------

/// Which buffer a reservation covers.
///
/// `TODO(cross-stream): replaced by aex_brain_application::pressure::ReservationClass at merge.`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReservationClass {
    /// Conversation context.
    Context,
    /// The canonical request body.
    CanonicalRequest,
    /// The SSE parser's rolling buffer.
    ParserBuffer,
    /// The preview coalescing buffer.
    PreviewBuffer,
    /// The assembled result.
    Result,
    /// A warm cache entry.
    WarmCacheEntry,
}

/// A byte-sized memory permit.
///
/// `TODO(cross-stream): replaced by aex_brain_application::pressure::MemoryReservation
/// at merge. The mux mints these; this crate only carries and honours them.`
#[derive(Debug)]
#[must_use]
pub struct MemoryReservation {
    /// How many bytes the holder may use.
    pub bytes: u64,
    /// Which buffer it covers.
    pub class: ReservationClass,
}

/// The reservations a dispatch was given.
#[derive(Debug, Default)]
pub struct ReservationSet {
    /// Every held permit.
    pub held: Vec<MemoryReservation>,
}

impl ReservationSet {
    /// The total reserved bytes for a class.
    #[must_use]
    pub fn bytes_for(&self, class: ReservationClass) -> u64 {
        self.held
            .iter()
            .filter(|reservation| reservation.class == class)
            .map(|reservation| reservation.bytes)
            .sum()
    }
}

// ---------------------------------------------------------------------------
// secret custody (plan 04)
// ---------------------------------------------------------------------------

/// Which generation of a workspace secret source a value came from.
///
/// `TODO(cross-stream): replaced by aex_secret_domain::generation::SourceGeneration at merge.`
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SourceGeneration(pub u64);

/// The workspace's revocation counter. Incrementing it invalidates every pin
/// taken at a lower value.
///
/// `TODO(cross-stream): replaced by aex_secret_domain::revocation::RevocationEpoch at merge.`
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RevocationEpoch(pub u64);

/// A pointer to stored ciphertext. Never the ciphertext itself.
///
/// `TODO(cross-stream): replaced by aex_secret_domain::custody::CiphertextRef at merge.`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CiphertextRef(pub BoundedString<256>);

/// The `AWS` `KMS` encryption context a decrypt must present.
///
/// `TODO(cross-stream): replaced by aex_secret_domain::custody::EncryptionContext at merge.`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncryptionContext {
    /// The binding workspace. `OD-18` makes this a `KMS` key-policy condition,
    /// not merely an advisory field.
    pub workspace: WorkspaceId,
    /// Any further bound members, canonical so two contexts compare by bytes.
    pub extra: CanonicalJson,
}

/// A stable provider-credential binding id: the `pcr_` record.
pub type BindingId = ProviderCredentialId;
