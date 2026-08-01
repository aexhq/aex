//! The `[package.metadata.aex]` schema.
//!
//! One parser, here, shared by every rule. The key set is **closed**: a key
//! outside it is a failure rather than a comment, because a metadata block
//! nobody validates is a metadata block nobody maintains.
//!
//! Thirteen keys. Nine were fixed by the delivery plan (`role`, `owner`,
//! `layers`, `concerns`, `seams`, `live_suite`, `scenarios`, `security_tier`,
//! `risk`) and are adopted verbatim; four were added by the test-architecture
//! plan (`artifact`, `deployable`, `targets`, `not_applicable`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::policy::Policy;
use crate::rules::Violation;

/// The closed key set, in declaration order.
pub const KEYS: &[&str] = &[
    "owner",
    "role",
    "artifact",
    "deployable",
    "live_suite",
    "layers",
    "concerns",
    "seams",
    "security_tier",
    "risk",
    "scenarios",
    "targets",
    "not_applicable",
];

/// One package's declared test ownership.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AexMeta {
    /// The owning stream.
    pub owner: String,
    /// What kind of package this is.
    pub role: String,
    /// What it produces, if anything.
    pub artifact: String,
    /// The deployable this package is or companions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deployable: Option<String>,
    /// The live companion that claims this package's live seams.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_suite: Option<String>,
    /// The layers this package owes evidence at.
    #[serde(default)]
    pub layers: Vec<String>,
    /// The concerns this package owes evidence for.
    #[serde(default)]
    pub concerns: Vec<String>,
    /// The external seams it touches, by id.
    #[serde(default)]
    pub seams: Vec<String>,
    /// What a defect here would expose.
    pub security_tier: String,
    /// What drives the advanced tool selection.
    #[serde(default)]
    pub risk: Vec<String>,
    /// The scenarios that observe it.
    #[serde(default)]
    pub scenarios: Vec<String>,
    /// Test target name to layer.
    #[serde(default)]
    pub targets: BTreeMap<String, String>,
    /// Field name to the structural reason it does not apply.
    #[serde(default)]
    pub not_applicable: BTreeMap<String, String>,
}

/// Where a metadata block came from, for the failure message.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Subject {
    /// The path that names the package, as it appears in a message.
    pub path: String,
    /// The package name.
    pub name: String,
}

impl AexMeta {
    /// Parses and validates one block.
    ///
    /// Returns the parsed block when every key and value is inside the schema,
    /// and the violations otherwise. Both are returned so one bad value does
    /// not hide the rest of a package's declaration.
    #[must_use]
    pub fn parse(
        subject: &Subject,
        value: &serde_json::Value,
        policy: &Policy,
    ) -> (Option<Self>, Vec<Violation>) {
        let mut violations = Vec::new();
        let Some(table) = value.as_object() else {
            violations.push(Violation {
                rule: "aex-metadata-bad-enum",
                detail: format!(
                    "`{}` declares `aex` as {}, not a table",
                    subject.path,
                    kind_of(value)
                ),
            });
            return (None, violations);
        };
        for key in table.keys() {
            if !KEYS.contains(&key.as_str()) {
                violations.push(Violation {
                    rule: "aex-metadata-unknown-key",
                    detail: format!(
                        "`{}` declares unknown key `aex.{key}`; the schema is closed",
                        subject.path
                    ),
                });
            }
        }
        let parsed: Self = match serde_json::from_value(value.clone()) {
            Ok(parsed) => parsed,
            Err(error) => {
                violations.push(Violation {
                    rule: "aex-metadata-bad-enum",
                    detail: format!("`{}` declares an unusable aex block: {error}", subject.path),
                });
                return (None, violations);
            }
        };
        violations.extend(parsed.check_value_sets(subject, policy));
        (Some(parsed), violations)
    }

    fn check_value_sets(&self, subject: &Subject, policy: &Policy) -> Vec<Violation> {
        let mut violations = Vec::new();
        let mut scalar = |field: &str, value: &str| {
            let permitted = policy.permitted(field);
            if !permitted.iter().any(|candidate| candidate == value) {
                violations.push(Violation {
                    rule: "aex-metadata-bad-enum",
                    detail: format!(
                        "`{}` declares {field} `{value}`; permitted {field}s: {}",
                        subject.path,
                        permitted.join(", ")
                    ),
                });
            }
        };
        scalar("owner", &self.owner);
        scalar("role", &self.role);
        scalar("artifact", &self.artifact);
        scalar("security_tier", &self.security_tier);
        for layer in &self.layers {
            scalar("layers", layer);
        }
        for concern in &self.concerns {
            scalar("concerns", concern);
        }
        for risk in &self.risk {
            scalar("risk", risk);
        }
        for layer in self.targets.values() {
            scalar("layers", layer);
        }
        for field in self.not_applicable.keys() {
            if !NOT_APPLICABLE_FIELDS.contains(&field.as_str()) {
                violations.push(Violation {
                    rule: "aex-metadata-unknown-key",
                    detail: format!(
                        "`{}` marks unknown field `{field}` not-applicable; excusable fields: {}",
                        subject.path,
                        NOT_APPLICABLE_FIELDS.join(", ")
                    ),
                });
            }
        }
        for (field, reason) in &self.not_applicable {
            if reason.trim().len() < 8 {
                violations.push(Violation {
                    rule: "aex-not-applicable-unjustified",
                    detail: format!(
                        "`{}` marks `{field}` not-applicable with no structural reason",
                        subject.path
                    ),
                });
            }
        }
        violations
    }

    /// Whether the package declares a layer.
    #[must_use]
    pub fn declares_layer(&self, layer: &str) -> bool {
        self.layers.iter().any(|declared| declared == layer)
    }

    /// Whether the package declares a concern.
    #[must_use]
    pub fn declares_concern(&self, concern: &str) -> bool {
        self.concerns.iter().any(|declared| declared == concern)
    }

    /// The structural reason `field` does not apply, if one was declared.
    #[must_use]
    pub fn excused(&self, field: &str) -> Option<&str> {
        self.not_applicable.get(field).map(String::as_str)
    }
}

/// The fields a `not_applicable` entry may name.
///
/// `targets` is the one that carries the "awaiting owner" state: a package that
/// declares what it owes but has not written it yet says so here, naming the
/// stream that will. That is what distinguishes an unwritten suite from a
/// silently omitted one.
pub const NOT_APPLICABLE_FIELDS: &[&str] = &[
    "live_suite",
    "deployable",
    "targets",
    "smoke",
    "e2e",
    "integration",
    "user",
    "scenarios",
];

fn kind_of(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "an array",
        serde_json::Value::Object(_) => "a table",
    }
}

#[cfg(test)]
mod tests {
    use super::{AexMeta, Subject};
    use crate::policy::Policy;

    fn subject() -> Subject {
        Subject {
            path: "crates/aex-foo".to_owned(),
            name: "aex-foo".to_owned(),
        }
    }

    fn block(extra: &str) -> serde_json::Value {
        let text = format!(
            r#"{{ "owner": "regional-domains", "role": "domain", "artifact": "none",
                  "layers": ["unit"], "concerns": ["property"], "seams": [],
                  "security_tier": "authority", "risk": ["concurrency"], "scenarios": []
                  {extra} }}"#
        );
        serde_json::from_str(&text).expect("the fixture parses")
    }

    #[test]
    fn a_well_formed_block_parses_with_no_violation() {
        let (parsed, violations) = AexMeta::parse(&subject(), &block(""), Policy::embedded());
        assert!(violations.is_empty(), "{violations:?}");
        let parsed = parsed.expect("the block parses");
        assert_eq!(parsed.role, "domain");
        assert!(parsed.declares_concern("property"));
        assert!(parsed.declares_layer("unit"));
        assert!(!parsed.declares_layer("smoke"));
    }

    #[test]
    fn an_unknown_key_is_reported_with_the_key_name() {
        let (_, violations) = AexMeta::parse(
            &subject(),
            &block(r#", "tier": "gold""#),
            Policy::embedded(),
        );
        let unknown: Vec<&str> = violations
            .iter()
            .filter(|violation| violation.rule == "aex-metadata-unknown-key")
            .map(|violation| violation.detail.as_str())
            .collect();
        assert_eq!(
            unknown,
            vec!["`crates/aex-foo` declares unknown key `aex.tier`; the schema is closed"]
        );
    }

    #[test]
    fn a_value_outside_a_closed_set_names_the_permitted_values() {
        let text = r#"{ "owner": "regional-domains", "role": "helper", "artifact": "none",
                        "layers": ["unit"], "concerns": ["property"], "seams": [],
                        "security_tier": "authority", "risk": ["none"], "scenarios": [] }"#;
        let value: serde_json::Value = serde_json::from_str(text).expect("the fixture parses");
        let (_, violations) = AexMeta::parse(&subject(), &value, Policy::embedded());
        let bad: Vec<&str> = violations
            .iter()
            .filter(|violation| violation.rule == "aex-metadata-bad-enum")
            .map(|violation| violation.detail.as_str())
            .collect();
        assert_eq!(bad.len(), 1, "{violations:?}");
        assert!(
            bad[0].starts_with(
                "`crates/aex-foo` declares role `helper`; permitted roles: contract, domain,"
            ),
            "{}",
            bad[0]
        );
    }

    #[test]
    fn an_unknown_layer_in_a_target_mapping_is_rejected() {
        let (_, violations) = AexMeta::parse(
            &subject(),
            &block(r#", "targets": { "properties": "fuzzing" }"#),
            Policy::embedded(),
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.detail.contains("declares layers `fuzzing`")),
            "{violations:?}"
        );
    }

    #[test]
    fn an_unexcusable_field_cannot_be_marked_not_applicable() {
        let (_, violations) = AexMeta::parse(
            &subject(),
            &block(r#", "not_applicable": { "property": "we do not do properties" }"#),
            Policy::embedded(),
        );
        assert!(
            violations.iter().any(|violation| {
                violation.rule == "aex-metadata-unknown-key"
                    && violation
                        .detail
                        .contains("marks unknown field `property` not-applicable")
            }),
            "{violations:?}"
        );
    }

    #[test]
    fn an_empty_not_applicable_reason_is_rejected() {
        let (_, violations) = AexMeta::parse(
            &subject(),
            &block(r#", "not_applicable": { "live_suite": "n/a" }"#),
            Policy::embedded(),
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.rule == "aex-not-applicable-unjustified"),
            "{violations:?}"
        );
    }

    #[test]
    fn a_non_table_aex_key_is_reported_rather_than_ignored() {
        let value = serde_json::Value::String("yes".to_owned());
        let (parsed, violations) = AexMeta::parse(&subject(), &value, Policy::embedded());
        assert!(parsed.is_none());
        assert_eq!(violations.len(), 1);
        assert!(
            violations[0]
                .detail
                .contains("declares `aex` as a string, not a table")
        );
    }
}
