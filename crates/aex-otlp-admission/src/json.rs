//! The hand-written OTLP/JSON decoder.
//!
//! `prost` does not implement the protobuf-JSON mapping, so this module decodes
//! OTLP/JSON into exactly the same generated structs the protobuf path
//! produces. The parity corpus asserts both encodings reach a byte-identical
//! canonical batch.
//!
//! The mapping rules implemented here are the protobuf-JSON ones OTLP pins:
//!
//! - 64-bit integers are accepted as a JSON number **or** a decimal string;
//! - `traceId`/`spanId` are lowercase hex of exactly 32/16 characters;
//! - enums are accepted as their name or their number;
//! - `attributes` carry the `AnyValue` union;
//! - both `lowerCamelCase` and the original proto field name are accepted,
//!   which the mapping requires;
//! - **unknown fields are rejected**, not ignored. An unknown field in a pinned
//!   protocol is either version drift or an attack, and both deserve a 4xx.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

use crate::decode::DecodedBatch;
use crate::error::{OtlpError, OtlpSignal};
use crate::proto::{
    collector_logs, collector_metrics, collector_trace, common, logs, metrics, resource, trace,
};

type Result<T> = std::result::Result<T, OtlpError>;

/// Decodes one OTLP/JSON request body.
///
/// # Errors
///
/// Returns [`OtlpError::Malformed`] for anything that is not valid JSON in the
/// pinned shape and [`OtlpError::UnknownField`] for a member the protocol does
/// not define.
pub fn decode_json(signal: OtlpSignal, bytes: &[u8]) -> Result<DecodedBatch> {
    let value: Value = serde_json::from_slice(bytes).map_err(|error| OtlpError::Malformed {
        encoding: "json",
        reason: error.to_string().into_boxed_str(),
    })?;
    Ok(match signal {
        OtlpSignal::Logs => {
            let mut reader = Reader::new(&value, "")?;
            let request = collector_logs::ExportLogsServiceRequest {
                resource_logs: reader.list("resourceLogs", "resource_logs", resource_logs)?,
            };
            reader.finish()?;
            DecodedBatch::Logs(Box::new(request))
        }
        OtlpSignal::Traces => {
            let mut reader = Reader::new(&value, "")?;
            let request = collector_trace::ExportTraceServiceRequest {
                resource_spans: reader.list("resourceSpans", "resource_spans", resource_spans)?,
            };
            reader.finish()?;
            DecodedBatch::Traces(Box::new(request))
        }
        OtlpSignal::Metrics => {
            let mut reader = Reader::new(&value, "")?;
            let request = collector_metrics::ExportMetricsServiceRequest {
                resource_metrics: reader.list(
                    "resourceMetrics",
                    "resource_metrics",
                    resource_metrics,
                )?,
            };
            reader.finish()?;
            DecodedBatch::Metrics(Box::new(request))
        }
    })
}

// --- the reader ------------------------------------------------------------

/// A bounded object reader that refuses any member it was not asked for.
struct Reader<'a> {
    map: &'a Map<String, Value>,
    path: String,
    seen: BTreeSet<String>,
}

impl<'a> Reader<'a> {
    fn new(value: &'a Value, path: &str) -> Result<Self> {
        let map = value.as_object().ok_or_else(|| malformed(path, "object"))?;
        Ok(Self {
            map,
            path: path.to_owned(),
            seen: BTreeSet::new(),
        })
    }

    fn take(&mut self, camel: &str, snake: &str) -> Option<&'a Value> {
        self.seen.insert(camel.to_owned());
        self.seen.insert(snake.to_owned());
        self.map
            .get(camel)
            .or_else(|| self.map.get(snake))
            .filter(|value| !value.is_null())
    }

    fn child_path(&self, member: &str) -> String {
        format!("{}/{member}", self.path)
    }

    fn list<T>(
        &mut self,
        camel: &str,
        snake: &str,
        mut item: impl FnMut(&Value, &str) -> Result<T>,
    ) -> Result<Vec<T>> {
        let path = self.child_path(camel);
        let Some(value) = self.take(camel, snake) else {
            return Ok(Vec::new());
        };
        let array = value.as_array().ok_or_else(|| malformed(&path, "array"))?;
        array
            .iter()
            .enumerate()
            .map(|(index, element)| item(element, &format!("{path}/{index}")))
            .collect()
    }

    fn message<T>(
        &mut self,
        camel: &str,
        snake: &str,
        parse: impl FnOnce(&Value, &str) -> Result<T>,
    ) -> Result<Option<T>> {
        let path = self.child_path(camel);
        match self.take(camel, snake) {
            None => Ok(None),
            Some(value) => parse(value, &path).map(Some),
        }
    }

    fn string(&mut self, camel: &str, snake: &str) -> Result<String> {
        let path = self.child_path(camel);
        match self.take(camel, snake) {
            None => Ok(String::new()),
            Some(value) => value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| malformed(&path, "string")),
        }
    }

    fn u64(&mut self, camel: &str, snake: &str) -> Result<u64> {
        let path = self.child_path(camel);
        match self.take(camel, snake) {
            None => Ok(0),
            Some(value) => integer64(value, &path).and_then(|raw| {
                u64::try_from(raw).map_err(|_| malformed(&path, "unsigned 64-bit integer"))
            }),
        }
    }

    fn u32(&mut self, camel: &str, snake: &str) -> Result<u32> {
        let path = self.child_path(camel);
        match self.take(camel, snake) {
            None => Ok(0),
            Some(value) => integer64(value, &path).and_then(|raw| {
                u32::try_from(raw).map_err(|_| malformed(&path, "unsigned 32-bit integer"))
            }),
        }
    }

    fn i32(&mut self, camel: &str, snake: &str) -> Result<i32> {
        let path = self.child_path(camel);
        match self.take(camel, snake) {
            None => Ok(0),
            Some(value) => integer64(value, &path).and_then(|raw| {
                i32::try_from(raw).map_err(|_| malformed(&path, "signed 32-bit integer"))
            }),
        }
    }

    fn f64(&mut self, camel: &str, snake: &str) -> Result<f64> {
        let path = self.child_path(camel);
        match self.take(camel, snake) {
            None => Ok(0.0),
            Some(value) => double(value, &path),
        }
    }

    fn optional_f64(&mut self, camel: &str, snake: &str) -> Result<Option<f64>> {
        let path = self.child_path(camel);
        match self.take(camel, snake) {
            None => Ok(None),
            Some(value) => double(value, &path).map(Some),
        }
    }

    fn bool(&mut self, camel: &str, snake: &str) -> Result<bool> {
        let path = self.child_path(camel);
        match self.take(camel, snake) {
            None => Ok(false),
            Some(value) => value.as_bool().ok_or_else(|| malformed(&path, "boolean")),
        }
    }

    fn hex_id(&mut self, camel: &str, snake: &str, width: usize) -> Result<prost::bytes::Bytes> {
        let path = self.child_path(camel);
        match self.take(camel, snake) {
            None => Ok(prost::bytes::Bytes::new()),
            Some(value) => hex_bytes(value, &path, width).map(prost::bytes::Bytes::from),
        }
    }

    fn hex_id_vec(&mut self, camel: &str, snake: &str, width: usize) -> Result<Vec<u8>> {
        let path = self.child_path(camel);
        match self.take(camel, snake) {
            None => Ok(Vec::new()),
            Some(value) => hex_bytes(value, &path, width),
        }
    }

    fn enumeration(
        &mut self,
        camel: &str,
        snake: &str,
        from_name: impl Fn(&str) -> Option<i32>,
    ) -> Result<i32> {
        let path = self.child_path(camel);
        match self.take(camel, snake) {
            None => Ok(0),
            Some(Value::String(name)) => {
                from_name(name).ok_or_else(|| malformed(&path, "a declared enum name"))
            }
            Some(value) => integer64(value, &path)
                .and_then(|raw| i32::try_from(raw).map_err(|_| malformed(&path, "an enum number"))),
        }
    }

    fn u64_list(&mut self, camel: &str, snake: &str) -> Result<Vec<u64>> {
        let path = self.child_path(camel);
        let Some(value) = self.take(camel, snake) else {
            return Ok(Vec::new());
        };
        let array = value.as_array().ok_or_else(|| malformed(&path, "array"))?;
        array
            .iter()
            .enumerate()
            .map(|(index, element)| {
                let at = format!("{path}/{index}");
                integer64(element, &at).and_then(|raw| {
                    u64::try_from(raw).map_err(|_| malformed(&at, "unsigned 64-bit integer"))
                })
            })
            .collect()
    }

    fn f64_list(&mut self, camel: &str, snake: &str) -> Result<Vec<f64>> {
        let path = self.child_path(camel);
        let Some(value) = self.take(camel, snake) else {
            return Ok(Vec::new());
        };
        let array = value.as_array().ok_or_else(|| malformed(&path, "array"))?;
        array
            .iter()
            .enumerate()
            .map(|(index, element)| double(element, &format!("{path}/{index}")))
            .collect()
    }

    fn string_list(&mut self, camel: &str, snake: &str) -> Result<Vec<String>> {
        let path = self.child_path(camel);
        let Some(value) = self.take(camel, snake) else {
            return Ok(Vec::new());
        };
        let array = value.as_array().ok_or_else(|| malformed(&path, "array"))?;
        array
            .iter()
            .enumerate()
            .map(|(index, element)| {
                element
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| malformed(&format!("{path}/{index}"), "string"))
            })
            .collect()
    }

    fn finish(self) -> Result<()> {
        for key in self.map.keys() {
            if !self.seen.contains(key.as_str()) {
                return Err(OtlpError::UnknownField {
                    field: key.as_str().into(),
                    at: if self.path.is_empty() {
                        "/".into()
                    } else {
                        self.path.into_boxed_str()
                    },
                });
            }
        }
        Ok(())
    }
}

fn malformed(path: &str, expected: &str) -> OtlpError {
    OtlpError::Malformed {
        encoding: "json",
        reason: format!(
            "expected {expected} at `{}`",
            if path.is_empty() { "/" } else { path }
        )
        .into_boxed_str(),
    }
}

/// A 64-bit integer, accepted as a JSON number or a decimal string.
fn integer64(value: &Value, path: &str) -> Result<i128> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .map(i128::from)
            .or_else(|| number.as_u64().map(i128::from))
            .ok_or_else(|| malformed(path, "an integer")),
        Value::String(text) => text
            .parse::<i128>()
            .map_err(|_| malformed(path, "a decimal integer string")),
        _ => Err(malformed(path, "an integer")),
    }
}

fn double(value: &Value, path: &str) -> Result<f64> {
    match value {
        Value::Number(number) => number.as_f64().ok_or_else(|| malformed(path, "a number")),
        // The mapping permits the three non-finite spellings, which AEX rejects:
        // a non-finite double has no canonical JSON spelling and would become
        // `null` silently downstream.
        Value::String(text) => match text.as_str() {
            "NaN" | "Infinity" | "-Infinity" => Err(malformed(
                path,
                "a finite number (AEX rejects non-finite doubles)",
            )),
            other => other
                .parse::<f64>()
                .ok()
                .filter(|parsed| parsed.is_finite())
                .ok_or_else(|| malformed(path, "a finite number")),
        },
        _ => Err(malformed(path, "a number")),
    }
}

fn hex_bytes(value: &Value, path: &str, width: usize) -> Result<Vec<u8>> {
    let text = value
        .as_str()
        .ok_or_else(|| malformed(path, "a lowercase hex string"))?;
    if text.is_empty() {
        return Ok(Vec::new());
    }
    if text.len() != width
        || !text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(malformed(
            path,
            &format!("exactly {width} lowercase hex characters"),
        ));
    }
    (0..width / 2)
        .map(|index| {
            u8::from_str_radix(&text[index * 2..index * 2 + 2], 16)
                .map_err(|_| malformed(path, "lowercase hex"))
        })
        .collect()
}

fn base64_bytes(value: &Value, path: &str) -> Result<Vec<u8>> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let text = value
        .as_str()
        .ok_or_else(|| malformed(path, "a base64 string"))?;
    let symbols: Vec<u8> = text.bytes().filter(|byte| *byte != b'=').collect();
    let mut out = Vec::with_capacity(symbols.len() * 3 / 4);
    let mut accumulator: u32 = 0;
    let mut bits: u32 = 0;
    for symbol in symbols {
        let Some(index) = TABLE.iter().position(|candidate| *candidate == symbol) else {
            return Err(malformed(path, "standard base64"));
        };
        accumulator = (accumulator << 6) | u32::try_from(index).unwrap_or_default();
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            #[allow(
                clippy::cast_possible_truncation,
                reason = "the shift leaves exactly one byte"
            )]
            out.push((accumulator >> bits) as u8);
        }
    }
    Ok(out)
}

// --- common ----------------------------------------------------------------

fn any_value(value: &Value, path: &str) -> Result<common::AnyValue> {
    let mut reader = Reader::new(value, path)?;
    let mut resolved: Option<common::any_value::Value> = None;
    let mut set = |candidate: common::any_value::Value, path: &str| -> Result<()> {
        if resolved.is_some() {
            return Err(malformed(path, "exactly one AnyValue member"));
        }
        resolved = Some(candidate);
        Ok(())
    };
    if let Some(inner) = reader.take("stringValue", "string_value") {
        let text = inner
            .as_str()
            .ok_or_else(|| malformed(path, "string"))?
            .to_owned();
        set(common::any_value::Value::StringValue(text), path)?;
    }
    if let Some(inner) = reader.take("boolValue", "bool_value") {
        let flag = inner.as_bool().ok_or_else(|| malformed(path, "boolean"))?;
        set(common::any_value::Value::BoolValue(flag), path)?;
    }
    if let Some(inner) = reader.take("intValue", "int_value") {
        let raw = integer64(inner, path)?;
        let narrowed = i64::try_from(raw).map_err(|_| malformed(path, "signed 64-bit integer"))?;
        set(common::any_value::Value::IntValue(narrowed), path)?;
    }
    if let Some(inner) = reader.take("doubleValue", "double_value") {
        set(
            common::any_value::Value::DoubleValue(double(inner, path)?),
            path,
        )?;
    }
    if let Some(inner) = reader.take("bytesValue", "bytes_value") {
        set(
            common::any_value::Value::BytesValue(base64_bytes(inner, path)?),
            path,
        )?;
    }
    if let Some(inner) = reader.take("arrayValue", "array_value") {
        let mut nested = Reader::new(inner, &format!("{path}/arrayValue"))?;
        let values = nested.list("values", "values", any_value)?;
        nested.finish()?;
        set(
            common::any_value::Value::ArrayValue(common::ArrayValue { values }),
            path,
        )?;
    }
    if let Some(inner) = reader.take("kvlistValue", "kvlist_value") {
        let mut nested = Reader::new(inner, &format!("{path}/kvlistValue"))?;
        let values = nested.list("values", "values", key_value)?;
        nested.finish()?;
        set(
            common::any_value::Value::KvlistValue(common::KeyValueList { values }),
            path,
        )?;
    }
    reader.finish()?;
    Ok(common::AnyValue { value: resolved })
}

fn key_value(value: &Value, path: &str) -> Result<common::KeyValue> {
    let mut reader = Reader::new(value, path)?;
    let key = reader.string("key", "key")?;
    let inner = reader.message("value", "value", any_value)?;
    let key_strindex = reader.i32("keyStrindex", "key_strindex")?;
    reader.finish()?;
    Ok(common::KeyValue {
        key,
        value: inner,
        key_strindex,
    })
}

fn instrumentation_scope(value: &Value, path: &str) -> Result<common::InstrumentationScope> {
    let mut reader = Reader::new(value, path)?;
    let scope = common::InstrumentationScope {
        name: reader.string("name", "name")?,
        version: reader.string("version", "version")?,
        attributes: reader.list("attributes", "attributes", key_value)?,
        dropped_attributes_count: reader
            .u32("droppedAttributesCount", "dropped_attributes_count")?,
    };
    reader.finish()?;
    Ok(scope)
}

fn entity_ref(value: &Value, path: &str) -> Result<common::EntityRef> {
    let mut reader = Reader::new(value, path)?;
    let entity = common::EntityRef {
        schema_url: reader.string("schemaUrl", "schema_url")?,
        r#type: reader.string("type", "type")?,
        id_keys: reader.string_list("idKeys", "id_keys")?,
        description_keys: reader.string_list("descriptionKeys", "description_keys")?,
    };
    reader.finish()?;
    Ok(entity)
}

fn resource_message(value: &Value, path: &str) -> Result<resource::Resource> {
    let mut reader = Reader::new(value, path)?;
    let parsed = resource::Resource {
        attributes: reader.list("attributes", "attributes", key_value)?,
        dropped_attributes_count: reader
            .u32("droppedAttributesCount", "dropped_attributes_count")?,
        entity_refs: reader.list("entityRefs", "entity_refs", entity_ref)?,
    };
    reader.finish()?;
    Ok(parsed)
}

// --- logs ------------------------------------------------------------------

fn resource_logs(value: &Value, path: &str) -> Result<logs::ResourceLogs> {
    let mut reader = Reader::new(value, path)?;
    let parsed = logs::ResourceLogs {
        resource: reader.message("resource", "resource", resource_message)?,
        scope_logs: reader.list("scopeLogs", "scope_logs", scope_logs)?,
        schema_url: reader.string("schemaUrl", "schema_url")?,
    };
    reader.finish()?;
    Ok(parsed)
}

fn scope_logs(value: &Value, path: &str) -> Result<logs::ScopeLogs> {
    let mut reader = Reader::new(value, path)?;
    let parsed = logs::ScopeLogs {
        scope: reader.message("scope", "scope", instrumentation_scope)?,
        log_records: reader.list("logRecords", "log_records", log_record)?,
        schema_url: reader.string("schemaUrl", "schema_url")?,
    };
    reader.finish()?;
    Ok(parsed)
}

fn log_record(value: &Value, path: &str) -> Result<logs::LogRecord> {
    let mut reader = Reader::new(value, path)?;
    let parsed = logs::LogRecord {
        time_unix_nano: reader.u64("timeUnixNano", "time_unix_nano")?,
        observed_time_unix_nano: reader.u64("observedTimeUnixNano", "observed_time_unix_nano")?,
        severity_number: reader.enumeration("severityNumber", "severity_number", |name| {
            logs::SeverityNumber::from_str_name(name).map(|value| value as i32)
        })?,
        severity_text: reader.string("severityText", "severity_text")?,
        body: reader.message("body", "body", any_value)?,
        attributes: reader.list("attributes", "attributes", key_value)?,
        dropped_attributes_count: reader
            .u32("droppedAttributesCount", "dropped_attributes_count")?,
        flags: reader.u32("flags", "flags")?,
        trace_id: reader.hex_id("traceId", "trace_id", 32)?,
        span_id: reader.hex_id("spanId", "span_id", 16)?,
        event_name: reader.string("eventName", "event_name")?,
    };
    reader.finish()?;
    Ok(parsed)
}

// --- traces ----------------------------------------------------------------

fn resource_spans(value: &Value, path: &str) -> Result<trace::ResourceSpans> {
    let mut reader = Reader::new(value, path)?;
    let parsed = trace::ResourceSpans {
        resource: reader.message("resource", "resource", resource_message)?,
        scope_spans: reader.list("scopeSpans", "scope_spans", scope_spans)?,
        schema_url: reader.string("schemaUrl", "schema_url")?,
    };
    reader.finish()?;
    Ok(parsed)
}

fn scope_spans(value: &Value, path: &str) -> Result<trace::ScopeSpans> {
    let mut reader = Reader::new(value, path)?;
    let parsed = trace::ScopeSpans {
        scope: reader.message("scope", "scope", instrumentation_scope)?,
        spans: reader.list("spans", "spans", span)?,
        schema_url: reader.string("schemaUrl", "schema_url")?,
    };
    reader.finish()?;
    Ok(parsed)
}

fn span(value: &Value, path: &str) -> Result<trace::Span> {
    let mut reader = Reader::new(value, path)?;
    let parsed = trace::Span {
        trace_id: reader.hex_id("traceId", "trace_id", 32)?,
        span_id: reader.hex_id("spanId", "span_id", 16)?,
        trace_state: reader.string("traceState", "trace_state")?,
        parent_span_id: reader.hex_id("parentSpanId", "parent_span_id", 16)?,
        flags: reader.u32("flags", "flags")?,
        name: reader.string("name", "name")?,
        kind: reader.enumeration("kind", "kind", |name| {
            trace::span::SpanKind::from_str_name(name).map(|value| value as i32)
        })?,
        start_time_unix_nano: reader.u64("startTimeUnixNano", "start_time_unix_nano")?,
        end_time_unix_nano: reader.u64("endTimeUnixNano", "end_time_unix_nano")?,
        attributes: reader.list("attributes", "attributes", key_value)?,
        dropped_attributes_count: reader
            .u32("droppedAttributesCount", "dropped_attributes_count")?,
        events: reader.list("events", "events", span_event)?,
        dropped_events_count: reader.u32("droppedEventsCount", "dropped_events_count")?,
        links: reader.list("links", "links", span_link)?,
        dropped_links_count: reader.u32("droppedLinksCount", "dropped_links_count")?,
        status: reader.message("status", "status", span_status)?,
    };
    reader.finish()?;
    Ok(parsed)
}

fn span_event(value: &Value, path: &str) -> Result<trace::span::Event> {
    let mut reader = Reader::new(value, path)?;
    let parsed = trace::span::Event {
        time_unix_nano: reader.u64("timeUnixNano", "time_unix_nano")?,
        name: reader.string("name", "name")?,
        attributes: reader.list("attributes", "attributes", key_value)?,
        dropped_attributes_count: reader
            .u32("droppedAttributesCount", "dropped_attributes_count")?,
    };
    reader.finish()?;
    Ok(parsed)
}

fn span_link(value: &Value, path: &str) -> Result<trace::span::Link> {
    let mut reader = Reader::new(value, path)?;
    let parsed = trace::span::Link {
        trace_id: reader.hex_id_vec("traceId", "trace_id", 32)?,
        span_id: reader.hex_id_vec("spanId", "span_id", 16)?,
        trace_state: reader.string("traceState", "trace_state")?,
        attributes: reader.list("attributes", "attributes", key_value)?,
        dropped_attributes_count: reader
            .u32("droppedAttributesCount", "dropped_attributes_count")?,
        flags: reader.u32("flags", "flags")?,
    };
    reader.finish()?;
    Ok(parsed)
}

fn span_status(value: &Value, path: &str) -> Result<trace::Status> {
    let mut reader = Reader::new(value, path)?;
    let parsed = trace::Status {
        message: reader.string("message", "message")?,
        code: reader.enumeration("code", "code", |name| {
            trace::status::StatusCode::from_str_name(name).map(|value| value as i32)
        })?,
    };
    reader.finish()?;
    Ok(parsed)
}

// --- metrics ---------------------------------------------------------------

fn resource_metrics(value: &Value, path: &str) -> Result<metrics::ResourceMetrics> {
    let mut reader = Reader::new(value, path)?;
    let parsed = metrics::ResourceMetrics {
        resource: reader.message("resource", "resource", resource_message)?,
        scope_metrics: reader.list("scopeMetrics", "scope_metrics", scope_metrics)?,
        schema_url: reader.string("schemaUrl", "schema_url")?,
    };
    reader.finish()?;
    Ok(parsed)
}

fn scope_metrics(value: &Value, path: &str) -> Result<metrics::ScopeMetrics> {
    let mut reader = Reader::new(value, path)?;
    let parsed = metrics::ScopeMetrics {
        scope: reader.message("scope", "scope", instrumentation_scope)?,
        metrics: reader.list("metrics", "metrics", metric)?,
        schema_url: reader.string("schemaUrl", "schema_url")?,
    };
    reader.finish()?;
    Ok(parsed)
}

fn metric(value: &Value, path: &str) -> Result<metrics::Metric> {
    let mut reader = Reader::new(value, path)?;
    let name = reader.string("name", "name")?;
    let description = reader.string("description", "description")?;
    let unit = reader.string("unit", "unit")?;
    let metadata = reader.list("metadata", "metadata", key_value)?;

    let mut data: Option<metrics::metric::Data> = None;
    let mut set = |candidate: metrics::metric::Data| -> Result<()> {
        if data.is_some() {
            return Err(malformed(path, "exactly one metric data member"));
        }
        data = Some(candidate);
        Ok(())
    };
    if let Some(inner) = reader.take("gauge", "gauge") {
        let mut nested = Reader::new(inner, &format!("{path}/gauge"))?;
        let gauge = metrics::Gauge {
            data_points: nested.list("dataPoints", "data_points", number_data_point)?,
        };
        nested.finish()?;
        set(metrics::metric::Data::Gauge(gauge))?;
    }
    if let Some(inner) = reader.take("sum", "sum") {
        let mut nested = Reader::new(inner, &format!("{path}/sum"))?;
        let sum = metrics::Sum {
            data_points: nested.list("dataPoints", "data_points", number_data_point)?,
            aggregation_temporality: nested.enumeration(
                "aggregationTemporality",
                "aggregation_temporality",
                temporality_from_name,
            )?,
            is_monotonic: nested.bool("isMonotonic", "is_monotonic")?,
        };
        nested.finish()?;
        set(metrics::metric::Data::Sum(sum))?;
    }
    if let Some(inner) = reader.take("histogram", "histogram") {
        let mut nested = Reader::new(inner, &format!("{path}/histogram"))?;
        let histogram = metrics::Histogram {
            data_points: nested.list("dataPoints", "data_points", histogram_data_point)?,
            aggregation_temporality: nested.enumeration(
                "aggregationTemporality",
                "aggregation_temporality",
                temporality_from_name,
            )?,
        };
        nested.finish()?;
        set(metrics::metric::Data::Histogram(histogram))?;
    }
    if let Some(inner) = reader.take("exponentialHistogram", "exponential_histogram") {
        let mut nested = Reader::new(inner, &format!("{path}/exponentialHistogram"))?;
        let histogram = metrics::ExponentialHistogram {
            data_points: nested.list(
                "dataPoints",
                "data_points",
                exponential_histogram_data_point,
            )?,
            aggregation_temporality: nested.enumeration(
                "aggregationTemporality",
                "aggregation_temporality",
                temporality_from_name,
            )?,
        };
        nested.finish()?;
        set(metrics::metric::Data::ExponentialHistogram(histogram))?;
    }
    if let Some(inner) = reader.take("summary", "summary") {
        let mut nested = Reader::new(inner, &format!("{path}/summary"))?;
        let summary = metrics::Summary {
            data_points: nested.list("dataPoints", "data_points", summary_data_point)?,
        };
        nested.finish()?;
        set(metrics::metric::Data::Summary(summary))?;
    }
    reader.finish()?;
    Ok(metrics::Metric {
        name,
        description,
        unit,
        metadata,
        data,
    })
}

fn temporality_from_name(name: &str) -> Option<i32> {
    metrics::AggregationTemporality::from_str_name(name).map(|value| value as i32)
}

fn number_data_point(value: &Value, path: &str) -> Result<metrics::NumberDataPoint> {
    let mut reader = Reader::new(value, path)?;
    let attributes = reader.list("attributes", "attributes", key_value)?;
    let start_time_unix_nano = reader.u64("startTimeUnixNano", "start_time_unix_nano")?;
    let time_unix_nano = reader.u64("timeUnixNano", "time_unix_nano")?;
    let exemplars = reader.list("exemplars", "exemplars", exemplar)?;
    let flags = reader.u32("flags", "flags")?;
    let mut point_value: Option<metrics::number_data_point::Value> = None;
    if let Some(inner) = reader.take("asDouble", "as_double") {
        point_value = Some(metrics::number_data_point::Value::AsDouble(double(
            inner, path,
        )?));
    }
    if let Some(inner) = reader.take("asInt", "as_int") {
        if point_value.is_some() {
            return Err(malformed(path, "exactly one data point value"));
        }
        let raw = integer64(inner, path)?;
        point_value = Some(metrics::number_data_point::Value::AsInt(
            i64::try_from(raw).map_err(|_| malformed(path, "signed 64-bit integer"))?,
        ));
    }
    reader.finish()?;
    Ok(metrics::NumberDataPoint {
        attributes,
        start_time_unix_nano,
        time_unix_nano,
        exemplars,
        flags,
        value: point_value,
    })
}

fn histogram_data_point(value: &Value, path: &str) -> Result<metrics::HistogramDataPoint> {
    let mut reader = Reader::new(value, path)?;
    let parsed = metrics::HistogramDataPoint {
        attributes: reader.list("attributes", "attributes", key_value)?,
        start_time_unix_nano: reader.u64("startTimeUnixNano", "start_time_unix_nano")?,
        time_unix_nano: reader.u64("timeUnixNano", "time_unix_nano")?,
        count: reader.u64("count", "count")?,
        sum: reader.optional_f64("sum", "sum")?,
        bucket_counts: reader.u64_list("bucketCounts", "bucket_counts")?,
        explicit_bounds: reader.f64_list("explicitBounds", "explicit_bounds")?,
        exemplars: reader.list("exemplars", "exemplars", exemplar)?,
        flags: reader.u32("flags", "flags")?,
        min: reader.optional_f64("min", "min")?,
        max: reader.optional_f64("max", "max")?,
    };
    reader.finish()?;
    Ok(parsed)
}

fn exponential_histogram_data_point(
    value: &Value,
    path: &str,
) -> Result<metrics::ExponentialHistogramDataPoint> {
    let mut reader = Reader::new(value, path)?;
    let parsed = metrics::ExponentialHistogramDataPoint {
        attributes: reader.list("attributes", "attributes", key_value)?,
        start_time_unix_nano: reader.u64("startTimeUnixNano", "start_time_unix_nano")?,
        time_unix_nano: reader.u64("timeUnixNano", "time_unix_nano")?,
        count: reader.u64("count", "count")?,
        sum: reader.optional_f64("sum", "sum")?,
        scale: reader.i32("scale", "scale")?,
        zero_count: reader.u64("zeroCount", "zero_count")?,
        positive: reader.message("positive", "positive", buckets)?,
        negative: reader.message("negative", "negative", buckets)?,
        flags: reader.u32("flags", "flags")?,
        exemplars: reader.list("exemplars", "exemplars", exemplar)?,
        min: reader.optional_f64("min", "min")?,
        max: reader.optional_f64("max", "max")?,
        zero_threshold: reader.f64("zeroThreshold", "zero_threshold")?,
    };
    reader.finish()?;
    Ok(parsed)
}

fn buckets(
    value: &Value,
    path: &str,
) -> Result<metrics::exponential_histogram_data_point::Buckets> {
    let mut reader = Reader::new(value, path)?;
    let parsed = metrics::exponential_histogram_data_point::Buckets {
        offset: reader.i32("offset", "offset")?,
        bucket_counts: reader.u64_list("bucketCounts", "bucket_counts")?,
    };
    reader.finish()?;
    Ok(parsed)
}

fn summary_data_point(value: &Value, path: &str) -> Result<metrics::SummaryDataPoint> {
    let mut reader = Reader::new(value, path)?;
    let parsed = metrics::SummaryDataPoint {
        attributes: reader.list("attributes", "attributes", key_value)?,
        start_time_unix_nano: reader.u64("startTimeUnixNano", "start_time_unix_nano")?,
        time_unix_nano: reader.u64("timeUnixNano", "time_unix_nano")?,
        count: reader.u64("count", "count")?,
        sum: reader.f64("sum", "sum")?,
        quantile_values: reader.list("quantileValues", "quantile_values", value_at_quantile)?,
        flags: reader.u32("flags", "flags")?,
    };
    reader.finish()?;
    Ok(parsed)
}

fn value_at_quantile(
    value: &Value,
    path: &str,
) -> Result<metrics::summary_data_point::ValueAtQuantile> {
    let mut reader = Reader::new(value, path)?;
    let parsed = metrics::summary_data_point::ValueAtQuantile {
        quantile: reader.f64("quantile", "quantile")?,
        value: reader.f64("value", "value")?,
    };
    reader.finish()?;
    Ok(parsed)
}

fn exemplar(value: &Value, path: &str) -> Result<metrics::Exemplar> {
    let mut reader = Reader::new(value, path)?;
    let filtered_attributes =
        reader.list("filteredAttributes", "filtered_attributes", key_value)?;
    let time_unix_nano = reader.u64("timeUnixNano", "time_unix_nano")?;
    let span_id = reader.hex_id("spanId", "span_id", 16)?;
    let trace_id = reader.hex_id("traceId", "trace_id", 32)?;
    let mut exemplar_value: Option<metrics::exemplar::Value> = None;
    if let Some(inner) = reader.take("asDouble", "as_double") {
        exemplar_value = Some(metrics::exemplar::Value::AsDouble(double(inner, path)?));
    }
    if let Some(inner) = reader.take("asInt", "as_int") {
        if exemplar_value.is_some() {
            return Err(malformed(path, "exactly one exemplar value"));
        }
        let raw = integer64(inner, path)?;
        exemplar_value = Some(metrics::exemplar::Value::AsInt(
            i64::try_from(raw).map_err(|_| malformed(path, "signed 64-bit integer"))?,
        ));
    }
    reader.finish()?;
    Ok(metrics::Exemplar {
        filtered_attributes,
        time_unix_nano,
        span_id,
        trace_id,
        value: exemplar_value,
    })
}

#[cfg(test)]
mod tests {
    use super::decode_json;
    use crate::decode::DecodedBatch;
    use crate::error::{OtlpError, OtlpSignal};

    #[test]
    fn a_log_record_decodes_with_string_and_numeric_sixty_four_bit_integers() {
        let numeric = br#"{"resourceLogs":[{"scopeLogs":[{"logRecords":[
            {"timeUnixNano":1700000000000000000,"severityNumber":9,
             "body":{"stringValue":"hello"}}]}]}]}"#;
        let stringy = br#"{"resourceLogs":[{"scopeLogs":[{"logRecords":[
            {"timeUnixNano":"1700000000000000000","severityNumber":"SEVERITY_NUMBER_INFO",
             "body":{"stringValue":"hello"}}]}]}]}"#;
        let left = decode_json(OtlpSignal::Logs, numeric).expect("decodes");
        let right = decode_json(OtlpSignal::Logs, stringy).expect("decodes");
        assert_eq!(
            left, right,
            "number and decimal-string forms are the same value"
        );
        assert_eq!(left.record_count(), 1);
    }

    #[test]
    fn an_unknown_member_is_rejected_rather_than_ignored() {
        let body = br#"{"resourceLogs":[{"scopeLogs":[{"logRecords":[{"nope":1}]}]}]}"#;
        let error = decode_json(OtlpSignal::Logs, body).expect_err("rejected");
        assert!(
            matches!(&error, OtlpError::UnknownField { field, .. } if &**field == "nope"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn an_identifier_must_be_exactly_lowercase_hex_of_the_declared_width() {
        for hostile in [r#""AABBCCDDEEFF00112233445566778899""#, r#""aabb""#, "12"] {
            let body = format!(
                r#"{{"resourceSpans":[{{"scopeSpans":[{{"spans":[{{"traceId":{hostile}}}]}}]}}]}}"#
            );
            assert!(
                decode_json(OtlpSignal::Traces, body.as_bytes()).is_err(),
                "`{hostile}` must be refused"
            );
        }
        let body = br#"{"resourceSpans":[{"scopeSpans":[{"spans":[
            {"traceId":"aabbccddeeff00112233445566778899","spanId":"0011223344556677"}]}]}]}"#;
        let batch = decode_json(OtlpSignal::Traces, body).expect("decodes");
        let DecodedBatch::Traces(request) = batch else {
            panic!("traces");
        };
        assert_eq!(
            request.resource_spans[0].scope_spans[0].spans[0]
                .trace_id
                .len(),
            16
        );
    }

    #[test]
    fn a_non_finite_double_is_refused() {
        let body = br#"{"resourceMetrics":[{"scopeMetrics":[{"metrics":[
            {"name":"m","gauge":{"dataPoints":[{"asDouble":"NaN"}]}}]}]}]}"#;
        assert!(decode_json(OtlpSignal::Metrics, body).is_err());
    }

    #[test]
    fn every_metric_shape_decodes_and_counts_its_points() {
        let body = br#"{"resourceMetrics":[{"scopeMetrics":[{"metrics":[
            {"name":"g","gauge":{"dataPoints":[{"asInt":"1"}]}},
            {"name":"s","sum":{"dataPoints":[{"asDouble":1.5}],
                "aggregationTemporality":"AGGREGATION_TEMPORALITY_DELTA","isMonotonic":true}},
            {"name":"h","histogram":{"dataPoints":[{"count":"2","sum":3.0,
                "bucketCounts":["1","1"],"explicitBounds":[1.0]}],
                "aggregationTemporality":2}},
            {"name":"e","exponentialHistogram":{"dataPoints":[{"count":"1","scale":0,
                "zeroCount":"0","positive":{"offset":0,"bucketCounts":["1"]}}],
                "aggregationTemporality":1}},
            {"name":"q","summary":{"dataPoints":[{"count":"1","sum":1.0,
                "quantileValues":[{"quantile":0.5,"value":1.0}]}]}}
        ]}]}]}"#;
        let batch = decode_json(OtlpSignal::Metrics, body).expect("decodes");
        assert_eq!(batch.record_count(), 5);
    }

    #[test]
    fn two_metric_data_members_are_refused() {
        let body = br#"{"resourceMetrics":[{"scopeMetrics":[{"metrics":[
            {"name":"m","gauge":{"dataPoints":[]},"sum":{"dataPoints":[]}}]}]}]}"#;
        assert!(decode_json(OtlpSignal::Metrics, body).is_err());
    }
}
