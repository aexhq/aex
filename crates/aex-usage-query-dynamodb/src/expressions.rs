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
    Generation, Grain, MAX_GENERATION, ProjectionKey, ProjectionKeyError, ProjectionKeys,
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
    /// The read did not reach the projection, or the service refused it for a
    /// reason that may not recur.
    ///
    /// Every call this crate makes is a read, so there is no ambiguous-commit
    /// arm here: nothing was written, whatever the outcome.
    #[error("the usage projection is unavailable: {reason}")]
    Unavailable {
        /// The service's own rendering, or why the request never arrived.
        reason: String,
    },
    /// The caller's role is denied the read.
    ///
    /// Separated from [`QueryError::Unavailable`] because a denial never
    /// improves by being retried: it is a deployment fact, not a transient one.
    #[error("the caller is denied the usage projection read")]
    Denied,
    /// The configured table does not exist.
    ///
    /// This is a composition failure and never a customer `404`; an empty
    /// projection answers with no rows, not with a missing table.
    #[error("`{table}` does not exist; the composition is misconfigured")]
    Misconfigured {
        /// The physical table the read was issued against.
        table: String,
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

/// Everything one bounded aggregate read needs.
///
/// Grouped into a struct rather than passed positionally: a caller cannot
/// transpose the two bucket bounds, or the workspace and the month, without the
/// type system noticing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AggregateRequest<'a> {
    /// The generation the read is pinned to.
    pub generation: Generation,
    /// The workspace being read.
    pub workspace: &'a WorkspaceId,
    /// The public category being read.
    pub category: PublicCategory,
    /// The `YYYY-MM` partition.
    pub month: &'a str,
    /// Hourly or daily rollups.
    pub granularity: Granularity,
    /// The inclusive lower bucket bound.
    pub from_bucket: &'a str,
    /// The exclusive upper bucket bound.
    pub until_bucket: &'a str,
    /// The row budget.
    pub limit: usize,
    /// Where the previous page stopped.
    pub after: Option<String>,
}

/// A bounded keyset read over one partition's coarse rollups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoarsePage {
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

/// Everything one bounded coarse read needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoarseRequest<'a> {
    /// The generation the read is pinned to.
    pub generation: Generation,
    /// The workspace being read.
    pub workspace: &'a WorkspaceId,
    /// The public category being read.
    pub category: PublicCategory,
    /// The `YYYY-MM` partition.
    pub month: &'a str,
    /// Which stored grain to read.
    pub grain: Grain,
    /// The inclusive lower bucket bound.
    pub from_bucket: &'a str,
    /// The exclusive upper bucket bound.
    pub until_bucket: &'a str,
    /// The row budget.
    pub limit: usize,
    /// Where the previous page stopped.
    pub after: Option<String>,
}

/// One decoded coarse rollup.
///
/// It carries no dimension identity, because the row does not have one. That
/// absence is the whole reason this face can be read online.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoarseRow {
    /// Which public category the quantity belongs to.
    pub public_category: PublicCategory,
    /// The grain the bucket is at.
    pub grain: Grain,
    /// The bucket label.
    pub bucket: String,
    /// The accumulated quantity.
    pub quantity: Quantity,
    /// The highest accepted sequence folded into this row.
    pub highest_sequence: AcceptedSequence,
}

impl CoarseRow {
    /// Whether this row is covered by a committed settlement receipt.
    ///
    /// Computed from two numbers already on rows the query reads, so labelling
    /// costs no extra storage and no extra read. Because coverage is read
    /// eventually consistently, a stale `settled` can mark a genuinely settled
    /// item `provisional` — the safe direction. The label never claims settled
    /// when it is not.
    #[must_use]
    pub const fn is_settled(&self, settled: AcceptedSequence) -> bool {
        self.highest_sequence.get() <= settled.get()
    }
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
        request: &AggregateRequest<'_>,
    ) -> Result<AggregatePage, QueryError> {
        if request.limit == 0 || request.limit > MAX_PAGE_ROWS {
            return Err(QueryError::PageBudget {
                requested: request.limit,
            });
        }
        if request.until_bucket <= request.from_bucket {
            return Err(QueryError::InvertedRange);
        }
        let partition = self.keys.aggregate_partition(
            request.generation,
            request.workspace,
            request.category,
            request.month,
        )?;
        Ok(AggregatePage {
            partition,
            from: format!("{}{}", request.granularity.prefix(), request.from_bucket),
            until: format!("{}{}", request.granularity.prefix(), request.until_bucket),
            limit: request.limit,
            after: request.after.clone(),
        })
    }

    /// A bounded page over one month of coarse rollups.
    ///
    /// This is the face the customer read answers from. Unlike
    /// [`ProjectionReads::aggregate_page`] its row count is not data-dependent:
    /// the coarse face has exactly one row per `(category, bucket)`, so the
    /// caller already knows the exact upper bound before issuing the read.
    ///
    /// # Errors
    ///
    /// Returns [`QueryError::PageBudget`] above [`MAX_PAGE_ROWS`],
    /// [`QueryError::InvertedRange`] for an empty or inverted range, and
    /// [`QueryError::Key`] when the partition cannot be built.
    pub fn coarse_page(self, request: &CoarseRequest<'_>) -> Result<CoarsePage, QueryError> {
        if request.limit == 0 || request.limit > MAX_PAGE_ROWS {
            return Err(QueryError::PageBudget {
                requested: request.limit,
            });
        }
        if request.until_bucket <= request.from_bucket {
            return Err(QueryError::InvertedRange);
        }
        let partition = self.keys.aggregate_partition(
            request.generation,
            request.workspace,
            request.category,
            request.month,
        )?;
        Ok(CoarsePage {
            partition,
            from: format!("{}{}", request.grain.coarse_prefix(), request.from_bucket),
            until: format!("{}{}", request.grain.coarse_prefix(), request.until_bucket),
            limit: request.limit,
            after: request.after.clone(),
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
        ProjectionReads::new().aggregate_page(&super::AggregateRequest {
            generation: Generation::FIRST,
            workspace: &workspace(),
            category: PublicCategory::Compute,
            month: "2026-08",
            granularity: Granularity::Hourly,
            from_bucket: "2026-08-01T00",
            until_bucket: "2026-08-02T00",
            limit,
            after: None,
        })
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
            ProjectionReads::new().aggregate_page(&super::AggregateRequest {
                generation: Generation::FIRST,
                workspace: &workspace(),
                category: PublicCategory::Storage,
                month: "2026-08",
                granularity: Granularity::Daily,
                from_bucket: from,
                until_bucket: until,
                limit: 10,
                after: None,
            })
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
        let outcome = ProjectionReads::new().aggregate_page(&super::AggregateRequest {
            generation: Generation::FIRST,
            workspace: &workspace(),
            category: PublicCategory::Storage,
            month: "2026-8",
            granularity: Granularity::Daily,
            from_bucket: "2026-08-01",
            until_bucket: "2026-08-02",
            limit: 10,
            after: None,
        });
        assert!(matches!(outcome, Err(QueryError::Key(_))));
    }
}
