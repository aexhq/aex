//! Registry publication evidence for one npm package unit.
//!
//! This is the npm counterpart of [`crate::oci`]'s readback: publication is not
//! believed because a `publish` command exited zero, it is believed because the
//! registry served back the same bytes under the same version with the
//! provenance attestation attached.
//!
//! Two identities are kept on purpose. `output.digest` in the envelope is the
//! SHA-256 this repository computed over the tarball it packed, which is the
//! same kind of locally earned identity every other unit carries. `integrity`
//! here is the registry's own SHA-512 subresource integrity string, which is
//! what an installer checks and what the composition manifest records. They are
//! different functions over the same bytes, so neither can stand in for the
//! other and both are verified against the tarball on disk.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::artifact::Location;
use crate::error::{Exit, Result, ToolError, Violation, io};
use crate::graph::inputs::Unit;

/// Schema discriminator for the publication evidence document.
pub const PUBLICATION_SCHEMA: &str = "aex.npm-publication.v1";

/// Registry readback joined to the exact workflow invocation that published it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NpmPublication {
    /// Schema discriminator.
    pub schema: String,
    /// Deployable registry unit id.
    pub unit: String,
    /// Published package name.
    pub package: String,
    /// Exact published version. Never a range.
    pub version: String,
    /// Registry SHA-512 subresource integrity over the served tarball.
    pub integrity: String,
    /// SHA-256 the release lane computed over the same bytes.
    pub tarball_digest: String,
    /// Exact tarball byte length.
    pub tarball_size_bytes: u64,
    /// Whether the registry reports an attached provenance attestation.
    pub provenance: bool,
    /// Immutable public tarball location.
    pub location: Location,
    /// Workflow run that published and read the version back.
    pub workflow: crate::oci::OciWorkflowRun,
}

/// What the publishing job observed, before any of it is believed.
#[derive(Debug, Clone, Copy)]
pub struct Readback<'a> {
    /// The exact tarball that was handed to the registry.
    pub tarball: &'a Path,
    /// `package.json` of the packed workspace member.
    pub manifest: &'a Path,
    /// Registry metadata for the published version, as served back.
    pub registry_metadata: &'a Path,
}

/// The subset of registry version metadata publication depends on.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegistryVersion {
    name: String,
    version: String,
    dist: RegistryDist,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegistryDist {
    integrity: String,
    tarball: String,
    #[serde(default)]
    attestations: Option<RegistryAttestations>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegistryAttestations {
    #[serde(default)]
    provenance: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct PackedManifest {
    name: String,
    version: String,
}

/// Verify that the registry serves the exact bytes this run packed, under the
/// version the source declares, with provenance attached.
///
/// # Errors
/// Returns [`Exit::ArtifactMismatch`] when the registry's name, version,
/// integrity or tarball location differs from the local bytes and source, and
/// [`Exit::ProvenanceMissing`] when the published version carries no provenance
/// attestation or the workflow binding is not the protected reusable lane.
pub fn verify_readback(
    unit: &Unit,
    files: Readback<'_>,
    workflow: crate::oci::OciWorkflowRun,
) -> Result<NpmPublication> {
    if unit.kind != "npm-package" {
        return Err(ToolError::single(
            Exit::Usage,
            "npm-readback-kind",
            format!(
                "unit `{}` has kind `{}` and publishes to no registry",
                unit.id, unit.kind
            ),
        ));
    }
    validate_workflow(unit, &workflow)?;

    let tarball = std::fs::read(files.tarball)
        .map_err(|error| io(&files.tarball.display().to_string(), &error))?;
    let manifest: PackedManifest = read_json(files.manifest, "npm-readback-manifest-json")?;
    let published: RegistryVersion =
        read_json(files.registry_metadata, "npm-readback-metadata-json")?;

    let integrity = crate::publication::npm_integrity(&tarball);
    let digest = crate::canon::digest_bytes(&tarball);
    let expected_uri = crate::publication::npm_tarball_uri(&unit.package, &manifest.version)?;

    let mut violations = Vec::new();
    if manifest.name != unit.package {
        violations.push(Violation::new(
            "npm-readback-package",
            format!(
                "packed manifest publishes `{}`, the registry row declares `{}`",
                manifest.name, unit.package
            ),
        ));
    }
    if published.name != unit.package || published.version != manifest.version {
        violations.push(Violation::new(
            "npm-readback-package",
            format!(
                "the registry served `{}@{}`, this run packed `{}@{}`",
                published.name, published.version, unit.package, manifest.version
            ),
        ));
    }
    if published.dist.integrity != integrity {
        violations.push(Violation::new(
            "npm-readback-integrity",
            "the registry's integrity value is not the integrity of the tarball this run packed",
        ));
    }
    if !crate::publication::valid_npm_integrity(&integrity) {
        violations.push(Violation::new(
            "npm-readback-integrity",
            format!("`{integrity}` is not a SHA-512 subresource integrity value"),
        ));
    }
    if published.dist.tarball != expected_uri {
        violations.push(Violation::new(
            "npm-readback-location",
            format!(
                "the registry serves `{}`, not the canonical immutable location `{expected_uri}`",
                published.dist.tarball
            ),
        ));
    }
    if !violations.is_empty() {
        return Err(ToolError::many(Exit::ArtifactMismatch, violations));
    }
    if published
        .dist
        .attestations
        .as_ref()
        .and_then(|attestations| attestations.provenance.as_ref())
        .is_none()
    {
        return Err(ToolError::single(
            Exit::ProvenanceMissing,
            "npm-readback-provenance",
            format!(
                "the registry reports no provenance attestation for `{}@{}`; trusted publishing is \
                 the only permitted publication path",
                unit.package, manifest.version
            ),
        ));
    }

    Ok(NpmPublication {
        schema: PUBLICATION_SCHEMA.to_owned(),
        unit: unit.id.clone(),
        package: unit.package.clone(),
        version: manifest.version,
        integrity,
        tarball_digest: digest,
        tarball_size_bytes: tarball.len() as u64,
        provenance: true,
        location: Location {
            kind: "npm".to_owned(),
            uri: expected_uri,
            immutable: true,
            object_version_id: None,
        },
        workflow,
    })
}

/// Verify one already-written publication document against itself.
///
/// The composition reads these documents long after the publishing job is gone,
/// so its fields are re-checked rather than trusted: an unearned or hand-edited
/// row would otherwise become a manifest identity nobody ever verified.
///
/// # Errors
/// Returns [`Exit::ManifestInvalid`] for a wrong schema, an inexact version, a
/// malformed integrity value, absent provenance, or a location that is not the
/// canonical tarball URI of the name and version it names.
pub fn validate_publication(publication: &NpmPublication) -> Result<()> {
    let mut violations = Vec::new();
    if publication.schema != PUBLICATION_SCHEMA {
        violations.push(Violation::new(
            "npm-publication-schema",
            format!("`{}` is not `{PUBLICATION_SCHEMA}`", publication.schema),
        ));
    }
    if !crate::publication::exact_npm_version(&publication.version) {
        violations.push(Violation::new(
            "npm-publication-version",
            format!(
                "`{}` is not an exact published npm version",
                publication.version
            ),
        ));
    }
    if !crate::publication::valid_npm_integrity(&publication.integrity) {
        violations.push(Violation::new(
            "npm-publication-integrity",
            format!(
                "`{}` is not a SHA-512 subresource integrity value",
                publication.integrity
            ),
        ));
    }
    if !publication.provenance {
        violations.push(Violation::new(
            "npm-publication-provenance",
            format!(
                "`{}` was published without a registry provenance attestation",
                publication.package
            ),
        ));
    }
    if !publication.location.immutable || publication.location.kind != "npm" {
        violations.push(Violation::new(
            "npm-publication-location",
            "a published version is an immutable registry location",
        ));
    }
    match crate::publication::npm_tarball_uri(&publication.package, &publication.version) {
        Ok(expected) if expected == publication.location.uri => {}
        Ok(expected) => violations.push(Violation::new(
            "npm-publication-location",
            format!(
                "`{}` is not the canonical tarball location `{expected}`",
                publication.location.uri
            ),
        )),
        Err(error) => violations.extend(error.violations),
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(ToolError::many(Exit::ManifestInvalid, violations))
    }
}

fn validate_workflow(unit: &Unit, workflow: &crate::oci::OciWorkflowRun) -> Result<()> {
    let valid = workflow.r#ref == "refs/heads/main"
        && workflow.path == ".github/workflows/_build-artifacts.yml"
        && workflow.job_name == unit.id
        && !workflow.run_id.is_empty()
        && !workflow.run_id.starts_with('0')
        && workflow.run_id.bytes().all(|byte| byte.is_ascii_digit())
        && workflow.run_attempt > 0
        && workflow.builder_id
            == format!(
                "https://github.com/{}/.github/workflows/_build-artifacts.yml@refs/heads/main",
                workflow.repository
            );
    if valid {
        Ok(())
    } else {
        Err(ToolError::single(
            Exit::ProvenanceMissing,
            "npm-readback-workflow",
            "publication must bind the exact protected-main reusable workflow, unit, run and attempt",
        ))
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path, rule: &'static str) -> Result<T> {
    let bytes = std::fs::read(path).map_err(|error| io(&path.display().to_string(), &error))?;
    serde_json::from_slice(&bytes).map_err(|error| {
        ToolError::single(
            Exit::ArtifactMismatch,
            rule,
            format!("`{}` does not parse: {error}", path.display()),
        )
    })
}
