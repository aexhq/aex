//! Vocabulary not yet published by the secret-custody and memory authorities.
//!
//! Brain provider-port, ticket, cancellation, preview and dispatch-evidence
//! concepts are imported from their owning crates; this module no longer
//! restates them.

use aex_model_catalog::BoundedString;
use aex_wire::CanonicalJson;
use aex_wire::ids::{ProviderCredentialId, WorkspaceId};
use serde::{Deserialize, Serialize};

pub use aex_secret_domain::{RevocationEpoch, SourceGeneration};

pub use aex_brain_application::ports::{
    BoxFuture, CancelToken, DispatchTicket, PreviewSink, ProviderDispatchError, ProviderOutcome,
    ProviderPort, UnknownResolution,
};
pub use aex_brain_domain::effect::{DispatchProof, DispatchStage};

/// Which buffer a reservation covers.
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

/// A byte-sized memory permit carried from mux admission.
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

/// A pointer to stored ciphertext. Never plaintext or ciphertext bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CiphertextRef(pub BoundedString<256>);

/// Temporary closed decrypt context until provider registration can bind the
/// decided secret-domain encryption context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncryptionContext {
    /// The binding workspace.
    pub workspace: WorkspaceId,
    /// Further canonical binding members.
    pub extra: CanonicalJson,
}

/// A stable provider-credential binding id: the `pcr_` record.
pub type BindingId = ProviderCredentialId;
