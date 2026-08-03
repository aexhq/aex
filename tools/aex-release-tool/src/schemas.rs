//! The seven release JSON Schemas, embedded.
//!
//! They live under `api/schemas/release/` so the contract generator needs no
//! special case, and they are embedded here so `schema print` works from a
//! binary with no working directory. The contracts stream generates
//! `aex_internal_contracts::release::*` from the same files; the Rust types in
//! this crate are the release tool's own reading of them, and a test asserts
//! the two agree on every fixture.

use crate::error::{Exit, Result, ToolError};

/// `aex.artifact-envelope.v1`.
pub const ARTIFACT_ENVELOPE: &str =
    include_str!("../../../api/schemas/release/artifact-envelope.json");
/// `aex.composition-manifest.v1`.
pub const COMPOSITION_MANIFEST: &str =
    include_str!("../../../api/schemas/release/composition-manifest.json");
/// `aex.verification-statement.v1`.
pub const VERIFICATION_STATEMENT: &str =
    include_str!("../../../api/schemas/release/verification-statement.json");
/// `aex.environment-binding.v1`.
pub const ENVIRONMENT_BINDING: &str =
    include_str!("../../../api/schemas/release/environment-binding.json");
/// `aex.resolved-placement.v1`.
pub const RESOLVED_PLACEMENT: &str =
    include_str!("../../../api/schemas/release/resolved-placement.json");
/// `aex.saved-plan-envelope.v1`.
pub const SAVED_PLAN_ENVELOPE: &str =
    include_str!("../../../api/schemas/release/saved-plan-envelope.json");
/// `aex.evidence-receipt.v1`.
pub const EVIDENCE_RECEIPT: &str =
    include_str!("../../../api/schemas/release/evidence-receipt.json");

/// Which schema to print.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum SchemaName {
    /// The artifact envelope.
    ArtifactEnvelope,
    /// The composition manifest.
    CompositionManifest,
    /// The verification statement.
    VerificationStatement,
    /// The environment binding.
    EnvironmentBinding,
    /// The resolved hosted placement.
    ResolvedPlacement,
    /// The saved Terraform plan envelope.
    SavedPlanEnvelope,
    /// The evidence receipt.
    EvidenceReceipt,
}

/// Every schema, by name.
pub const ALL: &[(SchemaName, &str, &str)] = &[
    (
        SchemaName::ArtifactEnvelope,
        "artifact-envelope",
        ARTIFACT_ENVELOPE,
    ),
    (
        SchemaName::CompositionManifest,
        "composition-manifest",
        COMPOSITION_MANIFEST,
    ),
    (
        SchemaName::VerificationStatement,
        "verification-statement",
        VERIFICATION_STATEMENT,
    ),
    (
        SchemaName::EnvironmentBinding,
        "environment-binding",
        ENVIRONMENT_BINDING,
    ),
    (
        SchemaName::ResolvedPlacement,
        "resolved-placement",
        RESOLVED_PLACEMENT,
    ),
    (
        SchemaName::SavedPlanEnvelope,
        "saved-plan-envelope",
        SAVED_PLAN_ENVELOPE,
    ),
    (
        SchemaName::EvidenceReceipt,
        "evidence-receipt",
        EVIDENCE_RECEIPT,
    ),
];

/// The schema text for a name.
#[must_use]
pub fn text(name: SchemaName) -> &'static str {
    match name {
        SchemaName::ArtifactEnvelope => ARTIFACT_ENVELOPE,
        SchemaName::CompositionManifest => COMPOSITION_MANIFEST,
        SchemaName::VerificationStatement => VERIFICATION_STATEMENT,
        SchemaName::EnvironmentBinding => ENVIRONMENT_BINDING,
        SchemaName::ResolvedPlacement => RESOLVED_PLACEMENT,
        SchemaName::SavedPlanEnvelope => SAVED_PLAN_ENVELOPE,
        SchemaName::EvidenceReceipt => EVIDENCE_RECEIPT,
    }
}

/// The schema as a parsed document.
///
/// # Errors
/// Returns [`Exit::Usage`] when an embedded schema does not parse, which can
/// only happen if a schema file was edited into invalid JSON.
pub fn document(name: SchemaName) -> Result<serde_json::Value> {
    serde_json::from_str(text(name)).map_err(|err| {
        ToolError::single(
            Exit::Usage,
            "schema-unparseable",
            format!("an embedded release schema does not parse: {err}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{ALL, document};

    #[test]
    fn every_embedded_schema_parses_and_declares_its_identity() {
        for (name, slug, _) in ALL {
            let schema = document(*name).unwrap();
            let id = schema["$id"].as_str().unwrap();
            assert_eq!(
                id,
                format!("https://schemas.aex.dev/release/v1/{slug}.json")
            );
            assert_eq!(
                schema["$schema"].as_str().unwrap(),
                "https://json-schema.org/draft/2020-12/schema"
            );
            assert_eq!(
                schema["additionalProperties"],
                serde_json::Value::Bool(false),
                "`{slug}` must be a closed object at the top level"
            );
        }
    }

    #[test]
    fn every_release_schema_file_is_embedded() {
        // A schema that exists on disk but is not embedded would be generated
        // by the contracts stream and enforced by nobody.
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../api/schemas/release");
        let mut on_disk: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
            .filter(|name| {
                std::path::Path::new(name)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
            })
            .collect();
        on_disk.sort();
        let mut embedded: Vec<String> = ALL
            .iter()
            .map(|(_, slug, _)| format!("{slug}.json"))
            .collect();
        embedded.sort();
        assert_eq!(on_disk, embedded);
    }
}
