//! The bounded cursor binding.
//!
//! A cursor is bounded because the walk is ordered: buckets before the current
//! one are exhausted and buckets after it are untouched, so only the current
//! bucket's per-shard positions are open — at most `signals x shards` entries.
//!
//! Encoding and signing belong to `aex_regional_http::cursor` (RS-17: one codec,
//! one `cur_` envelope, a 24-hour life and a timing-safe compare). This module
//! owns only the **binding**: the fields a cursor is bound to and the exhaustive
//! mismatch check, so a cursor that would mean something different on the next
//! request is rejected rather than silently reinterpreted.

use aex_observation_domain::order::{Direction, OrderBy, OrderTuple};
use aex_observation_domain::signal::{Signal, SignalSet};
use aex_wire::ids::{SessionId, WorkspaceId};
use aex_wire::types::{Region, Timestamp};

use crate::coverage::Snapshot;

/// Which revisions a trace read returns.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TraceRevisionMode {
    /// The highest revision at or below the snapshot.
    LatestAtSnapshot,
    /// Every revision.
    All,
}

/// The position a walk resumes from inside one segment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SegmentPosition {
    /// Which signal the segment carries.
    pub signal: Signal,
    /// Which shard of the bucket.
    pub shard: u8,
    /// The last sort key delivered from this shard.
    pub last_sk: Box<str>,
}

/// Everything a cursor is bound to.
///
/// Twelve fields, each of which changes what the next page would mean. A
/// mismatch on any of them is `400 invalid_cursor`; there is no "best effort"
/// resume.
#[derive(Clone, Debug, PartialEq)]
pub struct ObservationCursorBinding {
    /// The route that issued it.
    pub route: Box<str>,
    /// The region that issued it.
    pub region: Region,
    /// The owning workspace.
    pub workspace: WorkspaceId,
    /// The session, when the query was session-scoped.
    pub session: Option<SessionId>,
    /// The signals the query selected.
    pub signals: SignalSet,
    /// The digest of the normalized query.
    pub query_digest: [u8; 32],
    /// Which component the walk is keyed on.
    pub order_by: OrderBy,
    /// Which direction the walk runs.
    pub direction: Direction,
    /// Which revisions a trace read returns.
    pub trace_revisions: TraceRevisionMode,
    /// The pinned snapshot.
    pub snapshot: Snapshot,
    /// The scope deletion epoch at issue time.
    pub deletion_epoch: u64,
    /// The bucket the walk is inside.
    pub bucket: Box<str>,
    /// The open per-shard positions, at most `signals x shards`.
    pub positions: Vec<SegmentPosition>,
    /// The last tuple delivered.
    pub last_tuple: OrderTuple,
    /// When the cursor was issued.
    pub issued_at: Timestamp,
}

/// Why a cursor was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CursorError {
    /// A bound field does not match the current request.
    #[error("the cursor is bound to a different {field}")]
    Mismatch {
        /// Which bound field differs.
        field: &'static str,
    },
    /// The cursor is past its 24-hour life.
    #[error("the cursor expired")]
    Expired,
    /// The scope was deleted and re-created since the cursor was issued.
    #[error("the cursor is bound to a superseded deletion epoch")]
    DeletionEpochAdvanced,
    /// The cursor carries more open positions than the walk can have.
    #[error("the cursor carries {observed} open positions, above the {limit} bound")]
    Unbounded {
        /// The measured count.
        observed: usize,
        /// The bound.
        limit: usize,
    },
}

/// The life of a cursor.
pub const CURSOR_LIFETIME_MS: i64 = 24 * 60 * 60 * 1_000;

/// The largest number of shards one bucket may be spread over.
pub const MAX_SHARDS: usize = 64;

impl ObservationCursorBinding {
    /// Checks this cursor against the request that presented it.
    ///
    /// # Errors
    ///
    /// Returns [`CursorError::Mismatch`] naming the first bound field that
    /// differs, [`CursorError::Expired`] past the 24-hour life,
    /// [`CursorError::DeletionEpochAdvanced`] when the scope was deleted, and
    /// [`CursorError::Unbounded`] when the position list is larger than an
    /// ordered walk could produce.
    pub fn check(&self, request: &Self, now: Timestamp) -> Result<(), CursorError> {
        let checks: [(&'static str, bool); 9] = [
            ("route", self.route == request.route),
            ("region", self.region == request.region),
            ("workspace", self.workspace == request.workspace),
            ("session", self.session == request.session),
            ("signals", self.signals == request.signals),
            ("query", self.query_digest == request.query_digest),
            ("order", self.order_by == request.order_by),
            ("direction", self.direction == request.direction),
            (
                "trace revision mode",
                self.trace_revisions == request.trace_revisions,
            ),
        ];
        for (field, matched) in checks {
            if !matched {
                return Err(CursorError::Mismatch { field });
            }
        }
        if self.snapshot != request.snapshot {
            return Err(CursorError::Mismatch { field: "snapshot" });
        }
        if self.deletion_epoch != request.deletion_epoch {
            return Err(CursorError::DeletionEpochAdvanced);
        }
        if now
            .unix_millis()
            .saturating_sub(self.issued_at.unix_millis())
            >= CURSOR_LIFETIME_MS
        {
            return Err(CursorError::Expired);
        }
        let limit = self.signals.len() * MAX_SHARDS;
        if self.positions.len() > limit {
            return Err(CursorError::Unbounded {
                observed: self.positions.len(),
                limit,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CURSOR_LIFETIME_MS, CursorError, ObservationCursorBinding, SegmentPosition,
        TraceRevisionMode,
    };
    use crate::coverage::Snapshot;
    use aex_observation_domain::order::{Direction, OrderBy, OrderTuple};
    use aex_observation_domain::signal::{Signal, SignalSet};
    use aex_wire::ids::PrefixedId as _;
    use aex_wire::types::{Region, Timestamp};

    fn instant(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("representable")
    }

    fn binding() -> ObservationCursorBinding {
        ObservationCursorBinding {
            route: "observations_logs_query".into(),
            region: Region::ALL[0],
            workspace: aex_wire::ids::WorkspaceId::parse("wsp_0000000002e81840g2081040g2")
                .expect("fixture parses"),
            session: Some(
                aex_wire::ids::SessionId::parse("ses_0000000003ec1r60r30c1g60r3")
                    .expect("fixture parses"),
            ),
            signals: SignalSet::from_signal(Signal::Logs),
            query_digest: [7; 32],
            order_by: OrderBy::Time,
            direction: Direction::Ascending,
            trace_revisions: TraceRevisionMode::LatestAtSnapshot,
            snapshot: Snapshot::pin(instant(1_000), instant(1_000_000)).expect("pins"),
            deletion_epoch: 3,
            bucket: "2026-08-01T09".into(),
            positions: vec![SegmentPosition {
                signal: Signal::Logs,
                shard: 0,
                last_sk: "00000000000000000007".into(),
            }],
            last_tuple: OrderTuple::new(
                instant(10),
                Signal::Logs,
                aex_wire::ids::ObservationId::parse("obs_0000000001e40r2081040g2081")
                    .expect("fixture parses"),
                0,
            ),
            issued_at: instant(1_000),
        }
    }

    #[test]
    fn an_identical_binding_resumes() {
        let cursor = binding();
        cursor
            .check(&binding(), instant(2_000))
            .expect("an identical binding resumes");
    }

    #[test]
    fn a_mismatch_on_any_bound_field_is_refused_by_name() {
        let cursor = binding();

        let mut route = binding();
        route.route = "observations_spans_query".into();
        assert_eq!(
            cursor.check(&route, instant(2_000)),
            Err(CursorError::Mismatch { field: "route" })
        );

        let mut signals = binding();
        signals.signals = SignalSet::all();
        assert_eq!(
            cursor.check(&signals, instant(2_000)),
            Err(CursorError::Mismatch { field: "signals" })
        );

        let mut digest = binding();
        digest.query_digest = [8; 32];
        assert_eq!(
            cursor.check(&digest, instant(2_000)),
            Err(CursorError::Mismatch { field: "query" })
        );

        let mut order = binding();
        order.order_by = OrderBy::Accepted;
        assert_eq!(
            cursor.check(&order, instant(2_000)),
            Err(CursorError::Mismatch { field: "order" })
        );

        let mut direction = binding();
        direction.direction = Direction::Descending;
        assert_eq!(
            cursor.check(&direction, instant(2_000)),
            Err(CursorError::Mismatch { field: "direction" })
        );

        let mut revisions = binding();
        revisions.trace_revisions = TraceRevisionMode::All;
        assert_eq!(
            cursor.check(&revisions, instant(2_000)),
            Err(CursorError::Mismatch {
                field: "trace revision mode"
            })
        );

        let mut snapshot = binding();
        snapshot.snapshot = Snapshot::pin(instant(2_000), instant(1_000_000)).expect("pins");
        assert_eq!(
            cursor.check(&snapshot, instant(2_000)),
            Err(CursorError::Mismatch { field: "snapshot" })
        );

        let mut session = binding();
        session.session = None;
        assert_eq!(
            cursor.check(&session, instant(2_000)),
            Err(CursorError::Mismatch { field: "session" })
        );
    }

    #[test]
    fn a_changed_deletion_epoch_is_refused() {
        let cursor = binding();
        let mut advanced = binding();
        advanced.deletion_epoch = 4;
        assert_eq!(
            cursor.check(&advanced, instant(2_000)),
            Err(CursorError::DeletionEpochAdvanced)
        );
    }

    #[test]
    fn the_twenty_four_hour_boundary_is_exact() {
        let cursor = binding();
        cursor
            .check(&binding(), instant(1_000 + CURSOR_LIFETIME_MS - 1))
            .expect("one millisecond inside the life resumes");
        assert_eq!(
            cursor.check(&binding(), instant(1_000 + CURSOR_LIFETIME_MS)),
            Err(CursorError::Expired)
        );
    }

    #[test]
    fn a_position_list_larger_than_an_ordered_walk_could_produce_is_refused() {
        let mut oversized = binding();
        oversized.positions = (0..=64u16)
            .map(|shard| SegmentPosition {
                signal: Signal::Logs,
                shard: u8::try_from(shard % 256).expect("fits"),
                last_sk: "0".into(),
            })
            .collect();
        assert!(matches!(
            oversized.check(&binding(), instant(2_000)),
            Err(CursorError::Unbounded { .. })
        ));
    }
}
