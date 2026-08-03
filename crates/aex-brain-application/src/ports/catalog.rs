//! `CatalogPort`, `ClockPort` and `IdPort` — the three ports that supply what the pure
//! domain deliberately cannot read for itself.

use aex_brain_domain::effect::EffectKind;
use aex_brain_domain::ids::{
    AgentId, CatalogPin, DetachedOperationId, EffectId, JournalSeq, ModelSlug, OwnerToken,
    Timestamp, WakeId,
};
pub use aex_model_catalog::document::CatalogDigest;
use aex_model_catalog::document::DurableOperationSupport;
use aex_model_catalog::{CatalogError as ModelCatalogError, QualifiedModel};
use aex_wire::provider::ProviderId;

/// The signed immutable catalog an agent is pinned to for its whole life.
///
/// Every method is synchronous. The catalog is a signed artifact already resolved and
/// verified in memory, so a capability lookup cannot reach the network — if it could, a
/// capability lookup would become a failure mode and an agent's pinned configuration would
/// stop being pinned.
pub trait CatalogPort: Send + Sync + 'static {
    /// The digest of the artifact `pin` names.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogError::UnknownPin`] when this process does not hold it.
    fn digest(&self, pin: &CatalogPin) -> Result<CatalogDigest, CatalogError>;

    /// What the catalog says about one model.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogError`] when the pin is unknown, the model is absent, or the entry
    /// is staged rather than admitted. There is no guessed capability, context window or
    /// price: an unknown model is not routable, whatever a request asks for.
    fn model(
        &self,
        pin: &CatalogPin,
        provider: ProviderId,
        model: &ModelSlug,
    ) -> Result<QualifiedModel, CatalogError>;

    /// Whether a dispatched call to this model can be resumed or looked up.
    ///
    /// Infallible and defaulting to [`DurableOperationSupport::None`] for an unknown entry:
    /// the safe answer to "can this be resumed?" is "no", and making the caller handle an
    /// error here would tempt it to guess.
    fn durable_operation_support(
        &self,
        pin: &CatalogPin,
        provider: ProviderId,
        model: &ModelSlug,
    ) -> DurableOperationSupport;
}

/// Why a catalog lookup failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogError {
    /// This process does not hold the pinned artifact.
    #[error("catalog pin {pin} is not loaded")]
    UnknownPin {
        /// The pin.
        pin: CatalogPin,
    },
    /// The verified catalog refused the provider/model pair.
    #[error(transparent)]
    Lookup(#[from] ModelCatalogError),
}

/// A monotonic instant. Never comparable to wall-clock time, on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SteadyInstant(pub u64);

impl SteadyInstant {
    /// Milliseconds elapsed since `earlier`, saturating at zero.
    #[must_use]
    pub const fn saturating_since(self, earlier: Self) -> u64 {
        self.0.saturating_sub(earlier.0)
    }
}

/// Time, as the only two readings and the one timer the Brain is allowed to use.
///
/// Deadlines use [`ClockPort::steady`]; anything durable uses [`ClockPort::now`]. Keeping
/// them apart is what stops a clock adjustment from expiring a live deadline, and what
/// stops a monotonic reading from being written into a record where it would mean nothing
/// to another process.
pub trait ClockPort: Send + Sync + 'static {
    /// Wall clock, in epoch milliseconds. Durable records carry this.
    fn now(&self) -> Timestamp;

    /// A monotonic reading. Deadlines and elapsed-time measurements use this.
    fn steady(&self) -> SteadyInstant;

    /// Completes after `duration` without occupying an executor thread.
    ///
    /// The lease supervisor uses this timer rather than polling an effect in a loop. Keeping
    /// the timer behind the same port as the readings preserves the runtime-free application
    /// boundary and gives deterministic tests control of every renewal interleaving.
    fn sleep(&self, duration: core::time::Duration) -> super::BoxFuture<'_, ()>;
}

/// Identifier generation.
///
/// The first two are **deterministic** functions of their inputs, which is what makes a
/// retried fanout page and a redelivered wake idempotent without any extra durable state.
/// The rest are fresh; in particular a reused owner token is a correctness bug, because two
/// claim attempts by one task become indistinguishable.
pub trait IdPort: Send + Sync + 'static {
    /// The `ordinal`-th child of `parent`. Deterministic.
    fn child_agent_id(&self, parent: &AgentId, ordinal: u32) -> AgentId;

    /// The identity of the `kind` effect prepared by `agent` at `seq`. Deterministic.
    fn effect_id(&self, agent: &AgentId, seq: JournalSeq, kind: EffectKind) -> EffectId;

    /// A fresh token for one claim attempt.
    fn owner_token(&self) -> OwnerToken;

    /// A fresh wake identity.
    fn wake_id(&self) -> WakeId;

    /// A fresh detached-operation identity.
    fn operation_id(&self) -> DetachedOperationId;
}
