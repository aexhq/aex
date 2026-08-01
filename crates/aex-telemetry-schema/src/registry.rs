//! The owned, parsed view of `telemetry/registry.toml`.
//!
//! Parsing is strict: an unknown key, a duplicate name, a reserved-prefix reuse
//! or a reference to an undeclared attribute is a typed error. There is no
//! "unclassified attribute" state — every declaration carries exactly one
//! cardinality class and exactly one visibility class or it does not parse.

use std::collections::BTreeSet;

use serde::Deserialize;

use crate::spec::{Cardinality, Instrument, Visibility};

/// The registry source text compiled into this crate.
pub const SOURCE: &str = include_str!("../telemetry/registry.toml");

/// One declared attribute, as written in `registry.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttributeDecl {
    /// Dotted attribute name as it appears on the wire.
    pub name: String,
    /// One-line description carried into the generated documentation.
    pub brief: String,
    /// How many distinct values this attribute may take.
    pub cardinality: Cardinality,
    /// Who may see this attribute once a record leaves the process.
    pub visibility: Visibility,
    /// Maximum permitted rendered length in bytes.
    pub max_len: usize,
}

/// One declared instrument, as written in `registry.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricDecl {
    /// Dotted metric name.
    pub name: String,
    /// One-line description carried into the generated documentation.
    pub brief: String,
    /// Instrument kind.
    pub instrument: Instrument,
    /// `UCUM` unit string.
    pub unit: String,
    /// Attribute names this metric may be dimensioned by.
    pub attributes: Vec<String>,
}

/// One declared span or event, as written in `registry.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignalDecl {
    /// Dotted span or event name.
    pub name: String,
    /// One-line description carried into the generated documentation.
    pub brief: String,
    /// Attribute names this signal may carry.
    pub attributes: Vec<String>,
}

/// The whole registry.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    /// Registry schema version. Bumped when the generated shape changes.
    pub schema_version: String,
    /// Name prefixes owned by the telemetry runtime or an upstream convention.
    pub reserved_prefixes: Vec<String>,
    /// Declared attributes.
    #[serde(default, rename = "attribute")]
    pub attributes: Vec<AttributeDecl>,
    /// Declared instruments.
    #[serde(default, rename = "metric")]
    pub metrics: Vec<MetricDecl>,
    /// Declared spans.
    #[serde(default, rename = "span")]
    pub spans: Vec<SignalDecl>,
    /// Declared events.
    #[serde(default, rename = "event")]
    pub events: Vec<SignalDecl>,
}

/// Why a registry document was rejected.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    /// The document is not well-formed `TOML`, or a declaration is incomplete.
    #[error("registry is not parseable: {0}")]
    Parse(#[from] toml::de::Error),
    /// Two declarations of the same kind share a name.
    #[error("duplicate {kind} name `{name}`")]
    Duplicate {
        /// Which declaration kind the duplicate was found in.
        kind: &'static str,
        /// The repeated name.
        name: String,
    },
    /// A declared name reuses a prefix owned by the telemetry runtime.
    #[error("{kind} name `{name}` reuses reserved prefix `{prefix}`")]
    ReservedPrefix {
        /// Which declaration kind reused the prefix.
        kind: &'static str,
        /// The offending name.
        name: String,
        /// The reserved prefix it reused.
        prefix: String,
    },
    /// A signal or instrument references an attribute that was never declared.
    #[error("{kind} `{name}` references undeclared attribute `{attribute}`")]
    UndeclaredAttribute {
        /// Which declaration kind holds the dangling reference.
        kind: &'static str,
        /// The referencing name.
        name: String,
        /// The attribute that does not exist.
        attribute: String,
    },
    /// Names must be lowercase dotted segments.
    #[error("{kind} name `{name}` is not a lowercase dotted name")]
    MalformedName {
        /// Which declaration kind holds the malformed name.
        kind: &'static str,
        /// The offending name.
        name: String,
    },
}

impl Registry {
    /// Parses and validates a registry document.
    ///
    /// # Errors
    ///
    /// Returns [`RegistryError`] when the document is not parseable, declares a
    /// duplicate or malformed name, reuses a reserved prefix, or references an
    /// attribute that was never declared.
    pub fn parse(source: &str) -> Result<Self, RegistryError> {
        let registry: Self = toml::from_str(source)?;
        registry.validate()?;
        Ok(registry)
    }

    /// Parses and validates the registry compiled into this crate.
    ///
    /// # Errors
    ///
    /// Returns [`RegistryError`] for the same reasons as [`Registry::parse`].
    /// A failure here means the committed registry itself is invalid.
    pub fn embedded() -> Result<Self, RegistryError> {
        Self::parse(SOURCE)
    }

    fn validate(&self) -> Result<(), RegistryError> {
        let mut declared = BTreeSet::new();
        for attribute in &self.attributes {
            self.check_name("attribute", &attribute.name)?;
            if !declared.insert(attribute.name.as_str()) {
                return Err(RegistryError::Duplicate {
                    kind: "attribute",
                    name: attribute.name.clone(),
                });
            }
        }
        self.check_signals(
            "metric",
            self.metrics.iter().map(|m| (&m.name, &m.attributes)),
            &declared,
        )?;
        self.check_signals(
            "span",
            self.spans.iter().map(|s| (&s.name, &s.attributes)),
            &declared,
        )?;
        self.check_signals(
            "event",
            self.events.iter().map(|e| (&e.name, &e.attributes)),
            &declared,
        )?;
        Ok(())
    }

    fn check_signals<'a, I>(
        &self,
        kind: &'static str,
        entries: I,
        declared: &BTreeSet<&str>,
    ) -> Result<(), RegistryError>
    where
        I: Iterator<Item = (&'a String, &'a Vec<String>)>,
    {
        let mut seen = BTreeSet::new();
        for (name, attributes) in entries {
            self.check_name(kind, name)?;
            if !seen.insert(name.as_str()) {
                return Err(RegistryError::Duplicate {
                    kind,
                    name: name.clone(),
                });
            }
            for attribute in attributes {
                if !declared.contains(attribute.as_str()) {
                    return Err(RegistryError::UndeclaredAttribute {
                        kind,
                        name: name.clone(),
                        attribute: attribute.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    fn check_name(&self, kind: &'static str, name: &str) -> Result<(), RegistryError> {
        let well_formed = !name.is_empty()
            && !name.starts_with('.')
            && !name.ends_with('.')
            && !name.contains("..")
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_');
        if !well_formed {
            return Err(RegistryError::MalformedName {
                kind,
                name: name.to_owned(),
            });
        }
        for prefix in &self.reserved_prefixes {
            if name.starts_with(prefix.as_str()) {
                return Err(RegistryError::ReservedPrefix {
                    kind,
                    name: name.to_owned(),
                    prefix: prefix.clone(),
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Registry, RegistryError};

    const MINIMAL: &str = r#"
schema_version = "1"
reserved_prefixes = ["otel."]

[[attribute]]
name = "aex.plane"
brief = "plane"
cardinality = "fixed"
visibility = "public"
max_len = 8
"#;

    #[test]
    fn embedded_registry_is_valid() {
        let registry = Registry::embedded().expect("the committed registry parses");
        assert_eq!(registry.schema_version, "1");
        assert!(!registry.attributes.is_empty());
        assert!(!registry.metrics.is_empty());
        assert!(!registry.spans.is_empty());
        assert!(!registry.events.is_empty());
    }

    #[test]
    fn rejects_a_duplicate_attribute() {
        let source = format!(
            "{MINIMAL}\n[[attribute]]\nname = \"aex.plane\"\nbrief = \"again\"\n\
             cardinality = \"fixed\"\nvisibility = \"public\"\nmax_len = 8\n"
        );
        let error = Registry::parse(&source).expect_err("a duplicate attribute is rejected");
        assert!(
            matches!(
                error,
                RegistryError::Duplicate {
                    kind: "attribute",
                    ..
                }
            ),
            "{error:?}"
        );
    }

    #[test]
    fn rejects_a_reserved_prefix() {
        let source = MINIMAL.replace("aex.plane", "otel.scope.name");
        let error = Registry::parse(&source).expect_err("a reserved prefix is rejected");
        assert!(
            matches!(error, RegistryError::ReservedPrefix { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn rejects_an_undeclared_attribute_reference() {
        let source = format!(
            "{MINIMAL}\n[[span]]\nname = \"aex.op\"\nbrief = \"op\"\n\
             attributes = [\"aex.absent\"]\n"
        );
        let error = Registry::parse(&source).expect_err("a dangling reference is rejected");
        assert!(
            matches!(
                error,
                RegistryError::UndeclaredAttribute { kind: "span", .. }
            ),
            "{error:?}"
        );
    }

    #[test]
    fn rejects_a_malformed_name() {
        let source = MINIMAL.replace("aex.plane", "aex..plane");
        let error = Registry::parse(&source).expect_err("a malformed name is rejected");
        assert!(
            matches!(error, RegistryError::MalformedName { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn rejects_an_unclassified_attribute() {
        let source = MINIMAL.replace("visibility = \"public\"\n", "");
        let error = Registry::parse(&source).expect_err("an unclassified attribute is rejected");
        assert!(matches!(error, RegistryError::Parse(_)), "{error:?}");
    }

    #[test]
    fn rejects_an_unknown_key() {
        let source = format!("{MINIMAL}wat = true\n");
        let error = Registry::parse(&source).expect_err("an unknown key is rejected");
        assert!(matches!(error, RegistryError::Parse(_)), "{error:?}");
    }
}
