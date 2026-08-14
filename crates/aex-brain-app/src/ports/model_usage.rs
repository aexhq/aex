//! Durable handoff of provider-reported BYOK model usage.

use aex_brain_domain::ids::{EffectId, ModelSlug, SessionId, Timestamp};
use aex_brain_domain::wire_pending::{NormalizedUsage, ProviderId};
use aex_wire::ids::{OrganizationId, WorkspaceId};

use super::BoxFuture;

/// One complete assistant turn whose journal commit is already authoritative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelUsageObservation {
    /// Account attribution.
    pub organization: OrganizationId,
    /// Workspace attribution.
    pub workspace: WorkspaceId,
    /// Session attribution.
    pub session: SessionId,
    /// Retry-stable assistant identity.
    pub effect: EffectId,
    /// Official provider family.
    pub provider: ProviderId,
    /// Provider model slug.
    pub model: ModelSlug,
    /// Provider-reported usage and completeness.
    pub usage: NormalizedUsage,
    /// Commit time of the assistant record.
    pub observed_at: Timestamp,
}

/// Why a committed observation has not reached the central inbox queue yet.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelUsageError {
    /// The regional-to-central FIFO handoff failed; the journal marker remains pending.
    #[error("model-usage handoff failed: {0}")]
    Unavailable(String),
}

/// Whether the adapter durably accepted the handoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelUsagePublication {
    /// The queue accepted the deterministic facts; Brain must commit its marker.
    Accepted,
    /// Test-only composition with no external usage authority.
    #[cfg(any(test, feature = "testing"))]
    Disabled,
}

/// Publishes retry-stable facts after the assistant record is durable.
pub trait ModelUsagePort: Send + Sync + 'static {
    /// Publishes every field the provider actually reported.
    fn publish<'a>(
        &'a self,
        observation: &'a ModelUsageObservation,
    ) -> BoxFuture<'a, Result<ModelUsagePublication, ModelUsageError>>;
}

/// Test-only sink for compositions that do not exercise billing handoff.
#[cfg(any(test, feature = "testing"))]
#[derive(Debug, Default)]
pub struct NullModelUsagePort;

#[cfg(any(test, feature = "testing"))]
impl ModelUsagePort for NullModelUsagePort {
    fn publish<'a>(
        &'a self,
        _observation: &'a ModelUsageObservation,
    ) -> BoxFuture<'a, Result<ModelUsagePublication, ModelUsageError>> {
        Box::pin(async { Ok(ModelUsagePublication::Disabled) })
    }
}
