//! Vocabulary not yet published by the secret-custody and memory authorities.
//!
//! Brain provider-port, ticket, cancellation, preview and dispatch-evidence
//! concepts are imported from their owning crates; this module no longer
//! restates them.

use serde::{Deserialize, Serialize};

pub use aex_secret_domain::{CiphertextRef, EncryptionContext, RevocationEpoch, SourceGeneration};

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
