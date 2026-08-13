//! Artifact recipes, deterministic packaging, and the artifact envelope.
//!
//! An artifact's identity is its bytes. The envelope records everything that
//! shaped those bytes — source commit, toolchain, lockfile, build argv,
//! input-closure digest, base image — plus the startup supply-chain state and
//! receipts earned before publication. Post-deployment evidence is deliberately
//! absent: an artifact cannot contain proof that only exists once it is
//! deployed, so smoke, e2e, user, capacity and soak receipts live in the
//! verification statement instead.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use regex::Regex;
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
    /// The exact tarball the npm packer produced.
    NpmTarball,
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

/// Build-time inputs that bind the release tool catalogue into Brain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogBuildInputs {
    /// SHA-256 of the immutable built-in tool catalogue compiled into Brain.
    pub tool_catalog_sha256: String,
}

/// Build-time exact built-in tool catalogue digest variable.
pub const TOOL_CATALOG_SHA256_VAR: &str = "AEX_TOOL_CATALOG_SHA256";

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
                    // Node 22 loads a bare `handler.js` in a zip with no
                    // `package.json` as CommonJS; `bun build --target=node`
                    // emits ESM, which would raise UserCodeSyntaxError on
                    // every invoke. Pin the bundle format to CommonJS so the
                    // archive recipe and the runtime loading rule agree.
                    "--format=cjs".to_owned(),
                    "--outdir".to_owned(),
                    format!("services/{}/dist", unit.id),
                    format!("services/{}/src/handler.ts", unit.id),
                ],
                "lambda-zip",
                format!("services/{}/dist/handler.js", unit.id),
            ),
            "build-output" => (
                vec![
                    "bun".to_owned(),
                    "run".to_owned(),
                    "build:dashboard-output".to_owned(),
                ],
                "build-output",
                ".vercel/output".to_owned(),
            ),
            // The registry publishes the packer's own tarball, so the packer is
            // the recipe: re-archiving those bytes deterministically here would
            // produce a file `npm publish` has never seen and an integrity value
            // no installer would ever compute.
            //
            // `bun pm pack` has no workspace filter and packs whichever package
            // `--cwd` names; the unit id is that directory's name, the same
            // convention `ts-lambda` uses for `services/<id>`, and `graph
            // verify` refuses a row whose owning npm member sits anywhere else.
            // `--filename` resolves against the process directory rather than
            // `--cwd`, and bun refuses it together with `--destination`, so one
            // repository-relative path is both the flag and the recorded output.
            "npm-package" => (
                vec![
                    "bun".to_owned(),
                    "pm".to_owned(),
                    "pack".to_owned(),
                    "--cwd".to_owned(),
                    format!("packages/{}", unit.id),
                    "--filename".to_owned(),
                    npm_pack_path(&unit.id),
                ],
                "npm-tarball",
                npm_pack_path(&unit.id),
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

/// Repository-relative directory an npm unit's tarball is packed into.
#[must_use]
pub fn npm_pack_directory(unit: &str) -> String {
    format!("target/npm/{unit}")
}

/// Fixed tarball basename for an npm unit.
///
/// The packer's default name embeds the version, which would make the recipe's
/// declared output path change every time the package version does. A fixed
/// name keeps the recipe a function of the unit alone; the version is recorded
/// in the publication identity, where it is checked against the manifest.
#[must_use]
pub fn npm_pack_filename(unit: &str) -> String {
    format!("{unit}.tgz")
}

/// Repository-relative path to the exact bytes an npm unit publishes.
#[must_use]
pub fn npm_pack_path(unit: &str) -> String {
    format!("{}/{}", npm_pack_directory(unit), npm_pack_filename(unit))
}

/// Whether a unit kind's artifact must carry a signature before publication.
///
/// Two kinds cross a distribution boundary this repository does not control: a
/// binary a customer downloads, and a package a registry serves. Everything
/// else is fetched by digest from a location the composition already pins.
#[must_use]
pub fn kind_requires_signature(kind: &str) -> bool {
    matches!(kind, "rust-binary" | "npm-package")
}

/// Derives a plan whose recorded environment exactly binds the release tool
/// catalogue into the Brain binaries. Other units ignore these inputs.
///
/// # Errors
///
/// Propagates recipe canonicalization failure.
pub fn plan_with_catalog(unit: &Unit, catalog: Option<&CatalogBuildInputs>) -> Result<BuildPlan> {
    let mut build = plan(unit)?;
    if !requires_catalog_binding(unit) {
        return Ok(build);
    }
    if let Some(catalog) = catalog {
        validate_sha256(TOOL_CATALOG_SHA256_VAR, &catalog.tool_catalog_sha256)?;
        build.env.insert(
            TOOL_CATALOG_SHA256_VAR.to_owned(),
            catalog.tool_catalog_sha256.clone(),
        );
        build.digest = build_plan_digest(&build)?;
    }
    Ok(build)
}

/// Reads the build-time tool-catalogue input and verifies its digest before a
/// compiler sees it.
///
/// # Errors
///
/// Returns a usage error for a partial or mismatched binding, or an I/O error
/// when the catalogue source cannot be read.
pub fn catalog_inputs_from_environment(
    workspace_root: &Path,
) -> Result<Option<CatalogBuildInputs>> {
    let tool_catalog_sha256 = nonempty_environment(TOOL_CATALOG_SHA256_VAR);
    let Some(tool_catalog_sha256) = tool_catalog_sha256 else {
        return Ok(None);
    };
    let inputs = CatalogBuildInputs {
        tool_catalog_sha256,
    };
    validate_sha256(TOOL_CATALOG_SHA256_VAR, &inputs.tool_catalog_sha256)?;
    let actual_tool_catalog = tool_catalog_digest(workspace_root)?;
    if inputs.tool_catalog_sha256 != actual_tool_catalog {
        return Err(ToolError::single(
            Exit::Usage,
            "tool-catalog-build-digest-mismatch",
            format!(
                "the release-bound tool catalogue is {actual_tool_catalog}, not {}",
                inputs.tool_catalog_sha256
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
    let catalog = if requires_catalog_binding(unit) {
        catalog_inputs_from_environment(workspace_root)?
    } else {
        None
    };
    plan_with_catalog(unit, catalog.as_ref())
}

/// Produces a publication plan and refuses an unbound catalog consumer before build.
///
/// # Errors
///
/// Propagates release-input and recipe failures. Catalog consumers also fail
/// when the real release tool-catalogue digest is absent.
pub fn publication_plan(unit: &Unit, workspace_root: &Path) -> Result<BuildPlan> {
    let catalog = if requires_catalog_binding(unit) {
        catalog_inputs_from_environment(workspace_root)?
    } else {
        None
    };
    publication_plan_with_catalog(unit, catalog.as_ref())
}

fn publication_plan_with_catalog(
    unit: &Unit,
    catalog: Option<&CatalogBuildInputs>,
) -> Result<BuildPlan> {
    if requires_catalog_binding(unit) && catalog.is_none() {
        return Err(ToolError::single(
            Exit::Usage,
            "tool-catalog-build-binding-missing",
            format!(
                "{} publication requires the real build-bound tool-catalogue digest",
                unit.id
            ),
        ));
    }
    plan_with_catalog(unit, catalog)
}

pub(crate) fn requires_catalog_binding(unit: &Unit) -> bool {
    matches!(unit.id.as_str(), "brain-mux" | "session-stream-api")
}

fn nonempty_environment(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// Read the snapshot-bound immutable tool-catalogue digest from Brain source.
///
/// The catalogue crate proves in its property suite that this constant equals
/// the SHA-256 of its canonical built-in rows. Reading the source constant here
/// avoids coupling the small release tool to Brain's runtime dependency graph.
///
/// # Errors
/// Returns a classified refusal if the source omits or ambiguously declares
/// the snapshot identity.
///
/// # Panics
/// The regular expression is a compile-time constant; construction can only
/// fail if this source is changed to contain an invalid expression.
pub fn tool_catalog_digest(workspace_root: &Path) -> Result<String> {
    let path = workspace_root.join("crates/aex-brain-tool-catalog/src/catalog.rs");
    let source =
        std::fs::read_to_string(&path).map_err(|error| io(&path.display().to_string(), &error))?;
    let pattern = Regex::new(
        r#"(?s)pub\s+const\s+BUILTIN_CATALOG_DIGEST\s*:\s*&str\s*=\s*"(sha256:[0-9a-f]{64})"\s*;"#,
    )
    .expect("static tool catalogue identity regex");
    let digests = pattern
        .captures_iter(&source)
        .map(|capture| capture[1].to_owned())
        .collect::<Vec<_>>();
    if digests.len() != 1 {
        return Err(ToolError::single(
            Exit::Usage,
            "tool-catalog-snapshot-ambiguous",
            "Brain source must declare exactly one lowercase snapshot-bound tool catalogue digest",
        ));
    }
    Ok(digests[0].clone())
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

pub(crate) fn build_plan_digest(build: &BuildPlan) -> Result<String> {
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
        Form::Tarball => {
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
        Form::BuildOutput => {
            let entries = collect_build_output(input)?;
            pack::write_tar_gz(&entries, source_date_epoch)
        }
        // Deliberately a passthrough. The published artifact is the packer's
        // tarball, byte for byte: an installer verifies the registry's own
        // integrity value over exactly these bytes, so anything this function
        // rewrote would break that check while still looking packaged.
        Form::NpmTarball => {
            let data =
                std::fs::read(input).map_err(|err| io(&input.display().to_string(), &err))?;
            if data.is_empty() {
                return Err(ToolError::single(
                    Exit::Usage,
                    "npm-tarball-empty",
                    format!("`{}` is an empty npm tarball", input.display()),
                ));
            }
            Ok(data)
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

fn collect_build_output(root: &Path) -> Result<Vec<pack::Entry>> {
    let canonical_root = validate_build_output_root(root)?;
    let mut files = BTreeMap::<String, pack::Entry>::new();
    let mut function_configs = 0usize;
    for entry in walkdir::WalkDir::new(&canonical_root)
        .follow_links(false)
        .sort_by_file_name()
    {
        let entry = entry.map_err(|err| {
            ToolError::single(Exit::Usage, "io", format!("walking build output: {err}"))
        })?;
        if entry.file_type().is_dir() {
            continue;
        }
        let relative = entry.path().strip_prefix(&canonical_root).map_err(|_| {
            ToolError::single(
                Exit::Usage,
                "build-output-symlink-escape",
                format!(
                    "Build Output API member `{}` escaped its root",
                    entry.path().display()
                ),
            )
        })?;
        let name = archive_name(relative)?;
        let packaged = if entry.file_type().is_file() {
            let data = std::fs::read(entry.path())
                .map_err(|err| io(&entry.path().display().to_string(), &err))?;
            if entry.file_name() == ".vc-config.json" {
                validate_standalone_function_config(entry.path(), &data)?;
                function_configs += 1;
            }
            pack::Entry::regular(&name, data)
        } else if entry.file_type().is_symlink() {
            let target = validate_build_output_symlink(entry.path(), &canonical_root)?;
            pack::Entry::symlink(&name, &target)
        } else {
            return Err(ToolError::single(
                Exit::Usage,
                "build-output-non-regular",
                format!(
                    "Build Output API member `{}` is not a regular file or symlink",
                    entry.path().display()
                ),
            ));
        };
        if files.insert(name.clone(), packaged).is_some() {
            return Err(ToolError::single(
                Exit::Usage,
                "build-output-member-collision",
                format!("Build Output API declares member `{name}` twice"),
            ));
        }
    }
    if function_configs == 0 {
        return Err(ToolError::single(
            Exit::Usage,
            "build-output-function-config",
            "standalone Build Output API has no physical .vc-config.json",
        ));
    }
    validate_build_output_links(&files)?;
    Ok(files.into_values().collect())
}

fn validate_build_output_root(root: &Path) -> Result<PathBuf> {
    let canonical =
        std::fs::canonicalize(root).map_err(|err| io(&root.display().to_string(), &err))?;
    if !canonical.is_dir() {
        return Err(ToolError::single(
            Exit::Usage,
            "build-output-not-directory",
            format!("`{}` is not a Build Output API directory", root.display()),
        ));
    }
    let config: serde_json::Value = serde_json::from_slice(
        &std::fs::read(canonical.join("config.json"))
            .map_err(|err| io("Build Output API config.json", &err))?,
    )
    .map_err(|err| {
        ToolError::single(
            Exit::Usage,
            "build-output-config",
            format!("Build Output API config.json is not JSON: {err}"),
        )
    })?;
    if config.get("version").and_then(serde_json::Value::as_u64) != Some(3) {
        return Err(ToolError::single(
            Exit::Usage,
            "build-output-config",
            "Build Output API config.json must declare version 3",
        ));
    }
    Ok(canonical)
}

fn validate_build_output_links(files: &BTreeMap<String, pack::Entry>) -> Result<()> {
    let symlinks: Vec<_> = files
        .values()
        .filter(|entry| entry.is_symlink())
        .map(|entry| format!("{}/", entry.name))
        .collect();
    if let Some((member, link)) = files.keys().find_map(|member| {
        symlinks
            .iter()
            .find(|link| member.starts_with(link.as_str()))
            .map(|link| (member, link))
    }) {
        return Err(ToolError::single(
            Exit::Usage,
            "build-output-symlink-nesting",
            format!("Build Output API member `{member}` is nested below symlink `{link}`"),
        ));
    }
    for entry in files.values().filter(|entry| entry.is_symlink()) {
        let target = pack::resolve_link_name(
            &entry.name,
            entry.link_name.as_deref().expect("filtered symbolic link"),
        )
        .ok_or_else(|| {
            ToolError::single(
                Exit::Usage,
                "build-output-symlink-target",
                format!(
                    "Build Output API symlink `{}` escapes the archive",
                    entry.name
                ),
            )
        })?;
        let directory_prefix = format!("{target}/");
        let exact_regular = files
            .get(&target)
            .is_some_and(|candidate| !candidate.is_symlink());
        let populated_directory = files
            .keys()
            .any(|candidate| candidate.starts_with(&directory_prefix));
        if !exact_regular && !populated_directory {
            return Err(ToolError::single(
                Exit::Usage,
                "build-output-symlink-unpackaged",
                format!(
                    "Build Output API symlink `{}` resolves to `{target}`, which has no packaged file or populated directory",
                    entry.name
                ),
            ));
        }
    }
    Ok(())
}

fn archive_name(path: &Path) -> Result<String> {
    path.to_str()
        .map(|value| value.replace('\\', "/"))
        .ok_or_else(|| {
            ToolError::single(
                Exit::Usage,
                "archive-entry-name",
                format!("Build Output API member `{}` is not UTF-8", path.display()),
            )
        })
}

fn validate_standalone_function_config(path: &Path, data: &[u8]) -> Result<()> {
    let config: serde_json::Value = serde_json::from_slice(data).map_err(|err| {
        ToolError::single(
            Exit::Usage,
            "build-output-function-config",
            format!("`{}` is not JSON: {err}", path.display()),
        )
    })?;
    if config.get("filePathMap").is_some() {
        return Err(ToolError::single(
            Exit::Usage,
            "build-output-not-standalone",
            format!("`{}` still declares filePathMap", path.display()),
        ));
    }
    Ok(())
}

fn validate_build_output_symlink(path: &Path, root: &Path) -> Result<String> {
    let target = std::fs::read_link(path).map_err(|err| io(&path.display().to_string(), &err))?;
    let target_text = target.to_str().ok_or_else(|| {
        ToolError::single(
            Exit::Usage,
            "build-output-symlink-target",
            format!(
                "Build Output API symlink `{}` has a non-UTF-8 target",
                path.display()
            ),
        )
    })?;
    if !safe_relative_symlink_target(target_text) {
        return Err(ToolError::single(
            Exit::Usage,
            "build-output-symlink-target",
            format!(
                "Build Output API symlink `{}` has a non-normal relative target",
                path.display()
            ),
        ));
    }
    let resolved = std::fs::canonicalize(path).map_err(|err| {
        ToolError::single(
            Exit::Usage,
            "build-output-symlink-invalid",
            format!(
                "Build Output API symlink `{}` is broken or cyclic: {err}",
                path.display()
            ),
        )
    })?;
    if !resolved.starts_with(root) {
        return Err(ToolError::single(
            Exit::Usage,
            "build-output-symlink-escape",
            format!(
                "Build Output API symlink `{}` resolves outside its root",
                path.display()
            ),
        ));
    }
    let parent = std::fs::canonicalize(path.parent().unwrap_or(root))
        .map_err(|err| io(&path.display().to_string(), &err))?;
    if resolved.is_dir() && parent.starts_with(&resolved) {
        return Err(ToolError::single(
            Exit::Usage,
            "build-output-symlink-cycle",
            format!(
                "Build Output API symlink `{}` points to an ancestor",
                path.display()
            ),
        ));
    }
    Ok(target_text.to_owned())
}

fn safe_relative_symlink_target(target: &str) -> bool {
    if target.is_empty()
        || target.starts_with('/')
        || target.contains('\\')
        || target.as_bytes().get(1) == Some(&b':')
        || Path::new(target).is_absolute()
    {
        return false;
    }
    let mut saw_named_segment = false;
    for segment in target.split('/') {
        if segment.is_empty() || segment == "." {
            return false;
        }
        if segment == ".." {
            if saw_named_segment {
                return false;
            }
        } else {
            saw_named_segment = true;
        }
    }
    true
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
    /// Receipt-independent identity of the artifact bytes and everything that
    /// shaped them.
    pub artifact_subject_digest: String,
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
    /// Whether startup-mode publication deliberately deferred dependency,
    /// licence, vulnerability and SBOM analysis off the release critical path.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub supply_chain_deferred: bool,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// The image configuration digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oci_config_digest: Option<String>,
    /// Every compressed image layer digest, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub oci_layer_digests: Vec<String>,
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Licenses {
    /// Digest of the policy that produced the verdict.
    pub policy_digest: String,
    /// `allowed` when scanned, or the explicit startup deferral sentinel.
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Vulnerabilities {
    /// Scanner identity.
    pub scanner: String,
    /// Advisory database identity.
    pub database: String,
    /// When the scan ran.
    pub scanned_at: String,
    /// Zero for certified or deferred startup publication.
    pub unapproved_critical: u32,
    /// Zero for certified or deferred startup publication.
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
    /// Recompute the receipt-independent artifact subject identity.
    ///
    /// The subject deliberately excludes the workflow run, publication
    /// location, supply-chain verdicts and receipt references. Those fields
    /// describe who certified or where content-addressed bytes are stored; they
    /// do not change the bytes, source or build-input closure a receipt tested.
    /// The complete output identity remains in the projection, including OCI
    /// manifest, config and layer digests.
    ///
    /// # Errors
    /// Propagates serialization or canonicalization failure.
    pub fn compute_artifact_subject_digest(&self) -> Result<String> {
        let mut source = serde_json::to_value(&self.source).map_err(|err| {
            ToolError::single(
                Exit::EnvelopeInvalid,
                "artifact-subject-unserializable",
                err.to_string(),
            )
        })?;
        let source = source.as_object_mut().ok_or_else(|| {
            ToolError::single(
                Exit::EnvelopeInvalid,
                "artifact-subject-source-shape",
                "artifact source did not serialize as an object",
            )
        })?;
        source.remove("workflow");

        let mut output = serde_json::to_value(&self.output).map_err(|err| {
            ToolError::single(
                Exit::EnvelopeInvalid,
                "artifact-subject-unserializable",
                err.to_string(),
            )
        })?;
        let output = output.as_object_mut().ok_or_else(|| {
            ToolError::single(
                Exit::EnvelopeInvalid,
                "artifact-subject-output-shape",
                "artifact output did not serialize as an object",
            )
        })?;
        output.remove("location");

        canon::digest_document(&serde_json::json!({
            "schema": "aex.artifact-subject.v1",
            "unit": &self.unit,
            "media": &self.media,
            "source": source,
            "inputs": &self.inputs,
            "output": output,
        }))
    }

    /// Recompute and set the self-digest.
    ///
    /// # Errors
    /// Propagates canonicalization failure.
    pub fn seal(mut self) -> Result<Self> {
        self.artifact_subject_digest = self.compute_artifact_subject_digest()?;
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
            // The registry name and version are the whole of an npm identity, and
            // they live in the URI rather than in the unit id: a unit id matches
            // `^[a-z][a-z0-9-]{2,63}$` and can therefore never be a scoped npm
            // name. Re-deriving the canonical tarball location from what was
            // parsed is what makes a redirect or a hand-edited path a refusal.
            "npm" => {
                if let Err(err) =
                    crate::publication::npm_tarball_identity(&self.output.location.uri)
                {
                    structural.extend(err.violations);
                }
            }
            _ => {}
        }
        if (self.unit.kind == "npm-package") != (self.output.location.kind == "npm") {
            structural.push(Violation::new(
                "envelope-location-kind",
                format!(
                    "unit kind `{}` and location kind `{}` disagree about registry publication",
                    self.unit.kind, self.output.location.kind
                ),
            ));
        }
        if self.unit.kind == "npm-package" && self.media.form != "npm-tarball" {
            structural.push(Violation::new(
                "envelope-media-form",
                format!(
                    "an npm package publishes the packer's tarball, not media form `{}`",
                    self.media.form
                ),
            ));
        }
        let is_oci = self.unit.kind.starts_with("rust-oci-");
        let oci_identity_valid = self.output.oci_index_digest.is_none()
            && self.output.oci_child_digest.as_deref() == Some(&self.output.digest)
            && self
                .output
                .oci_config_digest
                .as_deref()
                .is_some_and(valid_sha256_digest)
            && !self.output.oci_layer_digests.is_empty()
            && self
                .output
                .oci_layer_digests
                .iter()
                .all(|digest| valid_sha256_digest(digest));
        if is_oci && !oci_identity_valid {
            structural.push(Violation::new(
                "envelope-oci-identity",
                "single-platform OCI output must bind its manifest, config and every layer digest",
            ));
        }
        if !is_oci
            && (self.output.oci_index_digest.is_some()
                || self.output.oci_child_digest.is_some()
                || self.output.oci_config_digest.is_some()
                || !self.output.oci_layer_digests.is_empty())
        {
            structural.push(Violation::new(
                "envelope-oci-identity",
                "blob output cannot carry OCI manifest, config or layer identities",
            ));
        }
        if self.receipts.is_empty() {
            structural.push(Violation::new(
                "envelope-no-receipts",
                "the envelope carries no receipt; publication would prove nothing",
            ));
        }
        let deferred_supply_chain_shape = self.sbom.format == "deferred-startup"
            && self.sbom.digest.is_empty()
            && self.sbom.uri.is_empty()
            && self.sbom.component_count == 0
            && self.licenses.policy_digest.is_empty()
            && self.licenses.verdict == "deferred-startup"
            && self.licenses.denials.is_empty()
            && self.licenses.inventory_digest.is_none()
            && self.vulnerabilities.scanner == "deferred-startup"
            && self.vulnerabilities.database.is_empty()
            && self.vulnerabilities.scanned_at.is_empty()
            && self.vulnerabilities.unapproved_critical == 0
            && self.vulnerabilities.unapproved_high == 0
            && self.vulnerabilities.approved_exceptions.is_empty();
        let scanned_supply_chain_shape =
            matches!(self.sbom.format.as_str(), "spdx-2.3" | "cyclonedx-1.6")
                && valid_sha256_digest(&self.sbom.digest)
                && valid_sha256_digest(&self.licenses.policy_digest)
                && self.licenses.verdict != "deferred-startup"
                && self
                    .licenses
                    .inventory_digest
                    .as_deref()
                    .is_none_or(valid_sha256_digest)
                && !self.vulnerabilities.scanner.is_empty()
                && self.vulnerabilities.scanner != "deferred-startup"
                && !self.vulnerabilities.database.is_empty()
                && time::OffsetDateTime::parse(
                    &self.vulnerabilities.scanned_at,
                    &time::format_description::well_known::Rfc3339,
                )
                .is_ok();
        if self.supply_chain_deferred && !deferred_supply_chain_shape {
            structural.push(Violation::new(
                "envelope-deferred-supply-chain-shape",
                "a deferred supply chain must carry only the exact startup deferral sentinels",
            ));
        } else if !self.supply_chain_deferred && deferred_supply_chain_shape {
            structural.push(Violation::new(
                "envelope-deferred-supply-chain-flag",
                "the startup deferral sentinels require supplyChainDeferred=true",
            ));
        } else if !self.supply_chain_deferred && !scanned_supply_chain_shape {
            structural.push(Violation::new(
                "envelope-scanned-supply-chain-shape",
                "a non-deferred supply chain must carry complete scanner-backed evidence",
            ));
        }
        for receipt in &self.receipts {
            if self.supply_chain_deferred
                && matches!(
                    receipt.class.as_str(),
                    "deny" | "sbom" | "license" | "vulnerability"
                )
            {
                structural.push(Violation::new(
                    "envelope-deferred-supply-chain-receipt",
                    format!(
                        "deferred supply-chain envelope cannot claim a `{}` receipt",
                        receipt.class
                    ),
                ));
            }
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
        let recomputed_subject = self.compute_artifact_subject_digest()?;
        if recomputed_subject != self.artifact_subject_digest {
            structural.push(Violation::new(
                "artifact-subject-digest-mismatch",
                format!(
                    "recorded artifactSubjectDigest `{}` does not match the canonical artifact subject `{recomputed_subject}`",
                    self.artifact_subject_digest
                ),
            ));
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
        if !self.supply_chain_deferred
            && (self.licenses.verdict != "allowed" || !self.licenses.denials.is_empty())
        {
            supply.push(Violation::new(
                "license-denied",
                format!(
                    "unit `{}` carries {} licence denial(s)",
                    self.unit.id,
                    self.licenses.denials.len()
                ),
            ));
        }
        if !self.supply_chain_deferred
            && (self.vulnerabilities.unapproved_critical > 0
                || self.vulnerabilities.unapproved_high > 0)
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
        if !self.supply_chain_deferred && self.sbom.component_count == 0 {
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

fn valid_sha256_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|bare| {
        bare.len() == 64
            && bare
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    })
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
/// The registry row is required, not optional: a unit id matches
/// `^[a-z][a-z0-9-]{2,63}$` and so can never be a scoped npm package name, and
/// the only place that mapping is declared is the row's own `package`.
///
/// # Errors
/// Returns [`Exit::EnvelopeInvalid`] for a unit kind with no publication rule,
/// a row that does not describe this envelope, or an unpublishable npm name.
pub fn publish_destination(envelope: &ArtifactEnvelope, unit: &Unit) -> Result<PublishDestination> {
    if unit.id != envelope.unit.id || unit.kind != envelope.unit.kind {
        return Err(ToolError::single(
            Exit::EnvelopeInvalid,
            "publish-destination-unit-mismatch",
            format!(
                "registry row `{}`/`{}` does not describe envelope unit `{}`/`{}`",
                unit.id, unit.kind, envelope.unit.id, envelope.unit.kind
            ),
        ));
    }
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
        "npm-package" => {
            crate::publication::split_npm_package(&unit.package)?;
            ("npm", unit.package.clone())
        }
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
        CatalogBuildInputs, Form, package, plan, plan_with_catalog, publication_plan_with_catalog,
    };
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

    fn session_stream_unit() -> Unit {
        let mut session = unit("rust-oci-service");
        session.id = "session-stream-api".to_owned();
        session.package = "session-stream-api".to_owned();
        session.bin = Some("session-stream-api".to_owned());
        session
    }

    fn tool_catalog_digest_fixture() -> String {
        "sha256:51e0b52e74bfd7883bf6dd5ac915d745cb54a7360ecb447cbeec59955ae61fdb".to_owned()
    }

    fn catalog_bindings() -> CatalogBuildInputs {
        CatalogBuildInputs {
            tool_catalog_sha256: tool_catalog_digest_fixture(),
        }
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
    fn the_dashboard_recipe_builds_a_portable_vercel_output_from_the_workspace_root() {
        let mut dashboard = unit("build-output");
        dashboard.id = "dashboard".to_owned();
        dashboard.package = "@aexhq/dashboard".to_owned();
        dashboard.bin = None;
        dashboard.target = "none".to_owned();
        dashboard.profile = "release".to_owned();
        dashboard.form = "tar.gz".to_owned();

        let planned = plan(&dashboard).unwrap();
        assert_eq!(planned.argv, vec!["bun", "run", "build:dashboard-output"]);
        assert_eq!(planned.input, ".vercel/output");
        assert_eq!(planned.form, "build-output");
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
    fn publication_refuses_every_unbound_catalog_consumer_before_build_planning() {
        for unit in [brain_unit(), session_stream_unit()] {
            let error = publication_plan_with_catalog(&unit, None)
                .expect_err("publication must fail closed");
            assert_eq!(
                error.rules(),
                vec!["tool-catalog-build-binding-missing"],
                "{}",
                unit.id
            );
        }
    }

    #[test]
    fn a_mismatched_tool_catalog_digest_fails_closed() {
        let inputs = CatalogBuildInputs {
            tool_catalog_sha256: "not-a-digest".to_owned(),
        };
        let error = plan_with_catalog(&brain_unit(), Some(&inputs))
            .expect_err("a malformed digest must fail");
        assert_eq!(error.rules(), vec!["model-catalog-build-digest-invalid"]);
    }

    #[test]
    fn a_bound_plan_records_exactly_the_tool_catalog_digest() {
        let inputs = catalog_bindings();
        let planned = plan_with_catalog(&brain_unit(), Some(&inputs)).expect("plan");
        assert_eq!(
            planned.env.get(super::TOOL_CATALOG_SHA256_VAR),
            Some(&inputs.tool_catalog_sha256)
        );
        assert_eq!(planned.env.len(), 4, "{:?}", planned.env);
        let bare = plan_with_catalog(&unit("rust-oci-service"), None).expect("plan");
        assert!(!bare.env.contains_key(super::TOOL_CATALOG_SHA256_VAR));
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
        let tree = temp.path().join(".vercel/output");
        std::fs::create_dir_all(tree.join("static")).unwrap();
        std::fs::create_dir_all(tree.join("functions/index.func")).unwrap();
        std::fs::write(tree.join("config.json"), b"{\"version\":3}\n").unwrap();
        std::fs::write(
            tree.join("functions/index.func/.vc-config.json"),
            b"{\"runtime\":\"nodejs22.x\"}\n",
        )
        .unwrap();
        std::fs::write(tree.join("static/app.js"), b"console.log(1)").unwrap();
        let first = package(Form::BuildOutput, &tree, 0, "bootstrap").unwrap();
        let second = package(Form::BuildOutput, &tree, 0, "bootstrap").unwrap();
        assert_eq!(first, second);
    }
}
