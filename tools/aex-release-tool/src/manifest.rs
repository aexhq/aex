//! The composition manifest and the environment scan that keeps it
//! plane-neutral.
//!
//! Production promotes a complete immutable manifest. A one-service release
//! creates a new *complete* manifest whose only change is that unit's digest,
//! so what is promoted is always a whole composition somebody verified, never a
//! delta applied to whatever happened to be running.
//!
//! `manifest validate --strict-environment-scan` treats an environment
//! identity, a mutable reference or an unresolved range as a hard error. There
//! is no warning level and no `--force`: a manifest that names an account, an
//! ARN or `:latest` is not a slightly worse manifest, it is a different kind of
//! object.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::canon;
use crate::error::{Exit, Result, ToolError, Violation};

/// One unit's entry in a composition.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestUnit {
    /// Artifact kind.
    pub kind: String,
    /// Envelope digest.
    pub envelope_digest: String,
    /// Artifact content digest.
    pub artifact_digest: String,
    /// Byte length.
    pub size_bytes: u64,
    /// Immutable location.
    pub location: crate::artifact::Location,
    /// Target platform.
    pub target: crate::artifact::Target,
    /// OCI index digest, where an index is published.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oci_index_digest: Option<String>,
    /// The child digest ECS runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oci_child_digest: Option<String>,
    /// Configuration schema version.
    pub config_schema_version: u32,
    /// Minimum applied central head.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_central_head: Option<String>,
    /// Adjacent-version compatibility.
    pub adjacent: crate::artifact::Adjacent,
}

/// A published package.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageRef {
    /// Exact version. Never a range.
    pub version: String,
    /// Registry integrity string.
    pub integrity: String,
    /// Whether registry provenance is attached.
    #[serde(default)]
    pub provenance: bool,
}

/// Central and regional migration identities.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Migrations {
    /// Central bundle.
    pub central: CentralMigrations,
    /// Regional bundle.
    pub regional: RegionalMigrations,
}

/// The central migration bundle.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CentralMigrations {
    /// Bundle digest.
    pub bundle_digest: String,
    /// Declared head version.
    pub head: String,
    /// The schema-admin image whose bytes embed the same migrations.
    pub admin_image_digest: String,
}

/// The regional table generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegionalMigrations {
    /// Bundle digest.
    pub bundle_digest: String,
    /// Generation number.
    pub generation: u32,
}

/// The infrastructure module bundle a root pins.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Infra {
    /// Module bundle digest.
    pub module_bundle_digest: String,
    /// Where the bundle is stored.
    pub source_archive_uri: String,
    /// Terraform version.
    pub terraform_version: String,
    /// Provider versions.
    pub provider_versions: BTreeMap<String, String>,
}

/// One deployment stage.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Stage {
    /// Stage name.
    pub name: String,
    /// Units in the stage.
    pub units: Vec<String>,
    /// Whether the stage's units may proceed together.
    pub mode: String,
    /// Why this stage sits where it does.
    pub rationale: String,
}

/// Policy digests pinned into the composition.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Policy {
    /// Pinned toolchain channel.
    pub toolchain_channel: String,
    /// Artifact policy digest.
    pub artifact_policy_digest: String,
    /// Freshness policy digest.
    pub freshness_policy_digest: String,
    /// Source policy version.
    pub source_policy_version: u32,
}

/// `aex.composition-manifest.v1`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompositionManifest {
    /// Schema discriminator.
    pub schema: String,
    /// Self-digest over the canonical bytes with `releaseId` and `annotations`
    /// removed.
    pub release_id: String,
    /// Generated contract bundle digest.
    pub contract_digest: String,
    /// Every unit.
    pub units: BTreeMap<String, ManifestUnit>,
    /// Published packages.
    #[serde(default)]
    pub packages: BTreeMap<String, BTreeMap<String, PackageRef>>,
    /// Migration identities.
    pub migrations: Migrations,
    /// Infrastructure module bundle.
    pub infra: Infra,
    /// Catalogue digests.
    #[serde(default)]
    pub catalogs: BTreeMap<String, String>,
    /// Deployment order.
    pub order: Vec<Stage>,
    /// Policy digests.
    pub policy: Policy,
    /// Human annotations. Excluded from the digest, so a changelog edit does
    /// not mint a new release identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<serde_json::Value>,
}

/// The default stage order.
///
/// Overriding it requires a stated rationale on the stage plus an executable
/// release-scenario test; a reordering nobody wrote down is a reordering nobody
/// verified.
pub const DEFAULT_ORDER: &[(&str, &[&str])] = &[
    ("central-schema", &["central-schema-admin"]),
    (
        "central-identity",
        &[
            "central-identity-api",
            "central-authz",
            "central-control-api",
        ],
    ),
    (
        "central-finance",
        &[
            "finance-api",
            "finance-ingest",
            "finance-settlement-worker",
            "finance-reconcile",
            "usage-receipt-dispatcher",
            "provider-cost-reconciler",
            "stripe-command-edge",
            "stripe-webhook-edge",
        ],
    ),
    ("central-control", &["central-control-worker"]),
    ("regional-keys", &["regional-secret-key-admin"]),
    (
        "regional-api",
        &[
            "regional-session-api",
            "regional-secret-api",
            "regional-observation-api",
            "regional-otlp",
        ],
    ),
    (
        "regional-workers",
        &[
            "session-operation-worker",
            "runtime-control-worker",
            "content-lifecycle-worker",
            "observation-reconciler",
            "observation-export-launcher",
            "observation-export-task",
            "usage-storage-worker",
            "usage-compute-worker",
            "usage-transfer-worker",
        ],
    ),
    ("regional-stream", &["regional-stream"]),
    ("runtime", &["brain-mux", "hands-image"]),
    ("web", &["dashboard", "site"]),
];

impl CompositionManifest {
    /// Recompute and set `releaseId`.
    ///
    /// # Errors
    /// Propagates canonicalization failure.
    pub fn seal(mut self) -> Result<Self> {
        "sha256:0".clone_into(&mut self.release_id);
        let value = serde_json::to_value(&self).map_err(|err| {
            ToolError::single(
                Exit::ManifestInvalid,
                "manifest-unserializable",
                err.to_string(),
            )
        })?;
        self.release_id = canon::digest_document_excluding(&value, &["releaseId", "annotations"])?;
        Ok(self)
    }

    /// Recompute the digest without mutating.
    ///
    /// # Errors
    /// Propagates canonicalization failure.
    pub fn digest(&self) -> Result<String> {
        let mut probe = self.clone();
        "sha256:0".clone_into(&mut probe.release_id);
        let value = serde_json::to_value(&probe).map_err(|err| {
            ToolError::single(
                Exit::ManifestInvalid,
                "manifest-unserializable",
                err.to_string(),
            )
        })?;
        canon::digest_document_excluding(&value, &["releaseId", "annotations"])
    }

    /// Structural validation, plus the environment scan when asked.
    ///
    /// # Errors
    /// Returns [`Exit::ManifestInvalid`] for a structural fault and
    /// [`Exit::ManifestEnvironmentIdentity`] when the scan finds an
    /// environment identity, mutable reference or unresolved range.
    pub fn validate(&self, strict_environment_scan: bool) -> Result<()> {
        let mut structural = Vec::new();
        if self.schema != "aex.composition-manifest.v1" {
            structural.push(Violation::new(
                "manifest-schema",
                format!("unknown manifest schema `{}`", self.schema),
            ));
        }
        let recomputed = self.digest()?;
        if recomputed != self.release_id {
            structural.push(Violation::new(
                "manifest-digest-mismatch",
                format!(
                    "recorded releaseId `{}` does not match the canonical bytes `{recomputed}`",
                    self.release_id
                ),
            ));
        }
        if self.units.is_empty() {
            structural.push(Violation::new(
                "manifest-empty",
                "a composition with no unit is not a release",
            ));
        }
        let ordered: Vec<&String> = self.order.iter().flat_map(|stage| &stage.units).collect();
        for unit in self.units.keys() {
            if !ordered.contains(&unit) {
                structural.push(Violation::new(
                    "manifest-unit-unordered",
                    format!("unit `{unit}` appears in no deployment stage"),
                ));
            }
        }
        for stage in &self.order {
            if !matches!(stage.mode.as_str(), "parallel" | "sequential") {
                structural.push(Violation::new(
                    "manifest-stage-mode",
                    format!(
                        "stage `{}` declares mode `{}`; permitted: parallel, sequential",
                        stage.name, stage.mode
                    ),
                ));
            }
            if stage.rationale.trim().len() < 8 {
                structural.push(Violation::new(
                    "manifest-stage-rationale",
                    format!(
                        "stage `{}` states no rationale for its position",
                        stage.name
                    ),
                ));
            }
            for unit in &stage.units {
                if !self.units.contains_key(unit) {
                    structural.push(Violation::new(
                        "manifest-stage-unknown-unit",
                        format!("stage `{}` orders unknown unit `{unit}`", stage.name),
                    ));
                }
            }
        }
        if !structural.is_empty() {
            return Err(ToolError::many(Exit::ManifestInvalid, structural));
        }
        if strict_environment_scan {
            let value = serde_json::to_value(self).map_err(|err| {
                ToolError::single(
                    Exit::ManifestInvalid,
                    "manifest-unserializable",
                    err.to_string(),
                )
            })?;
            let findings = scan_environment(&value);
            if !findings.is_empty() {
                return Err(ToolError::many(Exit::ManifestEnvironmentIdentity, findings));
            }
        }
        Ok(())
    }

    /// Replace one unit's entry, producing a new complete manifest.
    ///
    /// # Errors
    /// Propagates canonicalization failure.
    pub fn with_unit(mut self, id: &str, entry: ManifestUnit) -> Result<Self> {
        self.units.insert(id.to_owned(), entry);
        self.seal()
    }
}

/// Whether a reference names a mutable target instead of a digest.
#[must_use]
pub fn looks_like_mutable_reference(value: &str) -> bool {
    if value.contains("@sha256:") {
        return false;
    }
    // An image reference with a tag: `repo:tag` after the last `/`, where the
    // tag is not a port number.
    let last = value.rsplit('/').next().unwrap_or(value);
    if let Some((_, tag)) = last.rsplit_once(':')
        && !tag.is_empty()
        && !tag.chars().all(|ch| ch.is_ascii_digit())
        && !value.starts_with("sha256:")
        && !value.starts_with("http")
        && !value.starts_with("s3:")
    {
        return true;
    }
    false
}

/// Strings that never appear in a plane-neutral manifest.
const MUTABLE_WORDS: &[&str] = &["latest", "main", "master", "dev", "prd", "staging"];

/// Scan a document for environment identities, mutable references and
/// unresolved ranges.
#[must_use]
pub fn scan_environment(value: &serde_json::Value) -> Vec<Violation> {
    let mut findings = Vec::new();
    walk(value, "", &mut findings);
    findings.sort();
    findings.dedup();
    findings
}

fn walk(value: &serde_json::Value, path: &str, findings: &mut Vec<Violation>) {
    match value {
        serde_json::Value::Object(members) => {
            for (key, child) in members {
                // Annotations are human text and are excluded from the release
                // identity, so they are also excluded from the scan.
                if key == "annotations" {
                    continue;
                }
                walk(child, &format!("{path}.{key}"), findings);
            }
        }
        serde_json::Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                walk(child, &format!("{path}[{index}]"), findings);
            }
        }
        serde_json::Value::String(text) => scan_string(text, path, findings),
        _ => {}
    }
}

fn scan_string(text: &str, path: &str, findings: &mut Vec<Violation>) {
    if text.starts_with("arn:") {
        findings.push(Violation::new(
            "manifest-environment-identity",
            format!("`{path}` holds an ARN: `{text}`"),
        ));
    }
    if looks_like_account_id(text) {
        findings.push(Violation::new(
            "manifest-environment-identity",
            format!("`{path}` holds a 12-digit account identifier"),
        ));
    }
    if text.starts_with("https://") || text.starts_with("http://") {
        let host = text
            .split_once("//")
            .map_or("", |(_, rest)| rest.split('/').next().unwrap_or(""));
        if !matches!(
            host,
            "schemas.aex.dev" | "slsa.dev" | "json-schema.org" | "spdx.org" | "cyclonedx.org"
        ) {
            findings.push(Violation::new(
                "manifest-environment-identity",
                format!("`{path}` names host `{host}`, which is outside the schema host set"),
            ));
        }
    }
    if looks_like_mutable_reference(text) {
        findings.push(Violation::new(
            "manifest-mutable-reference",
            format!("`{path}` names a tag instead of a digest: `{text}`"),
        ));
    }
    if text.starts_with("file:") || text.starts_with("workspace:") {
        findings.push(Violation::new(
            "manifest-unresolved-range",
            format!("`{path}` holds an unresolved specifier: `{text}`"),
        ));
    }
    if is_semver_range(text) {
        findings.push(Violation::new(
            "manifest-unresolved-range",
            format!("`{path}` holds a version range rather than an exact version: `{text}`"),
        ));
    }
    if path.ends_with("Digest") || path.ends_with("digest") || path.ends_with("releaseId") {
        let lowered = text.to_ascii_lowercase();
        for word in MUTABLE_WORDS {
            if lowered.contains(word) {
                findings.push(Violation::new(
                    "manifest-mutable-reference",
                    format!("`{path}` holds `{text}` in a digest position"),
                ));
            }
        }
    }
}

fn looks_like_account_id(text: &str) -> bool {
    let candidates = text.split(|ch: char| !ch.is_ascii_digit());
    candidates.into_iter().any(|run| run.len() == 12)
}

fn is_semver_range(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    let first = trimmed.as_bytes()[0];
    if matches!(first, b'^' | b'~' | b'>' | b'<' | b'=') {
        return true;
    }
    // `ends_with` here is a version-range tail, not a file extension, so the
    // case-insensitive extension comparison clippy suggests does not apply.
    if trimmed == "*"
        || trimmed
            .rsplit_once('.')
            .is_some_and(|(_, tail)| tail == "x" || tail == "*")
    {
        // Not a file extension: this is the tail of a semantic-version range.
        return true;
    }
    false
}

/// The stages a set of units maps onto, in deployment order.
///
/// # Errors
/// Returns [`Exit::CompositionIncompatible`] when a unit belongs to no stage.
pub fn order_for(units: &[String]) -> Result<Vec<Stage>> {
    let mut stages = Vec::new();
    let mut placed: Vec<String> = Vec::new();
    for (name, members) in DEFAULT_ORDER {
        let selected: Vec<String> = members
            .iter()
            .filter(|member| units.iter().any(|unit| unit == *member))
            .map(|member| (*member).to_owned())
            .collect();
        if selected.is_empty() {
            continue;
        }
        placed.extend(selected.iter().cloned());
        stages.push(Stage {
            name: (*name).to_owned(),
            units: selected,
            mode: "parallel".to_owned(),
            rationale: format!(
                "default order stage `{name}`; the units in it share no ordering \
                 dependency on each other"
            ),
        });
    }
    let missing: Vec<&String> = units.iter().filter(|unit| !placed.contains(unit)).collect();
    if missing.is_empty() {
        Ok(stages)
    } else {
        Err(ToolError::many(
            Exit::CompositionIncompatible,
            missing
                .into_iter()
                .map(|unit| {
                    Violation::new(
                        "manifest-unit-unordered",
                        format!("unit `{unit}` belongs to no default deployment stage"),
                    )
                })
                .collect(),
        ))
    }
}

/// Which previous manifests a unit may roll back to.
///
/// The walk returns an empty set rather than a guess. An empty set means fix
/// forward, which is a decision, not a failure to compute one.
#[must_use]
pub fn rollback_candidates(
    history: &[CompositionManifest],
    current: &CompositionManifest,
    unit: &str,
) -> Vec<String> {
    let mut candidates = Vec::new();
    // Walk from the most recent backwards. A single storage-incompatible
    // manifest between here and there closes every older candidate too,
    // because rolling back past it would run new code against migrated state.
    let mut blocked = false;
    for manifest in history.iter().rev() {
        let Some(entry) = manifest.units.get(unit) else {
            continue;
        };
        if manifest.release_id == current.release_id {
            continue;
        }
        if !entry.adjacent.storage_compatible {
            blocked = true;
        }
        if blocked {
            continue;
        }
        if entry.adjacent.rollback_eligible {
            candidates.push(manifest.release_id.clone());
        }
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::{is_semver_range, looks_like_mutable_reference, order_for, scan_environment};
    use serde_json::json;

    #[test]
    fn an_account_identifier_is_an_environment_identity() {
        let findings = scan_environment(&json!({ "uri": "s3://bucket-522921482290/x" }));
        assert!(
            findings
                .iter()
                .any(|v| v.rule == "manifest-environment-identity")
        );
    }

    #[test]
    fn an_arn_is_an_environment_identity() {
        let findings = scan_environment(&json!({ "role": "arn:aws:iam::x:role/y" }));
        assert!(
            findings
                .iter()
                .any(|v| v.rule == "manifest-environment-identity")
        );
    }

    #[test]
    fn a_tagged_image_reference_is_a_mutable_reference() {
        assert!(looks_like_mutable_reference("registry/brain-mux:latest"));
        assert!(looks_like_mutable_reference("registry/brain-mux:v1.2.3"));
        assert!(!looks_like_mutable_reference(
            "registry/brain-mux@sha256:aa"
        ));
        assert!(!looks_like_mutable_reference("s3://bucket/key.zip"));
        assert!(!looks_like_mutable_reference("registry:5000/brain-mux"));
    }

    #[test]
    fn a_semver_range_is_unresolved() {
        for range in ["^1.2.3", "~1.2", ">=2", "*", "1.x", "1.2.*"] {
            assert!(is_semver_range(range), "`{range}` should be a range");
        }
        for exact in ["1.2.3", "0.1.0-canary.7.gabc1234"] {
            assert!(!is_semver_range(exact), "`{exact}` should be exact");
        }
    }

    #[test]
    fn a_workspace_or_file_specifier_is_unresolved() {
        let findings = scan_environment(&json!({ "version": "workspace:*" }));
        assert!(
            findings
                .iter()
                .any(|v| v.rule == "manifest-unresolved-range")
        );
        let findings = scan_environment(&json!({ "version": "file:../sdk" }));
        assert!(
            findings
                .iter()
                .any(|v| v.rule == "manifest-unresolved-range")
        );
    }

    #[test]
    fn a_schema_host_is_permitted_and_a_plane_endpoint_is_not() {
        assert!(
            scan_environment(&json!({ "id": "https://schemas.aex.dev/release/v1/x.json" }))
                .is_empty()
        );
        assert!(!scan_environment(&json!({ "endpoint": "https://dev-api.aex.dev" })).is_empty());
    }

    #[test]
    fn annotations_are_excluded_from_the_scan() {
        let findings = scan_environment(&json!({
            "annotations": { "changelog": "https://github.com/aexhq/aex/releases/x" }
        }));
        assert!(findings.is_empty(), "annotations are not release identity");
    }

    #[test]
    fn a_mutable_word_in_a_digest_position_is_rejected() {
        let findings = scan_environment(&json!({ "artifactDigest": "sha256:latest" }));
        assert!(
            findings
                .iter()
                .any(|v| v.rule == "manifest-mutable-reference")
        );
    }

    #[test]
    fn ordering_places_every_known_unit_and_rejects_an_unknown_one() {
        let stages = order_for(&[
            "brain-mux".to_owned(),
            "central-schema-admin".to_owned(),
            "regional-session-api".to_owned(),
        ])
        .unwrap();
        let names: Vec<&str> = stages.iter().map(|stage| stage.name.as_str()).collect();
        assert_eq!(names, vec!["central-schema", "regional-api", "runtime"]);
        let err = order_for(&["ghost-service".to_owned()]).unwrap_err();
        assert_eq!(err.rules(), vec!["manifest-unit-unordered"]);
        assert_eq!(err.exit.code(), 32);
    }
}
