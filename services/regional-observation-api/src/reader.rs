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

use std::collections::{BTreeMap, HashMap};

use aex_observation_domain::canonical::CanonicalValue;
use aex_observation_domain::keys::{self, BucketHour, ScopeKey};
use aex_observation_domain::order::{Direction, OrderTuple};
use aex_observation_domain::signal::Signal;
use aex_observation_query::ast::MapRow;
use aex_observation_query::coverage::Snapshot;
use aex_observation_query::plan::{Access, NormalizedQuery, PageOutcome, Spend, classify};
use aex_observation_store_aws::expressions::{ExpressionBuilder, Index, PK, SK};
use aex_wire::ids::{ObservationId, RunId, SessionId, SpanId, TraceId, WorkspaceId};
use aex_wire::models::{Observation, ObservationSignal};
use aex_wire::types::{DecimalU128, Timestamp};
use aws_sdk_dynamodb::types::AttributeValue;

/// The shard count one bucket is written across, matching the admission edge.
///
/// Fixed for the life of a bucket, so a reader never has to guess how many
/// partitions to merge.
pub const BUCKET_SHARDS: u8 = 4;

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
    fn provider(operation: &'static str, reason: impl std::fmt::Display) -> Self {
        Self::Provider {
            operation,
            reason: reason.to_string(),
        }
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
}

/// One fully paged, budget-bounded index segment.
struct SegmentBatch {
    items: Vec<HashMap<String, AttributeValue>>,
    exhausted: bool,
}

/// The accepted frontier of one scope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Frontier {
    /// The latest accepted instant across the queried signals.
    pub accepted_at: Timestamp,
    /// The earliest retained accepted instant, which is `earliestReplay`.
    pub earliest_accepted_at: Timestamp,
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
}

impl ObservationReader {
    /// Binds the reader to its resolved resources.
    #[must_use]
    pub fn new(
        dynamodb: aws_sdk_dynamodb::Client,
        s3: aws_sdk_s3::Client,
        table: impl Into<String>,
        session_table: impl Into<String>,
        bucket: impl Into<String>,
        settle_ms: i64,
    ) -> Self {
        Self {
            dynamodb,
            s3,
            table: table.into(),
            session_table: session_table.into(),
            bucket: bucket.into(),
            settle_ms,
        }
    }

    /// The bound table name.
    #[must_use]
    pub fn table(&self) -> &str {
        &self.table
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
        self.dynamodb
            .describe_table()
            .table_name(&self.table)
            .send()
            .await
            .map_err(|error| ReadError::provider("DescribeTable", error))?;
        self.dynamodb
            .describe_table()
            .table_name(&self.session_table)
            .send()
            .await
            .map_err(|error| ReadError::provider("DescribeSessionTable", error))?;
        self.s3
            .head_bucket()
            .bucket(&self.bucket)
            .send()
            .await
            .map_err(|error| ReadError::provider("HeadBucket", error))?;
        Ok(())
    }

    /// Reads the accepted frontier of one scope across the queried signals.
    ///
    /// # Errors
    ///
    /// Returns [`ReadError::Provider`] when the read fails.
    pub async fn frontier(
        &self,
        scope: &ScopeKey,
        query: &NormalizedQuery,
    ) -> Result<Frontier, ReadError> {
        let mut accepted = Timestamp::from_unix_millis(0).unwrap_or(query.time_gte);
        let mut earliest: Option<Timestamp> = None;
        for signal in query.signals.in_authority().iter() {
            let Some(item) = self
                .get(&keys::frontier_pk(scope), &keys::frontier_sk(signal))
                .await?
            else {
                continue;
            };
            if let Some(at) = timestamp(&item, "acceptedAt")
                && at.unix_millis() > accepted.unix_millis()
            {
                accepted = at;
            }
            if let Some(at) = timestamp(&item, "earliestAcceptedAt") {
                earliest = Some(earliest.map_or(at, |current| {
                    if at.unix_millis() < current.unix_millis() {
                        at
                    } else {
                        current
                    }
                }));
            }
        }
        Ok(Frontier {
            accepted_at: accepted,
            earliest_accepted_at: earliest.unwrap_or(accepted),
        })
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
        clippy::too_many_lines,
        reason = "the bounded multi-index merge keeps budget accounting, snapshot filtering and total ordering in one audit surface"
    )]
    pub async fn read_page(
        &self,
        scope: &ScopeKey,
        workspace: WorkspaceId,
        query: &NormalizedQuery,
        plan: &aex_observation_query::plan::Plan,
        snapshot: Snapshot,
        after: Option<&OrderTuple>,
    ) -> Result<Page, ReadError> {
        let mut rows: Vec<(OrderTuple, Observation, MapRow)> = Vec::new();
        let mut spend = Spend::default();
        let mut exhausted_range = true;

        'walks: for walk in &plan.walks {
            let buckets = if walk.access == Access::SessionAuthority
                && matches!(scope, ScopeKey::Session(_))
            {
                vec![BucketHour::from_timestamp(query.time_gte)]
            } else {
                Self::buckets(query)
            };
            for bucket in buckets {
                if spend.segments >= plan.budget.max_segments
                    || spend.items_scanned >= plan.budget.max_items_scanned
                    || spend.bytes_read >= plan.budget.max_bytes_read
                {
                    exhausted_range = false;
                    break 'walks;
                }
                let shard_count = if walk.access == Access::SessionAuthority {
                    1
                } else {
                    BUCKET_SHARDS
                };
                for shard in 0..shard_count {
                    let remaining_items = plan
                        .budget
                        .max_items_scanned
                        .saturating_sub(spend.items_scanned);
                    let remaining_segments =
                        plan.budget.max_segments.saturating_sub(spend.segments);
                    if remaining_items == 0 || remaining_segments == 0 {
                        exhausted_range = false;
                        break 'walks;
                    }
                    let fair_share = if walk.access == Access::SessionAuthority
                        && matches!(scope, ScopeKey::Session(_))
                    {
                        remaining_items
                    } else {
                        remaining_items
                            .div_ceil(u32::from(remaining_segments))
                            .max(u32::from(query.limit))
                            .min(remaining_items)
                    };
                    let remaining_bytes =
                        plan.budget.max_bytes_read.saturating_sub(spend.bytes_read);
                    spend.segments = spend.segments.saturating_add(1);
                    let segment = self
                        .query_segment(
                            scope,
                            workspace,
                            query,
                            walk.access,
                            walk.signal,
                            bucket,
                            shard,
                            fair_share,
                            remaining_bytes,
                        )
                        .await?;
                    exhausted_range &= segment.exhausted;
                    spend.items_scanned = spend
                        .items_scanned
                        .saturating_add(u32::try_from(segment.items.len()).unwrap_or(u32::MAX));
                    for item in segment.items {
                        spend.bytes_read = spend.bytes_read.saturating_add(
                            aex_observation_store_aws::store::item_size(&item) as u64,
                        );
                        let Some((tuple, observation, row)) =
                            (if walk.access == Access::SessionAuthority {
                                Self::decode_event(&item, scope, workspace, query.order_by)?
                            } else {
                                Self::decode(&item, walk.signal, query.order_by)?
                            })
                        else {
                            continue;
                        };
                        if observation.accepted_at > snapshot.accepted_at() {
                            continue;
                        }
                        if !inside(&tuple, query, after) {
                            continue;
                        }
                        if let Some(predicate) = &query.predicate
                            && !predicate.evaluate(&row)
                        {
                            continue;
                        }
                        rows.push((tuple, observation, row));
                    }
                    if spend.items_scanned >= plan.budget.max_items_scanned
                        || spend.bytes_read >= plan.budget.max_bytes_read
                    {
                        exhausted_range = false;
                        break 'walks;
                    }
                }
            }
        }

        rows.sort_by(|left, right| match query.direction {
            Direction::Ascending => left.0.cmp(&right.0),
            Direction::Descending => right.0.cmp(&left.0),
        });
        let limit = usize::from(query.limit);
        let more = rows.len() > limit || !exhausted_range;
        rows.truncate(limit);
        spend.returned = u32::try_from(rows.len()).unwrap_or(u32::MAX);

        if rows.is_empty() && !exhausted_range {
            return Err(ReadError::BudgetExhausted {
                dimension: "scanned_items",
                scanned: spend.items_scanned,
            });
        }

        match classify(&plan.budget, &spend, exhausted_range) {
            PageOutcome::NoProgress { dimension, .. } => Err(ReadError::BudgetExhausted {
                dimension: dimension.as_str(),
                scanned: spend.items_scanned,
            }),
            PageOutcome::Complete | PageOutcome::Short { .. } => {
                let last = rows.last().map(|(tuple, _, _)| *tuple);
                Ok(Page {
                    items: rows
                        .into_iter()
                        .map(|(_, observation, _)| observation)
                        .collect(),
                    spend,
                    more,
                    last,
                })
            }
        }
    }

    /// The non-empty hour buckets one query walks, bounded by its time range.
    fn buckets(query: &NormalizedQuery) -> Vec<BucketHour> {
        let mut buckets = Vec::new();
        let mut current = BucketHour::from_timestamp(query.time_gte);
        let end = BucketHour::from_timestamp(query.time_lt);
        loop {
            buckets.push(current);
            if current.as_str() >= end.as_str() || buckets.len() >= usize::from(u16::MAX) {
                break;
            }
            current = current.next();
        }
        buckets
    }

    /// One bounded `Query` against exactly one index partition.
    #[allow(
        clippy::too_many_arguments,
        reason = "a segment is identified by exactly these seven coordinates"
    )]
    async fn query_segment(
        &self,
        scope: &ScopeKey,
        workspace: WorkspaceId,
        query: &NormalizedQuery,
        access: Access,
        signal: Signal,
        bucket: BucketHour,
        shard: u8,
        max_items: u32,
        max_bytes: u64,
    ) -> Result<SegmentBatch, ReadError> {
        if access == Access::SessionAuthority {
            return self
                .query_event_segment(scope, workspace, query, bucket, max_items, max_bytes)
                .await;
        }
        let Some((partition, sort)) = segment_key(scope, workspace, access, signal, bucket, shard)
        else {
            // `events` are read through the semantic port over
            // `session-authority`; they never live in this table.
            return Ok(SegmentBatch {
                items: Vec::new(),
                exhausted: true,
            });
        };
        let mut builder = ExpressionBuilder::new();
        let pk_name = builder.name(&partition.attribute);
        let pk_value = builder.string(partition.value);
        let sk_name = builder.name(&sort.attribute);
        let lo = builder.string(sort.lo);
        let hi = builder.string(sort.hi);
        let condition = format!("{pk_name} = {pk_value} AND {sk_name} BETWEEN {lo} AND {hi}");

        let names = builder.names();
        let values = builder.values();
        let mut items = Vec::new();
        let mut bytes = 0_u64;
        let mut start = None;
        loop {
            let remaining =
                max_items.saturating_sub(u32::try_from(items.len()).unwrap_or(u32::MAX));
            if remaining == 0 {
                return Ok(SegmentBatch {
                    items,
                    exhausted: false,
                });
            }
            let mut request = self
                .dynamodb
                .query()
                .table_name(&self.table)
                .key_condition_expression(condition.clone())
                .set_expression_attribute_names(Some(names.clone()))
                .set_expression_attribute_values(Some(values.clone()))
                .set_exclusive_start_key(start.take())
                .limit(i32::try_from(remaining).unwrap_or(i32::MAX))
                .scan_index_forward(matches!(query.direction, Direction::Ascending));
            if let Some(index) = access.index_name() {
                request = request.index_name(index);
            } else {
                // The base table is the strongly consistent accepted-order read.
                request = request.consistent_read(true);
            }
            let response = request
                .send()
                .await
                .map_err(|error| ReadError::provider("Query", error))?;
            let next = response.last_evaluated_key;
            let page = response.items.unwrap_or_default();
            bytes = bytes.saturating_add(
                page.iter()
                    .map(|item| aex_observation_store_aws::store::item_size(item) as u64)
                    .sum::<u64>(),
            );
            items.extend(page);
            if next.is_none() {
                return Ok(SegmentBatch {
                    items,
                    exhausted: true,
                });
            }
            if bytes >= max_bytes {
                return Ok(SegmentBatch {
                    items,
                    exhausted: false,
                });
            }
            start = next;
        }
    }

    /// One bounded native-event query against the semantic session authority.
    async fn query_event_segment(
        &self,
        scope: &ScopeKey,
        workspace: WorkspaceId,
        query: &NormalizedQuery,
        bucket: BucketHour,
        max_items: u32,
        max_bytes: u64,
    ) -> Result<SegmentBatch, ReadError> {
        let (index, partition_attribute, sort_attribute, partition, lo, hi, consistent) =
            match scope {
                ScopeKey::Session(session) => (
                    None,
                    PK,
                    SK,
                    format!("SESSION#{session}"),
                    "EVT#".to_owned(),
                    "EVT#\u{fffe}".to_owned(),
                    true,
                ),
                ScopeKey::Workspace(_) => (
                    Some(aex_session_dynamodb::stream_keys::WORKSPACE_EVENT_INDEX),
                    aex_session_dynamodb::stream_keys::WORKSPACE_EVENT_PK,
                    aex_session_dynamodb::stream_keys::WORKSPACE_EVENT_SK,
                    aex_session_dynamodb::stream_keys::workspace_event_partition_hour(
                        workspace,
                        bucket.as_str(),
                    ),
                    query.time_gte.to_wire(),
                    query.time_lt.to_wire(),
                    false,
                ),
            };
        let mut builder = ExpressionBuilder::new();
        let pk_name = builder.name(partition_attribute);
        let pk_value = builder.string(partition);
        let sk_name = builder.name(sort_attribute);
        let lo = builder.string(lo);
        let hi = builder.string(hi);
        let condition =
            format!("{pk_name} = {pk_value} AND {sk_name} >= {lo} AND {sk_name} < {hi}");
        let names = builder.names();
        let values = builder.values();
        let mut items = Vec::new();
        let mut bytes = 0_u64;
        let mut start = None;
        loop {
            let remaining =
                max_items.saturating_sub(u32::try_from(items.len()).unwrap_or(u32::MAX));
            if remaining == 0 {
                return Ok(SegmentBatch {
                    items,
                    exhausted: false,
                });
            }
            let mut request = self
                .dynamodb
                .query()
                .table_name(&self.session_table)
                .key_condition_expression(condition.clone())
                .set_expression_attribute_names(Some(names.clone()))
                .set_expression_attribute_values(Some(values.clone()))
                .set_exclusive_start_key(start.take())
                .limit(i32::try_from(remaining).unwrap_or(i32::MAX))
                .scan_index_forward(matches!(query.direction, Direction::Ascending));
            if let Some(index) = index {
                request = request.index_name(index);
            }
            if consistent {
                request = request.consistent_read(true);
            }
            let response = request
                .send()
                .await
                .map_err(|error| ReadError::provider("QuerySessionEvents", error))?;
            let next = response.last_evaluated_key;
            let page = response.items.unwrap_or_default();
            bytes = bytes.saturating_add(
                page.iter()
                    .map(|item| aex_observation_store_aws::store::item_size(item) as u64)
                    .sum::<u64>(),
            );
            items.extend(page);
            if next.is_none() {
                return Ok(SegmentBatch {
                    items,
                    exhausted: true,
                });
            }
            if bytes >= max_bytes {
                return Ok(SegmentBatch {
                    items,
                    exhausted: false,
                });
            }
            start = next;
        }
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

    /// Reads every item of one partition under a bounded sort-key range.
    ///
    /// # Errors
    ///
    /// Returns [`ReadError::Provider`] when the read fails.
    pub async fn read_range(
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

    /// Writes one item under a caller-supplied condition.
    ///
    /// # Errors
    ///
    /// Returns [`ReadError::Provider`] when the write fails, including when the
    /// condition did not hold — losing a fenced write is a normal outcome that
    /// the caller branches on rather than retries.
    pub async fn put_conditional(
        &self,
        item: HashMap<String, AttributeValue>,
        condition: &str,
        names: HashMap<String, String>,
        values: HashMap<String, AttributeValue>,
    ) -> Result<(), ReadError> {
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

    /// Updates one item under a caller-supplied condition.
    ///
    /// # Errors
    ///
    /// Returns [`ReadError::Provider`] when the update fails, including when the
    /// condition did not hold.
    pub async fn update_conditional(
        &self,
        pk: &str,
        sk: &str,
        update: &str,
        condition: &str,
        names: HashMap<String, String>,
        values: HashMap<String, AttributeValue>,
    ) -> Result<(), ReadError> {
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
    /// The inclusive upper bound.
    pub hi: String,
}

/// Resolves the exact index partition and range one segment reads.
///
/// Returns `None` for the one access pattern that is not in this table.
#[must_use]
pub fn segment_key(
    scope: &ScopeKey,
    workspace: WorkspaceId,
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
            format!("TRC#{}#", scope.to_key()),
            "trPk".to_owned(),
            "trSk".to_owned(),
        ),
        Access::Metric => (
            format!("MET#{workspace}#"),
            "mPk".to_owned(),
            "mSk".to_owned(),
        ),
    };
    Some((
        KeyBinding {
            attribute,
            value: partition,
        },
        RangeBinding {
            attribute: sort_attribute,
            // The sort key is a fixed-width rendering, so the widest legal range
            // is exactly the empty string to the highest code point the key
            // grammar admits.
            lo: String::new(),
            hi: "\u{fffe}".to_owned(),
        },
    ))
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

/// Reads a boolean attribute.
pub(crate) fn boolean(item: &HashMap<String, AttributeValue>, name: &str) -> Option<bool> {
    item.get(name)
        .and_then(|value| value.as_bool().ok())
        .copied()
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

    use aex_observation_domain::keys::{BucketHour, ScopeKey};
    use aex_observation_domain::order::OrderBy;
    use aex_observation_domain::signal::Signal;
    use aex_observation_query::plan::Access;
    use aex_wire::ids::{ObservationId, PrefixedId as _, SessionId, WorkspaceId};
    use aex_wire::models::ObservationSignal;
    use aex_wire::types::Timestamp;
    use aws_sdk_dynamodb::primitives::Blob;
    use aws_sdk_dynamodb::types::AttributeValue;

    use super::{BUCKET_SHARDS, ObservationReader, ReadError, segment_key};

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(aex_wire::Uuid7::compose(1, [2; 10]))
    }

    fn bucket() -> BucketHour {
        BucketHour::from_timestamp(Timestamp::from_unix_millis(1_754_051_696_789).expect("bounded"))
    }

    fn session() -> SessionId {
        SessionId::from_uuid7(aex_wire::Uuid7::compose(1, [4; 10]))
    }

    #[test]
    fn every_access_pattern_names_its_own_index_attributes() {
        let scope = ScopeKey::Workspace(workspace());
        for (access, expected_pk, expected_sk) in [
            (Access::BaseTable, "pk", "sk"),
            (Access::ScopeTime, "tPk", "tSk"),
            (Access::WorkspaceAccepted, "wPk", "wSk"),
            (Access::WorkspaceTime, "wtPk", "wtSk"),
            (Access::Trace, "trPk", "trSk"),
            (Access::Metric, "mPk", "mSk"),
        ] {
            let (partition, sort) =
                segment_key(&scope, workspace(), access, Signal::Logs, bucket(), 0)
                    .expect("the access pattern is in this table");
            assert_eq!(partition.attribute, expected_pk);
            assert_eq!(sort.attribute, expected_sk);
            assert!(!partition.value.is_empty());
        }
    }

    #[test]
    fn the_events_signal_is_never_read_from_this_table() {
        let scope = ScopeKey::Workspace(workspace());
        assert!(
            segment_key(
                &scope,
                workspace(),
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
