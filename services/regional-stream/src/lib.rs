//! Read-only regional stream admission, quotas, authoritative wakes and drain.

pub mod config;

pub use config::Config;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use aex_wire::routes::{Plane, RouteId, TransportKind, route};

/// Configured wake mechanism. Both modes read the authority after a hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeMode {
    /// Shared `DynamoDB` Streams readers used only as hints.
    DdbStreams,
    /// Adaptive authority polling.
    Poll,
}

impl WakeMode {
    /// Parses configuration and enforces the two-reader task ceiling.
    ///
    /// # Errors
    ///
    /// Returns [`StreamConfigError`] for an unknown mode or more than two tasks
    /// in `ddb_streams` mode.
    pub fn parse(value: &str, max_tasks: u32) -> Result<Self, StreamConfigError> {
        match value {
            "ddb_streams" if max_tasks <= 2 => Ok(Self::DdbStreams),
            "ddb_streams" => Err(StreamConfigError::TooManyDdbStreamTasks),
            "poll" => Ok(Self::Poll),
            _ => Err(StreamConfigError::UnknownWakeMode),
        }
    }
}

/// Startup denial for stream configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StreamConfigError {
    /// `AEX_STREAM_WAKE_MODE` was not closed vocabulary.
    #[error("unknown stream wake mode")]
    UnknownWakeMode,
    /// `DynamoDB` Streams mode exceeded the two-task reader guidance.
    #[error("ddb_streams wake mode supports at most two tasks")]
    TooManyDdbStreamTasks,
}

/// Validated replay origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamOrigin {
    /// Resume after one signed cursor.
    Cursor(String),
    /// Begin at one timestamp in Unix milliseconds.
    Time(i64),
    /// Begin from the earliest retained record.
    Earliest,
    /// `listen` captures the current accepted position without replay.
    Listen,
}

impl StreamOrigin {
    /// Requires exactly one origin for a `stream` route.
    ///
    /// # Errors
    ///
    /// Returns [`OriginError`] unless exactly one field is present.
    pub fn for_stream(
        cursor: Option<&str>,
        time_ms: Option<i64>,
        earliest: bool,
    ) -> Result<Self, OriginError> {
        let count =
            usize::from(cursor.is_some()) + usize::from(time_ms.is_some()) + usize::from(earliest);
        if count != 1 {
            return Err(OriginError::ExactlyOneRequired);
        }
        if let Some(cursor) = cursor {
            return Ok(Self::Cursor(cursor.to_owned()));
        }
        if let Some(time_ms) = time_ms {
            return Ok(Self::Time(time_ms));
        }
        Ok(Self::Earliest)
    }

    /// Rejects every replay origin on a `listen` route.
    ///
    /// # Errors
    ///
    /// Returns [`OriginError::ListenForbidsOrigin`] when any field is present.
    pub fn for_listen(
        cursor: Option<&str>,
        time_ms: Option<i64>,
        earliest: bool,
    ) -> Result<Self, OriginError> {
        if cursor.is_some() || time_ms.is_some() || earliest {
            return Err(OriginError::ListenForbidsOrigin);
        }
        Ok(Self::Listen)
    }
}

/// Invalid stream/listen origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OriginError {
    /// A stream had zero or multiple origins.
    #[error("stream requires exactly one origin")]
    ExactlyOneRequired,
    /// Listen route named a replay origin.
    #[error("listen does not accept a replay origin")]
    ListenForbidsOrigin,
}

/// Models a wake followed by an authoritative read strictly after `sent`.
///
/// Wake payload/order/count is intentionally unused: a wake is only a latency hint.
#[must_use]
pub fn authoritative_after_wakes(authority: &[u64], sent: u64, _wakes: &[u64]) -> Vec<u64> {
    authority
        .iter()
        .copied()
        .filter(|position| *position > sent)
        .collect()
}

/// Connection budget class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionClass {
    /// Session events.
    Session,
    /// Logs, spans, metrics and traces.
    Observation,
    /// Unified telemetry, charged to both budgets.
    Telemetry,
}

/// Per-task and per-workspace hard caps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaLimits {
    /// Total sockets.
    pub total: u32,
    /// Session-class sockets.
    pub session: u32,
    /// Observation-class sockets.
    pub observation: u32,
    /// Sockets for one workspace.
    pub per_workspace: u32,
}

#[derive(Debug, Default)]
struct Counts {
    total: u32,
    session: u32,
    observation: u32,
    workspaces: BTreeMap<String, u32>,
}

/// Thread-safe quota authority; reservations release in `Drop` on every exit path.
#[derive(Clone)]
pub struct QuotaManager {
    limits: QuotaLimits,
    counts: Arc<Mutex<Counts>>,
}

impl QuotaManager {
    /// Validates non-zero, internally consistent limits.
    ///
    /// # Errors
    ///
    /// Returns [`QuotaError::InvalidLimits`] for zero or class caps above total.
    pub fn new(limits: QuotaLimits) -> Result<Self, QuotaError> {
        if limits.total == 0
            || limits.session == 0
            || limits.observation == 0
            || limits.per_workspace == 0
            || limits.session > limits.total
            || limits.observation > limits.total
        {
            return Err(QuotaError::InvalidLimits);
        }
        Ok(Self {
            limits,
            counts: Arc::new(Mutex::new(Counts::default())),
        })
    }

    /// Reserves all relevant class tokens atomically.
    ///
    /// # Errors
    ///
    /// Returns a typed capacity refusal without partially reserving.
    pub fn reserve(
        &self,
        workspace: impl Into<String>,
        class: ConnectionClass,
    ) -> Result<Reservation, QuotaError> {
        let workspace = workspace.into();
        if workspace.is_empty() {
            return Err(QuotaError::InvalidWorkspace);
        }
        let mut counts = self
            .counts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let workspace_count = counts.workspaces.get(&workspace).copied().unwrap_or(0);
        if counts.total >= self.limits.total {
            return Err(QuotaError::TotalCapacity);
        }
        if workspace_count >= self.limits.per_workspace {
            return Err(QuotaError::WorkspaceCapacity);
        }
        if matches!(class, ConnectionClass::Session | ConnectionClass::Telemetry)
            && counts.session >= self.limits.session
        {
            return Err(QuotaError::SessionCapacity);
        }
        if matches!(
            class,
            ConnectionClass::Observation | ConnectionClass::Telemetry
        ) && counts.observation >= self.limits.observation
        {
            return Err(QuotaError::ObservationCapacity);
        }
        counts.total += 1;
        if matches!(class, ConnectionClass::Session | ConnectionClass::Telemetry) {
            counts.session += 1;
        }
        if matches!(
            class,
            ConnectionClass::Observation | ConnectionClass::Telemetry
        ) {
            counts.observation += 1;
        }
        *counts.workspaces.entry(workspace.clone()).or_default() += 1;
        drop(counts);
        Ok(Reservation {
            counts: Arc::clone(&self.counts),
            workspace,
            class,
        })
    }

    /// `(total, session, observation)` for readiness and tests.
    #[must_use]
    pub fn counts(&self) -> (u32, u32, u32) {
        let counts = self
            .counts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (counts.total, counts.session, counts.observation)
    }
}

/// Reservation released automatically on disconnect, drain or task failure.
pub struct Reservation {
    counts: Arc<Mutex<Counts>>,
    workspace: String,
    class: ConnectionClass,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut counts = self
            .counts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        counts.total = counts.total.saturating_sub(1);
        if matches!(
            self.class,
            ConnectionClass::Session | ConnectionClass::Telemetry
        ) {
            counts.session = counts.session.saturating_sub(1);
        }
        if matches!(
            self.class,
            ConnectionClass::Observation | ConnectionClass::Telemetry
        ) {
            counts.observation = counts.observation.saturating_sub(1);
        }
        if let Some(workspace) = counts.workspaces.get_mut(&self.workspace) {
            *workspace = workspace.saturating_sub(1);
            if *workspace == 0 {
                counts.workspaces.remove(&self.workspace);
            }
        }
    }
}

/// Typed capacity refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum QuotaError {
    /// Limits were zero or inconsistent.
    #[error("stream quota limits are invalid")]
    InvalidLimits,
    /// Workspace id was empty.
    #[error("workspace identity is invalid")]
    InvalidWorkspace,
    /// Total task socket cap reached.
    #[error("stream total capacity exceeded")]
    TotalCapacity,
    /// Session class cap reached.
    #[error("stream session capacity exceeded")]
    SessionCapacity,
    /// Observation class cap reached.
    #[error("stream observation capacity exceeded")]
    ObservationCapacity,
    /// Per-workspace cap reached.
    #[error("stream workspace capacity exceeded")]
    WorkspaceCapacity,
}

/// Reconnect state emitted during drain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrainFrame {
    connection_id: u64,
    cursor: String,
}

impl DrainFrame {
    /// Exact last sent cursor for this connection.
    #[must_use]
    pub fn cursor(&self) -> &str {
        &self.cursor
    }
}

/// Readiness/drain registry for open sockets.
#[derive(Debug, Default)]
pub struct DrainCoordinator {
    ready: bool,
    cursors: BTreeMap<u64, String>,
}

impl DrainCoordinator {
    /// Starts ready with no sockets.
    #[must_use]
    pub fn new() -> Self {
        Self {
            ready: true,
            cursors: BTreeMap::new(),
        }
    }

    /// Tracks one connection's current sent cursor.
    pub fn register(&mut self, connection_id: u64, sent_cursor: impl Into<String>) {
        self.cursors.insert(connection_id, sent_cursor.into());
    }

    /// Whether new sockets may be admitted.
    #[must_use]
    pub const fn is_ready(&self) -> bool {
        self.ready
    }

    /// Flips readiness first, then snapshots exact reconnect state for every socket.
    pub fn begin(&mut self) -> Vec<DrainFrame> {
        self.ready = false;
        self.cursors
            .iter()
            .map(|(connection_id, cursor)| DrainFrame {
                connection_id: *connection_id,
                cursor: cursor.clone(),
            })
            .collect()
    }
}

/// All and only regional generated NDJSON operations.
#[must_use]
pub fn stream_route_ids() -> Vec<RouteId> {
    RouteId::ALL
        .iter()
        .copied()
        .filter(|id| {
            let descriptor = route(*id);
            descriptor.plane == Plane::Regional && descriptor.transport == TransportKind::Ndjson
        })
        .collect()
}
