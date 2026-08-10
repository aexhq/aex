//! Normalization, coverage and cursor binding for one public query.
//!
//! `aex-observation-query` owns the field policy, the total planner, the budget
//! classifier and the coverage algebra. This module is the boundary that turns
//! one wire request into the normalized form those functions take, and the one
//! place a cursor is minted, so there is exactly one codec (RS-17) and exactly
//! one field policy.

use aex_observation_domain::gap::{GapRecord, TimeWindow};
use aex_observation_domain::keys::ScopeKey;
use aex_observation_domain::order::{Direction, OrderBy, OrderTuple};
use aex_observation_domain::signal::{Signal, SignalSet};
use aex_observation_query::ObservationResume;
use aex_observation_query::ast::{self, CmpOp, Predicate, QueryError, TextOp};
use aex_observation_query::coverage::{Coverage, Snapshot};
use aex_observation_query::plan::{Budget, NormalizedQuery};
use aex_regional_http::cursor::{
    CursorBinding, CursorKey, CursorKeyRing, CursorRequestBinding, Order, SnapshotToken, SortTuple,
};
use aex_wire::canonical::to_jcs_bytes;
use aex_wire::cursor::Cursor;
use aex_wire::error::{ErrorCode, ErrorDetails, WireError, WireResult};
use aex_wire::ids::TelemetryGapId;
use aex_wire::models::{
    MissingInterval, ObservationCoverage, ObservationFilter, ObservationOperator, ObservationOrder,
    ObservationQuery, ObservationSignal, TimeRange,
};
use aex_wire::routes::RouteId;
use aex_wire::server::RequestContext;
use aex_wire::types::{DecimalU128, Region, Timestamp};

/// Resolves the signal the route names, which is the authority.
///
/// A request whose body names a different signal than its route is a
/// contradiction, not a preference: the route is the contract.
///
/// `telemetry` is the one wire signal that is not a stored signal: it selects
/// every signal, interleaved on the one ordering tuple.
///
/// # Errors
///
/// Returns `invalid_query` naming both spellings when the two disagree.
pub fn signal_for(
    route_signal: ObservationSignal,
    body: ObservationSignal,
) -> WireResult<SignalSet> {
    if route_signal != body {
        return Err(invalid_query(format!(
            "this route serves `{}`; the body names `{}`",
            route_signal.as_str(),
            body.as_str()
        )));
    }
    Ok(match route_signal {
        ObservationSignal::Telemetry => SignalSet::all(),
        ObservationSignal::Events => SignalSet::from_signal(Signal::Events),
        ObservationSignal::Logs => SignalSet::from_signal(Signal::Logs),
        ObservationSignal::Spans => SignalSet::from_signal(Signal::Spans),
        ObservationSignal::Metrics => SignalSet::from_signal(Signal::Metrics),
        ObservationSignal::Traces => SignalSet::from_signal(Signal::Traces),
    })
}

/// Normalizes one wire query against the closed field policy.
///
/// # Errors
///
/// Returns `invalid_query` naming the offending field for anything outside the
/// closed per-signal sets, including protected identity fields.
pub fn normalize(
    body: &ObservationQuery,
    signals: SignalSet,
    limit_ceiling: u16,
) -> WireResult<NormalizedQuery> {
    let selected: Vec<Signal> = signals.iter().collect();
    let predicate = body
        .filter
        .as_ref()
        .map(|filter| to_predicate(filter, &selected))
        .transpose()
        .map_err(|error| query_error(&error))?;
    let limit = body
        .limit
        .and_then(|value| u16::try_from(value).ok())
        .unwrap_or(aex_observation_domain::limits::QUERY_DEFAULT_LIMIT)
        .clamp(1, limit_ceiling);
    let query = NormalizedQuery {
        axis: aex_observation_query::plan::ScopeAxis::Workspace,
        signals,
        predicate,
        time_gte: body.time_range.gte,
        time_lt: body.time_range.lt,
        order_by: OrderBy::Time,
        direction: match body.order {
            Some(ObservationOrder::Descending) => Direction::Descending,
            _ => Direction::Ascending,
        },
        limit,
        trace_id: None,
        metric_name: None,
    };
    query.validate().map_err(|error| query_error(&error))?;
    Ok(query)
}

/// Binds a normalized query to one scope axis.
#[must_use]
pub fn with_scope(mut query: NormalizedQuery, scope: &ScopeKey) -> NormalizedQuery {
    query.axis = match scope {
        ScopeKey::Session { .. } => aex_observation_query::plan::ScopeAxis::Scope,
        ScopeKey::Workspace(_) => aex_observation_query::plan::ScopeAxis::Workspace,
    };
    query
}

/// Converts one bounded wire filter into the resolved predicate.
///
/// Structural bounds — depth, leaves, boolean children, `in` values, operand
/// length — are already enforced by the generated `Deserialize`. This is the
/// **field policy only**, never a second structural check.
///
/// # Errors
///
/// Returns [`QueryError`] naming the offending field or operator.
pub fn to_predicate(
    filter: &ObservationFilter,
    signals: &[Signal],
) -> Result<Predicate, QueryError> {
    match filter {
        ObservationFilter::All(all) => Ok(Predicate::And(children(&all.filters, signals)?)),
        ObservationFilter::Any(any) => Ok(Predicate::Or(children(&any.filters, signals)?)),
        ObservationFilter::Not(not) => {
            let mut resolved = children(&not.filters, signals)?;
            let single = if resolved.len() == 1 {
                resolved.remove(0)
            } else {
                Predicate::And(resolved)
            };
            Ok(Predicate::Not(Box::new(single)))
        }
        ObservationFilter::Compare(compare) => {
            let field = ast::resolve(&compare.field, signals)?;
            let value = operand(&compare.value);
            Ok(match compare.operator {
                ObservationOperator::Prefix => Predicate::Text {
                    field,
                    op: TextOp::Prefix,
                    value: compare.value.clone().into_boxed_str(),
                },
                other => Predicate::Cmp {
                    field,
                    op: compare_op(other),
                    value,
                },
            })
        }
        ObservationFilter::In(membership) => Ok(Predicate::In {
            field: ast::resolve(&membership.field, signals)?,
            negated: false,
            values: membership.values.iter().map(|text| operand(text)).collect(),
        }),
        ObservationFilter::Exists(exists) => Ok(Predicate::Exists {
            field: ast::resolve(&exists.field, signals)?,
        }),
    }
}

/// Resolves every child of a boolean node.
fn children(
    filters: &[ObservationFilter],
    signals: &[Signal],
) -> Result<Vec<Predicate>, QueryError> {
    filters
        .iter()
        .map(|filter| to_predicate(filter, signals))
        .collect()
}

/// Maps a wire operator onto the resolved comparison.
///
/// `prefix` is handled by the caller, because it is a text operator rather than
/// a comparison and carries a different operand type.
const fn compare_op(operator: ObservationOperator) -> CmpOp {
    match operator {
        ObservationOperator::Eq | ObservationOperator::Prefix => CmpOp::Eq,
        ObservationOperator::Ne => CmpOp::Ne,
        ObservationOperator::Lt => CmpOp::Lt,
        ObservationOperator::Lte => CmpOp::Lte,
        ObservationOperator::Gt => CmpOp::Gt,
        ObservationOperator::Gte => CmpOp::Gte,
    }
}

/// The operand a leaf compares against.
///
/// The wire carries every operand as a string, so a numeric or boolean spelling
/// is recovered here rather than forcing every stored number to be compared as
/// text — which would make `severityNumber > 9` false for `10`.
fn operand(text: &str) -> aex_observation_domain::canonical::CanonicalValue {
    use aex_observation_domain::canonical::CanonicalValue;

    if let Ok(value) = text.parse::<i64>() {
        return CanonicalValue::Int(value);
    }
    if let Ok(value) = text.parse::<f64>()
        && let Ok(canonical) = CanonicalValue::number(value)
    {
        return canonical;
    }
    match text {
        "true" => CanonicalValue::Bool(true),
        "false" => CanonicalValue::Bool(false),
        "null" => CanonicalValue::Null,
        other => CanonicalValue::Str(other.into()),
    }
}

/// Builds the public coverage of one answer.
///
/// # Errors
///
/// Returns a public query error when more than 100 relevant bounded or
/// unbounded gaps would have to be represented in one answer.
pub fn coverage(
    snapshot: Snapshot,
    accepted: Timestamp,
    earliest: Timestamp,
    signals: SignalSet,
    window: TimeWindow,
    gaps: &[GapRecord],
) -> WireResult<ObservationCoverage> {
    let mut missing = Vec::new();
    let mut unbounded = Vec::new();
    for gap in gaps {
        if !gap.revision.affects(signals, window) {
            continue;
        }
        if let Some(range) = gap.revision.time_range {
            let from = range.from().max(window.from());
            let to = range.to().min(window.to());
            if from < to {
                missing.push((gap.revision.gap_id.to_string(), from, to));
            }
        } else {
            unbounded.push(gap.revision.gap_id.to_string());
        }
    }
    if missing.len() > 100 || unbounded.len() > 100 {
        return Err(budget_exhausted(
            "gap_coverage",
            u32::try_from(missing.len().saturating_add(unbounded.len())).unwrap_or(u32::MAX),
            100,
        ));
    }
    let resolved: Coverage =
        Coverage::new(snapshot, accepted, earliest).with_gaps(missing.clone(), unbounded.clone());
    Ok(ObservationCoverage {
        accepted: resolved.accepted,
        caught_up: resolved.caught_up,
        complete: resolved.complete,
        earliest_replay: resolved.earliest_replay,
        indexed: resolved.indexed,
        missing_intervals: missing
            .into_iter()
            .map(|(gap_id, gte, lt)| {
                Ok(MissingInterval {
                    gap_id: gap_id
                        .parse()
                        .map_err(|_| WireError::new(ErrorCode::InternalError))?,
                    range: TimeRange { gte, lt },
                })
            })
            .collect::<WireResult<Vec<_>>>()?,
        snapshot: resolved.snapshot,
        unbounded_gaps: unbounded
            .into_iter()
            .map(|gap_id| {
                gap_id
                    .parse()
                    .map_err(|_| WireError::new(ErrorCode::InternalError))
            })
            .collect::<WireResult<Vec<_>>>()?,
    })
}

/// The digest that binds a cursor to the exact query it was issued against.
#[must_use]
pub fn query_digest(query: &NormalizedQuery, scope: &ScopeKey, deletion_epoch: u64) -> [u8; 32] {
    use sha2::Digest as _;

    let mut hasher = sha2::Sha256::new();
    hasher.update(scope.to_key().as_bytes());
    hasher.update(deletion_epoch.to_be_bytes());
    hasher.update([u8::from(query.direction == Direction::Descending)]);
    hasher.update(query.signals.bits().to_be_bytes());
    hasher.update(query.time_gte.unix_millis().to_be_bytes());
    hasher.update(query.time_lt.unix_millis().to_be_bytes());
    if let Some(predicate) = &query.predicate {
        hasher.update(format!("{predicate:?}").as_bytes());
    }
    hasher.finalize().into()
}

/// Digest of the stable selection a stream cursor resumes.
///
/// The original time/earliest origin is replaced by the delivered ordering
/// tuple, so it is intentionally absent. Signal, filter, direction and scope
/// remain bound; changing any of them refuses the cursor.
#[must_use]
pub fn stream_query_digest(
    query: &NormalizedQuery,
    scope: &ScopeKey,
    deletion_epoch: u64,
) -> [u8; 32] {
    use sha2::Digest as _;

    let mut hasher = sha2::Sha256::new();
    hasher.update(scope.to_key().as_bytes());
    hasher.update(deletion_epoch.to_be_bytes());
    hasher.update([u8::from(query.direction == Direction::Descending)]);
    hasher.update(query.signals.bits().to_be_bytes());
    if let Some(predicate) = &query.predicate {
        hasher.update(format!("{predicate:?}").as_bytes());
    }
    hasher.finalize().into()
}

/// Stable request binding for a gap-list continuation.
#[must_use]
pub fn gap_request_binding(
    cx: &RequestContext,
    route: RouteId,
    region: Region,
    scope: &ScopeKey,
    workspace: aex_wire::ids::WorkspaceId,
    query: &aex_wire::models::TelemetryGapQuery,
) -> CursorRequestBinding {
    use sha2::Digest as _;

    let mut hasher = sha2::Sha256::new();
    hasher.update(scope.to_key().as_bytes());
    hasher.update(query.time_range.gte.unix_millis().to_be_bytes());
    hasher.update(query.time_range.lt.unix_millis().to_be_bytes());
    hasher.update([query.recoverable.map_or(2, u8::from)]);
    if let Some(signals) = &query.signals {
        for signal in signals {
            hasher.update(signal.as_str().as_bytes());
            hasher.update([0]);
        }
    }
    CursorRequestBinding {
        route,
        principal_scope: principal_digest(cx),
        region,
        workspace_id: workspace,
        session_id: scope.session(),
        query_hash: hasher.finalize().into(),
        order: Order::Ascending,
    }
}

/// Completes a gap cursor binding at one settled ledger snapshot.
///
/// # Errors
///
/// Returns an internal wire error when the settled snapshot cannot form a
/// canonical cursor token.
pub fn gap_binding(
    request: &CursorRequestBinding,
    snapshot: Snapshot,
) -> WireResult<CursorBinding> {
    Ok(CursorBinding {
        route: request.route,
        principal_scope: request.principal_scope,
        region: request.region,
        workspace_id: request.workspace_id,
        session_id: request.session_id,
        query_hash: request.query_hash,
        order: request.order,
        snapshot: SnapshotToken::new(snapshot.to_wire().to_string())
            .map_err(|_| WireError::new(ErrorCode::InternalError))?,
    })
}

/// Issues a gap-list continuation after one complete gap-id group.
///
/// # Errors
///
/// Returns an internal wire error when the sort tuple or signed cursor cannot
/// be encoded.
pub fn issue_gap(
    key: &CursorKey,
    binding: &CursorBinding,
    opened_at: Timestamp,
    gap_id: TelemetryGapId,
    now: Timestamp,
) -> WireResult<Cursor> {
    let tuple = SortTuple::new(vec![opened_at.to_wire(), gap_id.to_string()])
        .map_err(|_| WireError::new(ErrorCode::InternalError))?;
    aex_regional_http::cursor::encode(key, binding, &tuple, now)
        .map_err(|_| WireError::new(ErrorCode::InternalError))
}

/// Authenticates a gap-list continuation and recovers its ledger snapshot.
///
/// # Errors
///
/// Returns an invalid-cursor error when the token, its binding, snapshot, or
/// tuple cannot be authenticated and decoded.
pub fn resume_gap(
    ring: &CursorKeyRing,
    token: &Cursor,
    request: &CursorRequestBinding,
    now: Timestamp,
) -> WireResult<(Snapshot, Timestamp, TelemetryGapId)> {
    let resumed = aex_regional_http::cursor::decode_resume(ring, token, request, now)
        .map_err(|_| WireError::new(ErrorCode::InvalidCursor))?;
    let millis = resumed
        .snapshot
        .as_str()
        .parse::<u128>()
        .ok()
        .and_then(|value| i64::try_from(value).ok())
        .and_then(|value| Timestamp::from_unix_millis(value).ok())
        .ok_or_else(|| WireError::new(ErrorCode::InvalidCursor))?;
    let [opened_at, gap_id] = resumed.tuple.parts() else {
        return Err(WireError::new(ErrorCode::InvalidCursor));
    };
    Ok((
        Snapshot::at(millis),
        Timestamp::parse(opened_at).map_err(|_| WireError::new(ErrorCode::InvalidCursor))?,
        gap_id
            .parse()
            .map_err(|_| WireError::new(ErrorCode::InvalidCursor))?,
    ))
}

/// The digest of the effective principal scope a cursor is bound to.
#[must_use]
pub fn principal_digest(cx: &RequestContext) -> [u8; 32] {
    use sha2::Digest as _;

    let encoded = to_jcs_bytes(&cx.principal).unwrap_or_default();
    sha2::Sha256::digest(&encoded).into()
}

/// Builds the stable request half of a finite-page cursor binding.
#[must_use]
pub fn page_request_binding(
    cx: &RequestContext,
    route: RouteId,
    region: Region,
    scope: &ScopeKey,
    workspace: aex_wire::ids::WorkspaceId,
    query: &NormalizedQuery,
    deletion_epoch: u64,
) -> CursorRequestBinding {
    CursorRequestBinding {
        route,
        principal_scope: principal_digest(cx),
        region,
        workspace_id: workspace,
        session_id: scope.session(),
        query_hash: query_digest(query, scope, deletion_epoch),
        order: match query.direction {
            Direction::Ascending => Order::Ascending,
            Direction::Descending => Order::Descending,
        },
    }
}

/// Builds the stable request half of an NDJSON reconnect binding.
#[must_use]
pub fn stream_request_binding(
    cx: &RequestContext,
    route: RouteId,
    region: Region,
    scope: &ScopeKey,
    workspace: aex_wire::ids::WorkspaceId,
    query: &NormalizedQuery,
    deletion_epoch: u64,
) -> CursorRequestBinding {
    CursorRequestBinding {
        route: canonical_stream_route(route),
        principal_scope: principal_digest(cx),
        region,
        workspace_id: workspace,
        session_id: scope.session(),
        query_hash: stream_query_digest(query, scope, deletion_epoch),
        order: match query.direction {
            Direction::Ascending => Order::Ascending,
            Direction::Descending => Order::Descending,
        },
    }
}

/// Builds a complete NDJSON cursor binding at one settled snapshot.
///
/// # Errors
///
/// Returns `internal_error` if the bounded snapshot cannot be represented.
pub fn stream_binding(
    request: &CursorRequestBinding,
    snapshot: Snapshot,
) -> WireResult<CursorBinding> {
    Ok(CursorBinding {
        route: request.route,
        principal_scope: request.principal_scope,
        region: request.region,
        workspace_id: request.workspace_id,
        session_id: request.session_id,
        query_hash: request.query_hash,
        order: request.order,
        snapshot: SnapshotToken::new(snapshot.to_wire().to_string())
            .map_err(|_| WireError::new(ErrorCode::InternalError))?,
    })
}

/// Authenticated continuation accepted by a replay stream.
#[derive(Clone, Debug)]
pub enum StreamResume {
    /// Legacy/global checkpoint used once a replay has exhausted every segment
    /// and a follow poll must look for newly accepted rows.
    Tuple(OrderTuple),
    /// Exact open-segment state while a bounded replay is still in progress.
    Segments(ObservationResume),
}

/// Builds a complete finite-page cursor binding at its original snapshot.
///
/// # Errors
///
/// Returns `internal_error` if the bounded snapshot cannot be represented.
pub fn page_binding(
    request: &CursorRequestBinding,
    snapshot: Snapshot,
) -> WireResult<CursorBinding> {
    stream_binding(request, snapshot)
}

/// Authenticates an NDJSON reconnect and recovers its snapshot and last tuple.
///
/// # Errors
///
/// Returns `invalid_cursor` for any signature, binding, lifetime or snapshot
/// representation failure.
pub fn resume_stream(
    ring: &CursorKeyRing,
    token: &Cursor,
    binding: &CursorRequestBinding,
    now: Timestamp,
) -> WireResult<(Snapshot, StreamResume)> {
    if let Ok(resumed) = aex_regional_http::cursor::decode_state_resume::<ObservationResume>(
        ring, token, binding, now,
    ) {
        resumed
            .state
            .validate()
            .map_err(|_| WireError::new(ErrorCode::InvalidCursor))?;
        let snapshot = snapshot_from_token(&resumed.snapshot)?;
        return Ok((
            Snapshot::at(snapshot),
            StreamResume::Segments(resumed.state),
        ));
    }
    let resumed = aex_regional_http::cursor::decode_resume(ring, token, binding, now)
        .map_err(|_| WireError::new(ErrorCode::InvalidCursor))?;
    let millis = snapshot_from_token(&resumed.snapshot)?;
    let tuple = tuple_from_parts(resumed.tuple.parts())?;
    Ok((Snapshot::at(millis), StreamResume::Tuple(tuple)))
}

/// Authenticates a finite-page continuation and recovers its original snapshot.
///
/// # Errors
///
/// Returns `invalid_cursor` for any signature, binding, lifetime or snapshot
/// representation failure.
pub fn resume_page(
    ring: &CursorKeyRing,
    token: &Cursor,
    binding: &CursorRequestBinding,
    now: Timestamp,
) -> WireResult<(Snapshot, ObservationResume)> {
    let resumed = aex_regional_http::cursor::decode_state_resume::<ObservationResume>(
        ring, token, binding, now,
    )
    .map_err(|_| WireError::new(ErrorCode::InvalidCursor))?;
    resumed
        .state
        .validate()
        .map_err(|_| WireError::new(ErrorCode::InvalidCursor))?;
    let snapshot = snapshot_from_token(&resumed.snapshot)?;
    Ok((Snapshot::at(snapshot), resumed.state))
}

fn snapshot_from_token(token: &SnapshotToken) -> WireResult<Timestamp> {
    token
        .as_str()
        .parse::<u128>()
        .ok()
        .and_then(|value| i64::try_from(value).ok())
        .and_then(|value| Timestamp::from_unix_millis(value).ok())
        .ok_or_else(|| WireError::new(ErrorCode::InvalidCursor))
}

fn canonical_stream_route(route: RouteId) -> RouteId {
    let operation = aex_wire::routes::route(route).operation_id;
    let Some(prefix) = operation.strip_suffix("_listen") else {
        return route;
    };
    let stream = format!("{prefix}_stream");
    RouteId::ALL
        .iter()
        .copied()
        .find(|candidate| aex_wire::routes::route(*candidate).operation_id == stream)
        .unwrap_or(route)
}

/// Mints the continuation for one page.
///
/// # Errors
///
/// Returns `internal_error` when the tuple cannot be encoded.
pub fn issue(
    key: &CursorKey,
    binding: &CursorBinding,
    last: OrderTuple,
    now: Timestamp,
) -> WireResult<Cursor> {
    let tuple = SortTuple::new(vec![
        last.primary.to_wire(),
        last.signal_rank.to_string(),
        last.observation_id.to_string(),
        last.revision.to_string(),
    ])
    .map_err(|_| WireError::new(ErrorCode::InternalError))?;
    aex_regional_http::cursor::encode(key, binding, &tuple, now)
        .map_err(|_| WireError::new(ErrorCode::InternalError))
}

/// Mints a finite-page continuation carrying exact per-segment progress.
///
/// # Errors
///
/// Returns `internal_error` when the bounded state cannot be encoded inside the
/// public cursor ceiling.
pub fn issue_page(
    key: &CursorKey,
    binding: &CursorBinding,
    resume: &ObservationResume,
    now: Timestamp,
) -> WireResult<Cursor> {
    resume
        .validate()
        .map_err(|_| WireError::new(ErrorCode::InternalError))?;
    aex_regional_http::cursor::encode_state(key, binding, resume, now)
        .map_err(|_| WireError::new(ErrorCode::InternalError))
}

fn tuple_from_parts(parts: &[String]) -> WireResult<OrderTuple> {
    let [primary, rank, id, revision] = parts else {
        return Err(WireError::new(ErrorCode::InvalidCursor));
    };
    let primary =
        Timestamp::parse(primary).map_err(|_| WireError::new(ErrorCode::InvalidCursor))?;
    let rank: u8 = rank
        .parse()
        .map_err(|_| WireError::new(ErrorCode::InvalidCursor))?;
    let observation_id = id
        .parse::<aex_wire::ids::ObservationId>()
        .map_err(|_| WireError::new(ErrorCode::InvalidCursor))?;
    let revision: u64 = revision
        .parse()
        .map_err(|_| WireError::new(ErrorCode::InvalidCursor))?;
    let signal = Signal::ALL
        .iter()
        .copied()
        .find(|candidate| candidate.rank() == rank)
        .ok_or_else(|| WireError::new(ErrorCode::InvalidCursor))?;
    Ok(OrderTuple::new(primary, signal, observation_id, revision))
}

/// The typed refusal a page that cannot progress returns.
///
/// This is the one budget outcome that is an error. It carries the measured
/// scanned count, the limiting dimension and the remedy, so a caller can act on
/// it rather than guess.
#[must_use]
pub fn budget_exhausted(dimension: &str, scanned: u32, limit: u32) -> WireError {
    WireError::new(ErrorCode::TelemetryQueryBudgetExhausted)
        .with_message(format!(
            "{dimension} exhausted after {scanned} items; narrow the time range, add an indexed \
             predicate, or use an export"
        ))
        .with_details(ErrorDetails::Limit(aex_wire::models::ErrorDetailsLimit {
            effective: DecimalU128::new(u128::from(limit)),
            limit: aex_wire::limits::LimitId::QueryPage,
            measured: DecimalU128::new(u128::from(scanned)),
        }))
}

/// Builds an `invalid_query` refusal naming what was wrong.
#[must_use]
pub fn invalid_query(reason: impl Into<String>) -> WireError {
    WireError::new(ErrorCode::InvalidQuery).with_message(reason)
}

/// Maps a field-policy failure onto the public vocabulary, naming the field.
#[must_use]
pub fn query_error(error: &QueryError) -> WireError {
    invalid_query(error.to_string())
}

/// Builds the budget one page reads under.
#[must_use]
pub const fn budget_for(configured: Budget, limit: u16) -> Budget {
    Budget {
        max_returned: limit,
        ..configured
    }
}

#[cfg(test)]
mod tests {
    use aex_observation_domain::gap::{GapRecord, GapRevision, TimeWindow};
    use aex_observation_domain::keys::BucketHour;
    use aex_observation_domain::keys::ScopeKey;
    use aex_observation_domain::order::{Direction, OrderBy, OrderTuple};
    use aex_observation_domain::signal::{Signal, SignalSet};
    use aex_observation_query::ast::Predicate;
    use aex_observation_query::coverage::Snapshot;
    use aex_observation_query::plan::{Access, NormalizedQuery, ScopeAxis};
    use aex_observation_query::{ObservationResume, ResumeKey, SegmentResume, SegmentState};
    use aex_regional_http::cursor::{
        CursorBinding, CursorKey, CursorKeyRing, CursorRequestBinding, Order, SnapshotToken,
    };
    use aex_wire::error::ErrorCode;
    use aex_wire::ids::{ObservationId, PrefixedId as _, TelemetryGapId, WorkspaceId};
    use aex_wire::models::{
        ObservationFilter, ObservationFilterCompare, ObservationFilterExists, ObservationOperator,
        ObservationSignal, TelemetryGapReason,
    };
    use aex_wire::routes::RouteId;
    use aex_wire::types::{Region, Timestamp};

    use super::{
        budget_exhausted, coverage, issue_page, operand, query_digest, resume_page, signal_for,
        to_predicate,
    };

    #[test]
    fn a_zero_progress_budget_failure_uses_its_exact_non_retryable_409() {
        let error = budget_exhausted("scanned_items", 50_000, 50_000);
        assert_eq!(error.code, ErrorCode::TelemetryQueryBudgetExhausted);
        assert_eq!(error.code.http_status(), 409);
        assert!(!error.code.retryable());
        assert!(
            error
                .message
                .as_deref()
                .is_some_and(|message| message.contains("scanned_items"))
        );
    }

    fn instant(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("fixture instant")
    }

    fn gap(id: &str, signal: Signal, range: Option<TimeWindow>, repaired: bool) -> GapRecord {
        let workspace =
            WorkspaceId::parse("wsp_0000000001e40r2081040g2081").expect("workspace fixture");
        let mut revision = GapRevision::open(
            TelemetryGapId::parse(id).expect("gap fixture"),
            SignalSet::from_signal(signal),
            TelemetryGapReason::SpoolLost,
            range,
            instant(1),
        );
        if repaired {
            revision = revision.repaired("retained-stage", instant(2));
        }
        GapRecord::try_new(
            workspace,
            ScopeKey::Workspace(workspace),
            revision,
            None,
            None,
            false,
        )
        .expect("gap record")
    }

    #[test]
    fn a_deletion_epoch_advance_changes_the_authenticated_query_binding() {
        let workspace = WorkspaceId::from_uuid7(aex_wire::Uuid7::compose(1, [4; 10]));
        let scope = ScopeKey::Workspace(workspace);
        let query = NormalizedQuery {
            axis: ScopeAxis::Workspace,
            signals: SignalSet::from_signal(Signal::Logs),
            predicate: None,
            time_gte: instant(1),
            time_lt: instant(2),
            order_by: OrderBy::Time,
            direction: Direction::Ascending,
            limit: 100,
            trace_id: None,
            metric_name: None,
        };

        assert_ne!(
            query_digest(&query, &scope, 3),
            query_digest(&query, &scope, 4)
        );
    }

    #[test]
    fn the_route_signal_is_the_authority_over_the_body() {
        let set = signal_for(ObservationSignal::Logs, ObservationSignal::Logs).expect("agreement");
        assert!(set.contains(Signal::Logs));
        let error = signal_for(ObservationSignal::Logs, ObservationSignal::Metrics)
            .expect_err("a body that names another signal is a contradiction");
        assert!(error.to_string().contains("logs"), "{error}");
        assert!(error.to_string().contains("metrics"), "{error}");
    }

    #[test]
    fn telemetry_selects_every_signal() {
        let set = signal_for(ObservationSignal::Telemetry, ObservationSignal::Telemetry)
            .expect("agreement");
        assert_eq!(set, SignalSet::all());
    }

    #[test]
    fn coverage_reports_exact_open_holes_and_ignores_repaired_or_other_signals() {
        let window = TimeWindow::new(instant(10), instant(20)).expect("query window");
        let known = gap(
            "gap_0000000001e40r2081040g2081",
            Signal::Logs,
            TimeWindow::new(instant(5), instant(15)),
            false,
        );
        let unbounded = gap("gap_0000000001e40r2081040g2082", Signal::Logs, None, false);
        let repaired = gap(
            "gap_0000000001e40r2081040g2083",
            Signal::Logs,
            TimeWindow::new(instant(10), instant(12)),
            true,
        );
        let metric = gap(
            "gap_0000000001e40r2081040g2084",
            Signal::Metrics,
            None,
            false,
        );
        let answer = coverage(
            Snapshot::at(instant(30)),
            instant(30),
            instant(0),
            SignalSet::from_signal(Signal::Logs),
            window,
            &[known, unbounded, repaired, metric],
        )
        .expect("coverage");
        assert!(!answer.complete);
        assert_eq!(answer.missing_intervals.len(), 1);
        assert_eq!(answer.missing_intervals[0].range.gte, instant(10));
        assert_eq!(answer.missing_intervals[0].range.lt, instant(15));
        assert_eq!(answer.unbounded_gaps.len(), 1);
    }

    #[test]
    fn a_protected_identity_field_is_refused_by_name() {
        let filter = ObservationFilter::Compare(ObservationFilterCompare {
            field: "workspaceId".to_owned(),
            operator: ObservationOperator::Eq,
            value: "ws_x".to_owned(),
        });
        let error = to_predicate(&filter, &[Signal::Logs]).expect_err("protected");
        assert!(error.to_string().contains("workspaceId"), "{error}");
    }

    #[test]
    fn an_unknown_field_is_refused_by_name() {
        let filter = ObservationFilter::Exists(ObservationFilterExists {
            field: "notAField".to_owned(),
        });
        let error = to_predicate(&filter, &[Signal::Logs]).expect_err("unknown");
        assert!(error.to_string().contains("notAField"), "{error}");
    }

    #[test]
    fn an_attribute_path_resolves_as_residual() {
        let filter = ObservationFilter::Exists(ObservationFilterExists {
            field: "attributes.tenant".to_owned(),
        });
        let predicate = to_predicate(&filter, &[Signal::Logs]).expect("attributes are admissible");
        assert!(
            predicate.has_residual(),
            "an attribute predicate needs the base item"
        );
    }

    #[test]
    fn a_numeric_operand_compares_as_a_number_not_as_text() {
        use aex_observation_domain::canonical::CanonicalValue;
        assert_eq!(operand("10"), CanonicalValue::Int(10));
        assert_eq!(operand("true"), CanonicalValue::Bool(true));
        assert_eq!(operand("null"), CanonicalValue::Null);
        assert!(matches!(operand("warn"), CanonicalValue::Str(_)));
    }

    #[test]
    fn a_prefix_comparison_becomes_a_text_predicate() {
        let filter = ObservationFilter::Compare(ObservationFilterCompare {
            field: "body".to_owned(),
            operator: ObservationOperator::Prefix,
            value: "GET /".to_owned(),
        });
        let predicate = to_predicate(&filter, &[Signal::Logs]).expect("prefix is admissible");
        assert!(matches!(predicate, Predicate::Text { .. }));
    }

    #[test]
    fn a_finite_resume_recovers_the_authenticated_original_snapshot() {
        let key = CursorKey::new("current", vec![7; 32]).expect("strong cursor key");
        let request = CursorRequestBinding {
            route: RouteId::ALL[0],
            principal_scope: [3; 32],
            region: Region::ALL[0],
            workspace_id: WorkspaceId::from_uuid7(aex_wire::Uuid7::compose(1, [4; 10])),
            session_id: None,
            query_hash: [5; 32],
            order: Order::Ascending,
        };
        let original = Snapshot::at(instant(1_000));
        let binding = CursorBinding {
            route: request.route,
            principal_scope: request.principal_scope,
            region: request.region,
            workspace_id: request.workspace_id,
            session_id: request.session_id,
            query_hash: request.query_hash,
            order: request.order,
            snapshot: SnapshotToken::new(original.to_wire().to_string()).expect("snapshot token"),
        };
        let last = OrderTuple::new(
            instant(900),
            Signal::Logs,
            ObservationId::from_uuid7(aex_wire::Uuid7::compose(1, [6; 10])),
            1,
        );
        let resume = ObservationResume::new(
            BucketHour::parse("1970-01-01T00").expect("bucket"),
            last,
            Vec::new(),
        )
        .expect("resume");
        let token = issue_page(&key, &binding, &resume, instant(1_100)).expect("cursor issues");
        let ring = CursorKeyRing::new(key, Vec::new()).expect("key ring");

        let (snapshot, resumed) =
            resume_page(&ring, &token, &request, instant(1_500)).expect("cursor resumes");

        assert_eq!(snapshot, original);
        assert_eq!(resumed.last_order().expect("last tuple"), last);
    }

    #[test]
    fn the_largest_live_bucket_resume_fits_the_public_cursor_ceiling() {
        let key = CursorKey::new("current", vec![9; 32]).expect("strong cursor key");
        let workspace = WorkspaceId::from_uuid7(aex_wire::Uuid7::compose(1, [4; 10]));
        let request = CursorRequestBinding {
            route: RouteId::ALL[0],
            principal_scope: [3; 32],
            region: Region::ALL[0],
            workspace_id: workspace,
            session_id: None,
            query_hash: [5; 32],
            order: Order::Ascending,
        };
        let snapshot = Snapshot::at(instant(1_000));
        let binding = CursorBinding {
            route: request.route,
            principal_scope: request.principal_scope,
            region: request.region,
            workspace_id: request.workspace_id,
            session_id: None,
            query_hash: request.query_hash,
            order: request.order,
            snapshot: SnapshotToken::new(snapshot.to_wire().to_string()).expect("snapshot"),
        };
        let last_id = ObservationId::from_uuid7(aex_wire::Uuid7::compose(1, [6; 10]));
        let last = OrderTuple::new(instant(900), Signal::Traces, last_id, 1);
        let session = aex_wire::ids::SessionId::from_uuid7(aex_wire::Uuid7::compose(2, [7; 10]));
        let mut segments = Vec::new();
        for signal in Signal::ALL {
            for shard in 0..4 {
                let event = *signal == Signal::Events;
                segments.push(SegmentResume {
                    access: if event {
                        Access::SessionAuthority
                    } else {
                        Access::WorkspaceTime
                    },
                    signal: *signal,
                    shard,
                    state: SegmentState::After(ResumeKey {
                        // Rendered by `ScopeKey`, so this ceiling is measured
                        // against the key the reader actually mints — which now
                        // carries the workspace as well as the session, and is
                        // ~31 bytes longer per segment.
                        scope: ScopeKey::Session { workspace, session }.to_key(),
                        primary_ms: 900,
                        accepted_ms: 950,
                        observation_id: ObservationId::from_uuid7(aex_wire::Uuid7::compose(
                            1,
                            [shard.saturating_add(signal.rank()); 10],
                        ))
                        .to_string(),
                        revision: 1,
                        base_shard: shard,
                        event_seq: event.then_some(u64::from(shard) + 1),
                    }),
                });
            }
        }
        let resume = ObservationResume::new(
            BucketHour::parse("1970-01-01T00").expect("bucket"),
            last,
            segments,
        )
        .expect("bounded resume");
        let token = issue_page(&key, &binding, &resume, instant(1_100)).expect("cursor fits");
        assert!(token.as_str().len() <= aex_wire::cursor::Cursor::MAX_BYTES);
    }
}
