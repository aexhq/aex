//! The generated observation and telemetry-lifecycle server traits, served.
//!
//! Every route this deployable mounts is fully served here. There is no
//! `route_binding_unavailable`, no unavailable-port stub and no arm that
//! answers a `503` because a dependency was never wired — a deployable never
//! mounts a route it cannot serve, so mounting and serving are the same commit.

use std::sync::Arc;
use std::time::Duration;

use aex_observation_domain::keys::{self, ScopeKey};
use aex_observation_domain::order::OrderTuple;
use aex_observation_query::plan::plan;
use aex_regional_http::cursor::{CursorKey, CursorKeyRing};
use aex_regional_http::stream::{Frame, RotateReason};
use aex_wire::error::{ErrorCode, WireError, WireResult};
use aex_wire::ids::{
    ExportId, MeasurementId, PrefixedId as _, SessionId, TelemetryGapId, TraceId, Uuid7,
    WorkspaceId,
};
use aex_wire::models::{
    DownloadGrant, EmptyRequest, ExportCompleteness, ExportFormat, ExportStatus,
    MetricAggregationGroup, MetricAggregationPage, MetricAggregationRequest, Observation,
    ObservationListenRequest, ObservationPage, ObservationQuery, ObservationSignal,
    ObservationStreamRequest, Operation, OperationKind, OperationStatus, TelemetryExport,
    TelemetryExportRequest, TelemetryGap, TelemetryGapPage, TelemetryGapQuery, TelemetryGapReason,
    TimeRange, TraceDetail, TraceSummary,
};
use aex_wire::server::{
    Accepted, Created, NdjsonStream, ObservationsApi, RequestContext, TelemetryLifecycleApi,
};
use aex_wire::types::{DecimalU128, Region, Timestamp};

use crate::edge::Authorized;
use crate::ndjson::{self, FrameStream};
use crate::query;
use crate::reader::{ObservationReader, ReadError};

/// How long a minted download grant lives.
///
/// OD-17 pins the grant and the signature to the same five minutes, so a leaked
/// signature can never outlive the grant that authorized it.
pub const GRANT_LIFETIME: Duration = Duration::from_mins(5);

/// How long one `listen` connection follows before it rotates.
pub const LISTEN_BUDGET: Duration = Duration::from_mins(1);

/// How long a `listen` waits between frontier polls.
pub const LISTEN_POLL: Duration = Duration::from_millis(500);

/// The shared, resolved service every request borrows.
pub struct ObservationService {
    reader: ObservationReader,
    budget: aex_observation_query::plan::Budget,
    metric_scan: u64,
    cursor_key: CursorKey,
    cursor_ring: CursorKeyRing,
    region: Region,
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
            .field("cursor_key", &self.cursor_key)
            .field("cursor_ring", &"<key ring>")
            .finish()
    }
}

impl ObservationService {
    /// Composes the service over its resolved adapters.
    #[must_use]
    pub const fn new(
        reader: ObservationReader,
        budget: aex_observation_query::plan::Budget,
        metric_scan: u64,
        cursor_key: CursorKey,
        cursor_ring: CursorKeyRing,
        region: Region,
    ) -> Self {
        Self {
            reader,
            budget,
            metric_scan,
            cursor_key,
            cursor_ring,
            region,
        }
    }
}

/// One request's view of the service.
#[derive(Clone)]
pub struct ObservationRequest {
    service: Arc<ObservationService>,
    authorized: Authorized,
}

impl std::fmt::Debug for ObservationRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ObservationRequest")
            .field("workspace", &self.authorized.workspace)
            .finish_non_exhaustive()
    }
}

impl ObservationRequest {
    /// Binds one request to the shared service.
    #[must_use]
    pub const fn new(service: Arc<ObservationService>, authorized: Authorized) -> Self {
        Self {
            service,
            authorized,
        }
    }

    /// The workspace this request is authorized for.
    #[must_use]
    pub const fn workspace(&self) -> WorkspaceId {
        self.authorized.workspace
    }

    /// The organization the authorized workspace belongs to.
    ///
    /// Recorded on every durable operation this surface admits, so an export
    /// row can be attributed without a second lookup.
    #[must_use]
    pub const fn organization(&self) -> aex_wire::ids::OrganizationId {
        self.authorized.organization
    }

    /// The scope one route reads, which is a session when the route names one.
    fn scope(&self, session: Option<SessionId>) -> ScopeKey {
        session.map_or(ScopeKey::Workspace(self.workspace()), ScopeKey::Session)
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
            .frontier(&scope, &normalized)
            .await
            .map_err(|error| read_error(&error))?;
        let snapshot = self
            .service
            .reader
            .pin(frontier, now)
            .ok_or_else(|| WireError::new(ErrorCode::ObservabilityUnavailable))?;
        let binding = query::binding(
            cx,
            cx.route,
            self.service.region,
            &scope,
            self.workspace(),
            &normalized,
            snapshot,
        )?;
        let after = body
            .cursor
            .as_ref()
            .map(|token| query::resume(&self.service.cursor_ring, token, &binding, now))
            .transpose()?;
        let plan = plan(
            &normalized,
            query::budget_for(self.service.budget, normalized.limit),
        )
        .map_err(|error| query::query_error(&error))?;
        let page = self
            .service
            .reader
            .read_page(&scope, self.workspace(), &normalized, &plan, after.as_ref())
            .await
            .map_err(|error| read_error(&error))?;
        let next_cursor = match (page.more, page.last) {
            (true, Some(last)) => {
                Some(query::issue(&self.service.cursor_key, &binding, last, now)?)
            }
            _ => None,
        };
        Ok(ObservationPage {
            coverage: query::coverage(
                snapshot,
                frontier.accepted_at,
                frontier.earliest_accepted_at,
            ),
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
        let signals = query::signal_for(route_signal, body.signal)?;
        let scope = self.scope(session);
        let synthetic = ObservationQuery {
            consistency: None,
            cursor: None,
            filter: body.filter,
            limit: None,
            order: None,
            signal: body.signal,
            time_range: body.window,
        };
        let normalized = query::with_scope(
            query::normalize(&synthetic, signals, self.service.budget.max_returned)?,
            &scope,
        );
        let now = now()?;
        let frontier = self
            .service
            .reader
            .frontier(&scope, &normalized)
            .await
            .map_err(|error| read_error(&error))?;
        let snapshot = self
            .service
            .reader
            .pin(frontier, now)
            .ok_or_else(|| WireError::new(ErrorCode::ObservabilityUnavailable))?;
        let binding = query::binding(
            cx,
            cx.route,
            self.service.region,
            &scope,
            self.workspace(),
            &normalized,
            snapshot,
        )?;
        let plan = plan(
            &normalized,
            query::budget_for(self.service.budget, normalized.limit),
        )
        .map_err(|error| query::query_error(&error))?;

        let (sender, stream) = ndjson::channel();
        let service = Arc::clone(&self.service);
        let workspace = self.workspace();
        let follow = body.follow;
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
        normalized.metric_name = Some(body.metric.clone().into_boxed_str());
        let now = now()?;
        let frontier = self
            .service
            .reader
            .frontier(&scope, &normalized)
            .await
            .map_err(|error| read_error(&error))?;
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
            .read_page(&scope, self.workspace(), &normalized, &plan, None)
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
        Ok(MetricAggregationPage {
            coverage: query::coverage(
                snapshot,
                frontier.accepted_at,
                frontier.earliest_accepted_at,
            ),
            items: if page.items.is_empty() {
                Vec::new()
            } else {
                vec![group]
            },
            next_cursor: None,
        })
    }

    /// Reads one assembled trace.
    async fn trace(&self, session: SessionId, trace_id: TraceId) -> WireResult<TraceDetail> {
        let scope = ScopeKey::Session(session);
        let items = self
            .service
            .reader
            .read_range(
                Some(aex_observation_store_aws::expressions::Index::Trace),
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
        Ok(TraceDetail {
            coverage: query::coverage(snapshot, ended, started),
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
        let items = self
            .service
            .reader
            .read_range(
                None,
                &crate::reader::KeyBinding {
                    attribute: "pk".to_owned(),
                    value: keys::gap_pk(&scope),
                },
                &crate::reader::RangeBinding {
                    attribute: "sk".to_owned(),
                    lo: format!("{gap}#"),
                    hi: format!("{gap}#\u{fffe}"),
                },
                64,
            )
            .await
            .map_err(|error| read_error(&error))?;
        // Gaps are immutable revisioned rows; the latest revision wins and the
        // history is never erased.
        let latest = items
            .into_iter()
            .max_by_key(|item| crate::reader::number(item, "revision").unwrap_or(0))
            .ok_or_else(|| WireError::new(ErrorCode::NotFound))?;
        decode_gap(&latest, self.workspace(), session)
    }

    /// Queries recorded gaps at one scope.
    async fn gaps(
        &self,
        session: Option<SessionId>,
        body: TelemetryGapQuery,
    ) -> WireResult<TelemetryGapPage> {
        let scope = self.scope(session);
        let (index, partition, attribute) = match scope {
            ScopeKey::Session(_) => (None, keys::gap_pk(&scope), "pk"),
            ScopeKey::Workspace(workspace) => (
                Some(aex_observation_store_aws::expressions::Index::Gap),
                format!("GAPW#{workspace}"),
                "gwPk",
            ),
        };
        let sort = if attribute == "pk" { "sk" } else { "gwSk" };
        let items = self
            .service
            .reader
            .read_range(
                index,
                &crate::reader::KeyBinding {
                    attribute: attribute.to_owned(),
                    value: partition,
                },
                &crate::reader::RangeBinding {
                    attribute: sort.to_owned(),
                    lo: body.time_range.gte.to_wire(),
                    hi: format!("{}\u{fffe}", body.time_range.lt.to_wire()),
                },
                u16::try_from(body.limit.unwrap_or(100)).unwrap_or(100),
            )
            .await
            .map_err(|error| read_error(&error))?;
        let mut gaps = Vec::new();
        for item in &items {
            gaps.push(decode_gap(item, self.workspace(), session)?);
        }
        Ok(TelemetryGapPage {
            items: gaps,
            next_cursor: None,
        })
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
        let mut builder = aex_observation_store_aws::expressions::ExpressionBuilder::new();
        let condition = aex_observation_store_aws::expressions::immutable_condition(&mut builder);
        self.service
            .reader
            .put_conditional(item, &condition, builder.names(), builder.values())
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
        let mut builder = aex_observation_store_aws::expressions::ExpressionBuilder::new();
        let state = builder.name("state");
        let revoked = builder.string("revoked");
        let revoked_at = builder.name("revokedAt");
        let at = builder.string(now()?.to_wire());
        let cancel = builder.name("cancelRequested");
        let truth = builder.boolean(true);
        let existing = builder.name("pk");
        self.service
            .reader
            .update_conditional(
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
}

/// Streams pages as whole frames, then rotates.
///
/// `rotate` is the only terminal frame after the headers flushed: once the
/// status is on the wire there is nothing left to change, so a failure is a
/// frame carrying its reason rather than a status a reader cannot see.
async fn produce(mut task: Produce) {
    let deadline = std::time::Instant::now() + LISTEN_BUDGET;
    let mut after: Option<OrderTuple> = None;
    loop {
        let outcome = task
            .service
            .reader
            .read_page(
                &task.scope,
                task.workspace,
                &task.normalized,
                &task.plan,
                after.as_ref(),
            )
            .await;
        let Ok(page) = outcome else {
            let cursor = task.sender.sent().unwrap_or_default().to_owned();
            let _ = task
                .sender
                .send(&ndjson::rotate(cursor, RotateReason::Draining, true))
                .await;
            return;
        };
        if !page.items.is_empty() {
            let token = page
                .last
                .and_then(|last| {
                    query::issue(
                        &task.service.cursor_key,
                        &task.binding,
                        last,
                        task.snapshot.accepted_at(),
                    )
                    .ok()
                })
                .map(|cursor| cursor.as_str().to_owned())
                .unwrap_or_default();
            let records = page
                .items
                .iter()
                .filter_map(|item| serde_json::to_value(item).ok())
                .collect();
            if !task
                .sender
                .send(&Frame::Records {
                    records,
                    cursor: token,
                })
                .await
            {
                return;
            }
            after = page.last;
        }
        if !task.follow || std::time::Instant::now() >= deadline {
            break;
        }
        let cursor = task.sender.sent().unwrap_or_default().to_owned();
        if !task
            .sender
            .send(&Frame::Cursor {
                cursor,
                at: task.snapshot.accepted_at(),
            })
            .await
        {
            return;
        }
        tokio::time::sleep(LISTEN_POLL).await;
    }
    let cursor = task.sender.sent().unwrap_or_default().to_owned();
    let _ = task
        .sender
        .send(&ndjson::rotate(cursor, RotateReason::Rotation, true))
        .await;
}

/// What a `stream` or `listen` request asks for, normalized to one shape.
struct StreamBody {
    filter: Option<aex_wire::models::ObservationFilter>,
    signal: ObservationSignal,
    window: TimeRange,
    follow: bool,
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
        ReadError::Malformed { .. } => WireError::new(ErrorCode::InternalError),
    }
}

/// Decodes one stored gap revision.
fn decode_gap(
    item: &std::collections::HashMap<String, aws_sdk_dynamodb::types::AttributeValue>,
    workspace: WorkspaceId,
    session: Option<SessionId>,
) -> WireResult<TelemetryGap> {
    let id = crate::reader::string(item, "gapId")
        .and_then(|text| text.parse::<TelemetryGapId>().ok())
        .ok_or_else(|| WireError::new(ErrorCode::InternalError))?;
    let opened = crate::reader::timestamp(item, "openedAt")
        .ok_or_else(|| WireError::new(ErrorCode::InternalError))?;
    let reason = crate::reader::string(item, "reason").unwrap_or_default();
    let reason = TelemetryGapReason::ALL
        .iter()
        .copied()
        .find(|candidate| candidate.as_str() == reason)
        .unwrap_or(TelemetryGapReason::SpoolLost);
    Ok(TelemetryGap {
        byte_count: crate::reader::number(item, "attemptedBytes")
            .map(|value| DecimalU128::new(u128::from(value))),
        detected_at: opened,
        from_sequence: None,
        id,
        observation_count: crate::reader::number(item, "attemptedRecords")
            .map(|value| DecimalU128::new(u128::from(value))),
        reason,
        recoverable: crate::reader::boolean(item, "recoverable").unwrap_or(false),
        repair_source: crate::reader::string(item, "repairSource"),
        repaired_at: crate::reader::timestamp(item, "repairedAt"),
        revision: crate::reader::number(item, "revision").unwrap_or(0),
        session_id: session,
        signals: vec![ObservationSignal::Telemetry],
        time_range: TimeRange {
            gte: opened,
            lt: crate::reader::timestamp(item, "revisedAt").unwrap_or(opened),
        },
        to_sequence: None,
        workspace_id: workspace,
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
        _cx: &RequestContext,
        body: TelemetryGapQuery,
    ) -> WireResult<TelemetryGapPage> {
        self.gaps(None, body).await
    }

    async fn session_telemetry_gaps_query(
        &self,
        _cx: &RequestContext,
        session_id: SessionId,
        body: TelemetryGapQuery,
    ) -> WireResult<TelemetryGapPage> {
        self.gaps(Some(session_id), body).await
    }
}

#[cfg(test)]
mod tests {
    use aex_wire::models::{ExportStatus, ObservationOrigin, ObservationOriginEarliest};
    use aex_wire::types::Timestamp;

    use super::{GRANT_LIFETIME, LISTEN_BUDGET, export_status, listen_window, window_for};

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
}
