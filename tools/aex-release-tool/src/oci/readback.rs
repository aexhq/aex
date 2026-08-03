//! Digest-only registry readback and workflow-attempt binding.

use std::path::Path;

use super::layout::{Manifest, identity, parse_json};
use super::{OciImageIdentity, OciPublication, OciWorkflowRun};
use crate::artifact::Location;
use crate::error::{Exit, Result, ToolError, Violation, io};

/// Verify the raw registry manifest, pulled config identity and extracted ELF,
/// then bind them to the exact publishing workflow attempt.
///
/// # Errors
/// Refuses any descriptor, config, ELF, source, workflow, or location mismatch.
pub fn verify_readback(
    expected: &OciImageIdentity,
    raw_manifest: &Path,
    pulled_config_digest: &str,
    pulled_binary: &Path,
    workflow: OciWorkflowRun,
) -> Result<OciPublication> {
    validate_workflow(expected, &workflow)?;
    let bytes = std::fs::read(raw_manifest)
        .map_err(|error| io(&raw_manifest.display().to_string(), &error))?;
    let manifest: Manifest = parse_json(&bytes, "oci-readback-manifest-json")?;
    let actual_manifest = crate::canon::digest_bytes(&bytes);
    let mut violations = Vec::new();
    if actual_manifest != expected.manifest.digest
        || bytes.len() as u64 != expected.manifest.size_bytes
    {
        violations.push(Violation::new(
            "oci-readback-manifest",
            "raw registry manifest digest/size differs from the reproducible build",
        ));
    }
    if identity(&manifest.config) != expected.config {
        violations.push(Violation::new(
            "oci-readback-config",
            "raw registry config descriptor differs from the reproducible build",
        ));
    }
    let layers: Vec<_> = manifest.layers.iter().map(identity).collect();
    if layers != expected.layers {
        violations.push(Violation::new(
            "oci-readback-layers",
            "raw registry layer descriptors differ from the reproducible build",
        ));
    }
    if pulled_config_digest != expected.config.digest {
        violations.push(Violation::new(
            "oci-readback-config",
            format!(
                "pulled config `{pulled_config_digest}` is not `{}`",
                expected.config.digest
            ),
        ));
    }
    let binary = std::fs::read(pulled_binary)
        .map_err(|error| io(&pulled_binary.display().to_string(), &error))?;
    if crate::canon::digest_bytes(&binary) != expected.binary_digest {
        violations.push(Violation::new(
            "oci-readback-binary",
            "ELF extracted from the pulled registry image differs from the build input",
        ));
    }
    if !violations.is_empty() {
        return Err(ToolError::many(Exit::ArtifactMismatch, violations));
    }
    let location = Location {
        kind: "oci".to_owned(),
        uri: crate::publication::ghcr_unit_uri(
            &expected.source.repository,
            &expected.unit,
            &expected.output_digest,
        )?,
        immutable: true,
        object_version_id: None,
    };
    Ok(OciPublication {
        schema: "aex.oci-publication.v1".to_owned(),
        image: expected.clone(),
        location,
        workflow,
    })
}

fn validate_workflow(expected: &OciImageIdentity, workflow: &OciWorkflowRun) -> Result<()> {
    let valid = workflow.repository == expected.source.repository
        && workflow.r#ref == "refs/heads/main"
        && workflow.path == ".github/workflows/_build-artifacts.yml"
        && workflow.job_name == expected.unit
        && !workflow.run_id.is_empty()
        && !workflow.run_id.starts_with('0')
        && workflow.run_id.bytes().all(|byte| byte.is_ascii_digit())
        && workflow.run_attempt > 0
        && workflow.builder_id
            == format!(
                "https://github.com/{}/.github/workflows/_build-artifacts.yml@refs/heads/main",
                expected.source.repository
            );
    if valid {
        Ok(())
    } else {
        Err(ToolError::single(
            Exit::ProvenanceMissing,
            "oci-readback-workflow",
            "readback must bind the exact protected-main reusable workflow, unit, run and attempt",
        ))
    }
}
