//! Query form to `DynamoDB` access pattern, and the budgets that bound it.
//!
//! `plan` is a **total function** from a normalized query plus a segment
//! directory plus a pinned snapshot to a [`Plan`]. There is no fallback, no
//! scan, and no `Scan` API call anywhere in this crate — a scan is how a bounded
//! query engine silently becomes an unbounded one.
//!
//! Removing the column store genuinely costs two things and §4.8 names them
//! without euphemism: wide metric aggregation and wide filtering over
//! non-indexed fields become **budget-bounded**. A budget-exhausted page returns
//! a short page *with a cursor*, which is correct and resumable. Only a first
//! segment that cannot yield a single item returns
//! `telemetry_query_budget_exhausted`.

use aex_observation_domain::limits;
use aex_observation_domain::order::{Direction, OrderBy};
use aex_observation_domain::signal::{Signal, SignalSet};

use crate::ast::{Predicate, QueryError};

/// Which scope a query reads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScopeAxis {
    /// A session, or a workspace-key observation with no session.
    Scope,
    /// Every session in the workspace.
    Workspace,
}

/// The index a walk reads.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    /// The base table, strongly consistent, accepted-ordered.
    BaseTable,
    /// `gsi_scope_time`.
    ScopeTime,
    /// `gsi_ws_accepted`.
    WorkspaceAccepted,
    /// `gsi_ws_time`.
    WorkspaceTime,
    /// `gsi_metric`, a point-partition read per UTC day.
    Metric,
    /// `gsi_trace`, one partition per `(scope, traceId)`.
    Trace,
    /// `gsi_gap`, the workspace gap axis.
    Gap,
    /// `session-authority`, through the read-only `SemanticEventSource` port.
    SessionAuthority,
}

impl Access {
    /// The index name, or `None` for the base table and the peer authority.
    #[must_use]
    pub const fn index_name(self) -> Option<&'static str> {
        match self {
            Self::BaseTable | Self::SessionAuthority => None,
            Self::ScopeTime => Some("gsi_scope_time"),
            Self::WorkspaceAccepted => Some("gsi_ws_accepted"),
            Self::WorkspaceTime => Some("gsi_ws_time"),
            Self::Metric => Some("gsi_metric"),
            Self::Trace => Some("gsi_trace"),
            Self::Gap => Some("gsi_gap"),
        }
    }

    /// Whether a read from this access is strongly consistent.
    ///
    /// Only the base table is. Everything else is settle-windowed, which is
    /// exactly what the pinned snapshot makes exact rather than hidden.
    #[must_use]
    pub const fn is_strongly_consistent(self) -> bool {
        matches!(self, Self::BaseTable)
    }
}

/// The budgets one page is read under.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Budget {
    /// The requested page size.
    pub max_returned: u16,
    /// Index items read, **not** items returned. This is what makes a wide
    /// `contains` bounded instead of unbounded.
    pub max_items_scanned: u32,
    /// Segments a page may open.
    pub max_segments: u16,
    /// Bytes a page may read.
    pub max_bytes_read: u64,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            max_returned: limits::QUERY_DEFAULT_LIMIT,
            max_items_scanned: limits::QUERY_MAX_ITEMS_SCANNED,
            max_segments: limits::QUERY_MAX_SEGMENTS,
            max_bytes_read: limits::QUERY_MAX_BYTES_READ,
        }
    }
}

/// Which budget dimension ended a page.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Dimension {
    /// The requested page size was reached — the ordinary case.
    Returned,
    /// The scanned-item budget was reached.
    ScannedItems,
    /// The segment budget was reached.
    Segments,
    /// The read-byte budget was reached.
    Bytes,
    /// The request deadline was reached.
    Deadline,
}

impl Dimension {
    /// The wire spelling reported in `ErrorDetails.dimension`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Returned => "returned",
            Self::ScannedItems => "scanned_items",
            Self::Segments => "segments",
            Self::Bytes => "read_bytes",
            Self::Deadline => "deadline",
        }
    }
}

/// What one page's read actually consumed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Spend {
    /// How many rows were emitted.
    pub returned: u32,
    /// How many index items were read.
    pub items_scanned: u32,
    /// How many segments were opened.
    pub segments: u16,
    /// How many bytes were read.
    pub bytes_read: u64,
    /// Whether the request deadline was reached.
    pub deadline_reached: bool,
}

/// How a page ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageOutcome {
    /// The walk finished the range; there is nothing after this page.
    Complete,
    /// The page ended early and a cursor resumes it. Correct and resumable.
    Short {
        /// Which dimension ended it.
        dimension: Dimension,
    },
    /// The first segment could make no progress at all.
    ///
    /// The only case that becomes `409 telemetry_query_budget_exhausted`. A
    /// page that yielded even one row is never this.
    NoProgress {
        /// Which dimension blocked it.
        dimension: Dimension,
        /// The measured scan.
        scanned: u32,
        /// The remedy the error names.
        remedy: &'static str,
    },
}

impl PageOutcome {
    /// Whether the caller must return a cursor.
    #[must_use]
    pub const fn needs_cursor(self) -> bool {
        matches!(self, Self::Short { .. })
    }

    /// Whether the caller must return `409 telemetry_query_budget_exhausted`.
    #[must_use]
    pub const fn is_error(self) -> bool {
        matches!(self, Self::NoProgress { .. })
    }
}

/// Classifies how a page ended.
///
/// A short page with a cursor is the honest answer and is contract-legal. An
/// error is reserved for the case where progress is genuinely impossible; a
/// query is never silently capped.
#[must_use]
pub fn classify(budget: &Budget, spend: &Spend, exhausted_range: bool) -> PageOutcome {
    let dimension = if spend.deadline_reached {
        Some(Dimension::Deadline)
    } else if spend.items_scanned >= budget.max_items_scanned {
        Some(Dimension::ScannedItems)
    } else if spend.bytes_read >= budget.max_bytes_read {
        Some(Dimension::Bytes)
    } else if spend.segments >= budget.max_segments {
        Some(Dimension::Segments)
    } else if u32::from(budget.max_returned) <= spend.returned {
        Some(Dimension::Returned)
    } else {
        None
    };

    match dimension {
        None if exhausted_range => PageOutcome::Complete,
        None => PageOutcome::Short {
            dimension: Dimension::Returned,
        },
        Some(dimension) if spend.returned > 0 => PageOutcome::Short { dimension },
        Some(Dimension::Returned) => PageOutcome::Complete,
        Some(dimension) => PageOutcome::NoProgress {
            dimension,
            scanned: spend.items_scanned,
            remedy: "narrow the time range, add an indexed predicate, or use an export",
        },
    }
}

/// A normalized, planned query.
#[derive(Clone, Debug, PartialEq)]
pub struct NormalizedQuery {
    /// Which scope axis.
    pub axis: ScopeAxis,
    /// Which signals.
    pub signals: SignalSet,
    /// The resolved filter.
    pub predicate: Option<Predicate>,
    /// The inclusive lower bound of the half-open observation-time window.
    pub time_gte: aex_wire::types::Timestamp,
    /// The exclusive upper bound.
    pub time_lt: aex_wire::types::Timestamp,
    /// Which component the walk is keyed on.
    pub order_by: OrderBy,
    /// Which direction the walk runs.
    pub direction: Direction,
    /// The page size.
    pub limit: u16,
    /// An exact trace, when the query names one.
    pub trace_id: Option<Box<str>>,
    /// An exact metric name, when the query names one.
    pub metric_name: Option<Box<str>>,
}

impl NormalizedQuery {
    /// Validates the shape a normalized query must have.
    ///
    /// # Errors
    ///
    /// Returns [`QueryError::TimeRange`] for an inverted or empty window and
    /// [`QueryError::Bound`] for a page size above the registered ceiling.
    pub fn validate(&self) -> Result<(), QueryError> {
        if self.time_gte >= self.time_lt {
            return Err(QueryError::TimeRange);
        }
        if self.limit == 0 || self.limit > limits::QUERY_MAX_LIMIT {
            return Err(QueryError::Bound {
                what: "page size",
                observed: usize::from(self.limit),
                limit: usize::from(limits::QUERY_MAX_LIMIT),
            });
        }
        Ok(())
    }
}

/// One walk the plan will perform.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Walk {
    /// Which signal it reads.
    pub signal: Signal,
    /// Which access pattern.
    pub access: Access,
}

/// The complete plan for one page.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// The walks, merged by the ordering tuple.
    pub walks: Vec<Walk>,
    /// Whether the residual predicate needs a base-table `BatchGetItem`.
    pub needs_base_fetch: bool,
    /// The budgets the page reads under.
    pub budget: Budget,
}

/// Plans one normalized query.
///
/// Total: every query form in the accepted table maps to exactly one access
/// pattern per signal, and nothing falls through to a scan.
///
/// # Errors
///
/// Returns whatever [`NormalizedQuery::validate`] rejects.
pub fn plan(query: &NormalizedQuery, budget: Budget) -> Result<Plan, QueryError> {
    query.validate()?;

    let walks = query
        .signals
        .iter()
        .map(|signal| Walk {
            signal,
            access: access_for(query, signal),
        })
        .collect();

    Ok(Plan {
        walks,
        needs_base_fetch: query
            .predicate
            .as_ref()
            .is_some_and(Predicate::has_residual),
        budget: Budget {
            max_returned: query.limit,
            ..budget
        },
    })
}

fn access_for(query: &NormalizedQuery, signal: Signal) -> Access {
    // `events` never live in `observation-authority`; they are read through the
    // `SemanticEventSource` port over `session-authority` (decision O-01).
    if signal == Signal::Events {
        return Access::SessionAuthority;
    }
    // Sparse indexes are observation-time ordered. Accepted-order queries must
    // stay on the accepted-time authority/index so each segment is monotonic in
    // the tuple the reader merges.
    if query.order_by == OrderBy::Time
        && query.trace_id.is_some()
        && matches!(signal, Signal::Spans | Signal::Traces)
    {
        return Access::Trace;
    }
    if query.order_by == OrderBy::Time && query.metric_name.is_some() && signal == Signal::Metrics {
        return Access::Metric;
    }
    match (query.axis, query.order_by) {
        (ScopeAxis::Scope, OrderBy::Time) => Access::ScopeTime,
        // Accepted order at scope axis is the base table, strongly consistent:
        // the stream tail and the durable cursor cannot tolerate index lag.
        (ScopeAxis::Scope, OrderBy::Accepted) => Access::BaseTable,
        (ScopeAxis::Workspace, OrderBy::Time) => Access::WorkspaceTime,
        (ScopeAxis::Workspace, OrderBy::Accepted) => Access::WorkspaceAccepted,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Access, Budget, Dimension, NormalizedQuery, PageOutcome, ScopeAxis, Spend, classify, plan,
    };
    use crate::ast::{FieldRef, Predicate, QueryError, TextOp};
    use aex_observation_domain::order::{Direction, OrderBy};
    use aex_observation_domain::signal::{Signal, SignalSet};
    use aex_wire::types::Timestamp;

    fn instant(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("representable")
    }

    fn query(axis: ScopeAxis, order_by: OrderBy, signals: SignalSet) -> NormalizedQuery {
        NormalizedQuery {
            axis,
            signals,
            predicate: None,
            time_gte: instant(0),
            time_lt: instant(1_000),
            order_by,
            direction: Direction::Ascending,
            limit: 100,
            trace_id: None,
            metric_name: None,
        }
    }

    #[test]
    fn every_declared_query_form_selects_its_exact_access_pattern() {
        let logs = SignalSet::from_signal(Signal::Logs);
        let cases = [
            (ScopeAxis::Scope, OrderBy::Time, Access::ScopeTime),
            (ScopeAxis::Scope, OrderBy::Accepted, Access::BaseTable),
            (ScopeAxis::Workspace, OrderBy::Time, Access::WorkspaceTime),
            (
                ScopeAxis::Workspace,
                OrderBy::Accepted,
                Access::WorkspaceAccepted,
            ),
        ];
        for (axis, order_by, expected) in cases {
            let planned = plan(&query(axis, order_by, logs), Budget::default()).expect("plans");
            assert_eq!(planned.walks[0].access, expected, "{axis:?}/{order_by:?}");
        }
        assert!(Access::BaseTable.is_strongly_consistent());
        assert!(!Access::ScopeTime.is_strongly_consistent());
        assert_eq!(Access::ScopeTime.index_name(), Some("gsi_scope_time"));
        assert_eq!(Access::BaseTable.index_name(), None);
    }

    #[test]
    fn events_are_read_from_the_session_authority_and_never_from_this_table() {
        let planned = plan(
            &query(
                ScopeAxis::Scope,
                OrderBy::Time,
                SignalSet::from_signal(Signal::Events),
            ),
            Budget::default(),
        )
        .expect("plans");
        assert_eq!(planned.walks[0].access, Access::SessionAuthority);
    }

    #[test]
    fn an_exact_trace_or_metric_name_selects_its_sparse_index() {
        let mut spans = query(
            ScopeAxis::Scope,
            OrderBy::Time,
            SignalSet::from_signal(Signal::Spans),
        );
        spans.trace_id = Some("aabb".into());
        assert_eq!(
            plan(&spans, Budget::default()).expect("plans").walks[0].access,
            Access::Trace
        );

        let mut metrics = query(
            ScopeAxis::Workspace,
            OrderBy::Time,
            SignalSet::from_signal(Signal::Metrics),
        );
        metrics.metric_name = Some("http.server.duration".into());
        assert_eq!(
            plan(&metrics, Budget::default()).expect("plans").walks[0].access,
            Access::Metric
        );
    }

    #[test]
    fn accepted_order_never_uses_an_observation_time_sparse_index() {
        let mut spans = query(
            ScopeAxis::Scope,
            OrderBy::Accepted,
            SignalSet::from_signal(Signal::Spans),
        );
        spans.trace_id = Some("aabb".into());
        assert_eq!(
            plan(&spans, Budget::default()).expect("plans").walks[0].access,
            Access::BaseTable
        );

        let mut metrics = query(
            ScopeAxis::Workspace,
            OrderBy::Accepted,
            SignalSet::from_signal(Signal::Metrics),
        );
        metrics.metric_name = Some("http.server.duration".into());
        assert_eq!(
            plan(&metrics, Budget::default()).expect("plans").walks[0].access,
            Access::WorkspaceAccepted
        );
    }

    #[test]
    fn several_signals_produce_one_walk_each_in_rank_order() {
        let planned = plan(
            &query(ScopeAxis::Scope, OrderBy::Time, SignalSet::all()),
            Budget::default(),
        )
        .expect("plans");
        assert_eq!(planned.walks.len(), 5);
        let signals: Vec<Signal> = planned.walks.iter().map(|walk| walk.signal).collect();
        assert_eq!(signals, Signal::ALL.to_vec());
    }

    #[test]
    fn a_residual_predicate_requires_a_base_table_fetch() {
        let mut residual = query(
            ScopeAxis::Scope,
            OrderBy::Time,
            SignalSet::from_signal(Signal::Logs),
        );
        residual.predicate = Some(Predicate::Text {
            field: FieldRef::Attribute("route".into()),
            op: TextOp::Contains,
            value: "runs".into(),
        });
        assert!(
            plan(&residual, Budget::default())
                .expect("plans")
                .needs_base_fetch
        );

        let mut indexed = query(
            ScopeAxis::Scope,
            OrderBy::Time,
            SignalSet::from_signal(Signal::Logs),
        );
        indexed.predicate = Some(Predicate::Exists {
            field: FieldRef::Signal(Signal::Logs, "logger".into()),
        });
        assert!(
            !plan(&indexed, Budget::default())
                .expect("plans")
                .needs_base_fetch
        );
    }

    #[test]
    fn an_inverted_range_or_an_oversized_page_is_refused() {
        let mut inverted = query(
            ScopeAxis::Scope,
            OrderBy::Time,
            SignalSet::from_signal(Signal::Logs),
        );
        inverted.time_lt = inverted.time_gte;
        assert_eq!(
            plan(&inverted, Budget::default()),
            Err(QueryError::TimeRange)
        );

        let mut oversized = query(
            ScopeAxis::Scope,
            OrderBy::Time,
            SignalSet::from_signal(Signal::Logs),
        );
        oversized.limit = 1_001;
        assert!(matches!(
            plan(&oversized, Budget::default()),
            Err(QueryError::Bound { .. })
        ));
    }

    #[test]
    fn each_exhausted_budget_ends_the_page_short_with_a_cursor() {
        let budget = Budget {
            max_returned: 100,
            max_items_scanned: 1_000,
            max_segments: 4,
            max_bytes_read: 1_024,
        };
        let cases = [
            (
                Spend {
                    returned: 5,
                    items_scanned: 1_000,
                    ..Spend::default()
                },
                Dimension::ScannedItems,
            ),
            (
                Spend {
                    returned: 5,
                    bytes_read: 1_024,
                    ..Spend::default()
                },
                Dimension::Bytes,
            ),
            (
                Spend {
                    returned: 5,
                    segments: 4,
                    ..Spend::default()
                },
                Dimension::Segments,
            ),
            (
                Spend {
                    returned: 5,
                    deadline_reached: true,
                    ..Spend::default()
                },
                Dimension::Deadline,
            ),
        ];
        for (spend, dimension) in cases {
            let outcome = classify(&budget, &spend, false);
            assert_eq!(outcome, PageOutcome::Short { dimension });
            assert!(outcome.needs_cursor());
            assert!(!outcome.is_error());
        }
    }

    #[test]
    fn only_a_no_progress_first_segment_becomes_the_typed_error() {
        let budget = Budget {
            max_returned: 100,
            max_items_scanned: 1_000,
            max_segments: 64,
            max_bytes_read: 1 << 20,
        };
        let stuck = Spend {
            returned: 0,
            items_scanned: 1_000,
            ..Spend::default()
        };
        let outcome = classify(&budget, &stuck, false);
        assert!(outcome.is_error());
        assert!(!outcome.needs_cursor());
        match outcome {
            PageOutcome::NoProgress {
                dimension,
                scanned,
                remedy,
            } => {
                assert_eq!(dimension, Dimension::ScannedItems);
                assert_eq!(dimension.as_str(), "scanned_items");
                assert_eq!(scanned, 1_000);
                assert!(remedy.contains("export"));
            }
            other => panic!("expected a no-progress outcome, got {other:?}"),
        }
    }

    #[test]
    fn an_exhausted_range_inside_every_budget_is_complete() {
        let budget = Budget::default();
        let spend = Spend {
            returned: 3,
            ..Spend::default()
        };
        assert_eq!(classify(&budget, &spend, true), PageOutcome::Complete);
        // A full page over a range that is not exhausted is short, not complete.
        let full = Spend {
            returned: u32::from(budget.max_returned),
            ..Spend::default()
        };
        assert_eq!(
            classify(&budget, &full, false),
            PageOutcome::Short {
                dimension: Dimension::Returned
            }
        );
    }
}
