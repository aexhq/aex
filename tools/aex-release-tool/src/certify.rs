//! CI-only conversion of an honest local draft into a published envelope.
//!
//! [`crate::describe`] deliberately leaves workflow, location, supply-chain
//! and provenance fields unearned. This module is the only bridge that fills
//! them. File-backed identities are recomputed, receipt self-digests and exact
//! run bindings are verified, and workflow/scanner verdicts remain explicit
//! claims for the protected workflow to obtain from their named producers.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::artifact::{
    ArtifactEnvelope, Licenses, Location, Provenance, ReceiptRef, Sbom, Signature, Vulnerabilities,
    Workflow,
};
use crate::canon;
use crate::error::{Exit, Result, ToolError, Violation};
use crate::evidence::{FreshnessPolicy, Receipt};
use crate::graph::inputs::Unit;

/// Claims whose truth comes from a CI action or scanner rather than the source
/// tree. Digests and counts backed by local files are recomputed by
/// [`certify`]; they are intentionally absent from this input.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CertificationClaims {
    /// GitHub workflow identity that produced the bytes.
    pub workflow: Workflow,
    /// Immutable public location of the artifact bytes/OCI manifest.
    pub location: Location,
    /// SBOM format and immutable URI. Digest/count are recomputed.
    pub sbom_format: String,
    /// Immutable SBOM URI.
    pub sbom_uri: String,
    /// Licence policy verdict. Inventory digest is recomputed.
    pub licenses: Licenses,
    /// Advisory scanner verdict.
    pub vulnerabilities: Vulnerabilities,
    /// Attestation identity. Bundle digest is recomputed.
    pub provenance: Provenance,
    /// Detached signature identity where policy requires one.
    pub signature: Signature,
}

/// Files whose bytes back certification claims.
#[derive(Debug, Clone, Copy)]
pub struct CertificationFiles<'a> {
    /// Packaged unit artifact. Required for blob kinds; omitted for OCI because
    /// its identity is the registry manifest digest.
    pub artifact: Option<&'a Path>,
    /// `CycloneDX` JSON document.
    pub sbom: &'a Path,
    /// Full licence inventory emitted by the passing licence scan.
    pub license_inventory: &'a Path,
    /// Downloaded GitHub attestation bundle whose verification produced the
    /// provenance claim.
    pub provenance_bundle: &'a Path,
}

/// Fill an unearned local draft from immutable CI evidence and verify the
/// resulting envelope before returning it.
///
/// # Errors
/// Returns a classified refusal when the draft/unit disagree, a backing file
/// is missing/tampered, a receipt is unsound or misbound, a required class is
/// absent, or the resulting envelope violates publication policy.
#[allow(clippy::too_many_lines)]
pub fn certify(
    mut draft: ArtifactEnvelope,
    unit: &Unit,
    claims: CertificationClaims,
    files: CertificationFiles<'_>,
    receipts: &[Receipt],
    freshness: &FreshnessPolicy,
) -> Result<ArtifactEnvelope> {
    validate_draft(&draft, unit)?;
    validate_workflow(&draft, &claims.workflow)?;

    if unit.kind.starts_with("rust-oci-") {
        if files.artifact.is_some() {
            return Err(ToolError::single(
                Exit::ArtifactMismatch,
                "certify-oci-file",
                "an OCI envelope is verified against its registry manifest digest, not a tar file",
            ));
        }
    } else {
        let artifact = files.artifact.ok_or_else(|| {
            ToolError::single(
                Exit::ArtifactMismatch,
                "certify-artifact-missing",
                format!("unit `{}` has no packaged artifact file", unit.id),
            )
        })?;
        crate::publication::verify_blob(artifact, &draft.output.digest, draft.output.size_bytes)?;
    }

    let sbom_bytes = read(files.sbom, "certify-sbom-missing")?;
    let sbom_value: serde_json::Value = serde_json::from_slice(&sbom_bytes).map_err(|err| {
        ToolError::single(
            Exit::SupplyChainDenied,
            "certify-sbom-json",
            format!("`{}` is not JSON: {err}", files.sbom.display()),
        )
    })?;
    let component_count = sbom_value
        .get("components")
        .and_then(serde_json::Value::as_array)
        .map_or(0, |components| components.len() as u64);
    let cyclonedx_16 = sbom_value
        .get("bomFormat")
        .and_then(serde_json::Value::as_str)
        == Some("CycloneDX")
        && sbom_value
            .get("specVersion")
            .and_then(serde_json::Value::as_str)
            == Some("1.6");
    if component_count == 0 || !cyclonedx_16 {
        return Err(ToolError::single(
            Exit::SupplyChainDenied,
            "certify-sbom-empty",
            "the supplied SBOM must be a CycloneDX 1.6 document with at least one component",
        ));
    }
    let sbom_digest = canon::digest_bytes(&sbom_bytes);
    let expected_sbom_uri = crate::publication::github_release_aux_uri(
        &draft.source.repository,
        &draft.source.commit_sha,
        &claims.workflow.run_id,
        u64::from(claims.workflow.run_attempt),
        &unit.id,
        &sbom_digest,
        crate::publication::AuxiliaryAsset {
            class: "sbom",
            extension: "cdx.json",
        },
    )?;
    if claims.sbom_format != "cyclonedx-1.6" || claims.sbom_uri != expected_sbom_uri {
        return Err(ToolError::single(
            Exit::SupplyChainDenied,
            "certify-sbom-identity",
            format!("the SBOM must be CycloneDX 1.6 at `{expected_sbom_uri}`"),
        ));
    }

    let license_bytes = read(files.license_inventory, "certify-license-inventory-missing")?;
    let provenance_bytes = read(files.provenance_bundle, "certify-provenance-bundle-missing")?;

    draft.source.workflow = claims.workflow.clone();
    draft.output.location = claims.location.clone();
    let artifact_subject_digest = draft.compute_artifact_subject_digest()?;
    if artifact_subject_digest != draft.artifact_subject_digest {
        return Err(ToolError::single(
            Exit::ArtifactMismatch,
            "certify-artifact-subject-drift",
            "applying certification claims changed the artifact subject identity",
        ));
    }
    let receipt_refs = validate_receipts(
        &draft,
        unit,
        &claims.workflow,
        &artifact_subject_digest,
        receipts,
        freshness,
    )?;
    let mut licenses = claims.licenses;
    licenses.inventory_digest = Some(canon::digest_bytes(&license_bytes));
    let mut provenance = claims.provenance;
    provenance.bundle_digest = canon::digest_bytes(&provenance_bytes);
    if provenance.uri.as_deref().is_none_or(str::is_empty) {
        return Err(ToolError::single(
            Exit::ProvenanceMissing,
            "certify-provenance-uri",
            "the official attestation bundle needs an immutable public URI",
        ));
    }
    let expected_attestation_prefix = format!(
        "https://github.com/{}/attestations/",
        draft.source.repository
    );
    if provenance.predicate_type != "https://slsa.dev/provenance/v1"
        || !provenance.attested
        || !provenance
            .uri
            .as_deref()
            .is_some_and(|uri| uri.starts_with(&expected_attestation_prefix))
    {
        return Err(ToolError::single(
            Exit::ProvenanceMissing,
            "certify-provenance-identity",
            "provenance must be an attested SLSA v1 statement under this repository's GitHub attestation authority",
        ));
    }
    if provenance.builder_id != claims.workflow.builder_id {
        return Err(ToolError::single(
            Exit::ProvenanceMissing,
            "certify-builder-mismatch",
            "workflow and provenance builder identities differ",
        ));
    }

    validate_signature(unit, &claims.signature)?;
    if !valid_sha256(&licenses.policy_digest)
        || licenses.verdict != "allowed"
        || !licenses.denials.is_empty()
        || claims.vulnerabilities.scanner.is_empty()
        || claims.vulnerabilities.database.is_empty()
        || claims.vulnerabilities.scanned_at.is_empty()
        || claims.vulnerabilities.unapproved_critical != 0
        || claims.vulnerabilities.unapproved_high != 0
    {
        return Err(ToolError::single(
            Exit::SupplyChainDenied,
            "certify-supply-chain-verdict",
            "licence and vulnerability claims must carry exact passing scanner identities",
        ));
    }
    draft.sbom = Sbom {
        format: claims.sbom_format,
        digest: sbom_digest,
        uri: claims.sbom_uri,
        component_count,
    };
    draft.licenses = licenses;
    draft.vulnerabilities = claims.vulnerabilities;
    draft.provenance = provenance;
    draft.signature = claims.signature;
    draft.receipts = receipt_refs;
    let certified = draft.seal()?;
    certified.verify(files.artifact, unit.kind == "rust-binary")?;
    Ok(certified)
}

fn validate_draft(draft: &ArtifactEnvelope, unit: &Unit) -> Result<()> {
    let mut violations = Vec::new();
    let recomputed_subject = draft.compute_artifact_subject_digest()?;
    if draft.artifact_subject_digest != recomputed_subject {
        violations.push(Violation::new(
            "certify-artifact-subject-mismatch",
            format!(
                "draft artifactSubjectDigest `{}` does not match the canonical artifact subject `{recomputed_subject}`",
                draft.artifact_subject_digest
            ),
        ));
    }
    if draft.unit.id != unit.id
        || draft.unit.kind != unit.kind
        || draft.unit.plane != unit.plane
        || draft.identities.config_schema_version != unit.config_schema_version
    {
        violations.push(Violation::new(
            "certify-unit-mismatch",
            format!(
                "draft does not describe registry unit `{}` exactly",
                unit.id
            ),
        ));
    }
    if !draft.source.tree_clean || draft.source.r#ref.as_deref() != Some("refs/heads/main") {
        violations.push(Violation::new(
            "certify-source-ref",
            "certification accepts only a clean protected-main source checkout",
        ));
    }
    if draft.output.location.kind != "local"
        || draft.output.location.immutable
        || draft.provenance.attested
    {
        violations.push(Violation::new(
            "certify-draft-not-unearned",
            "certification accepts only the explicit local/unattested draft emitted by `artifact describe`",
        ));
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(ToolError::many(Exit::EnvelopeInvalid, violations))
    }
}

fn validate_workflow(draft: &ArtifactEnvelope, workflow: &Workflow) -> Result<()> {
    let positive_run = !workflow.run_id.is_empty()
        && !workflow.run_id.starts_with('0')
        && workflow.run_id.bytes().all(|byte| byte.is_ascii_digit());
    if workflow.repository != draft.source.repository
        || workflow.r#ref != "refs/heads/main"
        || workflow.path != ".github/workflows/_build-artifacts.yml"
        || !positive_run
        || workflow.run_attempt == 0
        || workflow.job_name.is_empty()
        || workflow.builder_id.is_empty()
    {
        return Err(ToolError::single(
            Exit::ProvenanceMissing,
            "certify-workflow-identity",
            "workflow must be the exact protected-main artifact builder with positive run identity",
        ));
    }
    Ok(())
}

fn validate_receipts(
    draft: &ArtifactEnvelope,
    unit: &Unit,
    workflow: &Workflow,
    artifact_subject_digest: &str,
    receipts: &[Receipt],
    freshness: &FreshnessPolicy,
) -> Result<Vec<ReceiptRef>> {
    let mut by_class = BTreeMap::new();
    let mut violations = Vec::new();
    for receipt in receipts {
        if let Err(err) = receipt.verify() {
            violations.extend(err.violations);
            continue;
        }
        if receipt.source.repository != draft.source.repository
            || receipt.source.commit_sha != draft.source.commit_sha
            || receipt.source.workflow_run_id != workflow.run_id
            || receipt.source.run_attempt != workflow.run_attempt
            || !receipt.subject.unit_ids.iter().any(|id| id == &unit.id)
        {
            violations.push(Violation::new(
                "certify-receipt-binding",
                format!(
                    "receipt `{}` is not bound to unit `{}` and this exact source run",
                    receipt.receipt_id, unit.id
                ),
            ));
            continue;
        }
        let Some(freshness_rule) = freshness.class.get(&receipt.class) else {
            violations.push(Violation::new(
                "certify-receipt-freshness-class",
                format!(
                    "receipt `{}` has class `{}` with no freshness policy",
                    receipt.receipt_id, receipt.class
                ),
            ));
            continue;
        };
        match freshness_rule.bound_to.as_str() {
            "artifact" => {
                if receipt.subject.artifact_subject_digest.as_deref()
                    != Some(artifact_subject_digest)
                {
                    violations.push(Violation::new(
                        "certify-receipt-artifact-subject",
                        format!(
                            "receipt `{}` is not bound to artifact subject `{artifact_subject_digest}`",
                            receipt.receipt_id
                        ),
                    ));
                    continue;
                }
            }
            "commit" | "release" | "none" => {}
            binding => {
                violations.push(Violation::new(
                    "certify-receipt-freshness-binding",
                    format!(
                        "receipt class `{}` has unknown freshness binding `{binding}`",
                        receipt.class
                    ),
                ));
                continue;
            }
        }
        if by_class.insert(receipt.class.clone(), receipt).is_some() {
            violations.push(Violation::new(
                "certify-receipt-duplicate",
                format!("unit `{}` has two `{}` receipts", unit.id, receipt.class),
            ));
        }
    }
    for required in &unit.required_receipts {
        if !by_class.contains_key(required) {
            violations.push(Violation::new(
                "certify-receipt-missing",
                format!("unit `{}` requires a passing `{required}` receipt", unit.id),
            ));
        }
    }
    if !violations.is_empty() {
        return Err(ToolError::many(Exit::EvidenceMissing, violations));
    }
    Ok(by_class
        .into_values()
        .map(|receipt| ReceiptRef {
            class: receipt.class.clone(),
            receipt_digest: receipt.receipt_digest.clone(),
            source: crate::artifact::ReceiptSource {
                repository: receipt.source.repository.clone(),
                commit_sha: receipt.source.commit_sha.clone(),
                workflow_run_id: receipt.source.workflow_run_id.clone(),
                run_attempt: receipt.source.run_attempt,
            },
            conclusion: receipt.conclusion.clone(),
        })
        .collect())
}

fn validate_signature(unit: &Unit, signature: &Signature) -> Result<()> {
    let valid = if unit.kind == "rust-binary" {
        signature.present
            && signature.kind == "sigstore-cosign"
            && signature.key_id.as_deref().is_some_and(|id| !id.is_empty())
            && signature.bundle_digest.as_deref().is_some_and(valid_sha256)
    } else {
        !signature.present
            && signature.kind == "none"
            && signature.key_id.is_none()
            && signature.bundle_digest.is_none()
    };
    if valid {
        Ok(())
    } else {
        Err(ToolError::single(
            Exit::ProvenanceMissing,
            "certify-signature-policy",
            format!(
                "unit kind `{}` does not satisfy its signature policy",
                unit.kind
            ),
        ))
    }
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn read(path: &Path, rule: &'static str) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|err| {
        ToolError::single(
            Exit::SupplyChainDenied,
            rule,
            format!("cannot read `{}`: {err}", path.display()),
        )
    })
}
