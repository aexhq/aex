//! Deterministic, plane-neutral public release inputs.
//!
//! The hosted release engine consumes a raw Linux release-tool executable and
//! a Terraform module bundle. Both are public source artifacts: they are built
//! without a plane binding, cloud credentials, or private repository content.

use std::path::Path;
use std::process::Command;

use crate::canon;
use crate::error::{Exit, Result, ToolError, Violation};
use crate::pack::{Entry, SOURCE_DATE_EPOCH_DEFAULT, write_tar_gz};

/// Fixed public asset name for the raw Linux `x86_64` release tool.
pub const RELEASE_TOOL_ASSET: &str = "aex-release-tool";
/// Fixed public asset name for the Terraform module source bundle.
pub const MODULE_BUNDLE_ASSET: &str = "terraform-modules.tar.gz";

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
