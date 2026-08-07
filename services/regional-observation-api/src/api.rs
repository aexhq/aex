//! The generated observation and telemetry-lifecycle server traits, served.
//!
//! Every route this deployable mounts is fully served here. There is no
//! `route_binding_unavailable`, no unavailable-port stub and no arm that
//! answers a `503` because a dependency was never wired — a deployable never
//! mounts a route it cannot serve, so mounting and serving are the same commit.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use aex_observation_domain::gap::{GapRecord, TimeWindow};
use aex_observation_domain::keys::{self, ScopeKey};
use aex_observation_domain::order::{OrderBy, OrderTuple};
use aex_observation_domain::signal::{Signal, SignalSet};
use aex_observation_query::ast::{CmpOp, FieldRef, Predicate};
use aex_observation_query::plan::plan;
use aex_regional_http::cursor::CursorKeyRing;
use aex_regional_http::stream::split_records;
use aex_wire::error::{ErrorCode, WireError, WireResult};
use aex_wire::ids::{
    ExportId, MeasurementId, PrefixedId as _, SessionId, TelemetryGapId, TraceId, Uuid7,
    WorkspaceId,
};
use aex_wire::models::{
    DownloadGrant, EmptyRequest, ExportCompleteness, ExportFormat, ExportStatus,
    MetricAggregationGroup, MetricAggregationPage, MetricAggregationRequest, Observation,
    ObservationFrame, ObservationFrameCursor, ObservationFrameGap, ObservationListenRequest,
    ObservationPage, ObservationQuery, ObservationSignal, ObservationStreamRequest, Operation,
    OperationKind, OperationStatus, RotateReason, TelemetryExport, TelemetryExportRequest,
    TelemetryGap, TelemetryGapPage, TelemetryGapQuery, TimeRange, TraceDetail, TraceSummary,
};
use aex_wire::server::{
    Accepted, Created, NdjsonStream, ObservationsApi, RequestContext, TelemetryLifecycleApi,
};
use aex_wire::types::{DecimalU128, Region, Timestamp};

use crate::counters::{ReadCounter, ReadCounters};
use crate::frontier::{Frontier, ScopeDeletionState};
use crate::gap_watch::{CycleTrigger, GapObservation, GapWatch};
use crate::ndjson::{self, FrameStream};
use crate::query;
use crate::reader::{ObservationReader, ReadError};
use aex_regional_http::context::RequestContext as EdgeContext;

/// How long a minted download grant lives.
///
/// OD-17 pins the grant and the signature to the same five minutes, so a leaked
/// signature can never outlive the grant that authorized it.
pub const GRANT_LIFETIME: Duration = Duration::from_mins(5);

/// How long one `listen` connection follows before it rotates.
pub const LISTEN_BUDGET: Duration = Duration::from_mins(15);

/// Fastest correctness fallback after an authoritative read made progress.
pub const LISTEN_POLL_MIN: Duration = Duration::from_millis(250);

/// Slowest correctness fallback when wake hints are absent or lost.
pub const LISTEN_POLL_MAX: Duration = Duration::from_secs(15);

/// Heartbeat/checkpoint interval required by the public stream contract.
pub const LISTEN_HEARTBEAT: Duration = Duration::from_secs(15);

/// Runtime policy for long-lived observation streams.
///
/// Both production composition roots must supply a regional authorization
/// revalidator. There is deliberately no unauthenticated default: a service
/// that cannot renew mutable authorization facts cannot be constructed.
#[derive(Clone)]
pub struct StreamPolicy {
    /// Maximum lifetime of one follow connection.
    pub connection_budget: Duration,
    /// Fastest delay between authoritative fallback reads.
    pub poll_min: Duration,
    /// Slowest delay between authoritative fallback reads.
    pub poll_max: Duration,
    /// Interval between resumability checkpoints while the authority is idle.
    pub heartbeat: Duration,
    /// Interval between mutable authorization projection checks.
    pub authorization_check: Duration,
    /// How long a socket trusts an unchanged gap-change hint before reading the
    /// gap ledger anyway.
    pub gap_recovery: Duration,
    /// Whole frames admitted to one connection's bounded channel.
    pub buffered_frames: usize,
    /// Longest wait to hand one whole frame to the transport.
    pub write_stall: Duration,
    /// Shared process drain signal.
    pub draining: Arc<AtomicBool>,
    /// Shared keyed wake hints, when the serving process has a wake reader.
    pub wakes: Option<crate::wake::WakeHub>,
    /// Fail-closed projection lease for a long-lived socket.
    pub revalidator: Arc<dyn StreamRevalidator>,
}

impl std::fmt::Debug for StreamPolicy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StreamPolicy")
            .field("connection_budget", &self.connection_budget)
            .field("poll_min", &self.poll_min)
            .field("poll_max", &self.poll_max)
            .field("heartbeat", &self.heartbeat)
            .field("authorization_check", &self.authorization_check)
            .field("gap_recovery", &self.gap_recovery)
            .field("buffered_frames", &self.buffered_frames)
            .field("write_stall", &self.write_stall)
            .field("wakes", &self.wakes.is_some())
            .field("revalidator", &"<regional authorization lease>")
            .finish_non_exhaustive()
    }
}

/// Mutable authorization facts a long-lived socket must periodically renew.
#[async_trait::async_trait]
pub trait StreamRevalidator: Send + Sync {
    /// Re-checks key/workspace/account epochs, pause and placement without
    /// retaining credential plaintext.
    async fn revalidate(
        &self,
        authorization: &aex_regional_http::context::RegionalAuthorization,
        scope: &ScopeKey,
    ) -> WireResult<()>;
}

impl StreamPolicy {
    /// Builds the default bounded stream policy around a mandatory regional
    /// authorization lease.
    #[must_use]
    pub fn new(revalidator: Arc<dyn StreamRevalidator>) -> Self {
        Self {
            connection_budget: LISTEN_BUDGET,
            poll_min: LISTEN_POLL_MIN,
            poll_max: LISTEN_POLL_MAX,
            heartbeat: LISTEN_HEARTBEAT,
            authorization_check: LISTEN_HEARTBEAT,
            gap_recovery: crate::gap_watch::GAP_RECOVERY_INTERVAL,
            buffered_frames: crate::ndjson::DEFAULT_CHANNEL_FRAMES,
            write_stall: Duration::from_secs(10),
            draining: Arc::new(AtomicBool::new(false)),
            wakes: None,
            revalidator,
        }
    }
}

/// The shared, resolved service every request borrows.
pub struct ObservationService {
    reader: ObservationReader,
    budget: aex_observation_query::plan::Budget,
    metric_scan: u64,
    cursor_ring: CursorKeyRing,
    region: Region,
    stream: StreamPolicy,
}

impl std::fmt::Debug for ObservationService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ObservationService")
            .field("table", &self.reader.table())
            .field("bucket", &self.reader.bucket())
            .field("budget", &self.budget)
            .field("metric_scan", &self.metric_scan)
            .field("region", &self.region)
            .field("cursor_ring", &"<key ring>")
            .field("stream", &self.stream)
            .finish()
    }
}

impl ObservationService {
    /// Composes the service over its resolved adapters.
    #[must_use]
    pub fn new(
        reader: ObservationReader,
        budget: aex_observation_query::plan::Budget,
        metric_scan: u64,
        cursor_ring: CursorKeyRing,
        region: Region,
        stream: StreamPolicy,
    ) -> Self {
        Self {
            reader,
            budget,
            metric_scan,
            cursor_ring,
            region,
            stream,
        }
    }

    /// Replaces the long-lived connection policy used by streaming routes.
    #[must_use]
    pub fn with_stream_policy(mut self, stream: StreamPolicy) -> Self {
        self.stream = stream;
        self
    }
}

/// One request's view of the service.
#[derive(Clone)]
pub struct ObservationRequest {
    service: Arc<ObservationService>,
    authorized: EdgeContext,
}

impl std::fmt::Debug for ObservationRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ObservationRequest")
            .field("workspace", &self.authorized.auth.workspace_id)
            .finish_non_exhaustive()
    }
}

impl ObservationRequest {
    /// Binds one request to the shared service.
    #[must_use]
    pub fn new(service: Arc<ObservationService>, authorized: EdgeContext) -> Self {
        Self {
            service,
            authorized,
        }
    }

    /// The workspace this request is authorized for.
    #[must_use]
    pub const fn workspace(&self) -> WorkspaceId {
        self.authorized.auth.workspace_id
    }

    /// The organization the authorized workspace belongs to.
    ///
    /// Recorded on every durable operation this surface admits, so an export
    /// row can be attributed without a second lookup.
    #[must_use]
    pub const fn organization(&self) -> aex_wire::ids::OrganizationId {
        self.authorized.auth.organization_id
    }

    /// The scope one route reads, which is a session when the route names one.
    fn scope(&self, session: Option<SessionId>) -> ScopeKey {
        session.map_or(ScopeKey::Workspace(self.workspace()), ScopeKey::Session)
    }

    /// Strongly reads and applies the deletion fence for a non-paged route.
    async fn deletion_epoch(&self, scope: &ScopeKey) -> WireResult<u64> {
        let (epoch, state) = self
            .service
            .reader
            .deletion_fence(scope, self.workspace())
            .await
            .map_err(|error| read_error(&error))?;
        ensure_queryable(scope, state)?;
        Ok(epoch)
    }

    /// Serves one bounded page.
    async fn page(
        &self,
        cx: &RequestContext,
        session: Option<SessionId>,
        route_signal: ObservationSignal,
        body: ObservationQuery,
    ) -> WireResult<ObservationPage> {
        let signals = query::signal_for(route_signal, body.signal)?;
        let scope = self.scope(session);
        let normalized = query::with_scope(
            query::normalize(&body, signals, self.service.budget.max_returned)?,
            &scope,
        );
        let now = now()?;
        let frontier = self
            .service
            .reader
            .frontier(&scope, self.workspace(), &normalized)
            .await
            .map_err(|error| read_error(&error))?;
        let request_binding = query::page_request_binding(
            cx,
            cx.route,
            self.service.region,
            &scope,
            self.workspace(),
            &normalized,
            frontier.deletion_epoch,
        );
        let (snapshot, resume) = if let Some(token) = body.cursor.as_ref() {
            let (snapshot, resume) =
                query::resume_page(&self.service.cursor_ring, token, &request_binding, now)?;
            (snapshot, Some(resume))
        } else {
            let snapshot = self
                .service
                .reader
                .pin(frontier, now)
                .ok_or_else(|| WireError::new(ErrorCode::ObservabilityUnavailable))?;
            (snapshot, None)
        };
        ensure_queryable(&scope, frontier.deletion_state)?;
        let binding = query::page_binding(&request_binding, snapshot)?;
        let plan = plan(
            &normalized,
            query::budget_for(self.service.budget, normalized.limit),
        )
        .map_err(|error| query::query_error(&error))?;
        let page = self
            .service
            .reader
            .read_page(
                &scope,
                self.workspace(),
                &normalized,
                &plan,
                snapshot,
                None,
                resume.as_ref(),
                false,
            )
            .await
            .map_err(|error| read_error(&error))?;
        let next_cursor = match (page.more, page.resume.as_ref()) {
            (true, Some(resume)) => Some(query::issue_page(
                self.service.cursor_ring.current(),
                &binding,
                resume,
                now,
            )?),
            _ => None,
        };
        let gaps = self
            .service
            .reader
            .gap_history(&scope, self.workspace(), snapshot)
            .await
            .map_err(|error| read_error(&error))?;
        let window = TimeWindow::new(normalized.time_gte, normalized.time_lt)
            .ok_or_else(|| query::invalid_query("observation timeRange must be non-empty"))?;
        Ok(ObservationPage {
            coverage: query::coverage(
                snapshot,
                frontier.accepted_at,
                frontier.earliest_accepted_at,
                normalized.signals,
                window,
                &gaps,
            )?,
            items: page.items,
            next_cursor,
        })
    }

    /// Serves one NDJSON replay or follow.
    async fn frames(
        &self,
        cx: &RequestContext,
        session: Option<SessionId>,
        route_signal: ObservationSignal,
        body: StreamBody,
    ) -> WireResult<NdjsonStream<FrameStream>> {
        let StreamBody {
            filter,
            signal,
            window,
            follow,
            resume,
        } = body;
        let signals = query::signal_for(route_signal, signal)?;
        let scope = self.scope(session);
        let synthetic = ObservationQuery {
            consistency: None,
            cursor: None,
            filter,
            limit: None,
            order: None,
            signal,
            time_range: window,
        };
        let mut normalized = query::with_scope(
            query::normalize(&synthetic, signals, self.service.budget.max_returned)?,
            &scope,
        );
        // Replay and tail share the authority's accepted-order rail. This makes
        // a wake followed by a strongly consistent read lossless even when a
        // producer reports an old observation timestamp after the socket opens.
        normalized.order_by = OrderBy::Accepted;
        let now = now()?;
        let frontier = self
            .service
            .reader
            .frontier(&scope, self.workspace(), &normalized)
            .await
            .map_err(|error| read_error(&error))?;
        let request_binding = query::stream_request_binding(
            cx,
            cx.route,
            self.service.region,
            &scope,
            self.workspace(),
            &normalized,
            frontier.deletion_epoch,
        );
        let (snapshot, after, segment_resume) = if let Some(cursor) = resume.as_ref() {
            let (snapshot, resumed) =
                query::resume_stream(&self.service.cursor_ring, cursor, &request_binding, now)?;
            match resumed {
                query::StreamResume::Tuple(after) => (snapshot, Some(after), None),
                query::StreamResume::Segments(resume) => (snapshot, None, Some(resume)),
            }
        } else {
            let snapshot = self
                .service
                .reader
                .pin(frontier, now)
                .ok_or_else(|| WireError::new(ErrorCode::ObservabilityUnavailable))?;
            (snapshot, None, None)
        };
        ensure_queryable(&scope, frontier.deletion_state)?;
        let binding = query::stream_binding(&request_binding, snapshot)?;
        let plan = plan(
            &normalized,
            query::budget_for(self.service.budget, normalized.limit),
        )
        .map_err(|error| query::query_error(&error))?;

        let (sender, stream) = ndjson::channel(
            self.service.stream.buffered_frames,
            self.service.stream.write_stall,
            Arc::clone(self.service.reader.counters()),
        );
        let service = Arc::clone(&self.service);
        let wake = service
            .stream
            .wakes
            .as_ref()
            .map(|hub| hub.subscribe(scope));
        let workspace = self.workspace();
        let authorization = self.authorized.auth.clone();
        tokio::spawn(produce(Produce {
            sender,
            service,
            workspace,
            scope,
            normalized,
            plan,
            binding,
            snapshot,
            follow,
            after,
            resume: segment_resume,
            frontier,
            request_id: cx.request_id.clone(),
            wake,
            authorization,
            seen_gaps: std::collections::BTreeMap::new(),
        }));
        Ok(NdjsonStream(stream))
    }

    /// Serves one bounded metric aggregation.
    async fn aggregate(
        &self,
        cx: &RequestContext,
        session: Option<SessionId>,
        body: MetricAggregationRequest,
    ) -> WireResult<MetricAggregationPage> {
        if body.metric.trim().is_empty() {
            return Err(query::invalid_query(
                "an exact metric name is required; `gsi_metric` is a point-partition read",
            ));
        }
        let scope = self.scope(session);
        let synthetic = ObservationQuery {
            consistency: None,
            cursor: None,
            filter: body.filter.clone(),
            limit: body.limit,
            order: None,
            signal: ObservationSignal::Metrics,
            time_range: body.time_range,
        };
        let signals = query::signal_for(ObservationSignal::Metrics, ObservationSignal::Metrics)?;
        let mut normalized = query::with_scope(
            query::normalize(&synthetic, signals, self.service.budget.max_returned)?,
            &scope,
        );
        bind_metric_selection(&mut normalized, &scope, &body.metric);
        let now = now()?;
        let frontier = self
            .service
            .reader
            .frontier(&scope, self.workspace(), &normalized)
            .await
            .map_err(|error| read_error(&error))?;
        ensure_queryable(&scope, frontier.deletion_state)?;
        let snapshot = self
            .service
            .reader
            .pin(frontier, now)
            .ok_or_else(|| WireError::new(ErrorCode::ObservabilityUnavailable))?;
        let plan = plan(
            &normalized,
            query::budget_for(self.service.budget, normalized.limit),
        )
        .map_err(|error| query::query_error(&error))?;
        let page = self
            .service
            .reader
            .read_page(
                &scope,
                self.workspace(),
                &normalized,
                &plan,
                snapshot,
                None,
                None,
                false,
            )
            .await
            .map_err(|error| read_error(&error))?;
        // The aggregation is a single bounded streaming pass; the scan budget is
        // the one place removing the column store genuinely costs capability, so
        // it is measured rather than hidden.
        let scanned = u64::from(page.spend.items_scanned);
        if scanned > self.service.metric_scan {
            return Err(query::budget_exhausted(
                "metric_aggregate_scan",
                page.spend.items_scanned,
                u32::try_from(self.service.metric_scan).unwrap_or(u32::MAX),
            ));
        }
        let group = MetricAggregationGroup {
            key: std::collections::BTreeMap::new(),
            sample_count: DecimalU128::new(page.items.len() as u128),
            value: f64::from(u32::try_from(page.items.len()).unwrap_or(u32::MAX)),
        };
        let _ = cx;
        let gaps = self
            .service
            .reader
            .gap_history(&scope, self.workspace(), snapshot)
            .await
            .map_err(|error| read_error(&error))?;
        let window = TimeWindow::new(normalized.time_gte, normalized.time_lt)
            .ok_or_else(|| query::invalid_query("metric timeRange must be non-empty"))?;
        Ok(MetricAggregationPage {
            coverage: query::coverage(
                snapshot,
                frontier.accepted_at,
                frontier.earliest_accepted_at,
                normalized.signals,
                window,
                &gaps,
            )?,
            items: if page.items.is_empty() {
                Vec::new()
            } else {
                vec![group]
            },
            next_cursor: None,
        })
    }

    /// Reads one assembled trace.
    #[allow(
        clippy::too_many_lines,
        reason = "assembling a trace keeps its bounded hydration and wire projection in one request path"
    )]
    async fn trace(&self, session: SessionId, trace_id: TraceId) -> WireResult<TraceDetail> {
        let scope = ScopeKey::Session(session);
        self.deletion_epoch(&scope).await?;
        let items = self
            .service
            .reader
            .read_hydrated_range(
                aex_observation_store_dynamodb::expressions::Index::Trace,
                &crate::reader::KeyBinding {
                    attribute: "trPk".to_owned(),
                    value: format!("TRC#{}#{trace_id}", scope.to_key()),
                },
                &crate::reader::RangeBinding {
                    attribute: "trSk".to_owned(),
                    lo: String::new(),
                    hi: "\u{fffe}".to_owned(),
                },
                self.service.budget.max_returned,
            )
            .await
            .map_err(|error| read_error(&error))?;
        if items.is_empty() {
            return Err(WireError::new(ErrorCode::NotFound));
        }
        let mut spans: Vec<Observation> = Vec::new();
        let mut started =
            Timestamp::from_unix_millis(0).unwrap_or_else(|_| unreachable_timestamp());
        let mut ended = started;
        let mut root = String::new();
        for item in &items {
            let Some(signal) = crate::reader::stored_signal(item) else {
                continue;
            };
            let Some(time) = crate::reader::timestamp(item, "time") else {
                continue;
            };
            if spans.is_empty() || time.unix_millis() < started.unix_millis() {
                started = time;
            }
            if time.unix_millis() > ended.unix_millis() {
                ended = time;
            }
            if root.is_empty() {
                root =
                    crate::reader::string(item, "metricName").unwrap_or_else(|| "root".to_owned());
            }
            if let Some(id) = crate::reader::string(item, "observationId")
                .and_then(|text| text.parse::<aex_wire::ids::ObservationId>().ok())
            {
                spans.push(Observation {
                    accepted_at: crate::reader::timestamp(item, "acceptedAt").unwrap_or(time),
                    body: empty_body(),
                    id,
                    observed_at: time,
                    run_id: None,
                    sequence: DecimalU128::new(u128::from(
                        crate::reader::number(item, "acceptedSeq").unwrap_or(0),
                    )),
                    session_id: Some(session),
                    signal,
                    span_id: crate::reader::string(item, "spanId")
                        .and_then(|text| aex_wire::ids::SpanId::parse(&text).ok()),
                    trace_id: Some(trace_id),
                    workspace_id: self.workspace(),
                });
            }
        }
        let snapshot = aex_observation_query::coverage::Snapshot::pin(ended, ended)
            .ok_or_else(|| WireError::new(ErrorCode::ObservabilityUnavailable))?;
        let gaps = self
            .service
            .reader
            .gap_history(&scope, self.workspace(), snapshot)
            .await
            .map_err(|error| read_error(&error))?;
        let window_end = Timestamp::from_unix_millis(
            ended
                .unix_millis()
                .checked_add(1)
                .ok_or_else(|| WireError::new(ErrorCode::InternalError))?,
        )
        .map_err(|_| WireError::new(ErrorCode::InternalError))?;
        let window = TimeWindow::new(started, window_end)
            .ok_or_else(|| WireError::new(ErrorCode::InternalError))?;
        Ok(TraceDetail {
            coverage: query::coverage(
                snapshot,
                ended,
                started,
                SignalSet::from_signal(Signal::Spans)
                    .with(Signal::Logs)
                    .with(Signal::Traces),
                window,
                &gaps,
            )?,
            summary: TraceSummary {
                ended_at: ended,
                root_name: root,
                span_count: DecimalU128::new(spans.len() as u128),
                started_at: started,
                trace_id,
                workspace_id: Some(self.workspace()),
            },
            spans,
        })
    }

    /// Reads one recorded gap.
    async fn gap(
        &self,
        session: Option<SessionId>,
        gap: TelemetryGapId,
    ) -> WireResult<TelemetryGap> {
        let scope = self.scope(session);
        self.deletion_epoch(&scope).await?;
        let record = self
            .service
            .reader
            .latest_gap(&scope, gap)
            .await
            .map_err(|error| read_error(&error))?
            .ok_or_else(|| WireError::new(ErrorCode::NotFound))?;
        if record.workspace != self.workspace() {
            return Err(WireError::new(ErrorCode::NotFound));
        }
        Ok(gap_to_wire(&record))
    }

    /// Queries recorded gaps at one scope.
    async fn gaps(
        &self,
        cx: &RequestContext,
        session: Option<SessionId>,
        body: TelemetryGapQuery,
    ) -> WireResult<TelemetryGapPage> {
        let scope = self.scope(session);
        self.deletion_epoch(&scope).await?;
        let now = now()?;
        let request_binding = query::gap_request_binding(
            cx,
            cx.route,
            self.service.region,
            &scope,
            self.workspace(),
            &body,
        );
        let (snapshot, after) = if let Some(cursor) = &body.cursor {
            let (snapshot, opened_at, gap_id) =
                query::resume_gap(&self.service.cursor_ring, cursor, &request_binding, now)?;
            (snapshot, Some((opened_at, gap_id)))
        } else {
            (
                self.service
                    .reader
                    .settled_snapshot(now)
                    .ok_or_else(|| WireError::new(ErrorCode::ObservabilityUnavailable))?,
                None,
            )
        };
        let mut records = self
            .service
            .reader
            .gap_history(&scope, self.workspace(), snapshot)
            .await
            .map_err(|error| read_error(&error))?;
        let window = TimeWindow::new(body.time_range.gte, body.time_range.lt)
            .ok_or_else(|| query::invalid_query("gap timeRange must be non-empty"))?;
        let selected = gap_signal_set(body.signals.as_deref());
        records.retain(|record| {
            record.revision.signals.intersects(selected)
                && record
                    .revision
                    .time_range
                    .is_none_or(|range| range.intersects(window))
                && body
                    .recoverable
                    .is_none_or(|recoverable| record.recoverable == recoverable)
                && after
                    .is_none_or(|after| (record.revision.opened_at, record.revision.gap_id) > after)
        });
        records.sort_by_key(|record| (record.revision.opened_at, record.revision.gap_id));
        let limit = usize::try_from(body.limit.unwrap_or(100)).unwrap_or(100);
        let more = records.len() > limit;
        records.truncate(limit);
        let binding = query::gap_binding(&request_binding, snapshot)?;
        let next_cursor = if more {
            records
                .last()
                .map(|last| {
                    query::issue_gap(
                        self.service.cursor_ring.current(),
                        &binding,
                        last.revision.opened_at,
                        last.revision.gap_id,
                        now,
                    )
                })
                .transpose()?
        } else {
            None
        };
        let items = records.iter().map(gap_to_wire).collect();
        Ok(TelemetryGapPage { items, next_cursor })
    }

    /// Admits one durable export operation.
    async fn admit_export(
        &self,
        cx: &RequestContext,
        session: Option<SessionId>,
        body: TelemetryExportRequest,
    ) -> WireResult<Accepted> {
        let operation_id = cx
            .operation_id
            .ok_or_else(|| WireError::new(ErrorCode::InvalidRequest))?;
        let now = now()?;
        let export_id = ExportId::from_uuid7(
            Uuid7::from_bytes(*uuid::Uuid::now_v7().as_bytes())
                .map_err(|_| WireError::new(ErrorCode::InternalError))?,
        );
        let scope = self.scope(session);
        let deletion_epoch = self.deletion_epoch(&scope).await?;
        let mut item = std::collections::HashMap::new();
        item.insert(
            "pk".to_owned(),
            aws_sdk_dynamodb::types::AttributeValue::S(keys::export_pk(
                self.workspace(),
                export_id,
            )),
        );
        item.insert(
            "sk".to_owned(),
            aws_sdk_dynamodb::types::AttributeValue::S("STATE".to_owned()),
        );
        for (name, value) in [
            ("itemType", "export".to_owned()),
            ("exportId", export_id.to_string()),
            ("operationId", operation_id.to_string()),
            ("workspaceId", self.workspace().to_string()),
            ("organizationId", self.organization().to_string()),
            ("scopeKey", scope.to_key()),
            ("format", body.format.as_str().to_owned()),
            ("completeness", body.completeness.as_str().to_owned()),
            ("state", "admitted".to_owned()),
            ("clientToken", export_id.to_string()),
            ("createdAt", now.to_wire()),
            (
                "cPk",
                keys::control_pk(keys::ControlDomain::ExportLaunch, 0),
            ),
            ("cSk", keys::control_sk(now, &export_id.to_string())),
        ] {
            item.insert(
                name.to_owned(),
                aws_sdk_dynamodb::types::AttributeValue::S(value),
            );
        }
        item.insert(
            "fence".to_owned(),
            aws_sdk_dynamodb::types::AttributeValue::N("0".to_owned()),
        );
        item.insert(
            "cancelRequested".to_owned(),
            aws_sdk_dynamodb::types::AttributeValue::Bool(false),
        );
        item.insert(
            "deletionEpochPinned".to_owned(),
            aws_sdk_dynamodb::types::AttributeValue::N(deletion_epoch.to_string()),
        );
        let mut builder = aex_observation_store_dynamodb::expressions::ExpressionBuilder::new();
        let condition = aex_observation_store_dynamodb::expressions::immutable_condition(&mut builder);
        self.service
            .reader
            .put_export_control(item, &condition, builder.names(), builder.values())
            .await
            .map_err(|error| read_error(&error))?;
        Ok(Accepted(Operation {
            cancelable: true,
            updated_at: now,
            workspace_id: self.workspace(),
            committed_at: None,
            created_at: now,
            error: None,
            id: operation_id,
            kind: OperationKind::TelemetryExport,
            progress: None,
            result: None,
            session_id: session,
            started_at: None,
            status: OperationStatus::Queued,
            terminal_at: None,
        }))
    }

    /// Reads one export record.
    async fn export(
        &self,
        session: Option<SessionId>,
        export: ExportId,
    ) -> WireResult<TelemetryExport> {
        self.deletion_epoch(&self.scope(session)).await?;
        let item = self
            .service
            .reader
            .read_item(&keys::export_pk(self.workspace(), export), "STATE")
            .await
            .map_err(|error| read_error(&error))?
            .ok_or_else(|| WireError::new(ErrorCode::NotFound))?;
        decode_export(&item, self.workspace(), session, export)
    }

    /// Revokes one export and schedules its reaping.
    async fn revoke(
        &self,
        session: Option<SessionId>,
        export: ExportId,
    ) -> WireResult<TelemetryExport> {
        let mut builder = aex_observation_store_dynamodb::expressions::ExpressionBuilder::new();
        let state = builder.name("state");
        let revoked = builder.string("revoked");
        let revoked_at = builder.name("revokedAt");
        let at = builder.string(now()?.to_wire());
        let cancel = builder.name("cancelRequested");
        let truth = builder.boolean(true);
        let existing = builder.name("pk");
        self.service
            .reader
            .update_export_control(
                &keys::export_pk(self.workspace(), export),
                "STATE",
                &format!("SET {state} = {revoked}, {revoked_at} = {at}, {cancel} = {truth}"),
                &format!("attribute_exists({existing})"),
                builder.names(),
                builder.values(),
            )
            .await
            .map_err(|error| read_error(&error))?;
        self.export(session, export).await
    }

    /// Mints a download grant for one ready export.
    async fn grant(
        &self,
        session: Option<SessionId>,
        export: ExportId,
    ) -> WireResult<Created<DownloadGrant>> {
        let record = self.export(session, export).await?;
        if record.status != ExportStatus::Ready {
            // A grant is mintable only in `ready`; reading an operation never
            // mints a URL.
            return Err(WireError::new(ErrorCode::PreconditionFailed));
        }
        let item = self
            .service
            .reader
            .read_item(&keys::export_pk(self.workspace(), export), "STATE")
            .await
            .map_err(|error| read_error(&error))?
            .ok_or_else(|| WireError::new(ErrorCode::NotFound))?;
        let key = crate::reader::string(&item, "objectKey")
            .ok_or_else(|| WireError::new(ErrorCode::PreconditionFailed))?;
        let url = self
            .service
            .reader
            .presign_export(&key, GRANT_LIFETIME)
            .await
            .map_err(|error| read_error(&error))?;
        let size = crate::reader::number(&item, "objectBytes").unwrap_or(0);
        let expires = Timestamp::from_unix_millis(
            now()?.unix_millis() + i64::try_from(GRANT_LIFETIME.as_millis()).unwrap_or(300_000),
        )
        .map_err(|_| WireError::new(ErrorCode::InternalError))?;
        Ok(Created(DownloadGrant {
            authorized_bytes: DecimalU128::new(u128::from(size)),
            expires_at: expires,
            measurement_id: MeasurementId::from_uuid7(
                Uuid7::from_bytes(*uuid::Uuid::now_v7().as_bytes())
                    .map_err(|_| WireError::new(ErrorCode::InternalError))?,
            ),
            range: None,
            sha256: record
                .manifest_hash
                .ok_or_else(|| WireError::new(ErrorCode::PreconditionFailed))?,
            size_bytes: DecimalU128::new(u128::from(size)),
            url: aex_wire::types::HttpsUrl::parse(&url)
                .map_err(|_| WireError::new(ErrorCode::InternalError))?,
        }))
    }
}

fn bind_metric_selection(
    query: &mut aex_observation_query::plan::NormalizedQuery,
    scope: &ScopeKey,
    metric: &str,
) {
    let exact = Predicate::Cmp {
        field: FieldRef::Signal(Signal::Metrics, "metricName".into()),
        op: CmpOp::Eq,
        value: aex_observation_domain::canonical::CanonicalValue::Str(metric.into()),
    };
    query.predicate = Some(match query.predicate.take() {
        Some(existing) => Predicate::And(vec![exact, existing]),
        None => exact,
    });
    query.metric_name = match scope {
        ScopeKey::Workspace(_) => Some(metric.into()),
        // The workspace metric index has no session component. Session
        // aggregation stays on the session-bound scope-time partitions and
        // applies the exact projected metric-name predicate there.
        ScopeKey::Session(_) => None,
    };
}

/// Everything one frame producer owns for the life of a connection.
struct Produce {
    sender: ndjson::FrameSender,
    service: Arc<ObservationService>,
    workspace: WorkspaceId,
    scope: ScopeKey,
    normalized: aex_observation_query::plan::NormalizedQuery,
    plan: aex_observation_query::plan::Plan,
    binding: aex_regional_http::cursor::CursorBinding,
    snapshot: aex_observation_query::coverage::Snapshot,
    follow: bool,
    after: Option<OrderTuple>,
    resume: Option<aex_observation_query::ObservationResume>,
    frontier: Frontier,
    request_id: aex_wire::types::RequestId,
    wake: Option<crate::wake::WakeSubscription>,
    authorization: aex_regional_http::context::RegionalAuthorization,
    seen_gaps: std::collections::BTreeMap<TelemetryGapId, u64>,
}

/// Counts one producer's lifetime against the process's read accounting.
///
/// A producer returns from about a dozen places, and a disconnect is one of
/// them. Binding the close to `Drop` is what makes "sockets opened" and "sockets
/// closed" reconcile on every exit path rather than on the ones somebody
/// remembered.
struct SocketLifetime(Arc<ReadCounters>);

impl SocketLifetime {
    fn open(counters: &Arc<ReadCounters>) -> Self {
        counters.record(ReadCounter::SocketOpened);
        Self(Arc::clone(counters))
    }
}

impl Drop for SocketLifetime {
    fn drop(&mut self) {
        self.0.record(ReadCounter::SocketClosed);
    }
}

/// Streams pages as whole frames, then rotates.
///
/// `rotate` is the only terminal frame after the headers flushed: once the
/// status is on the wire there is nothing left to change, so a failure is a
/// frame carrying its reason rather than a status a reader cannot see.
#[allow(
    clippy::too_many_lines,
    reason = "one socket state machine keeps replay, renewal, heartbeat, wake and terminal-frame ordering auditable"
)]
async fn produce(mut task: Produce) {
    let _socket = SocketLifetime::open(task.service.reader.counters());
    let deadline = std::time::Instant::now() + task.service.stream.connection_budget;
    let mut after = task.after;
    let mut resume = task.resume;
    let mut gap_watch = GapWatch::new(task.service.stream.gap_recovery);
    let mut trigger = CycleTrigger::Fallback;
    let mut gaps = Vec::new();
    let mut poll_delay = task.service.stream.poll_min;
    let mut next_heartbeat = std::time::Instant::now() + task.service.stream.heartbeat;
    // Revalidate once before the first authoritative read, then periodically.
    // This closes the handshake-to-producer race for revocation, pause,
    // placement and session deletion without retaining credential plaintext.
    let mut next_authorization_check = std::time::Instant::now();
    let Some(query_window) = TimeWindow::new(task.normalized.time_gte, task.normalized.time_lt)
    else {
        let _ = task
            .sender
            .send(&ndjson::rotate(
                None,
                RotateReason::UpstreamUnavailable,
                false,
            ))
            .await;
        return;
    };
    loop {
        if task.service.stream.draining.load(Ordering::Acquire) {
            let cursor = task.sender.sent().cloned();
            let _ = task
                .sender
                .send(&ndjson::rotate(cursor, RotateReason::ServerRotating, true))
                .await;
            return;
        }
        if std::time::Instant::now() >= next_authorization_check {
            task.service
                .reader
                .counters()
                .record(ReadCounter::AuthorizationRenewal);
            if let Err(error) = task
                .service
                .stream
                .revalidator
                .revalidate(&task.authorization, &task.scope)
                .await
            {
                let cursor = task.sender.sent().cloned();
                let (_, envelope, _) = error.into_response_parts(&task.request_id, None);
                let _ = task
                    .sender
                    .send(&ndjson::failed(cursor, envelope.error))
                    .await;
                return;
            }
            next_authorization_check =
                std::time::Instant::now() + task.service.stream.authorization_check;
        }
        // A cycle that does not refresh the frontier has no hint to consult:
        // a finite walk pins one snapshot for all of its pages.
        let mut gap_change = None;
        if task.follow {
            let refreshed = async {
                let current = now()?;
                let bundle = task
                    .service
                    .reader
                    .frontier_bundle(&task.scope, task.workspace, &task.normalized)
                    .await
                    .map_err(|error| read_error(&error))?;
                let frontier = bundle.combine(&task.normalized);
                ensure_queryable(&task.scope, frontier.deletion_state)?;
                let snapshot = task
                    .service
                    .reader
                    .pin(frontier, current)
                    .ok_or_else(|| WireError::new(ErrorCode::ObservabilityUnavailable))?;
                Ok::<_, WireError>((frontier, snapshot, bundle.gap_change))
            }
            .await;
            let Ok((frontier, snapshot, change)) = refreshed else {
                let cursor = task.sender.sent().cloned();
                let _ = task
                    .sender
                    .send(&ndjson::rotate(
                        cursor,
                        RotateReason::UpstreamUnavailable,
                        true,
                    ))
                    .await;
                return;
            };
            task.frontier = frontier;
            task.snapshot = snapshot;
            let Ok(token) =
                aex_regional_http::cursor::SnapshotToken::new(snapshot.to_wire().to_string())
            else {
                let cursor = task.sender.sent().cloned();
                let _ = task
                    .sender
                    .send(&ndjson::rotate(
                        cursor,
                        RotateReason::UpstreamUnavailable,
                        false,
                    ))
                    .await;
                return;
            };
            task.binding.snapshot = token;
            gap_change = Some(change);
        }
        // The hint decides only whether the ledger is worth reading; the ledger
        // it last returned still decides what the coverage and gap frames below
        // say. A cycle that reuses is a cycle that proved nothing was appended.
        let observation = gap_change.map_or(GapObservation::Pinned, |change| {
            GapObservation::Observed { change, trigger }
        });
        let read = gap_watch
            .gap_history(
                observation,
                task.service.reader.counters(),
                std::time::Instant::now(),
                gaps,
                task.service
                    .reader
                    .gap_history(&task.scope, task.workspace, task.snapshot),
            )
            .await;
        let Ok(current) = read else {
            let cursor = task.sender.sent().cloned();
            let _ = task
                .sender
                .send(&ndjson::rotate(
                    cursor,
                    RotateReason::UpstreamUnavailable,
                    true,
                ))
                .await;
            return;
        };
        gaps = current;
        let current_coverage = match query::coverage(
            task.snapshot,
            task.frontier.accepted_at,
            task.frontier.earliest_accepted_at,
            task.normalized.signals,
            query_window,
            &gaps,
        ) {
            Ok(coverage) => coverage,
            Err(error) => {
                let cursor = task.sender.sent().cloned();
                let (_, envelope, _) = error.into_response_parts(&task.request_id, None);
                let _ = task
                    .sender
                    .send(&ndjson::failed(cursor, envelope.error))
                    .await;
                return;
            }
        };
        for gap in unseen_gaps(
            &gaps,
            task.normalized.signals,
            query_window,
            &task.seen_gaps,
        ) {
            if !task
                .sender
                .send(&ObservationFrame::Gap(ObservationFrameGap {
                    gap: gap_to_wire(gap),
                }))
                .await
            {
                return;
            }
            task.seen_gaps
                .insert(gap.revision.gap_id, gap.revision.revision);
        }
        let outcome = task
            .service
            .reader
            .read_page(
                &task.scope,
                task.workspace,
                &task.normalized,
                &task.plan,
                task.snapshot,
                after.as_ref(),
                resume.as_ref(),
                task.follow,
            )
            .await;
        let Ok(page) = outcome else {
            let cursor = task.sender.sent().cloned();
            let _ = task
                .sender
                .send(&ndjson::rotate(
                    cursor,
                    RotateReason::UpstreamUnavailable,
                    true,
                ))
                .await;
            return;
        };
        let counters = task.service.reader.counters();
        counters.add(
            ReadCounter::RecordsReturned,
            page.items.len().try_into().unwrap_or(u64::MAX),
        );
        counters.add(ReadCounter::BytesReturned, page.spend.bytes_read);
        let more = page.more;
        let page_resume = page.resume.clone();
        let progressed = !page.items.is_empty();
        if !page.items.is_empty() {
            let Some(last) = page.last else {
                let cursor = task.sender.sent().cloned();
                let _ = task
                    .sender
                    .send(&ndjson::rotate(
                        cursor,
                        RotateReason::UpstreamUnavailable,
                        false,
                    ))
                    .await;
                return;
            };
            let record_frames = match split_records(page.items) {
                Ok(frames) => frames,
                Err(error) => {
                    let cursor = task.sender.sent().cloned();
                    let (_, envelope, _) = WireError::new(ErrorCode::InternalError)
                        .with_message(error.to_string())
                        .into_response_parts(&task.request_id, None);
                    let _ = task
                        .sender
                        .send(&ndjson::failed(cursor, envelope.error))
                        .await;
                    return;
                }
            };
            for frame in record_frames {
                if !task.sender.send(&frame).await {
                    return;
                }
            }
            let issued_at = now().unwrap_or_else(|_| task.snapshot.accepted_at());
            let issued = match (more, page_resume.as_ref()) {
                (true, Some(resume)) => query::issue_page(
                    task.service.cursor_ring.current(),
                    &task.binding,
                    resume,
                    issued_at,
                ),
                _ => query::issue(
                    task.service.cursor_ring.current(),
                    &task.binding,
                    last,
                    issued_at,
                ),
            };
            let Ok(cursor) = issued else {
                let cursor = task.sender.sent().cloned();
                let _ = task
                    .sender
                    .send(&ndjson::rotate(
                        cursor,
                        RotateReason::UpstreamUnavailable,
                        false,
                    ))
                    .await;
                return;
            };
            if !task
                .sender
                .send(&ObservationFrame::Cursor(ObservationFrameCursor {
                    coverage: current_coverage.clone(),
                    cursor,
                }))
                .await
            {
                return;
            }
            poll_delay = task.service.stream.poll_min;
            next_heartbeat = std::time::Instant::now() + task.service.stream.heartbeat;
        }
        if more {
            resume = page_resume;
            after = None;
        } else {
            resume = None;
            if let Some(last) = page.last {
                after = Some(last);
            }
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
        if !task.follow {
            if more {
                continue;
            }
            break;
        }
        if more {
            continue;
        }
        let now = std::time::Instant::now();
        if now >= next_heartbeat {
            if let Some(cursor) = task.sender.sent().cloned()
                && !task
                    .sender
                    .send(&ObservationFrame::Cursor(ObservationFrameCursor {
                        coverage: current_coverage.clone(),
                        cursor,
                    }))
                    .await
            {
                return;
            }
            next_heartbeat = now + task.service.stream.heartbeat;
        }
        if !progressed {
            poll_delay = poll_delay
                .saturating_mul(2)
                .min(task.service.stream.poll_max);
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let until_heartbeat = next_heartbeat.saturating_duration_since(std::time::Instant::now());
        let until_authorization =
            next_authorization_check.saturating_duration_since(std::time::Instant::now());
        let wait = poll_delay
            .min(remaining)
            .min(until_heartbeat)
            .min(until_authorization);
        let woke = if let Some(wake) = task.wake.as_mut() {
            wake.wait(wait).await
        } else {
            tokio::time::sleep(wait).await;
            false
        };
        // A wake is a latency hint and never authority, so both paths run the
        // same authoritative cycle next. They are counted apart only so the
        // hint's contribution to the cycle rate is measurable.
        task.service.reader.counters().record(if woke {
            ReadCounter::WakePoll
        } else {
            ReadCounter::FallbackPoll
        });
        trigger = if woke {
            CycleTrigger::Wake
        } else {
            CycleTrigger::Fallback
        };
        if woke {
            poll_delay = task.service.stream.poll_min;
        }
    }
    let cursor = task.sender.sent().cloned();
    let _ = task
        .sender
        .send(&ndjson::rotate(cursor, RotateReason::BudgetExhausted, true))
        .await;
}

/// What a `stream` or `listen` request asks for, normalized to one shape.
struct StreamBody {
    filter: Option<aex_wire::models::ObservationFilter>,
    signal: ObservationSignal,
    window: TimeRange,
    follow: bool,
    resume: Option<aex_wire::cursor::Cursor>,
}

/// The window a replay origin selects.
fn window_for(
    origin: &aex_wire::models::ObservationOrigin,
    now: Timestamp,
) -> WireResult<TimeRange> {
    use aex_wire::models::ObservationOrigin;

    let lt = Timestamp::from_unix_millis(now.unix_millis() + 1)
        .map_err(|_| WireError::new(ErrorCode::InternalError))?;
    let gte = match origin {
        ObservationOrigin::Time(at) => at.at,
        // The authority is the replay rail and retains everything until an
        // explicit deletion, so `earliest` is the beginning of retained data.
        ObservationOrigin::Earliest(_) | ObservationOrigin::Cursor(_) => {
            Timestamp::from_unix_millis(0).map_err(|_| WireError::new(ErrorCode::InternalError))?
        }
    };
    Ok(TimeRange { gte, lt })
}

/// The window a `listen` follows from: the current accepted position onward.
fn listen_window(now: Timestamp) -> WireResult<TimeRange> {
    Ok(TimeRange {
        gte: now,
        lt: Timestamp::from_unix_millis(
            now.unix_millis() + i64::try_from(LISTEN_BUDGET.as_millis()).unwrap_or(60_000) + 1,
        )
        .map_err(|_| WireError::new(ErrorCode::InternalError))?,
    })
}

/// The current instant, as the fixed-width wire spelling.
fn now() -> WireResult<Timestamp> {
    Timestamp::from_datetime_trunc_ms(time::OffsetDateTime::now_utc())
        .map_err(|_| WireError::new(ErrorCode::InternalError))
}

/// The empty canonical body a projected read carries when the item is a key.
fn empty_body() -> aex_wire::canonical::CanonicalJson {
    aex_wire::canonical::CanonicalJson::parse("{}").expect("the empty object is canonical")
}

/// A timestamp the bounded range always admits.
fn unreachable_timestamp() -> Timestamp {
    Timestamp::from_unix_millis(0).unwrap_or_else(|_| {
        Timestamp::parse("1970-01-01T00:00:00.000Z").expect("the epoch always parses")
    })
}

/// Applies the authoritative deletion fence after any cursor epoch comparison.
fn ensure_queryable(scope: &ScopeKey, state: ScopeDeletionState) -> WireResult<()> {
    match (scope, state) {
        (_, ScopeDeletionState::Open) => Ok(()),
        (ScopeKey::Session(_), ScopeDeletionState::Deleting) => {
            Err(WireError::new(ErrorCode::SessionDeleting))
        }
        (ScopeKey::Session(_), ScopeDeletionState::Deleted) => {
            Err(WireError::new(ErrorCode::SessionDeleted))
        }
        (ScopeKey::Workspace(_), ScopeDeletionState::Deleting | ScopeDeletionState::Deleted) => {
            Err(WireError::new(ErrorCode::Gone))
        }
    }
}

/// Maps a read failure onto the public vocabulary.
fn read_error(error: &ReadError) -> WireError {
    match error {
        ReadError::BudgetExhausted { dimension, scanned } => {
            query::budget_exhausted(dimension, *scanned, *scanned)
        }
        // The authority itself is unreachable. `503 observability_unavailable`
        // now means exactly that, never a lagging projection.
        ReadError::Provider { .. } => WireError::new(ErrorCode::ObservabilityUnavailable)
            .with_retry_after(Duration::from_secs(1)),
        ReadError::InvalidResume => WireError::new(ErrorCode::InvalidCursor),
        ReadError::Malformed { .. } | ReadError::InvalidWriteTarget { .. } => {
            WireError::new(ErrorCode::InternalError)
        }
    }
}

/// Converts a validated durable gap without inventing missing evidence.
fn gap_to_wire(record: &GapRecord) -> TelemetryGap {
    TelemetryGap {
        byte_count: record
            .attempted_bytes
            .map(|value| DecimalU128::new(u128::from(value))),
        detected_at: record.revision.opened_at,
        from_sequence: record
            .revision
            .ordinal_range
            .map(|range| DecimalU128::new(u128::from(range.lo()))),
        id: record.revision.gap_id,
        observation_count: record
            .attempted_records
            .map(|value| DecimalU128::new(u128::from(value))),
        reason: record.revision.reason,
        recoverable: record.recoverable,
        repair_source: record.revision.repair_source.as_deref().map(str::to_owned),
        repaired_at: record
            .revision
            .repair_source
            .as_ref()
            .map(|_| record.revision.revised_at),
        revision: record.revision.revision,
        session_id: record.scope.session(),
        signals: record
            .revision
            .signals
            .iter()
            .map(Signal::to_wire)
            .collect(),
        time_range: record.revision.time_range.map(|range| TimeRange {
            gte: range.from(),
            lt: range.to(),
        }),
        to_sequence: record
            .revision
            .ordinal_range
            .map(|range| DecimalU128::new(u128::from(range.hi()))),
        workspace_id: record.workspace,
    }
}

/// Selects relevant lifecycle revisions not yet emitted on this connection.
///
/// The latest repaired revision is intentionally included: a reader that saw
/// the opening frame must also learn that completeness changed.
fn unseen_gaps<'a>(
    gaps: &'a [GapRecord],
    signals: SignalSet,
    window: TimeWindow,
    seen: &std::collections::BTreeMap<TelemetryGapId, u64>,
) -> Vec<&'a GapRecord> {
    gaps.iter()
        .filter(|gap| {
            gap.revision.signals.intersects(signals)
                && gap
                    .revision
                    .time_range
                    .is_none_or(|range| range.intersects(window))
                && seen
                    .get(&gap.revision.gap_id)
                    .is_none_or(|revision| *revision < gap.revision.revision)
        })
        .collect()
}

/// Expands the public `telemetry` selector and unions every requested signal.
fn gap_signal_set(signals: Option<&[ObservationSignal]>) -> SignalSet {
    signals.map_or_else(SignalSet::all, |signals| {
        signals
            .iter()
            .copied()
            .fold(SignalSet::EMPTY, |set, signal| {
                SignalSet::from_wire(signal)
                    .iter()
                    .fold(set, SignalSet::with)
            })
    })
}

/// Decodes one stored export record.
fn decode_export(
    item: &std::collections::HashMap<String, aws_sdk_dynamodb::types::AttributeValue>,
    workspace: WorkspaceId,
    session: Option<SessionId>,
    export: ExportId,
) -> WireResult<TelemetryExport> {
    let created = crate::reader::timestamp(item, "createdAt")
        .ok_or_else(|| WireError::new(ErrorCode::InternalError))?;
    let format = crate::reader::string(item, "format").unwrap_or_default();
    let completeness = crate::reader::string(item, "completeness").unwrap_or_default();
    Ok(TelemetryExport {
        completeness: ExportCompleteness::ALL
            .iter()
            .copied()
            .find(|candidate| candidate.as_str() == completeness)
            .unwrap_or(ExportCompleteness::AllowGaps),
        created_at: created,
        expires_at: crate::reader::timestamp(item, "expiresAt"),
        format: ExportFormat::ALL
            .iter()
            .copied()
            .find(|candidate| candidate.as_str() == format)
            .unwrap_or(ExportFormat::Ndjson),
        gap_ids: None,
        id: export,
        manifest_hash: crate::reader::string(item, "manifestHash")
            .and_then(|text| aex_wire::ids::ContentHash::parse(&text).ok()),
        ready_at: crate::reader::timestamp(item, "readyAt"),
        revoked_at: crate::reader::timestamp(item, "revokedAt"),
        session_id: session,
        size_bytes: crate::reader::number(item, "objectBytes")
            .map(|value| DecimalU128::new(u128::from(value))),
        status: export_status(&crate::reader::string(item, "state").unwrap_or_default()),
        workspace_id: workspace,
    })
}

/// Maps the durable export state onto its public status.
fn export_status(state: &str) -> ExportStatus {
    match state {
        "ready" => ExportStatus::Ready,
        "expired" => ExportStatus::Expired,
        "revoked" => ExportStatus::Revoked,
        _ => ExportStatus::Preparing,
    }
}

/// Declares the workspace-scoped page routes of one signal.
macro_rules! workspace_pages {
    ($($method:ident => $signal:ident),* $(,)?) => {
        $(
            async fn $method(
                &self,
                cx: &RequestContext,
                body: ObservationQuery,
            ) -> WireResult<ObservationPage> {
                self.page(cx, None, ObservationSignal::$signal, body).await
            }
        )*
    };
}

/// Declares the session-scoped page routes of one signal.
macro_rules! session_pages {
    ($($method:ident => $signal:ident),* $(,)?) => {
        $(
            async fn $method(
                &self,
                cx: &RequestContext,
                session_id: SessionId,
                body: ObservationQuery,
            ) -> WireResult<ObservationPage> {
                self.page(cx, Some(session_id), ObservationSignal::$signal, body).await
            }
        )*
    };
}

/// Declares the workspace-scoped replay routes of one signal.
macro_rules! workspace_streams {
    ($($method:ident => $signal:ident),* $(,)?) => {
        $(
            async fn $method(
                &self,
                cx: &RequestContext,
                body: ObservationStreamRequest,
            ) -> WireResult<NdjsonStream<FrameStream>> {
                let resume = match &body.origin {
                    aex_wire::models::ObservationOrigin::Cursor(origin) => {
                        Some(origin.cursor.clone())
                    }
                    _ => None,
                };
                let window = window_for(&body.origin, now()?)?;
                self.frames(
                    cx,
                    None,
                    ObservationSignal::$signal,
                    StreamBody {
                        filter: body.filter,
                        signal: body.signal,
                        window,
                        follow: false,
                        resume,
                    },
                )
                .await
            }
        )*
    };
}

/// Declares the session-scoped replay routes of one signal.
macro_rules! session_streams {
    ($($method:ident => $signal:ident),* $(,)?) => {
        $(
            async fn $method(
                &self,
                cx: &RequestContext,
                session_id: SessionId,
                body: ObservationStreamRequest,
            ) -> WireResult<NdjsonStream<FrameStream>> {
                let resume = match &body.origin {
                    aex_wire::models::ObservationOrigin::Cursor(origin) => {
                        Some(origin.cursor.clone())
                    }
                    _ => None,
                };
                let window = window_for(&body.origin, now()?)?;
                self.frames(
                    cx,
                    Some(session_id),
                    ObservationSignal::$signal,
                    StreamBody {
                        filter: body.filter,
                        signal: body.signal,
                        window,
                        follow: false,
                        resume,
                    },
                )
                .await
            }
        )*
    };
}

/// Declares the workspace-scoped follow routes of one signal.
macro_rules! workspace_listens {
    ($($method:ident => $signal:ident),* $(,)?) => {
        $(
            async fn $method(
                &self,
                cx: &RequestContext,
                body: ObservationListenRequest,
            ) -> WireResult<NdjsonStream<FrameStream>> {
                let window = listen_window(now()?)?;
                self.frames(
                    cx,
                    None,
                    ObservationSignal::$signal,
                    StreamBody {
                        filter: body.filter,
                        signal: body.signal,
                        window,
                        follow: true,
                        resume: None,
                    },
                )
                .await
            }
        )*
    };
}

/// Declares the session-scoped follow routes of one signal.
macro_rules! session_listens {
    ($($method:ident => $signal:ident),* $(,)?) => {
        $(
            async fn $method(
                &self,
                cx: &RequestContext,
                session_id: SessionId,
                body: ObservationListenRequest,
            ) -> WireResult<NdjsonStream<FrameStream>> {
                let window = listen_window(now()?)?;
                self.frames(
                    cx,
                    Some(session_id),
                    ObservationSignal::$signal,
                    StreamBody {
                        filter: body.filter,
                        signal: body.signal,
                        window,
                        follow: true,
                        resume: None,
                    },
                )
                .await
            }
        )*
    };
}

impl ObservationsApi for ObservationRequest {
    type FrameStream = FrameStream;

    workspace_pages! {
        observations_events_query => Events,
        observations_logs_query => Logs,
        observations_metrics_query => Metrics,
        observations_spans_query => Spans,
        observations_telemetry_query => Telemetry,
        observations_traces_query => Traces,
    }

    session_pages! {
        session_observations_events_query => Events,
        session_observations_logs_query => Logs,
        session_observations_metrics_query => Metrics,
        session_observations_spans_query => Spans,
        session_observations_telemetry_query => Telemetry,
        session_observations_traces_query => Traces,
    }

    workspace_streams! {
        observations_events_stream => Events,
        observations_logs_stream => Logs,
        observations_metrics_stream => Metrics,
        observations_spans_stream => Spans,
        observations_telemetry_stream => Telemetry,
        observations_traces_stream => Traces,
    }

    session_streams! {
        session_observations_events_stream => Events,
        session_observations_logs_stream => Logs,
        session_observations_metrics_stream => Metrics,
        session_observations_spans_stream => Spans,
        session_observations_telemetry_stream => Telemetry,
        session_observations_traces_stream => Traces,
    }

    workspace_listens! {
        observations_events_listen => Events,
        observations_logs_listen => Logs,
        observations_metrics_listen => Metrics,
        observations_spans_listen => Spans,
        observations_telemetry_listen => Telemetry,
        observations_traces_listen => Traces,
    }

    session_listens! {
        session_observations_events_listen => Events,
        session_observations_logs_listen => Logs,
        session_observations_metrics_listen => Metrics,
        session_observations_spans_listen => Spans,
        session_observations_telemetry_listen => Telemetry,
        session_observations_traces_listen => Traces,
    }

    async fn observations_metrics_aggregate(
        &self,
        cx: &RequestContext,
        body: MetricAggregationRequest,
    ) -> WireResult<MetricAggregationPage> {
        self.aggregate(cx, None, body).await
    }

    async fn session_observations_metrics_aggregate(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: MetricAggregationRequest,
    ) -> WireResult<MetricAggregationPage> {
        self.aggregate(cx, Some(session_id), body).await
    }

    async fn session_observations_trace_get(
        &self,
        _cx: &RequestContext,
        session_id: SessionId,
        trace_id: TraceId,
    ) -> WireResult<TraceDetail> {
        self.trace(session_id, trace_id).await
    }
}

impl TelemetryLifecycleApi for ObservationRequest {
    async fn telemetry_export_create(
        &self,
        cx: &RequestContext,
        body: TelemetryExportRequest,
    ) -> WireResult<Accepted> {
        self.admit_export(cx, None, body).await
    }

    async fn session_telemetry_export_create(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: TelemetryExportRequest,
    ) -> WireResult<Accepted> {
        self.admit_export(cx, Some(session_id), body).await
    }

    async fn telemetry_export_get(
        &self,
        _cx: &RequestContext,
        export_id: ExportId,
    ) -> WireResult<TelemetryExport> {
        self.export(None, export_id).await
    }

    async fn session_telemetry_export_get(
        &self,
        _cx: &RequestContext,
        session_id: SessionId,
        export_id: ExportId,
    ) -> WireResult<TelemetryExport> {
        self.export(Some(session_id), export_id).await
    }

    async fn telemetry_export_revoke(
        &self,
        _cx: &RequestContext,
        export_id: ExportId,
        _body: EmptyRequest,
    ) -> WireResult<TelemetryExport> {
        self.revoke(None, export_id).await
    }

    async fn session_telemetry_export_revoke(
        &self,
        _cx: &RequestContext,
        session_id: SessionId,
        export_id: ExportId,
        _body: EmptyRequest,
    ) -> WireResult<TelemetryExport> {
        self.revoke(Some(session_id), export_id).await
    }

    async fn telemetry_export_download_create(
        &self,
        _cx: &RequestContext,
        export_id: ExportId,
        _body: EmptyRequest,
    ) -> WireResult<Created<DownloadGrant>> {
        self.grant(None, export_id).await
    }

    async fn session_telemetry_export_download_create(
        &self,
        _cx: &RequestContext,
        session_id: SessionId,
        export_id: ExportId,
        _body: EmptyRequest,
    ) -> WireResult<Created<DownloadGrant>> {
        self.grant(Some(session_id), export_id).await
    }

    async fn telemetry_gap_get(
        &self,
        _cx: &RequestContext,
        gap_id: TelemetryGapId,
    ) -> WireResult<TelemetryGap> {
        self.gap(None, gap_id).await
    }

    async fn session_telemetry_gap_get(
        &self,
        _cx: &RequestContext,
        session_id: SessionId,
        gap_id: TelemetryGapId,
    ) -> WireResult<TelemetryGap> {
        self.gap(Some(session_id), gap_id).await
    }

    async fn telemetry_gaps_query(
        &self,
        cx: &RequestContext,
        body: TelemetryGapQuery,
    ) -> WireResult<TelemetryGapPage> {
        self.gaps(cx, None, body).await
    }

    async fn session_telemetry_gaps_query(
        &self,
        cx: &RequestContext,
        session_id: SessionId,
        body: TelemetryGapQuery,
    ) -> WireResult<TelemetryGapPage> {
        self.gaps(cx, Some(session_id), body).await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use aex_observation_domain::canonical::CanonicalValue;
    use aex_observation_domain::gap::{GapRecord, GapRevision, TimeWindow};
    use aex_observation_domain::keys::{BucketHour, ScopeKey};
    use aex_observation_domain::order::{Direction, OrderBy};
    use aex_observation_domain::signal::{Signal, SignalSet};
    use aex_observation_query::ast::MapRow;
    use aex_observation_query::plan::{Access, Budget, NormalizedQuery, ScopeAxis, plan};
    use aex_wire::ids::{PrefixedId as _, SessionId, TelemetryGapId, Uuid7, WorkspaceId};
    use aex_wire::models::{
        ExportStatus, ObservationOrigin, ObservationOriginEarliest, TelemetryGapReason,
    };
    use aex_wire::types::Timestamp;

    use super::{
        GRANT_LIFETIME, LISTEN_BUDGET, SocketLifetime, bind_metric_selection, export_status,
        gap_to_wire, listen_window, unseen_gaps, window_for,
    };
    use crate::counters::{ReadCounter, ReadCounters};

    #[test]
    fn a_producer_that_ends_for_any_reason_closes_exactly_one_socket() {
        // A producer returns from a dozen places, disconnect included. Binding
        // the close to the guard's `Drop` is what makes the two counters
        // reconcile without every exit path remembering to.
        let counters = std::sync::Arc::new(ReadCounters::default());
        {
            let _socket = SocketLifetime::open(&counters);
            assert_eq!(counters.total(ReadCounter::SocketOpened), 1);
            assert_eq!(counters.total(ReadCounter::SocketClosed), 0);
        }
        assert_eq!(counters.total(ReadCounter::SocketClosed), 1);
    }

    fn gap() -> GapRecord {
        let workspace =
            WorkspaceId::parse("wsp_0000000001e40r2081040g2081").expect("workspace fixture");
        GapRecord::try_new(
            workspace,
            ScopeKey::Workspace(workspace),
            GapRevision::open(
                TelemetryGapId::parse("gap_0000000001e40r2081040g2081").expect("gap fixture"),
                SignalSet::from_signal(Signal::Logs),
                TelemetryGapReason::SpoolLost,
                None,
                Timestamp::from_unix_millis(30).expect("instant"),
            ),
            Some(2),
            Some(64),
            false,
        )
        .expect("gap record")
    }

    #[test]
    fn a_grant_and_its_signature_expire_together() {
        // OD-17: matching the two removes the gap where a leaked signature
        // outlives the grant that authorized it.
        assert_eq!(GRANT_LIFETIME.as_secs(), 300);
    }

    #[test]
    fn an_earliest_origin_replays_from_the_beginning_of_retained_data() {
        let now = Timestamp::from_unix_millis(1_754_051_696_789).expect("bounded");
        let window = window_for(
            &ObservationOrigin::Earliest(ObservationOriginEarliest {}),
            now,
        )
        .expect("a bounded window");
        assert_eq!(window.gte.unix_millis(), 0);
        assert!(window.lt.unix_millis() > now.unix_millis());
    }

    #[test]
    fn a_listen_follows_from_now_and_is_bounded_by_its_own_budget() {
        let now = Timestamp::from_unix_millis(1_754_051_696_789).expect("bounded");
        let window = listen_window(now).expect("a bounded window");
        assert_eq!(window.gte, now);
        assert!(
            window.lt.unix_millis() - now.unix_millis()
                > i64::try_from(LISTEN_BUDGET.as_millis()).unwrap_or(0) - 1
        );
    }

    #[test]
    fn only_a_ready_export_is_ready() {
        assert_eq!(export_status("ready"), ExportStatus::Ready);
        assert_eq!(export_status("revoked"), ExportStatus::Revoked);
        assert_eq!(export_status("expired"), ExportStatus::Expired);
        for preparing in ["admitted", "launching", "generating", "failed", ""] {
            assert_eq!(export_status(preparing), ExportStatus::Preparing);
        }
    }

    #[test]
    fn two_session_metric_queries_use_isolated_scope_partitions_and_exact_names() {
        let workspace = WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]));
        let first = ScopeKey::Session(SessionId::from_uuid7(Uuid7::compose(2, [2; 10])));
        let second = ScopeKey::Session(SessionId::from_uuid7(Uuid7::compose(2, [3; 10])));
        let from = Timestamp::parse("2026-08-01T09:00:00.000Z").expect("time");
        let mut query = NormalizedQuery {
            axis: ScopeAxis::Scope,
            signals: SignalSet::from_signal(Signal::Metrics),
            predicate: None,
            time_gte: from,
            time_lt: Timestamp::parse("2026-08-01T10:00:00.000Z").expect("time"),
            order_by: OrderBy::Time,
            direction: Direction::Ascending,
            limit: 100,
            trace_id: None,
            metric_name: None,
        };
        bind_metric_selection(&mut query, &first, "http.server.duration");
        let planned = plan(&query, Budget::default()).expect("query plans");
        assert_eq!(planned.walks[0].access, Access::ScopeTime);

        let bucket = BucketHour::from_timestamp(from);
        let first_partition = crate::reader::segment_key(
            &first,
            workspace,
            &query,
            Access::ScopeTime,
            Signal::Metrics,
            bucket,
            0,
        )
        .expect("first session segment")
        .0
        .value;
        let second_partition = crate::reader::segment_key(
            &second,
            workspace,
            &query,
            Access::ScopeTime,
            Signal::Metrics,
            bucket,
            0,
        )
        .expect("second session segment")
        .0
        .value;
        assert_ne!(first_partition, second_partition);

        let predicate = query.predicate.as_ref().expect("exact name predicate");
        let matching = MapRow {
            fields: std::collections::BTreeMap::from([(
                "metricName".to_owned(),
                CanonicalValue::Str("http.server.duration".into()),
            )]),
            attributes: std::collections::BTreeMap::new(),
        };
        let other = MapRow {
            fields: std::collections::BTreeMap::from([(
                "metricName".to_owned(),
                CanonicalValue::Str("process.cpu.time".into()),
            )]),
            attributes: std::collections::BTreeMap::new(),
        };
        assert!(predicate.evaluate(&matching));
        assert!(!predicate.evaluate(&other));
    }

    #[test]
    fn an_unbounded_gap_stays_unbounded_on_the_wire() {
        let wire = gap_to_wire(&gap());
        assert!(wire.time_range.is_none());
        assert_eq!(
            wire.observation_count
                .map(aex_wire::types::DecimalU128::get),
            Some(2)
        );
    }

    #[test]
    fn a_stream_emits_each_relevant_revision_once_including_repair() {
        let open = gap();
        let window = TimeWindow::new(
            Timestamp::from_unix_millis(0).expect("instant"),
            Timestamp::from_unix_millis(100).expect("instant"),
        )
        .expect("window");
        let mut seen = BTreeMap::new();
        assert_eq!(
            unseen_gaps(
                std::slice::from_ref(&open),
                SignalSet::from_signal(Signal::Logs),
                window,
                &seen
            )
            .len(),
            1
        );
        seen.insert(open.revision.gap_id, open.revision.revision);
        assert!(
            unseen_gaps(
                std::slice::from_ref(&open),
                SignalSet::from_signal(Signal::Logs),
                window,
                &seen
            )
            .is_empty()
        );

        let mut repaired = open;
        repaired.revision = repaired.revision.repaired(
            "retained-stage",
            Timestamp::from_unix_millis(40).expect("instant"),
        );
        assert_eq!(
            unseen_gaps(
                &[repaired],
                SignalSet::from_signal(Signal::Logs),
                window,
                &seen
            )
            .len(),
            1,
            "the repaired revision changes completeness and must be surfaced"
        );
    }
}
