//! The ports the observation use cases depend on.
//!
//! Every port is read-only or write-only by construction, so a composition that
//! must not write cannot: `regional-stream` links a store type that exposes no
//! mutating method at all, and `observation-export-launcher` links no read port.

use aex_observation_domain::gap::{GapRecord, TimeWindow};
use aex_observation_domain::keys::ScopeKey;
use aex_observation_domain::signal::SignalSet;
use aex_wire::types::Timestamp;

/// Why a port call failed.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PortError {
    /// The backing authority is unreachable.
    #[error("the {authority} authority is unreachable: {reason}")]
    Unavailable {
        /// Which authority.
        authority: &'static str,
        /// What failed.
        reason: Box<str>,
    },
    /// The commit's transport outcome is unknown.
    ///
    /// Never retried blindly: the caller resolves it by batch identity before
    /// responding.
    #[error("the commit outcome is unknown; resolve it by batch identity")]
    CommitAmbiguous,
    /// The scope's deletion epoch advanced under the caller.
    #[error("the scope was deleted; the pinned epoch {pinned} is stale")]
    DeletionEpochAdvanced {
        /// The epoch the caller pinned.
        pinned: u64,
    },
    /// The regional ingress gate is closed.
    #[error("the regional ingress gate is closed: {reason}")]
    GateClosed {
        /// Why the gate closed.
        reason: &'static str,
        /// How long the client should wait.
        retry_after_ms: u64,
    },
}

/// One request for a page of session events.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventPageRequest {
    /// Which scope.
    pub scope: ScopeKey,
    /// The half-open observation-time window.
    pub window: TimeWindow,
    /// The page size.
    pub limit: u16,
    /// The last event sequence already delivered.
    pub after_seq: Option<u64>,
}

/// One session-journal event, read through the port.
///
/// `events` are **not** in `observation-authority` (decision O-01): they are the
/// session/Brain journal, already an ordered, durable, deletion-fenced authority
/// written inside the transactions that produce them. Copying them here would
/// add a write to the hottest transaction in the system and create a second
/// completeness question for one fact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticEvent {
    /// The public observation identity, `obs_`-prefixed.
    // TODO(cross-stream): replaced by aex_wire::ids::ObservationId once the
    // regional-stores stream makes `session_event.eventId` an ObservationId.
    pub event_id: Box<str>,
    /// The journal position, monotone with `occurred_at`.
    pub event_seq: u64,
    /// When it happened.
    pub occurred_at: Timestamp,
    /// The event type.
    pub kind: Box<str>,
}

/// One page of session events.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EventPage {
    /// The events, in `eventSeq` order.
    pub items: Vec<SemanticEvent>,
    /// The last sequence delivered, when the page is non-empty.
    pub last_seq: Option<u64>,
}

/// A read-only view of the session journal.
///
/// The only way this stream reads `events`. It never encodes a
/// `session-authority` key itself; it calls the peer's published `keys`/`codec`.
pub trait SemanticEventSource: Send + Sync {
    /// Reads one page of events.
    ///
    /// # Errors
    ///
    /// Returns [`PortError::Unavailable`] when `session-authority` cannot be
    /// read. It never returns an empty page in place of an error.
    fn read_events(&self, request: &EventPageRequest) -> Result<EventPage, PortError>;
}

/// What one admission asks the authority to commit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommitRequest {
    /// Which scope.
    pub scope: ScopeKey,
    /// Which signals the batch carries.
    pub signals: SignalSet,
    /// How many records.
    pub records: u32,
    /// How many canonical bytes.
    pub logical_bytes: u64,
    /// The scope's deletion epoch, pinned before staging.
    pub pinned_deletion_epoch: u64,
    /// When admission asserts the batch was accepted.
    pub accepted_at: Timestamp,
}

/// What the authority recorded.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommitReceipt {
    /// The first accepted ordinal.
    pub accepted_lo: u64,
    /// The last accepted ordinal.
    pub accepted_hi: u64,
    /// When the commit landed.
    pub accepted_at: Timestamp,
    /// How many series the batch newly claimed.
    pub new_series: u32,
}

/// The durable observation authority, write side.
#[async_trait::async_trait]
pub trait ObservationAuthority: Send + Sync {
    /// Reads the scope's pinned deletion epoch.
    ///
    /// # Errors
    ///
    /// Returns [`PortError::Unavailable`] when the authority cannot be read.
    async fn deletion_epoch(&self, scope: &ScopeKey) -> Result<u64, PortError>;

    /// Runs the nine-step admission for one prepared batch.
    ///
    /// # Errors
    ///
    /// Returns [`PortError::CommitAmbiguous`] for an unknown transport outcome,
    /// [`PortError::DeletionEpochAdvanced`] when the scope was deleted under the
    /// caller, and [`PortError::GateClosed`] when the regional ingress gate is
    /// closed.
    async fn commit(&self, request: &CommitRequest) -> Result<CommitReceipt, PortError>;
}

/// The append-only durable telemetry-gap ledger.
#[async_trait::async_trait]
pub trait GapSink: Send + Sync {
    /// Appends an explicit scoped gap revision.
    ///
    /// # Errors
    ///
    /// Returns [`PortError::Unavailable`] when the gap cannot be written. A gap
    /// that cannot be recorded is never treated as absent.
    async fn append_gap(&self, record: &GapRecord) -> Result<(), PortError>;
}

#[cfg(test)]
mod tests {
    use super::{EventPage, PortError};

    #[test]
    fn an_empty_event_page_carries_no_position() {
        let page = EventPage::default();
        assert!(page.items.is_empty());
        assert_eq!(page.last_seq, None);
    }

    #[test]
    fn every_port_failure_names_what_it_was() {
        let unavailable = PortError::Unavailable {
            authority: "session",
            reason: "timeout".into(),
        };
        assert!(unavailable.to_string().contains("session"));
        assert_eq!(
            PortError::CommitAmbiguous.to_string(),
            "the commit outcome is unknown; resolve it by batch identity"
        );
        assert!(
            PortError::DeletionEpochAdvanced { pinned: 3 }
                .to_string()
                .contains('3')
        );
        assert!(
            PortError::GateClosed {
                reason: "spool age",
                retry_after_ms: 1_000,
            }
            .to_string()
            .contains("spool age")
        );
    }
}
