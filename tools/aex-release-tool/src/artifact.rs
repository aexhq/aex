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
    /// An AWS Lambda `MicroVM` image service ZIP.
    MicrovmZip,
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
    /// Repository-relative path the build command produces. Packaging consumes
    /// this exact path; workflows must not reconstruct it from the unit id.
    pub input: String,
    /// Archive member loaded by the runtime, where the form has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<String>,
    /// Digest-pinned runtime base for an OCI image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_image: Option<String>,
    /// Digest over the argv and environment, recorded in the envelope.
    pub digest: String,
}

/// Build-time inputs that turn `brain-mux` catalog verification into release authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCatalogBuildInputs {
    /// Canonical JSON containing the bounded release trust-root set.
    pub trust_roots_json: String,
    /// SHA-256 of the exact canonical trust-root JSON.
    pub trust_roots_sha256: String,
    /// Stable workspace-relative path of the collection copied into the binary.
    pub collection_file: String,
    /// SHA-256 of the exact collection bytes.
    pub collection_sha256: String,
}

/// Build-time canonical publisher trust-root-set variable.
pub const MODEL_CATALOG_TRUST_ROOTS_JSON_VAR: &str = "AEX_MODEL_CATALOG_TRUST_ROOTS_JSON";
/// Build-time exact trust-root-set digest variable.
pub const MODEL_CATALOG_TRUST_ROOTS_SHA256_VAR: &str = "AEX_MODEL_CATALOG_TRUST_ROOTS_SHA256";
/// Build-time exact collection-file variable.
pub const MODEL_CATALOG_COLLECTION_FILE_VAR: &str = "AEX_MODEL_CATALOG_COLLECTION_FILE";
/// Build-time exact collection digest variable.
pub const MODEL_CATALOG_COLLECTION_SHA256_VAR: &str = "AEX_MODEL_CATALOG_COLLECTION_SHA256";

const MODEL_CATALOG_TRUST_ROOTS_SCHEMA: &str = "aex.model-catalog-trust-roots.v1";
const MAX_MODEL_CATALOG_TRUST_ROOTS: usize = 8;
const MAX_MODEL_CATALOG_TRUST_ROOTS_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelCatalogTrustRoots {
    // Field order is JCS order for the restricted ASCII document below.
    keys: Vec<ModelCatalogTrustRoot>,
    schema: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ModelCatalogTrustRoot {
    // Field order is JCS order.
    key_id: String,
    sec1: String,
}

fn microvm_plan(unit: &Unit) -> Result<(Vec<String>, &'static str, String)> {
    let shape = unit.microvm.as_ref().ok_or_else(|| {
        ToolError::single(
            Exit::Usage,
            "artifact-microvm-shape-missing",
            format!("unit `{}` declares no MicroVM variant", unit.id),
        )
    })?;
    let output = format!("target/microvm/{}", unit.id);
    Ok((
        vec![
            "cargo".to_owned(),
            "run".to_owned(),
            "--locked".to_owned(),
            "--release".to_owned(),
            "--package".to_owned(),
            "hands-image".to_owned(),
            "--".to_owned(),
            "artifact".to_owned(),
            "--variant".to_owned(),
            shape.variant.clone(),
            "--out".to_owned(),
            output.clone(),
        ],
        "microvm-zip",
        output,
    ))
}

fn rust_plan(unit: &Unit) -> Result<Option<(Vec<String>, &'static str, String)>> {
    let binary = || {
        unit.bin.clone().ok_or_else(|| {
            ToolError::single(
                Exit::Usage,
                "artifact-binary-target-missing",
                format!(
                    "unit `{}` does not declare its Cargo binary target",
                    unit.id
                ),
            )
        })
    };
    let rust_target = unit.target.strip_suffix(".2.34").unwrap_or(&unit.target);
    let planned = match unit.kind.as_str() {
        "rust-lambda" => {
            let binary = binary()?;
            Some((
                vec![
                    "cargo".to_owned(),
                    "lambda".to_owned(),
                    "build".to_owned(),
                    "--profile".to_owned(),
                    unit.profile.clone(),
                    "--package".to_owned(),
                    unit.package.clone(),
                    "--target".to_owned(),
                    unit.target.clone(),
                ],
                "lambda-zip",
                format!("target/lambda/{binary}/bootstrap"),
            ))
        }
        kind @ ("rust-oci-service" | "rust-oci-task" | "rust-binary") => {
            let binary = binary()?;
            Some((
                vec![
                    "cargo".to_owned(),
                    "zigbuild".to_owned(),
                    "--release".to_owned(),
                    "--package".to_owned(),
                    unit.package.clone(),
                    "--target".to_owned(),
                    unit.target.clone(),
                ],
                if kind == "rust-binary" {
                    "tarball"
                } else {
                    "oci"
                },
                format!("target/{rust_target}/release/{binary}"),
            ))
        }
        _ => None,
    };
    Ok(planned)
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
    let (argv, form, input) = if let Some(planned) = rust_plan(unit)? {
        planned
    } else {
        match unit.kind.as_str() {
            // `cargo lambda build` drives `cargo zigbuild`, so the `bootstrap`
            // rename is native and the glibc floor is explicit rather than
            // whatever a container image happened to ship.
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
                format!("services/{}/dist/handler.js", unit.id),
            ),
            "build-output" => (
                vec!["bun".to_owned(), "run".to_owned(), "build".to_owned()],
                "build-output",
                format!("services/{}/dist", unit.id),
            ),
            "microvm-image" => microvm_plan(unit)?,
            other => {
                return Err(ToolError::single(
                    Exit::Usage,
                    "artifact-recipe-unknown",
                    format!("unit `{}` has kind `{other}`, which has no recipe", unit.id),
                ));
            }
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
        "input": input,
        "entrypoint": unit.entrypoint,
        "baseImage": unit.base_image,
    }))?;
    Ok(BuildPlan {
        unit: unit.id.clone(),
        kind: unit.kind.clone(),
        target: unit.target.clone(),
        profile: unit.profile.clone(),
        argv,
        env,
        form: form.to_owned(),
        input,
        entrypoint: unit.entrypoint.clone(),
        base_image: unit.base_image.clone(),
        digest,
    })
}

/// Derives a plan whose recorded environment exactly binds `brain-mux` to a
/// publisher trust-root set and collection. Other units ignore these inputs.
///
/// # Errors
///
/// Propagates recipe canonicalization failure.
pub fn plan_with_model_catalog(
    unit: &Unit,
    catalog: Option<&ModelCatalogBuildInputs>,
) -> Result<BuildPlan> {
    let mut build = plan(unit)?;
    if unit.id != "brain-mux" {
        return Ok(build);
    }
    if let Some(catalog) = catalog {
        validate_model_catalog_build_inputs(catalog)?;
        build.env.insert(
            MODEL_CATALOG_TRUST_ROOTS_JSON_VAR.to_owned(),
            catalog.trust_roots_json.clone(),
        );
        build.env.insert(
            MODEL_CATALOG_TRUST_ROOTS_SHA256_VAR.to_owned(),
            catalog.trust_roots_sha256.clone(),
        );
        build.env.insert(
            MODEL_CATALOG_COLLECTION_FILE_VAR.to_owned(),
            catalog.collection_file.clone(),
        );
        build.env.insert(
            MODEL_CATALOG_COLLECTION_SHA256_VAR.to_owned(),
            catalog.collection_sha256.clone(),
        );
        build.digest = build_plan_digest(&build)?;
    }
    Ok(build)
}

/// Reads the all-or-none build-time catalog inputs and verifies the collection
/// digest before a compiler sees them.
///
/// # Errors
///
/// Returns a usage error for partial/invalid bindings or an I/O error when the
/// exact collection cannot be read.
pub fn model_catalog_inputs_from_environment(
    workspace_root: &Path,
) -> Result<Option<ModelCatalogBuildInputs>> {
    model_catalog_inputs(
        workspace_root,
        [
            nonempty_environment(MODEL_CATALOG_TRUST_ROOTS_JSON_VAR),
            nonempty_environment(MODEL_CATALOG_TRUST_ROOTS_SHA256_VAR),
            nonempty_environment(MODEL_CATALOG_COLLECTION_FILE_VAR),
            nonempty_environment(MODEL_CATALOG_COLLECTION_SHA256_VAR),
        ],
    )
}

fn model_catalog_inputs(
    workspace_root: &Path,
    bindings: [Option<String>; 4],
) -> Result<Option<ModelCatalogBuildInputs>> {
    let [
        trust_roots_json,
        trust_roots_sha256,
        collection_file,
        collection_sha256,
    ] = bindings;
    let present = [
        trust_roots_json.is_some(),
        trust_roots_sha256.is_some(),
        collection_file.is_some(),
        collection_sha256.is_some(),
    ];
    if present.iter().any(|value| *value) && !present.iter().all(|value| *value) {
        return Err(ToolError::single(
            Exit::Usage,
            "model-catalog-build-binding-partial",
            format!(
                "{MODEL_CATALOG_TRUST_ROOTS_JSON_VAR}, \
                 {MODEL_CATALOG_TRUST_ROOTS_SHA256_VAR}, \
                 {MODEL_CATALOG_COLLECTION_FILE_VAR} and \
                 {MODEL_CATALOG_COLLECTION_SHA256_VAR} must be supplied together"
            ),
        ));
    }
    let (
        Some(trust_roots_json),
        Some(trust_roots_sha256),
        Some(collection_file),
        Some(collection_sha256),
    ) = (
        trust_roots_json,
        trust_roots_sha256,
        collection_file,
        collection_sha256,
    )
    else {
        return Ok(None);
    };
    let inputs = ModelCatalogBuildInputs {
        trust_roots_json,
        trust_roots_sha256,
        collection_file,
        collection_sha256,
    };
    validate_model_catalog_build_inputs(&inputs)?;
    let canonical_root = std::fs::canonicalize(workspace_root)
        .map_err(|error| io(&workspace_root.display().to_string(), &error))?;
    let logical_collection = workspace_root.join(&inputs.collection_file);
    let resolved_collection = std::fs::canonicalize(&logical_collection)
        .map_err(|error| io(&logical_collection.display().to_string(), &error))?;
    if !resolved_collection.starts_with(&canonical_root) {
        return Err(ToolError::single(
            Exit::Usage,
            "model-catalog-collection-path-escape",
            format!(
                "the release-bound collection `{}` resolves outside the workspace root",
                inputs.collection_file
            ),
        ));
    }
    let bytes = std::fs::read(&resolved_collection)
        .map_err(|error| io(&resolved_collection.display().to_string(), &error))?;
    if bytes.is_empty() {
        return Err(ToolError::single(
            Exit::Usage,
            "model-catalog-collection-empty",
            "the release-bound model catalog collection is empty",
        ));
    }
    let actual = canon::digest_bytes(&bytes);
    if actual != inputs.collection_sha256 {
        return Err(ToolError::single(
            Exit::Usage,
            "model-catalog-collection-digest-mismatch",
            format!(
                "the release-bound collection at `{}` is {actual}, not {}",
                inputs.collection_file, inputs.collection_sha256
            ),
        ));
    }
    Ok(Some(inputs))
}

/// Produces the release plan, including any exact build-bound catalog inputs.
///
/// # Errors
///
/// Propagates release-input and recipe failures.
pub fn release_plan(unit: &Unit, workspace_root: &Path) -> Result<BuildPlan> {
    let catalog = if unit.id == "brain-mux" {
        model_catalog_inputs_from_environment(workspace_root)?
    } else {
        None
    };
    plan_with_model_catalog(unit, catalog.as_ref())
}

/// Produces a publication plan and refuses an unbound `brain-mux` before build.
///
/// # Errors
///
/// Propagates release-input and recipe failures. `brain-mux` also fails when
/// the real release trust roots and signed collection are absent.
pub fn publication_plan(unit: &Unit, workspace_root: &Path) -> Result<BuildPlan> {
    let catalog = if unit.id == "brain-mux" {
        model_catalog_inputs_from_environment(workspace_root)?
    } else {
        None
    };
    publication_plan_with_model_catalog(unit, catalog.as_ref())
}

fn publication_plan_with_model_catalog(
    unit: &Unit,
    catalog: Option<&ModelCatalogBuildInputs>,
) -> Result<BuildPlan> {
    if unit.id == "brain-mux" && catalog.is_none() {
        return Err(ToolError::single(
            Exit::Usage,
            "model-catalog-build-binding-missing",
            "brain-mux publication requires the real build-bound publisher trust-root set and \
             signed catalog collection",
        ));
    }
    plan_with_model_catalog(unit, catalog)
}

fn nonempty_environment(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn validate_model_catalog_build_inputs(inputs: &ModelCatalogBuildInputs) -> Result<()> {
    validate_workspace_relative_path(&inputs.collection_file)?;
    validate_sha256(
        MODEL_CATALOG_TRUST_ROOTS_SHA256_VAR,
        &inputs.trust_roots_sha256,
    )?;
    validate_sha256(
        MODEL_CATALOG_COLLECTION_SHA256_VAR,
        &inputs.collection_sha256,
    )?;
    if inputs.trust_roots_json.len() > MAX_MODEL_CATALOG_TRUST_ROOTS_BYTES {
        return Err(ToolError::single(
            Exit::Usage,
            "model-catalog-trust-roots-too-large",
            format!(
                "the trust-root document is {} bytes, over the {}-byte bound",
                inputs.trust_roots_json.len(),
                MAX_MODEL_CATALOG_TRUST_ROOTS_BYTES
            ),
        ));
    }
    let roots: ModelCatalogTrustRoots =
        serde_json::from_str(&inputs.trust_roots_json).map_err(|error| {
            ToolError::single(
                Exit::Usage,
                "model-catalog-trust-roots-malformed",
                error.to_string(),
            )
        })?;
    if roots.schema != MODEL_CATALOG_TRUST_ROOTS_SCHEMA
        || roots.keys.is_empty()
        || roots.keys.len() > MAX_MODEL_CATALOG_TRUST_ROOTS
    {
        return Err(ToolError::single(
            Exit::Usage,
            "model-catalog-trust-roots-invalid",
            format!(
                "trust roots require schema `{MODEL_CATALOG_TRUST_ROOTS_SCHEMA}` and 1..={MAX_MODEL_CATALOG_TRUST_ROOTS} keys"
            ),
        ));
    }
    for root in &roots.keys {
        validate_trust_root(root)?;
    }
    if roots
        .keys
        .windows(2)
        .any(|pair| pair[0].key_id >= pair[1].key_id)
    {
        return Err(ToolError::single(
            Exit::Usage,
            "model-catalog-trust-roots-unsorted",
            "publisher trust-root key ids must be strictly sorted and unique",
        ));
    }
    let canonical = canon::to_string(&roots)?;
    if canonical != inputs.trust_roots_json {
        return Err(ToolError::single(
            Exit::Usage,
            "model-catalog-trust-roots-not-canonical",
            "publisher trust roots must be exact canonical JSON with no trailing bytes",
        ));
    }
    let actual = canon::digest_bytes(inputs.trust_roots_json.as_bytes());
    if actual != inputs.trust_roots_sha256 {
        return Err(ToolError::single(
            Exit::Usage,
            "model-catalog-trust-roots-digest-mismatch",
            format!(
                "the release-bound trust-root document is {actual}, not {}",
                inputs.trust_roots_sha256
            ),
        ));
    }
    Ok(())
}

fn validate_workspace_relative_path(path: &str) -> Result<()> {
    let valid = !path.is_empty()
        && !path.contains('\\')
        && path.split('/').all(|component| {
            !component.is_empty()
                && !matches!(component, "." | "..")
                && component
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        });
    if !valid {
        return Err(ToolError::single(
            Exit::Usage,
            "model-catalog-collection-path-unstable",
            "the collection path must be a normalized workspace-relative forward-slash path \
             with no empty, current, parent, absolute, drive, or runner-specific component",
        ));
    }
    Ok(())
}

fn validate_sha256(name: &str, digest: &str) -> Result<()> {
    if digest.len() != 71
        || !digest.starts_with("sha256:")
        || !digest[7..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(ToolError::single(
            Exit::Usage,
            "model-catalog-build-digest-invalid",
            format!("{name} must be a sha256: prefix and 64 lowercase hexadecimal digits"),
        ));
    }
    Ok(())
}

fn validate_trust_root(root: &ModelCatalogTrustRoot) -> Result<()> {
    let key_id_valid = !root.key_id.is_empty()
        && root.key_id.len() <= 64
        && root
            .key_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
    let sec1_valid = root.sec1.len() == 130
        && root.sec1.starts_with("04")
        && root
            .sec1
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
    let decoded = sec1_valid.then(|| hex::decode(&root.sec1).ok()).flatten();
    if !key_id_valid
        || decoded
            .as_deref()
            .is_none_or(|bytes| p256::ecdsa::VerifyingKey::from_sec1_bytes(bytes).is_err())
    {
        return Err(ToolError::single(
            Exit::Usage,
            "model-catalog-trust-root-invalid",
            format!(
                "publisher key `{}` is not a valid id and uncompressed lowercase P-256 SEC1 key",
                root.key_id
            ),
        ));
    }
    Ok(())
}

fn build_plan_digest(build: &BuildPlan) -> Result<String> {
    canon::digest_document(&serde_json::json!({
        "argv": build.argv,
        "env": build.env,
        "target": build.target,
        "profile": build.profile,
        "input": build.input,
        "entrypoint": build.entrypoint,
        "baseImage": build.base_image,
    }))
}

/// Every recipe in the registry.
///
/// # Errors
/// Propagates a unit with no recipe.
pub fn recipes(units: &Units) -> Result<Vec<BuildPlan>> {
    units.units.iter().map(plan).collect()
}

/// Every release recipe with exact build-bound inputs applied.
///
/// # Errors
///
/// Propagates any invalid release binding or recipe.
pub fn release_recipes(units: &Units, workspace_root: &Path) -> Result<Vec<BuildPlan>> {
    units
        .units
        .iter()
        .map(|unit| release_plan(unit, workspace_root))
        .collect()
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
        Form::MicrovmZip => {
            if !input.is_dir() {
                return Err(ToolError::single(
                    Exit::Usage,
                    "microvm-context-not-directory",
                    format!(
                        "`{}` is not a MicroVM service context directory",
                        input.display()
                    ),
                ));
            }
            let entries = collect_tree(input)?;
            pack::write_zip(&entries)
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
        Form::Oci => Err(ToolError::single(
            Exit::Usage,
            "artifact-form-requires-registry",
            "an OCI image is assembled over a digest-pinned base whose blobs come from a \
             registry and cannot be produced offline. Use `artifact plan` to print the exact \
             build invocation.",
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

const LOCATION_KINDS: &[&str] = &[
    "github-release",
    "oci",
    "s3",
    "ecr",
    "npm",
    "gha-artifact",
    "local",
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
        let positive_run = !self.source.workflow.run_id.is_empty()
            && !self.source.workflow.run_id.starts_with('0')
            && self
                .source
                .workflow
                .run_id
                .bytes()
                .all(|byte| byte.is_ascii_digit());
        if self.source.r#ref.as_deref() != Some("refs/heads/main")
            || self.source.workflow.repository != self.source.repository
            || self.source.workflow.r#ref != "refs/heads/main"
            || self.source.workflow.path != ".github/workflows/_build-artifacts.yml"
            || !positive_run
            || self.source.workflow.run_attempt == 0
        {
            structural.push(Violation::new(
                "envelope-workflow-identity",
                "published bytes must come from the exact protected-main reusable artifact workflow and a positive run identity",
            ));
        }
        if self.provenance.builder_id != self.source.workflow.builder_id {
            structural.push(Violation::new(
                "envelope-builder-mismatch",
                "the envelope workflow and provenance builder identities differ",
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
        if !LOCATION_KINDS.contains(&self.output.location.kind.as_str()) {
            structural.push(Violation::new(
                "envelope-location-kind",
                format!(
                    "location kind `{}` is outside the closed release vocabulary",
                    self.output.location.kind
                ),
            ));
        }
        match self.output.location.kind.as_str() {
            "github-release" => {
                let expected = crate::publication::github_release_unit_uri(
                    &self.source.repository,
                    &self.source.commit_sha,
                    &self.source.workflow.run_id,
                    u64::from(self.source.workflow.run_attempt),
                    &self.unit.id,
                    &self.output.digest,
                    &self.media.form,
                );
                match expected {
                    Ok(expected) if expected == self.output.location.uri => {}
                    Ok(expected) => structural.push(Violation::new(
                        "envelope-github-release-location",
                        format!(
                            "`{}` is not the exact source/run/content-bound URI `{expected}`",
                            self.output.location.uri
                        ),
                    )),
                    Err(err) => structural.extend(err.violations),
                }
                if self.unit.kind.starts_with("rust-oci-") {
                    structural.push(Violation::new(
                        "envelope-location-kind",
                        "OCI units must use a digest-only `oci` location, not a release tarball",
                    ));
                }
            }
            "oci" => {
                let expected = crate::publication::ghcr_unit_uri(
                    &self.source.repository,
                    &self.unit.id,
                    &self.output.digest,
                );
                match expected {
                    Ok(expected) if expected == self.output.location.uri => {}
                    Ok(expected) => structural.push(Violation::new(
                        "envelope-oci-location",
                        format!(
                            "`{}` is not the exact GHCR digest reference `{expected}`",
                            self.output.location.uri
                        ),
                    )),
                    Err(err) => structural.extend(err.violations),
                }
                if !self.unit.kind.starts_with("rust-oci-") {
                    structural.push(Violation::new(
                        "envelope-location-kind",
                        format!(
                            "unit kind `{}` is a blob and cannot claim an OCI manifest location",
                            self.unit.kind
                        ),
                    ));
                }
            }
            _ => {}
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
            if receipt.source.repository != self.source.repository
                || receipt.source.commit_sha != self.source.commit_sha
                || receipt.source.workflow_run_id != self.source.workflow.run_id
                || receipt.source.run_attempt != self.source.workflow.run_attempt
            {
                structural.push(Violation::new(
                    "envelope-receipt-binding",
                    format!(
                        "receipt class `{}` is not bound to this exact repository, commit and workflow attempt",
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
    let (kind, key) = match envelope.unit.kind.as_str() {
        "rust-lambda" | "ts-lambda" | "rust-binary" | "build-output" | "microvm-image" => (
            "github-release",
            crate::publication::unit_asset_name(
                &envelope.unit.id,
                &envelope.output.digest,
                &envelope.media.form,
            )?,
        ),
        "rust-oci-service" | "rust-oci-task" => (
            "oci",
            format!("{}@{}", envelope.unit.id, envelope.output.digest),
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
    use super::{
        Form, MODEL_CATALOG_TRUST_ROOTS_SCHEMA, ModelCatalogBuildInputs, model_catalog_inputs,
        package, plan, plan_with_model_catalog, publication_plan_with_model_catalog,
        validate_workspace_relative_path,
    };
    use crate::canon;
    use crate::graph::inputs::Unit;

    fn unit(kind: &str) -> Unit {
        toml::from_str(&format!(
            r#"
id = "regional-session-api"
kind = "{kind}"
plane = "regional"
package = "regional-session-api"
bin = "regional-session-api"
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

    fn brain_unit() -> Unit {
        let mut brain = unit("rust-oci-task");
        brain.id = "brain-mux".to_owned();
        brain.package = "brain-mux".to_owned();
        brain.bin = Some("brain-mux".to_owned());
        brain
    }

    fn trust_roots() -> String {
        let signing = p256::ecdsa::SigningKey::from_slice(&[7; 32]).expect("fixture key");
        canon::to_string(&serde_json::json!({
            "keys": [{
                "keyId": "aex-catalog-fixture",
                "sec1": hex::encode(
                    signing.verifying_key().to_sec1_point(false).as_bytes()
                ),
            }],
            "schema": MODEL_CATALOG_TRUST_ROOTS_SCHEMA,
        }))
        .expect("canonical trust roots")
    }

    fn bindings(root: &std::path::Path) -> [Option<String>; 4] {
        let relative = "release-inputs/catalog.json";
        let collection = root.join(relative);
        std::fs::create_dir_all(collection.parent().expect("collection parent")).unwrap();
        std::fs::write(&collection, b"signed collection").unwrap();
        let roots = trust_roots();
        [
            Some(roots.clone()),
            Some(canon::digest_bytes(roots.as_bytes())),
            Some(relative.to_owned()),
            Some(canon::digest_bytes(b"signed collection")),
        ]
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
    fn publication_refuses_an_unbound_brain_before_build_planning() {
        let error = publication_plan_with_model_catalog(&brain_unit(), None)
            .expect_err("publication must fail closed");
        assert_eq!(error.rules(), vec!["model-catalog-build-binding-missing"]);
    }

    #[test]
    fn partial_and_digest_mismatched_release_inputs_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let roots = trust_roots();
        let error = model_catalog_inputs(temp.path(), [Some(roots), None, None, None])
            .expect_err("partial inputs must fail");
        assert_eq!(error.rules(), vec!["model-catalog-build-binding-partial"]);

        let mut mismatched_roots = bindings(temp.path());
        mismatched_roots[1] = Some(format!("sha256:{}", "0".repeat(64)));
        let error = model_catalog_inputs(temp.path(), mismatched_roots)
            .expect_err("trust-root digest mismatch must fail");
        assert_eq!(
            error.rules(),
            vec!["model-catalog-trust-roots-digest-mismatch"]
        );

        let mut mismatched_collection = bindings(temp.path());
        mismatched_collection[3] = Some(format!("sha256:{}", "0".repeat(64)));
        let error = model_catalog_inputs(temp.path(), mismatched_collection)
            .expect_err("collection digest mismatch must fail");
        assert_eq!(
            error.rules(),
            vec!["model-catalog-collection-digest-mismatch"]
        );
    }

    #[test]
    fn collection_paths_are_stable_and_plans_ignore_checkout_roots() {
        for unstable in [
            "/release-inputs/catalog.json",
            "C:/release-inputs/catalog.json",
            "release-inputs/../catalog.json",
            "release-inputs\\catalog.json",
        ] {
            assert!(validate_workspace_relative_path(unstable).is_err());
        }

        let first_root = tempfile::tempdir().unwrap();
        let second_root = tempfile::tempdir().unwrap();
        let first = model_catalog_inputs(first_root.path(), bindings(first_root.path()))
            .expect("first binding")
            .expect("configured");
        let second = model_catalog_inputs(second_root.path(), bindings(second_root.path()))
            .expect("second binding")
            .expect("configured");
        assert_eq!(first, second);
        let first_plan = plan_with_model_catalog(&brain_unit(), Some(&first)).expect("first plan");
        let second_plan =
            plan_with_model_catalog(&brain_unit(), Some(&second)).expect("second plan");
        assert_eq!(first_plan.digest, second_plan.digest);
        assert_eq!(first_plan.env, second_plan.env);
    }

    #[test]
    fn trust_roots_require_canonical_sorted_unique_json() {
        let roots = trust_roots();
        let signing = p256::ecdsa::SigningKey::from_slice(&[8; 32]).expect("fixture key");
        let second = hex::encode(signing.verifying_key().to_sec1_point(false).as_bytes());
        let unsorted = format!(
            "{{\"keys\":[{{\"keyId\":\"z\",\"sec1\":\"{second}\"}},{}],\"schema\":\"{MODEL_CATALOG_TRUST_ROOTS_SCHEMA}\"}}",
            &roots[9..roots.find("],\"schema\"").expect("keys close")]
        );
        let inputs = ModelCatalogBuildInputs {
            trust_roots_sha256: canon::digest_bytes(unsorted.as_bytes()),
            trust_roots_json: unsorted,
            collection_file: "release-inputs/catalog.json".to_owned(),
            collection_sha256: canon::digest_bytes(b"signed collection"),
        };
        let error = plan_with_model_catalog(&brain_unit(), Some(&inputs))
            .expect_err("unsorted roots must fail");
        assert_eq!(error.rules(), vec!["model-catalog-trust-roots-unsorted"]);

        let noncanonical = ModelCatalogBuildInputs {
            trust_roots_sha256: canon::digest_bytes(format!("{roots}\n").as_bytes()),
            trust_roots_json: format!("{roots}\n"),
            collection_file: "release-inputs/catalog.json".to_owned(),
            collection_sha256: canon::digest_bytes(b"signed collection"),
        };
        let error = plan_with_model_catalog(&brain_unit(), Some(&noncanonical))
            .expect_err("trailing bytes must fail");
        assert_eq!(
            error.rules(),
            vec!["model-catalog-trust-roots-not-canonical"]
        );
    }

    #[test]
    fn an_unknown_unit_kind_has_no_recipe() {
        let err = plan(&unit("wasm-module")).unwrap_err();
        assert_eq!(err.rules(), vec!["artifact-recipe-unknown"]);
    }

    #[test]
    fn oci_packaging_refuses_rather_than_produce_a_partial_image() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("bin");
        std::fs::write(&input, b"x").unwrap();
        let err = package(Form::Oci, &input, 0, "bootstrap").unwrap_err();
        assert_eq!(err.rules(), vec!["artifact-form-requires-registry"]);
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
