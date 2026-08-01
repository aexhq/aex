//! The deterministic renderer that turns a [`Registry`] into `src/generated.rs`.
//!
//! Determinism is the point: every collection is sorted by name before it is
//! rendered, so the same registry always produces byte-identical output. The
//! `generated_source_is_byte_identical` test compares this renderer's output
//! against the committed file, which makes a hand edit of the generated source a
//! test failure rather than a silent divergence.
//!
//! The renderer also has to be a `rustfmt` fixed point, because `cargo fmt
//! --all` runs over the generated file like any other source. [`MAX_WIDTH`] and
//! [`ARRAY_WIDTH`] mirror the two `rustfmt` settings that decide the layouts
//! this file emits; a shape that drifts is caught by the byte-identical test
//! immediately after `cargo fmt`.

use std::fmt::{Arguments, Write as _};

use crate::registry::Registry;

/// `rustfmt`'s `max_width`, as pinned in `rustfmt.toml`.
pub const MAX_WIDTH: usize = 100;

/// `rustfmt`'s `array_width` under the default small-heuristics profile.
///
/// An array whose comma-separated contents are wider than this is laid out one
/// element per line, whatever the total line width.
pub const ARRAY_WIDTH: usize = 60;

/// Renders the committed contents of `src/generated.rs` for `registry`.
///
/// The output always ends with a newline and never contains a carriage return,
/// so the committed file compares equal on every platform.
#[must_use]
pub fn render(registry: &Registry) -> String {
    let mut out = String::with_capacity(16 * 1024);
    out.push_str(HEADER);
    render_prelude(&mut out, registry);
    render_attributes(&mut out, registry);
    render_metrics(&mut out, registry);
    render_spans(&mut out, registry);
    render_events(&mut out, registry);
    out.push_str(&section("compile-time reserved-name rule"));
    out.push_str(FOOTER);
    out
}

/// The Rust constant identifier this crate generates for `name`.
///
/// `aex.plane` with an empty prefix becomes `AEX_PLANE`; `aex.process.started`
/// with prefix `EVENT_` becomes `EVENT_AEX_PROCESS_STARTED`.
#[must_use]
pub fn constant_ident(prefix: &str, name: &str) -> String {
    let mut ident = String::with_capacity(prefix.len() + name.len());
    ident.push_str(prefix);
    for character in name.chars() {
        ident.push(if character == '.' {
            '_'
        } else {
            character.to_ascii_uppercase()
        });
    }
    ident
}

/// Appends formatted text. Writing into a `String` is infallible; the `Result`
/// exists only to satisfy the trait, so discarding it cannot hide a failure.
fn push(out: &mut String, arguments: Arguments<'_>) {
    let _ = out.write_fmt(arguments);
}

/// A `pub const NAME: &str = "value";` item, wrapped the way `rustfmt` wraps it.
fn string_constant(ident: &str, value: &str) -> String {
    let single = format!("pub const {ident}: &str = \"{value}\";");
    if single.len() <= MAX_WIDTH {
        format!("{single}\n")
    } else {
        format!("pub const {ident}: &str =\n    \"{value}\";\n")
    }
}

/// A slice-valued field, laid out the way `rustfmt` lays it out.
fn slice_field(indent: usize, field: &str, items: &[String]) -> String {
    let contents = items.join(", ");
    let single = format!("{:indent$}{field}: &[{contents}],", "");
    if contents.len() <= ARRAY_WIDTH && single.len() <= MAX_WIDTH {
        return format!("{single}\n");
    }
    let mut out = format!("{:indent$}{field}: &[\n", "");
    let inner = indent + 4;
    for item in items {
        push(&mut out, format_args!("{:inner$}{item},\n", ""));
    }
    push(&mut out, format_args!("{:indent$}],\n", ""));
    out
}

fn render_prelude(out: &mut String, registry: &Registry) {
    out.push_str("/// Registry schema version this file was generated from.\n");
    push(
        out,
        format_args!(
            "pub const SCHEMA_VERSION: &str = \"{}\";\n\n",
            registry.schema_version
        ),
    );
    out.push_str("/// Name prefixes owned by the telemetry runtime. Product code may not reuse\n");
    out.push_str(
        "/// them; [`ATTRIBUTES`], [`METRICS`], [`SPANS`] and [`EVENTS`] are proved free\n",
    );
    out.push_str("/// of them at compile time by the `const` block at the end of this file.\n");
    let mut prefixes: Vec<String> = registry
        .reserved_prefixes
        .iter()
        .map(|prefix| format!("\"{prefix}\""))
        .collect();
    prefixes.sort_unstable();
    let contents = prefixes.join(", ");
    let single = format!("pub const RESERVED_PREFIXES: &[&str] = &[{contents}];");
    if contents.len() <= ARRAY_WIDTH && single.len() <= MAX_WIDTH {
        push(out, format_args!("{single}\n"));
    } else {
        out.push_str("pub const RESERVED_PREFIXES: &[&str] = &[\n");
        for prefix in &prefixes {
            push(out, format_args!("    {prefix},\n"));
        }
        out.push_str("];\n");
    }
}

fn render_attributes(out: &mut String, registry: &Registry) {
    out.push_str(&section("attribute names"));
    let mut attributes: Vec<_> = registry.attributes.iter().collect();
    attributes.sort_unstable_by(|left, right| left.name.cmp(&right.name));
    for attribute in &attributes {
        push(
            out,
            format_args!(
                "/// `{name}` — {brief}\n///\n/// Cardinality: `{cardinality}`. Visibility: \
                 `{visibility}`. Maximum length: {max_len} bytes.\n",
                name = attribute.name,
                brief = attribute.brief,
                cardinality = attribute.cardinality.as_str(),
                visibility = attribute.visibility.as_str(),
                max_len = attribute.max_len,
            ),
        );
        out.push_str(&string_constant(
            &constant_ident("", &attribute.name),
            &attribute.name,
        ));
        out.push('\n');
    }
    out.push_str("/// Every declared attribute, ordered by name.\n");
    out.push_str("pub const ATTRIBUTES: &[AttributeSpec] = &[\n");
    for attribute in &attributes {
        push(
            out,
            format_args!(
                "    AttributeSpec {{\n        name: {ident},\n        cardinality: \
                 {cardinality},\n        visibility: {visibility},\n        max_len: \
                 {max_len},\n    }},\n",
                ident = constant_ident("", &attribute.name),
                cardinality = attribute.cardinality.variant(),
                visibility = attribute.visibility.variant(),
                max_len = attribute.max_len,
            ),
        );
    }
    out.push_str("];\n");
}

fn render_metrics(out: &mut String, registry: &Registry) {
    out.push_str(&section("instrument names"));
    let mut metrics: Vec<_> = registry.metrics.iter().collect();
    metrics.sort_unstable_by(|left, right| left.name.cmp(&right.name));
    for metric in &metrics {
        push(
            out,
            format_args!(
                "/// `{name}` — {brief}\n///\n/// Instrument: `{instrument}`. Unit: `{unit}`.\n",
                name = metric.name,
                brief = metric.brief,
                instrument = metric.instrument.as_str(),
                unit = metric.unit,
            ),
        );
        out.push_str(&string_constant(
            &constant_ident("METRIC_", &metric.name),
            &metric.name,
        ));
        out.push('\n');
    }
    out.push_str("/// Every declared instrument, ordered by name.\n");
    out.push_str("pub const METRICS: &[MetricSpec] = &[\n");
    for metric in &metrics {
        push(
            out,
            format_args!(
                "    MetricSpec {{\n        name: {ident},\n        instrument: \
                 {instrument},\n        unit: \"{unit}\",\n",
                ident = constant_ident("METRIC_", &metric.name),
                instrument = metric.instrument.variant(),
                unit = metric.unit,
            ),
        );
        out.push_str(&slice_field(
            8,
            "attributes",
            &attribute_idents(&metric.attributes),
        ));
        out.push_str("    },\n");
    }
    out.push_str("];\n");
}

fn render_spans(out: &mut String, registry: &Registry) {
    out.push_str(&section("span names"));
    let mut spans: Vec<_> = registry.spans.iter().collect();
    spans.sort_unstable_by(|left, right| left.name.cmp(&right.name));
    for span in &spans {
        push(out, format_args!("/// `{}` — {}\n", span.name, span.brief));
        out.push_str(&string_constant(
            &constant_ident("SPAN_", &span.name),
            &span.name,
        ));
        out.push('\n');
    }
    out.push_str("/// Every declared span, ordered by name.\n");
    out.push_str("pub const SPANS: &[SpanSpec] = &[\n");
    for span in &spans {
        push(
            out,
            format_args!(
                "    SpanSpec {{\n        name: {ident},\n",
                ident = constant_ident("SPAN_", &span.name),
            ),
        );
        out.push_str(&slice_field(
            8,
            "attributes",
            &attribute_idents(&span.attributes),
        ));
        out.push_str("    },\n");
    }
    out.push_str("];\n");
}

fn render_events(out: &mut String, registry: &Registry) {
    out.push_str(&section("event names"));
    let mut events: Vec<_> = registry.events.iter().collect();
    events.sort_unstable_by(|left, right| left.name.cmp(&right.name));
    for event in &events {
        push(
            out,
            format_args!("/// `{}` — {}\n", event.name, event.brief),
        );
        out.push_str(&string_constant(
            &constant_ident("EVENT_", &event.name),
            &event.name,
        ));
        out.push('\n');
    }
    out.push_str("/// Every declared event, ordered by name.\n");
    out.push_str("pub const EVENTS: &[EventSpec] = &[\n");
    for event in &events {
        push(
            out,
            format_args!(
                "    EventSpec {{\n        name: {ident},\n",
                ident = constant_ident("EVENT_", &event.name),
            ),
        );
        out.push_str(&slice_field(
            8,
            "attributes",
            &attribute_idents(&event.attributes),
        ));
        out.push_str("    },\n");
    }
    out.push_str("];\n");
}

fn attribute_idents(attributes: &[String]) -> Vec<String> {
    let mut names: Vec<String> = attributes
        .iter()
        .map(|name| constant_ident("", name))
        .collect();
    names.sort_unstable();
    names
}

fn section(title: &str) -> String {
    let rule = "-".repeat(76_usize.saturating_sub(title.len()));
    format!("\n// --- {title} {rule}\n\n")
}

const HEADER: &str = "\
//! Generated from `telemetry/registry.toml` by `aex_telemetry_schema::codegen`.
//!
//! Do not edit this file. Change `telemetry/registry.toml` and rerun
//! `cargo run -p aex-telemetry-schema --example regenerate`; the
//! `generated_source_is_byte_identical` test fails if the two disagree.

use crate::spec::{
    AttributeSpec, Cardinality, EventSpec, Instrument, MetricSpec, SpanSpec, Visibility,
};

";

const FOOTER: &str = "\
// Every declared name is proved free of every reserved prefix while the crate is
// compiled, so a reserved-prefix collision can never reach a running process.
const _: () = {
    let mut index = 0;
    while index < ATTRIBUTES.len() {
        assert!(
            !crate::reserved::matches_any(ATTRIBUTES[index].name, RESERVED_PREFIXES),
            \"an attribute name reuses a reserved prefix\"
        );
        index += 1;
    }
    let mut index = 0;
    while index < METRICS.len() {
        assert!(
            !crate::reserved::matches_any(METRICS[index].name, RESERVED_PREFIXES),
            \"a metric name reuses a reserved prefix\"
        );
        index += 1;
    }
    let mut index = 0;
    while index < SPANS.len() {
        assert!(
            !crate::reserved::matches_any(SPANS[index].name, RESERVED_PREFIXES),
            \"a span name reuses a reserved prefix\"
        );
        index += 1;
    }
    let mut index = 0;
    while index < EVENTS.len() {
        assert!(
            !crate::reserved::matches_any(EVENTS[index].name, RESERVED_PREFIXES),
            \"an event name reuses a reserved prefix\"
        );
        index += 1;
    }
};
";

#[cfg(test)]
mod tests {
    use super::{ARRAY_WIDTH, MAX_WIDTH, constant_ident, render, slice_field, string_constant};
    use crate::registry::Registry;

    #[test]
    fn constant_identifiers_are_screaming_snake_case() {
        assert_eq!(constant_ident("", "aex.plane"), "AEX_PLANE");
        assert_eq!(
            constant_ident("EVENT_", "aex.process.started"),
            "EVENT_AEX_PROCESS_STARTED"
        );
        assert_eq!(
            constant_ident("METRIC_", "aex.operation.duration"),
            "METRIC_AEX_OPERATION_DURATION"
        );
    }

    #[test]
    fn a_short_constant_stays_on_one_line() {
        assert_eq!(
            string_constant("AEX_PLANE", "aex.plane"),
            "pub const AEX_PLANE: &str = \"aex.plane\";\n"
        );
    }

    #[test]
    fn an_over_wide_constant_wraps_after_the_equals_sign() {
        let ident = "M".repeat(MAX_WIDTH);
        let rendered = string_constant(&ident, "value");
        assert!(rendered.contains("&str =\n    \"value\";"), "{rendered}");
    }

    #[test]
    fn a_narrow_slice_field_stays_on_one_line() {
        let items = vec!["A".to_owned(), "B".to_owned()];
        assert_eq!(
            slice_field(8, "attributes", &items),
            "        attributes: &[A, B],\n"
        );
    }

    #[test]
    fn an_over_wide_slice_field_takes_one_element_per_line() {
        let items: Vec<String> = (0..8)
            .map(|index| format!("ATTRIBUTE_NUMBER_{index}"))
            .collect();
        let rendered = slice_field(8, "attributes", &items);
        assert!(items.join(", ").len() > ARRAY_WIDTH);
        assert!(
            rendered.starts_with("        attributes: &[\n"),
            "{rendered}"
        );
        assert!(
            rendered.contains("            ATTRIBUTE_NUMBER_0,\n"),
            "{rendered}"
        );
        assert!(rendered.ends_with("        ],\n"), "{rendered}");
    }

    #[test]
    fn rendering_is_deterministic() {
        let registry = Registry::embedded().expect("the committed registry parses");
        assert_eq!(render(&registry), render(&registry));
    }

    #[test]
    fn rendered_source_uses_unix_line_endings() {
        let registry = Registry::embedded().expect("the committed registry parses");
        let rendered = render(&registry);
        assert!(
            !rendered.contains('\r'),
            "generated source must not contain a carriage return"
        );
        assert!(
            rendered.ends_with('\n'),
            "generated source must end with a newline"
        );
    }
}
