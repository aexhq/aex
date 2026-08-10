//! The pure query plan: which partitions a usage read visits, in what order,
//! and exactly how many rows it may touch.
//!
//! Nothing here talks to a table, and that is the point. The plan is computed
//! **arithmetically, never discovered** — `partitions = requested categories ×
//! the UTC months the range intersects` — so the exact upper bound on rows is
//! known before the first read. That is what makes an over-budget query a
//! pre-flight refusal rather than a mid-read truncation, and it only holds
//! because the coarse face carries one row per `(category, bucket)` and no
//! dimension identity (`aex_usage_domain::projection::ProjectionKeys::coarse`).
//!
//! Three rules the shape enforces rather than documents:
//!
//! - **All bucketing is UTC and there is no timezone parameter.** A range must
//!   be aligned to whole buckets of the requested grain. A misaligned range is
//!   refused naming the required alignment; clamping it to the enclosing
//!   buckets would report a wider interval than was asked for, on a money read.
//! - **A `total` is never paged.** A partial total is a wrong number, so a
//!   `total` whose whole plan exceeds the row budget is refused naming the
//!   maximum span rather than returned with a cursor.
//! - **Sub-hour boundaries are unanswerable, by anyone.** The only rows fine
//!   enough are the detail rows, and they are the one row family with a TTL, so
//!   a sub-hour answer would be exact this quarter and silently absent next. A
//!   query whose exactness expires is worse than one that refuses on day one.

use aex_usage_domain::meter::PublicCategory;
use aex_usage_domain::projection::{Generation, Grain};
use aex_usage_domain::wire_pending::{Timestamp, WorkspaceId};

use crate::expressions::MAX_PAGE_ROWS;

/// The widest range a single query may span.
///
/// Slightly over a year, so "the last twelve months" is expressible with room
/// for the caller's month boundaries, and short enough that the partition
/// fan-out stays bounded at four categories × fourteen months.
pub const MAX_RANGE_DAYS: u64 = 400;

/// Milliseconds in one hour.
const HOUR_MILLIS: i64 = 60 * 60 * 1_000;
/// Milliseconds in one day.
const DAY_MILLIS: i64 = 24 * HOUR_MILLIS;

/// The time grain one query answers at.
///
/// Distinct from [`Grain`], which names a stored row family: `Total` is a shape
/// of answer, not a shape of row, and it is read from whichever stored grain
/// answers it exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Bucket {
    /// One item per whole UTC hour.
    Hour,
    /// One item per whole UTC day.
    Day,
    /// One item for the whole range.
    Total,
}

impl Bucket {
    /// The alignment a range must satisfy, in milliseconds.
    ///
    /// `Total` aligns to the hour: it is read from stored hourly rows whenever
    /// the range is not also day-aligned, and an hour is the finest stored
    /// grain that does not expire.
    const fn alignment_millis(self) -> i64 {
        match self {
            Self::Hour | Self::Total => HOUR_MILLIS,
            Self::Day => DAY_MILLIS,
        }
    }

    /// The alignment a refusal names.
    #[must_use]
    pub const fn required_alignment(self) -> &'static str {
        match self {
            Self::Hour | Self::Total => "whole UTC hours",
            Self::Day => "whole UTC days",
        }
    }
}

/// Why a plan could not be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    /// The half-open range ends at or before it starts.
    #[error("the requested range ends at or before it starts")]
    InvertedRange,
    /// The range is not aligned to whole buckets of the requested grain.
    #[error("the requested range must be aligned to {required}")]
    Misaligned {
        /// The alignment the caller must round to.
        required: &'static str,
    },
    /// The range is wider than one query may span.
    #[error("the requested range spans {days} days; the maximum is {MAX_RANGE_DAYS}")]
    RangeTooWide {
        /// How wide the range is, rounded up to whole days.
        days: u64,
    },
    /// No category was requested.
    #[error("a usage query must name at least one category")]
    NoCategories,
    /// A `total` would have to read more rows than one answer may.
    ///
    /// Refused rather than paged: a partial total is a wrong number.
    #[error(
        "a total over {categories} categories and {buckets} buckets would read {rows} rows; \
         the maximum is {MAX_PAGE_ROWS}, which is {maximum_buckets} buckets at this grain"
    )]
    TotalTooWide {
        /// How many categories were requested.
        categories: usize,
        /// How many buckets the range covers at the grain it would be read at.
        buckets: u64,
        /// The rows the whole plan would touch.
        rows: u64,
        /// The widest span that fits, in buckets.
        maximum_buckets: u64,
    },
    /// A derived instant was not representable.
    #[error("the requested range is not representable")]
    Unrepresentable,
}

/// One partition the plan visits, in plan order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedPartition {
    /// The public category this partition holds.
    pub category: PublicCategory,
    /// The `YYYY-MM` partition.
    pub month: String,
    /// The stored grain the rows are read from.
    pub grain: Grain,
    /// The inclusive lower bucket bound.
    pub from_bucket: String,
    /// The exclusive upper bucket bound.
    pub until_bucket: String,
    /// The exact number of rows this partition can hold for this range.
    pub rows: u64,
}

/// The whole plan, with its row bound computed before any read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryPlan {
    /// The generation the plan is pinned to.
    pub generation: Generation,
    /// The answer shape.
    pub bucket: Bucket,
    /// The stored grain every partition reads from.
    pub grain: Grain,
    /// The partitions, in the fixed order a cursor's ordinal indexes into.
    pub partitions: Vec<PlannedPartition>,
    /// The exact upper bound on rows the whole plan can touch.
    pub rows: u64,
}

impl QueryPlan {
    /// How many partitions the plan visits.
    #[must_use]
    pub fn len(&self) -> usize {
        self.partitions.len()
    }

    /// Whether the plan visits nothing, which a legal range never produces.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.partitions.is_empty()
    }
}

/// Everything one plan needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanRequest<'a> {
    /// The generation the read is pinned to.
    pub generation: Generation,
    /// The workspace being read.
    pub workspace: &'a WorkspaceId,
    /// The requested categories.
    pub categories: &'a [PublicCategory],
    /// The answer shape.
    pub bucket: Bucket,
    /// The inclusive lower bound of the service-time window.
    pub from: Timestamp,
    /// The exclusive upper bound of the service-time window.
    pub until: Timestamp,
}

/// Builds the plan, or refuses before any read is issued.
///
/// The categories are normalised into [`PublicCategory::ALL`] order and
/// deduplicated, so two callers asking for the same set in different orders get
/// the same plan and therefore the same cursor positions.
///
/// # Errors
///
/// Returns [`PlanError::NoCategories`] for an empty category set,
/// [`PlanError::InvertedRange`] for an empty or inverted range,
/// [`PlanError::Misaligned`] for a range that is not on whole buckets of the
/// requested grain, [`PlanError::RangeTooWide`] above [`MAX_RANGE_DAYS`], and
/// [`PlanError::TotalTooWide`] when a `total` would have to read more rows than
/// one answer may hold.
pub fn plan(request: &PlanRequest<'_>) -> Result<QueryPlan, PlanError> {
    let mut categories: Vec<PublicCategory> = PublicCategory::ALL
        .into_iter()
        .filter(|candidate| request.categories.contains(candidate))
        .collect();
    categories.dedup();
    if categories.is_empty() {
        return Err(PlanError::NoCategories);
    }

    let from = request.from.unix_millis();
    let until = request.until.unix_millis();
    if until <= from {
        return Err(PlanError::InvertedRange);
    }
    let alignment = request.bucket.alignment_millis();
    if from.rem_euclid(alignment) != 0 || until.rem_euclid(alignment) != 0 {
        return Err(PlanError::Misaligned {
            required: request.bucket.required_alignment(),
        });
    }
    let span = until.checked_sub(from).ok_or(PlanError::Unrepresentable)?;
    let days = u64::try_from(span.div_euclid(DAY_MILLIS) + i64::from(span % DAY_MILLIS != 0))
        .map_err(|_| PlanError::Unrepresentable)?;
    if days > MAX_RANGE_DAYS {
        return Err(PlanError::RangeTooWide { days });
    }

    // A `total` is read from whichever stored grain answers it exactly: daily
    // when the range happens to be day-aligned, hourly otherwise. Reading the
    // coarser rows where they are exact is what keeps a year-long total inside
    // the row budget at all.
    let grain = match request.bucket {
        Bucket::Hour => Grain::Hourly,
        Bucket::Day => Grain::Daily,
        Bucket::Total => {
            if from.rem_euclid(DAY_MILLIS) == 0 && until.rem_euclid(DAY_MILLIS) == 0 {
                Grain::Daily
            } else {
                Grain::Hourly
            }
        }
    };
    let step = match grain {
        Grain::Hourly => HOUR_MILLIS,
        Grain::Daily => DAY_MILLIS,
    };
    let buckets = u64::try_from(span.div_euclid(step)).map_err(|_| PlanError::Unrepresentable)?;

    // A total is one answer, so its whole plan is budgeted, not its pages.
    if request.bucket == Bucket::Total {
        let categories_len =
            u64::try_from(categories.len()).map_err(|_| PlanError::Unrepresentable)?;
        let rows = buckets
            .checked_mul(categories_len)
            .ok_or(PlanError::Unrepresentable)?;
        let maximum = u64::try_from(MAX_PAGE_ROWS).map_err(|_| PlanError::Unrepresentable)?;
        if rows > maximum {
            return Err(PlanError::TotalTooWide {
                categories: categories.len(),
                buckets,
                rows,
                maximum_buckets: maximum / categories_len,
            });
        }
    }

    let months = months_between(request.from, request.until)?;
    let mut partitions = Vec::with_capacity(categories.len() * months.len());
    let mut rows: u64 = 0;
    for category in &categories {
        for month in &months {
            let partition = plan_partition(*category, month, grain, from, until, step)?;
            rows = rows
                .checked_add(partition.rows)
                .ok_or(PlanError::Unrepresentable)?;
            partitions.push(partition);
        }
    }

    Ok(QueryPlan {
        generation: request.generation,
        bucket: request.bucket,
        grain,
        partitions,
        rows,
    })
}

/// One `(category, month)` partition and the exact rows it can hold.
fn plan_partition(
    category: PublicCategory,
    month: &MonthSpan,
    grain: Grain,
    from: i64,
    until: i64,
    step: i64,
) -> Result<PlannedPartition, PlanError> {
    // The stored rows are bucketed by the *start* of the measurement's service
    // time, so a partition holds exactly the buckets whose start falls inside
    // both the month and the requested range.
    let lower = from.max(month.start);
    let upper = until.min(month.end);
    let count = if upper > lower {
        u64::try_from((upper - lower).div_euclid(step)).map_err(|_| PlanError::Unrepresentable)?
    } else {
        0
    };
    let from_bucket = bucket_string(lower, grain)?;
    let until_bucket = bucket_string(upper, grain)?;
    Ok(PlannedPartition {
        category,
        month: month.label.clone(),
        grain,
        from_bucket,
        until_bucket,
        rows: count,
    })
}

/// The bucket label one instant falls in, at one grain.
fn bucket_string(millis: i64, grain: Grain) -> Result<String, PlanError> {
    let instant = Timestamp::from_unix_millis(millis).map_err(|_| PlanError::Unrepresentable)?;
    Ok(match grain {
        Grain::Hourly => instant.hour_bucket(),
        Grain::Daily => instant.day_bucket(),
    })
}

/// One UTC month the range intersects.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MonthSpan {
    /// The `YYYY-MM` label.
    label: String,
    /// The first instant of the month.
    start: i64,
    /// The first instant of the next month.
    end: i64,
}

/// Every UTC month the half-open range intersects, in order.
///
/// Walked one day at a time rather than computed from a calendar library: the
/// range is capped at [`MAX_RANGE_DAYS`], so this is at most four hundred
/// steps, and it cannot disagree with `Timestamp::month_bucket`, which is what
/// the key grammar itself uses.
fn months_between(from: Timestamp, until: Timestamp) -> Result<Vec<MonthSpan>, PlanError> {
    let mut spans: Vec<MonthSpan> = Vec::new();
    let mut cursor = from.unix_millis().div_euclid(DAY_MILLIS) * DAY_MILLIS;
    let end = until.unix_millis();
    while cursor < end {
        let instant =
            Timestamp::from_unix_millis(cursor).map_err(|_| PlanError::Unrepresentable)?;
        let label = instant.month_bucket();
        match spans.last_mut() {
            Some(last) if last.label == label => last.end = cursor + DAY_MILLIS,
            _ => spans.push(MonthSpan {
                label,
                start: cursor,
                end: cursor + DAY_MILLIS,
            }),
        }
        cursor += DAY_MILLIS;
    }
    // The first month starts where the range does when the range starts mid-day,
    // and the last month ends where the range does.
    if let Some(first) = spans.first_mut() {
        first.start = first.start.min(from.unix_millis());
    }
    if let Some(last) = spans.last_mut() {
        last.end = last.end.max(end);
    }
    Ok(spans)
}

#[cfg(test)]
mod tests {
    use super::{
        Bucket, MAX_RANGE_DAYS, PlanError, PlanRequest, plan,
    };
    use aex_usage_domain::meter::PublicCategory;
    use aex_usage_domain::projection::{Generation, Grain};
    use aex_usage_domain::wire_pending::{Timestamp, WorkspaceId};

    fn workspace() -> WorkspaceId {
        WorkspaceId::parse("ws-1").expect("workspace")
    }

    fn at(value: &str) -> Timestamp {
        Timestamp::parse(value).expect("an instant")
    }

    fn request<'a>(
        categories: &'a [PublicCategory],
        bucket: Bucket,
        from: &str,
        until: &str,
        workspace: &'a WorkspaceId,
    ) -> PlanRequest<'a> {
        PlanRequest {
            generation: Generation::FIRST,
            workspace,
            categories,
            bucket,
            from: at(from),
            until: at(until),
        }
    }

    #[test]
    fn the_plan_is_categories_times_months_and_its_row_bound_is_arithmetic() {
        let workspace = workspace();
        let plan = plan(&request(
            &[PublicCategory::Compute, PublicCategory::Storage],
            Bucket::Day,
            "2026-07-30T00:00:00.000Z",
            "2026-08-03T00:00:00.000Z",
            &workspace,
        ))
        .expect("plans");

        // Two categories over two months.
        assert_eq!(plan.len(), 4);
        assert_eq!(plan.grain, Grain::Daily);
        // July 30 and 31, then August 1 and 2 — the range is half-open, so
        // August 3 is not in it. Four buckets per category.
        assert_eq!(plan.rows, 2 * (2 + 2));
        assert_eq!(
            plan.partitions
                .iter()
                .map(|partition| (partition.category, partition.month.clone(), partition.rows))
                .collect::<Vec<_>>(),
            vec![
                (PublicCategory::Storage, "2026-07".to_owned(), 2),
                (PublicCategory::Storage, "2026-08".to_owned(), 2),
                (PublicCategory::Compute, "2026-07".to_owned(), 2),
                (PublicCategory::Compute, "2026-08".to_owned(), 2),
            ],
            "the plan order is fixed, so a cursor ordinal means one thing"
        );
    }

    #[test]
    fn the_category_set_is_normalised_so_two_spellings_plan_identically() {
        let workspace = workspace();
        let one = plan(&request(
            &[PublicCategory::Compute, PublicCategory::Storage],
            Bucket::Day,
            "2026-08-01T00:00:00.000Z",
            "2026-08-02T00:00:00.000Z",
            &workspace,
        ))
        .expect("plans");
        let other = plan(&request(
            &[
                PublicCategory::Storage,
                PublicCategory::Compute,
                PublicCategory::Storage,
            ],
            Bucket::Day,
            "2026-08-01T00:00:00.000Z",
            "2026-08-02T00:00:00.000Z",
            &workspace,
        ))
        .expect("plans");
        assert_eq!(one, other);
    }

    #[test]
    fn a_misaligned_range_is_refused_naming_the_alignment_rather_than_widened() {
        let workspace = workspace();
        let error = plan(&request(
            &[PublicCategory::Compute],
            Bucket::Day,
            "2026-08-01T06:00:00.000Z",
            "2026-08-02T00:00:00.000Z",
            &workspace,
        ))
        .expect_err("a part-day is not a day");
        assert_eq!(
            error,
            PlanError::Misaligned {
                required: "whole UTC days"
            }
        );

        // An hour grain refuses a sub-hour boundary for the same reason.
        assert_eq!(
            plan(&request(
                &[PublicCategory::Compute],
                Bucket::Hour,
                "2026-08-01T00:30:00.000Z",
                "2026-08-01T01:00:00.000Z",
                &workspace,
            ))
            .expect_err("a half hour is not an hour"),
            PlanError::Misaligned {
                required: "whole UTC hours"
            }
        );
    }

    #[test]
    fn an_inverted_or_empty_range_is_refused() {
        let workspace = workspace();
        for (from, until) in [
            ("2026-08-02T00:00:00.000Z", "2026-08-01T00:00:00.000Z"),
            ("2026-08-01T00:00:00.000Z", "2026-08-01T00:00:00.000Z"),
        ] {
            assert_eq!(
                plan(&request(
                    &[PublicCategory::Compute],
                    Bucket::Day,
                    from,
                    until,
                    &workspace
                ))
                .expect_err("an empty range answers nothing"),
                PlanError::InvertedRange
            );
        }
    }

    #[test]
    fn a_range_wider_than_the_cap_is_refused() {
        let workspace = workspace();
        let error = plan(&request(
            &[PublicCategory::Compute],
            Bucket::Day,
            "2025-01-01T00:00:00.000Z",
            "2026-08-01T00:00:00.000Z",
            &workspace,
        ))
        .expect_err("over the cap");
        assert!(
            matches!(error, PlanError::RangeTooWide { days } if days > MAX_RANGE_DAYS),
            "{error}"
        );
    }

    #[test]
    fn a_query_naming_no_category_is_refused_rather_than_answered_with_nothing() {
        let workspace = workspace();
        assert_eq!(
            plan(&request(
                &[],
                Bucket::Day,
                "2026-08-01T00:00:00.000Z",
                "2026-08-02T00:00:00.000Z",
                &workspace
            ))
            .expect_err("no categories"),
            PlanError::NoCategories
        );
    }

    #[test]
    fn a_total_reads_the_coarsest_grain_that_answers_it_exactly() {
        let workspace = workspace();
        let aligned = plan(&request(
            &[PublicCategory::Compute],
            Bucket::Total,
            "2026-08-01T00:00:00.000Z",
            "2026-08-08T00:00:00.000Z",
            &workspace,
        ))
        .expect("plans");
        assert_eq!(aligned.grain, Grain::Daily);
        assert_eq!(aligned.rows, 7);

        let ragged = plan(&request(
            &[PublicCategory::Compute],
            Bucket::Total,
            "2026-08-01T06:00:00.000Z",
            "2026-08-02T06:00:00.000Z",
            &workspace,
        ))
        .expect("plans");
        assert_eq!(
            ragged.grain,
            Grain::Hourly,
            "a range that is not day-aligned must be summed from hours or the \
             total would be for a different interval"
        );
        assert_eq!(ragged.rows, 24);
    }

    #[test]
    fn an_over_budget_total_is_refused_naming_the_maximum_span_and_reads_nothing() {
        let workspace = workspace();
        // Hourly, so 400 days is 9,600 buckets against a 500-row budget.
        let error = plan(&request(
            &[PublicCategory::Compute],
            Bucket::Total,
            "2026-01-01T06:00:00.000Z",
            "2026-06-01T06:00:00.000Z",
            &workspace,
        ))
        .expect_err("a partial total is a wrong number");
        let PlanError::TotalTooWide {
            maximum_buckets,
            rows,
            ..
        } = error
        else {
            panic!("expected a total budget refusal, got {error}");
        };
        assert!(rows > 500);
        assert_eq!(maximum_buckets, 500);

        // Four categories divide the same budget four ways, and the refusal
        // says so rather than making the caller guess.
        let error = plan(&request(
            &PublicCategory::ALL,
            Bucket::Total,
            "2026-01-01T00:00:00.000Z",
            "2026-08-01T00:00:00.000Z",
            &workspace,
        ))
        .expect_err("four categories over seven months");
        let PlanError::TotalTooWide {
            maximum_buckets, ..
        } = error
        else {
            panic!("expected a total budget refusal, got {error}");
        };
        assert_eq!(maximum_buckets, 125);
    }

    #[test]
    fn a_month_partition_holds_only_the_buckets_inside_both_the_month_and_the_range() {
        let workspace = workspace();
        let plan = plan(&request(
            &[PublicCategory::Storage],
            Bucket::Hour,
            "2026-07-31T22:00:00.000Z",
            "2026-08-01T02:00:00.000Z",
            &workspace,
        ))
        .expect("plans");
        assert_eq!(plan.len(), 2);
        assert_eq!(plan.partitions[0].month, "2026-07");
        assert_eq!(plan.partitions[0].rows, 2);
        assert_eq!(plan.partitions[0].from_bucket, "2026-07-31T22");
        assert_eq!(plan.partitions[1].month, "2026-08");
        assert_eq!(plan.partitions[1].rows, 2);
        assert_eq!(plan.partitions[1].from_bucket, "2026-08-01T00");
        assert_eq!(plan.rows, 4);
    }

    #[test]
    fn the_widest_legal_fan_out_stays_bounded() {
        let workspace = workspace();
        let plan = plan(&request(
            &PublicCategory::ALL,
            Bucket::Day,
            "2025-07-01T00:00:00.000Z",
            "2026-08-05T00:00:00.000Z",
            &workspace,
        ))
        .expect("plans");
        assert!(
            plan.len() <= 4 * 14,
            "four categories by fourteen months is the widest legal fan-out, \
             got {}",
            plan.len()
        );
    }
}
