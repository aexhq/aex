//! Normalization, identity overwrite and the reserved namespace policy.
//!
//! Everything a customer can influence is either overwritten from the
//! authenticated scope or removed. The counts land on the admission receipt as
//! `overwrittenFields`, so an operator can see how much a producer tried to
//! claim rather than only that it failed.

use std::collections::BTreeMap;

use aex_internal_contracts::RunId;
use aex_observation_domain::canonical::{CanonicalValue, attribute_digest};
use aex_observation_domain::series::SeriesHash;
use aex_observation_domain::signal::Signal;
use aex_wire::ids::OrganizationId;
use aex_wire::ids::{AgentId, SessionId, WorkspaceId};
use aex_wire::types::Timestamp;

use crate::decode::DecodedBatch;
use crate::error::{OtlpError, RecordPointer};
use crate::limits::OtlpLimits;
use crate::proto::{common, logs, metrics, trace};

/// The reserved attribute namespace no customer batch may write into.
pub const RESERVED_PREFIX: &str = "aex.";

/// The reserved sub-namespace whose presence rejects the whole batch.
pub const RESERVED_INTERNAL_PREFIX: &str = "aex.internal.";

/// The scope attributes AEX sets itself after removing every customer copy.
pub const SCOPE_ATTRIBUTES: &[&str] = &[
    "aex.workspace.id",
    "aex.session.id",
    "aex.run.id",
    "aex.agent.id",
    "aex.operation.id",
];

/// The protected identity fields a customer batch may never assert.
pub const PROTECTED_FIELDS: &[&str] = &[
    "workspaceId",
    "organizationId",
    "sessionId",
    "runId",
    "agentId",
    "operationId",
];

/// The authenticated scope an admitted batch is bound to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthenticatedScope {
    /// The owning organization.
    pub organization_id: OrganizationId,
    /// The owning workspace.
    pub workspace_id: WorkspaceId,
    /// The session, when the credential names one.
    pub session_id: Option<SessionId>,
    /// The run, when the credential names one.
    pub run_id: Option<RunId>,
    /// The agent, when the credential names one.
    pub agent_id: Option<AgentId>,
}

impl AuthenticatedScope {
    /// Asserts the hierarchy an AEX credential must satisfy.
    ///
    /// # Errors
    ///
    /// Returns [`OtlpError::ScopeHierarchy`] when a run has no session or an
    /// agent has no run. A batch whose scope cannot be placed in the tree has no
    /// unambiguous owner and is refused rather than guessed at.
    pub const fn validate(&self) -> Result<(), OtlpError> {
        if self.run_id.is_some() && self.session_id.is_none() {
            return Err(OtlpError::ScopeHierarchy {
                reason: "a run implies a session",
            });
        }
        if self.agent_id.is_some() && self.run_id.is_none() {
            return Err(OtlpError::ScopeHierarchy {
                reason: "an agent implies a run",
            });
        }
        Ok(())
    }
}

/// What the identity overwrite actually changed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OverwriteReport {
    /// How many protected identity fields were overwritten or removed.
    pub overwritten_fields: u32,
    /// How many reserved `aex.*` attributes were removed.
    pub removed_reserved: u32,
}

impl OverwriteReport {
    /// Folds another report into this one.
    pub const fn merge(&mut self, other: Self) {
        self.overwritten_fields += other.overwritten_fields;
        self.removed_reserved += other.removed_reserved;
    }
}

/// One normalized observation, ready to be canonicalized and staged.
#[derive(Clone, Debug, PartialEq)]
pub struct NormalizedObservation {
    /// Which signal it belongs to.
    pub signal: Signal,
    /// The producer's event time.
    pub time: Timestamp,
    /// The closed per-signal indexed scalar fields.
    pub indexed: BTreeMap<String, CanonicalValue>,
    /// The customer string attributes.
    pub attr_s: BTreeMap<String, String>,
    /// The customer number attributes.
    pub attr_n: BTreeMap<String, f64>,
    /// The customer boolean attributes.
    pub attr_b: BTreeMap<String, bool>,
    /// The signal-specific payload.
    pub body: CanonicalValue,
    /// The stable digest over the normalized attribute map.
    pub attr_digest: String,
    /// The W3C trace, when the record carries one.
    pub trace_id: Option<String>,
    /// The W3C span, when the record carries one.
    pub span_id: Option<String>,
    /// The metric name, on the `metrics` signal only.
    pub metric_name: Option<String>,
    /// The metric series identity, on the `metrics` signal only.
    pub series_hash: Option<SeriesHash>,
    /// Where the record sat in the decoded request.
    pub pointer: RecordPointer,
}

impl NormalizedObservation {
    /// The canonical byte length used for the per-observation ceiling and the
    /// logical-byte usage fact.
    ///
    /// # Errors
    ///
    /// Returns [`OtlpError::Malformed`] when the body cannot be canonicalized.
    pub fn logical_bytes(&self) -> Result<usize, OtlpError> {
        self.body
            .canonical_len()
            .map_err(|error| OtlpError::Malformed {
                encoding: "canonical",
                reason: error.to_string().into_boxed_str(),
            })
    }
}

/// A whole normalized batch plus what the overwrite changed.
#[derive(Clone, Debug, PartialEq)]
pub struct NormalizedBatch {
    /// The observations, in decoded order.
    pub observations: Vec<NormalizedObservation>,
    /// What the overwrite changed.
    pub report: OverwriteReport,
}

/// How many data points one metric carries.
#[must_use]
pub fn metric_point_count(metric: &metrics::Metric) -> usize {
    match &metric.data {
        None => 0,
        Some(metrics::metric::Data::Gauge(gauge)) => gauge.data_points.len(),
        Some(metrics::metric::Data::Sum(sum)) => sum.data_points.len(),
        Some(metrics::metric::Data::Histogram(histogram)) => histogram.data_points.len(),
        Some(metrics::metric::Data::ExponentialHistogram(histogram)) => histogram.data_points.len(),
        Some(metrics::metric::Data::Summary(summary)) => summary.data_points.len(),
    }
}

/// Normalizes a decoded batch under the authenticated scope.
///
/// # Errors
///
/// Returns [`OtlpError::ReservedAttribute`] for any `aex.internal.*` attribute,
/// [`OtlpError::RecordBound`] for an attribute count, key, value, array or
/// normalized-size violation, and [`OtlpError::ScopeHierarchy`] for an
/// inconsistent scope.
pub fn normalize(
    batch: &DecodedBatch,
    scope: &AuthenticatedScope,
    limits: &OtlpLimits,
) -> Result<NormalizedBatch, OtlpError> {
    scope.validate()?;
    let mut observations = Vec::new();
    let mut report = OverwriteReport::default();
    match batch {
        DecodedBatch::Logs(request) => {
            for (resource_index, resource_logs) in request.resource_logs.iter().enumerate() {
                let base =
                    resource_attributes(resource_logs.resource.as_ref(), limits, &mut report)?;
                for (scope_index, scope_logs) in resource_logs.scope_logs.iter().enumerate() {
                    let logger = scope_logs
                        .scope
                        .as_ref()
                        .map(|inner| inner.name.clone())
                        .unwrap_or_default();
                    for (record_index, record) in scope_logs.log_records.iter().enumerate() {
                        let pointer = RecordPointer::new(resource_index, scope_index, record_index);
                        observations.push(normalize_log(
                            record,
                            &base,
                            &logger,
                            pointer,
                            limits,
                            &mut report,
                        )?);
                    }
                }
            }
        }
        DecodedBatch::Traces(request) => {
            for (resource_index, resource_spans) in request.resource_spans.iter().enumerate() {
                let base =
                    resource_attributes(resource_spans.resource.as_ref(), limits, &mut report)?;
                let service_name = base
                    .get("service.name")
                    .and_then(CanonicalValue::as_str)
                    .map(str::to_owned);
                for (scope_index, scope_spans) in resource_spans.scope_spans.iter().enumerate() {
                    let scope_name = scope_spans
                        .scope
                        .as_ref()
                        .map(|inner| inner.name.clone())
                        .unwrap_or_default();
                    for (record_index, record) in scope_spans.spans.iter().enumerate() {
                        let pointer = RecordPointer::new(resource_index, scope_index, record_index);
                        observations.push(normalize_span(
                            record,
                            &base,
                            service_name.as_deref(),
                            &scope_name,
                            pointer,
                            limits,
                            &mut report,
                        )?);
                    }
                }
            }
        }
        DecodedBatch::Metrics(request) => {
            for (resource_index, resource_metrics) in request.resource_metrics.iter().enumerate() {
                let base =
                    resource_attributes(resource_metrics.resource.as_ref(), limits, &mut report)?;
                for (scope_index, scope_metrics) in
                    resource_metrics.scope_metrics.iter().enumerate()
                {
                    let mut record_index = 0usize;
                    for metric in &scope_metrics.metrics {
                        for point in normalize_metric(
                            metric,
                            &base,
                            scope,
                            resource_index,
                            scope_index,
                            &mut record_index,
                            limits,
                            &mut report,
                        )? {
                            observations.push(point);
                        }
                    }
                }
            }
        }
    }

    for observation in &observations {
        let bytes = observation.logical_bytes()?;
        if bytes > limits.normalized_max {
            return Err(OtlpError::RecordBound {
                at: observation.pointer.to_path().into_boxed_str(),
                bound: "normalized observation bytes",
                observed: bytes,
                limit: limits.normalized_max,
            });
        }
    }

    Ok(NormalizedBatch {
        observations,
        report,
    })
}

fn resource_attributes(
    resource: Option<&crate::proto::resource::Resource>,
    limits: &OtlpLimits,
    report: &mut OverwriteReport,
) -> Result<BTreeMap<String, CanonicalValue>, OtlpError> {
    let attributes = resource.map_or(&[][..], |inner| inner.attributes.as_slice());
    attribute_map(attributes, RecordPointer::default(), limits, report)
}

/// Converts an attribute list, enforcing every per-record bound and the
/// reserved-namespace policy.
fn attribute_map(
    attributes: &[common::KeyValue],
    pointer: RecordPointer,
    limits: &OtlpLimits,
    report: &mut OverwriteReport,
) -> Result<BTreeMap<String, CanonicalValue>, OtlpError> {
    if attributes.len() > limits.max_attributes {
        return Err(OtlpError::RecordBound {
            at: pointer.to_path().into_boxed_str(),
            bound: "attributes",
            observed: attributes.len(),
            limit: limits.max_attributes,
        });
    }
    let mut out = BTreeMap::new();
    for attribute in attributes {
        if attribute.key.len() > limits.max_attribute_key_bytes {
            return Err(OtlpError::RecordBound {
                at: pointer.to_path().into_boxed_str(),
                bound: "attribute key bytes",
                observed: attribute.key.len(),
                limit: limits.max_attribute_key_bytes,
            });
        }
        if attribute.key.starts_with(RESERVED_INTERNAL_PREFIX) {
            return Err(OtlpError::ReservedAttribute {
                at: pointer.to_path().into_boxed_str(),
                key: attribute.key.as_str().into(),
            });
        }
        if attribute.key.starts_with(RESERVED_PREFIX)
            || PROTECTED_FIELDS.contains(&attribute.key.as_str())
        {
            report.removed_reserved += 1;
            continue;
        }
        let Some(value) = attribute.value.as_ref() else {
            continue;
        };
        out.insert(
            attribute.key.clone(),
            convert_any(value, pointer, limits, 0)?,
        );
    }
    Ok(out)
}

fn convert_any(
    value: &common::AnyValue,
    pointer: RecordPointer,
    limits: &OtlpLimits,
    depth: usize,
) -> Result<CanonicalValue, OtlpError> {
    const MAX_DEPTH: usize = 8;
    if depth > MAX_DEPTH {
        return Err(OtlpError::RecordBound {
            at: pointer.to_path().into_boxed_str(),
            bound: "attribute nesting depth",
            observed: depth,
            limit: MAX_DEPTH,
        });
    }
    Ok(match &value.value {
        None => CanonicalValue::Null,
        Some(common::any_value::Value::StringValue(text)) => {
            if text.len() > limits.max_attribute_value_bytes {
                return Err(OtlpError::RecordBound {
                    at: pointer.to_path().into_boxed_str(),
                    bound: "attribute value bytes",
                    observed: text.len(),
                    limit: limits.max_attribute_value_bytes,
                });
            }
            CanonicalValue::Str(text.as_str().into())
        }
        Some(common::any_value::Value::BoolValue(flag)) => CanonicalValue::Bool(*flag),
        Some(common::any_value::Value::IntValue(number)) => CanonicalValue::Int(*number),
        Some(common::any_value::Value::DoubleValue(number)) => CanonicalValue::number(*number)
            .map_err(|error| OtlpError::Malformed {
                encoding: "canonical",
                reason: error.to_string().into_boxed_str(),
            })?,
        Some(common::any_value::Value::BytesValue(bytes)) => {
            if bytes.len() > limits.max_attribute_value_bytes {
                return Err(OtlpError::RecordBound {
                    at: pointer.to_path().into_boxed_str(),
                    bound: "attribute value bytes",
                    observed: bytes.len(),
                    limit: limits.max_attribute_value_bytes,
                });
            }
            CanonicalValue::Str(hex(bytes).into())
        }
        Some(common::any_value::Value::ArrayValue(array)) => {
            if array.values.len() > limits.max_array_elements {
                return Err(OtlpError::RecordBound {
                    at: pointer.to_path().into_boxed_str(),
                    bound: "array elements",
                    observed: array.values.len(),
                    limit: limits.max_array_elements,
                });
            }
            CanonicalValue::Array(
                array
                    .values
                    .iter()
                    .map(|element| convert_any(element, pointer, limits, depth + 1))
                    .collect::<Result<Vec<_>, _>>()?,
            )
        }
        Some(common::any_value::Value::KvlistValue(list)) => {
            if list.values.len() > limits.max_attributes {
                return Err(OtlpError::RecordBound {
                    at: pointer.to_path().into_boxed_str(),
                    bound: "attributes",
                    observed: list.values.len(),
                    limit: limits.max_attributes,
                });
            }
            let mut map = BTreeMap::new();
            for entry in &list.values {
                let Some(inner) = entry.value.as_ref() else {
                    continue;
                };
                map.insert(
                    entry.key.clone(),
                    convert_any(inner, pointer, limits, depth + 1)?,
                );
            }
            CanonicalValue::Map(map)
        }
        Some(common::any_value::Value::StringValueStrindex(index)) => {
            CanonicalValue::Int(i64::from(*index))
        }
    })
}

/// The four typed attribute maps an observation item stores.
type SplitAttributes = (
    BTreeMap<String, String>,
    BTreeMap<String, f64>,
    BTreeMap<String, bool>,
    BTreeMap<String, CanonicalValue>,
);

fn split_attributes(merged: BTreeMap<String, CanonicalValue>) -> SplitAttributes {
    let mut strings = BTreeMap::new();
    let mut numbers = BTreeMap::new();
    let mut booleans = BTreeMap::new();
    let mut rest = BTreeMap::new();
    for (key, value) in merged {
        match value {
            CanonicalValue::Str(text) => {
                strings.insert(key, text.into_string());
            }
            CanonicalValue::Int(number) => {
                #[allow(
                    clippy::cast_precision_loss,
                    reason = "the numeric attribute index is a double by contract"
                )]
                numbers.insert(key, number as f64);
            }
            CanonicalValue::Num(number) => {
                numbers.insert(key, number);
            }
            CanonicalValue::Bool(flag) => {
                booleans.insert(key, flag);
            }
            other => {
                rest.insert(key, other);
            }
        }
    }
    (strings, numbers, booleans, rest)
}

fn finish(
    signal: Signal,
    time: Timestamp,
    indexed: BTreeMap<String, CanonicalValue>,
    merged: BTreeMap<String, CanonicalValue>,
    body: CanonicalValue,
    pointer: RecordPointer,
) -> Result<NormalizedObservation, OtlpError> {
    let digest = attribute_digest(&merged).map_err(|error| OtlpError::Malformed {
        encoding: "canonical",
        reason: error.to_string().into_boxed_str(),
    })?;
    let (attr_s, attr_n, attr_b, _rest) = split_attributes(merged);
    Ok(NormalizedObservation {
        signal,
        time,
        indexed,
        attr_s,
        attr_n,
        attr_b,
        body,
        attr_digest: digest,
        trace_id: None,
        span_id: None,
        metric_name: None,
        series_hash: None,
        pointer,
    })
}

fn normalize_log(
    record: &logs::LogRecord,
    base: &BTreeMap<String, CanonicalValue>,
    logger: &str,
    pointer: RecordPointer,
    limits: &OtlpLimits,
    report: &mut OverwriteReport,
) -> Result<NormalizedObservation, OtlpError> {
    let mut merged = base.clone();
    merged.extend(attribute_map(&record.attributes, pointer, limits, report)?);

    let nanos = if record.time_unix_nano == 0 {
        record.observed_time_unix_nano
    } else {
        record.time_unix_nano
    };
    let time = timestamp_from_nanos(nanos, pointer)?;
    let body = record
        .body
        .as_ref()
        .map(|value| convert_any(value, pointer, limits, 0))
        .transpose()?
        .unwrap_or(CanonicalValue::Null);

    let stream = merged
        .get("aex.log.stream")
        .and_then(CanonicalValue::as_str)
        .map(str::to_owned);

    let mut indexed = BTreeMap::new();
    indexed.insert(
        "severityNumber".to_owned(),
        CanonicalValue::Int(i64::from(record.severity_number)),
    );
    indexed.insert(
        "severityText".to_owned(),
        CanonicalValue::Str(record.severity_text.as_str().into()),
    );
    indexed.insert("logger".to_owned(), CanonicalValue::Str(logger.into()));
    indexed.insert(
        "stream".to_owned(),
        stream.map_or(CanonicalValue::Null, |text| {
            CanonicalValue::Str(text.into())
        }),
    );

    let mut observation = finish(Signal::Logs, time, indexed, merged, body, pointer)?;
    observation.trace_id = optional_hex(&record.trace_id);
    observation.span_id = optional_hex(&record.span_id);
    report.overwritten_fields += 1;
    Ok(observation)
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "one parameter and one branch per normalized input"
)]
fn normalize_span(
    record: &trace::Span,
    base: &BTreeMap<String, CanonicalValue>,
    service_name: Option<&str>,
    scope_name: &str,
    pointer: RecordPointer,
    limits: &OtlpLimits,
    report: &mut OverwriteReport,
) -> Result<NormalizedObservation, OtlpError> {
    let mut merged = base.clone();
    merged.extend(attribute_map(&record.attributes, pointer, limits, report)?);

    let time = timestamp_from_nanos(record.start_time_unix_nano, pointer)?;
    let duration = record
        .end_time_unix_nano
        .saturating_sub(record.start_time_unix_nano);

    let mut indexed = BTreeMap::new();
    indexed.insert(
        "name".to_owned(),
        CanonicalValue::Str(record.name.as_str().into()),
    );
    indexed.insert(
        "kind".to_owned(),
        CanonicalValue::Int(i64::from(record.kind)),
    );
    indexed.insert(
        "status".to_owned(),
        CanonicalValue::Int(i64::from(
            record.status.as_ref().map_or(0, |status| status.code),
        )),
    );
    indexed.insert(
        "durationNs".to_owned(),
        CanonicalValue::Int(i64::try_from(duration).unwrap_or(i64::MAX)),
    );
    indexed.insert(
        "serviceName".to_owned(),
        service_name.map_or(CanonicalValue::Null, |name| {
            CanonicalValue::Str(name.into())
        }),
    );
    indexed.insert(
        "scopeName".to_owned(),
        CanonicalValue::Str(scope_name.into()),
    );
    indexed.insert(
        "parentSpanId".to_owned(),
        optional_hex(&record.parent_span_id).map_or(CanonicalValue::Null, |text| {
            CanonicalValue::Str(text.into())
        }),
    );

    // Span events and links are canonical child structures inside the body, not
    // separate observations: they have no independent identity and duplicating
    // them would double the batch's record count for no query benefit.
    let mut body = BTreeMap::new();
    body.insert(
        "traceState".to_owned(),
        CanonicalValue::Str(record.trace_state.as_str().into()),
    );
    body.insert(
        "events".to_owned(),
        CanonicalValue::Array(
            record
                .events
                .iter()
                .map(|event| {
                    let mut entry = BTreeMap::new();
                    entry.insert(
                        "name".to_owned(),
                        CanonicalValue::Str(event.name.as_str().into()),
                    );
                    entry.insert(
                        "timeUnixNano".to_owned(),
                        CanonicalValue::Int(
                            i64::try_from(event.time_unix_nano).unwrap_or(i64::MAX),
                        ),
                    );
                    CanonicalValue::Map(entry)
                })
                .collect(),
        ),
    );
    body.insert(
        "links".to_owned(),
        CanonicalValue::Array(
            record
                .links
                .iter()
                .map(|link| {
                    let mut entry = BTreeMap::new();
                    entry.insert(
                        "traceId".to_owned(),
                        CanonicalValue::Str(hex(&link.trace_id).into()),
                    );
                    entry.insert(
                        "spanId".to_owned(),
                        CanonicalValue::Str(hex(&link.span_id).into()),
                    );
                    CanonicalValue::Map(entry)
                })
                .collect(),
        ),
    );
    if let Some(status) = record.status.as_ref() {
        body.insert(
            "statusMessage".to_owned(),
            CanonicalValue::Str(status.message.as_str().into()),
        );
    }

    let mut observation = finish(
        Signal::Spans,
        time,
        indexed,
        merged,
        CanonicalValue::Map(body),
        pointer,
    )?;
    observation.trace_id = optional_hex(&record.trace_id);
    observation.span_id = optional_hex(&record.span_id);
    report.overwritten_fields += 1;
    Ok(observation)
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "one parameter and one branch per metric shape"
)]
fn normalize_metric(
    metric: &metrics::Metric,
    base: &BTreeMap<String, CanonicalValue>,
    scope: &AuthenticatedScope,
    resource_index: usize,
    scope_index: usize,
    record_index: &mut usize,
    limits: &OtlpLimits,
    report: &mut OverwriteReport,
) -> Result<Vec<NormalizedObservation>, OtlpError> {
    let mut out = Vec::new();
    let (kind, temporality, monotonic) = match &metric.data {
        None => return Ok(out),
        Some(metrics::metric::Data::Gauge(_)) => ("gauge", 0, false),
        Some(metrics::metric::Data::Sum(sum)) => {
            ("sum", sum.aggregation_temporality, sum.is_monotonic)
        }
        Some(metrics::metric::Data::Histogram(histogram)) => {
            ("histogram", histogram.aggregation_temporality, false)
        }
        Some(metrics::metric::Data::ExponentialHistogram(histogram)) => (
            "exponential_histogram",
            histogram.aggregation_temporality,
            false,
        ),
        Some(metrics::metric::Data::Summary(_)) => ("summary", 0, false),
    };
    let temporality_name = match temporality {
        1 => "delta",
        2 => "cumulative",
        _ => "unspecified",
    };

    let mut push = |attributes: &[common::KeyValue],
                    time_nanos: u64,
                    value: f64,
                    extra: BTreeMap<String, CanonicalValue>|
     -> Result<(), OtlpError> {
        let pointer = RecordPointer::new(resource_index, scope_index, *record_index);
        *record_index += 1;
        let mut merged = base.clone();
        merged.extend(attribute_map(attributes, pointer, limits, report)?);
        let time = timestamp_from_nanos(time_nanos, pointer)?;

        let mut indexed = BTreeMap::new();
        indexed.insert(
            "name".to_owned(),
            CanonicalValue::Str(metric.name.as_str().into()),
        );
        indexed.insert("kind".to_owned(), CanonicalValue::Str(kind.into()));
        indexed.insert(
            "unit".to_owned(),
            CanonicalValue::Str(metric.unit.as_str().into()),
        );
        indexed.insert(
            "temporality".to_owned(),
            CanonicalValue::Str(temporality_name.into()),
        );
        indexed.insert("monotonic".to_owned(), CanonicalValue::Bool(monotonic));
        indexed.insert(
            "value".to_owned(),
            CanonicalValue::number(value).map_err(|error| OtlpError::Malformed {
                encoding: "canonical",
                reason: error.to_string().into_boxed_str(),
            })?,
        );

        let attribute_pairs: Vec<(String, String)> = merged
            .iter()
            .map(|(key, value)| (key.clone(), render_scalar(value)))
            .collect();
        let series = SeriesHash::compute(
            scope.workspace_id.to_string().as_str(),
            &metric.name,
            kind,
            &metric.unit,
            temporality_name,
            monotonic,
            &attribute_pairs,
        );

        let mut observation = finish(
            Signal::Metrics,
            time,
            indexed,
            merged,
            CanonicalValue::Map(extra),
            pointer,
        )?;
        observation.metric_name = Some(metric.name.clone());
        observation.series_hash = Some(series);
        report.overwritten_fields += 1;
        out.push(observation);
        Ok(())
    };

    match &metric.data {
        None => {}
        Some(metrics::metric::Data::Gauge(gauge)) => {
            for point in &gauge.data_points {
                push(
                    &point.attributes,
                    point.time_unix_nano,
                    number_value(point),
                    BTreeMap::new(),
                )?;
            }
        }
        Some(metrics::metric::Data::Sum(sum)) => {
            for point in &sum.data_points {
                push(
                    &point.attributes,
                    point.time_unix_nano,
                    number_value(point),
                    BTreeMap::new(),
                )?;
            }
        }
        Some(metrics::metric::Data::Histogram(histogram)) => {
            for point in &histogram.data_points {
                let mut extra = BTreeMap::new();
                extra.insert(
                    "count".to_owned(),
                    CanonicalValue::Int(i64::try_from(point.count).unwrap_or(i64::MAX)),
                );
                extra.insert(
                    "bucketCounts".to_owned(),
                    CanonicalValue::Array(
                        point
                            .bucket_counts
                            .iter()
                            .map(|count| {
                                CanonicalValue::Int(i64::try_from(*count).unwrap_or(i64::MAX))
                            })
                            .collect(),
                    ),
                );
                extra.insert(
                    "explicitBounds".to_owned(),
                    CanonicalValue::Array(
                        point
                            .explicit_bounds
                            .iter()
                            .filter_map(|bound| CanonicalValue::number(*bound).ok())
                            .collect(),
                    ),
                );
                push(
                    &point.attributes,
                    point.time_unix_nano,
                    point.sum.unwrap_or_default(),
                    extra,
                )?;
            }
        }
        Some(metrics::metric::Data::ExponentialHistogram(histogram)) => {
            for point in &histogram.data_points {
                let mut extra = BTreeMap::new();
                extra.insert(
                    "count".to_owned(),
                    CanonicalValue::Int(i64::try_from(point.count).unwrap_or(i64::MAX)),
                );
                extra.insert(
                    "scale".to_owned(),
                    CanonicalValue::Int(i64::from(point.scale)),
                );
                extra.insert(
                    "zeroCount".to_owned(),
                    CanonicalValue::Int(i64::try_from(point.zero_count).unwrap_or(i64::MAX)),
                );
                push(
                    &point.attributes,
                    point.time_unix_nano,
                    point.sum.unwrap_or_default(),
                    extra,
                )?;
            }
        }
        Some(metrics::metric::Data::Summary(summary)) => {
            for point in &summary.data_points {
                let mut extra = BTreeMap::new();
                extra.insert(
                    "count".to_owned(),
                    CanonicalValue::Int(i64::try_from(point.count).unwrap_or(i64::MAX)),
                );
                extra.insert(
                    "quantiles".to_owned(),
                    CanonicalValue::Array(
                        point
                            .quantile_values
                            .iter()
                            .filter_map(|quantile| {
                                let mut entry = BTreeMap::new();
                                entry.insert(
                                    "quantile".to_owned(),
                                    CanonicalValue::number(quantile.quantile).ok()?,
                                );
                                entry.insert(
                                    "value".to_owned(),
                                    CanonicalValue::number(quantile.value).ok()?,
                                );
                                Some(CanonicalValue::Map(entry))
                            })
                            .collect(),
                    ),
                );
                push(&point.attributes, point.time_unix_nano, point.sum, extra)?;
            }
        }
    }
    Ok(out)
}

fn number_value(point: &metrics::NumberDataPoint) -> f64 {
    match point.value {
        Some(metrics::number_data_point::Value::AsDouble(number)) => number,
        #[allow(
            clippy::cast_precision_loss,
            reason = "the scalar surface of a metric point is a double by contract"
        )]
        Some(metrics::number_data_point::Value::AsInt(number)) => number as f64,
        None => 0.0,
    }
}

fn render_scalar(value: &CanonicalValue) -> String {
    match value {
        CanonicalValue::Str(text) => text.to_string(),
        CanonicalValue::Int(number) => number.to_string(),
        CanonicalValue::Num(number) => format!("{number}"),
        CanonicalValue::Bool(flag) => flag.to_string(),
        CanonicalValue::Null => String::new(),
        other => other.kind().to_owned(),
    }
}

fn timestamp_from_nanos(nanos: u64, pointer: RecordPointer) -> Result<Timestamp, OtlpError> {
    #[allow(
        clippy::cast_possible_wrap,
        reason = "the millisecond value is bounded by the wire timestamp range"
    )]
    let millis = (nanos / 1_000_000) as i64;
    Timestamp::from_unix_millis(millis).map_err(|_| OtlpError::RecordBound {
        at: pointer.to_path().into_boxed_str(),
        bound: "observation time",
        observed: usize::try_from(millis).unwrap_or(usize::MAX),
        limit: usize::MAX,
    })
}

fn optional_hex(bytes: &[u8]) -> Option<String> {
    if bytes.iter().all(|byte| *byte == 0) {
        return None;
    }
    Some(hex(bytes))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{AuthenticatedScope, normalize};
    use crate::decode::DecodedBatch;
    use crate::error::OtlpError;
    use crate::limits::OtlpLimits;
    use crate::proto::{collector_logs, common, logs};

    fn scope() -> AuthenticatedScope {
        AuthenticatedScope {
            organization_id: PrefixedId::parse("org_0000000001e40r2081040g2081")
                .expect("fixture parses"),
            workspace_id: PrefixedId::parse("wsp_0000000002e81840g2081040g2")
                .expect("fixture parses"),
            session_id: Some(
                PrefixedId::parse("ses_0000000003ec1r60r30c1g60r3").expect("fixture parses"),
            ),
            run_id: None,
            agent_id: None,
        }
    }

    use aex_wire::ids::PrefixedId;

    fn attribute(key: &str, value: &str) -> common::KeyValue {
        common::KeyValue {
            key: key.to_owned(),
            value: Some(common::AnyValue {
                value: Some(common::any_value::Value::StringValue(value.to_owned())),
            }),
            key_strindex: 0,
        }
    }

    fn logs_batch(attributes: Vec<common::KeyValue>) -> DecodedBatch {
        DecodedBatch::Logs(Box::new(collector_logs::ExportLogsServiceRequest {
            resource_logs: vec![logs::ResourceLogs {
                resource: None,
                scope_logs: vec![logs::ScopeLogs {
                    scope: None,
                    log_records: vec![logs::LogRecord {
                        time_unix_nano: 1_700_000_000_000_000_000,
                        attributes,
                        ..logs::LogRecord::default()
                    }],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        }))
    }

    #[test]
    fn an_internal_reserved_attribute_rejects_the_whole_batch() {
        let batch = logs_batch(vec![attribute("aex.internal.secret", "x")]);
        let error = normalize(&batch, &scope(), &OtlpLimits::REGISTERED).expect_err("rejected");
        assert!(matches!(error, OtlpError::ReservedAttribute { .. }));
    }

    #[test]
    fn every_reserved_and_protected_field_is_removed_and_counted() {
        let batch = logs_batch(vec![
            attribute("aex.workspace.id", "wsp_hostile"),
            attribute("workspaceId", "wsp_hostile"),
            attribute("sessionId", "ses_hostile"),
            attribute("service.name", "keep-me"),
        ]);
        let normalized = normalize(&batch, &scope(), &OtlpLimits::REGISTERED).expect("normalizes");
        assert_eq!(normalized.report.removed_reserved, 3);
        let observation = &normalized.observations[0];
        assert!(observation.attr_s.contains_key("service.name"));
        for hostile in ["aex.workspace.id", "workspaceId", "sessionId"] {
            assert!(
                !observation.attr_s.contains_key(hostile),
                "`{hostile}` survived the overwrite"
            );
        }
    }

    #[test]
    fn an_attribute_count_above_the_ceiling_is_a_typed_record_bound() {
        let limits = OtlpLimits {
            max_attributes: 2,
            ..OtlpLimits::REGISTERED
        };
        let batch = logs_batch(vec![
            attribute("a", "1"),
            attribute("b", "2"),
            attribute("c", "3"),
        ]);
        let error = normalize(&batch, &scope(), &limits).expect_err("rejected");
        assert!(matches!(
            error,
            OtlpError::RecordBound {
                bound: "attributes",
                observed: 3,
                limit: 2,
                ..
            }
        ));
    }

    #[test]
    fn an_agent_without_a_run_is_a_scope_hierarchy_violation() {
        let mut hostile = scope();
        hostile.agent_id =
            Some(PrefixedId::parse("agt_0000000004eg2881040g208104").expect("fixture parses"));
        let batch = logs_batch(Vec::new());
        let error = normalize(&batch, &hostile, &OtlpLimits::REGISTERED).expect_err("rejected");
        assert!(matches!(error, OtlpError::ScopeHierarchy { .. }));
    }
}
