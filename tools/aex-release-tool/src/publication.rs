//! Deterministic, plane-neutral public release inputs.
//!
//! The hosted release engine consumes a raw Linux release-tool executable and
//! a Terraform module bundle. Both are public source artifacts: they are built
//! without a plane binding, cloud credentials, or private repository content.

use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::canon;
use crate::error::{Exit, Result, ToolError, Violation};
use crate::pack::{Entry, SOURCE_DATE_EPOCH_DEFAULT, write_tar_gz};

/// Fixed public asset name for the raw Linux `x86_64` release tool.
pub const RELEASE_TOOL_ASSET: &str = "aex-release-tool";
/// Fixed public asset name for the Terraform module source bundle.
pub const MODULE_BUNDLE_ASSET: &str = "terraform-modules.tar.gz";
/// Fixed public asset name for the generated regional table definition bundle.
pub const REGIONAL_TABLES_ASSET: &str = "regional-tables.json";

/// Exact identities carried by the generated regional table bundle.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegionalTablesIdentity {
    /// SHA-256 over the transported JSON bytes.
    pub digest: String,
    /// Exact transported byte length.
    pub size_bytes: u64,
    /// BLAKE3 identity over the canonical table-definition array.
    pub definitions_digest: String,
    /// Authored monotone regional schema generation.
    pub generation: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegionalTablesDocument {
    schema: String,
    generation: u32,
    digest: String,
    tables: Vec<serde_json::Value>,
}

/// Closed naming pair for an auxiliary unit asset.
#[derive(Debug, Clone, Copy)]
pub struct AuxiliaryAsset<'a> {
    /// Evidence class encoded in the basename.
    pub class: &'a str,
    /// Exact safe suffix encoded in the basename.
    pub extension: &'a str,
}

/// The closed, plane-neutral public location for one non-OCI unit artifact.
///
/// The release tag binds source commit and workflow run/attempt. The asset
/// basename additionally binds the unit and byte digest, so neither a retry nor
/// a second unit can alias an already named object.
///
/// # Errors
/// Returns [`Exit::EnvelopeInvalid`] for an invalid source identity, unit id,
/// digest, or artifact form.
pub fn github_release_unit_uri(
    repository: &str,
    commit_sha: &str,
    run_id: &str,
    run_attempt: u64,
    unit: &str,
    digest: &str,
    form: &str,
) -> Result<String> {
    validate_public_identity(repository, commit_sha, run_id, run_attempt, unit, digest)?;
    let asset = unit_asset_name(unit, digest, form)?;
    Ok(format!(
        "https://github.com/{repository}/releases/download/main-{commit_sha}-run-{run_id}-attempt-{run_attempt}/{asset}"
    ))
}

/// The closed, plane-neutral OCI reference for one service/task image.
///
/// Public CI publishes to GHCR. Private release later performs a
/// registry-to-registry copy to ECR and reads the destination manifest digest
/// back; the public subject remains this digest-only reference.
///
/// # Errors
/// Returns [`Exit::EnvelopeInvalid`] for an invalid source identity, unit id or
/// digest.
pub fn ghcr_unit_uri(repository: &str, unit: &str, digest: &str) -> Result<String> {
    validate_public_identity(repository, &"a".repeat(40), "1", 1, unit, digest)?;
    Ok(format!(
        "oci://{}@{digest}",
        ghcr_unit_repository(repository, unit)?
    ))
}

/// Closed GHCR repository for one public OCI unit, without a mutable tag.
///
/// # Errors
/// Returns [`Exit::EnvelopeInvalid`] for an invalid repository or unit id.
pub fn ghcr_unit_repository(repository: &str, unit: &str) -> Result<String> {
    let valid_segment = |segment: &str| {
        !segment.is_empty()
            && segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    };
    let repository_valid = repository
        .split_once('/')
        .is_some_and(|(owner, repo)| valid_segment(owner) && valid_segment(repo));
    if !repository_valid || !safe_unit_id(unit) {
        return Err(ToolError::single(
            Exit::EnvelopeInvalid,
            "publication-oci-repository",
            "OCI publication requires exact `owner/repository` and a safe unit id",
        ));
    }
    let Some((owner, repo)) = repository.split_once('/') else {
        return Err(ToolError::single(
            Exit::EnvelopeInvalid,
            "publication-oci-repository",
            "OCI publication requires exact `owner/repository` and a safe unit id",
        ));
    };
    Ok(format!(
        "ghcr.io/{}/{}-units/{unit}",
        owner.to_ascii_lowercase(),
        repo.to_ascii_lowercase()
    ))
}

/// Content-addressed release asset basename for one non-OCI unit.
///
/// # Errors
/// Returns [`Exit::EnvelopeInvalid`] for an unsupported form or malformed
/// digest/unit id.
pub fn unit_asset_name(unit: &str, digest: &str, form: &str) -> Result<String> {
    if !safe_unit_id(unit) {
        return Err(ToolError::single(
            Exit::EnvelopeInvalid,
            "publication-unit-id",
            format!("`{unit}` is not a safe release asset unit id"),
        ));
    }
    let bare = digest.strip_prefix("sha256:").ok_or_else(|| {
        ToolError::single(
            Exit::EnvelopeInvalid,
            "publication-digest",
            format!("`{digest}` is not a SHA-256 digest"),
        )
    })?;
    if bare.len() != 64
        || !bare
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(ToolError::single(
            Exit::EnvelopeInvalid,
            "publication-digest",
            format!("`{digest}` is not a lowercase SHA-256 digest"),
        ));
    }
    let extension = match form {
        "zip" => "zip",
        "tar.gz" | "build-output" => "tar.gz",
        other => {
            return Err(ToolError::single(
                Exit::EnvelopeInvalid,
                "publication-form",
                format!("artifact form `{other}` is not a GitHub Release blob"),
            ));
        }
    };
    Ok(format!("unit-{unit}-{bare}.{extension}"))
}

/// Public blob form implied by a deployable kind in a composition manifest.
///
/// Manifests deliberately do not repeat the envelope's media block; this
/// mapping is therefore the one way to cross-check a manifest's release asset
/// basename without trusting a filename supplied by the manifest itself.
#[must_use]
pub fn blob_form_for_kind(kind: &str) -> Option<&'static str> {
    match kind {
        "rust-lambda" | "ts-lambda" | "microvm-image" => Some("zip"),
        "rust-binary" | "build-output" => Some("tar.gz"),
        _ => None,
    }
}

/// Exact release URL for one content-addressed auxiliary unit document.
///
/// # Errors
/// Returns [`Exit::EnvelopeInvalid`] for an unsafe class/extension or malformed
/// immutable identity.
pub fn github_release_aux_uri(
    repository: &str,
    commit_sha: &str,
    run_id: &str,
    run_attempt: u64,
    unit: &str,
    digest: &str,
    asset: AuxiliaryAsset<'_>,
) -> Result<String> {
    validate_public_identity(repository, commit_sha, run_id, run_attempt, unit, digest)?;
    let AuxiliaryAsset { class, extension } = asset;
    if !matches!(class, "sbom" | "provenance" | "signature" | "receipt")
        || !matches!(extension, "json" | "jsonl" | "cdx.json" | "sigstore.json")
    {
        return Err(ToolError::single(
            Exit::EnvelopeInvalid,
            "publication-auxiliary-name",
            "auxiliary release assets use a closed class and extension vocabulary",
        ));
    }
    let bare = digest.strip_prefix("sha256:").ok_or_else(|| {
        ToolError::single(
            Exit::EnvelopeInvalid,
            "publication-digest",
            "auxiliary assets require a SHA-256 digest",
        )
    })?;
    Ok(format!(
        "https://github.com/{repository}/releases/download/main-{commit_sha}-run-{run_id}-attempt-{run_attempt}/{class}-{unit}-{bare}.{extension}"
    ))
}

fn validate_public_identity(
    repository: &str,
    commit_sha: &str,
    run_id: &str,
    run_attempt: u64,
    unit: &str,
    digest: &str,
) -> Result<()> {
    let valid_segment = |segment: &str| {
        !segment.is_empty()
            && segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    };
    let repository_valid = repository
        .split_once('/')
        .is_some_and(|(owner, repo)| valid_segment(owner) && valid_segment(repo));
    let sha_valid = commit_sha.len() == 40
        && commit_sha
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
    let run_valid = !run_id.is_empty()
        && !run_id.starts_with('0')
        && run_id.bytes().all(|byte| byte.is_ascii_digit());
    if !repository_valid
        || !sha_valid
        || !run_valid
        || run_attempt == 0
        || !safe_unit_id(unit)
        || unit_asset_name(unit, digest, "zip").is_err()
    {
        return Err(ToolError::single(
            Exit::EnvelopeInvalid,
            "publication-identity",
            "repository, commit, run, attempt, unit and digest must be exact immutable identities",
        ));
    }
    Ok(())
}

fn safe_unit_id(unit: &str) -> bool {
    (3..=64).contains(&unit.len())
        && unit.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && unit
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// Package every Git-tracked Terraform module source into a deterministic
/// archive rooted at `modules/`.
///
/// # Errors
/// Returns [`Exit::Usage`] if Git cannot enumerate the committed module
/// closure or a member is missing, unsafe, link-shaped, or not source material.
pub fn package_module_bundle(root: &Path) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z", "--cached", "--", "infra/modules"])
        .output()
        .map_err(|err| {
            ToolError::single(
                Exit::Usage,
                "module-bundle-git",
                format!("cannot enumerate committed Terraform modules: {err}"),
            )
        })?;
    if !output.status.success() {
        return Err(ToolError::single(
            Exit::Usage,
            "module-bundle-git",
            format!(
                "git ls-files failed while enumerating Terraform modules: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    let paths = String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    package_module_bundle_from_paths(root, &paths)
}

/// Package an already-authoritative list of repository-relative module paths.
///
/// This lower-level entry point exists so the membership rules and byte
/// determinism can be tested without making a fixture impersonate a Git work
/// tree. Production callers use [`package_module_bundle`].
///
/// # Errors
/// Returns [`Exit::Usage`] for an empty, unsafe, non-source, missing, or linked
/// member.
pub fn package_module_bundle_from_paths(root: &Path, paths: &[String]) -> Result<Vec<u8>> {
    if paths.is_empty() {
        return Err(ToolError::single(
            Exit::Usage,
            "module-bundle-empty",
            "the committed Terraform module closure is empty",
        ));
    }

    let mut entries = Vec::with_capacity(paths.len());
    let mut violations = Vec::new();
    for path in paths {
        let normalized = path.replace('\\', "/");
        let Some(member) = normalized.strip_prefix("infra/") else {
            violations.push(Violation::new(
                "module-bundle-member-outside-root",
                format!("`{path}` is outside `infra/modules/`"),
            ));
            continue;
        };
        if !member.starts_with("modules/")
            || normalized.starts_with('/')
            || normalized
                .split('/')
                .any(|part| matches!(part, "" | "." | ".."))
        {
            violations.push(Violation::new(
                "module-bundle-member-outside-root",
                format!("`{path}` is not a safe member below `infra/modules/`"),
            ));
            continue;
        }
        if member.split('/').any(|segment| segment == ".terraform")
            || !permitted_module_source(member)
        {
            violations.push(Violation::new(
                "module-bundle-member-forbidden",
                format!("`{path}` is not Terraform module source material"),
            ));
            continue;
        }

        let source = root.join(Path::new(&normalized));
        let metadata = match std::fs::symlink_metadata(&source) {
            Ok(metadata) => metadata,
            Err(err) => {
                violations.push(Violation::new(
                    "module-bundle-member-missing",
                    format!("`{path}` cannot be read: {err}"),
                ));
                continue;
            }
        };
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            violations.push(Violation::new(
                "module-bundle-member-not-regular",
                format!("`{path}` is not a regular file"),
            ));
            continue;
        }
        match std::fs::read(&source) {
            Ok(bytes) => entries.push(Entry::regular(member, bytes)),
            Err(err) => violations.push(Violation::new(
                "module-bundle-member-missing",
                format!("`{path}` cannot be read: {err}"),
            )),
        }
    }
    if !violations.is_empty() {
        return Err(ToolError::many(Exit::Usage, violations));
    }
    write_tar_gz(&entries, SOURCE_DATE_EPOCH_DEFAULT)
}

/// Read and validate the checked-in generated regional table definition
/// bundle without changing its bytes.
///
/// The JSON file is a separate release identity from the Terraform module
/// archive. Terraform roots decode these exact bytes and pass the definitions
/// into the module; the module never reads a public checkout path.
///
/// # Errors
/// Returns [`Exit::Usage`] when the generated member is missing, link-shaped,
/// malformed, empty, or carries an invalid schema/digest identity.
pub fn regional_tables_bundle(root: &Path) -> Result<(Vec<u8>, RegionalTablesIdentity)> {
    let path = root.join("migrations/regional/generated/regional-tables.json");
    let metadata = std::fs::symlink_metadata(&path).map_err(|err| {
        ToolError::single(
            Exit::Usage,
            "regional-tables-missing",
            format!("`{}` cannot be read: {err}", path.display()),
        )
    })?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(ToolError::single(
            Exit::Usage,
            "regional-tables-not-regular",
            format!("`{}` is not a regular file", path.display()),
        ));
    }
    let bytes = std::fs::read(&path).map_err(|err| {
        ToolError::single(
            Exit::Usage,
            "regional-tables-missing",
            format!("`{}` cannot be read: {err}", path.display()),
        )
    })?;
    let document: RegionalTablesDocument = serde_json::from_slice(&bytes).map_err(|err| {
        ToolError::single(
            Exit::Usage,
            "regional-tables-json",
            format!(
                "`{}` is not the closed generated bundle: {err}",
                path.display()
            ),
        )
    })?;
    if document.schema != "aex.regional-tables.v1"
        || document.generation == 0
        || document.tables.is_empty()
        || !valid_blake3(&document.digest)
    {
        return Err(ToolError::single(
            Exit::Usage,
            "regional-tables-identity",
            "the generated regional bundle requires schema `aex.regional-tables.v1`, a positive generation, a non-empty table array and one lowercase BLAKE3 digest",
        ));
    }
    let identity = RegionalTablesIdentity {
        digest: canon::digest_bytes(&bytes),
        size_bytes: bytes.len() as u64,
        definitions_digest: document.digest,
        generation: document.generation,
    };
    Ok((bytes, identity))
}

/// Verify the exact transported bytes and the generated definition identity.
///
/// # Errors
/// Returns [`Exit::ArtifactMismatch`] for byte mismatch and
/// [`Exit::ManifestInvalid`] when the JSON's BLAKE3 identity differs from the
/// composition.
pub fn verify_regional_tables_bundle(
    path: &Path,
    digest: &str,
    size_bytes: u64,
    definitions_digest: &str,
    generation: u32,
) -> Result<()> {
    let bytes = std::fs::read(path).map_err(|err| {
        ToolError::single(
            Exit::ArtifactMismatch,
            "regional-tables-missing",
            format!("`{}` cannot be read: {err}", path.display()),
        )
    })?;
    let actual_digest = canon::digest_bytes(&bytes);
    let actual_size = bytes.len() as u64;
    let mut byte_violations = Vec::new();
    if actual_digest != digest {
        byte_violations.push(Violation::new(
            "published-blob-digest-mismatch",
            format!("recorded `{digest}`, downloaded `{actual_digest}`"),
        ));
    }
    if actual_size != size_bytes {
        byte_violations.push(Violation::new(
            "published-blob-size-mismatch",
            format!("recorded `{size_bytes}` bytes, downloaded `{actual_size}` bytes"),
        ));
    }
    if !byte_violations.is_empty() {
        return Err(ToolError::many(Exit::ArtifactMismatch, byte_violations));
    }
    let document: RegionalTablesDocument = serde_json::from_slice(&bytes).map_err(|err| {
        ToolError::single(
            Exit::ManifestInvalid,
            "regional-tables-json",
            format!(
                "`{}` is not the closed generated bundle: {err}",
                path.display()
            ),
        )
    })?;
    if document.schema != "aex.regional-tables.v1"
        || document.generation == 0
        || document.generation != generation
        || document.tables.is_empty()
        || !valid_blake3(&document.digest)
        || document.digest != definitions_digest
    {
        return Err(ToolError::single(
            Exit::ManifestInvalid,
            "regional-tables-identity",
            "the acquired regional table bundle is empty, malformed or not bound to the composition definitions digest",
        ));
    }
    Ok(())
}

fn valid_blake3(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("blake3:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn permitted_module_source(member: &str) -> bool {
    let name = member.rsplit('/').next().unwrap_or_default();
    name == "README.md"
        || name == "aex.toml"
        || name == ".terraform.lock.hcl"
        || name.as_bytes().ends_with(b".tf")
        || name.ends_with(".tftest.hcl")
}

/// Verify the exact digest and byte length of a downloaded public input.
///
/// # Errors
/// Returns [`Exit::ArtifactMismatch`] when the subject is missing or differs
/// from either recorded identity.
pub fn verify_blob(path: &Path, digest: &str, size_bytes: u64) -> Result<()> {
    let bytes = std::fs::read(path).map_err(|err| {
        ToolError::single(
            Exit::ArtifactMismatch,
            "published-blob-missing",
            format!("`{}` cannot be read: {err}", path.display()),
        )
    })?;
    let actual_digest = canon::digest_bytes(&bytes);
    let actual_size = bytes.len() as u64;
    let mut violations = Vec::new();
    if actual_digest != digest {
        violations.push(Violation::new(
            "published-blob-digest-mismatch",
            format!("recorded `{digest}`, downloaded `{actual_digest}`"),
        ));
    }
    if actual_size != size_bytes {
        violations.push(Violation::new(
            "published-blob-size-mismatch",
            format!("recorded `{size_bytes}` bytes, downloaded `{actual_size}` bytes"),
        ));
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(ToolError::many(Exit::ArtifactMismatch, violations))
    }
}
