//! The bounded `observation-authority` reader.
//!
//! There is no projection and no materializer: this table is both the authority
//! and the query engine. Every read is a `Query` against a named index, bounded
//! by the page budget, and **no `Scan` exists anywhere in this deployable** — a
//! scan is how a bounded query engine silently becomes an unbounded one.
//!
//! Nothing customer-supplied reaches an expression string. Key conditions are
//! built through [`ExpressionBuilder`], which emits generated `#n0` / `:v0`
//! placeholders, and every key component is formatted only from values the
//! domain already validated.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::Arc;

use aex_observation_domain::canonical::CanonicalValue;
use aex_observation_domain::gap::GapRecord;
use aex_observation_domain::keys::{self, BucketHour, ScopeKey};
use aex_observation_domain::order::{Direction, OrderTuple, order_sort_key};
use aex_observation_domain::signal::Signal;
use aex_observation_query::ast::MapRow;
use aex_observation_query::coverage::Snapshot;
use aex_observation_query::plan::{Access, NormalizedQuery, Spend};
use aex_observation_query::{ObservationResume, ResumeKey, SegmentResume, SegmentState};
use aex_observation_store_aws::expressions::{ExpressionBuilder, Index, PK, SK};
use aex_observation_store_aws::gap::decode as decode_gap;
use aex_session_dynamodb::measure::DYNAMODB_ITEM_CEILING;
use aex_wire::ids::{
    ObservationId, RunId, SessionId, SpanId, TelemetryGapId, TraceId, WorkspaceId,
};
use aex_wire::models::{Observation, ObservationSignal};
use aex_wire::types::{DecimalU128, Timestamp};
use aws_sdk_dynamodb::types::{AttributeValue, KeysAndAttributes};
use futures::StreamExt as _;

use crate::counters::{ReadCounter, ReadCounters};
use crate::frontier::{BundleRequest, Frontier, FrontierBundle, ScopeDeletionState};

/// The shard count one bucket is written across, matching the admission edge.
///
/// Fixed for the life of a bucket, so a reader never has to guess how many
/// partitions to merge.
pub const BUCKET_SHARDS: u8 = 4;

/// `DynamoDB` accepts at most one hundred keys in one `BatchGetItem` request.
pub(crate) const BATCH_GET_MAX_KEYS: usize = 100;
/// Unprocessed keys are retried a small, bounded number of times in addition
/// to the SDK's transport retry policy.
const BATCH_GET_UNPROCESSED_RETRIES: u8 = 3;
/// The maximum provider reads allowed to execute at once while filling the
/// heads needed for an exact k-way merge.
const MAX_PARALLEL_SEGMENT_READS: usize = 16;
/// `DynamoDB` stops one `Query` after at most one mebibyte of evaluated items.
const DDB_QUERY_MAX_BYTES: u64 = 1024 * 1024;

/// Maximum immutable gap revisions one query may inspect before failing closed.
pub const GAP_REVISION_BUDGET: u32 = 5_000;

/// Why a read could not be served.
#[derive(Clone, Debug, thiserror::Error)]
pub enum ReadError {
    /// The authority itself is unreachable.
    #[error("`{operation}` failed: {reason}")]
    Provider {
        /// Which call.
        operation: &'static str,
        /// What the provider reported, without its own body.
        reason: String,
    },
    /// A stored item could not be decoded.
    #[error("stored item is missing attribute `{attribute}`")]
    Malformed {
        /// Which attribute.
        attribute: &'static str,
    },
    /// Authenticated continuation state does not describe this exact plan.
    #[error("cursor resume state does not match the current query plan")]
    InvalidResume,
    /// A write adapter was asked to target a row outside export control.
    #[error("{operation} refused a non-export-control key")]
    InvalidWriteTarget {
        /// Which write path rejected the key.
        operation: &'static str,
    },
    /// A single page could make no progress at all.
    ///
    /// This is the only budget outcome that is an error. Every other exhausted
    /// dimension ends the page early **with a cursor**, which is correct,
    /// resumable and visible.
    #[error("no page could progress: {dimension} was exhausted after {scanned} items")]
    BudgetExhausted {
        /// The limiting dimension.
        dimension: &'static str,
        /// The measured scanned count.
        scanned: u32,
    },
}

impl ReadError {
    /// Builds a provider failure without carrying an upstream body.
    pub(crate) fn provider(operation: &'static str, reason: impl std::fmt::Display) -> Self {
        Self::Provider {
            operation,
            reason: reason.to_string(),
        }
    }
}

/// Drains one `BatchGetItem` request set, retrying only the unprocessed keys.
///
/// `DynamoDB` reports a throttled key by returning it in `UnprocessedKeys`
/// rather than by failing, and it omits that key's row from `Responses` exactly
/// as it omits a row that does not exist. Every caller therefore decodes only
/// after this returns: until the unprocessed set is empty, a missing row is not
/// evidence of anything.
///
/// # Errors
///
/// Returns [`ReadError::Provider`] when the provider call fails, and when keys
/// are still unprocessed after [`BATCH_GET_UNPROCESSED_RETRIES`] retries.
pub(crate) async fn drain_batch_get(
    dynamodb: &aws_sdk_dynamodb::Client,
    request_items: HashMap<String, KeysAndAttributes>,
    unprocessed_reason: &'static str,
) -> Result<Vec<(String, HashMap<String, AttributeValue>)>, ReadError> {
    let mut pending = request_items;
    let mut returned = Vec::new();
    let mut retries = 0_u8;
    loop {
        let response = dynamodb
            .batch_get_item()
            .set_request_items(Some(pending))
            .send()
            .await
            .map_err(|error| ReadError::provider("BatchGetItem", error))?;
        for (table, items) in response.responses.unwrap_or_default() {
            returned.extend(items.into_iter().map(|item| (table.clone(), item)));
        }
        pending = response.unprocessed_keys.unwrap_or_default();
        if pending.values().all(|keys| keys.keys().is_empty()) {
            return Ok(returned);
        }
        if retries >= BATCH_GET_UNPROCESSED_RETRIES {
            return Err(ReadError::Provider {
                operation: "BatchGetItem",
                reason: unprocessed_reason.to_owned(),
            });
        }
        retries = retries.saturating_add(1);
        tokio::time::sleep(std::time::Duration::from_millis(
            5_u64 << u32::from(retries),
        ))
        .await;
    }
}

/// One page of observations plus what it cost and where it stopped.
#[derive(Clone, Debug)]
pub struct Page {
    /// The observations, in the ordering tuple's order.
    pub items: Vec<Observation>,
    /// What the page spent.
    pub spend: Spend,
    /// Whether more may exist beyond this page.
    pub more: bool,
    /// The last tuple emitted, which the cursor binds.
    pub last: Option<OrderTuple>,
    /// Exact per-segment progress when more may exist.
    pub resume: Option<ObservationResume>,
}

/// One provider page from a physical index segment.
struct SegmentBatch {
    items: Vec<HashMap<String, AttributeValue>>,
    exhausted: bool,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SegmentDescriptor {
    bucket: BucketHour,
    access: Access,
    signal: Signal,
    shard: u8,
}

struct Candidate {
    tuple: OrderTuple,
    observation: Observation,
    row: MapRow,
    resume_key: ResumeKey,
}

struct OpenSegment {
    descriptor: SegmentDescriptor,
    state: SegmentState,
    buffered: VecDeque<Candidate>,
    provider_exhausted: bool,
    opened_this_page: bool,
}

struct SegmentLoad {
    buffered: VecDeque<Candidate>,
    provider_exhausted: bool,
    items_scanned: u32,
    bytes_read: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RefillReservation {
    max_items: u32,
    max_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RefillOutcome {
    Loaded,
    Blocked(&'static str),
}

/// Reserves the provider-hard maximum for one query and its optional base-row
/// hydration. The query limit is sent to `DynamoDB`, so at most `max_items`
/// index/table items and the same number of hydrated base items can arrive.
fn provider_read_reservation(max_items: u32, hydrate: bool) -> u64 {
    let provider_item_max = u64::try_from(DYNAMODB_ITEM_CEILING).unwrap_or(u64::MAX);
    let item_bound = u64::from(max_items).saturating_mul(provider_item_max);
    let query = item_bound.min(DDB_QUERY_MAX_BYTES);
    query.saturating_add(if hydrate { item_bound } else { 0 })
}

/// Finds the largest common provider page that can be reserved for every head
/// the exact merge needs. A refill either reserves the whole wave before any
/// future starts or issues no read at all.
fn reserve_refill_wave(
    hydration: &[bool],
    max_items: u32,
    remaining_bytes: u64,
) -> Option<Vec<RefillReservation>> {
    if hydration.is_empty() {
        return Some(Vec::new());
    }
    let mut lo = 1_u32;
    let mut hi = max_items;
    let mut admitted = 0_u32;
    while lo <= hi {
        let middle = lo + (hi - lo) / 2;
        let reserved = hydration.iter().fold(0_u64, |total, hydrate| {
            total.saturating_add(provider_read_reservation(middle, *hydrate))
        });
        if reserved <= remaining_bytes {
            admitted = middle;
            lo = middle.saturating_add(1);
        } else {
            hi = middle.saturating_sub(1);
        }
    }
    (admitted > 0).then(|| {
        hydration
            .iter()
            .map(|hydrate| RefillReservation {
                max_items: admitted,
                max_bytes: provider_read_reservation(admitted, *hydrate),
            })
            .collect()
    })
}

async fn settle_refills<I, F, T>(refills: I) -> Vec<T>
where
    I: IntoIterator<Item = F>,
    F: std::future::Future<Output = T>,
{
    futures::stream::iter(refills)
        .buffered(MAX_PARALLEL_SEGMENT_READS)
        .collect()
        .await
}

fn validate_refills(
    settled: Vec<(RefillReservation, Result<SegmentLoad, ReadError>)>,
    remaining_items: u32,
    remaining_bytes: u64,
) -> Result<Vec<SegmentLoad>, ReadError> {
    let mut loads = Vec::with_capacity(settled.len());
    let mut failure = None;
    for (reservation, result) in settled {
        match result {
            Ok(load)
                if load.items_scanned <= reservation.max_items
                    && load.bytes_read <= reservation.max_bytes =>
            {
                loads.push(load);
            }
            Ok(load) => {
                failure.get_or_insert_with(|| ReadError::Provider {
                    operation: "QueryBudget",
                    reason: format!(
                        "provider returned {} items / {} bytes after {} items / {} bytes were reserved",
                        load.items_scanned,
                        load.bytes_read,
                        reservation.max_items,
                        reservation.max_bytes
                    ),
                });
            }
            Err(error) => {
                failure.get_or_insert(error);
            }
        }
    }
    if let Some(error) = failure {
        return Err(error);
    }
    let wave_bytes = loads
        .iter()
        .fold(0_u64, |total, load| total.saturating_add(load.bytes_read));
    let wave_items = loads.iter().fold(0_u32, |total, load| {
        total.saturating_add(load.items_scanned)
    });
    if wave_items > remaining_items || wave_bytes > remaining_bytes {
        return Err(ReadError::Provider {
            operation: "QueryBudget",
            reason: format!(
                "settled refill used {wave_items} items / {wave_bytes} bytes with \
                 {remaining_items} items / {remaining_bytes} bytes remaining"
            ),
        });
    }
    Ok(loads)
}

fn segment_needs_hydration(
    descriptor: SegmentDescriptor,
    plan: &aex_observation_query::plan::Plan,
) -> bool {
    matches!(descriptor.access, Access::Metric | Access::Trace)
        || (plan.needs_base_fetch && descriptor.access.index_name().is_some())
}

/// The bounded reader over the observation authority.
#[derive(Clone, Debug)]
pub struct ObservationReader {
    dynamodb: aws_sdk_dynamodb::Client,
    s3: aws_sdk_s3::Client,
    table: String,
    session_table: String,
    bucket: String,
    settle_ms: i64,
    counters: Arc<ReadCounters>,
}

impl ObservationReader {
    /// Binds the reader to its resolved resources and the process's accounting.
    ///
    /// The counters are a constructor argument rather than an optional setter so
    /// a composition that forgets them fails to compile instead of reporting a
    /// silent zero for every read this process makes.
    #[must_use]
    pub fn new(
        dynamodb: aws_sdk_dynamodb::Client,
        s3: aws_sdk_s3::Client,
        table: impl Into<String>,
        session_table: impl Into<String>,
        bucket: impl Into<String>,
        settle_ms: i64,
        counters: Arc<ReadCounters>,
    ) -> Self {
        Self {
            dynamodb,
            s3,
            table: table.into(),
            session_table: session_table.into(),
            bucket: bucket.into(),
            settle_ms,
            counters,
        }
    }

    /// The bound table name.
    #[must_use]
    pub fn table(&self) -> &str {
        &self.table
    }

    /// The process-local read accounting every socket shares.
    #[must_use]
    pub fn counters(&self) -> &Arc<ReadCounters> {
        &self.counters
    }

    /// The bound bucket name.
    #[must_use]
    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// Proves the table and bucket are reachable.
    ///
    /// # Errors
    ///
    /// Returns [`ReadError::Provider`] when either probe fails. A probe that has
    /// not passed is never assumed to have passed.
    pub async fn probe(&self) -> Result<(), ReadError> {
        let observation = async {
            self.dynamodb
                .describe_table()
                .table_name(&self.table)
                .send()
                .await
                .map_err(|error| ReadError::provider("DescribeTable", error))
        };
        let session = async {
            self.dynamodb
                .describe_table()
                .table_name(&self.session_table)
                .send()
                .await
                .map_err(|error| ReadError::provider("DescribeSessionTable", error))
        };
        let bucket = async {
            self.s3
                .head_bucket()
                .bucket(&self.bucket)
                .send()
                .await
                .map_err(|error| ReadError::provider("HeadBucket", error))
        };
        tokio::try_join!(observation, session, bucket)?;
        Ok(())
    }

    /// Reads the deletion fence, every selected signal frontier and the
    /// gap-change hint in one request.
    ///
    /// This is the read an idle follow cycle repeats. It is one `BatchGetItem`
    /// whatever the signal selection is, so a socket's idle cost stops scaling
    /// with the number of signals it follows.
    ///
    /// # Errors
    ///
    /// Returns [`ReadError::Provider`] when the batch cannot be completed and
    /// [`ReadError::Malformed`] when a returned row cannot be decoded. A row the
    /// provider left unprocessed is never mistaken for a row that is absent.
    pub async fn frontier_bundle(
        &self,
        scope: &ScopeKey,
        workspace: WorkspaceId,
        query: &NormalizedQuery,
    ) -> Result<FrontierBundle, ReadError> {
        let request =
            BundleRequest::plan(scope, workspace, query, &self.table, &self.session_table);
        self.counters.record(ReadCounter::FrontierBatch);
        request.read(&self.dynamodb).await?.decode(scope, workspace)
    }

    /// Reads the accepted frontier of one scope across the queried signals.
    ///
    /// # Errors
    ///
    /// Returns [`ReadError::Provider`] when the read fails.
    pub async fn frontier(
        &self,
        scope: &ScopeKey,
        workspace: WorkspaceId,
        query: &NormalizedQuery,
    ) -> Result<Frontier, ReadError> {
        Ok(self
            .frontier_bundle(scope, workspace, query)
            .await?
            .combine(query))
    }

    /// Reads the deletion fence from the authority that owns the queried scope.
    ///
    /// Finite routes need the fence alone. A follow socket needs it beside every
    /// signal frontier and reads [`Self::frontier_bundle`] instead.
    ///
    /// # Errors
    ///
    /// Returns [`ReadError::Provider`] when the authoritative point read fails,
    /// and [`ReadError::Malformed`] when the strongly read fence is incomplete.
    pub async fn deletion_fence(
        &self,
        scope: &ScopeKey,
        workspace: WorkspaceId,
    ) -> Result<(u64, ScopeDeletionState), ReadError> {
        if let ScopeKey::Session(session) = scope {
            let (pk, sk) = aex_session_dynamodb::stream_keys::head(*session);
            let item = self.get_session(&pk, sk).await?;
            return crate::frontier::decode_session_fence(item.as_ref(), workspace);
        }
        let item = self
            .get(&keys::frontier_pk(scope), keys::DELETION_SK)
            .await?;
        crate::frontier::decode_workspace_fence(item.as_ref())
    }

    /// Pins the snapshot every page of one query reads at.
    ///
    /// Everything at or below the pinned snapshot is complete in every index by
    /// construction: admission asserts its clock skew is inside a bound the
    /// settle window dominates.
    #[must_use]
    pub fn pin(&self, frontier: Frontier, now: Timestamp) -> Option<Snapshot> {
        let floor = now.unix_millis().saturating_sub(self.settle_ms);
        let pinned = frontier.accepted_at.unix_millis().min(floor);
        Timestamp::from_unix_millis(pinned.max(0))
            .ok()
            .map(Snapshot::at)
    }

    /// Pins a gap-ledger snapshot to the same measured GSI settle window.
    #[must_use]
    pub fn settled_snapshot(&self, now: Timestamp) -> Option<Snapshot> {
        Timestamp::from_unix_millis(now.unix_millis().saturating_sub(self.settle_ms).max(0))
            .ok()
            .map(Snapshot::at)
    }

    /// Reads one bounded page.
    ///
    /// The walk is ordered, so a page stops at the limit, at a budget, or at the
    /// end of the range, and always reports whether more may exist. A short page
    /// with a cursor is the honest answer; an empty successful page is never
    /// returned in place of an error.
    ///
    /// # Errors
    ///
    /// Returns [`ReadError::BudgetExhausted`] only when the first segment alone
    /// exceeds the scan budget before yielding a single item, and
    /// [`ReadError::Provider`] when the authority is unreachable.
    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "the page state machine binds one query and maintains its exact merge/cursor invariants in one reviewable transaction"
    )]
    pub async fn read_page(
        &self,
        scope: &ScopeKey,
        workspace: WorkspaceId,
        query: &NormalizedQuery,
        plan: &aex_observation_query::plan::Plan,
        snapshot: Snapshot,
        after: Option<&OrderTuple>,
        resume: Option<&ObservationResume>,
        preserve_snapshot_tail: bool,
    ) -> Result<Page, ReadError> {
        if preserve_snapshot_tail
            && query.order_by != aex_observation_domain::order::OrderBy::Accepted
        {
            return Err(ReadError::InvalidResume);
        }
        let groups = Self::segment_groups(scope, query, plan);
        if groups.is_empty() {
            return Ok(Page {
                items: Vec::new(),
                spend: Spend::default(),
                more: false,
                last: None,
                resume: None,
            });
        }

        let (first_group, prior_last) = if let Some(resume) = resume {
            resume.validate().map_err(|_| ReadError::InvalidResume)?;
            let bucket = resume
                .parsed_bucket()
                .map_err(|_| ReadError::InvalidResume)?;
            let index = groups
                .iter()
                .position(|(candidate, _)| *candidate == bucket)
                .ok_or(ReadError::InvalidResume)?;
            (
                index,
                Some(resume.last_order().map_err(|_| ReadError::InvalidResume)?),
            )
        } else {
            (0, after.copied())
        };

        let mut rows = Vec::new();
        let mut spend = Spend::default();
        let mut last_progressed = prior_last;
        let mut last_returned = prior_last;
        let mut progressed_this_page = false;

        for (group_index, (bucket, descriptors)) in groups.iter().enumerate().skip(first_group) {
            let resume_states = if group_index == first_group {
                resume.map(|resume| {
                    resume
                        .segments
                        .iter()
                        .map(|segment| {
                            (
                                (segment.access, segment.signal, segment.shard),
                                segment.state.clone(),
                            )
                        })
                        .collect::<BTreeMap<_, _>>()
                })
            } else {
                None
            };
            if let Some(states) = &resume_states {
                let expected = descriptors
                    .iter()
                    .map(|segment| (segment.access, segment.signal, segment.shard))
                    .collect::<BTreeSet<_>>();
                if states.keys().copied().collect::<BTreeSet<_>>() != expected {
                    return Err(ReadError::InvalidResume);
                }
            }
            let mut open = descriptors
                .iter()
                .map(|descriptor| OpenSegment {
                    descriptor: *descriptor,
                    state: resume_states
                        .as_ref()
                        .and_then(|states| {
                            states.get(&(descriptor.access, descriptor.signal, descriptor.shard))
                        })
                        .cloned()
                        .unwrap_or(SegmentState::Unstarted),
                    buffered: VecDeque::new(),
                    provider_exhausted: false,
                    opened_this_page: false,
                })
                .collect::<Vec<_>>();
            for segment in &mut open {
                if segment.state == SegmentState::Exhausted {
                    segment.provider_exhausted = true;
                }
            }

            loop {
                if let RefillOutcome::Blocked(dimension) = self
                    .refill_segments(scope, workspace, query, plan, &mut open, &mut spend)
                    .await?
                {
                    if !progressed_this_page {
                        return Err(ReadError::BudgetExhausted {
                            dimension,
                            scanned: spend.items_scanned,
                        });
                    }
                    let Some(last) = last_progressed else {
                        return Err(ReadError::InvalidResume);
                    };
                    let resume = page_resume(*bucket, last, &open)?;
                    spend.returned = u32::try_from(rows.len()).unwrap_or(u32::MAX);
                    return Ok(Page {
                        items: rows,
                        spend,
                        more: true,
                        last: Some(last),
                        resume: Some(resume),
                    });
                }

                let next = open
                    .iter()
                    .enumerate()
                    .filter_map(|(index, segment)| {
                        segment.buffered.front().map(|candidate| (index, candidate))
                    })
                    .min_by(|(_, left), (_, right)| {
                        query.direction.compare(&left.tuple, &right.tuple)
                    })
                    .map(|(index, _)| index);
                let Some(index) = next else {
                    break;
                };
                let Some(candidate) = open[index].buffered.pop_front() else {
                    return Err(ReadError::InvalidResume);
                };
                if preserve_snapshot_tail
                    && candidate.observation.accepted_at > snapshot.accepted_at()
                {
                    // Accepted-order provider segments are monotone. A follower
                    // must leave the first row beyond its current snapshot
                    // unconsumed so the next, later snapshot can reveal it.
                    // Returning a global tuple rather than a segment cursor
                    // deliberately re-reads predicate-only trailing progress,
                    // which is the safe side of the no-skip boundary.
                    open[index].buffered.push_front(candidate);
                    spend.returned = u32::try_from(rows.len()).unwrap_or(u32::MAX);
                    return Ok(Page {
                        items: rows,
                        spend,
                        more: false,
                        last: last_returned,
                        resume: None,
                    });
                }
                last_progressed = Some(candidate.tuple);
                progressed_this_page = true;
                open[index].state =
                    if open[index].buffered.is_empty() && open[index].provider_exhausted {
                        SegmentState::Exhausted
                    } else {
                        SegmentState::After(candidate.resume_key.clone())
                    };

                if candidate.observation.accepted_at > snapshot.accepted_at()
                    || !inside(&candidate.tuple, query, prior_last.as_ref())
                    || query
                        .predicate
                        .as_ref()
                        .is_some_and(|predicate| !predicate.evaluate(&candidate.row))
                {
                    continue;
                }
                rows.push(candidate.observation);
                last_returned = Some(candidate.tuple);
                if rows.len() == usize::from(query.limit) {
                    let current_has_more = open
                        .iter()
                        .any(|segment| !segment.buffered.is_empty() || !segment.provider_exhausted);
                    let more = current_has_more || group_index + 1 < groups.len();
                    spend.returned = u32::try_from(rows.len()).unwrap_or(u32::MAX);
                    let resume = more
                        .then(|| page_resume(*bucket, candidate.tuple, &open))
                        .transpose()?;
                    return Ok(Page {
                        items: rows,
                        spend,
                        more,
                        last: Some(candidate.tuple),
                        resume,
                    });
                }
            }
        }

        spend.returned = u32::try_from(rows.len()).unwrap_or(u32::MAX);
        Ok(Page {
            items: rows,
            spend,
            more: false,
            last: last_returned,
            resume: None,
        })
    }

    fn segment_groups(
        scope: &ScopeKey,
        query: &NormalizedQuery,
        plan: &aex_observation_query::plan::Plan,
    ) -> Vec<(BucketHour, Vec<SegmentDescriptor>)> {
        let mut grouped = BTreeMap::<BucketHour, Vec<SegmentDescriptor>>::new();
        for walk in &plan.walks {
            let shard_count = if matches!(
                walk.access,
                Access::SessionAuthority | Access::Metric | Access::Trace
            ) {
                1
            } else {
                BUCKET_SHARDS
            };
            for bucket in Self::ordered_buckets(scope, query, walk.access) {
                for shard in 0..shard_count {
                    grouped.entry(bucket).or_default().push(SegmentDescriptor {
                        bucket,
                        access: walk.access,
                        signal: walk.signal,
                        shard,
                    });
                }
            }
        }
        let mut groups = grouped.into_iter().collect::<Vec<_>>();
        for (_, descriptors) in &mut groups {
            descriptors.sort_by_key(|segment| (segment.signal, segment.access, segment.shard));
        }
        if query.direction == Direction::Descending {
            groups.reverse();
        }
        groups
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "a parallel refill binds the query, plan, open segment set and shared page spend"
    )]
    async fn refill_segments(
        &self,
        scope: &ScopeKey,
        workspace: WorkspaceId,
        query: &NormalizedQuery,
        plan: &aex_observation_query::plan::Plan,
        open: &mut [OpenSegment],
        spend: &mut Spend,
    ) -> Result<RefillOutcome, ReadError> {
        let needy = open
            .iter()
            .enumerate()
            .filter(|(_, segment)| segment.buffered.is_empty() && !segment.provider_exhausted)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if needy.is_empty() {
            return Ok(RefillOutcome::Loaded);
        }
        let newly_opened = needy
            .iter()
            .filter(|index| !open[**index].opened_this_page)
            .count();
        let remaining_segments = plan.budget.max_segments.saturating_sub(spend.segments);
        let remaining_items = plan
            .budget
            .max_items_scanned
            .saturating_sub(spend.items_scanned);
        if usize::from(remaining_segments) < newly_opened {
            return Ok(RefillOutcome::Blocked("segments"));
        }
        if usize::try_from(remaining_items).unwrap_or(usize::MAX) < needy.len() {
            return Ok(RefillOutcome::Blocked("scanned_items"));
        }
        if spend.bytes_read >= plan.budget.max_bytes_read {
            return Ok(RefillOutcome::Blocked("bytes"));
        }
        let max_batch_items = (remaining_items / u32::try_from(needy.len()).unwrap_or(u32::MAX))
            .min(u32::from(query.limit).max(1));
        let remaining_bytes = plan.budget.max_bytes_read.saturating_sub(spend.bytes_read);
        let hydration = needy
            .iter()
            .map(|index| segment_needs_hydration(open[*index].descriptor, plan))
            .collect::<Vec<_>>();
        let Some(reservations) = reserve_refill_wave(&hydration, max_batch_items, remaining_bytes)
        else {
            return Ok(RefillOutcome::Blocked("bytes"));
        };
        for index in &needy {
            if !open[*index].opened_this_page {
                open[*index].opened_this_page = true;
                spend.segments = spend.segments.saturating_add(1);
            }
        }
        let requests = needy
            .iter()
            .zip(reservations)
            .map(|(index, reservation)| {
                (
                    open[*index].descriptor,
                    open[*index].state.clone(),
                    reservation,
                )
            })
            .collect::<Vec<_>>();
        // Every live segment needs a head before the k-way merge may choose its
        // next row, but polling every provider request at once would make the
        // fanout itself unbounded. `buffered` preserves request order while
        // limiting in-flight reads; waiting for the complete set preserves the
        // merge invariant.
        let settled = settle_refills(requests.into_iter().map(
            |(descriptor, state, reservation)| async move {
                let result = self
                    .load_segment(
                        scope,
                        workspace,
                        query,
                        plan,
                        descriptor,
                        &state,
                        reservation.max_items,
                    )
                    .await;
                (reservation, result)
            },
        ))
        .await;
        let loads = validate_refills(settled, remaining_items, remaining_bytes)?;
        for (index, load) in needy.into_iter().zip(loads) {
            spend.items_scanned = spend.items_scanned.saturating_add(load.items_scanned);
            spend.bytes_read = spend.bytes_read.saturating_add(load.bytes_read);
            open[index].provider_exhausted = load.provider_exhausted;
            open[index].buffered = load.buffered;
            if open[index].buffered.is_empty() && open[index].provider_exhausted {
                open[index].state = SegmentState::Exhausted;
            }
        }
        Ok(RefillOutcome::Loaded)
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "loading one immutable segment requires its full physical coordinate and bounded provider page size"
    )]
    async fn load_segment(
        &self,
        scope: &ScopeKey,
        workspace: WorkspaceId,
        query: &NormalizedQuery,
        plan: &aex_observation_query::plan::Plan,
        descriptor: SegmentDescriptor,
        state: &SegmentState,
        batch_items: u32,
    ) -> Result<SegmentLoad, ReadError> {
        let start = match state {
            SegmentState::Unstarted => None,
            SegmentState::After(key) => {
                Some(resume_key_map(scope, workspace, query, descriptor, key)?)
            }
            SegmentState::Exhausted => return Err(ReadError::InvalidResume),
        };
        let batch = self
            .query_segment_page(scope, workspace, query, descriptor, batch_items, start)
            .await?;
        if batch.items.is_empty() && !batch.exhausted {
            return Err(ReadError::Provider {
                operation: "Query",
                reason: "provider returned an empty page with a continuation key".to_owned(),
            });
        }
        let items_scanned = u32::try_from(batch.items.len()).unwrap_or(u32::MAX);
        let mut bytes_read = batch
            .items
            .iter()
            .map(|item| aex_observation_store_aws::store::item_size(item) as u64)
            .sum::<u64>();
        let resume_keys = batch
            .items
            .iter()
            .map(|item| item_resume_key(item, descriptor, scope))
            .collect::<Result<Vec<_>, _>>()?;
        let needs_hydration = segment_needs_hydration(descriptor, plan);
        let items = if needs_hydration {
            let hydrated = self.hydrate_base_rows(batch.items).await?;
            bytes_read = bytes_read.saturating_add(
                hydrated
                    .iter()
                    .map(|item| aex_observation_store_aws::store::item_size(item) as u64)
                    .sum::<u64>(),
            );
            hydrated
        } else {
            batch.items
        };
        if items.len() != resume_keys.len() {
            return Err(ReadError::Malformed { attribute: "pk" });
        }
        let mut buffered = VecDeque::with_capacity(items.len());
        for (item, resume_key) in items.into_iter().zip(resume_keys) {
            let decoded = if descriptor.access == Access::SessionAuthority {
                Self::decode_event(&item, scope, workspace, query.order_by)?
            } else {
                Self::decode(&item, descriptor.signal, query.order_by)?
            }
            .ok_or(ReadError::Malformed {
                attribute: "observationId",
            })?;
            buffered.push_back(Candidate {
                tuple: decoded.0,
                observation: decoded.1,
                row: decoded.2,
                resume_key,
            });
        }
        Ok(SegmentLoad {
            buffered,
            provider_exhausted: batch.exhausted,
            items_scanned,
            bytes_read,
        })
    }

    /// Rehydrates slim GSI rows from the strongly consistent base table before
    /// evaluating residual `attributes.*` predicates.
    async fn hydrate_base_rows(
        &self,
        projected: Vec<HashMap<String, AttributeValue>>,
    ) -> Result<Vec<HashMap<String, AttributeValue>>, ReadError> {
        let mut hydrated = HashMap::with_capacity(projected.len());
        for chunk in projected.chunks(BATCH_GET_MAX_KEYS) {
            let keys = chunk
                .iter()
                .map(primary_key)
                .collect::<Result<Vec<_>, _>>()?;
            let request = aws_sdk_dynamodb::types::KeysAndAttributes::builder()
                .set_keys(Some(keys))
                .consistent_read(true)
                .build()
                .map_err(|error| ReadError::provider("BatchGetItem", error))?;
            let returned = drain_batch_get(
                &self.dynamodb,
                HashMap::from([(self.table.clone(), request)]),
                "provider repeatedly returned unprocessed base-row keys",
            )
            .await?;
            for (_, item) in returned {
                hydrated.insert(primary_key_pair(&item)?, item);
            }
        }

        projected
            .iter()
            .map(primary_key_pair)
            .collect::<Result<Vec<_>, _>>()
            .map(|keys| {
                keys.into_iter()
                    .filter_map(|key| hydrated.remove(&key))
                    .collect()
            })
    }

    /// The non-empty hour buckets one query walks, bounded by its time range.
    fn buckets(query: &NormalizedQuery) -> Vec<BucketHour> {
        let mut buckets = Vec::new();
        let mut current = BucketHour::from_timestamp(query.time_gte);
        let end = query.time_lt.to_datetime();
        while current.start() < end && buckets.len() < usize::from(u16::MAX) {
            buckets.push(current);
            // An exclusive upper bound inside the current hour is terminal.
            // Stopping here also avoids asking for the successor of the last
            // representable wire hour.
            if BucketHour::from_timestamp(query.time_lt) == current {
                break;
            }
            current = current.next();
        }
        buckets
    }

    /// The exact logical hour buckets one access opens, in provider direction.
    ///
    /// Trace partitions span all time and metric partitions span one day, but
    /// they are deliberately reopened through disjoint hour ranges. That keeps
    /// them in the same bucket-major merge as hourly event and dense-index
    /// partitions, so no later-hour row can escape ahead of another signal.
    fn ordered_buckets(
        _scope: &ScopeKey,
        query: &NormalizedQuery,
        _access: Access,
    ) -> Vec<BucketHour> {
        let mut buckets = Self::buckets(query);
        if query.direction == Direction::Descending {
            buckets.reverse();
        }
        buckets
    }

    /// Reads one provider page from exactly one physical segment.
    async fn query_segment_page(
        &self,
        scope: &ScopeKey,
        workspace: WorkspaceId,
        query: &NormalizedQuery,
        descriptor: SegmentDescriptor,
        max_items: u32,
        start: Option<HashMap<String, AttributeValue>>,
    ) -> Result<SegmentBatch, ReadError> {
        if descriptor.access == Access::SessionAuthority {
            return self
                .query_event_segment(scope, workspace, query, descriptor.bucket, max_items, start)
                .await;
        }
        let Some((partition, sort)) = segment_key(
            scope,
            workspace,
            query,
            descriptor.access,
            descriptor.signal,
            descriptor.bucket,
            descriptor.shard,
        ) else {
            return Err(ReadError::Provider {
                operation: "QueryPlan",
                reason: format!(
                    "{:?} requires an exact sparse-index selector",
                    descriptor.access
                ),
            });
        };
        let mut builder = ExpressionBuilder::new();
        let pk_name = builder.name(&partition.attribute);
        let pk_value = builder.string(partition.value);
        let sk_name = builder.name(&sort.attribute);
        let lo = builder.string(sort.lo);
        let hi = builder.string(sort.hi);
        let condition =
            format!("{pk_name} = {pk_value} AND {sk_name} >= {lo} AND {sk_name} < {hi}");

        let names = builder.names();
        let values = builder.values();
        self.counters.record(ReadCounter::ObservationPage);
        let mut request = self
            .dynamodb
            .query()
            .table_name(&self.table)
            .key_condition_expression(condition)
            .set_expression_attribute_names(Some(names))
            .set_expression_attribute_values(Some(values))
            .set_exclusive_start_key(start)
            .limit(i32::try_from(max_items).unwrap_or(i32::MAX))
            .scan_index_forward(matches!(query.direction, Direction::Ascending));
        if let Some(index) = descriptor.access.index_name() {
            request = request.index_name(index);
        } else {
            request = request.consistent_read(true);
        }
        let response = request
            .send()
            .await
            .map_err(|error| ReadError::provider("Query", error))?;
        Ok(SegmentBatch {
            exhausted: response.last_evaluated_key.is_none(),
            items: response.items.unwrap_or_default(),
        })
    }

    /// One bounded native-event query against the semantic session authority.
    async fn query_event_segment(
        &self,
        scope: &ScopeKey,
        workspace: WorkspaceId,
        query: &NormalizedQuery,
        bucket: BucketHour,
        max_items: u32,
        start: Option<HashMap<String, AttributeValue>>,
    ) -> Result<SegmentBatch, ReadError> {
        let (index, partition_attribute, sort_attribute, partition) = match scope {
            ScopeKey::Session(session) => (
                aex_session_dynamodb::stream_keys::SESSION_EVENT_INDEX,
                aex_session_dynamodb::stream_keys::SESSION_EVENT_PK,
                aex_session_dynamodb::stream_keys::SESSION_EVENT_SK,
                aex_session_dynamodb::stream_keys::session_event_partition_hour(
                    *session,
                    bucket.as_str(),
                ),
            ),
            ScopeKey::Workspace(_) => (
                aex_session_dynamodb::stream_keys::WORKSPACE_EVENT_INDEX,
                aex_session_dynamodb::stream_keys::WORKSPACE_EVENT_PK,
                aex_session_dynamodb::stream_keys::WORKSPACE_EVENT_SK,
                aex_session_dynamodb::stream_keys::workspace_event_partition_hour(
                    workspace,
                    bucket.as_str(),
                ),
            ),
        };
        let mut builder = ExpressionBuilder::new();
        let pk_name = builder.name(partition_attribute);
        let pk_value = builder.string(partition);
        let sk_name = builder.name(sort_attribute);
        let lo = builder.string(query.time_gte.to_wire());
        let hi = builder.string(query.time_lt.to_wire());
        let condition =
            format!("{pk_name} = {pk_value} AND {sk_name} >= {lo} AND {sk_name} < {hi}");
        let names = builder.names();
        let values = builder.values();
        self.counters.record(ReadCounter::ObservationPage);
        let response = self
            .dynamodb
            .query()
            .table_name(&self.session_table)
            .index_name(index)
            .key_condition_expression(condition)
            .set_expression_attribute_names(Some(names))
            .set_expression_attribute_values(Some(values))
            .set_exclusive_start_key(start)
            .limit(i32::try_from(max_items).unwrap_or(i32::MAX))
            .scan_index_forward(matches!(query.direction, Direction::Ascending))
            .send()
            .await
            .map_err(|error| ReadError::provider("QuerySessionEvents", error))?;
        Ok(SegmentBatch {
            exhausted: response.last_evaluated_key.is_none(),
            items: response.items.unwrap_or_default(),
        })
    }

    /// Decodes one stored item into the wire shape and its filter row.
    fn decode(
        item: &HashMap<String, AttributeValue>,
        signal: Signal,
        order_by: aex_observation_domain::order::OrderBy,
    ) -> Result<Option<(OrderTuple, Observation, MapRow)>, ReadError> {
        let Some(id) =
            string(item, "observationId").and_then(|text| text.parse::<ObservationId>().ok())
        else {
            return Ok(None);
        };
        let time = timestamp(item, "time").ok_or(ReadError::Malformed { attribute: "time" })?;
        let accepted_at = timestamp(item, "acceptedAt").ok_or(ReadError::Malformed {
            attribute: "acceptedAt",
        })?;
        let workspace = string(item, "workspaceId")
            .and_then(|text| text.parse::<WorkspaceId>().ok())
            .ok_or(ReadError::Malformed {
                attribute: "workspaceId",
            })?;
        let revision = number(item, "revision").unwrap_or(1);
        let accepted_seq = number(item, "acceptedSeq").unwrap_or(0);
        let body = body_json(item);
        let observation = Observation {
            accepted_at,
            body,
            id,
            observed_at: time,
            run_id: string(item, "runId").and_then(|text| text.parse::<RunId>().ok()),
            sequence: DecimalU128::new(u128::from(accepted_seq)),
            session_id: string(item, "sessionId").and_then(|text| text.parse::<SessionId>().ok()),
            signal: signal.to_wire(),
            span_id: string(item, "spanId").and_then(|text| SpanId::parse(&text).ok()),
            trace_id: string(item, "traceId").and_then(|text| TraceId::parse(&text).ok()),
            workspace_id: workspace,
        };
        let tuple = OrderTuple::new(
            match order_by {
                aex_observation_domain::order::OrderBy::Time => time,
                aex_observation_domain::order::OrderBy::Accepted => accepted_at,
            },
            signal,
            id,
            revision,
        );
        Ok(Some((tuple, observation, filter_row(item))))
    }

    /// Decodes one native session event into the shared observation wire shape.
    fn decode_event(
        item: &HashMap<String, AttributeValue>,
        scope: &ScopeKey,
        asserted_workspace: WorkspaceId,
        order_by: aex_observation_domain::order::OrderBy,
    ) -> Result<Option<(OrderTuple, Observation, MapRow)>, ReadError> {
        use aex_session_dynamodb::wire_pending::Body;

        let event =
            aex_session_dynamodb::event::decode(item).map_err(|_| ReadError::Malformed {
                attribute: "session_event",
            })?;
        if event.workspace != asserted_workspace {
            return Err(ReadError::Malformed {
                attribute: "workspaceId",
            });
        }
        let session = string(item, "sessionId")
            .and_then(|text| text.parse::<SessionId>().ok())
            .ok_or(ReadError::Malformed {
                attribute: "sessionId",
            })?;
        if let ScopeKey::Session(asserted_session) = scope
            && session != *asserted_session
        {
            return Err(ReadError::Malformed {
                attribute: "sessionId",
            });
        }
        let body = match event.body {
            Body::Inline(bytes) => std::str::from_utf8(&bytes)
                .ok()
                .and_then(|text| aex_wire::canonical::CanonicalJson::parse(text).ok())
                .ok_or(ReadError::Malformed {
                    attribute: "bodyInline",
                })?,
            Body::Digest(digest) => aex_wire::canonical::CanonicalJson::from_value(
                &serde_json::json!({ "bodySha256": digest }),
            )
            .map_err(|_| ReadError::Malformed {
                attribute: "bodyDigest",
            })?,
        };
        let tuple = OrderTuple::new(
            match order_by {
                aex_observation_domain::order::OrderBy::Time
                | aex_observation_domain::order::OrderBy::Accepted => event.occurred_at,
            },
            Signal::Events,
            event.event_id,
            1,
        );
        let observation = Observation {
            accepted_at: event.occurred_at,
            body,
            id: event.event_id,
            observed_at: event.occurred_at,
            run_id: event.run,
            sequence: DecimalU128::new(u128::from(event.event_seq)),
            session_id: Some(session),
            signal: ObservationSignal::Events,
            span_id: None,
            trace_id: None,
            workspace_id: event.workspace,
        };
        let mut row_item = item.clone();
        row_item.insert(
            "time".to_owned(),
            AttributeValue::S(event.occurred_at.to_wire()),
        );
        row_item.insert(
            "acceptedAt".to_owned(),
            AttributeValue::S(event.occurred_at.to_wire()),
        );
        Ok(Some((tuple, observation, filter_row(&row_item))))
    }

    /// A single `GetItem` on the bound table.
    async fn get(
        &self,
        pk: &str,
        sk: &str,
    ) -> Result<Option<HashMap<String, AttributeValue>>, ReadError> {
        let response = self
            .dynamodb
            .get_item()
            .table_name(&self.table)
            .key(PK, AttributeValue::S(pk.to_owned()))
            .key(SK, AttributeValue::S(sk.to_owned()))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| ReadError::provider("GetItem", error))?;
        Ok(response.item)
    }

    /// A strongly consistent point read on the peer-owned session authority.
    async fn get_session(
        &self,
        pk: &str,
        sk: &str,
    ) -> Result<Option<HashMap<String, AttributeValue>>, ReadError> {
        self.counters.record(ReadCounter::SessionHead);
        let response = self
            .dynamodb
            .get_item()
            .table_name(&self.session_table)
            .key(PK, AttributeValue::S(pk.to_owned()))
            .key(SK, AttributeValue::S(sk.to_owned()))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| ReadError::provider("GetSessionHead", error))?;
        Ok(response.item)
    }

    /// Reads every item of one partition under a bounded sort-key range.
    ///
    /// # Errors
    ///
    /// Returns [`ReadError::Provider`] when the read fails.
    pub(crate) async fn read_range(
        &self,
        index: Option<Index>,
        partition: &KeyBinding,
        sort: &RangeBinding,
        limit: u16,
    ) -> Result<Vec<HashMap<String, AttributeValue>>, ReadError> {
        let mut builder = ExpressionBuilder::new();
        let pk_name = builder.name(&partition.attribute);
        let pk_value = builder.string(partition.value.clone());
        let sk_name = builder.name(&sort.attribute);
        let lo = builder.string(sort.lo.clone());
        let hi = builder.string(sort.hi.clone());
        let condition = format!("{pk_name} = {pk_value} AND {sk_name} BETWEEN {lo} AND {hi}");
        let mut request = self
            .dynamodb
            .query()
            .table_name(&self.table)
            .key_condition_expression(condition)
            .set_expression_attribute_names(Some(builder.names()))
            .set_expression_attribute_values(Some(builder.values()))
            .limit(i32::from(limit));
        if let Some(index) = index {
            request = request.index_name(index.as_str());
        } else {
            request = request.consistent_read(true);
        }
        let response = request
            .send()
            .await
            .map_err(|error| ReadError::provider("Query", error))?;
        Ok(response.items.unwrap_or_default())
    }

    /// Reads a `KEYS_ONLY` index range and rehydrates its base rows in the same
    /// order before returning them.
    ///
    /// # Errors
    ///
    /// Returns [`ReadError::Provider`] when either the index query or bounded
    /// base-table hydration fails.
    pub(crate) async fn read_hydrated_range(
        &self,
        index: Index,
        partition: &KeyBinding,
        sort: &RangeBinding,
        limit: u16,
    ) -> Result<Vec<HashMap<String, AttributeValue>>, ReadError> {
        let projected = self.read_range(Some(index), partition, sort, limit).await?;
        self.hydrate_base_rows(projected).await
    }

    /// Reads the latest immutable revision of one gap by its authoritative key.
    ///
    /// # Errors
    ///
    /// Returns [`ReadError`] when the authoritative lookup fails or returns a
    /// malformed gap row.
    pub async fn latest_gap(
        &self,
        scope: &ScopeKey,
        gap: TelemetryGapId,
    ) -> Result<Option<GapRecord>, ReadError> {
        let mut builder = ExpressionBuilder::new();
        let pk = builder.name(PK);
        let pk_value = builder.string(keys::gap_pk(scope));
        let sk = builder.name(SK);
        let prefix = builder.string(format!("{gap}#"));
        let response = self
            .dynamodb
            .query()
            .table_name(&self.table)
            .key_condition_expression(format!("{pk} = {pk_value} AND begins_with({sk}, {prefix})"))
            .set_expression_attribute_names(Some(builder.names()))
            .set_expression_attribute_values(Some(builder.values()))
            .consistent_read(true)
            .scan_index_forward(false)
            .limit(1)
            .send()
            .await
            .map_err(|error| ReadError::provider("QueryGap", error))?;
        response
            .items()
            .first()
            .map(decode_gap)
            .transpose()
            .map_err(|_| ReadError::Malformed {
                attribute: "telemetry_gap",
            })
    }

    /// Reads the latest revision of every gap visible at one settled snapshot.
    ///
    /// Session reads are strongly consistent base-table queries. Workspace reads
    /// use the sparse index to discover keys, then strongly hydrate full base
    /// rows in bounded `BatchGetItem` calls before applying lifecycle semantics.
    ///
    /// # Errors
    ///
    /// Returns [`ReadError`] when the lookup, hydration, or canonical decoding
    /// of a durable gap revision fails.
    pub async fn gap_history(
        &self,
        scope: &ScopeKey,
        workspace: WorkspaceId,
        snapshot: Snapshot,
    ) -> Result<Vec<GapRecord>, ReadError> {
        let (index, partition_attribute, partition_value) = match scope {
            ScopeKey::Session(_) => (None, PK, keys::gap_pk(scope)),
            ScopeKey::Workspace(_) => (
                Some(Index::Gap),
                Index::Gap.partition_key(),
                format!("GAPW#{workspace}"),
            ),
        };
        let mut builder = ExpressionBuilder::new();
        let partition = builder.name(partition_attribute);
        let value = builder.string(partition_value);
        let mut start = None;
        let mut rows = Vec::new();
        loop {
            self.counters.record(ReadCounter::GapHistory);
            let mut request = self
                .dynamodb
                .query()
                .table_name(&self.table)
                .key_condition_expression(format!("{partition} = {value}"))
                .set_expression_attribute_names(Some(builder.names()))
                .set_expression_attribute_values(Some(builder.values()))
                .set_exclusive_start_key(start.take())
                .limit(250);
            if let Some(index) = index {
                request = request.index_name(index.as_str());
            } else {
                request = request.consistent_read(true);
            }
            let response = request
                .send()
                .await
                .map_err(|error| ReadError::provider("QueryGaps", error))?;
            rows.extend(response.items.unwrap_or_default());
            if rows.len() > GAP_REVISION_BUDGET as usize {
                return Err(ReadError::BudgetExhausted {
                    dimension: "gap_revisions",
                    scanned: u32::try_from(rows.len()).unwrap_or(u32::MAX),
                });
            }
            start = response.last_evaluated_key;
            if start.is_none() {
                break;
            }
        }
        if index.is_some() {
            rows = self.hydrate_gap_rows(&rows).await?;
        }
        let mut latest = BTreeMap::<TelemetryGapId, GapRecord>::new();
        for item in &rows {
            let record = decode_gap(item).map_err(|_| ReadError::Malformed {
                attribute: "telemetry_gap",
            })?;
            let belongs = match scope {
                ScopeKey::Session(_) => record.scope == *scope,
                ScopeKey::Workspace(_) => record.workspace == workspace,
            };
            if !belongs || record.revision.revised_at > snapshot.accepted_at() {
                continue;
            }
            match latest.get(&record.revision.gap_id) {
                Some(found) if found.revision.revision >= record.revision.revision => {}
                _ => {
                    latest.insert(record.revision.gap_id, record);
                }
            }
        }
        Ok(latest.into_values().collect())
    }

    async fn hydrate_gap_rows(
        &self,
        projected: &[HashMap<String, AttributeValue>],
    ) -> Result<Vec<HashMap<String, AttributeValue>>, ReadError> {
        let mut hydrated = Vec::with_capacity(projected.len());
        for chunk in projected.chunks(100) {
            let keys: Result<Vec<_>, _> = chunk
                .iter()
                .map(|item| {
                    Ok(HashMap::from([
                        (
                            PK.to_owned(),
                            item.get(PK)
                                .cloned()
                                .ok_or(ReadError::Malformed { attribute: PK })?,
                        ),
                        (
                            SK.to_owned(),
                            item.get(SK)
                                .cloned()
                                .ok_or(ReadError::Malformed { attribute: SK })?,
                        ),
                    ]))
                })
                .collect();
            let request = KeysAndAttributes::builder()
                .set_keys(Some(keys?))
                .consistent_read(true)
                .build()
                .map_err(|error| ReadError::provider("BatchGetItem", error))?;
            let response = self
                .dynamodb
                .batch_get_item()
                .request_items(self.table.clone(), request)
                .send()
                .await
                .map_err(|error| ReadError::provider("BatchGetItem", error))?;
            hydrated.extend(
                response
                    .responses
                    .and_then(|mut responses| responses.remove(&self.table))
                    .unwrap_or_default(),
            );
            if response
                .unprocessed_keys
                .as_ref()
                .and_then(|unprocessed| unprocessed.get(&self.table))
                .is_some_and(|keys| !keys.keys().is_empty())
            {
                return Err(ReadError::Provider {
                    operation: "BatchGetItem",
                    reason: "unprocessed gap keys remained".to_owned(),
                });
            }
        }
        if hydrated.len() != projected.len() {
            return Err(ReadError::Malformed {
                attribute: "telemetry_gap",
            });
        }
        Ok(hydrated)
    }

    /// Reads one control or state item by its exact key.
    ///
    /// # Errors
    ///
    /// Returns [`ReadError::Provider`] when the read fails.
    pub async fn read_item(
        &self,
        pk: &str,
        sk: &str,
    ) -> Result<Option<HashMap<String, AttributeValue>>, ReadError> {
        self.get(pk, sk).await
    }

    /// Writes one export-control item under a caller-supplied condition.
    ///
    /// # Errors
    ///
    /// Returns [`ReadError::Provider`] when the write fails, including when the
    /// condition did not hold — losing a fenced write is a normal outcome that
    /// the caller branches on rather than retries.
    pub(crate) async fn put_export_control(
        &self,
        item: HashMap<String, AttributeValue>,
        condition: &str,
        names: HashMap<String, String>,
        values: HashMap<String, AttributeValue>,
    ) -> Result<(), ReadError> {
        validate_export_control_item(&item)?;
        self.dynamodb
            .put_item()
            .table_name(&self.table)
            .set_item(Some(item))
            .condition_expression(condition)
            .set_expression_attribute_names(Some(names))
            .set_expression_attribute_values(Some(values))
            .send()
            .await
            .map_err(|error| ReadError::provider("PutItem", error))?;
        Ok(())
    }

    /// Updates one export-control item under a caller-supplied condition.
    ///
    /// # Errors
    ///
    /// Returns [`ReadError::Provider`] when the update fails, including when the
    /// condition did not hold.
    pub(crate) async fn update_export_control(
        &self,
        pk: &str,
        sk: &str,
        update: &str,
        condition: &str,
        names: HashMap<String, String>,
        values: HashMap<String, AttributeValue>,
    ) -> Result<(), ReadError> {
        if !is_export_control_key(pk, sk) {
            return Err(ReadError::InvalidWriteTarget {
                operation: "UpdateItem",
            });
        }
        self.dynamodb
            .update_item()
            .table_name(&self.table)
            .key(PK, AttributeValue::S(pk.to_owned()))
            .key(SK, AttributeValue::S(sk.to_owned()))
            .update_expression(update)
            .condition_expression(condition)
            .set_expression_attribute_names(Some(names))
            .set_expression_attribute_values(Some(values))
            .send()
            .await
            .map_err(|error| ReadError::provider("UpdateItem", error))?;
        Ok(())
    }

    /// Mints a presigned download for one ready export object.
    ///
    /// # Errors
    ///
    /// Returns [`ReadError::Provider`] when the presign fails.
    pub async fn presign_export(
        &self,
        key: &str,
        lifetime: std::time::Duration,
    ) -> Result<String, ReadError> {
        let config = aws_sdk_s3::presigning::PresigningConfig::expires_in(lifetime)
            .map_err(|error| ReadError::provider("PresigningConfig", error))?;
        let request = self
            .s3
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .presigned(config)
            .await
            .map_err(|error| ReadError::provider("GetObject", error))?;
        Ok(request.uri().to_owned())
    }
}

/// One resolved partition-key binding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyBinding {
    /// The attribute the key lives on.
    pub attribute: String,
    /// The exact key value.
    pub value: String,
}

/// One resolved sort-key range.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RangeBinding {
    /// The attribute the range applies to.
    pub attribute: String,
    /// The inclusive lower bound.
    pub lo: String,
    /// The upper bound. Segment reads treat it as exclusive; utility reads may
    /// elect to include it for prefix-style ranges.
    pub hi: String,
}

/// Resolves the exact index partition and range one segment reads.
///
/// Returns `None` for the one access pattern that is not in this table.
#[must_use]
pub fn segment_key(
    scope: &ScopeKey,
    workspace: WorkspaceId,
    query: &NormalizedQuery,
    access: Access,
    signal: Signal,
    bucket: BucketHour,
    shard: u8,
) -> Option<(KeyBinding, RangeBinding)> {
    let signal_name = signal.as_str();
    let (partition, attribute, sort_attribute) = match access {
        Access::SessionAuthority => return None,
        Access::BaseTable | Access::Gap => (
            keys::observation_pk(scope, signal, bucket, shard),
            PK.to_owned(),
            SK.to_owned(),
        ),
        Access::ScopeTime => (
            format!(
                "OBT#{}#{signal_name}#{}#{shard:02}",
                scope.to_key(),
                bucket.as_str()
            ),
            "tPk".to_owned(),
            "tSk".to_owned(),
        ),
        Access::WorkspaceAccepted => (
            format!(
                "OBWA#{workspace}#{signal_name}#{}#{shard:02}",
                bucket.as_str()
            ),
            "wPk".to_owned(),
            "wSk".to_owned(),
        ),
        Access::WorkspaceTime => (
            format!(
                "OBWT#{workspace}#{signal_name}#{}#{shard:02}",
                bucket.as_str()
            ),
            "wtPk".to_owned(),
            "wtSk".to_owned(),
        ),
        Access::Trace => (
            format!("TRC#{}#{}", scope.to_key(), query.trace_id.as_deref()?),
            "trPk".to_owned(),
            "trSk".to_owned(),
        ),
        Access::Metric => (
            format!(
                "MET#{workspace}#{}#{}",
                query.metric_name.as_deref()?,
                bucket.day()
            ),
            "mPk".to_owned(),
            "mSk".to_owned(),
        ),
    };
    let bucket_start = Timestamp::from_datetime_trunc_ms(bucket.start()).ok()?;
    let lo = query.time_gte.max(bucket_start);
    let terminal_bucket = BucketHour::from_timestamp(query.time_lt);
    let hi = if terminal_bucket == bucket {
        query.time_lt
    } else {
        Timestamp::from_datetime_trunc_ms(bucket.next().start()).ok()?
    };
    Some((
        KeyBinding {
            attribute,
            value: partition,
        },
        RangeBinding {
            attribute: sort_attribute,
            // Every physical ordering key begins with the fixed-width primary
            // timestamp. Prefix bounds therefore select precisely this
            // logical half-open hour without fabricating a tuple sentinel.
            lo: lo.to_wire(),
            hi: hi.to_wire(),
        },
    ))
}

fn page_resume(
    bucket: BucketHour,
    last: OrderTuple,
    open: &[OpenSegment],
) -> Result<ObservationResume, ReadError> {
    ObservationResume::new(
        bucket,
        last,
        open.iter()
            .map(|segment| SegmentResume {
                access: segment.descriptor.access,
                signal: segment.descriptor.signal,
                shard: segment.descriptor.shard,
                state: segment.state.clone(),
            })
            .collect(),
    )
    .map_err(|_| ReadError::InvalidResume)
}

fn item_resume_key(
    item: &HashMap<String, AttributeValue>,
    descriptor: SegmentDescriptor,
    asserted_scope: &ScopeKey,
) -> Result<ResumeKey, ReadError> {
    if descriptor.access == Access::SessionAuthority {
        let session = string(item, "sessionId")
            .and_then(|value| value.parse::<SessionId>().ok())
            .ok_or(ReadError::Malformed {
                attribute: "sessionId",
            })?;
        if let ScopeKey::Session(asserted) = asserted_scope
            && session != *asserted
        {
            return Err(ReadError::Malformed {
                attribute: "sessionId",
            });
        }
        let occurred = timestamp(item, "occurredAt").ok_or(ReadError::Malformed {
            attribute: "occurredAt",
        })?;
        return Ok(ResumeKey {
            scope: ScopeKey::Session(session).to_key(),
            primary_ms: occurred.unix_millis(),
            accepted_ms: occurred.unix_millis(),
            observation_id: string(item, "eventId").ok_or(ReadError::Malformed {
                attribute: "eventId",
            })?,
            revision: 1,
            base_shard: 0,
            event_seq: Some(number(item, "eventSeq").ok_or(ReadError::Malformed {
                attribute: "eventSeq",
            })?),
        });
    }

    let base = string(item, PK)
        .as_deref()
        .and_then(keys::parse_observation_pk)
        .ok_or(ReadError::Malformed { attribute: PK })?;
    if base.signal != descriptor.signal {
        return Err(ReadError::Malformed {
            attribute: "signal",
        });
    }
    let primary = match descriptor.access {
        Access::BaseTable | Access::WorkspaceAccepted => timestamp(item, "acceptedAt"),
        _ => timestamp(item, "time"),
    }
    .ok_or(ReadError::Malformed { attribute: "time" })?;
    let accepted = timestamp(item, "acceptedAt").ok_or(ReadError::Malformed {
        attribute: "acceptedAt",
    })?;
    Ok(ResumeKey {
        scope: base.scope.to_key(),
        primary_ms: primary.unix_millis(),
        accepted_ms: accepted.unix_millis(),
        observation_id: string(item, "observationId").ok_or(ReadError::Malformed {
            attribute: "observationId",
        })?,
        revision: number(item, "revision").unwrap_or(1),
        base_shard: base.shard,
        event_seq: None,
    })
}

fn resume_key_map(
    scope: &ScopeKey,
    workspace: WorkspaceId,
    query: &NormalizedQuery,
    descriptor: SegmentDescriptor,
    key: &ResumeKey,
) -> Result<HashMap<String, AttributeValue>, ReadError> {
    key.validate().map_err(|_| ReadError::InvalidResume)?;
    let row_scope = ScopeKey::parse(&key.scope).map_err(|_| ReadError::InvalidResume)?;
    let primary =
        Timestamp::from_unix_millis(key.primary_ms).map_err(|_| ReadError::InvalidResume)?;
    let accepted =
        Timestamp::from_unix_millis(key.accepted_ms).map_err(|_| ReadError::InvalidResume)?;
    let observation_id = key
        .observation_id
        .parse::<ObservationId>()
        .map_err(|_| ReadError::InvalidResume)?;
    let primary_sk = order_sort_key(OrderTuple::new(
        primary,
        descriptor.signal,
        observation_id,
        key.revision,
    ));

    if descriptor.access == Access::SessionAuthority {
        let session = row_scope.session().ok_or(ReadError::InvalidResume)?;
        if let ScopeKey::Session(asserted) = scope
            && session != *asserted
        {
            return Err(ReadError::InvalidResume);
        }
        let event_seq = key.event_seq.ok_or(ReadError::InvalidResume)?;
        let (resume_partition_attribute, resume_sort_attribute, partition) = match scope {
            ScopeKey::Session(asserted) => (
                aex_session_dynamodb::stream_keys::SESSION_EVENT_PK,
                aex_session_dynamodb::stream_keys::SESSION_EVENT_SK,
                aex_session_dynamodb::stream_keys::session_event_partition_hour(
                    *asserted,
                    descriptor.bucket.as_str(),
                ),
            ),
            ScopeKey::Workspace(_) => (
                aex_session_dynamodb::stream_keys::WORKSPACE_EVENT_PK,
                aex_session_dynamodb::stream_keys::WORKSPACE_EVENT_SK,
                aex_session_dynamodb::stream_keys::workspace_event_partition_hour(
                    workspace,
                    descriptor.bucket.as_str(),
                ),
            ),
        };
        return Ok(HashMap::from([
            (
                PK.to_owned(),
                AttributeValue::S(format!("SESSION#{session}")),
            ),
            (
                SK.to_owned(),
                AttributeValue::S(format!("EVT#{}", keys::pad_seq(u128::from(event_seq)))),
            ),
            (
                resume_partition_attribute.to_owned(),
                AttributeValue::S(partition),
            ),
            (
                resume_sort_attribute.to_owned(),
                AttributeValue::S(primary_sk),
            ),
        ]));
    }

    if key.event_seq.is_some() || !descriptor.signal.in_observation_authority() {
        return Err(ReadError::InvalidResume);
    }
    let accepted_bucket = BucketHour::from_timestamp(accepted);
    let authority_partition = keys::observation_pk(
        &row_scope,
        descriptor.signal,
        accepted_bucket,
        key.base_shard,
    );
    let authority_sort = order_sort_key(OrderTuple::new(
        accepted,
        descriptor.signal,
        observation_id,
        key.revision,
    ));
    let mut result = HashMap::from([
        (PK.to_owned(), AttributeValue::S(authority_partition)),
        (SK.to_owned(), AttributeValue::S(authority_sort)),
    ]);
    if let Some((partition, sort)) = segment_key(
        scope,
        workspace,
        query,
        descriptor.access,
        descriptor.signal,
        descriptor.bucket,
        descriptor.shard,
    ) {
        if descriptor.access.index_name().is_some() {
            result.insert(partition.attribute, AttributeValue::S(partition.value));
            result.insert(sort.attribute, AttributeValue::S(primary_sk));
        }
    } else {
        return Err(ReadError::InvalidResume);
    }
    Ok(result)
}

/// Whether one tuple is inside the query's range and after the cursor.
fn inside(tuple: &OrderTuple, query: &NormalizedQuery, after: Option<&OrderTuple>) -> bool {
    let primary = tuple.primary.unix_millis();
    if primary < query.time_gte.unix_millis() || primary >= query.time_lt.unix_millis() {
        return false;
    }
    match (after, query.direction) {
        (None, _) => true,
        (Some(previous), Direction::Ascending) => tuple > previous,
        (Some(previous), Direction::Descending) => tuple < previous,
    }
}

/// The canonical body, inline or as a reference the caller can resolve.
fn body_json(item: &HashMap<String, AttributeValue>) -> aex_wire::canonical::CanonicalJson {
    if let Some(blob) = item.get("bodyInline").and_then(|value| value.as_b().ok())
        && let Ok(text) = std::str::from_utf8(blob.as_ref())
        && let Ok(canonical) = aex_wire::canonical::CanonicalJson::parse(text)
    {
        return canonical;
    }
    let reference = serde_json::json!({
        "bodyS3Key": string(item, "bodyS3Key").unwrap_or_default(),
        "bodySha256": string(item, "bodySha256").unwrap_or_default(),
    });
    aex_wire::canonical::CanonicalJson::from_value(&reference).unwrap_or_else(|_| {
        aex_wire::canonical::CanonicalJson::parse("{}").expect("the empty object is canonical")
    })
}

/// Projects one stored item onto the filter row the residual evaluator reads.
fn filter_row(item: &HashMap<String, AttributeValue>) -> MapRow {
    let mut fields = BTreeMap::new();
    let mut attributes = BTreeMap::new();
    if let Some(indexed) = item.get("indexed").and_then(|value| value.as_m().ok()) {
        for (name, value) in indexed {
            if let Some(canonical) = canonical(value) {
                fields.insert(name.clone(), canonical);
            }
        }
    }
    for name in [
        "time",
        "acceptedAt",
        "type",
        "sessionId",
        "runId",
        "traceId",
        "spanId",
        "metricName",
    ] {
        if let Some(text) = string(item, name) {
            fields.insert(name.to_owned(), CanonicalValue::Str(text.into()));
        }
    }
    for source in ["attrS", "attrN", "attrB"] {
        if let Some(map) = item.get(source).and_then(|value| value.as_m().ok()) {
            for (name, value) in map {
                if let Some(canonical) = canonical(value) {
                    attributes.insert(name.clone(), canonical);
                }
            }
        }
    }
    MapRow { fields, attributes }
}

/// Converts one stored attribute into a canonical value.
fn canonical(value: &AttributeValue) -> Option<CanonicalValue> {
    match value {
        AttributeValue::S(text) => Some(CanonicalValue::Str(text.clone().into_boxed_str())),
        AttributeValue::N(text) => {
            text.parse::<i64>()
                .map(CanonicalValue::Int)
                .ok()
                .or_else(|| {
                    text.parse::<f64>()
                        .ok()
                        .and_then(|value| CanonicalValue::number(value).ok())
                })
        }
        AttributeValue::Bool(flag) => Some(CanonicalValue::Bool(*flag)),
        AttributeValue::Null(_) => Some(CanonicalValue::Null),
        _ => None,
    }
}

/// Extracts the exact base-table key carried by every GSI projection.
fn primary_key(
    item: &HashMap<String, AttributeValue>,
) -> Result<HashMap<String, AttributeValue>, ReadError> {
    let (pk, sk) = primary_key_pair(item)?;
    Ok(HashMap::from([
        (PK.to_owned(), AttributeValue::S(pk)),
        (SK.to_owned(), AttributeValue::S(sk)),
    ]))
}

/// Extracts a stable lookup key while proving both key attributes are strings.
pub(crate) fn primary_key_pair(
    item: &HashMap<String, AttributeValue>,
) -> Result<(String, String), ReadError> {
    let pk = string(item, PK).ok_or(ReadError::Malformed { attribute: PK })?;
    let sk = string(item, SK).ok_or(ReadError::Malformed { attribute: SK })?;
    Ok((pk, sk))
}

/// The only key family the finite observation API may mutate.
fn is_export_control_key(pk: &str, sk: &str) -> bool {
    pk.starts_with("EXPORT#") && sk == "STATE"
}

/// Proves a new row is the exact export-control shape before any provider call.
fn validate_export_control_item(item: &HashMap<String, AttributeValue>) -> Result<(), ReadError> {
    let pk = string(item, PK).ok_or(ReadError::InvalidWriteTarget {
        operation: "PutItem",
    })?;
    let sk = string(item, SK).ok_or(ReadError::InvalidWriteTarget {
        operation: "PutItem",
    })?;
    if is_export_control_key(&pk, &sk) && string(item, "itemType").as_deref() == Some("export") {
        Ok(())
    } else {
        Err(ReadError::InvalidWriteTarget {
            operation: "PutItem",
        })
    }
}

/// Reads a string attribute.
pub(crate) fn string(item: &HashMap<String, AttributeValue>, name: &str) -> Option<String> {
    item.get(name).and_then(|value| value.as_s().ok()).cloned()
}

/// Reads a numeric attribute.
pub(crate) fn number(item: &HashMap<String, AttributeValue>, name: &str) -> Option<u64> {
    item.get(name)
        .and_then(|value| value.as_n().ok())
        .and_then(|text| text.parse().ok())
}

/// Reads a fixed-width timestamp attribute.
pub(crate) fn timestamp(item: &HashMap<String, AttributeValue>, name: &str) -> Option<Timestamp> {
    string(item, name).and_then(|text| Timestamp::parse(&text).ok())
}

/// The wire signal of one stored item.
pub(crate) fn stored_signal(item: &HashMap<String, AttributeValue>) -> Option<ObservationSignal> {
    string(item, "signal")
        .and_then(|text| Signal::parse(&text))
        .map(Signal::to_wire)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use aex_observation_domain::keys::{self, BucketHour, ScopeKey};
    use aex_observation_domain::order::{Direction, OrderBy, OrderTuple, order_sort_key};
    use aex_observation_domain::signal::{Signal, SignalSet};
    use aex_observation_query::plan::{Access, Budget, NormalizedQuery, ScopeAxis, plan};
    use aex_session_dynamodb::measure::DYNAMODB_ITEM_CEILING;
    use aex_wire::ids::{ObservationId, PrefixedId as _, SessionId, WorkspaceId};
    use aex_wire::models::ObservationSignal;
    use aex_wire::types::Timestamp;
    use aws_sdk_dynamodb::config::{BehaviorVersion, Credentials, Region};
    use aws_sdk_dynamodb::primitives::Blob;
    use aws_sdk_dynamodb::types::AttributeValue;
    use aws_smithy_http_client::test_util::{ReplayEvent, StaticReplayClient};
    use aws_smithy_types::body::SdkBody;

    use super::{
        BUCKET_SHARDS, MAX_PARALLEL_SEGMENT_READS, ObservationReader, ReadError, SegmentDescriptor,
        filter_row, is_export_control_key, item_resume_key, provider_read_reservation,
        reserve_refill_wave, resume_key_map, segment_key, settle_refills,
        validate_export_control_item,
    };
    use crate::counters::{ReadCounter, ReadCounters};
    use crate::frontier::{GapChange, ScopeDeletionState};
    use crate::gap_watch::{CycleTrigger, GAP_RECOVERY_INTERVAL, GapObservation, GapWatch};

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(aex_wire::Uuid7::compose(1, [2; 10]))
    }

    fn bucket() -> BucketHour {
        BucketHour::from_timestamp(Timestamp::from_unix_millis(1_754_051_696_789).expect("bounded"))
    }

    fn session() -> SessionId {
        SessionId::from_uuid7(aex_wire::Uuid7::compose(1, [4; 10]))
    }

    fn normalized_query() -> NormalizedQuery {
        NormalizedQuery {
            axis: ScopeAxis::Workspace,
            signals: SignalSet::from_signal(Signal::Logs),
            predicate: None,
            time_gte: Timestamp::from_unix_millis(1_754_051_696_000).expect("bounded"),
            time_lt: Timestamp::from_unix_millis(1_754_055_296_000).expect("bounded"),
            order_by: OrderBy::Time,
            direction: Direction::Ascending,
            limit: 100,
            trace_id: Some("0123456789abcdef0123456789abcdef".into()),
            metric_name: Some("http.server.duration".into()),
        }
    }

    fn replaying_reader(response: &str) -> (ObservationReader, StaticReplayClient) {
        replaying_reader_responses(&[response])
    }

    fn replaying_reader_responses(responses: &[&str]) -> (ObservationReader, StaticReplayClient) {
        let replay = StaticReplayClient::new(
            responses
                .iter()
                .map(|response| {
                    ReplayEvent::new(
                        http::Request::builder()
                            .method("POST")
                            .uri("https://dynamodb.eu-west-1.amazonaws.com/")
                            .body(SdkBody::empty())
                            .expect("a request"),
                        http::Response::builder()
                            .status(200)
                            .body(SdkBody::from((*response).to_owned()))
                            .expect("a response"),
                    )
                })
                .collect(),
        );
        let dynamodb = aws_sdk_dynamodb::Client::from_conf(
            aws_sdk_dynamodb::Config::builder()
                .behavior_version(BehaviorVersion::latest())
                .region(Region::new("eu-west-1"))
                .credentials_provider(Credentials::new(
                    "AKIDTESTTESTTESTTEST",
                    "test-secret",
                    None,
                    None,
                    "aex-tests",
                ))
                .http_client(replay.clone())
                .build(),
        );
        let s3 = aws_sdk_s3::Client::from_conf(
            aws_sdk_s3::Config::builder()
                .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
                .region(aws_sdk_s3::config::Region::new("eu-west-1"))
                .credentials_provider(aws_sdk_s3::config::Credentials::new(
                    "AKIDTESTTESTTESTTEST",
                    "test-secret",
                    None,
                    None,
                    "aex-tests",
                ))
                .build(),
        );
        (
            ObservationReader::new(
                dynamodb,
                s3,
                "observation-authority",
                "session-authority",
                "observations",
                2_000,
                std::sync::Arc::new(ReadCounters::default()),
            ),
            replay,
        )
    }

    fn observation_response(
        scope: &ScopeKey,
        accepted: Timestamp,
        observed: Timestamp,
        id: ObservationId,
        revision: u64,
    ) -> String {
        let pk = keys::observation_pk(scope, Signal::Logs, BucketHour::from_timestamp(accepted), 0);
        let sk = order_sort_key(OrderTuple::new(accepted, Signal::Logs, id, revision));
        format!(
            r#"{{"Items":[{{"pk":{{"S":"{pk}"}},"sk":{{"S":"{sk}"}},"observationId":{{"S":"{id}"}},"revision":{{"N":"{revision}"}},"signal":{{"S":"logs"}},"workspaceId":{{"S":"{}"}},"time":{{"S":"{}"}},"acceptedAt":{{"S":"{}"}},"acceptedSeq":{{"N":"7"}},"bodyInline":{{"B":"e30="}},"indexed":{{"M":{{}}}}}}],"Count":1,"ScannedCount":1}}"#,
            workspace(),
            observed.to_wire(),
            accepted.to_wire(),
        )
    }

    #[test]
    fn write_keys_are_confined_to_export_control_state() {
        assert!(is_export_control_key("EXPORT#workspace#export", "STATE"));
        assert!(!is_export_control_key("FRONTIER#workspace", "STATE"));
        assert!(!is_export_control_key(
            "EXPORT#workspace#export",
            "CHECKPOINT"
        ));
    }

    #[test]
    fn a_put_refuses_admission_and_frontier_rows_before_the_provider() {
        for (pk, sk, item_type) in [
            ("OBS#scope#logs", "record", "observation"),
            ("FRONTIER#scope", "logs", "frontier"),
            ("EXPORT#workspace#export", "STATE", "frontier"),
        ] {
            let item = HashMap::from([
                ("pk".to_owned(), AttributeValue::S(pk.to_owned())),
                ("sk".to_owned(), AttributeValue::S(sk.to_owned())),
                (
                    "itemType".to_owned(),
                    AttributeValue::S(item_type.to_owned()),
                ),
            ]);
            assert!(matches!(
                validate_export_control_item(&item),
                Err(ReadError::InvalidWriteTarget {
                    operation: "PutItem"
                })
            ));
        }
    }

    #[tokio::test]
    async fn keys_only_rows_are_hydrated_before_residual_evaluation() {
        let (reader, _replay) = replaying_reader(
            r#"{"Responses":{"observation-authority":[{"pk":{"S":"OBS#scope#logs"},"sk":{"S":"record"},"attrS":{"M":{"environment":{"S":"production"}}}}]},"UnprocessedKeys":{}}"#,
        );
        let projected = vec![HashMap::from([
            (
                "pk".to_owned(),
                AttributeValue::S("OBS#scope#logs".to_owned()),
            ),
            ("sk".to_owned(), AttributeValue::S("record".to_owned())),
        ])];

        let hydrated = reader
            .hydrate_base_rows(projected)
            .await
            .expect("the base row hydrates");

        assert_eq!(hydrated.len(), 1);
        assert!(
            filter_row(&hydrated[0])
                .attributes
                .contains_key("environment")
        );
    }

    #[tokio::test]
    async fn an_exhausted_page_keeps_the_exact_returned_revision_as_its_follow_position() {
        let scope = ScopeKey::Workspace(workspace());
        let accepted = Timestamp::parse("2026-08-01T09:01:00.000Z").expect("accepted");
        let observed = Timestamp::parse("2026-08-01T09:00:30.000Z").expect("observed");
        let id = ObservationId::from_uuid7(aex_wire::Uuid7::compose(
            u64::try_from(accepted.unix_millis()).expect("positive fixture"),
            [8; 10],
        ));
        let item = observation_response(&scope, accepted, observed, id, 7);
        let empty = r#"{"Items":[],"Count":0,"ScannedCount":0}"#;
        let (reader, _replay) = replaying_reader_responses(&[&item, empty, empty, empty]);
        let mut query = normalized_query();
        query.trace_id = None;
        query.metric_name = None;
        query.time_gte = Timestamp::parse("2026-08-01T09:00:00.000Z").expect("range");
        query.time_lt = Timestamp::parse("2026-08-01T10:00:00.000Z").expect("range");
        let planned = plan(&query, Budget::default()).expect("plans");
        let page = reader
            .read_page(
                &scope,
                workspace(),
                &query,
                &planned,
                aex_observation_query::coverage::Snapshot::at(
                    Timestamp::parse("2026-08-01T09:02:00.000Z").expect("snapshot"),
                ),
                None,
                None,
                false,
            )
            .await
            .expect("page reads");

        assert_eq!(page.items.len(), 1);
        assert_eq!(page.last.expect("follow position").revision, 7);
    }

    #[tokio::test]
    async fn a_follower_does_not_advance_past_a_row_beyond_its_current_snapshot() {
        let scope = ScopeKey::Workspace(workspace());
        let accepted = Timestamp::parse("2026-08-01T09:03:00.000Z").expect("accepted");
        let id = ObservationId::from_uuid7(aex_wire::Uuid7::compose(
            u64::try_from(accepted.unix_millis()).expect("positive fixture"),
            [9; 10],
        ));
        let item = observation_response(&scope, accepted, accepted, id, 2);
        let empty = r#"{"Items":[],"Count":0,"ScannedCount":0}"#;
        let (reader, _replay) = replaying_reader_responses(&[&item, empty, empty, empty]);
        let mut query = normalized_query();
        query.trace_id = None;
        query.metric_name = None;
        query.order_by = OrderBy::Accepted;
        query.time_gte = Timestamp::parse("2026-08-01T09:00:00.000Z").expect("range");
        query.time_lt = Timestamp::parse("2026-08-01T10:00:00.000Z").expect("range");
        let planned = plan(&query, Budget::default()).expect("plans");
        let page = reader
            .read_page(
                &scope,
                workspace(),
                &query,
                &planned,
                aex_observation_query::coverage::Snapshot::at(
                    Timestamp::parse("2026-08-01T09:02:00.000Z").expect("snapshot"),
                ),
                None,
                None,
                true,
            )
            .await
            .expect("page reads");

        assert!(page.items.is_empty());
        assert!(page.last.is_none());
        assert!(page.resume.is_none());
        assert!(!page.more);
    }

    /// One `BatchGetItem` reply carrying the rows a workspace bundle requested.
    fn workspace_bundle_response(scope: &ScopeKey, signals: &[(Signal, &str)]) -> String {
        bundle_response_with_hint(scope, signals, None)
    }

    /// A workspace bundle reply, optionally carrying a published hint row.
    ///
    /// `None` is the reply of a workspace that has never had a gap, which is the
    /// common case and the one the hint has to make cheap.
    fn bundle_response_with_hint(
        scope: &ScopeKey,
        signals: &[(Signal, &str)],
        appends: Option<u64>,
    ) -> String {
        let rows = std::iter::once(format!(
            r#"{{"pk":{{"S":"{}"}},"sk":{{"S":"{}"}},"deletionEpoch":{{"N":"4"}},"state":{{"S":"none"}}}}"#,
            keys::frontier_pk(scope),
            keys::DELETION_SK,
        ))
        .chain(signals.iter().map(|(signal, accepted)| {
            format!(
                r#"{{"pk":{{"S":"{}"}},"sk":{{"S":"{}"}},"acceptedAt":{{"S":"{accepted}"}}}}"#,
                keys::frontier_pk(scope),
                keys::frontier_sk(*signal),
            )
        }))
        .chain(appends.map(|appends| {
            format!(
                r#"{{"pk":{{"S":"{}"}},"sk":{{"S":"{}"}},"itemType":{{"S":"gap_change_hint"}},"gapAppends":{{"N":"{appends}"}}}}"#,
                keys::gap_hint_pk(workspace()),
                keys::GAP_HINT_SK,
            )
        }))
        .collect::<Vec<_>>()
        .join(",");
        format!(r#"{{"Responses":{{"observation-authority":[{rows}]}},"UnprocessedKeys":{{}}}}"#)
    }

    fn provider_requests(replay: &StaticReplayClient) -> usize {
        replay.actual_requests().count()
    }

    #[tokio::test]
    async fn a_workspace_frontier_bundle_uses_exactly_one_provider_request() {
        let scope = ScopeKey::Workspace(workspace());
        let mut query = normalized_query();
        query.signals = SignalSet::from_signal(Signal::Logs)
            .with(Signal::Spans)
            .with(Signal::Metrics)
            .with(Signal::Traces);
        let response = workspace_bundle_response(
            &scope,
            &[
                (Signal::Logs, "2026-08-01T09:05:00.000Z"),
                (Signal::Spans, "2026-08-01T09:04:00.000Z"),
                (Signal::Metrics, "2026-08-01T09:06:00.000Z"),
                (Signal::Traces, "2026-08-01T09:07:00.000Z"),
            ],
        );
        let (reader, replay) = replaying_reader(&response);

        let bundle = reader
            .frontier_bundle(&scope, workspace(), &query)
            .await
            .expect("the bundle reads");

        assert_eq!(
            provider_requests(&replay),
            1,
            "four signals and a fence are one request, not five"
        );
        assert_eq!(bundle.signals.len(), 4);
        assert_eq!(bundle.deletion_epoch, 4);
        assert_eq!(bundle.deletion_state, ScopeDeletionState::Open);
        assert_eq!(
            bundle.combine(&query).accepted_at,
            Timestamp::parse("2026-08-01T09:04:00.000Z").expect("the minimum frontier")
        );
        assert_eq!(reader.counters().total(ReadCounter::FrontierBatch), 1);
        assert_eq!(
            reader.counters().total(ReadCounter::SessionHead),
            0,
            "a workspace fence is not a session head read"
        );
    }

    #[tokio::test]
    async fn a_session_frontier_bundle_uses_exactly_one_multi_table_provider_request() {
        let scope = ScopeKey::Session(session());
        let mut query = normalized_query();
        query.signals = SignalSet::from_signal(Signal::Logs).with(Signal::Spans);
        let (partition, sort) = aex_session_dynamodb::stream_keys::head(session());
        let response = format!(
            r#"{{"Responses":{{"session-authority":[{{"pk":{{"S":"{partition}"}},"sk":{{"S":"{sort}"}},"itemType":{{"S":"session_head"}},"workspaceId":{{"S":"{}"}},"lifecycle":{{"S":"active"}},"deletionEpoch":{{"N":"11"}}}}],"observation-authority":[{{"pk":{{"S":"{}"}},"sk":{{"S":"{}"}},"acceptedAt":{{"S":"2026-08-01T09:05:00.000Z"}}}},{{"pk":{{"S":"{}"}},"sk":{{"S":"{}"}},"acceptedAt":{{"S":"2026-08-01T09:03:00.000Z"}}}}]}},"UnprocessedKeys":{{}}}}"#,
            workspace(),
            keys::frontier_pk(&scope),
            keys::frontier_sk(Signal::Logs),
            keys::frontier_pk(&scope),
            keys::frontier_sk(Signal::Spans),
        );
        let (reader, replay) = replaying_reader(&response);

        let bundle = reader
            .frontier_bundle(&scope, workspace(), &query)
            .await
            .expect("the bundle reads");

        assert_eq!(
            provider_requests(&replay),
            1,
            "two authorities are one BatchGetItem, not one call each"
        );
        assert_eq!(bundle.deletion_epoch, 11);
        assert_eq!(bundle.deletion_state, ScopeDeletionState::Open);
        assert_eq!(bundle.signals.len(), 2);
        assert_eq!(
            reader.counters().total(ReadCounter::SessionHead),
            0,
            "the session head rides in the batch rather than costing its own read"
        );
    }

    #[tokio::test]
    async fn unprocessed_frontier_keys_are_retried_and_then_fail_closed() {
        let scope = ScopeKey::Workspace(workspace());
        let mut query = normalized_query();
        query.signals = SignalSet::from_signal(Signal::Logs);
        let throttled = format!(
            r#"{{"Responses":{{}},"UnprocessedKeys":{{"observation-authority":{{"Keys":[{{"pk":{{"S":"{}"}},"sk":{{"S":"{}"}}}}],"ConsistentRead":true}}}}}}"#,
            keys::frontier_pk(&scope),
            keys::frontier_sk(Signal::Logs),
        );
        let (reader, replay) =
            replaying_reader_responses(&[&throttled, &throttled, &throttled, &throttled]);

        let error = reader
            .frontier_bundle(&scope, workspace(), &query)
            .await
            .expect_err("an undrained batch is never a complete bundle");

        assert!(
            matches!(&error, ReadError::Provider { operation, reason }
                if *operation == "BatchGetItem"
                    && reason.contains("unprocessed frontier keys")),
            "{error}"
        );
        assert_eq!(
            provider_requests(&replay),
            4,
            "one attempt plus three bounded retries"
        );
    }

    #[tokio::test]
    async fn the_gap_change_hint_rides_the_bundle_rather_than_costing_a_request() {
        let scope = ScopeKey::Workspace(workspace());
        let mut query = normalized_query();
        query.signals = SignalSet::from_signal(Signal::Logs);
        let response = bundle_response_with_hint(
            &scope,
            &[(Signal::Logs, "2026-08-01T09:05:00.000Z")],
            Some(12),
        );
        let (reader, replay) = replaying_reader(&response);

        let bundle = reader
            .frontier_bundle(&scope, workspace(), &query)
            .await
            .expect("the bundle reads");

        assert_eq!(
            provider_requests(&replay),
            1,
            "detecting `unchanged` must cost nothing beyond the read already made"
        );
        assert!(
            matches!(bundle.gap_change, GapChange::Counted(count) if count.get() == 12),
            "{:?}",
            bundle.gap_change
        );
        assert_eq!(reader.counters().total(ReadCounter::FrontierBatch), 1);
    }

    #[tokio::test]
    async fn a_workspace_with_no_hint_row_reports_absence_rather_than_a_count() {
        let scope = ScopeKey::Workspace(workspace());
        let mut query = normalized_query();
        query.signals = SignalSet::from_signal(Signal::Logs);
        let response =
            bundle_response_with_hint(&scope, &[(Signal::Logs, "2026-08-01T09:05:00.000Z")], None);
        let (reader, _replay) = replaying_reader(&response);

        let bundle = reader
            .frontier_bundle(&scope, workspace(), &query)
            .await
            .expect("the bundle reads");

        assert_eq!(bundle.gap_change, GapChange::Unpublished);
    }

    #[tokio::test]
    async fn a_throttled_hint_row_never_becomes_an_unchanged_one() {
        // The hint obeys the rule the rest of the bundle obeys: a row the
        // provider left unprocessed comes back looking exactly like a row that
        // does not exist, and absence is a value the reader acts on. Decoding
        // before the batch drained would turn a throttle into "nothing was
        // appended", which is the one answer that suppresses a ledger read.
        let scope = ScopeKey::Workspace(workspace());
        let mut query = normalized_query();
        query.signals = SignalSet::from_signal(Signal::Logs);
        let partial = format!(
            r#"{{"Responses":{{"observation-authority":[{{"pk":{{"S":"{}"}},"sk":{{"S":"{}"}},"deletionEpoch":{{"N":"4"}},"state":{{"S":"none"}}}},{{"pk":{{"S":"{}"}},"sk":{{"S":"{}"}},"acceptedAt":{{"S":"2026-08-01T09:05:00.000Z"}}}}]}},"UnprocessedKeys":{{"observation-authority":{{"Keys":[{{"pk":{{"S":"{}"}},"sk":{{"S":"{}"}}}}],"ConsistentRead":true}}}}}}"#,
            keys::frontier_pk(&scope),
            keys::DELETION_SK,
            keys::frontier_pk(&scope),
            keys::frontier_sk(Signal::Logs),
            keys::gap_hint_pk(workspace()),
            keys::GAP_HINT_SK,
        );
        let (reader, replay) =
            replaying_reader_responses(&[&partial, &partial, &partial, &partial]);

        let error = reader
            .frontier_bundle(&scope, workspace(), &query)
            .await
            .expect_err("an undrained hint key is not an absent hint row");

        assert!(
            matches!(&error, ReadError::Provider { operation, .. } if *operation == "BatchGetItem"),
            "{error}"
        );
        assert_eq!(
            provider_requests(&replay),
            4,
            "one attempt plus three bounded retries"
        );
    }

    #[tokio::test]
    async fn an_unchanged_hint_stops_an_idle_cycle_from_querying_gap_history() {
        // Four idle follow cycles over a workspace whose hint stands still. Every
        // cycle still reads its bundle; only the first reads the ledger.
        let scope = ScopeKey::Workspace(workspace());
        let mut query = normalized_query();
        query.signals = SignalSet::from_signal(Signal::Logs);
        let bundle = bundle_response_with_hint(
            &scope,
            &[(Signal::Logs, "2026-08-01T09:05:00.000Z")],
            Some(3),
        );
        let empty_ledger = r#"{"Items":[],"Count":0,"ScannedCount":0}"#.to_owned();
        let (reader, replay) =
            replaying_reader_responses(&[&bundle, &empty_ledger, &bundle, &bundle, &bundle]);
        let snapshot = aex_observation_query::coverage::Snapshot::at(
            Timestamp::parse("2026-08-01T09:02:00.000Z").expect("snapshot"),
        );
        let mut watch = GapWatch::new(GAP_RECOVERY_INTERVAL);
        let start = std::time::Instant::now();
        let mut gaps = Vec::new();

        for cycle in 0..4 {
            let read = reader
                .frontier_bundle(&scope, workspace(), &query)
                .await
                .expect("the bundle reads");
            gaps = watch
                .gap_history(
                    GapObservation::Observed {
                        change: read.gap_change,
                        trigger: CycleTrigger::Fallback,
                    },
                    reader.counters(),
                    start + std::time::Duration::from_secs(15) * cycle,
                    gaps,
                    reader.gap_history(&scope, workspace(), snapshot),
                )
                .await
                .expect("the ledger reads when it is asked to");
        }

        assert_eq!(reader.counters().total(ReadCounter::FrontierBatch), 4);
        assert_eq!(
            reader.counters().total(ReadCounter::GapHistory),
            1,
            "four idle cycles read the ledger once, not four times"
        );
        assert_eq!(
            reader.counters().total(ReadCounter::GapHintUnreadable),
            0,
            "a readable hint is not a defect"
        );
        assert_eq!(
            provider_requests(&replay),
            5,
            "four bundles and the one ledger read they scheduled"
        );
    }

    #[tokio::test]
    async fn an_unreadable_hint_costs_the_ledger_read_it_exists_to_avoid_and_is_counted() {
        let scope = ScopeKey::Workspace(workspace());
        let mut query = normalized_query();
        query.signals = SignalSet::from_signal(Signal::Logs);
        let corrupt = format!(
            r#"{{"Responses":{{"observation-authority":[{{"pk":{{"S":"{}"}},"sk":{{"S":"{}"}},"deletionEpoch":{{"N":"4"}},"state":{{"S":"none"}}}},{{"pk":{{"S":"{}"}},"sk":{{"S":"{}"}},"acceptedAt":{{"S":"2026-08-01T09:05:00.000Z"}}}},{{"pk":{{"S":"{}"}},"sk":{{"S":"{}"}},"itemType":{{"S":"gap_change_hint"}},"gapAppends":{{"S":"three"}}}}]}},"UnprocessedKeys":{{}}}}"#,
            keys::frontier_pk(&scope),
            keys::DELETION_SK,
            keys::frontier_pk(&scope),
            keys::frontier_sk(Signal::Logs),
            keys::gap_hint_pk(workspace()),
            keys::GAP_HINT_SK,
        );
        let empty_ledger = r#"{"Items":[],"Count":0,"ScannedCount":0}"#.to_owned();
        let (reader, replay) =
            replaying_reader_responses(&[&corrupt, &empty_ledger, &corrupt, &empty_ledger]);
        let snapshot = aex_observation_query::coverage::Snapshot::at(
            Timestamp::parse("2026-08-01T09:02:00.000Z").expect("snapshot"),
        );
        let mut watch = GapWatch::new(GAP_RECOVERY_INTERVAL);
        let start = std::time::Instant::now();
        let mut gaps = Vec::new();

        for cycle in 0..2 {
            let read = reader
                .frontier_bundle(&scope, workspace(), &query)
                .await
                .expect("a corrupt hint never fails the bundle every socket shares");
            assert_eq!(read.gap_change, GapChange::Unreadable);
            gaps = watch
                .gap_history(
                    GapObservation::Observed {
                        change: read.gap_change,
                        trigger: CycleTrigger::Fallback,
                    },
                    reader.counters(),
                    start + std::time::Duration::from_secs(15) * cycle,
                    gaps,
                    reader.gap_history(&scope, workspace(), snapshot),
                )
                .await
                .expect("the ledger reads");
        }

        assert_eq!(
            reader.counters().total(ReadCounter::GapHistory),
            2,
            "an unreadable hint never means unchanged"
        );
        assert_eq!(reader.counters().total(ReadCounter::GapHintUnreadable), 2);
        assert_eq!(provider_requests(&replay), 4);
    }

    #[tokio::test]
    async fn a_throttled_frontier_row_never_becomes_an_absent_one() {
        // The same reply shape means "this row does not exist" and "this row was
        // not read". Only the drained unprocessed set separates them, so a
        // partial batch must not decode into a scope with no deletion fence.
        let scope = ScopeKey::Workspace(workspace());
        let mut query = normalized_query();
        query.signals = SignalSet::from_signal(Signal::Logs);
        let partial = format!(
            r#"{{"Responses":{{"observation-authority":[{{"pk":{{"S":"{}"}},"sk":{{"S":"{}"}},"acceptedAt":{{"S":"2026-08-01T09:05:00.000Z"}}}}]}},"UnprocessedKeys":{{"observation-authority":{{"Keys":[{{"pk":{{"S":"{}"}},"sk":{{"S":"{}"}}}}],"ConsistentRead":true}}}}}}"#,
            keys::frontier_pk(&scope),
            keys::frontier_sk(Signal::Logs),
            keys::frontier_pk(&scope),
            keys::DELETION_SK,
        );
        let (reader, _replay) =
            replaying_reader_responses(&[&partial, &partial, &partial, &partial]);

        assert!(matches!(
            reader.frontier_bundle(&scope, workspace(), &query).await,
            Err(ReadError::Provider { .. })
        ));
    }

    #[tokio::test]
    async fn a_malformed_frontier_row_is_refused_rather_than_defaulted() {
        let scope = ScopeKey::Workspace(workspace());
        let mut query = normalized_query();
        query.signals = SignalSet::from_signal(Signal::Logs);
        let response = format!(
            r#"{{"Responses":{{"observation-authority":[{{"pk":{{"S":"{}"}},"sk":{{"S":"{}"}},"deletionEpoch":{{"N":"1"}},"state":{{"S":"none"}}}},{{"pk":{{"S":"{}"}},"sk":{{"S":"{}"}},"earliestAcceptedAt":{{"S":"2026-08-01T09:00:00.000Z"}}}}]}},"UnprocessedKeys":{{}}}}"#,
            keys::frontier_pk(&scope),
            keys::DELETION_SK,
            keys::frontier_pk(&scope),
            keys::frontier_sk(Signal::Logs),
        );
        let (reader, _replay) = replaying_reader(&response);

        assert!(matches!(
            reader.frontier_bundle(&scope, workspace(), &query).await,
            Err(ReadError::Malformed {
                attribute: "acceptedAt"
            })
        ));
    }

    #[tokio::test]
    async fn a_frontier_bundle_refuses_a_row_it_did_not_request() {
        let scope = ScopeKey::Workspace(workspace());
        let mut query = normalized_query();
        query.signals = SignalSet::from_signal(Signal::Logs);
        let response = format!(
            r#"{{"Responses":{{"observation-authority":[{{"pk":{{"S":"{}"}},"sk":{{"S":"{}"}},"acceptedAt":{{"S":"2026-08-01T09:05:00.000Z"}}}}]}},"UnprocessedKeys":{{}}}}"#,
            keys::frontier_pk(&ScopeKey::Session(session())),
            keys::frontier_sk(Signal::Logs),
        );
        let (reader, _replay) = replaying_reader(&response);

        let error = reader
            .frontier_bundle(&scope, workspace(), &query)
            .await
            .expect_err("a foreign row is never decoded into this scope");

        assert!(
            matches!(&error, ReadError::Provider { reason, .. } if reason.contains("did not request")),
            "{error}"
        );
    }

    #[tokio::test]
    async fn a_representative_idle_cycle_reports_the_reads_it_actually_made() {
        let scope = ScopeKey::Workspace(workspace());
        let mut query = normalized_query();
        query.trace_id = None;
        query.metric_name = None;
        query.order_by = OrderBy::Accepted;
        query.time_gte = Timestamp::parse("2026-08-01T09:00:00.000Z").expect("range");
        query.time_lt = Timestamp::parse("2026-08-01T10:00:00.000Z").expect("range");
        let bundle =
            workspace_bundle_response(&scope, &[(Signal::Logs, "2026-08-01T09:30:00.000Z")]);
        let empty = r#"{"Items":[],"Count":0,"ScannedCount":0}"#;
        let (reader, replay) =
            replaying_reader_responses(&[&bundle, empty, empty, empty, empty, empty]);
        let snapshot = aex_observation_query::coverage::Snapshot::at(
            Timestamp::parse("2026-08-01T09:02:00.000Z").expect("snapshot"),
        );
        let planned = plan(&query, Budget::default()).expect("plans");

        reader
            .frontier_bundle(&scope, workspace(), &query)
            .await
            .expect("the bundle reads");
        reader
            .gap_history(&scope, workspace(), snapshot)
            .await
            .expect("the gap ledger reads");
        reader
            .read_page(
                &scope,
                workspace(),
                &query,
                &planned,
                snapshot,
                None,
                None,
                true,
            )
            .await
            .expect("the page reads");

        let counters = reader.counters();
        assert_eq!(counters.total(ReadCounter::FrontierBatch), 1);
        assert_eq!(counters.total(ReadCounter::GapHistory), 1);
        assert_eq!(
            counters.total(ReadCounter::ObservationPage),
            u64::from(BUCKET_SHARDS)
        );
        assert_eq!(counters.total(ReadCounter::SessionHead), 0);
        assert_eq!(
            provider_requests(&replay),
            2 + usize::from(BUCKET_SHARDS),
            "the counters account for every request the cycle actually made"
        );
    }

    #[test]
    fn every_access_pattern_names_its_own_index_attributes() {
        let scope = ScopeKey::Workspace(workspace());
        let query = normalized_query();
        for (access, expected_pk, expected_sk) in [
            (Access::BaseTable, "pk", "sk"),
            (Access::ScopeTime, "tPk", "tSk"),
            (Access::WorkspaceAccepted, "wPk", "wSk"),
            (Access::WorkspaceTime, "wtPk", "wtSk"),
            (Access::Trace, "trPk", "trSk"),
            (Access::Metric, "mPk", "mSk"),
        ] {
            let (partition, sort) = segment_key(
                &scope,
                workspace(),
                &query,
                access,
                Signal::Logs,
                bucket(),
                0,
            )
            .expect("the access pattern is in this table");
            assert_eq!(partition.attribute, expected_pk);
            assert_eq!(sort.attribute, expected_sk);
            assert!(!partition.value.is_empty());
        }
    }

    #[test]
    fn the_events_signal_is_never_read_from_this_table() {
        let scope = ScopeKey::Workspace(workspace());
        let query = normalized_query();
        assert!(
            segment_key(
                &scope,
                workspace(),
                &query,
                Access::SessionAuthority,
                Signal::Events,
                bucket(),
                0
            )
            .is_none(),
            "events live in session-authority and are read through the port"
        );
    }

    #[test]
    fn sparse_reader_partitions_match_the_writer_templates() {
        let scope = ScopeKey::Workspace(workspace());
        let query = normalized_query();
        let (trace, trace_range) = segment_key(
            &scope,
            workspace(),
            &query,
            Access::Trace,
            Signal::Spans,
            bucket(),
            0,
        )
        .expect("the trace selector is exact");
        assert_eq!(
            trace.value,
            format!("TRC#{}#0123456789abcdef0123456789abcdef", scope.to_key())
        );
        assert_eq!(trace_range.lo, query.time_gte.to_wire());
        assert_eq!(
            trace_range.hi,
            Timestamp::from_datetime_trunc_ms(bucket().next().start())
                .expect("next hour")
                .to_wire()
        );

        let (metric, range) = segment_key(
            &scope,
            workspace(),
            &query,
            Access::Metric,
            Signal::Metrics,
            bucket(),
            0,
        )
        .expect("the metric selector is exact");
        assert_eq!(
            metric.value,
            format!(
                "MET#{}#http.server.duration#{}",
                workspace(),
                bucket().day()
            )
        );
        assert_eq!(range.lo, query.time_gte.to_wire());
        assert_eq!(
            range.hi,
            Timestamp::from_datetime_trunc_ms(bucket().next().start())
                .expect("next hour")
                .to_wire()
        );
    }

    #[test]
    fn sparse_access_without_its_exact_selector_fails_closed() {
        let scope = ScopeKey::Workspace(workspace());
        let mut query = normalized_query();
        query.trace_id = None;
        assert!(
            segment_key(
                &scope,
                workspace(),
                &query,
                Access::Trace,
                Signal::Spans,
                bucket(),
                0,
            )
            .is_none()
        );
        query.metric_name = None;
        assert!(
            segment_key(
                &scope,
                workspace(),
                &query,
                Access::Metric,
                Signal::Metrics,
                bucket(),
                0,
            )
            .is_none()
        );
    }

    #[test]
    fn a_consumed_index_row_reconstructs_the_exact_provider_resume_key() {
        let scope = ScopeKey::Session(session());
        let signal = Signal::Logs;
        let time = Timestamp::parse("2025-08-01T09:02:03.004Z").expect("time");
        let accepted = Timestamp::parse("2025-08-01T09:02:04.005Z").expect("accepted");
        let id = ObservationId::from_uuid7(aex_wire::Uuid7::compose(1, [9; 10]));
        let descriptor = SegmentDescriptor {
            bucket: BucketHour::from_timestamp(time),
            access: Access::ScopeTime,
            signal,
            shard: 2,
        };
        let authority_partition =
            keys::observation_pk(&scope, signal, BucketHour::from_timestamp(accepted), 3);
        let authority_sort = order_sort_key(OrderTuple::new(accepted, signal, id, 1));
        let (partition, sort) = segment_key(
            &scope,
            workspace(),
            &normalized_query(),
            descriptor.access,
            signal,
            descriptor.bucket,
            descriptor.shard,
        )
        .expect("scope-time segment");
        let index_sk = order_sort_key(OrderTuple::new(time, signal, id, 1));
        let item = HashMap::from([
            (
                "pk".to_owned(),
                AttributeValue::S(authority_partition.clone()),
            ),
            ("sk".to_owned(), AttributeValue::S(authority_sort.clone())),
            (
                partition.attribute.clone(),
                AttributeValue::S(partition.value.clone()),
            ),
            (sort.attribute.clone(), AttributeValue::S(index_sk.clone())),
            (
                "observationId".to_owned(),
                AttributeValue::S(id.to_string()),
            ),
            ("revision".to_owned(), AttributeValue::N("1".to_owned())),
            ("time".to_owned(), AttributeValue::S(time.to_wire())),
            (
                "acceptedAt".to_owned(),
                AttributeValue::S(accepted.to_wire()),
            ),
        ]);
        let compact = item_resume_key(&item, descriptor, &scope).expect("compact key");
        let rebuilt = resume_key_map(
            &scope,
            workspace(),
            &normalized_query(),
            descriptor,
            &compact,
        )
        .expect("resume key rebuilds");
        assert_eq!(rebuilt["pk"].as_s().expect("pk"), &authority_partition);
        assert_eq!(rebuilt["sk"].as_s().expect("sk"), &authority_sort);
        assert_eq!(
            rebuilt[partition.attribute.as_str()]
                .as_s()
                .expect("index pk"),
            &partition.value
        );
        assert_eq!(
            rebuilt[sort.attribute.as_str()].as_s().expect("index sk"),
            &index_sk
        );
    }

    #[test]
    fn telemetry_segments_are_grouped_bucket_first_before_any_later_bucket() {
        let scope = ScopeKey::Session(session());
        let mut query = normalized_query();
        query.axis = ScopeAxis::Scope;
        query.signals = SignalSet::all();
        query.trace_id = None;
        query.metric_name = None;
        query.time_gte = Timestamp::parse("2026-08-01T09:00:00.000Z").expect("time");
        query.time_lt = Timestamp::parse("2026-08-01T11:00:00.000Z").expect("time");
        let planned = plan(&query, Budget::default()).expect("plans");
        let groups = ObservationReader::segment_groups(&scope, &query, &planned);

        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0.as_str(), "2026-08-01T09");
        assert_eq!(groups[1].0.as_str(), "2026-08-01T10");
        assert_eq!(
            groups[0].1.len(),
            17,
            "one event plus four shards x four signals"
        );
        assert_eq!(
            groups[0]
                .1
                .iter()
                .filter(|segment| segment.signal == Signal::Events)
                .count(),
            1
        );
        assert_eq!(groups[0].1.len(), MAX_PARALLEL_SEGMENT_READS + 1);
    }

    #[test]
    fn sixteen_hydrated_refills_reserve_the_whole_wave_below_the_byte_ceiling() {
        let ceiling = 32 * 1024 * 1024;
        let hydration = vec![true; MAX_PARALLEL_SEGMENT_READS];
        let reservations = reserve_refill_wave(&hydration, 1_000, ceiling).expect("the wave fits");

        assert_eq!(reservations.len(), MAX_PARALLEL_SEGMENT_READS);
        assert_eq!(reservations[0].max_items, 2);
        assert!(
            reservations
                .iter()
                .map(|reservation| reservation.max_bytes)
                .sum::<u64>()
                <= ceiling
        );
        assert!(
            hydration
                .iter()
                .map(|hydrate| provider_read_reservation(3, *hydrate))
                .sum::<u64>()
                > ceiling,
            "the next larger common provider page must be refused"
        );
    }

    #[test]
    fn a_hydrated_refill_starts_at_the_exact_reservation_boundary() {
        let minimum = provider_read_reservation(1, true);
        assert_eq!(
            minimum,
            2 * u64::try_from(DYNAMODB_ITEM_CEILING).expect("the provider limit fits u64")
        );
        assert!(reserve_refill_wave(&[true], 1, minimum - 1).is_none());
        assert_eq!(
            reserve_refill_wave(&[true], 1, minimum).expect("the exact boundary is admitted")[0]
                .max_bytes,
            minimum
        );
    }

    #[tokio::test]
    async fn an_unreservable_required_head_fails_before_any_provider_read() {
        let (reader, _replay) = replaying_reader_responses(&[]);
        let mut query = normalized_query();
        query.signals = SignalSet::from_signal(Signal::Spans);
        query.metric_name = None;
        let minimum = provider_read_reservation(1, true);
        let planned = plan(
            &query,
            Budget {
                max_bytes_read: minimum - 1,
                ..Budget::default()
            },
        )
        .expect("the query plans");

        let error = reader
            .read_page(
                &ScopeKey::Workspace(workspace()),
                workspace(),
                &query,
                &planned,
                aex_observation_query::coverage::Snapshot::at(query.time_lt),
                None,
                None,
                false,
            )
            .await
            .expect_err("one required hydrated head cannot be reserved");

        assert!(matches!(
            error,
            ReadError::BudgetExhausted {
                dimension: "bytes",
                scanned: 0
            }
        ));
    }

    #[tokio::test]
    async fn parallel_refills_are_capped_and_every_failure_sibling_settles() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let completed = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let refill_active = Arc::clone(&active);
        let refill_maximum = Arc::clone(&maximum);
        let refill_completed = Arc::clone(&completed);
        let refill_release = Arc::clone(&release);
        let refills = (0..=MAX_PARALLEL_SEGMENT_READS).map(move |index| {
            let active = Arc::clone(&refill_active);
            let maximum = Arc::clone(&refill_maximum);
            let completed = Arc::clone(&refill_completed);
            let release = Arc::clone(&refill_release);
            async move {
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                maximum.fetch_max(current, Ordering::SeqCst);
                let permit = release.acquire().await.expect("the fixture stays open");
                permit.forget();
                active.fetch_sub(1, Ordering::SeqCst);
                completed.fetch_add(1, Ordering::SeqCst);
                if index == 0 { Err(index) } else { Ok(index) }
            }
        });
        let task = tokio::spawn(async move { settle_refills(refills).await });

        for _ in 0..1_000 {
            if active.load(Ordering::SeqCst) == MAX_PARALLEL_SEGMENT_READS {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(
            active.load(Ordering::SeqCst),
            MAX_PARALLEL_SEGMENT_READS,
            "one more provider future must wait outside the concurrency window"
        );
        release.add_permits(MAX_PARALLEL_SEGMENT_READS + 1);
        let results = task.await.expect("the refill set joins");

        assert_eq!(maximum.load(Ordering::SeqCst), MAX_PARALLEL_SEGMENT_READS);
        assert_eq!(completed.load(Ordering::SeqCst), results.len());
        assert_eq!(results.len(), MAX_PARALLEL_SEGMENT_READS + 1);
        assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
    }

    #[test]
    fn mixed_trace_and_event_pagination_is_bucket_major_in_both_directions() {
        let scope = ScopeKey::Session(session());
        for (direction, expected) in [
            (
                Direction::Ascending,
                ["2026-08-01T23", "2026-08-02T00", "2026-08-02T01"],
            ),
            (
                Direction::Descending,
                ["2026-08-02T01", "2026-08-02T00", "2026-08-01T23"],
            ),
        ] {
            let mut query = normalized_query();
            query.axis = ScopeAxis::Scope;
            query.signals = SignalSet::from_signal(Signal::Events).with(Signal::Spans);
            query.metric_name = None;
            query.time_gte = Timestamp::parse("2026-08-01T23:30:00.000Z").expect("time");
            query.time_lt = Timestamp::parse("2026-08-02T01:30:00.000Z").expect("time");
            query.direction = direction;
            let planned = plan(&query, Budget::default()).expect("plans");
            let groups = ObservationReader::segment_groups(&scope, &query, &planned);

            assert_eq!(
                groups
                    .iter()
                    .map(|(bucket, _)| bucket.as_str())
                    .collect::<Vec<_>>(),
                expected
            );
            for (bucket, descriptors) in &groups {
                assert_eq!(descriptors.len(), 2, "one event and one trace head");
                assert!(
                    descriptors
                        .iter()
                        .any(|segment| segment.access == Access::SessionAuthority)
                );
                let trace = descriptors
                    .iter()
                    .find(|segment| segment.access == Access::Trace)
                    .expect("trace segment shares every event hour");
                let (_, range) = segment_key(
                    &scope,
                    workspace(),
                    &query,
                    trace.access,
                    trace.signal,
                    *bucket,
                    trace.shard,
                )
                .expect("trace key");
                let (lo, hi) = expected_mixed_hour_range(*bucket);
                assert_eq!((range.lo.as_str(), range.hi.as_str()), (lo, hi));
            }
        }
    }

    #[test]
    fn mixed_metric_and_log_pagination_reuses_days_through_disjoint_hours() {
        let scope = ScopeKey::Workspace(workspace());
        for (direction, expected) in [
            (
                Direction::Ascending,
                ["2026-08-01T23", "2026-08-02T00", "2026-08-02T01"],
            ),
            (
                Direction::Descending,
                ["2026-08-02T01", "2026-08-02T00", "2026-08-01T23"],
            ),
        ] {
            let mut query = normalized_query();
            query.axis = ScopeAxis::Workspace;
            query.signals = SignalSet::from_signal(Signal::Logs).with(Signal::Metrics);
            query.trace_id = None;
            query.time_gte = Timestamp::parse("2026-08-01T23:30:00.000Z").expect("time");
            query.time_lt = Timestamp::parse("2026-08-02T01:30:00.000Z").expect("time");
            query.direction = direction;
            let planned = plan(&query, Budget::default()).expect("plans");
            let groups = ObservationReader::segment_groups(&scope, &query, &planned);

            assert_eq!(
                groups
                    .iter()
                    .map(|(bucket, _)| bucket.as_str())
                    .collect::<Vec<_>>(),
                expected
            );
            let mut metric_partitions = HashMap::new();
            for (bucket, descriptors) in &groups {
                assert_eq!(descriptors.len(), 5, "four log shards and one metric head");
                let metric = descriptors
                    .iter()
                    .find(|segment| segment.access == Access::Metric)
                    .expect("metric segment shares every log hour");
                let (partition, range) = segment_key(
                    &scope,
                    workspace(),
                    &query,
                    metric.access,
                    metric.signal,
                    *bucket,
                    metric.shard,
                )
                .expect("metric key");
                metric_partitions.insert(bucket.as_str().to_owned(), partition.value);
                let (lo, hi) = expected_mixed_hour_range(*bucket);
                assert_eq!((range.lo.as_str(), range.hi.as_str()), (lo, hi));
            }
            assert_ne!(
                metric_partitions["2026-08-01T23"],
                metric_partitions["2026-08-02T00"]
            );
            assert_eq!(
                metric_partitions["2026-08-02T00"],
                metric_partitions["2026-08-02T01"]
            );
        }
    }

    fn expected_mixed_hour_range(bucket: BucketHour) -> (&'static str, &'static str) {
        match bucket.as_str() {
            "2026-08-01T23" => ("2026-08-01T23:30:00.000Z", "2026-08-02T00:00:00.000Z"),
            "2026-08-02T00" => ("2026-08-02T00:00:00.000Z", "2026-08-02T01:00:00.000Z"),
            "2026-08-02T01" => ("2026-08-02T01:00:00.000Z", "2026-08-02T01:30:00.000Z"),
            other => panic!("unexpected bucket {other}"),
        }
    }

    #[test]
    fn a_native_event_decodes_to_the_shared_observation_shape() {
        let occurred = Timestamp::from_unix_millis(1_754_051_696_789).expect("bounded");
        let item = HashMap::from([
            (
                "itemType".to_owned(),
                AttributeValue::S("session_event".to_owned()),
            ),
            (
                "workspaceId".to_owned(),
                AttributeValue::S(workspace().to_string()),
            ),
            (
                "sessionId".to_owned(),
                AttributeValue::S(session().to_string()),
            ),
            ("eventSeq".to_owned(), AttributeValue::N("7".to_owned())),
            (
                "eventId".to_owned(),
                AttributeValue::S(
                    ObservationId::from_uuid7(aex_wire::Uuid7::compose(1, [5; 10])).to_string(),
                ),
            ),
            (
                "type".to_owned(),
                AttributeValue::S("run.admitted".to_owned()),
            ),
            (
                "bodyInline".to_owned(),
                AttributeValue::B(Blob::new(br#"{"ok":true}"#)),
            ),
            (
                "occurredAt".to_owned(),
                AttributeValue::S(occurred.to_wire()),
            ),
            (
                "outboxState".to_owned(),
                AttributeValue::S("pending".to_owned()),
            ),
        ]);
        let (_, observation, _) = ObservationReader::decode_event(
            &item,
            &ScopeKey::Session(session()),
            workspace(),
            OrderBy::Accepted,
        )
        .expect("valid native event")
        .expect("event is observable");
        assert_eq!(observation.signal, ObservationSignal::Events);
        assert_eq!(observation.session_id, Some(session()));
        assert_eq!(observation.sequence.get(), 7);
        assert_eq!(observation.observed_at, occurred);
    }

    #[test]
    fn bucket_walks_are_half_open_and_reverse_as_a_whole() {
        let mut query = normalized_query();
        query.time_gte = Timestamp::parse("2026-08-01T09:00:00.000Z").expect("bounded");
        query.time_lt = Timestamp::parse("2026-08-01T10:00:00.000Z").expect("bounded");
        query.direction = Direction::Ascending;
        let scope = ScopeKey::Workspace(workspace());

        let ascending = ObservationReader::ordered_buckets(&scope, &query, Access::ScopeTime);
        assert_eq!(
            ascending.iter().map(BucketHour::as_str).collect::<Vec<_>>(),
            ["2026-08-01T09"]
        );

        query.time_lt = Timestamp::parse("2026-08-01T10:00:00.001Z").expect("bounded");
        query.direction = Direction::Descending;
        let descending = ObservationReader::ordered_buckets(&scope, &query, Access::ScopeTime);
        assert_eq!(
            descending
                .iter()
                .map(BucketHour::as_str)
                .collect::<Vec<_>>(),
            ["2026-08-01T10", "2026-08-01T09"]
        );
    }

    #[test]
    fn a_budget_failure_names_its_limiting_dimension() {
        let error = ReadError::BudgetExhausted {
            dimension: "scanned_items",
            scanned: 50_000,
        };
        assert!(error.to_string().contains("scanned_items"), "{error}");
        assert!(error.to_string().contains("50000"), "{error}");
    }

    #[test]
    fn the_reader_and_the_writer_agree_on_the_shard_count() {
        // A reader that merged a different number of partitions than the writer
        // produced would silently lose observations.
        assert_eq!(BUCKET_SHARDS, 4);
    }
}
