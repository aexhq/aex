//! Fail-closed clean-source evidence for OCI planning and publication.

use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::error::{Exit, Result, ToolError};

/// Exact Git status query proven clean at a publication boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OciCleanSource {
    /// Schema discriminator.
    pub schema: String,
    /// True only after the exact status command returned no bytes.
    pub clean: bool,
    /// Stable porcelain format used for the check.
    pub status_format: String,
    /// Untracked-file handling used for the check.
    pub untracked_files: String,
}

/// Require a clean index, worktree, and complete untracked-file set.
///
/// The dirty path list is deliberately not emitted: only the fail-closed fact
/// belongs in publication evidence, and an untracked filename can itself be a
/// secret.
///
/// # Errors
/// Refuses a failed Git query or any staged, tracked, or untracked change.
pub fn verify_clean(root: &Path) -> Result<OciCleanSource> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain=v1", "--untracked-files=all"])
        .output()
        .map_err(|error| {
            ToolError::single(
                Exit::ArtifactMismatch,
                "oci-source-status",
                format!("could not run the closed Git status query: {error}"),
            )
        })?;
    if !output.status.success() {
        return Err(ToolError::single(
            Exit::ArtifactMismatch,
            "oci-source-status",
            "the closed Git status query failed",
        ));
    }
    if !output.stdout.is_empty() {
        return Err(ToolError::single(
            Exit::ArtifactMismatch,
            "oci-source-dirty",
            "tracked, staged, or untracked source changes exist; OCI identity minting is refused",
        ));
    }
    Ok(OciCleanSource {
        schema: "aex.oci-clean-source.v1".to_owned(),
        clean: true,
        status_format: "porcelain-v1".to_owned(),
        untracked_files: "all".to_owned(),
    })
}
