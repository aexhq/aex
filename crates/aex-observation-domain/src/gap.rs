//! Explicit telemetry gaps.
//!
//! A gap is the durable statement "this window is known to be incomplete". It is
//! never a silent omission and never disappears: every change appends a new
//! immutable revision, and a repair records its provenance rather than erasing
//! the history.
//!
//! Decision O-19 lives here: [`PRODUCIBLE_REASONS`] is every reason this
//! codebase can mint, and it deliberately excludes
//! [`TelemetryGapReason::ReplayExpired`]. Removing Kinesis removed the only
//! mechanism that could expire a replay; the wire keeps the value for
//! vocabulary stability and [`GapRevision::try_open`] refuses it outright.

use aex_wire::ids::{TelemetryGapId, WorkspaceId};
use aex_wire::models::TelemetryGapReason;
use aex_wire::types::Timestamp;

use crate::signal::SignalSet;

/// Every gap reason this codebase can produce.
///
/// `replay_expired` is absent by construction, not by convention.
pub const PRODUCIBLE_REASONS: &[TelemetryGapReason] = &[
    TelemetryGapReason::AdmissionRejected,
    TelemetryGapReason::SpoolLost,
    TelemetryGapReason::ProducerDropped,
    TelemetryGapReason::AuthorityUnavailable,
];

/// Why a gap could not be constructed or appended.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum GapError {
    /// A gap without a signal could never intersect a query.
    #[error("a gap must affect at least one concrete signal")]
    EmptySignals,
    /// A workspace-scoped key was paired with another workspace owner.
    #[error("workspace scope {scope} does not match owning workspace {workspace}")]
    WorkspaceMismatch {
        /// Workspace encoded by the scope.
        scope: WorkspaceId,
        /// Workspace supplied by the owning record.
        workspace: WorkspaceId,
    },
    /// The reason is in the wire vocabulary but is not one this codebase mints.
    #[error("`{reason}` is not a reason this codebase can produce")]
    UnproducibleReason {
        /// The refused wire spelling.
        reason: &'static str,
    },
    /// A revision was appended at or below the ledger's highest revision.
    #[error("revision {attempted} does not follow revision {latest}")]
    RevisionNotMonotone {
        /// The highest revision already recorded.
        latest: u64,
        /// The revision that was offered.
        attempted: u64,
    },
}

/// Where one gap revision sits in its life.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum GapState {
    /// The producer will retry; the window may yet fill itself.
    PendingRetry,
    /// The window is known incomplete and no retry is outstanding.
    Open,
    /// The window was filled; `repair_source` names how.
    Repaired,
}

impl GapState {
    /// Every state, in declared order.
    pub const ALL: &'static [GapState] =
        &[GapState::PendingRetry, GapState::Open, GapState::Repaired];

    /// The durable spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PendingRetry => "pending_retry",
            Self::Open => "open",
            Self::Repaired => "repaired",
        }
    }

    /// Whether a query intersecting this revision is incomplete.
    #[must_use]
    pub const fn is_open(self) -> bool {
        matches!(self, Self::PendingRetry | Self::Open)
    }
}

/// A half-open observation-time window, `[from, to)`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TimeWindow {
    from: Timestamp,
    to: Timestamp,
}

impl TimeWindow {
    /// Builds a non-empty window, or `None` when the bounds are equal or inverted.
    #[must_use]
    pub fn new(from: Timestamp, to: Timestamp) -> Option<Self> {
        (from < to).then_some(Self { from, to })
    }

    /// The inclusive lower bound.
    #[must_use]
    pub const fn from(self) -> Timestamp {
        self.from
    }

    /// The exclusive upper bound.
    #[must_use]
    pub const fn to(self) -> Timestamp {
        self.to
    }

    /// Whether the two windows share at least one instant.
    ///
    /// Both are half-open, so windows that merely touch at a bound do not
    /// intersect — which is what makes adjacent hour buckets non-overlapping.
    #[must_use]
    pub fn intersects(self, other: Self) -> bool {
        self.from < other.to && other.from < self.to
    }

    /// Whether an instant falls inside.
    #[must_use]
    pub fn contains(self, at: Timestamp) -> bool {
        self.from <= at && at < self.to
    }
}

/// An inclusive range of accepted ordinals.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OrdinalRange {
    lo: u64,
    hi: u64,
}

impl OrdinalRange {
    /// Builds a range, or `None` when the bounds are inverted.
    #[must_use]
    pub const fn new(lo: u64, hi: u64) -> Option<Self> {
        if lo > hi { None } else { Some(Self { lo, hi }) }
    }

    /// The inclusive lower bound.
    #[must_use]
    pub const fn lo(self) -> u64 {
        self.lo
    }

    /// The inclusive upper bound.
    #[must_use]
    pub const fn hi(self) -> u64 {
        self.hi
    }

    /// Whether an ordinal falls inside.
    #[must_use]
    pub const fn contains(self, ordinal: u64) -> bool {
        self.lo <= ordinal && ordinal <= self.hi
    }

    /// How many ordinals the range covers.
    #[must_use]
    pub const fn len(self) -> u64 {
        self.hi - self.lo + 1
    }

    /// Never: an [`OrdinalRange`] always covers at least one ordinal.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        false
    }
}

/// One immutable revision of one gap.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GapRevision {
    /// Stable identity across revisions.
    pub gap_id: TelemetryGapId,
    /// Monotonic within the gap; the first revision is `0`.
    pub revision: u64,
    /// Where the revision sits in its life.
    pub state: GapState,
    /// Which signals the hole affects.
    pub signals: SignalSet,
    /// Why the hole exists.
    pub reason: TelemetryGapReason,
    /// The affected observation-time window, when it is known.
    pub time_range: Option<TimeWindow>,
    /// The affected accepted ordinals, when they are known.
    pub ordinal_range: Option<OrdinalRange>,
    /// Whether the extent of the loss is unknown.
    ///
    /// An unbounded gap is reported through `unboundedGaps` and never invents a
    /// `missingInterval`: no API fabricates an interval it cannot prove.
    pub unbounded: bool,
    /// When this revision was written.
    pub revised_at: Timestamp,
    /// When the gap was first opened.
    pub opened_at: Timestamp,
    /// What repaired the gap, on a `repaired` revision.
    pub repair_source: Option<Box<str>>,
}

/// One scoped durable gap revision and its accounting evidence.
///
/// Scope and workspace are deliberately carried together: a session identifier
/// does not encode its owning workspace, while the workspace gap index needs
/// that owner on every revision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GapRecord {
    /// Owning workspace used by the sparse workspace index.
    pub workspace: WorkspaceId,
    /// Session or direct-workspace scope affected by the gap.
    pub scope: crate::keys::ScopeKey,
    /// Immutable lifecycle revision.
    pub revision: GapRevision,
    /// Number of attempted observations, when known.
    pub attempted_records: Option<u64>,
    /// Number of attempted canonical bytes, when known.
    pub attempted_bytes: Option<u64>,
    /// Whether retained evidence can still repair the gap.
    pub recoverable: bool,
}

impl GapRecord {
    /// Binds one revision to its owning workspace and scope.
    ///
    /// # Errors
    ///
    /// Returns [`GapError::WorkspaceMismatch`] when a direct workspace scope is
    /// paired with another workspace, or [`GapError::EmptySignals`] when the
    /// revision could never intersect a query.
    pub fn try_new(
        workspace: WorkspaceId,
        scope: crate::keys::ScopeKey,
        revision: GapRevision,
        attempted_records: Option<u64>,
        attempted_bytes: Option<u64>,
        recoverable: bool,
    ) -> Result<Self, GapError> {
        if revision.signals.is_empty() {
            return Err(GapError::EmptySignals);
        }
        if let crate::keys::ScopeKey::Workspace(scope_workspace) = scope
            && scope_workspace != workspace
        {
            return Err(GapError::WorkspaceMismatch {
                scope: scope_workspace,
                workspace,
            });
        }
        Ok(Self {
            workspace,
            scope,
            revision,
            attempted_records,
            attempted_bytes,
            recoverable,
        })
    }
}

impl GapRevision {
    /// Opens a gap.
    ///
    /// # Errors
    ///
    /// Returns [`GapError::UnproducibleReason`] for any reason outside
    /// [`PRODUCIBLE_REASONS`].
    pub fn try_open(
        gap_id: TelemetryGapId,
        signals: SignalSet,
        reason: TelemetryGapReason,
        time_range: Option<TimeWindow>,
        at: Timestamp,
    ) -> Result<Self, GapError> {
        if signals.is_empty() {
            return Err(GapError::EmptySignals);
        }
        if !PRODUCIBLE_REASONS.contains(&reason) {
            return Err(GapError::UnproducibleReason {
                reason: reason.as_str(),
            });
        }
        Ok(Self {
            gap_id,
            revision: 0,
            state: GapState::Open,
            signals,
            reason,
            time_range,
            ordinal_range: None,
            unbounded: time_range.is_none(),
            revised_at: at,
            opened_at: at,
            repair_source: None,
        })
    }

    /// Opens a gap with a reason this codebase can produce.
    ///
    /// # Panics
    ///
    /// Panics when `reason` is outside [`PRODUCIBLE_REASONS`]. Callers holding
    /// an untrusted reason use [`GapRevision::try_open`].
    #[must_use]
    pub fn open(
        gap_id: TelemetryGapId,
        signals: SignalSet,
        reason: TelemetryGapReason,
        time_range: Option<TimeWindow>,
        at: Timestamp,
    ) -> Self {
        Self::try_open(gap_id, signals, reason, time_range, at)
            .expect("`open` is only called with a producible reason")
    }

    /// Narrows the gap to an exact accepted-ordinal range.
    #[must_use]
    pub fn with_ordinals(mut self, range: OrdinalRange) -> Self {
        self.ordinal_range = Some(range);
        self
    }

    /// The next revision, carrying every field forward unchanged.
    #[must_use]
    pub fn revised(&self, at: Timestamp) -> Self {
        Self {
            revision: self.revision + 1,
            revised_at: at,
            ..self.clone()
        }
    }

    /// The revision that records the repair and its provenance.
    #[must_use]
    pub fn repaired(&self, source: &str, at: Timestamp) -> Self {
        Self {
            revision: self.revision + 1,
            state: GapState::Repaired,
            revised_at: at,
            repair_source: Some(source.into()),
            ..self.clone()
        }
    }

    /// Whether this revision makes a query over `signals` and `window`
    /// incomplete.
    #[must_use]
    pub fn affects(&self, signals: SignalSet, window: TimeWindow) -> bool {
        if !self.state.is_open() || !self.signals.intersects(signals) {
            return false;
        }
        match self.time_range {
            // An unbounded gap has no provable extent, so it affects every
            // window over its signals. Claiming otherwise would be a guess.
            None => true,
            Some(range) => range.intersects(window),
        }
    }
}

/// Every revision of every gap in one scope.
#[derive(Clone, Debug, Default)]
pub struct GapLedger {
    revisions: std::collections::BTreeMap<TelemetryGapId, Vec<GapRevision>>,
}

impl GapLedger {
    /// An empty ledger.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends one revision.
    ///
    /// # Errors
    ///
    /// Returns [`GapError::RevisionNotMonotone`] when the offered revision does
    /// not strictly follow the highest one recorded, which is how a replayed
    /// write is rejected rather than silently duplicating history.
    pub fn append(&mut self, revision: GapRevision) -> Result<(), GapError> {
        let entry = self.revisions.entry(revision.gap_id).or_default();
        if let Some(latest) = entry.last() {
            if latest.revision.checked_add(1) != Some(revision.revision) {
                return Err(GapError::RevisionNotMonotone {
                    latest: latest.revision,
                    attempted: revision.revision,
                });
            }
        } else if revision.revision != 0 {
            return Err(GapError::RevisionNotMonotone {
                latest: 0,
                attempted: revision.revision,
            });
        }
        entry.push(revision);
        Ok(())
    }

    /// Every recorded revision of one gap, oldest first.
    #[must_use]
    pub fn revisions(&self, gap_id: &TelemetryGapId) -> &[GapRevision] {
        self.revisions.get(gap_id).map_or(&[], Vec::as_slice)
    }

    /// The highest recorded revision of one gap.
    #[must_use]
    pub fn latest(&self, gap_id: &TelemetryGapId) -> Option<&GapRevision> {
        self.revisions.get(gap_id).and_then(|list| list.last())
    }

    /// The latest revision of every gap, in gap-id order.
    pub fn latest_all(&self) -> impl Iterator<Item = &GapRevision> {
        self.revisions.values().filter_map(|list| list.last())
    }

    /// Every open gap with a known window that intersects the query.
    pub fn intersecting_open(
        &self,
        signals: SignalSet,
        window: TimeWindow,
    ) -> impl Iterator<Item = &GapRevision> {
        self.latest_all().filter(move |revision| {
            revision.affects(signals, window) && revision.time_range.is_some()
        })
    }

    /// Every open gap over these signals whose extent is unknown.
    pub fn unbounded_open(
        &self,
        signals: SignalSet,
        window: TimeWindow,
    ) -> impl Iterator<Item = &GapRevision> {
        self.latest_all()
            .filter(move |revision| revision.affects(signals, window) && revision.unbounded)
    }

    /// The reportable missing intervals for a query.
    ///
    /// An unbounded gap contributes nothing here by construction; it is reported
    /// through [`GapLedger::unbounded_open`] instead.
    #[must_use]
    pub fn missing_intervals(
        &self,
        signals: SignalSet,
        window: TimeWindow,
    ) -> Vec<(TelemetryGapId, TimeWindow)> {
        self.intersecting_open(signals, window)
            .filter_map(|revision| revision.time_range.map(|range| (revision.gap_id, range)))
            .collect()
    }

    /// Whether the query window has no known hole.
    #[must_use]
    pub fn is_complete(&self, signals: SignalSet, window: TimeWindow) -> bool {
        !self
            .latest_all()
            .any(|revision| revision.affects(signals, window))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GapError, GapLedger, GapRevision, GapState, OrdinalRange, PRODUCIBLE_REASONS, TimeWindow,
    };
    use crate::signal::{Signal, SignalSet};
    use aex_wire::ids::{PrefixedId, TelemetryGapId};
    use aex_wire::models::TelemetryGapReason;
    use aex_wire::types::Timestamp;

    fn gap_id() -> TelemetryGapId {
        PrefixedId::parse("gap_0000000001e40r2081040g2081").expect("fixture parses")
    }

    fn instant(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("representable")
    }

    #[test]
    fn the_producible_set_excludes_the_removed_replay_horizon() {
        assert_eq!(PRODUCIBLE_REASONS.len(), 4);
        assert!(!PRODUCIBLE_REASONS.contains(&TelemetryGapReason::ReplayExpired));
    }

    #[test]
    fn a_first_revision_must_be_zero() {
        let mut ledger = GapLedger::new();
        let mut revision = GapRevision::open(
            gap_id(),
            SignalSet::from_signal(Signal::Logs),
            TelemetryGapReason::SpoolLost,
            None,
            instant(0),
        );
        revision.revision = 3;
        assert_eq!(
            ledger.append(revision),
            Err(GapError::RevisionNotMonotone {
                latest: 0,
                attempted: 3
            })
        );
    }

    #[test]
    fn an_ordinal_narrowing_is_carried_forward_by_a_revision() {
        let range = OrdinalRange::new(4, 8).expect("ordered");
        let opened = GapRevision::open(
            gap_id(),
            SignalSet::from_signal(Signal::Logs),
            TelemetryGapReason::SpoolLost,
            None,
            instant(0),
        )
        .with_ordinals(range);
        let next = opened.revised(instant(1));
        assert_eq!(next.ordinal_range, Some(range));
        assert_eq!(next.state, GapState::Open);
        assert!(!range.is_empty());
        assert_eq!(range.lo(), 4);
        assert_eq!(range.hi(), 8);
    }

    #[test]
    fn a_gap_window_must_not_be_empty() {
        assert_eq!(TimeWindow::new(instant(7), instant(7)), None);
    }

    #[test]
    fn revisions_must_be_contiguous() {
        let mut ledger = GapLedger::new();
        let opened = GapRevision::open(
            gap_id(),
            SignalSet::from_signal(Signal::Logs),
            TelemetryGapReason::SpoolLost,
            None,
            instant(0),
        );
        ledger
            .append(opened.clone())
            .expect("revision zero appends");
        let mut skipped = opened.revised(instant(2));
        skipped.revision = 2;
        assert_eq!(
            ledger.append(skipped),
            Err(GapError::RevisionNotMonotone {
                latest: 0,
                attempted: 2,
            })
        );
    }

    #[test]
    fn the_state_spellings_are_stable() {
        assert_eq!(GapState::ALL.len(), 3);
        assert_eq!(GapState::PendingRetry.as_str(), "pending_retry");
        assert!(GapState::PendingRetry.is_open());
        assert!(!GapState::Repaired.is_open());
    }
}
