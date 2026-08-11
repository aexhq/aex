//! `usage_query`, the one money-adjacent read this deployable serves.
//!
//! The route reads the **coarse** rollup face — one row per generation,
//! workspace, public category, month and bucket, carrying no dimension identity
//! — never the per-tuple `H#`/`D#` rows. That is what makes the number of rows a
//! request touches arithmetic rather than data, and therefore what makes an
//! over-budget request a refusal issued before the first read.
//!
//! Four rules the shape enforces rather than documents.
//!
//! **The plan is built and budgeted before anything is read.** A misaligned
//! range, an over-wide range and an over-budget `total` are all refused by
//! [`aex_usage_query_dynamodb::plan`] with zero store calls. Nothing here
//! widens, clamps or truncates.
//!
//! **The coverage row is read first and eventually, the rollups after and
//! strongly.** The fold applies facts in contiguous sequence order behind the
//! coverage fence, so a stale coverage read can only understate completeness,
//! and rollups read afterwards reflect at least what it named. That is what
//! makes `completeThrough` a claim that is always true rather than usually
//! true. Reversing the order would let a coverage row name a frontier ahead of
//! the very rows the same response carries.
//!
//! **One failed partition fails the whole query.** No partial page, no
//! per-partition degradation, no retry with fewer categories. A page missing a
//! category is a bill missing a line, and no flag makes a customer read it
//! correctly.
//!
//! **A `total` never returns a cursor.** A partial total is a wrong number
//! rather than a short answer, so an over-budget total is refused up front.

use aex_regional_http::cursor::{CursorBinding, Order, SnapshotToken};
use aex_regional_http::projection::ProjectionError;
use aex_usage_domain::frontier::{AcceptedSequence, FrontierState};
use aex_usage_domain::meter::{Category, PublicCategory};
use aex_usage_domain::projection::Generation;
use aex_usage_domain::wire_pending::WorkspaceId as UsageWorkspaceId;
use aex_usage_query_dynamodb::expressions::{CoarseRequest, CoarseRow, CoverageRow, QueryError};
use aex_usage_query_dynamodb::plan::{Bucket, QueryPlan, UsagePlanError};
use aex_wire::cursor::Cursor;
use aex_wire::error::{ErrorCode, WireError, WireResult};
use aex_wire::models;
use aex_wire::routes::RouteId;
use aex_wire::server::{RequestContext as WireContext, UsageApi};
use aex_wire::types::{DecimalU128, Timestamp};

use super::handlers::Routes;

/// The largest page a caller may ask for.
///
/// The plane's own item ceiling, matched deliberately: one refusal rule per
/// plane is worth more than one per route.
pub const MAX_PAGE_ITEMS: u32 = 100;

/// The page a caller who names no limit gets.
pub const DEFAULT_PAGE_ITEMS: u32 = 25;

/// The resumable position inside a plan.
///
/// The plan order is fixed and derived from the normalized query, which the
/// cursor binds, so an ordinal means exactly one partition. `after` is the last
/// sort key consumed inside that partition — the thing a bare keyset token
/// cannot express, because it cannot say "partitions zero through k are
/// exhausted".
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PlanPosition {
    /// The ordinal of the next partition in the fixed plan order.
    pub partition: u32,
    /// The last sort key consumed inside it, when the partition is part-read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
}

/// The normalized query a cursor is bound to.
///
/// Hashed through the one workspace canonicalization rule with a
/// domain-separating prefix, exactly as the operations list does, so a caller
/// cannot carry a cursor from one query shape into another.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct NormalizedQuery<'a> {
    bucket: &'a str,
    categories: Vec<&'a str>,
    group_by: Vec<&'a str>,
    gte: String,
    lt: String,
}

impl Routes {
    /// The page budget, refused rather than clamped.
    fn usage_budget(limit: Option<u32>) -> WireResult<u32> {
        let requested = limit.unwrap_or(DEFAULT_PAGE_ITEMS);
        if requested == 0 || requested > MAX_PAGE_ITEMS {
            return Err(WireError::new(ErrorCode::InvalidRequest).with_message("page limit"));
        }
        Ok(requested)
    }

    /// The digest the cursor binds the query shape to.
    fn usage_query_hash(body: &models::UsageQuery) -> WireResult<[u8; 32]> {
        use sha2::Digest as _;

        // Normalised, not copied: the same set in a different order must produce
        // the same digest, because it produces the same plan.
        let mut categories: Vec<&str> = requested_categories(body)
            .into_iter()
            .map(PublicCategory::id)
            .collect();
        categories.sort_unstable();
        let mut group_by: Vec<&str> = body
            .group_by
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|value| value.as_str())
            .collect();
        group_by.sort_unstable();
        group_by.dedup();

        let normalized = NormalizedQuery {
            bucket: body.bucket.as_str(),
            categories,
            group_by,
            gte: body.time_range.gte.to_wire(),
            lt: body.time_range.lt.to_wire(),
        };
        let bytes = aex_wire::canonical::to_jcs_bytes(&normalized)
            .map_err(|_| WireError::new(ErrorCode::InternalError))?;
        let mut digest = sha2::Sha256::new();
        digest.update(b"aex.regional.usage.query.v1\0");
        digest.update(bytes);
        Ok(digest.finalize().into())
    }

    /// The binding every page of one query is minted and verified under.
    ///
    /// The snapshot carries the pinned generation and the pinned
    /// `completeThrough`, both authenticated, so a generation flip or a
    /// re-pinned watermark invalidates every outstanding cursor rather than
    /// letting two generations be concatenated into one answer.
    fn usage_binding(
        &self,
        generation: Generation,
        complete_through: u128,
        query_hash: [u8; 32],
    ) -> WireResult<CursorBinding> {
        let snapshot = format!("{generation}:C{complete_through}");
        Ok(CursorBinding {
            route: RouteId::UsageQuery,
            principal_scope: self.cx.auth.credential_binding,
            region: self.cx.auth.placement,
            workspace_id: self.cx.auth.workspace_id,
            session_id: None,
            query_hash,
            order: Order::Ascending,
            snapshot: SnapshotToken::new(&snapshot)
                .map_err(|error| WireError::from(ProjectionError::Cursor(error)))?,
        })
    }
}

/// The categories a query asks for, defaulting to all four.
fn requested_categories(body: &models::UsageQuery) -> Vec<PublicCategory> {
    let Some(requested) = body.categories.as_deref() else {
        return PublicCategory::ALL.to_vec();
    };
    PublicCategory::ALL
        .into_iter()
        .filter(|candidate| {
            requested
                .iter()
                .any(|asked| wire_category(*asked) == *candidate)
        })
        .collect()
}

/// The domain category one wire category names.
const fn wire_category(category: models::UsageCategory) -> PublicCategory {
    match category {
        models::UsageCategory::Storage => PublicCategory::Storage,
        models::UsageCategory::Compute => PublicCategory::Compute,
        models::UsageCategory::Memory => PublicCategory::Memory,
        models::UsageCategory::DataTransfer => PublicCategory::DataTransfer,
    }
}

/// The wire authority face one domain authority names.
const fn wire_authority(category: Category) -> models::UsageAuthority {
    match category {
        Category::Storage => models::UsageAuthority::Storage,
        Category::Compute => models::UsageAuthority::Compute,
        Category::Transfer => models::UsageAuthority::Transfer,
    }
}

/// The answer shape one wire bucket names.
const fn wire_bucket(bucket: models::UsageBucket) -> Bucket {
    match bucket {
        models::UsageBucket::Hour => Bucket::Hour,
        models::UsageBucket::Day => Bucket::Day,
        models::UsageBucket::Total => Bucket::Total,
    }
}

/// A plan refusal, as the route's own declared vocabulary.
///
/// Every one of these is a request the caller can fix by asking a different
/// question, so they are `invalid_query` carrying the reason rather than a
/// generic 400 the caller has to guess at.
fn plan_refusal(error: &UsagePlanError) -> WireError {
    WireError::new(ErrorCode::InvalidQuery).with_message(error.to_string())
}

/// A projection failure, as the route's own declared vocabulary.
///
/// A denied read and a missing table are composition failures, not customer
/// conditions: an empty projection answers with no rows, so neither may become
/// a 404 or a confidently empty page.
fn read_refusal(error: &QueryError) -> WireError {
    match error {
        QueryError::Unavailable { .. } => WireError::new(ErrorCode::UsageUnavailable),
        QueryError::PageBudget { .. } => {
            WireError::new(ErrorCode::InvalidRequest).with_message("page limit")
        }
        QueryError::InvertedRange => {
            WireError::new(ErrorCode::InvalidQuery).with_message(error.to_string())
        }
        QueryError::Denied
        | QueryError::Misconfigured { .. }
        | QueryError::Key(_)
        | QueryError::MissingAttribute { .. }
        | QueryError::MalformedAttribute { .. }
        | QueryError::ItemTypeMismatch { .. } => WireError::new(ErrorCode::InternalError),
    }
}

/// One row, as the wire aggregate its category discriminates.
fn to_wire_item(
    row: &CoarseRow,
    region: aex_wire::types::Region,
    workspace: aex_wire::ids::WorkspaceId,
    settled: AcceptedSequence,
    service_time: models::TimeRange,
) -> models::UsageAggregate {
    let attribution = models::UsageAttribution {
        region,
        workspace_id: workspace,
        // Permanently absent on this route. The coarse face carries no session,
        // run or operation identity, and saying so is honest rather than an
        // omission a client should wait for.
        session_id: None,
        operation_id: None,
        source: row.public_category.category().id().to_owned(),
        service_time,
    };
    let settlement = if row.is_settled(settled) {
        models::UsageSettlement::Settled
    } else {
        models::UsageSettlement::Provisional
    };
    let quantity = DecimalU128::new(row.quantity.get());
    match row.public_category {
        PublicCategory::Storage => models::UsageAggregate::Storage(models::UsageStorageAggregate {
            attribution,
            settlement,
            byte_minutes: quantity,
        }),
        PublicCategory::Compute => models::UsageAggregate::Compute(models::UsageComputeAggregate {
            attribution,
            settlement,
            millicpu_milliseconds: quantity,
        }),
        PublicCategory::Memory => models::UsageAggregate::Memory(models::UsageMemoryAggregate {
            attribution,
            settlement,
            byte_milliseconds: quantity,
        }),
        PublicCategory::DataTransfer => {
            models::UsageAggregate::DataTransfer(models::UsageDataTransferAggregate {
                attribution,
                settlement,
                egress_bytes: quantity,
            })
        }
    }
}

/// One coverage row, as the wire frontier.
fn to_wire_frontier(
    coverage: &CoverageRow,
    region: aex_wire::types::Region,
    workspace: aex_wire::ids::WorkspaceId,
    complete_through: u128,
    includes_through: u128,
) -> models::UsageFrontier {
    let (state, stalled_at, stall_reason) = match coverage.state {
        FrontierState::Advancing => (models::UsageFrontierState::Advancing, None, None),
        FrontierState::Quarantined { at, reason } => (
            models::UsageFrontierState::Stalled,
            Some(DecimalU128::new(u128::from(at.get()))),
            Some(reason.id().to_owned()),
        ),
    };
    models::UsageFrontier {
        region,
        workspace_id: workspace,
        category: wire_authority(coverage.public_category.category()),
        accepted_sequence: DecimalU128::new(u128::from(coverage.accepted.get())),
        projected_sequence: DecimalU128::new(u128::from(coverage.projected.get())),
        published_sequence: DecimalU128::new(u128::from(coverage.published.get())),
        settled_sequence: DecimalU128::new(u128::from(coverage.settled.get())),
        complete_through: DecimalU128::new(complete_through),
        includes_through: DecimalU128::new(includes_through),
        state,
        stalled_at,
        stall_reason,
        service_through: coverage
            .service_through
            .and_then(|through| Timestamp::parse(&through.to_canonical()).ok()),
    }
}

impl UsageApi for Routes {
    async fn usage_query(
        &self,
        _cx: &WireContext,
        _query: models::UsageQueryQuery,
        body: models::UsageQuery,
    ) -> WireResult<models::UsagePage> {
        let limit = Self::usage_budget(body.limit)?;
        let bucket = wire_bucket(body.bucket);
        let categories = requested_categories(&body);

        // The generation pointer. An absent pointer is the honest state of a
        // region whose projection has never been cut over, and it is not a
        // synonym for the first generation: reading against the wrong one is
        // confidently empty rather than visibly absent.
        let store = self.shared.usage.as_ref();
        let Some(generation) = store
            .current_generation()
            .await
            .map_err(|error| read_refusal(&error))?
        else {
            return Err(WireError::new(ErrorCode::UsageUnavailable));
        };

        let workspace = UsageWorkspaceId::parse(&self.cx.auth.workspace_id.to_string())
            .map_err(|_| WireError::new(ErrorCode::InternalError))?;
        let from = usage_instant(body.time_range.gte)?;
        let until = usage_instant(body.time_range.lt)?;

        // Everything refusable is refused here, with zero store calls behind it.
        let plan = aex_usage_query_dynamodb::plan::plan(&aex_usage_query_dynamodb::PlanRequest {
            generation,
            workspace: &workspace,
            categories: &categories,
            bucket,
            from,
            until,
        })
        .map_err(|error| plan_refusal(&error))?;

        // Coverage first, and eventually: staleness here can only understate
        // completeness, and a coverage row read after the rollups could name a
        // frontier ahead of them.
        let mut coverage = Vec::new();
        for authority in [
            PublicCategory::Storage,
            PublicCategory::Compute,
            PublicCategory::DataTransfer,
        ] {
            if !categories
                .iter()
                .any(|category| category.category() == authority.category())
            {
                continue;
            }
            if let Some(row) = store
                .coverage_eventual(generation, &workspace, authority)
                .await
                .map_err(|error| read_refusal(&error))?
            {
                coverage.push(row);
            }
        }
        // `completeThrough` is the weakest observed projected position across
        // the authorities this answer draws on. Taking the strongest would
        // claim completeness for a category whose fold is further behind.
        let complete_through = coverage
            .iter()
            .map(|row| u128::from(row.projected.get()))
            .min()
            .unwrap_or(0);

        let query_hash = Self::usage_query_hash(&body)?;
        let binding = self.usage_binding(generation, complete_through, query_hash)?;
        let start = self.resume_plan(body.cursor.as_ref(), &binding)?;

        let (items, next, includes_through) = self
            .read_plan(&plan, &workspace, &binding, start, limit, &coverage, &body)
            .await?;

        // Re-checked after the read, following the pattern every other paged
        // collection here uses: a generation flip mid-request refuses the page
        // rather than mixing two generations into one answer.
        let current = store
            .current_generation()
            .await
            .map_err(|error| read_refusal(&error))?;
        if current != Some(generation) {
            return Err(WireError::new(ErrorCode::InvalidCursor));
        }

        let frontiers = coverage
            .iter()
            .map(|row| {
                to_wire_frontier(
                    row,
                    self.cx.auth.placement,
                    self.cx.auth.workspace_id,
                    complete_through,
                    includes_through,
                )
            })
            .collect();
        Ok(models::UsagePage {
            items,
            next_cursor: next,
            frontiers,
        })
    }
}

impl Routes {
    /// The plan position a cursor resumes from, or the start of the plan.
    fn resume_plan(
        &self,
        cursor: Option<&Cursor>,
        binding: &CursorBinding,
    ) -> WireResult<PlanPosition> {
        let Some(cursor) = cursor else {
            return Ok(PlanPosition {
                partition: 0,
                after: None,
            });
        };
        let resumed = aex_regional_http::cursor::decode_state_resume::<PlanPosition>(
            &self.shared.cursor_keys,
            cursor,
            &binding.into(),
            self.now()?,
        )
        .map_err(|error| WireError::from(ProjectionError::Cursor(error)))?;
        // The snapshot is authenticated, so a cursor minted against another
        // generation or another pinned watermark is refused rather than
        // resumed against this plan.
        if resumed.snapshot != binding.snapshot {
            return Err(WireError::new(ErrorCode::InvalidCursor));
        }
        Ok(resumed.state)
    }

    /// Walks the plan from `start` until the item budget is spent.
    #[allow(clippy::too_many_arguments)]
    #[allow(
        clippy::too_many_lines,
        reason = "one partition walk, read straight through; splitting it would \
                  hide the budget and continuation bookkeeping from each other"
    )]
    async fn read_plan(
        &self,
        plan: &QueryPlan,
        workspace: &UsageWorkspaceId,
        binding: &CursorBinding,
        start: PlanPosition,
        limit: u32,
        coverage: &[CoverageRow],
        body: &models::UsageQuery,
    ) -> WireResult<(Vec<models::UsageAggregate>, Option<Cursor>, u128)> {
        let store = self.shared.usage.as_ref();
        let mut items: Vec<models::UsageAggregate> = Vec::new();
        let mut includes_through: u128 = 0;
        let mut after = start.after;
        let mut ordinal = usize::try_from(start.partition)
            .map_err(|_| WireError::new(ErrorCode::InvalidCursor))?;
        if ordinal > plan.partitions.len() {
            return Err(WireError::new(ErrorCode::InvalidCursor));
        }
        let mut totals: std::collections::BTreeMap<PublicCategory, (u128, AcceptedSequence)> =
            std::collections::BTreeMap::new();

        while ordinal < plan.partitions.len() {
            let partition = &plan.partitions[ordinal];
            if partition.rows == 0 {
                ordinal += 1;
                after = None;
                continue;
            }
            let remaining = usize::try_from(limit)
                .map_err(|_| WireError::new(ErrorCode::InternalError))?
                .saturating_sub(items.len());
            // A `total` is never paged, so it reads its whole plan; the planner
            // has already refused any plan that would not fit.
            let budget = if plan.bucket == Bucket::Total {
                usize::try_from(partition.rows)
                    .map_err(|_| WireError::new(ErrorCode::InternalError))?
            } else {
                if remaining == 0 {
                    break;
                }
                remaining
            };
            let page = store
                .coarse(&CoarseRequest {
                    generation: plan.generation,
                    workspace,
                    category: partition.category,
                    month: &partition.month,
                    grain: partition.grain,
                    from_bucket: &partition.from_bucket,
                    until_bucket: &partition.until_bucket,
                    limit: budget,
                    after: after.clone(),
                })
                .await
                .map_err(|error| read_refusal(&error))?;

            for row in &page.rows {
                includes_through = includes_through.max(u128::from(row.highest_sequence.get()));
                let settled = settled_for(coverage, row.public_category);
                if plan.bucket == Bucket::Total {
                    let entry = totals
                        .entry(row.public_category)
                        .or_insert((0, AcceptedSequence::ORIGIN));
                    entry.0 = entry.0.saturating_add(row.quantity.get());
                    if row.highest_sequence.get() > entry.1.get() {
                        entry.1 = row.highest_sequence;
                    }
                } else {
                    items.push(to_wire_item(
                        row,
                        self.cx.auth.placement,
                        self.cx.auth.workspace_id,
                        settled,
                        bucket_range(&row.bucket, row.grain, body)?,
                    ));
                }
            }

            match page.next {
                Some(next) if plan.bucket != Bucket::Total => {
                    after = Some(next);
                    // The partition continued, so the position stays on it.
                    if items.len()
                        >= usize::try_from(limit)
                            .map_err(|_| WireError::new(ErrorCode::InternalError))?
                    {
                        break;
                    }
                }
                _ => {
                    ordinal += 1;
                    after = None;
                }
            }
        }

        if plan.bucket == Bucket::Total {
            for (category, (quantity, highest)) in totals {
                let settled = settled_for(coverage, category);
                let row = CoarseRow {
                    public_category: category,
                    grain: plan.grain,
                    bucket: String::new(),
                    quantity: aex_usage_domain::quantity::Quantity::new(quantity)
                        .map_err(|_| WireError::new(ErrorCode::InternalError))?,
                    highest_sequence: highest,
                };
                items.push(to_wire_item(
                    &row,
                    self.cx.auth.placement,
                    self.cx.auth.workspace_id,
                    settled,
                    body.time_range.clone(),
                ));
            }
            // A partial total is a wrong number, so a total is never continued.
            return Ok((items, None, includes_through));
        }

        let next = if ordinal >= plan.partitions.len() {
            None
        } else {
            let position = PlanPosition {
                partition: u32::try_from(ordinal)
                    .map_err(|_| WireError::new(ErrorCode::InternalError))?,
                after,
            };
            Some(
                aex_regional_http::cursor::encode_state(
                    self.shared.cursor_keys.current(),
                    binding,
                    &position,
                    self.now()?,
                )
                .map_err(|error| WireError::from(ProjectionError::Cursor(error)))?,
            )
        };
        Ok((items, next, includes_through))
    }
}

/// The settled position one public category reports under.
///
/// Memory reports under compute, because they are one contiguous sequence
/// behind one authority and therefore one coverage row.
fn settled_for(coverage: &[CoverageRow], category: PublicCategory) -> AcceptedSequence {
    coverage
        .iter()
        .find(|row| row.public_category.category() == category.category())
        .map_or(AcceptedSequence::ORIGIN, |row| row.settled)
}

/// The half-open service-time interval one bucket label covers.
fn bucket_range(
    bucket: &str,
    grain: aex_usage_domain::projection::Grain,
    body: &models::UsageQuery,
) -> WireResult<models::TimeRange> {
    let start = match grain {
        aex_usage_domain::projection::Grain::Hourly => format!("{bucket}:00:00.000Z"),
        aex_usage_domain::projection::Grain::Daily => format!("{bucket}T00:00:00.000Z"),
    };
    let gte = Timestamp::parse(&start).map_err(|_| WireError::new(ErrorCode::InternalError))?;
    let width = match grain {
        aex_usage_domain::projection::Grain::Hourly => 60 * 60 * 1_000,
        aex_usage_domain::projection::Grain::Daily => 24 * 60 * 60 * 1_000,
    };
    let lt = Timestamp::from_unix_millis(gte.unix_millis().saturating_add(width))
        .map_err(|_| WireError::new(ErrorCode::InternalError))?;
    // A bucket never reports outside the window that was asked for.
    let _ = body;
    Ok(models::TimeRange { gte, lt })
}

/// One wire instant as the usage domain's own instant.
fn usage_instant(value: Timestamp) -> WireResult<aex_usage_domain::wire_pending::Timestamp> {
    aex_usage_domain::wire_pending::Timestamp::from_unix_millis(value.unix_millis())
        .map_err(|_| WireError::new(ErrorCode::InvalidQuery).with_message("time range"))
}
