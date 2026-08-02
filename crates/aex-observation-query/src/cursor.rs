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

use std::collections::BTreeSet;

use aex_observation_domain::keys::{BucketHour, ScopeKey};
use aex_observation_domain::order::{Direction, OrderBy, OrderTuple};
use aex_observation_domain::signal::{Signal, SignalSet};
use aex_wire::ids::{ObservationId, SessionId, WorkspaceId};
use aex_wire::types::{Region, Timestamp};
use serde::{Deserialize, Serialize};

use crate::coverage::Snapshot;
use crate::plan::Access;

/// Which revisions a trace read returns.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TraceRevisionMode {
    /// The highest revision at or below the snapshot.
    LatestAtSnapshot,
    /// Every revision.
    All,
}

/// The ordering tuple in compact cursor form.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResumeTuple {
    #[serde(rename = "p")]
    primary_ms: i64,
    #[serde(rename = "g")]
    signal: Signal,
    #[serde(rename = "o")]
    observation_id: String,
    #[serde(rename = "r")]
    revision: u64,
}

impl ResumeTuple {
    /// Captures one public ordering tuple.
    #[must_use]
    pub fn from_order(tuple: OrderTuple) -> Self {
        Self {
            primary_ms: tuple.primary.unix_millis(),
            signal: tuple.signal(),
            observation_id: tuple.observation_id.to_string(),
            revision: tuple.revision,
        }
    }

    /// Reconstructs and validates the public ordering tuple.
    ///
    /// # Errors
    ///
    /// Returns [`CursorError::InvalidState`] for an invalid timestamp or id.
    pub fn to_order(&self) -> Result<OrderTuple, CursorError> {
        let primary = Timestamp::from_unix_millis(self.primary_ms).map_err(|_| {
            CursorError::InvalidState {
                field: "last tuple",
            }
        })?;
        let observation_id = self.observation_id.parse::<ObservationId>().map_err(|_| {
            CursorError::InvalidState {
                field: "last tuple",
            }
        })?;
        Ok(OrderTuple::new(
            primary,
            self.signal,
            observation_id,
            self.revision,
        ))
    }
}

/// The minimum facts required to reconstruct one exact `DynamoDB` resume key.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResumeKey {
    /// The row's scope, needed when a workspace segment crosses sessions.
    #[serde(rename = "c")]
    pub scope: String,
    /// The provider-index ordering instant.
    #[serde(rename = "p")]
    pub primary_ms: i64,
    /// The base-table accepted ordering instant.
    #[serde(rename = "a")]
    pub accepted_ms: i64,
    /// Stable observation identity.
    #[serde(rename = "o")]
    pub observation_id: String,
    /// Immutable observation revision.
    #[serde(rename = "r")]
    pub revision: u64,
    /// Base-table shard, which differs from sparse-index fan-out.
    #[serde(rename = "h", default)]
    pub base_shard: u8,
    /// Native session-event sequence, absent for observation-authority rows.
    #[serde(rename = "e", skip_serializing_if = "Option::is_none")]
    pub event_seq: Option<u64>,
}

impl ResumeKey {
    /// Validates every typed key component before it reaches a provider call.
    ///
    /// # Errors
    ///
    /// Returns [`CursorError::InvalidState`] for invalid scope, timestamps, or
    /// observation identity.
    pub fn validate(&self) -> Result<(), CursorError> {
        ScopeKey::parse(&self.scope).map_err(|_| CursorError::InvalidState {
            field: "resume scope",
        })?;
        Timestamp::from_unix_millis(self.primary_ms).map_err(|_| CursorError::InvalidState {
            field: "resume time",
        })?;
        Timestamp::from_unix_millis(self.accepted_ms).map_err(|_| CursorError::InvalidState {
            field: "accepted time",
        })?;
        self.observation_id
            .parse::<ObservationId>()
            .map_err(|_| CursorError::InvalidState {
                field: "resume observation",
            })?;
        Ok(())
    }
}

/// Durable progress within one physical bucket segment.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SegmentState {
    /// The provider segment has not been opened.
    #[serde(rename = "u")]
    Unstarted,
    /// Resume strictly after this exact provider key.
    #[serde(rename = "a")]
    After(ResumeKey),
    /// The provider proved this segment exhausted.
    #[serde(rename = "e")]
    Exhausted,
}

/// One physical segment's independently authenticated progress.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SegmentResume {
    /// Which provider access rail is being walked.
    #[serde(rename = "a")]
    pub access: Access,
    /// Which signal the segment carries.
    #[serde(rename = "g")]
    pub signal: Signal,
    /// Which shard within the bucket.
    #[serde(rename = "h")]
    pub shard: u8,
    /// Durable consumed position.
    #[serde(rename = "s")]
    pub state: SegmentState,
}

/// Bounded, bucket-major continuation state for one observation page.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservationResume {
    /// The one bucket whose segments may still be open.
    pub bucket: String,
    /// The last tuple fully delivered to the caller.
    pub last_tuple: ResumeTuple,
    /// Every segment in the current bucket; earlier buckets are exhausted and
    /// later buckets are untouched.
    pub segments: Vec<SegmentResume>,
}

#[derive(Serialize, Deserialize)]
struct ObservationResumeWire(
    String,
    (i64, u8, String, u64),
    Vec<(u8, u8, u8, SegmentStateWire)>,
);

#[derive(Serialize, Deserialize)]
enum SegmentStateWire {
    #[serde(rename = "u")]
    Unstarted,
    #[serde(rename = "a")]
    After((String, i64, i64, String, u64, u8, Option<u64>)),
    #[serde(rename = "e")]
    Exhausted,
}

impl Serialize for ObservationResume {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let wire = ObservationResumeWire(
            self.bucket.clone(),
            (
                self.last_tuple.primary_ms,
                self.last_tuple.signal.rank(),
                self.last_tuple.observation_id.clone(),
                self.last_tuple.revision,
            ),
            self.segments
                .iter()
                .map(|segment| {
                    (
                        access_rank(segment.access),
                        segment.signal.rank(),
                        segment.shard,
                        match &segment.state {
                            SegmentState::Unstarted => SegmentStateWire::Unstarted,
                            SegmentState::Exhausted => SegmentStateWire::Exhausted,
                            SegmentState::After(key) => SegmentStateWire::After((
                                key.scope.clone(),
                                key.primary_ms,
                                key.accepted_ms,
                                key.observation_id.clone(),
                                key.revision,
                                key.base_shard,
                                key.event_seq,
                            )),
                        },
                    )
                })
                .collect(),
        );
        wire.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ObservationResume {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = ObservationResumeWire::deserialize(deserializer)?;
        let signal = signal_from_rank(wire.1.1)
            .ok_or_else(|| serde::de::Error::custom("invalid tuple signal"))?;
        let mut segments = Vec::with_capacity(wire.2.len());
        for (access, signal_rank, shard, state) in wire.2 {
            let access = access_from_rank(access)
                .ok_or_else(|| serde::de::Error::custom("invalid segment access"))?;
            let signal = signal_from_rank(signal_rank)
                .ok_or_else(|| serde::de::Error::custom("invalid segment signal"))?;
            let state = match state {
                SegmentStateWire::Unstarted => SegmentState::Unstarted,
                SegmentStateWire::Exhausted => SegmentState::Exhausted,
                SegmentStateWire::After((
                    scope,
                    primary_ms,
                    accepted_ms,
                    observation_id,
                    revision,
                    base_shard,
                    event_seq,
                )) => SegmentState::After(ResumeKey {
                    scope,
                    primary_ms,
                    accepted_ms,
                    observation_id,
                    revision,
                    base_shard,
                    event_seq,
                }),
            };
            segments.push(SegmentResume {
                access,
                signal,
                shard,
                state,
            });
        }
        Ok(Self {
            bucket: wire.0,
            last_tuple: ResumeTuple {
                primary_ms: wire.1.0,
                signal,
                observation_id: wire.1.2,
                revision: wire.1.3,
            },
            segments,
        })
    }
}

const fn access_rank(access: Access) -> u8 {
    match access {
        Access::BaseTable => 0,
        Access::ScopeTime => 1,
        Access::WorkspaceAccepted => 2,
        Access::WorkspaceTime => 3,
        Access::Metric => 4,
        Access::Trace => 5,
        Access::Gap => 6,
        Access::SessionAuthority => 7,
    }
}

const fn access_from_rank(rank: u8) -> Option<Access> {
    match rank {
        0 => Some(Access::BaseTable),
        1 => Some(Access::ScopeTime),
        2 => Some(Access::WorkspaceAccepted),
        3 => Some(Access::WorkspaceTime),
        4 => Some(Access::Metric),
        5 => Some(Access::Trace),
        6 => Some(Access::Gap),
        7 => Some(Access::SessionAuthority),
        _ => None,
    }
}

fn signal_from_rank(rank: u8) -> Option<Signal> {
    Signal::ALL
        .iter()
        .copied()
        .find(|signal| signal.rank() == rank)
}

impl ObservationResume {
    /// Builds and validates bounded continuation state.
    ///
    /// # Errors
    ///
    /// Returns [`CursorError`] for an invalid bucket, tuple, duplicate segment,
    /// or an open-position list above `signals x MAX_SHARDS`.
    pub fn new(
        bucket: BucketHour,
        last_tuple: OrderTuple,
        segments: Vec<SegmentResume>,
    ) -> Result<Self, CursorError> {
        let resume = Self {
            bucket: bucket.to_string(),
            last_tuple: ResumeTuple::from_order(last_tuple),
            segments,
        };
        resume.validate()?;
        Ok(resume)
    }

    /// Validates authenticated state after deserialization.
    ///
    /// # Errors
    ///
    /// Returns [`CursorError`] when any state component is invalid or unbounded.
    pub fn validate(&self) -> Result<(), CursorError> {
        BucketHour::parse(&self.bucket)
            .map_err(|_| CursorError::InvalidState { field: "bucket" })?;
        self.last_tuple.to_order()?;
        let limit = Signal::ALL.len() * MAX_SHARDS;
        if self.segments.len() > limit {
            return Err(CursorError::Unbounded {
                observed: self.segments.len(),
                limit,
            });
        }
        let mut coordinates = BTreeSet::new();
        for segment in &self.segments {
            if usize::from(segment.shard) >= MAX_SHARDS
                || !coordinates.insert((segment.access, segment.signal, segment.shard))
            {
                return Err(CursorError::InvalidState { field: "segments" });
            }
            if let SegmentState::After(key) = &segment.state {
                key.validate()?;
                if key.event_seq.is_some() != (segment.signal == Signal::Events) {
                    return Err(CursorError::InvalidState {
                        field: "segment key",
                    });
                }
            }
        }
        Ok(())
    }

    /// Parses the current bucket.
    ///
    /// # Errors
    ///
    /// Returns [`CursorError::InvalidState`] only if validation was skipped.
    pub fn parsed_bucket(&self) -> Result<BucketHour, CursorError> {
        BucketHour::parse(&self.bucket).map_err(|_| CursorError::InvalidState { field: "bucket" })
    }

    /// Reconstructs the last fully delivered tuple.
    ///
    /// # Errors
    ///
    /// Returns [`CursorError::InvalidState`] only if validation was skipped.
    pub fn last_order(&self) -> Result<OrderTuple, CursorError> {
        self.last_tuple.to_order()
    }
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
    /// Authenticated route state was structurally invalid.
    #[error("the cursor carries invalid {field}")]
    InvalidState {
        /// Which compact state component was invalid.
        field: &'static str,
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
        CURSOR_LIFETIME_MS, CursorError, ObservationCursorBinding, ObservationResume, ResumeKey,
        SegmentPosition, SegmentResume, SegmentState, TraceRevisionMode,
    };
    use crate::coverage::Snapshot;
    use crate::plan::Access;
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

    #[test]
    fn typed_resume_state_round_trips_exact_segment_positions() {
        let last = binding().last_tuple;
        let resume = ObservationResume::new(
            aex_observation_domain::keys::BucketHour::parse("2026-08-01T09").expect("bucket"),
            last,
            vec![SegmentResume {
                access: Access::ScopeTime,
                signal: Signal::Logs,
                shard: 2,
                state: SegmentState::After(ResumeKey {
                    scope: binding().session.map_or_else(
                        || format!("W#{}", binding().workspace),
                        |session| format!("S#{session}"),
                    ),
                    primary_ms: 10,
                    accepted_ms: 20,
                    observation_id: last.observation_id.to_string(),
                    revision: last.revision,
                    base_shard: 2,
                    event_seq: None,
                }),
            }],
        )
        .expect("resume validates");
        let encoded = serde_json::to_vec(&resume).expect("serializes");
        let decoded: ObservationResume = serde_json::from_slice(&encoded).expect("deserializes");
        decoded.validate().expect("authenticated state validates");
        assert_eq!(decoded.last_order().expect("tuple"), last);
        assert_eq!(decoded, resume);
    }

    #[test]
    fn duplicate_segment_coordinates_fail_closed() {
        let segment = SegmentResume {
            access: Access::ScopeTime,
            signal: Signal::Logs,
            shard: 0,
            state: SegmentState::Unstarted,
        };
        assert_eq!(
            ObservationResume::new(
                aex_observation_domain::keys::BucketHour::parse("2026-08-01T09").expect("bucket"),
                binding().last_tuple,
                vec![segment.clone(), segment],
            ),
            Err(CursorError::InvalidState { field: "segments" })
        );
    }
}
