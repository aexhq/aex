//! Artifact recipes, deterministic packaging, and the artifact envelope.
//!
//! An artifact's identity is its bytes. The envelope records everything that
//! shaped those bytes — source commit, toolchain, lockfile, build argv,
//! input-closure digest, base image — plus the supply-chain verdicts and the
//! receipts earned before publication. Post-deployment evidence is deliberately
//! absent: an artifact cannot contain proof that only exists once it is
//! deployed, so smoke, e2e, user, capacity and soak receipts live in the
//! verification statement instead.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::canon;
use crate::error::{Exit, Result, ToolError, Violation, io};
use crate::graph::inputs::{Unit, Units};
use crate::pack;

/// How an artifact is packaged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum Form {
    /// A single-entry `bootstrap` ZIP for AWS Lambda.
    LambdaZip,
    /// An OCI image layout.
    Oci,
    /// A guest root filesystem image.
    Rootfs,
    /// A `.tar.gz` of a file set.
    Tarball,
    /// A `.tar.gz` of a build output tree.
    BuildOutput,
}

/// The exact build invocation for one unit. Printing it is the whole point:
/// the release lane never runs a compiler, so somebody else must be able to
/// reproduce the bytes from this record alone.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildPlan {
    /// Unit id.
    pub unit: String,
    /// Artifact kind.
    pub kind: String,
    /// Target triple, including the glibc floor where one is pinned.
    pub target: String,
    /// Cargo profile or build mode.
    pub profile: String,
    /// The exact argv.
    pub argv: Vec<String>,
    /// Environment the build must be run with.
    pub env: BTreeMap<String, String>,
    /// Packaged form.
    pub form: String,
    /// Digest over the argv and environment, recorded in the envelope.
    pub digest: String,
}

/// Derive the build plan for one unit.
///
/// This function is pure. It reads no file, spawns no process and creates no
/// directory, which is what makes `artifact plan` safe to run in an
/// unprivileged job.
///
/// # Errors
/// Returns [`Exit::Usage`] for a unit kind with no recipe.
pub fn plan(unit: &Unit) -> Result<BuildPlan> {
    let package = unit.package.clone();
    let (argv, form) = match unit.kind.as_str() {
        // `cargo lambda build` drives `cargo zigbuild`, so the `bootstrap`
        // rename is native and the glibc floor is explicit rather than
        // whatever a container image happened to ship.
        "rust-lambda" => (
            vec![
                "cargo".to_owned(),
                "lambda".to_owned(),
                "build".to_owned(),
                "--profile".to_owned(),
                unit.profile.clone(),
                "--package".to_owned(),
                package,
                "--target".to_owned(),
                unit.target.clone(),
            ],
            "lambda-zip",
        ),
        "rust-oci-service" | "rust-oci-task" => (
            vec![
                "cargo".to_owned(),
                "zigbuild".to_owned(),
                "--release".to_owned(),
                "--package".to_owned(),
                package,
                "--target".to_owned(),
                unit.target.clone(),
            ],
            "oci",
        ),
        "rust-binary" => (
            vec![
                "cargo".to_owned(),
                "zigbuild".to_owned(),
                "--release".to_owned(),
                "--package".to_owned(),
                package,
                "--target".to_owned(),
                unit.target.clone(),
            ],
            "tarball",
        ),
        // The entry is the module that exports the Lambda handler symbol, which
        // is `handler.ts` in both edges. The output directory is the package's
        // own, so two edges built in one job cannot overwrite each other. There
        // is no minify flag: `bun build` does not minify unless asked, and
        // `--minify=false` is a parse error rather than a no-op — writing the
        // default down is what made this recipe unrunnable.
        "ts-lambda" => (
            vec![
                "bun".to_owned(),
                "build".to_owned(),
                "--target=node".to_owned(),
                "--outdir".to_owned(),
                format!("services/{}/dist", unit.id),
                format!("services/{}/src/handler.ts", unit.id),
            ],
            "lambda-zip",
        ),
        "build-output" => (
            vec!["bun".to_owned(), "run".to_owned(), "build".to_owned()],
            "build-output",
        ),
        "rootfs" => (
            vec![
                "runtimes".to_owned(),
                "hands-image".to_owned(),
                "build".to_owned(),
            ],
            "rootfs",
        ),
        other => {
            return Err(ToolError::single(
                Exit::Usage,
                "artifact-recipe-unknown",
                format!("unit `{}` has kind `{other}`, which has no recipe", unit.id),
            ));
        }
    };
    let env = BTreeMap::from([
        ("CARGO_INCREMENTAL".to_owned(), "0".to_owned()),
        ("SOURCE_DATE_EPOCH".to_owned(), "0".to_owned()),
        ("RUSTFLAGS".to_owned(), String::new()),
    ]);
    let digest = canon::digest_document(&serde_json::json!({
        "argv": argv,
        "env": env,
        "target": unit.target,
        "profile": unit.profile,
    }))?;
    Ok(BuildPlan {
        unit: unit.id.clone(),
        kind: unit.kind.clone(),
        target: unit.target.clone(),
        profile: unit.profile.clone(),
        argv,
        env,
        form: form.to_owned(),
        digest,
    })
}

/// Every recipe in the registry.
///
/// # Errors
/// Propagates a unit with no recipe.
pub fn recipes(units: &Units) -> Result<Vec<BuildPlan>> {
    units.units.iter().map(plan).collect()
}

/// Package a built input into its artifact bytes.
///
/// `entrypoint` is the archive member name for [`Form::LambdaZip`] and is
/// ignored by every other form. It comes from the unit's own `entrypoint`
/// field, because the custom runtime loads `bootstrap` and a Node runtime loads
/// the file its handler symbol names: one hard-coded name would produce an
/// archive one of the two runtimes cannot start.
///
/// # Errors
/// Returns [`Exit::Usage`] when the input cannot be read, and for the two forms
/// that cannot be produced without a registry.
pub fn package(
    form: Form,
    input: &Path,
    source_date_epoch: u64,
    entrypoint: &str,
) -> Result<Vec<u8>> {
    match form {
        Form::LambdaZip => {
            let data =
                std::fs::read(input).map_err(|err| io(&input.display().to_string(), &err))?;
            pack::write_zip(&[pack::Entry::executable(entrypoint, data)])
        }
        Form::Tarball | Form::BuildOutput => {
            let entries = if input.is_dir() {
                collect_tree(input)?
            } else {
                let name = input
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("artifact")
                    .to_owned();
                let data =
                    std::fs::read(input).map_err(|err| io(&input.display().to_string(), &err))?;
                vec![pack::Entry::regular(&name, data)]
            };
            pack::write_tar_gz(&entries, source_date_epoch)
        }
        Form::Oci | Form::Rootfs => Err(ToolError::single(
            Exit::Usage,
            "artifact-form-requires-registry",
            "an OCI image is assembled over a digest-pinned base whose blobs come from a \
             registry, and a rootfs comes from the Hands image build; neither can be \
             produced offline. Use `artifact plan` to print the exact build invocation.",
        )),
    }
}

fn collect_tree(root: &Path) -> Result<Vec<pack::Entry>> {
    let mut entries = Vec::new();
    for entry in walkdir::WalkDir::new(root).sort_by_file_name() {
        let entry = entry.map_err(|err| {
            ToolError::single(Exit::Usage, "io", format!("walking build output: {err}"))
        })?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(root)
            .unwrap_or(entry.path())
            .to_string_lossy()
            .replace('\\', "/");
        let data = std::fs::read(entry.path())
            .map_err(|err| io(&entry.path().display().to_string(), &err))?;
        entries.push(pack::Entry::regular(&relative, data));
    }
    Ok(entries)
}

/// `sha256` over the canonical, sorted list of `(path, blob digest)` for an
/// artifact's input closure, concatenated with the build identity.
///
/// It is recomputed, never cached across commits: a cached closure digest is a
/// claim about bytes nobody re-read.
///
/// # Errors
/// Propagates canonicalization failure.
pub fn input_closure_digest(
    files: &BTreeMap<String, String>,
    build: &BuildPlan,
    toolchain: &Toolchain,
    base_image: Option<&str>,
) -> Result<String> {
    canon::digest_document(&serde_json::json!({
        "files": files,
        "buildCommandDigest": build.digest,
        "toolchain": toolchain,
        "profile": build.profile,
        "target": build.target,
        "baseImage": base_image,
    }))
}

// ---------------------------------------------------------------------------
// The artifact envelope
// ---------------------------------------------------------------------------

/// `aex.artifact-envelope.v1`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactEnvelope {
    /// Schema discriminator.
    pub schema: String,
    /// Self-digest over the canonical bytes with this field removed.
    pub envelope_digest: String,
    /// What was built.
    pub unit: UnitIdentity,
    /// How it is encoded.
    pub media: Media,
    /// Where the source came from.
    pub source: Source,
    /// What shaped the bytes.
    pub inputs: Inputs,
    /// The bytes themselves.
    pub output: Output,
    /// Contract, telemetry, config and migration identities.
    pub identities: Identities,
    /// Minimum and adjacent composition constraints.
    pub composition: Composition,
    /// Software bill of materials.
    pub sbom: Sbom,
    /// Licence verdict.
    pub licenses: Licenses,
    /// Advisory verdict.
    pub vulnerabilities: Vulnerabilities,
    /// Build provenance.
    pub provenance: Provenance,
    /// Signature, where one applies.
    pub signature: Signature,
    /// Pre-publication receipts.
    pub receipts: Vec<ReceiptRef>,
    /// Retention class.
    pub retention: Retention,
    /// Envelope creation time.
    pub created_at: String,
}

/// What was built.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnitIdentity {
    /// Unit id.
    pub id: String,
    /// Artifact kind.
    pub kind: String,
    /// Which plane the unit runs in.
    pub plane: String,
    /// Optional grouping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
}

/// How the artifact is encoded.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Media {
    /// IANA media type.
    pub media_type: String,
    /// Container form.
    pub form: String,
}

/// Where the source came from.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Source {
    /// `owner/repo`.
    pub repository: String,
    /// Commit built.
    pub commit_sha: String,
    /// Always true: a dirty tree cannot mint an identity.
    pub tree_clean: bool,
    /// Git ref built.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#ref: Option<String>,
    /// The workflow that built it.
    pub workflow: Workflow,
}

/// The building workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Workflow {
    /// Workflow repository.
    pub repository: String,
    /// Workflow ref.
    pub r#ref: String,
    /// Workflow file path.
    pub path: String,
    /// Run id.
    pub run_id: String,
    /// Run attempt.
    pub run_attempt: u32,
    /// Job name.
    pub job_name: String,
    /// SLSA builder identity.
    pub builder_id: String,
}

/// The pinned toolchain.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Toolchain {
    /// Release channel, pinned exactly.
    pub channel: String,
    /// `rustc --version` output version.
    pub rustc_version: String,
    /// `rustc` commit hash.
    pub rustc_commit_hash: String,
    /// Host triple.
    pub host: String,
    /// Target triple.
    pub target: String,
    /// Installed components.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<String>,
    /// Packager version, where the packager shapes bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub packager_version: Option<String>,
}

/// The build command.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuildCommand {
    /// Exact argv.
    pub argv: Vec<String>,
    /// Environment.
    pub env: BTreeMap<String, String>,
    /// Digest over argv and environment.
    pub digest: String,
}

/// A digest-pinned base image.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BaseImage {
    /// Human-readable reference.
    pub r#ref: String,
    /// Content digest.
    pub digest: String,
}

/// What shaped the bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Inputs {
    /// `Cargo.lock` or `bun.lock` digest.
    pub lockfile_digest: String,
    /// Toolchain identity.
    pub toolchain: Toolchain,
    /// Build profile.
    pub build_profile: String,
    /// Build command.
    pub build_command: BuildCommand,
    /// Input-closure digest.
    pub input_closure_digest: String,
    /// How many files were in the closure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_closure_count: Option<u64>,
    /// Base image, for OCI forms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_image: Option<BaseImage>,
    /// Build arguments.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub build_args: BTreeMap<String, String>,
    /// `SOURCE_DATE_EPOCH`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_date_epoch: Option<u64>,
}

/// The target platform.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Target {
    /// Operating system.
    pub os: String,
    /// Architecture.
    pub architecture: String,
    /// Rust target triple.
    pub triple: String,
    /// glibc floor, where one is pinned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub libc_version: Option<String>,
}

/// Where the bytes are stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Location {
    /// Storage kind.
    pub kind: String,
    /// Storage URI.
    pub uri: String,
    /// Always true: a mutable destination is not an identity.
    pub immutable: bool,
    /// S3 object version, where versioning is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_version_id: Option<String>,
}

/// Detached debug symbols.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Symbols {
    /// Whether symbols were retained.
    pub present: bool,
    /// Symbol bundle digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    /// Symbol bundle URI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
}

/// The bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Output {
    /// Content digest.
    pub digest: String,
    /// Byte length.
    pub size_bytes: u64,
    /// Target platform.
    pub target: Target,
    /// OCI index digest, where an index is published.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oci_index_digest: Option<String>,
    /// The child digest ECS actually runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oci_child_digest: Option<String>,
    /// Where the bytes live.
    pub location: Location,
    /// Detached symbols.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbols: Option<Symbols>,
}

/// Migration identities.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MigrationIdentity {
    /// Minimum applied central head.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_central_head: Option<String>,
    /// Central bundle digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub central_bundle_digest: Option<String>,
    /// Regional bundle digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regional_bundle_digest: Option<String>,
    /// Regional table generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regional_generation: Option<u32>,
}

/// Catalogue identities.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Catalogs {
    /// Model catalogue digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Tool catalogue digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
}

/// Contract, telemetry, config and migration identities.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Identities {
    /// Generated contract bundle digest.
    pub contract_digest: String,
    /// Telemetry schema digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry_schema_digest: Option<String>,
    /// Configuration schema version.
    pub config_schema_version: u32,
    /// Configuration environment namespace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_env_namespace: Option<String>,
    /// Migration identities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migration: Option<MigrationIdentity>,
    /// Catalogue identities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalogs: Option<Catalogs>,
}

/// One minimum-composition constraint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MinimumConstraint {
    /// The other unit.
    pub unit: String,
    /// What must hold.
    pub constraint: String,
    /// The constraint value, where one applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

/// Adjacent-version compatibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Adjacent {
    /// Whether the adjacent version reads the same durable state.
    pub storage_compatible: bool,
    /// Whether the adjacent version speaks the same protocol.
    pub protocol_compatible: bool,
    /// Whether rolling back to the adjacent version is permitted.
    pub rollback_eligible: bool,
    /// Why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
}

/// Composition constraints.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Composition {
    /// What else must be present.
    #[serde(default)]
    pub minimum: Vec<MinimumConstraint>,
    /// Adjacent-version compatibility.
    pub adjacent: Adjacent,
}

/// Software bill of materials.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Sbom {
    /// SBOM format.
    pub format: String,
    /// SBOM digest.
    pub digest: String,
    /// SBOM URI.
    pub uri: String,
    /// Component count.
    pub component_count: u64,
}

/// Licence verdict.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Licenses {
    /// Digest of the policy that produced the verdict.
    pub policy_digest: String,
    /// Always `allowed`; a denial is a failed build, not a recorded state.
    pub verdict: String,
    /// Always empty.
    pub denials: Vec<String>,
    /// Digest of the full licence inventory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_digest: Option<String>,
}

/// An approved advisory exception.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdvisoryException {
    /// Advisory id.
    pub id: String,
    /// Why it is accepted.
    pub reason: String,
    /// When the acceptance lapses.
    pub expires_at: String,
    /// Who accepted it.
    pub approver: String,
}

/// Advisory verdict.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Vulnerabilities {
    /// Scanner identity.
    pub scanner: String,
    /// Advisory database identity.
    pub database: String,
    /// When the scan ran.
    pub scanned_at: String,
    /// Always zero.
    pub unapproved_critical: u32,
    /// Always zero.
    pub unapproved_high: u32,
    /// Time-boxed, attributed exceptions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approved_exceptions: Vec<AdvisoryException>,
}

/// Build provenance.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Provenance {
    /// Predicate type.
    pub predicate_type: String,
    /// Attestation bundle digest.
    pub bundle_digest: String,
    /// Attestation URI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    /// Builder identity.
    pub builder_id: String,
    /// Whether the attestation verified.
    pub attested: bool,
}

/// Signature, where one applies.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Signature {
    /// Whether a signature is present.
    pub present: bool,
    /// Signature scheme.
    pub kind: String,
    /// Key identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,
    /// Signature bundle digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_digest: Option<String>,
}

/// Where a receipt came from.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReceiptSource {
    /// Repository.
    pub repository: String,
    /// Commit.
    pub commit_sha: String,
    /// Workflow run id.
    pub workflow_run_id: String,
    /// Run attempt.
    pub run_attempt: u32,
}

/// A receipt referenced by the envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReceiptRef {
    /// Receipt class.
    pub class: String,
    /// Receipt digest.
    pub receipt_digest: String,
    /// Where it came from.
    pub source: ReceiptSource,
    /// Always `passed`.
    pub conclusion: String,
}

/// Retention class.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Retention {
    /// Class name.
    pub class: String,
    /// When the object may be collected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

/// Receipt classes an artifact may carry before publication.
///
/// Deployed evidence is deliberately absent from this list.
pub const PREPUBLICATION_RECEIPT_CLASSES: &[&str] = &[
    "boundary",
    "conformance",
    "contract",
    "deny",
    "determinism",
    "integration",
    "license",
    "lint",
    "package-integrity",
    "property",
    "sbom",
    "unit",
    "vulnerability",
];

impl ArtifactEnvelope {
    /// Recompute and set the self-digest.
    ///
    /// # Errors
    /// Propagates canonicalization failure.
    pub fn seal(mut self) -> Result<Self> {
        "sha256:0".clone_into(&mut self.envelope_digest);
        let value = serde_json::to_value(&self).map_err(|err| {
            ToolError::single(
                Exit::EnvelopeInvalid,
                "envelope-unserializable",
                err.to_string(),
            )
        })?;
        self.envelope_digest = canon::digest_document_excluding(&value, &["envelopeDigest"])?;
        Ok(self)
    }

    /// Verify structural invariants, the self-digest, and optionally the bytes.
    ///
    /// # Errors
    /// Returns the classification of the first failure class encountered:
    /// [`Exit::EnvelopeInvalid`], [`Exit::ArtifactMismatch`],
    /// [`Exit::ProvenanceMissing`] or [`Exit::SupplyChainDenied`].
    // One function on purpose: every structural, byte, provenance and
    // supply-chain check for one artifact, in the order their exit codes are
    // classified. Splitting it would scatter that order across four call sites.
    #[allow(clippy::too_many_lines)]
    pub fn verify(&self, file: Option<&Path>, require_signature: bool) -> Result<()> {
        let mut structural = Vec::new();
        if self.schema != "aex.artifact-envelope.v1" {
            structural.push(Violation::new(
                "envelope-schema",
                format!("unknown envelope schema `{}`", self.schema),
            ));
        }
        if !self.source.tree_clean {
            structural.push(Violation::new(
                "envelope-dirty-tree",
                "the build tree was not clean; a dirty tree cannot mint an identity",
            ));
        }
        if !self.output.location.immutable {
            structural.push(Violation::new(
                "envelope-mutable-location",
                format!(
                    "`{}` is not an immutable destination",
                    self.output.location.uri
                ),
            ));
        }
        if crate::manifest::looks_like_mutable_reference(&self.output.location.uri) {
            structural.push(Violation::new(
                "envelope-mutable-location",
                format!(
                    "`{}` names a tag or branch instead of a digest",
                    self.output.location.uri
                ),
            ));
        }
        if self.receipts.is_empty() {
            structural.push(Violation::new(
                "envelope-no-receipts",
                "the envelope carries no receipt; publication would prove nothing",
            ));
        }
        for receipt in &self.receipts {
            if receipt.conclusion != "passed" {
                structural.push(Violation::new(
                    "envelope-receipt-not-passed",
                    format!(
                        "receipt class `{}` concluded `{}`",
                        receipt.class, receipt.conclusion
                    ),
                ));
            }
            if !PREPUBLICATION_RECEIPT_CLASSES.contains(&receipt.class.as_str()) {
                structural.push(Violation::new(
                    "envelope-receipt-class",
                    format!(
                        "receipt class `{}` is post-deployment evidence and belongs to the \
                         verification statement, not to an artifact envelope",
                        receipt.class
                    ),
                ));
            }
        }
        let recomputed = {
            let value = serde_json::to_value(self).map_err(|err| {
                ToolError::single(
                    Exit::EnvelopeInvalid,
                    "envelope-unserializable",
                    err.to_string(),
                )
            })?;
            canon::digest_document_excluding(&value, &["envelopeDigest"])?
        };
        if recomputed != self.envelope_digest {
            structural.push(Violation::new(
                "envelope-digest-mismatch",
                format!(
                    "recorded envelopeDigest `{}` does not match the canonical bytes `{recomputed}`",
                    self.envelope_digest
                ),
            ));
        }
        if !structural.is_empty() {
            return Err(ToolError::many(Exit::EnvelopeInvalid, structural));
        }

        if let Some(path) = file {
            let bytes = std::fs::read(path).map_err(|err| io(&path.display().to_string(), &err))?;
            let digest = canon::digest_bytes(&bytes);
            let mut mismatches = Vec::new();
            if digest != self.output.digest {
                mismatches.push(Violation::new(
                    "artifact-digest-mismatch",
                    format!(
                        "`{}` hashes to `{digest}`; the envelope records `{}`",
                        path.display(),
                        self.output.digest
                    ),
                ));
            }
            if bytes.len() as u64 != self.output.size_bytes {
                mismatches.push(Violation::new(
                    "artifact-size-mismatch",
                    format!(
                        "`{}` is {} bytes; the envelope records {}",
                        path.display(),
                        bytes.len(),
                        self.output.size_bytes
                    ),
                ));
            }
            if !mismatches.is_empty() {
                return Err(ToolError::many(Exit::ArtifactMismatch, mismatches));
            }
        }

        if !self.provenance.attested || self.provenance.bundle_digest.is_empty() {
            return Err(ToolError::single(
                Exit::ProvenanceMissing,
                "provenance-unattested",
                format!(
                    "unit `{}` carries no verified build provenance",
                    self.unit.id
                ),
            ));
        }
        if require_signature && (!self.signature.present || self.signature.kind == "none") {
            return Err(ToolError::single(
                Exit::ProvenanceMissing,
                "signature-missing",
                format!(
                    "unit `{}` is unsigned and policy requires a signature",
                    self.unit.id
                ),
            ));
        }

        let mut supply = Vec::new();
        if self.licenses.verdict != "allowed" || !self.licenses.denials.is_empty() {
            supply.push(Violation::new(
                "license-denied",
                format!(
                    "unit `{}` carries {} licence denial(s)",
                    self.unit.id,
                    self.licenses.denials.len()
                ),
            ));
        }
        if self.vulnerabilities.unapproved_critical > 0 || self.vulnerabilities.unapproved_high > 0
        {
            supply.push(Violation::new(
                "advisory-denied",
                format!(
                    "unit `{}` carries {} unapproved critical and {} unapproved high advisories",
                    self.unit.id,
                    self.vulnerabilities.unapproved_critical,
                    self.vulnerabilities.unapproved_high
                ),
            ));
        }
        if self.sbom.component_count == 0 {
            supply.push(Violation::new(
                "sbom-empty",
                format!("unit `{}` has an SBOM with no components", self.unit.id),
            ));
        }
        if supply.is_empty() {
            Ok(())
        } else {
            Err(ToolError::many(Exit::SupplyChainDenied, supply))
        }
    }
}

/// The immutable destination an artifact publishes to.
#[derive(Debug, Clone, Serialize)]
pub struct PublishDestination {
    /// Storage kind.
    pub kind: String,
    /// Content-addressed key or reference, relative to the environment's
    /// bucket or repository. The environment prefix is a binding value and is
    /// deliberately not part of the public artifact identity.
    pub key: String,
    /// Always true.
    pub immutable: bool,
}

/// Derive the immutable destination for an envelope.
///
/// # Errors
/// Returns [`Exit::EnvelopeInvalid`] for a unit kind with no publication rule.
pub fn publish_destination(envelope: &ArtifactEnvelope) -> Result<PublishDestination> {
    let bare = envelope
        .output
        .digest
        .strip_prefix("sha256:")
        .unwrap_or(&envelope.output.digest);
    let (kind, key) = match envelope.unit.kind.as_str() {
        "rust-lambda" | "ts-lambda" => ("s3", format!("lambda/{}/{bare}.zip", envelope.unit.id)),
        "rust-oci-service" | "rust-oci-task" => (
            "ecr",
            format!("{}@{}", envelope.unit.id, envelope.output.digest),
        ),
        "rust-binary" | "rootfs" | "build-output" => (
            "s3",
            format!("{}/{}/{bare}", envelope.unit.kind, envelope.unit.id),
        ),
        "npm-package" => ("npm", envelope.unit.id.clone()),
        other => {
            return Err(ToolError::single(
                Exit::EnvelopeInvalid,
                "publish-destination-unknown",
                format!("unit kind `{other}` has no publication rule"),
            ));
        }
    };
    Ok(PublishDestination {
        kind: kind.to_owned(),
        key,
        immutable: true,
    })
}

#[cfg(test)]
mod tests {
    use super::{Form, package, plan};
    use crate::graph::inputs::Unit;

    fn unit(kind: &str) -> Unit {
        toml::from_str(&format!(
            r#"
id = "regional-session-api"
kind = "{kind}"
plane = "regional"
package = "regional-session-api"
target = "aarch64-unknown-linux-gnu.2.34"
profile = "release-lambda"
form = "zip"
config_env_namespace = "AEX_REGIONAL_SESSION_"
config_schema_version = 1
required_receipts = ["unit"]
alarm_spec = "regional-session-api"
"#
        ))
        .unwrap()
    }

    #[test]
    fn the_lambda_recipe_is_a_stable_argv() {
        let first = plan(&unit("rust-lambda")).unwrap();
        let second = plan(&unit("rust-lambda")).unwrap();
        assert_eq!(first.argv, second.argv);
        assert_eq!(first.digest, second.digest);
        assert_eq!(
            first.argv,
            vec![
                "cargo",
                "lambda",
                "build",
                "--profile",
                "release-lambda",
                "--package",
                "regional-session-api",
                "--target",
                "aarch64-unknown-linux-gnu.2.34"
            ]
        );
        assert_eq!(first.env["CARGO_INCREMENTAL"], "0");
    }

    #[test]
    fn planning_produces_no_side_effect_in_the_working_directory() {
        let before: Vec<_> = std::fs::read_dir(".")
            .unwrap()
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.file_name())
            .collect();
        plan(&unit("rust-oci-service")).unwrap();
        let after: Vec<_> = std::fs::read_dir(".")
            .unwrap()
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.file_name())
            .collect();
        assert_eq!(before.len(), after.len(), "`artifact plan` never builds");
    }

    #[test]
    fn an_unknown_unit_kind_has_no_recipe() {
        let err = plan(&unit("wasm-module")).unwrap_err();
        assert_eq!(err.rules(), vec!["artifact-recipe-unknown"]);
    }

    #[test]
    fn oci_and_rootfs_packaging_refuse_rather_than_produce_a_partial_image() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("bin");
        std::fs::write(&input, b"x").unwrap();
        for form in [Form::Oci, Form::Rootfs] {
            let err = package(form, &input, 0, "bootstrap").unwrap_err();
            assert_eq!(err.rules(), vec!["artifact-form-requires-registry"]);
        }
    }

    #[test]
    fn packaging_a_build_output_tree_is_byte_stable() {
        let temp = tempfile::tempdir().unwrap();
        let tree = temp.path().join("out");
        std::fs::create_dir_all(tree.join("static")).unwrap();
        std::fs::write(tree.join("index.html"), b"<!doctype html>").unwrap();
        std::fs::write(tree.join("static/app.js"), b"console.log(1)").unwrap();
        let first = package(Form::BuildOutput, &tree, 0, "bootstrap").unwrap();
        let second = package(Form::BuildOutput, &tree, 0, "bootstrap").unwrap();
        assert_eq!(first, second);
    }
}
