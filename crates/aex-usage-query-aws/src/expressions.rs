//! The read side of `usage-query-projection`.
//!
//! The key grammar itself lives in [`aex_usage_domain::projection`] because the
//! writer and the reader must agree about where a row lives, and a second copy
//! is exactly how they would stop agreeing. This module re-exports it and adds
//! the bounded read expressions: a keyset page over one partition, the coverage
//! vector, and the generation pointer.
//!
//! Nothing here writes. That is proved rather than asserted — see
//! `tests/write_incapability.rs` for the source-conformance and link-graph
//! halves of `U-20`.

pub use aex_usage_domain::projection::{
    Generation, MAX_GENERATION, ProjectionKey, ProjectionKeyError, ProjectionKeys,
};

use aex_usage_domain::frontier::{AcceptedSequence, FrontierState, PoisonReason};
use aex_usage_domain::meter::PublicCategory;
use aex_usage_domain::quantity::Quantity;
use aex_usage_domain::wire_pending::{Timestamp, WorkspaceId};

/// The largest number of rows one page may read.
///
/// A read that cannot answer inside this budget returns a cursor rather than a
/// bigger scan: an unbounded group-by over a busy workspace is the one way this
/// table can turn a customer request into an outage.
pub const MAX_PAGE_ROWS: usize = 500;

/// Why a read could not be built or decoded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QueryError {
    /// A key could not be built.
    #[error(transparent)]
    Key(#[from] ProjectionKeyError),
    /// The requested page size exceeded the hard budget.
    #[error("page size {requested} exceeds the {MAX_PAGE_ROWS} row budget")]
    PageBudget {
        /// What the caller asked for.
        requested: usize,
    },
    /// The half-open range was inverted or empty.
    #[error("the requested range ends at or before it starts")]
    InvertedRange,
    /// A row was missing an attribute the projected set declares.
    #[error("projected row is missing required attribute `{attribute}`")]
    MissingAttribute {
        /// The attribute that was absent.
        attribute: &'static str,
    },
    /// A row could not be decoded.
    #[error("attribute `{attribute}` is malformed: {reason}")]
    MalformedAttribute {
        /// The attribute that was refused.
        attribute: &'static str,
        /// Why it was refused.
        reason: String,
    },
    /// A row carried an item type this reader does not serve.
    #[error("expected item type `{expected}` but the row carries `{actual}`")]
    ItemTypeMismatch {
        /// What was asked for.
        expected: &'static str,
        /// What the row says it is.
        actual: String,
    },
}

/// How a page is bucketed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Granularity {
    /// One row per hour per dimension tuple.
    Hourly,
    /// One row per day per dimension tuple.
    Daily,
}

impl Granularity {
    /// The sort-key prefix this granularity occupies.
    #[must_use]
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::Hourly => "H#",
            Self::Daily => "D#",
        }
    }
}

/// A bounded keyset read over one aggregate partition.
///
/// The partition is always fully qualified — generation, workspace, public
/// category and month — so a read can never straddle two workspaces or two
/// generations however the caller builds its query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AggregatePage {
    /// The partition to read.
    pub partition: String,
    /// The inclusive lower sort-key bound.
    pub from: String,
    /// The exclusive upper sort-key bound.
    pub until: String,
    /// The largest number of rows this read may return.
    pub limit: usize,
    /// Where the previous page stopped, when there was one.
    pub after: Option<String>,
}

/// Read-only expression builders.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProjectionReads {
    keys: ProjectionKeys,
}

impl ProjectionReads {
    /// A fresh builder.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            keys: ProjectionKeys,
        }
    }

    /// The key grammar this reader is built on.
    #[must_use]
    pub const fn keys(self) -> ProjectionKeys {
        self.keys
    }

    /// A bounded page over one month of aggregates.
    ///
    /// # Errors
    ///
    /// Returns [`QueryError::PageBudget`] above [`MAX_PAGE_ROWS`],
    /// [`QueryError::InvertedRange`] for an empty or inverted range, and
    /// [`QueryError::Key`] when the partition cannot be built.
    pub fn aggregate_page(
        self,
        generation: Generation,
        workspace: &WorkspaceId,
        category: PublicCategory,
        month: &str,
        granularity: Granularity,
        from_bucket: &str,
        until_bucket: &str,
        limit: usize,
        after: Option<String>,
    ) -> Result<AggregatePage, QueryError> {
        if limit == 0 || limit > MAX_PAGE_ROWS {
            return Err(QueryError::PageBudget { requested: limit });
        }
        if until_bucket <= from_bucket {
            return Err(QueryError::InvertedRange);
        }
        let partition = self
            .keys
            .aggregate_partition(generation, workspace, category, month)?;
        Ok(AggregatePage {
            partition,
            from: format!("{}{from_bucket}", granularity.prefix()),
            until: format!("{}{until_bucket}", granularity.prefix()),
            limit,
            after,
        })
    }

    /// The coverage vector key for one workspace and category.
    ///
    /// # Errors
    ///
    /// Returns [`QueryError::Key`] when the key cannot be built.
    pub fn coverage(
        self,
        generation: Generation,
        workspace: &WorkspaceId,
        category: PublicCategory,
    ) -> Result<ProjectionKey, QueryError> {
        Ok(self.keys.coverage(generation, workspace, category)?)
    }

    /// The generation pointer key.
    #[must_use]
    pub fn generation_pointer(self) -> ProjectionKey {
        self.keys.generation_pointer()
    }
}

/// One decoded aggregate row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AggregateRow {
    /// Which public category the quantity belongs to.
    pub public_category: PublicCategory,
    /// The meter identifier, absent for a zero-dollar observability rollup.
    pub meter: Option<String>,
    /// The half-open bucket start.
    pub bucket_start: String,
    /// The half-open bucket end.
    pub bucket_end: String,
    /// The accumulated quantity.
    pub quantity: Quantity,
    /// How many facts contributed.
    pub fact_count: u64,
    /// The declared dimension tuple, stored so a reader never inverts the hash.
    pub dimensions: Vec<(String, String)>,
    /// The highest accepted sequence folded into this row.
    pub highest_sequence: AcceptedSequence,
}

/// The copied frontier one category's page reports.
///
/// This is what lets a customer read distinguish "you used nothing" from "we
/// have not folded your facts yet". A page without it would be confidently empty
/// rather than honestly incomplete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageRow {
    /// Which public category this coverage is for.
    pub public_category: PublicCategory,
    /// The last fact admitted by the authority.
    pub accepted: AcceptedSequence,
    /// The last fact folded into this projection.
    pub projected: AcceptedSequence,
    /// The last fact delivered to central settlement.
    pub published: AcceptedSequence,
    /// The last fact covered by a committed settlement receipt.
    pub settled: AcceptedSequence,
    /// How far service time is known to be complete.
    pub service_through: Option<Timestamp>,
    /// Whether the frontier is advancing or parked.
    pub state: FrontierState,
}

impl CoverageRow {
    /// Whether this category's fold is parked behind a poisoned record.
    #[must_use]
    pub const fn is_stalled(&self) -> bool {
        matches!(self.state, FrontierState::Quarantined { .. })
    }

    /// Why the fold is parked, when it is.
    #[must_use]
    pub const fn stall_reason(&self) -> Option<PoisonReason> {
        match self.state {
            FrontierState::Quarantined { reason, .. } => Some(reason),
            FrontierState::Advancing => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Generation, Granularity, MAX_PAGE_ROWS, ProjectionReads, QueryError};
    use aex_usage_domain::frontier::{AcceptedSequence, FrontierState, PoisonReason};
    use aex_usage_domain::meter::PublicCategory;
    use aex_usage_domain::wire_pending::WorkspaceId;

    fn workspace() -> WorkspaceId {
        WorkspaceId::parse("ws-1").expect("workspace")
    }

    fn page(limit: usize) -> Result<super::AggregatePage, QueryError> {
        ProjectionReads::new().aggregate_page(
            Generation::FIRST,
            &workspace(),
            PublicCategory::Compute,
            "2026-08",
            Granularity::Hourly,
            "2026-08-01T00",
            "2026-08-02T00",
            limit,
            None,
        )
    }

    #[test]
    fn a_page_is_fully_qualified_so_it_cannot_straddle_a_workspace_or_a_generation() {
        let page = page(100).expect("builds");
        assert_eq!(page.partition, "G0000#ws-1#compute#2026-08");
        assert_eq!(page.from, "H#2026-08-01T00");
        assert_eq!(page.until, "H#2026-08-02T00");
        assert_eq!(page.limit, 100);
        assert!(page.after.is_none());
    }

    #[test]
    fn the_two_granularities_occupy_disjoint_sort_key_space() {
        let hourly = Granularity::Hourly.prefix();
        let daily = Granularity::Daily.prefix();
        assert_ne!(hourly, daily);
        assert!(
            !hourly.starts_with(daily) && !daily.starts_with(hourly),
            "an hourly range must never pick up a daily rollup and double-count"
        );
    }

    #[test]
    fn a_page_beyond_the_row_budget_is_refused_rather_than_scanned() {
        assert!(matches!(
            page(MAX_PAGE_ROWS + 1),
            Err(QueryError::PageBudget { .. })
        ));
        assert!(matches!(page(0), Err(QueryError::PageBudget { .. })));
        assert!(page(MAX_PAGE_ROWS).is_ok());
    }

    #[test]
    fn an_inverted_or_empty_range_is_refused() {
        let build = |from: &str, until: &str| {
            ProjectionReads::new().aggregate_page(
                Generation::FIRST,
                &workspace(),
                PublicCategory::Storage,
                "2026-08",
                Granularity::Daily,
                from,
                until,
                10,
                None,
            )
        };
        assert!(matches!(
            build("2026-08-02", "2026-08-01"),
            Err(QueryError::InvertedRange)
        ));
        assert!(matches!(
            build("2026-08-01", "2026-08-01"),
            Err(QueryError::InvertedRange)
        ));
        assert!(build("2026-08-01", "2026-08-02").is_ok());
    }

    #[test]
    fn coverage_reports_a_stall_rather_than_an_empty_page() {
        let advancing = super::CoverageRow {
            public_category: PublicCategory::Memory,
            accepted: AcceptedSequence::new(9).expect("positive"),
            projected: AcceptedSequence::new(9).expect("positive"),
            published: AcceptedSequence::new(9).expect("positive"),
            settled: AcceptedSequence::new(4).expect("positive"),
            service_through: None,
            state: FrontierState::Advancing,
        };
        assert!(!advancing.is_stalled());
        assert!(advancing.stall_reason().is_none());

        let parked = super::CoverageRow {
            state: FrontierState::Quarantined {
                at: AcceptedSequence::new(10).expect("positive"),
                reason: PoisonReason::Undecodable,
            },
            ..advancing
        };
        assert!(parked.is_stalled());
        assert_eq!(parked.stall_reason(), Some(PoisonReason::Undecodable));
    }

    #[test]
    fn a_malformed_month_still_fails_through_the_shared_grammar() {
        let outcome = ProjectionReads::new().aggregate_page(
            Generation::FIRST,
            &workspace(),
            PublicCategory::Storage,
            "2026-8",
            Granularity::Daily,
            "2026-08-01",
            "2026-08-02",
            10,
            None,
        );
        assert!(matches!(outcome, Err(QueryError::Key(_))));
    }
}
