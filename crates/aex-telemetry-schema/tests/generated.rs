//! Evidence that the committed generated source is exactly what the registry
//! renders, and that the reserved-name and classification rules hold for
//! arbitrary inputs rather than only for the names that happen to be declared.

use aex_telemetry_schema::{
    codegen,
    generated::{ATTRIBUTES, EVENTS, METRICS, RESERVED_PREFIXES, SCHEMA_VERSION, SPANS},
    is_reserved,
    registry::Registry,
    spec::Visibility,
};
use proptest::prelude::*;

const COMMITTED: &str = include_str!("../src/generated.rs");

#[test]
fn generated_source_is_byte_identical() {
    let registry = Registry::embedded().expect("the committed registry parses");
    let rendered = codegen::render(&registry);
    assert_eq!(
        rendered, COMMITTED,
        "src/generated.rs is stale; rerun `cargo run -p aex-telemetry-schema --example regenerate`"
    );
}

#[test]
fn generated_source_matches_the_registry_contents() {
    let registry = Registry::embedded().expect("the committed registry parses");
    assert_eq!(SCHEMA_VERSION, registry.schema_version);
    assert_eq!(ATTRIBUTES.len(), registry.attributes.len());
    assert_eq!(METRICS.len(), registry.metrics.len());
    assert_eq!(SPANS.len(), registry.spans.len());
    assert_eq!(EVENTS.len(), registry.events.len());
    assert_eq!(RESERVED_PREFIXES.len(), registry.reserved_prefixes.len());
}

#[test]
fn exportable_attributes_are_exactly_the_public_ones() {
    for spec in ATTRIBUTES {
        assert_eq!(
            aex_telemetry_schema::is_exportable(spec.name),
            spec.visibility == Visibility::Public,
            "{}",
            spec.name
        );
    }
}

fn suffix() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::char::range('a', 'z'), 0..24)
        .prop_map(|characters| characters.into_iter().collect())
}

proptest! {
    /// Anything built from a reserved prefix stays reserved, whatever follows it.
    #[test]
    fn reserved_prefixes_capture_every_extension(index in 0_usize..8, tail in suffix()) {
        let prefix = RESERVED_PREFIXES[index % RESERVED_PREFIXES.len()];
        let candidate = format!("{prefix}{tail}");
        prop_assert!(is_reserved(&candidate));
    }

    /// No declared name can ever be reserved, whichever declaration is inspected.
    #[test]
    fn declared_names_are_never_reserved(index in any::<usize>()) {
        let spec = ATTRIBUTES[index % ATTRIBUTES.len()];
        prop_assert!(!is_reserved(spec.name));
        let spec = METRICS[index % METRICS.len()];
        prop_assert!(!is_reserved(spec.name));
        let spec = SPANS[index % SPANS.len()];
        prop_assert!(!is_reserved(spec.name));
        let spec = EVENTS[index % EVENTS.len()];
        prop_assert!(!is_reserved(spec.name));
    }

    /// A name has at most one declaration, so it has at most one cardinality
    /// class and at most one visibility class.
    #[test]
    fn every_name_has_at_most_one_classification(name in "[a-z.]{0,32}") {
        let declared = ATTRIBUTES.iter().filter(|spec| spec.name == name).count();
        prop_assert!(declared <= 1);
        let classified = usize::from(aex_telemetry_schema::visibility_of(&name).is_some())
            + usize::from(aex_telemetry_schema::cardinality_of(&name).is_some());
        prop_assert_eq!(classified, declared * 2);
    }

    /// An undeclared name is never exportable and never a metric dimension.
    #[test]
    fn undeclared_names_fail_closed(tail in suffix()) {
        let name = format!("undeclared.{tail}");
        prop_assume!(ATTRIBUTES.iter().all(|spec| spec.name != name));
        prop_assert!(!aex_telemetry_schema::is_exportable(&name));
        prop_assert!(!aex_telemetry_schema::is_metric_dimension(&name));
    }
}
