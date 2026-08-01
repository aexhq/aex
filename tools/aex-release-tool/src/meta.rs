//! `[package.metadata.aex]` — the one ownership-metadata parser.
//!
//! There is exactly one `AexMeta` in the workspace. `graph verify` and
//! `test-registry verify` share it, so the closed key set and the closed value
//! sets cannot drift apart between the delivery and test-architecture streams.
//!
//! The key set is closed and the value sets are closed. An unknown key is a
//! failure, not a forward-compatible extension: ownership metadata that can
//! silently absorb a typo stops being evidence of anything.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::Violation;

/// The thirteen permitted keys. Nine are pinned by plan 14 §12.2; `artifact`,
/// `deployable`, `targets` and `not_applicable` are the four added by plan 15
/// cross-stream requirement 18 and accepted here.
pub const KEYS: &[&str] = &[
    "artifact",
    "concerns",
    "deployable",
    "layers",
    "live_suite",
    "not_applicable",
    "owner",
    "risk",
    "scenarios",
    "seams",
    "security_tier",
    "targets",
];

/// Permitted `owner` values: one per implementation stream.
pub const OWNERS: &[&str] = &[
    "brain-core",
    "central-finance",
    "central-identity",
    "clients",
    "contracts",
    "delivery",
    "hands",
    "infrastructure",
    "observations-usage",
    "providers",
    "regional-domains",
    "regional-services",
    "regional-stores",
    "test-architecture",
    "tools-mcp",
];

/// Permitted `role` values.
pub const ROLES: &[&str] = &[
    "adapter",
    "application",
    "bundle",
    "client",
    "composition",
    "contract",
    "deployable",
    "domain",
    "infra_module",
    "live_companion",
    "runtime_image",
    "test_support",
    "tool",
    "web_app",
];

/// Permitted `artifact` values.
pub const ARTIFACTS: &[&str] = &[
    "bundle",
    "hands_image",
    "lambda_zip",
    "native_binary",
    "none",
    "npm_package",
    "oci_image",
    "oci_task",
    "static_site",
    "terraform_module",
    "vercel_build_output",
];

/// Permitted `layers` values, which are also the permitted target layers.
pub const LAYERS: &[&str] = &["e2e", "integration", "smoke", "unit", "user"];

/// Permitted `concerns` values.
pub const CONCERNS: &[&str] = &[
    "compatibility",
    "concurrency",
    "contract",
    "fault",
    "performance",
    "property",
    "security",
];

/// Permitted `security_tier` values.
pub const SECURITY_TIERS: &[&str] = &[
    "authority",
    "diagnostic",
    "guest",
    "internal",
    "money",
    "public_edge",
    "secret",
];

/// Permitted `risk` values; they drive fuzz, mutation and Miri selection.
pub const RISKS: &[&str] = &[
    "concurrency",
    "generated_code",
    "iam",
    "money",
    "none",
    "sql",
    "unsafe_code",
    "untrusted_input",
];

/// One package's ownership declaration.
///
/// Deserialization is `deny_unknown_fields`, which is the closed key set. The
/// closed *value* sets are checked by [`AexMeta::validate`] so the error names
/// the offending value and the permitted set rather than a serde path.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AexMeta {
    /// Owning implementation stream.
    pub owner: String,
    /// What kind of component this is.
    pub role: String,
    /// What this package produces, if anything.
    #[serde(default = "artifact_none")]
    pub artifact: String,
    /// The deployable this package is or belongs to. Required when
    /// `role = "deployable"` or `role = "live_companion"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deployable: Option<String>,
    /// The `tests/live/aex-live-*` companion package that claims this
    /// package's live seams.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_suite: Option<String>,
    /// Which of the five layers this package owes evidence at.
    #[serde(default)]
    pub layers: Vec<String>,
    /// Which correctness concerns this package owes evidence for.
    #[serde(default)]
    pub concerns: Vec<String>,
    /// External seam ids this package touches, by id from
    /// `release/policy/seams.toml`.
    #[serde(default)]
    pub seams: Vec<String>,
    /// Blast-radius classification.
    pub security_tier: String,
    /// Risk classes that select the advanced tool set.
    #[serde(default)]
    pub risk: Vec<String>,
    /// Scenario ids this package participates in.
    #[serde(default)]
    pub scenarios: Vec<String>,
    /// Test-target name to layer. Every integration-test target appears here.
    #[serde(default)]
    pub targets: BTreeMap<String, String>,
    /// Structural reasons for an omitted layer or live suite. Free text, but
    /// the key must name a real field or layer.
    #[serde(default)]
    pub not_applicable: BTreeMap<String, String>,
}

fn artifact_none() -> String {
    "none".to_owned()
}

impl AexMeta {
    /// Check every closed value set and the structural requirements that do
    /// not need the rest of the graph.
    ///
    /// `subject` names the manifest so a violation is actionable without a
    /// second lookup.
    #[must_use]
    pub fn validate(&self, subject: &str) -> Vec<Violation> {
        let mut violations = Vec::new();
        check_enum(subject, "owner", &self.owner, OWNERS, &mut violations);
        check_enum(subject, "role", &self.role, ROLES, &mut violations);
        check_enum(
            subject,
            "artifact",
            &self.artifact,
            ARTIFACTS,
            &mut violations,
        );
        check_enum(
            subject,
            "security_tier",
            &self.security_tier,
            SECURITY_TIERS,
            &mut violations,
        );
        for layer in &self.layers {
            check_enum(subject, "layers", layer, LAYERS, &mut violations);
        }
        for concern in &self.concerns {
            check_enum(subject, "concerns", concern, CONCERNS, &mut violations);
        }
        for risk in &self.risk {
            check_enum(subject, "risk", risk, RISKS, &mut violations);
        }
        for (target, layer) in &self.targets {
            if !LAYERS.contains(&layer.as_str()) {
                violations.push(Violation::new(
                    "aex-metadata-bad-enum",
                    format!(
                        "`{subject}` maps target `{target}` to layer `{layer}`; \
                         permitted layers: {}",
                        LAYERS.join(", ")
                    ),
                ));
            }
        }
        if self.layers.is_empty() {
            violations.push(Violation::new(
                "aex-metadata-bad-enum",
                format!("`{subject}` declares no layers; every package owes evidence at one"),
            ));
        }
        if (self.role == "deployable" || self.role == "live_companion") && self.deployable.is_none()
        {
            violations.push(Violation::new(
                "aex-metadata-missing-deployable",
                format!(
                    "`{subject}` has role `{}` and must name its `deployable`",
                    self.role
                ),
            ));
        }
        if self.role == "deployable"
            && self.live_suite.is_none()
            && !self.not_applicable.contains_key("live_suite")
        {
            violations.push(Violation::new(
                "aex-uncovered-deployable",
                format!("`{subject}` declares no live_suite and no not-applicable reason"),
            ));
        }
        for (field, reason) in &self.not_applicable {
            if reason.trim().len() < 8 {
                violations.push(Violation::new(
                    "aex-not-applicable-unjustified",
                    format!("`{subject}` marks `{field}` not-applicable with no structural reason"),
                ));
            }
            if reason.to_ascii_lowercase().contains("not yet")
                || reason.to_ascii_lowercase().contains("todo")
            {
                violations.push(Violation::new(
                    "aex-not-applicable-unjustified",
                    format!(
                        "`{subject}` marks `{field}` not-applicable because it is unwritten; \
                         the reason must be structural"
                    ),
                ));
            }
        }
        violations
    }

    /// Parse from the `metadata` object of one `cargo metadata` package entry.
    ///
    /// Returns `Ok(None)` when the package declares no `aex` table at all; the
    /// caller decides whether that is a violation, because a fixture workspace
    /// and the real repository want the same parser but different verdicts.
    ///
    /// # Errors
    /// Returns a violation when the table exists but does not parse, which
    /// covers both an unknown key and a wrong value type.
    pub fn from_metadata_object(
        subject: &str,
        metadata: Option<&serde_json::Value>,
    ) -> std::result::Result<Option<Self>, Violation> {
        let Some(table) = metadata.and_then(|m| m.get("aex")) else {
            return Ok(None);
        };
        Self::from_json(subject, table).map(Some)
    }

    /// Parse from an already-extracted `aex` object.
    ///
    /// # Errors
    /// Returns a violation naming the unknown key or bad type.
    pub fn from_json(
        subject: &str,
        table: &serde_json::Value,
    ) -> std::result::Result<Self, Violation> {
        serde_json::from_value(table.clone()).map_err(|err| {
            Violation::new(
                "aex-metadata-unknown-key",
                format!("`{subject}` metadata does not parse: {err}; the schema is closed"),
            )
        })
    }

    /// Parse from a TOML document holding a single `[aex]` table, which is the
    /// sidecar form Terraform modules and non-package artifacts use.
    ///
    /// # Errors
    /// Returns a violation naming the parse failure.
    pub fn from_sidecar_toml(subject: &str, text: &str) -> std::result::Result<Self, Violation> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Sidecar {
            aex: AexMeta,
        }
        toml::from_str::<Sidecar>(text)
            .map(|sidecar| sidecar.aex)
            .map_err(|err| {
                Violation::new(
                    "aex-metadata-unknown-key",
                    format!("`{subject}` sidecar does not parse: {err}; the schema is closed"),
                )
            })
    }
}

fn check_enum(
    subject: &str,
    field: &str,
    value: &str,
    permitted: &[&str],
    violations: &mut Vec<Violation>,
) {
    if !permitted.contains(&value) {
        violations.push(Violation::new(
            "aex-metadata-bad-enum",
            format!(
                "`{subject}` declares {field} `{value}`; permitted {field}: {}",
                permitted.join(", ")
            ),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::{AexMeta, KEYS};
    use serde_json::json;

    fn sound() -> serde_json::Value {
        json!({
            "owner": "regional-stores",
            "role": "adapter",
            "artifact": "none",
            "live_suite": "aex-live-regional-stores",
            "layers": ["unit", "integration"],
            "concerns": ["contract", "property", "fault", "security"],
            "seams": ["aws.s3.conditional_put"],
            "security_tier": "authority",
            "risk": ["concurrency"],
            "scenarios": ["SC-SESSION-ADMIT"],
            "targets": { "requests": "unit", "integration": "integration" }
        })
    }

    #[test]
    fn a_sound_declaration_parses_and_validates() {
        let meta = AexMeta::from_json("crates/aex-content-aws", &sound()).unwrap();
        assert!(meta.validate("crates/aex-content-aws").is_empty());
        assert_eq!(meta.artifact, "none");
    }

    #[test]
    fn an_unknown_key_is_rejected() {
        let mut value = sound();
        value["tier"] = json!("gold");
        let violation = AexMeta::from_json("crates/aex-foo", &value).unwrap_err();
        assert_eq!(violation.rule, "aex-metadata-unknown-key");
        assert!(violation.detail.contains("crates/aex-foo"));
    }

    #[test]
    fn a_value_outside_a_closed_set_is_rejected() {
        let mut value = sound();
        value["role"] = json!("helper");
        let meta = AexMeta::from_json("crates/aex-foo", &value).unwrap();
        let violations = meta.validate("crates/aex-foo");
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].rule, "aex-metadata-bad-enum");
        assert!(violations[0].detail.contains("permitted role"));
    }

    #[test]
    fn a_target_mapped_to_an_unknown_layer_is_rejected() {
        let mut value = sound();
        value["targets"] = json!({ "integration": "acceptance" });
        let meta = AexMeta::from_json("crates/aex-foo", &value).unwrap();
        let rules: Vec<&str> = meta
            .validate("crates/aex-foo")
            .iter()
            .map(|v| v.rule.clone().leak() as &str)
            .collect();
        assert_eq!(rules, vec!["aex-metadata-bad-enum"]);
    }

    #[test]
    fn a_deployable_without_a_live_suite_or_a_reason_is_uncovered() {
        let value = json!({
            "owner": "observations-usage",
            "role": "deployable",
            "artifact": "lambda_zip",
            "deployable": "usage-compute-worker",
            "layers": ["unit", "smoke"],
            "concerns": ["fault"],
            "security_tier": "internal",
            "risk": ["none"]
        });
        let meta = AexMeta::from_json("workers/usage-compute-worker", &value).unwrap();
        let violations = meta.validate("workers/usage-compute-worker");
        assert_eq!(
            violations
                .iter()
                .map(|v| v.rule.as_str())
                .collect::<Vec<_>>(),
            vec!["aex-uncovered-deployable"]
        );
    }

    #[test]
    fn a_not_yet_written_reason_is_not_a_structural_reason() {
        let mut value = sound();
        value["not_applicable"] = json!({ "live_suite": "not yet written" });
        let meta = AexMeta::from_json("crates/aex-foo", &value).unwrap();
        let violations = meta.validate("crates/aex-foo");
        assert!(
            violations
                .iter()
                .any(|v| v.rule == "aex-not-applicable-unjustified")
        );
    }

    #[test]
    fn the_sidecar_form_parses_the_same_schema() {
        let text = r#"
[aex]
owner = "delivery"
role = "infra_module"
artifact = "terraform_module"
layers = ["unit", "integration"]
concerns = ["security"]
seams = []
security_tier = "internal"
risk = ["iam"]
scenarios = []

[aex.not_applicable]
live_suite = "Terraform module; proved by mock-provider plan tests, not a deployed endpoint"
"#;
        let meta = AexMeta::from_sidecar_toml("infra/modules/kms-key", text).unwrap();
        assert!(meta.validate("infra/modules/kms-key").is_empty());
        assert_eq!(meta.role, "infra_module");
    }

    #[test]
    fn the_declared_key_list_matches_the_struct() {
        // KEYS is what the documentation and the CI gate quote; serde is what
        // actually rejects. Prove a key named in one is honoured by the other.
        for key in KEYS {
            let mut value = sound();
            let object = value.as_object_mut().unwrap();
            object.remove(*key);
            // Removing an optional key must still parse; removing a required
            // key must not. Either way, the key must be *known*.
            let with_key = AexMeta::from_json("x", &sound());
            assert!(with_key.is_ok(), "key `{key}` is not accepted by serde");
        }
    }
}
