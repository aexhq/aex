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

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::canon;
use crate::error::{Exit, Result, ToolError, Violation};

const LOCATION_KINDS: &[&str] = &[
    "github-release",
    "oci",
    "s3",
    "ecr",
    "npm",
    "gha-artifact",
    "local",
];

/// Lambda resource shape in the public composition contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestLambdaShape {
    /// Memory allocation in MiB.
    #[serde(rename = "memoryMiB")]
    pub memory_mb: u32,
    /// Function timeout in seconds.
    pub timeout_s: u32,
    /// Reserved concurrency.
    pub reserved_concurrency: u32,
}

impl From<crate::graph::inputs::LambdaShape> for ManifestLambdaShape {
    fn from(shape: crate::graph::inputs::LambdaShape) -> Self {
        Self {
            memory_mb: shape.memory_mb,
            timeout_s: shape.timeout_s,
            reserved_concurrency: shape.reserved_concurrency,
        }
    }
}

/// Fargate resource shape in the public composition contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestFargateShape {
    /// Task CPU units.
    pub cpu: u32,
    /// Task memory in MiB.
    #[serde(rename = "memoryMiB")]
    pub memory_mb: u32,
    /// Desired task count.
    pub desired_count: u32,
    /// Drain deadline in seconds.
    pub stop_timeout_s: u32,
    /// Container listening port.
    pub port: u16,
}

impl From<crate::graph::inputs::FargateShape> for ManifestFargateShape {
    fn from(shape: crate::graph::inputs::FargateShape) -> Self {
        Self {
            cpu: shape.cpu,
            memory_mb: shape.memory_mb,
            desired_count: shape.desired_count,
            stop_timeout_s: shape.stop_timeout_s,
            port: shape.port,
        }
    }
}

/// `MicroVM` image shape in the public composition contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestMicrovmShape {
    /// Stable image variant token.
    pub variant: String,
    /// Minimum guest memory accepted by the image.
    #[serde(rename = "minimumMemoryMiB")]
    pub minimum_memory_mib: u32,
    /// Whether the browser package layer is present.
    pub browser: bool,
}

impl From<crate::graph::inputs::MicrovmShape> for ManifestMicrovmShape {
    fn from(shape: crate::graph::inputs::MicrovmShape) -> Self {
        Self {
            variant: shape.variant,
            minimum_memory_mib: shape.minimum_memory_mib,
            browser: shape.browser,
        }
    }
}

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
    /// Lambda resource shape, copied from the deployable registry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lambda: Option<ManifestLambdaShape>,
    /// Fargate resource shape, copied from the deployable registry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fargate: Option<ManifestFargateShape>,
    /// `MicroVM` image shape, copied from the deployable registry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub microvm: Option<ManifestMicrovmShape>,
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
    /// SHA-256 over the transported JSON bytes.
    pub bundle_digest: String,
    /// Exact transported byte length.
    pub bundle_size_bytes: u64,
    /// Commit/run-addressed public HTTPS release asset.
    pub bundle_uri: String,
    /// BLAKE3 identity over the canonical decoded table definitions.
    pub definitions_digest: String,
    /// Generation number.
    pub generation: u32,
}

/// Exact public source and workflow run that minted the release inputs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicSource {
    /// Public GitHub repository.
    pub repository: String,
    /// Exact public source commit.
    pub commit_sha: String,
    /// GitHub workflow run id used in the immutable release tag.
    pub workflow_run_id: String,
    /// GitHub workflow attempt used in the immutable release tag.
    pub workflow_run_attempt: u64,
}

/// Raw, directly executable public release-tool identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseToolIdentity {
    /// Exact release-tool semantic version.
    pub version: String,
    /// SHA-256 digest of the raw executable bytes.
    pub digest: String,
    /// Exact raw executable byte length.
    pub size_bytes: u64,
    /// Commit/run-addressed public HTTPS release asset.
    pub uri: String,
    /// Rust target triple of the executable.
    pub target: String,
}

/// The infrastructure module bundle a root pins.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Infra {
    /// Module bundle digest.
    pub module_bundle_digest: String,
    /// Exact bundle byte length.
    pub module_bundle_size_bytes: u64,
    /// Commit/run-addressed public HTTPS release asset.
    pub module_bundle_uri: String,
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
    /// Exact public source and workflow run.
    pub source: PublicSource,
    /// Raw public release-tool identity consumed by hosted acquisition.
    pub release_tool: ReleaseToolIdentity,
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

/// The composition identities no artifact envelope carries.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompositionInputs {
    /// Generated contract bundle digest.
    pub contract_digest: String,
    /// Exact public source and workflow run.
    pub source: PublicSource,
    /// Raw public release-tool identity.
    pub release_tool: ReleaseToolIdentity,
    /// Published packages.
    #[serde(default)]
    pub packages: BTreeMap<String, BTreeMap<String, PackageRef>>,
    /// Migration identities.
    pub migrations: Migrations,
    /// Terraform module bundle and provider closure.
    pub infra: Infra,
    /// Catalogue digests.
    #[serde(default)]
    pub catalogs: BTreeMap<String, String>,
    /// Policy digests.
    pub policy: Policy,
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
    // After finance, because the merged central edge serves the billing group
    // and invokes `stripe-command-edge`: publishing it first would put a
    // listener in front of a money surface whose provider edge is still the
    // previous release. It is its own stage rather than an addition to
    // `central-finance` because it is the alternative composition to three
    // units in two earlier stages, and a stage is a set that goes together.
    ("central-edge", &["central-api"]),
    ("central-control", &["central-control-worker"]),
    ("regional-keys", &["regional-secret-key-admin"]),
    (
        "regional-api",
        &[
            "session-stream-api",
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
            "regional-capacity-controller",
            "regional-control",
            "observation-reconciler",
            "observation-export-launcher",
            "observation-export-task",
            "usage-storage-worker",
            "usage-compute-worker",
            "usage-transfer-worker",
        ],
    ),
    // The agent is its own stage and it comes first, because `hands-image`
    // embeds the agent binary: publishing them together would let an image whose
    // rootfs holds the previous agent reach a plane as if it held the new one.
    ("runtime-agent", &["hands-agent"]),
    (
        "runtime",
        &[
            "brain-mux",
            "tool-executor",
            "hands-image-512mb",
            "hands-image-1gb",
            "hands-image-2gb",
            "hands-image-4gb",
            "hands-image-8gb",
        ],
    ),
    ("web", &["dashboard", "site"]),
    // Last, and deliberately after every plane. A published package version can
    // never be withdrawn, so publishing an SDK that speaks routes the deployed
    // composition does not serve yet would ship a promise nothing keeps and
    // leave deprecation as the only reverse gear.
    ("published-packages", &["sdk"]),
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
        validate_public_inputs(self, &mut structural);
        for (id, unit) in &self.units {
            validate_manifest_unit_shape(id, unit, &mut structural);
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

    /// Validate the manifest and the exact public inputs downloaded for hosted
    /// acquisition.
    ///
    /// # Errors
    /// Propagates manifest validation failures and returns
    /// [`Exit::ArtifactMismatch`] when either downloaded subject is missing,
    /// truncated, or altered.
    pub fn validate_acquired_inputs(
        &self,
        release_tool: &std::path::Path,
        module_bundle: &std::path::Path,
        regional_tables: &std::path::Path,
        strict_environment_scan: bool,
    ) -> Result<()> {
        self.validate(strict_environment_scan)?;
        crate::publication::verify_blob(
            release_tool,
            &self.release_tool.digest,
            self.release_tool.size_bytes,
        )?;
        crate::publication::verify_blob(
            module_bundle,
            &self.infra.module_bundle_digest,
            self.infra.module_bundle_size_bytes,
        )?;
        crate::publication::verify_regional_tables_bundle(
            regional_tables,
            &self.migrations.regional.bundle_digest,
            self.migrations.regional.bundle_size_bytes,
            &self.migrations.regional.definitions_digest,
            self.migrations.regional.generation,
        )
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

impl ManifestUnit {
    /// The manifest entry an envelope and its registry row describe.
    ///
    /// Artifact identity fields are copied from the verified envelope. Resource
    /// shapes are copied from `release/units.toml`, the plane-neutral deployment
    /// authority; neither set is re-derived or transcribed.
    #[must_use]
    pub fn from_envelope(
        envelope: &crate::artifact::ArtifactEnvelope,
        registry: &crate::graph::inputs::Unit,
    ) -> Self {
        Self {
            kind: envelope.unit.kind.clone(),
            envelope_digest: envelope.envelope_digest.clone(),
            artifact_digest: envelope.output.digest.clone(),
            size_bytes: envelope.output.size_bytes,
            location: envelope.output.location.clone(),
            target: envelope.output.target.clone(),
            oci_index_digest: envelope.output.oci_index_digest.clone(),
            oci_child_digest: envelope.output.oci_child_digest.clone(),
            config_schema_version: envelope.identities.config_schema_version,
            required_central_head: envelope
                .identities
                .migration
                .as_ref()
                .and_then(|migration| migration.required_central_head.clone()),
            lambda: registry.lambda.map(Into::into),
            fargate: registry.fargate.map(Into::into),
            microvm: registry.microvm.clone().map(Into::into),
            adjacent: envelope.composition.adjacent.clone(),
        }
    }
}

fn validate_manifest_unit_shape(id: &str, unit: &ManifestUnit, violations: &mut Vec<Violation>) {
    let wants_lambda = matches!(unit.kind.as_str(), "rust-lambda" | "ts-lambda");
    let wants_fargate = matches!(unit.kind.as_str(), "rust-oci-service" | "rust-oci-task");
    let wants_microvm = unit.kind == "microvm-image";

    if wants_lambda {
        match unit.lambda {
            None => violations.push(Violation::new(
                "manifest-unit-shape-missing",
                format!("unit `{id}` is a Lambda and carries no Lambda shape"),
            )),
            Some(shape) => {
                if !(128..=10_240).contains(&shape.memory_mb)
                    || !(1..=900).contains(&shape.timeout_s)
                {
                    violations.push(Violation::new(
                        "manifest-unit-shape-invalid",
                        format!("unit `{id}` carries an invalid Lambda resource shape"),
                    ));
                }
            }
        }
    }
    if wants_fargate {
        match unit.fargate {
            None => violations.push(Violation::new(
                "manifest-unit-shape-missing",
                format!("unit `{id}` runs on Fargate and carries no Fargate shape"),
            )),
            Some(shape) if shape.cpu == 0 || shape.memory_mb == 0 || shape.stop_timeout_s == 0 => {
                violations.push(Violation::new(
                    "manifest-unit-shape-invalid",
                    format!("unit `{id}` carries an invalid Fargate resource shape"),
                ));
            }
            Some(shape) => {
                if unit.kind == "rust-oci-service" && shape.port == 0 {
                    violations.push(Violation::new(
                        "manifest-unit-shape-invalid",
                        format!("unit `{id}` is a service and carries no listening port"),
                    ));
                }
                if unit.kind == "rust-oci-task" && (shape.desired_count != 0 || shape.port != 0) {
                    violations.push(Violation::new(
                        "manifest-unit-shape-conflict",
                        format!(
                            "unit `{id}` is a one-shot task and must carry desiredCount = 0 and port = 0"
                        ),
                    ));
                }
            }
        }
    }
    if wants_microvm {
        match &unit.microvm {
            None => violations.push(Violation::new(
                "manifest-unit-shape-missing",
                format!("unit `{id}` is a MicroVM image and carries no MicroVM shape"),
            )),
            Some(shape) if shape.variant.trim().is_empty() || shape.minimum_memory_mib == 0 => {
                violations.push(Violation::new(
                    "manifest-unit-shape-invalid",
                    format!("unit `{id}` carries an invalid MicroVM image shape"),
                ));
            }
            Some(_) => {}
        }
    }

    if unit.lambda.is_some() && !wants_lambda {
        violations.push(Violation::new(
            "manifest-unit-shape-conflict",
            format!("unit `{id}` of kind `{}` carries a Lambda shape", unit.kind),
        ));
    }
    if unit.fargate.is_some() && !wants_fargate {
        violations.push(Violation::new(
            "manifest-unit-shape-conflict",
            format!(
                "unit `{id}` of kind `{}` carries a Fargate shape",
                unit.kind
            ),
        ));
    }
    if unit.microvm.is_some() && !wants_microvm {
        violations.push(Violation::new(
            "manifest-unit-shape-conflict",
            format!(
                "unit `{id}` of kind `{}` carries a MicroVM shape",
                unit.kind
            ),
        ));
    }
}

/// Assemble a complete composition from a set of envelopes.
///
/// The unit set is the envelopes' own, and the order is derived from
/// [`DEFAULT_ORDER`], so a unit nobody described cannot appear and a unit no
/// stage places is a refusal rather than an unordered entry. Completeness
/// against the deployable registry is checked by the caller, which is the only
/// layer that knows which absences are recorded and which are holes.
///
/// # Errors
/// Returns [`Exit::CompositionIncompatible`] when a unit belongs to no stage,
/// and propagates canonicalization failure.
pub fn new_manifest(
    inputs: CompositionInputs,
    envelopes: &BTreeMap<String, crate::artifact::ArtifactEnvelope>,
    registry: &crate::graph::inputs::Units,
) -> Result<CompositionManifest> {
    let registered: BTreeMap<&str, &crate::graph::inputs::Unit> = registry
        .units
        .iter()
        .map(|unit| (unit.id.as_str(), unit))
        .collect();
    let units: BTreeMap<String, ManifestUnit> = envelopes
        .iter()
        .map(|(id, envelope)| {
            registered
                .get(id.as_str())
                .map(|unit| (id.clone(), ManifestUnit::from_envelope(envelope, unit)))
                .ok_or_else(|| {
                    ToolError::single(
                        Exit::CompositionIncompatible,
                        "manifest-unit-unregistered",
                        format!("envelope `{id}` names no deployable in release/units.toml"),
                    )
                })
        })
        .collect::<Result<_>>()?;
    let order = order_for(&units.keys().cloned().collect::<Vec<_>>())?;
    CompositionManifest {
        schema: "aex.composition-manifest.v1".to_owned(),
        release_id: "sha256:0".to_owned(),
        contract_digest: inputs.contract_digest,
        source: inputs.source,
        release_tool: inputs.release_tool,
        units,
        packages: inputs.packages,
        migrations: inputs.migrations,
        infra: inputs.infra,
        catalogs: inputs.catalogs,
        order,
        policy: inputs.policy,
        annotations: None,
    }
    .seal()
}

/// Index and verify the exact certified envelope set used by a public
/// composition handoff.
///
/// Unlike [`unaccounted_units`], a handoff has no unearned escape hatch: every
/// registered deployable must have one and only one complete envelope. The
/// returned map is also the canonical artifact-store document consumed by
/// hosted admission.
///
/// # Errors
/// Returns [`Exit::CompositionIncompatible`] for an incomplete, duplicate or
/// registry-inconsistent set, and [`Exit::EvidenceMissing`] when any envelope
/// is not fully certified or lacks a receipt required by its registry row.
pub fn verify_handoff_envelopes(
    registry: &crate::graph::inputs::Units,
    envelopes: Vec<crate::artifact::ArtifactEnvelope>,
) -> Result<BTreeMap<String, crate::artifact::ArtifactEnvelope>> {
    let registered: BTreeMap<&str, &crate::graph::inputs::Unit> = registry
        .units
        .iter()
        .map(|unit| (unit.id.as_str(), unit))
        .collect();
    let mut indexed = BTreeMap::new();
    let mut topology = Vec::new();

    for envelope in envelopes {
        let id = envelope.unit.id.clone();
        if indexed.contains_key(&id) {
            topology.push(Violation::new(
                "handoff-envelope-duplicate",
                format!("deployable `{id}` has more than one envelope"),
            ));
            continue;
        }
        match registered.get(id.as_str()) {
            None => topology.push(Violation::new(
                "handoff-envelope-unregistered",
                format!("envelope `{id}` names no deployable in release/units.toml"),
            )),
            Some(unit) => {
                let required_central_head = envelope
                    .identities
                    .migration
                    .as_ref()
                    .and_then(|migration| migration.required_central_head.as_ref());
                if envelope.unit.kind != unit.kind
                    || envelope.unit.plane != unit.plane
                    || envelope.media.form != unit.form
                    || envelope.output.target.triple != unit.target
                    || envelope.identities.config_schema_version != unit.config_schema_version
                    || envelope.identities.config_env_namespace.as_deref()
                        != unit.config_env_namespace.as_deref()
                    || required_central_head != unit.required_central_head.as_ref()
                {
                    topology.push(Violation::new(
                        "handoff-envelope-registry-mismatch",
                        format!(
                            "envelope `{id}` does not exactly match its kind, plane, form, target, configuration and migration registry fields"
                        ),
                    ));
                }
            }
        }
        indexed.insert(id, envelope);
    }

    topology.extend(unaccounted_units(registry, &indexed, &[]));
    if !topology.is_empty() {
        topology.sort();
        topology.dedup();
        return Err(ToolError::many(Exit::CompositionIncompatible, topology));
    }

    let mut incomplete = Vec::new();
    for (id, envelope) in &indexed {
        let unit = registered[id.as_str()];
        if let Err(err) =
            envelope.verify(None, crate::artifact::kind_requires_signature(&unit.kind))
        {
            incomplete.extend(err.violations.into_iter().map(|violation| {
                Violation::new(violation.rule, format!("unit `{id}`: {}", violation.detail))
            }));
        }
        let carried: BTreeSet<&str> = envelope
            .receipts
            .iter()
            .map(|receipt| receipt.class.as_str())
            .collect();
        for required in &unit.required_receipts {
            if !carried.contains(required.as_str()) {
                incomplete.push(Violation::new(
                    "handoff-receipt-missing",
                    format!("unit `{id}` requires a passing `{required}` receipt"),
                ));
            }
        }
    }
    if !incomplete.is_empty() {
        incomplete.sort();
        incomplete.dedup();
        return Err(ToolError::many(Exit::EvidenceMissing, incomplete));
    }
    Ok(indexed)
}

/// Bind a complete certified envelope store to the non-envelope composition
/// inputs and produce a strict, plane-neutral manifest.
///
/// # Errors
/// Returns [`Exit::CompositionIncompatible`] when the envelopes do not all
/// name the exact public source/run, contract, toolchain, migration and
/// catalogue identities carried by the composition inputs. Strict manifest
/// validation errors are propagated.
pub fn new_handoff_manifest(
    inputs: CompositionInputs,
    envelopes: &BTreeMap<String, crate::artifact::ArtifactEnvelope>,
    registry: &crate::graph::inputs::Units,
) -> Result<CompositionManifest> {
    let mut violations = Vec::new();
    for (id, envelope) in envelopes {
        if envelope.source.repository != inputs.source.repository
            || envelope.source.workflow.repository != inputs.source.repository
            || envelope.source.commit_sha != inputs.source.commit_sha
            || envelope.source.workflow.run_id != inputs.source.workflow_run_id
            || u64::from(envelope.source.workflow.run_attempt) != inputs.source.workflow_run_attempt
        {
            violations.push(Violation::new(
                "handoff-source-mismatch",
                format!(
                    "unit `{id}` is not bound to composition source `{}` at commit `{}`, run {}, attempt {}",
                    inputs.source.repository,
                    inputs.source.commit_sha,
                    inputs.source.workflow_run_id,
                    inputs.source.workflow_run_attempt
                ),
            ));
        }
        if envelope.identities.contract_digest != inputs.contract_digest {
            violations.push(Violation::new(
                "handoff-contract-mismatch",
                format!(
                    "unit `{id}` contract `{}` differs from composition contract `{}`",
                    envelope.identities.contract_digest, inputs.contract_digest
                ),
            ));
        }
        if envelope.inputs.toolchain.channel != inputs.policy.toolchain_channel {
            violations.push(Violation::new(
                "handoff-toolchain-mismatch",
                format!(
                    "unit `{id}` toolchain `{}` differs from composition toolchain `{}`",
                    envelope.inputs.toolchain.channel, inputs.policy.toolchain_channel
                ),
            ));
        }
        if let Some(migration) = &envelope.identities.migration {
            if migration
                .central_bundle_digest
                .as_ref()
                .is_some_and(|digest| digest != &inputs.migrations.central.bundle_digest)
            {
                violations.push(Violation::new(
                    "handoff-central-migration-mismatch",
                    format!("unit `{id}` carries a different central migration bundle"),
                ));
            }
            if migration
                .regional_bundle_digest
                .as_ref()
                .is_some_and(|digest| digest != &inputs.migrations.regional.bundle_digest)
                || migration
                    .regional_generation
                    .is_some_and(|generation| generation != inputs.migrations.regional.generation)
            {
                violations.push(Violation::new(
                    "handoff-regional-migration-mismatch",
                    format!("unit `{id}` carries a different regional migration identity"),
                ));
            }
        }
        if let Some(catalogs) = &envelope.identities.catalogs {
            for (name, digest) in [("model", &catalogs.model), ("tool", &catalogs.tool)] {
                if let Some(digest) = digest
                    && inputs.catalogs.get(name) != Some(digest)
                {
                    violations.push(Violation::new(
                        "handoff-catalog-mismatch",
                        format!("unit `{id}` carries a different `{name}` catalogue digest"),
                    ));
                }
            }
        }
    }
    if let Some(admin) = envelopes.get("central-schema-admin")
        && admin.output.digest != inputs.migrations.central.admin_image_digest
    {
        violations.push(Violation::new(
            "handoff-central-admin-image-mismatch",
            "the central migration admin image digest differs from the central-schema-admin artifact digest",
        ));
    }
    if !violations.is_empty() {
        violations.sort();
        violations.dedup();
        return Err(ToolError::many(Exit::CompositionIncompatible, violations));
    }

    let manifest = new_manifest(inputs, envelopes, registry)?;
    manifest.validate(true)?;
    Ok(manifest)
}

// Keep this accumulator together so one admission run reports every malformed
// public identity rather than turning review into a sequence of failures.
#[allow(clippy::too_many_lines)]
fn validate_public_inputs(manifest: &CompositionManifest, violations: &mut Vec<Violation>) {
    let sha256 = |value: &str| {
        value.len() == 71
            && value.starts_with("sha256:")
            && value[7..]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    };
    let sha1 = |value: &str| {
        value.len() == 40
            && value
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    };
    let exact_version = |value: &str| {
        regex::Regex::new(r"^[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?$")
            .expect("static exact-version regex")
            .is_match(value)
    };

    if manifest.source.repository != "aexhq/aex" {
        violations.push(Violation::new(
            "manifest-public-repository",
            format!(
                "`{}` is not the public source repository `aexhq/aex`",
                manifest.source.repository
            ),
        ));
    }
    if !sha1(&manifest.source.commit_sha) {
        violations.push(Violation::new(
            "manifest-public-commit",
            format!(
                "`{}` is not an exact lowercase Git commit",
                manifest.source.commit_sha
            ),
        ));
    }
    if manifest.source.workflow_run_id.is_empty()
        || !manifest
            .source
            .workflow_run_id
            .bytes()
            .all(|byte| byte.is_ascii_digit())
        || manifest.source.workflow_run_id.starts_with('0')
    {
        violations.push(Violation::new(
            "manifest-public-run-id",
            format!(
                "`{}` is not a positive GitHub workflow run id",
                manifest.source.workflow_run_id
            ),
        ));
    }
    if manifest.source.workflow_run_attempt == 0 {
        violations.push(Violation::new(
            "manifest-public-run-attempt",
            "the GitHub workflow run attempt must be positive",
        ));
    }
    if !exact_version(&manifest.release_tool.version) {
        violations.push(Violation::new(
            "manifest-release-tool-version",
            format!(
                "`{}` is not an exact release-tool version",
                manifest.release_tool.version
            ),
        ));
    }
    if !sha256(&manifest.release_tool.digest) {
        violations.push(Violation::new(
            "manifest-release-tool-digest",
            format!("`{}` is not a SHA-256 digest", manifest.release_tool.digest),
        ));
    }
    if manifest.release_tool.size_bytes == 0 {
        violations.push(Violation::new(
            "manifest-release-tool-size",
            "the raw release tool must contain at least one byte",
        ));
    }
    if manifest.release_tool.target != "x86_64-unknown-linux-musl" {
        violations.push(Violation::new(
            "manifest-release-tool-target",
            format!(
                "`{}` is not the hosted acquisition target `x86_64-unknown-linux-musl`",
                manifest.release_tool.target
            ),
        ));
    }
    if !sha256(&manifest.infra.module_bundle_digest) {
        violations.push(Violation::new(
            "manifest-module-bundle-digest",
            format!(
                "`{}` is not a SHA-256 digest",
                manifest.infra.module_bundle_digest
            ),
        ));
    }
    if manifest.infra.module_bundle_size_bytes == 0 {
        violations.push(Violation::new(
            "manifest-module-bundle-size",
            "the Terraform module bundle must contain at least one byte",
        ));
    }
    if !sha256(&manifest.migrations.regional.bundle_digest) {
        violations.push(Violation::new(
            "manifest-regional-bundle-digest",
            format!(
                "`{}` is not a SHA-256 digest",
                manifest.migrations.regional.bundle_digest
            ),
        ));
    }
    if manifest.migrations.regional.bundle_size_bytes == 0 {
        violations.push(Violation::new(
            "manifest-regional-bundle-size",
            "the regional table bundle must contain at least one byte",
        ));
    }
    if !manifest
        .migrations
        .regional
        .definitions_digest
        .strip_prefix("blake3:")
        .is_some_and(|bare| {
            bare.len() == 64
                && bare
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
    {
        violations.push(Violation::new(
            "manifest-regional-definitions-digest",
            format!(
                "`{}` is not a lowercase BLAKE3 digest",
                manifest.migrations.regional.definitions_digest
            ),
        ));
    }

    if sha1(&manifest.source.commit_sha)
        && !manifest.source.workflow_run_id.is_empty()
        && manifest.source.workflow_run_attempt > 0
    {
        let base = format!(
            "https://github.com/{}/releases/download/main-{}-run-{}-attempt-{}",
            manifest.source.repository,
            manifest.source.commit_sha,
            manifest.source.workflow_run_id,
            manifest.source.workflow_run_attempt,
        );
        let expected_tool = format!("{base}/{}", crate::publication::RELEASE_TOOL_ASSET);
        if manifest.release_tool.uri != expected_tool {
            violations.push(Violation::new(
                "manifest-release-tool-uri",
                format!(
                    "`{}` is not the exact public release-tool asset `{expected_tool}`",
                    manifest.release_tool.uri
                ),
            ));
        }
        let expected_modules = format!("{base}/{}", crate::publication::MODULE_BUNDLE_ASSET);
        if manifest.infra.module_bundle_uri != expected_modules {
            violations.push(Violation::new(
                "manifest-module-bundle-uri",
                format!(
                    "`{}` is not the exact public module asset `{expected_modules}`",
                    manifest.infra.module_bundle_uri
                ),
            ));
        }
        let expected_regional = format!("{base}/{}", crate::publication::REGIONAL_TABLES_ASSET);
        if manifest.migrations.regional.bundle_uri != expected_regional {
            violations.push(Violation::new(
                "manifest-regional-bundle-uri",
                format!(
                    "`{}` is not the exact public regional table asset `{expected_regional}`",
                    manifest.migrations.regional.bundle_uri
                ),
            ));
        }
    }

    for (unit, entry) in &manifest.units {
        if !LOCATION_KINDS.contains(&entry.location.kind.as_str()) {
            violations.push(Violation::new(
                "manifest-location-kind",
                format!(
                    "unit `{unit}` location kind `{}` is outside the closed release vocabulary",
                    entry.location.kind
                ),
            ));
            continue;
        }
        match entry.location.kind.as_str() {
            "github-release" => {
                let Some(form) = crate::publication::blob_form_for_kind(&entry.kind) else {
                    violations.push(Violation::new(
                        "manifest-location-kind",
                        format!(
                            "unit `{unit}` kind `{}` cannot publish as a release blob",
                            entry.kind
                        ),
                    ));
                    continue;
                };
                match crate::publication::github_release_unit_uri(
                    &manifest.source.repository,
                    &manifest.source.commit_sha,
                    &manifest.source.workflow_run_id,
                    manifest.source.workflow_run_attempt,
                    unit,
                    &entry.artifact_digest,
                    form,
                ) {
                    Ok(expected) if expected == entry.location.uri => {}
                    Ok(expected) => violations.push(Violation::new(
                        "manifest-github-release-location",
                        format!(
                            "unit `{unit}` location `{}` is not `{expected}`",
                            entry.location.uri
                        ),
                    )),
                    Err(err) => violations.extend(err.violations),
                }
            }
            "oci" => match crate::publication::ghcr_unit_uri(
                &manifest.source.repository,
                unit,
                &entry.artifact_digest,
            ) {
                Ok(expected)
                    if expected == entry.location.uri && entry.kind.starts_with("rust-oci-") => {}
                Ok(expected) => violations.push(Violation::new(
                    "manifest-oci-location",
                    format!(
                        "unit `{unit}` location `{}` is not the OCI-kind-bound digest reference `{expected}`",
                        entry.location.uri
                    ),
                )),
                Err(err) => violations.extend(err.violations),
            },
            // A registry location names a package and version, not a digest, so
            // the cross-check is that the composition also records that exact
            // version under `packages.npm` — an entry the manifest cannot invent
            // because `manifest inputs` derives it from the registry readback.
            "npm" => match crate::publication::npm_tarball_identity(&entry.location.uri) {
                Ok((package, version)) => {
                    let recorded = manifest
                        .packages
                        .get("npm")
                        .and_then(|registry| registry.get(&package));
                    match recorded {
                        Some(reference)
                            if reference.version == version
                                && entry.kind == "npm-package"
                                && reference.provenance => {}
                        _ => violations.push(Violation::new(
                            "manifest-npm-location",
                            format!(
                                "unit `{unit}` publishes `{package}@{version}`, which is not \
                                 recorded as an attested npm package identity"
                            ),
                        )),
                    }
                }
                Err(err) => violations.extend(err.violations),
            },
            _ => {}
        }
        if (entry.kind == "npm-package") != (entry.location.kind == "npm") {
            violations.push(Violation::new(
                "manifest-location-kind",
                format!(
                    "unit `{unit}` kind `{}` and location kind `{}` disagree about registry \
                     publication",
                    entry.kind, entry.location.kind
                ),
            ));
        }
    }
}

/// Deployables that are neither described by an envelope nor recorded as
/// unearned.
///
/// A composition that simply omits a deployable is a release nobody can tell is
/// incomplete. A recorded gap and a hole nobody noticed are different facts, and
/// this is the only place that distinction can still be drawn — once the
/// manifest is written both look identical.
#[must_use]
pub fn unaccounted_units(
    registry: &crate::graph::inputs::Units,
    described: &BTreeMap<String, crate::artifact::ArtifactEnvelope>,
    recorded: &[aex_workspace_check::registry::UnearnedRow],
) -> Vec<Violation> {
    registry
        .units
        .iter()
        .filter(|unit| {
            !described.contains_key(&unit.id) && !recorded.iter().any(|row| row.subject == unit.id)
        })
        .map(|unit| {
            Violation::new(
                "manifest-unit-unaccounted",
                format!(
                    "deployable `{}` has no envelope and no row in the unearned ledger; a \
                     composition that simply omits it is a release nobody can tell is incomplete",
                    unit.id
                ),
            )
        })
        .collect()
}

/// What changed between two compositions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestDiff {
    /// Schema discriminator.
    pub schema: &'static str,
    /// The release compared from.
    pub from: String,
    /// The release compared to.
    pub to: String,
    /// Units only the newer manifest holds.
    pub added: Vec<String>,
    /// Units only the older manifest holds.
    pub removed: Vec<String>,
    /// Units whose artifact digest changed.
    pub changed: Vec<UnitChange>,
    /// Closed, ordered list of non-unit composition inputs that changed.
    pub inputs_changed: Vec<CompositionInputChange>,
}

/// A non-unit composition input whose identity changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CompositionInputChange {
    /// Generated API/SDK contract bundle.
    Contract,
    /// Central migration bundle or head.
    CentralMigrations,
    /// Regional table bundle, definitions, or generation.
    RegionalMigrations,
    /// Terraform module bundle.
    Infra,
}

/// One unit whose bytes differ between two compositions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnitChange {
    /// Unit id.
    pub unit: String,
    /// The older artifact digest.
    pub from: String,
    /// The newer artifact digest.
    pub to: String,
    /// Whether the newer entry permits rolling back to the older one.
    pub rollback_eligible: bool,
}

/// Compare two compositions.
///
/// A diff is a description, never a decision: it reports that a unit's bytes
/// changed and whether the newer entry says a rollback is permitted, and leaves
/// the promotion question to `admit`.
#[must_use]
pub fn diff(from: &CompositionManifest, to: &CompositionManifest) -> ManifestDiff {
    let added: Vec<String> = to
        .units
        .keys()
        .filter(|id| !from.units.contains_key(*id))
        .cloned()
        .collect();
    let removed: Vec<String> = from
        .units
        .keys()
        .filter(|id| !to.units.contains_key(*id))
        .cloned()
        .collect();
    let changed: Vec<UnitChange> = to
        .units
        .iter()
        .filter_map(|(id, entry)| {
            let previous = from.units.get(id)?;
            (previous.artifact_digest != entry.artifact_digest).then(|| UnitChange {
                unit: id.clone(),
                from: previous.artifact_digest.clone(),
                to: entry.artifact_digest.clone(),
                rollback_eligible: entry.adjacent.rollback_eligible,
            })
        })
        .collect();
    let mut inputs_changed = Vec::new();
    if from.contract_digest != to.contract_digest {
        inputs_changed.push(CompositionInputChange::Contract);
    }
    if from.migrations.central.bundle_digest != to.migrations.central.bundle_digest
        || from.migrations.central.head != to.migrations.central.head
    {
        inputs_changed.push(CompositionInputChange::CentralMigrations);
    }
    if from.migrations.regional.bundle_digest != to.migrations.regional.bundle_digest
        || from.migrations.regional.definitions_digest != to.migrations.regional.definitions_digest
        || from.migrations.regional.generation != to.migrations.regional.generation
    {
        inputs_changed.push(CompositionInputChange::RegionalMigrations);
    }
    if from.infra.module_bundle_digest != to.infra.module_bundle_digest {
        inputs_changed.push(CompositionInputChange::Infra);
    }
    ManifestDiff {
        schema: "aex.composition-diff.v1",
        from: from.release_id.clone(),
        to: to.release_id.clone(),
        added,
        removed,
        changed,
        inputs_changed,
    }
}

/// Whether a reference names a mutable target instead of a digest.
#[must_use]
pub fn looks_like_mutable_reference(value: &str) -> bool {
    if value.contains("@sha256:") || valid_prefixed_hex_digest(value, "blake3:", 64) {
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
    let exact_public_asset = is_exact_public_asset_uri(text, path);
    if text.starts_with("arn:") {
        findings.push(Violation::new(
            "manifest-environment-identity",
            format!("`{path}` holds an ARN: `{text}`"),
        ));
    }
    if !is_manifest_cryptographic_identity(text, path, exact_public_asset)
        && looks_like_account_id(text)
    {
        findings.push(Violation::new(
            "manifest-environment-identity",
            format!("`{path}` holds a 12-digit account identifier"),
        ));
    }
    if text.starts_with("https://") || text.starts_with("http://") {
        let host = text
            .split_once("//")
            .map_or("", |(_, rest)| rest.split('/').next().unwrap_or(""));
        if !exact_public_asset
            && !matches!(
                host,
                "schemas.aex.dev" | "slsa.dev" | "json-schema.org" | "spdx.org" | "cyclonedx.org"
            )
        {
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

fn valid_prefixed_hex_digest(value: &str, prefix: &str, digits: usize) -> bool {
    value.strip_prefix(prefix).is_some_and(|payload| {
        payload.len() == digits
            && payload
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    })
}

fn valid_lower_hex(value: &str, digits: usize) -> bool {
    value.len() == digits
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn is_exact_public_asset_uri(text: &str, path: &str) -> bool {
    let unit_location = path.starts_with(".units.") && path.ends_with(".location.uri");
    let host = text
        .strip_prefix("https://")
        .or_else(|| text.strip_prefix("http://"))
        .map_or("", |rest| rest.split('/').next().unwrap_or(""));
    (host == "github.com"
        && (matches!(
            path,
            ".releaseTool.uri" | ".infra.moduleBundleUri" | ".migrations.regional.bundleUri"
        ) || unit_location))
        || (unit_location && crate::publication::npm_tarball_identity(text).is_ok())
}

fn is_manifest_cryptographic_identity(text: &str, path: &str, exact_public_asset: bool) -> bool {
    let digest_position =
        path.ends_with("Digest") || path.ends_with("digest") || path.ends_with("releaseId");
    let commit_position = path.ends_with("commitSha");
    let oci_location = path.starts_with(".units.")
        && path.ends_with(".location.uri")
        && text
            .rsplit_once("@sha256:")
            .is_some_and(|(_, digest)| valid_lower_hex(digest, 64));

    exact_public_asset
        || (digest_position
            && (valid_prefixed_hex_digest(text, "sha256:", 64)
                || valid_prefixed_hex_digest(text, "blake3:", 64)))
        || (commit_position && valid_lower_hex(text, 40))
        || oci_location
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
    use super::{DEFAULT_ORDER, is_semver_range, looks_like_mutable_reference, order_for};
    use super::{diff, scan_environment};
    use serde_json::json;

    fn manifest(release: &str, units: &[(&str, &str)]) -> super::CompositionManifest {
        let mut document = json!({
            "schema": "aex.composition-manifest.v1",
            "releaseId": release,
            "contractDigest": "sha256:aa",
            "source": {
                "repository": "aexhq/aex",
                "commitSha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "workflowRunId": "123",
                "workflowRunAttempt": 1
            },
            "releaseTool": {
                "version": "0.1.0",
                "digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "sizeBytes": 8192,
                "uri": "https://github.com/aexhq/aex/releases/download/main-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-run-123-attempt-1/aex-release-tool",
                "target": "x86_64-unknown-linux-musl"
            },
            "units": {},
            "migrations": {
                "central": { "bundleDigest": "sha256:bb", "head": "0007", "adminImageDigest": "sha256:cc" },
                "regional": {
                    "bundleDigest": "sha256:dd",
                    "bundleSizeBytes": 1,
                    "bundleUri": "https://github.com/aexhq/aex/releases/download/main-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-run-123-attempt-1/regional-tables.json",
                    "definitionsDigest": "blake3:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
                    "generation": 1
                }
            },
            "infra": {
                "moduleBundleDigest": "sha256:ee",
                "moduleBundleSizeBytes": 16384,
                "moduleBundleUri": "https://github.com/aexhq/aex/releases/download/main-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-run-123-attempt-1/terraform-modules.tar.gz",
                "terraformVersion": "1.14.0",
                "providerVersions": {}
            },
            "order": [{
                "name": "regional-stream",
                "units": [],
                "mode": "parallel",
                "rationale": "the fixture's only stage"
            }],
            "policy": {
                "toolchainChannel": "1.97.1",
                "artifactPolicyDigest": "sha256:ff",
                "freshnessPolicyDigest": "sha256:01",
                "sourcePolicyVersion": 1
            }
        });
        for (id, digest) in units {
            document["units"][*id] = json!({
                "kind": "rust-lambda",
                "envelopeDigest": "sha256:02",
                "artifactDigest": digest,
                "sizeBytes": 1,
                "location": { "kind": "s3", "uri": "lambda/x/y.zip", "immutable": true },
                "target": { "os": "linux", "architecture": "arm64", "triple": "aarch64-unknown-linux-gnu" },
                "configSchemaVersion": 1,
                "adjacent": {
                    "storageCompatible": true,
                    "protocolCompatible": true,
                    "rollbackEligible": true
                }
            });
            document["order"][0]["units"]
                .as_array_mut()
                .unwrap()
                .push(json!(id));
        }
        serde_json::from_value(document).expect("a fixture manifest")
    }

    #[test]
    fn the_agent_is_ordered_before_the_image_that_embeds_it() {
        let position = |unit: &str| {
            DEFAULT_ORDER
                .iter()
                .position(|(_, members)| members.contains(&unit))
                .unwrap_or_else(|| panic!("`{unit}` belongs to no stage"))
        };
        assert!(
            position("hands-agent") < position("hands-image-512mb"),
            "the image embeds the agent, so publishing them in one stage would let an \
             image carrying the previous agent reach a plane as if it carried the new one"
        );
    }

    #[test]
    fn every_shipped_unit_belongs_to_a_stage() {
        // A deployable the default order cannot place makes a complete
        // composition impossible, and the failure would only appear at the point
        // somebody tried to build one.
        let text = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../release/units.toml"),
        )
        .expect("release/units.toml");
        let units: crate::graph::inputs::Units = toml::from_str(&text).expect("the unit registry");
        let ids: Vec<String> = units.units.iter().map(|unit| unit.id.clone()).collect();
        assert!(!ids.is_empty());
        order_for(&ids).expect("every registry unit must belong to a default stage");
    }

    #[test]
    fn a_deployable_that_is_neither_described_nor_recorded_is_a_hole() {
        let registry: crate::graph::inputs::Units = toml::from_str(
            r#"
schema = "aex.units.v1"

[[unit]]
id = "regional-stream"
kind = "rust-oci-service"
plane = "regional"
package = "regional-stream"
target = "aarch64-unknown-linux-gnu.2.34"
profile = "release"
form = "oci-image"
config_env_namespace = "AEX_STREAM_"
config_schema_version = 1
required_receipts = ["unit"]
alarm_spec = "regional-stream"

[[unit]]
id = "regional-otlp"
kind = "rust-lambda"
plane = "regional"
package = "regional-otlp"
target = "aarch64-unknown-linux-gnu.2.34"
profile = "release-lambda"
form = "zip"
config_env_namespace = "AEX_OTLP_"
config_schema_version = 1
required_receipts = ["unit"]
alarm_spec = "regional-otlp"
"#,
        )
        .unwrap();
        let described = std::collections::BTreeMap::new();
        let row = |subject: &str| aex_workspace_check::registry::UnearnedRow {
            reason_class: "awaiting_owner".to_owned(),
            subject: subject.to_owned(),
            owner: "delivery".to_owned(),
            blocking_rule: "manifest-unit-unaccounted".to_owned(),
            detail: "no registry to push the image to".to_owned(),
        };

        let holes = super::unaccounted_units(&registry, &described, &[row("regional-stream")]);
        assert_eq!(holes.len(), 1);
        assert!(holes[0].detail.contains("regional-otlp"));

        let accounted = super::unaccounted_units(
            &registry,
            &described,
            &[row("regional-stream"), row("regional-otlp")],
        );
        assert!(
            accounted.is_empty(),
            "a recorded gap and a hole nobody noticed are different facts"
        );
    }

    #[test]
    fn a_diff_names_what_changed_and_leaves_the_decision_alone() {
        let from = manifest("sha256:from", &[("regional-stream", "sha256:10")]);
        let to = manifest(
            "sha256:to",
            &[
                ("regional-stream", "sha256:11"),
                ("regional-otlp", "sha256:12"),
            ],
        );
        let report = diff(&from, &to);
        assert_eq!(report.added, vec!["regional-otlp"]);
        assert!(report.removed.is_empty());
        assert_eq!(report.changed.len(), 1);
        assert_eq!(report.changed[0].unit, "regional-stream");
        assert_eq!(report.changed[0].from, "sha256:10");
        assert_eq!(report.changed[0].to, "sha256:11");
        assert!(report.inputs_changed.is_empty());

        let mut regional_change = to.clone();
        regional_change.migrations.regional.definitions_digest =
            format!("blake3:{}", "ef".repeat(32));
        assert_eq!(
            diff(&to, &regional_change).inputs_changed,
            vec![super::CompositionInputChange::RegionalMigrations]
        );

        let reverse = diff(&to, &from);
        assert_eq!(reverse.removed, vec!["regional-otlp"]);
        assert!(reverse.added.is_empty());
    }

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
    fn decimal_runs_inside_cryptographic_identities_are_not_account_ids() {
        let sha256 = format!("sha256:{}{}", "0".repeat(12), "ab".repeat(26));
        let blake3 = format!("blake3:{}{}", "0".repeat(12), "cd".repeat(26));
        let commit = format!("{}{}", "0".repeat(12), "ab".repeat(14));
        let uri = format!("https://github.com/aexhq/aex/releases/download/x/unit-{sha256}.zip");

        assert!(scan_environment(&json!({ "digest": sha256 })).is_empty());
        assert!(scan_environment(&json!({ "definitionsDigest": blake3 })).is_empty());
        assert!(scan_environment(&json!({ "commitSha": commit })).is_empty());
        assert!(scan_environment(&json!({ "releaseTool": { "uri": uri } })).is_empty());
    }

    #[test]
    fn an_exact_npm_tarball_is_a_plane_neutral_public_asset() {
        let uri = crate::publication::npm_tarball_uri("@aexhq/sdk", "0.50.0").unwrap();
        let exact = json!({ "units": { "sdk": { "location": { "uri": uri } } } });
        assert!(scan_environment(&exact).is_empty());

        let mutable = json!({
            "units": {
                "sdk": {
                    "location": {
                        "uri": "https://registry.npmjs.org/@aexhq/sdk/-/sdk-latest.tgz"
                    }
                }
            }
        });
        assert!(
            scan_environment(&mutable)
                .iter()
                .any(|violation| violation.rule == "manifest-environment-identity")
        );
    }

    #[test]
    fn an_account_id_padded_to_a_hash_length_remains_an_environment_identity() {
        let findings = scan_environment(&json!({
            "uri": "s3://aaaaaaaaaaaaaa522921482290bbbbbbbbbbbbbb/x"
        }));
        assert!(
            findings
                .iter()
                .any(|violation| violation.rule == "manifest-environment-identity")
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
        assert!(!looks_like_mutable_reference(&format!(
            "blake3:{}",
            "ab".repeat(32)
        )));
        assert!(looks_like_mutable_reference("blake3:latest"));
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
            "session-stream-api".to_owned(),
        ])
        .unwrap();
        let names: Vec<&str> = stages.iter().map(|stage| stage.name.as_str()).collect();
        assert_eq!(names, vec!["central-schema", "regional-api", "runtime"]);
        let err = order_for(&["ghost-service".to_owned()]).unwrap_err();
        assert_eq!(err.rules(), vec!["manifest-unit-unordered"]);
        assert_eq!(err.exit.code(), 32);
    }
}
