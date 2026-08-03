//! Reproducible, single-platform OCI image production evidence.
//!
//! Image bytes contain only source-stable inputs. Workflow run and attempt
//! identities belong to the external publication record and GitHub
//! attestation; putting them in image labels would make an identical source
//! commit produce a different digest on retry.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::artifact::{BaseImage, BuildPlan, Location};
use crate::error::{Exit, Result, ToolError, io};
use crate::graph::inputs::Unit;

const SUPPORTED_TARGET: &str = "aarch64-unknown-linux-gnu.2.34";

/// Source-stable public identity embedded in an image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OciSource {
    /// `owner/repository`.
    pub repository: String,
    /// Exact 40-character Git commit.
    pub commit_sha: String,
}

/// External workflow identity. This is never embedded in image bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OciWorkflowRun {
    /// Workflow repository.
    pub repository: String,
    /// Protected source ref.
    pub r#ref: String,
    /// Reusable workflow path.
    pub path: String,
    /// Positive GitHub run id.
    pub run_id: String,
    /// Positive run attempt.
    pub run_attempt: u32,
    /// Matrix job name.
    pub job_name: String,
    /// GitHub builder identity.
    pub builder_id: String,
}

/// Deterministic inputs to the minimal image context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OciBuildBinding {
    /// Schema discriminator.
    pub schema: String,
    /// Unit id.
    pub unit: String,
    /// Unit kind.
    pub kind: String,
    /// Exact Cargo binary target.
    pub bin: String,
    /// Exact Rust target plus glibc floor.
    pub target: String,
    /// GHCR repository without a tag or digest.
    pub image_repository: String,
    /// Pinned runtime base.
    pub base_image: BaseImage,
    /// Digest of the copied ELF.
    pub binary_digest: String,
    /// Digest of the authoritative Cargo recipe.
    pub recipe_digest: String,
    /// Source identity.
    pub source: OciSource,
    /// Source-stable image labels.
    pub labels: BTreeMap<String, String>,
}

/// One content-addressed OCI descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OciDescriptorIdentity {
    /// Media type.
    pub media_type: String,
    /// SHA-256 content digest.
    pub digest: String,
    /// Exact byte length.
    pub size_bytes: u64,
}

/// Complete single-platform OCI identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OciImageIdentity {
    /// Schema discriminator.
    pub schema: String,
    /// Unit id.
    pub unit: String,
    /// Unit kind.
    pub kind: String,
    /// Binary target.
    pub bin: String,
    /// Rust target.
    pub target: String,
    /// GHCR repository without a tag or digest.
    pub image_repository: String,
    /// Source identity.
    pub source: OciSource,
    /// Runtime base.
    pub base_image: BaseImage,
    /// Cargo recipe digest.
    pub recipe_digest: String,
    /// Copied ELF digest.
    pub binary_digest: String,
    /// Root digest published to the registry.
    pub output_digest: String,
    /// Image manifest descriptor.
    pub manifest: OciDescriptorIdentity,
    /// Image config descriptor.
    pub config: OciDescriptorIdentity,
    /// Every compressed layer descriptor, in order.
    pub layers: Vec<OciDescriptorIdentity>,
}

/// Registry readback joined to the exact workflow invocation that published it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OciPublication {
    /// Schema discriminator.
    pub schema: String,
    /// Exact image identity read back.
    pub image: OciImageIdentity,
    /// Immutable digest-only public location.
    pub location: Location,
    /// Workflow run that published and read back the image.
    pub workflow: OciWorkflowRun,
}

/// Create a new minimal, deterministic image context.
///
/// The destination must not exist. This makes two reproducibility builds use
/// genuinely separate contexts instead of silently reusing files from the
/// first invocation.
///
/// # Errors
/// Refuses unsupported units, malformed source/base identities, a non-AArch64
/// ELF, a recipe mismatch, an existing destination, or I/O failure.
pub fn prepare_context(
    unit: &Unit,
    plan: &BuildPlan,
    binary: &Path,
    destination: &Path,
    source: OciSource,
) -> Result<OciBuildBinding> {
    validate_unit_and_plan(unit, plan)?;
    validate_source(&source)?;
    let bin = unit.bin.as_deref().ok_or_else(|| {
        ToolError::single(
            Exit::Usage,
            "oci-bin-missing",
            format!("OCI unit `{}` has no binary target", unit.id),
        )
    })?;
    let bytes = std::fs::read(binary).map_err(|error| io(&binary.display().to_string(), &error))?;
    validate_aarch64_elf(&bytes)?;
    let base_image = parse_base_image(unit.base_image.as_deref())?;
    let binary_digest = crate::canon::digest_bytes(&bytes);
    let image_repository = crate::publication::ghcr_unit_repository(&source.repository, &unit.id)?;
    let labels = stable_labels(unit, plan, &source, &base_image, &binary_digest);
    let binding = OciBuildBinding {
        schema: "aex.oci-build-binding.v1".to_owned(),
        unit: unit.id.clone(),
        kind: unit.kind.clone(),
        bin: bin.to_owned(),
        target: unit.target.clone(),
        image_repository,
        base_image,
        binary_digest,
        recipe_digest: plan.digest.clone(),
        source,
        labels,
    };

    std::fs::create_dir(destination)
        .map_err(|error| io(&destination.display().to_string(), &error))?;
    std::fs::write(destination.join("artifact"), bytes)
        .map_err(|error| io(&destination.join("artifact").display().to_string(), &error))?;
    let dockerfile = dockerfile(&binding);
    std::fs::write(destination.join("Dockerfile"), dockerfile).map_err(|error| {
        io(
            &destination.join("Dockerfile").display().to_string(),
            &error,
        )
    })?;
    Ok(binding)
}

fn validate_unit_and_plan(unit: &Unit, plan: &BuildPlan) -> Result<()> {
    if !matches!(unit.kind.as_str(), "rust-oci-service" | "rust-oci-task") {
        return Err(ToolError::single(
            Exit::Usage,
            "oci-unit-kind",
            format!(
                "unit `{}` is `{}`, not an OCI service/task",
                unit.id, unit.kind
            ),
        ));
    }
    if unit.target != SUPPORTED_TARGET || unit.form != "oci-image" {
        return Err(ToolError::single(
            Exit::Usage,
            "oci-unit-target",
            format!(
                "unit `{}` must use target `{SUPPORTED_TARGET}` and form `oci-image`",
                unit.id
            ),
        ));
    }
    let expected = crate::artifact::plan(unit)?;
    let digest = crate::artifact::build_plan_digest(plan)?;
    let base_environment_matches = expected
        .env
        .iter()
        .all(|(key, value)| plan.env.get(key) == Some(value));
    let environment_keys_are_closed = plan.env.keys().all(|key| {
        expected.env.contains_key(key)
            || (unit.id == "brain-mux"
                && matches!(
                    key.as_str(),
                    crate::artifact::MODEL_CATALOG_TRUST_ROOTS_JSON_VAR
                        | crate::artifact::MODEL_CATALOG_TRUST_ROOTS_SHA256_VAR
                        | crate::artifact::MODEL_CATALOG_COLLECTION_FILE_VAR
                        | crate::artifact::MODEL_CATALOG_COLLECTION_SHA256_VAR
                ))
    });
    let catalog_values = [
        crate::artifact::MODEL_CATALOG_TRUST_ROOTS_JSON_VAR,
        crate::artifact::MODEL_CATALOG_TRUST_ROOTS_SHA256_VAR,
        crate::artifact::MODEL_CATALOG_COLLECTION_FILE_VAR,
        crate::artifact::MODEL_CATALOG_COLLECTION_SHA256_VAR,
    ]
    .into_iter()
    .filter_map(|key| plan.env.get(key))
    .collect::<Vec<_>>();
    let catalog_binding_is_complete = catalog_values.is_empty()
        || (unit.id == "brain-mux"
            && catalog_values.len() == 4
            && catalog_values.iter().all(|value| !value.is_empty()));
    if plan.unit != unit.id
        || plan.kind != unit.kind
        || plan.target != unit.target
        || plan.profile != expected.profile
        || plan.argv != expected.argv
        || !base_environment_matches
        || !environment_keys_are_closed
        || !catalog_binding_is_complete
        || plan.form != "oci"
        || plan.input != expected.input
        || plan.entrypoint != expected.entrypoint
        || plan.base_image != unit.base_image
        || plan.digest != digest
    {
        return Err(ToolError::single(
            Exit::Usage,
            "oci-recipe-mismatch",
            format!(
                "the build plan is not the authoritative OCI recipe for `{}`",
                unit.id
            ),
        ));
    }
    Ok(())
}

fn validate_source(source: &OciSource) -> Result<()> {
    let repository = source
        .repository
        .split_once('/')
        .is_some_and(|(owner, repo)| {
            safe_component(owner) && safe_component(repo) && !repo.contains('/')
        });
    if !repository || !valid_hex(&source.commit_sha, 40) {
        return Err(ToolError::single(
            Exit::Usage,
            "oci-source-identity",
            "OCI source requires exact `owner/repository` and a lowercase 40-character commit",
        ));
    }
    Ok(())
}

fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn valid_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn valid_sha256(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|bare| valid_hex(bare, 64))
}

fn validate_aarch64_elf(bytes: &[u8]) -> Result<()> {
    let valid = bytes.len() >= 64
        && bytes.get(..4) == Some(b"\x7fELF")
        && bytes[4] == 2
        && bytes[5] == 1
        && bytes[6] == 1
        && matches!(u16::from_le_bytes([bytes[16], bytes[17]]), 2 | 3)
        && u16::from_le_bytes([bytes[18], bytes[19]]) == 183;
    if valid {
        Ok(())
    } else {
        Err(ToolError::single(
            Exit::ArtifactMismatch,
            "oci-binary-elf",
            "the OCI input must be a 64-bit little-endian AArch64 ELF",
        ))
    }
}

fn parse_base_image(reference: Option<&str>) -> Result<BaseImage> {
    let reference = reference.ok_or_else(|| {
        ToolError::single(
            Exit::Usage,
            "oci-base-missing",
            "OCI unit has no runtime base",
        )
    })?;
    let (_, digest) = reference.rsplit_once('@').ok_or_else(|| {
        ToolError::single(
            Exit::Usage,
            "oci-base-unpinned",
            format!("base image `{reference}` is not pinned by digest"),
        )
    })?;
    if !valid_sha256(digest) || reference.matches('@').count() != 1 {
        return Err(ToolError::single(
            Exit::Usage,
            "oci-base-unpinned",
            format!("base image `{reference}` is not pinned by one SHA-256 digest"),
        ));
    }
    Ok(BaseImage {
        r#ref: reference.to_owned(),
        digest: digest.to_owned(),
    })
}

fn stable_labels(
    unit: &Unit,
    plan: &BuildPlan,
    source: &OciSource,
    base: &BaseImage,
    binary_digest: &str,
) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("dev.aex.base-image-digest".to_owned(), base.digest.clone()),
        (
            "dev.aex.bin".to_owned(),
            unit.bin.clone().unwrap_or_default(),
        ),
        ("dev.aex.binary-digest".to_owned(), binary_digest.to_owned()),
        ("dev.aex.recipe-digest".to_owned(), plan.digest.clone()),
        ("dev.aex.target".to_owned(), unit.target.clone()),
        ("dev.aex.unit".to_owned(), unit.id.clone()),
        (
            "org.opencontainers.image.revision".to_owned(),
            source.commit_sha.clone(),
        ),
        (
            "org.opencontainers.image.source".to_owned(),
            format!("https://github.com/{}", source.repository),
        ),
        ("org.opencontainers.image.title".to_owned(), unit.id.clone()),
    ])
}

fn dockerfile(binding: &OciBuildBinding) -> String {
    let mut text = format!(
        "FROM {}\nCOPY --chmod=0555 artifact /usr/local/bin/{}\n",
        binding.base_image.r#ref, binding.bin
    );
    for (key, value) in &binding.labels {
        text.push_str("LABEL ");
        text.push_str(key);
        text.push_str("=\"");
        text.push_str(value);
        text.push_str("\"\n");
    }
    text.push_str("ENTRYPOINT [\"/usr/local/bin/");
    text.push_str(&binding.bin);
    text.push_str("\"]\nCMD []\n");
    text
}

mod layout;
mod readback;

pub use layout::{inspect_layout, manifest_blob_path, verify_reproducible};
pub use readback::verify_readback;

fn invalid(rule: &str, message: impl Into<String>) -> ToolError {
    ToolError::single(Exit::ArtifactMismatch, rule, message)
}
