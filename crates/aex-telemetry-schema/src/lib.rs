//! `aex-telemetry-schema` owns the internal operational telemetry vocabulary: a
//! reviewed declarative registry of attribute, instrument, span and event names,
//! the cardinality and visibility class of every attribute, and the reserved-name
//! rules that keep product code from colliding with runtime-owned names.
//!
//! The registry is a vocabulary and policy compiler, not an event bus. It fixes
//! names, units, allowed attributes, cardinality and sensitivity so two services
//! cannot invent incompatible `session_id` and `sessionId` fields, and so an
//! unbounded customer identifier cannot become a metric dimension.
//!
//! # Invariants
//!
//! - every declared attribute carries exactly one cardinality class and exactly
//!   one visibility class; an unclassified attribute does not parse
//! - no declared name reuses a reserved prefix, and that is proved while the
//!   crate compiles, not at start-up
//! - `src/generated.rs` is byte-identical to a fresh render of
//!   `telemetry/registry.toml`; a hand edit is a test failure
//! - a signal may only reference attributes the registry declares
//!
//! # Not this crate's job
//!
//! - exporting, batching or service initialisation: that is
//!   `aex-platform-telemetry`, which is the only crate permitted to depend on an
//!   `OpenTelemetry` implementation
//! - customer-visible observations: those go through `aex-observation-application`
//!   and obey its durable admission contract
//! - deciding whether a given deployable emits a given signal; that coverage
//!   requirement is recorded per deployable, not here

pub mod codegen;
pub mod generated;
pub mod registry;
pub mod reserved;
pub mod spec;

use spec::{AttributeSpec, Cardinality, Visibility};

/// Whether `name` reuses a prefix owned by the telemetry runtime.
#[must_use]
pub const fn is_reserved(name: &str) -> bool {
    reserved::matches_any(name, generated::RESERVED_PREFIXES)
}

/// The declared specification of `name`, if the registry declares it.
#[must_use]
pub fn attribute(name: &str) -> Option<&'static AttributeSpec> {
    generated::ATTRIBUTES.iter().find(|spec| spec.name == name)
}

/// The visibility class of `name`.
///
/// An undeclared attribute has no class. Callers that redact must treat that as
/// "not exportable": an attribute nobody classified is not evidence that it is
/// safe to publish.
#[must_use]
pub fn visibility_of(name: &str) -> Option<Visibility> {
    attribute(name).map(|spec| spec.visibility)
}

/// The cardinality class of `name`, or `None` when the registry does not declare it.
#[must_use]
pub fn cardinality_of(name: &str) -> Option<Cardinality> {
    attribute(name).map(|spec| spec.cardinality)
}

/// Whether `name` is declared and classified as safe to export.
///
/// Fails closed: an undeclared name is never exportable.
#[must_use]
pub fn is_exportable(name: &str) -> bool {
    visibility_of(name).is_some_and(Visibility::is_exportable)
}

/// Whether `name` may be used as a metric dimension.
///
/// Fails closed: an undeclared name, an unbounded name and a non-public name are
/// all rejected.
#[must_use]
pub fn is_metric_dimension(name: &str) -> bool {
    match attribute(name) {
        Some(spec) => {
            spec.visibility.is_exportable() && !matches!(spec.cardinality, Cardinality::Unbounded)
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        attribute, cardinality_of, generated, is_exportable, is_metric_dimension, is_reserved,
        visibility_of,
    };
    use crate::spec::{Cardinality, Visibility};

    #[test]
    fn every_attribute_has_exactly_one_classification() {
        for spec in generated::ATTRIBUTES {
            assert_eq!(
                cardinality_of(spec.name),
                Some(spec.cardinality),
                "{}",
                spec.name
            );
            assert_eq!(
                visibility_of(spec.name),
                Some(spec.visibility),
                "{}",
                spec.name
            );
            let matches = generated::ATTRIBUTES
                .iter()
                .filter(|other| other.name == spec.name);
            assert_eq!(
                matches.count(),
                1,
                "`{}` is declared more than once",
                spec.name
            );
        }
    }

    #[test]
    fn attributes_are_sorted_by_name() {
        let mut previous: Option<&str> = None;
        for spec in generated::ATTRIBUTES {
            if let Some(previous) = previous {
                assert!(
                    previous < spec.name,
                    "`{previous}` must sort before `{}`",
                    spec.name
                );
            }
            previous = Some(spec.name);
        }
    }

    #[test]
    fn no_declared_name_reuses_a_reserved_prefix() {
        for spec in generated::ATTRIBUTES {
            assert!(!is_reserved(spec.name), "{}", spec.name);
        }
        for spec in generated::METRICS {
            assert!(!is_reserved(spec.name), "{}", spec.name);
        }
        for spec in generated::SPANS {
            assert!(!is_reserved(spec.name), "{}", spec.name);
        }
        for spec in generated::EVENTS {
            assert!(!is_reserved(spec.name), "{}", spec.name);
        }
    }

    #[test]
    fn every_signal_references_declared_attributes_only() {
        let declared = |name: &str| attribute(name).is_some();
        for spec in generated::METRICS {
            for name in spec.attributes {
                assert!(declared(name), "metric `{}` references `{name}`", spec.name);
                assert!(
                    is_metric_dimension(name),
                    "`{name}` may not be a metric dimension"
                );
            }
        }
        for spec in generated::SPANS {
            for name in spec.attributes {
                assert!(declared(name), "span `{}` references `{name}`", spec.name);
            }
        }
        for spec in generated::EVENTS {
            for name in spec.attributes {
                assert!(declared(name), "event `{}` references `{name}`", spec.name);
            }
        }
    }

    #[test]
    fn classification_fails_closed_for_undeclared_names() {
        assert_eq!(visibility_of("aex.not.declared"), None);
        assert_eq!(cardinality_of("aex.not.declared"), None);
        assert!(!is_exportable("aex.not.declared"));
        assert!(!is_metric_dimension("aex.not.declared"));
    }

    #[test]
    fn identifiers_are_internal_and_never_metric_dimensions() {
        for name in [
            generated::AEX_SESSION_ID,
            generated::AEX_RUN_ID,
            generated::AEX_WORKSPACE_ID,
            generated::AEX_ORGANIZATION_ID,
            generated::AEX_ACTIVATION_ID,
            generated::AEX_REQUEST_ID,
        ] {
            assert_eq!(cardinality_of(name), Some(Cardinality::Unbounded), "{name}");
            assert_eq!(visibility_of(name), Some(Visibility::Internal), "{name}");
            assert!(!is_exportable(name), "{name}");
            assert!(!is_metric_dimension(name), "{name}");
        }
    }

    #[test]
    fn forbidden_attributes_are_declared_so_they_can_be_dropped_by_name() {
        for name in [
            generated::AEX_CUSTOMER_EMAIL,
            generated::AEX_SECRET_PLAINTEXT,
            generated::AEX_PAYMENT_INSTRUMENT,
        ] {
            assert_eq!(visibility_of(name), Some(Visibility::Forbidden), "{name}");
            assert!(!is_exportable(name), "{name}");
        }
    }
}
