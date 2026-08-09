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
    #[serde(default)]
    pub sbom_format: String,
    /// Immutable SBOM URI.
    #[serde(default)]
    pub sbom_uri: String,
    /// Licence policy verdict. Inventory digest is recomputed.
    #[serde(default)]
    pub licenses: Licenses,
    /// Advisory scanner verdict.
    #[serde(default)]
    pub vulnerabilities: Vulnerabilities,
    /// Attestation identity. Bundle digest is recomputed.
    pub provenance: CertificationProvenance,
    /// Detached signature identity where policy requires one.
    pub signature: Signature,
}

/// Provenance identity claimed by the protected workflow.
///
/// The bundle digest is deliberately absent: [`certify`] computes it from the
/// exact attestation bundle supplied alongside these claims.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CertificationProvenance {
    /// Predicate type.
    pub predicate_type: String,
    /// Attestation URI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    /// Builder identity.
    pub builder_id: String,
    /// Whether the attestation verified.
    pub attested: bool,
}

/// Files whose bytes back certification claims.
#[derive(Debug, Clone, Copy)]
pub struct CertificationFiles<'a> {
    /// Packaged unit artifact. Required for blob kinds; omitted for OCI because
    /// its identity is the registry manifest digest.
    pub artifact: Option<&'a Path>,
    /// `CycloneDX` JSON document.
    pub sbom: Option<&'a Path>,
    /// Full licence inventory emitted by the passing licence scan.
    pub license_inventory: Option<&'a Path>,
    /// Full vulnerability verdict emitted by the passing artifact scan.
    pub vulnerability_verdict: Option<&'a Path>,
    /// Downloaded GitHub attestation bundle whose verification produced the
    /// provenance claim.
    pub provenance_bundle: &'a Path,
    /// Verified `cosign sign-blob` bundle for the downloadable Rust binary.
    pub signature_bundle: Option<&'a Path>,
    /// Startup-mode release: keep exact build/publication/provenance checks but
    /// defer scanners and their receipts outside the release critical path.
    pub defer_supply_chain: bool,
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

    let supply_files = if files.defer_supply_chain {
        None
    } else {
        Some((
            files.sbom.ok_or_else(|| {
                ToolError::single(
                    Exit::SupplyChainDenied,
                    "certify-sbom-missing",
                    "strict supply-chain certification requires an SBOM",
                )
            })?,
            files.license_inventory.ok_or_else(|| {
                ToolError::single(
                    Exit::SupplyChainDenied,
                    "certify-license-inventory-missing",
                    "strict supply-chain certification requires a licence inventory",
                )
            })?,
            files.vulnerability_verdict.ok_or_else(|| {
                ToolError::single(
                    Exit::SupplyChainDenied,
                    "certify-vulnerability-verdict-missing",
                    "strict supply-chain certification requires a vulnerability verdict",
                )
            })?,
        ))
    };

    let (component_count, sbom_digest) = if let Some((sbom, _, _)) = supply_files {
        let sbom_bytes = read(sbom, "certify-sbom-missing")?;
        let sbom_value: serde_json::Value = serde_json::from_slice(&sbom_bytes).map_err(|err| {
            ToolError::single(
                Exit::SupplyChainDenied,
                "certify-sbom-json",
                format!("`{}` is not JSON: {err}", sbom.display()),
            )
        })?;
        let is_named_component = |value: &serde_json::Value| {
            value
                .get("name")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|name| !name.is_empty())
        };
        let detected_component_count = sbom_value
            .get("components")
            .and_then(serde_json::Value::as_array)
            .map_or(0, |components| {
                components
                    .iter()
                    .filter(|component| is_named_component(component))
                    .count() as u64
            });
        let subject_component_count = u64::from(
            sbom_value
                .pointer("/metadata/component")
                .is_some_and(is_named_component),
        );
        let component_count = detected_component_count + subject_component_count;
        let cyclonedx_16 = sbom_value
            .get("bomFormat")
            .and_then(serde_json::Value::as_str)
            == Some("CycloneDX")
            && sbom_value
                .get("specVersion")
                .and_then(serde_json::Value::as_str)
                == Some("1.6");
        let sbom_subject_bound = sbom_value
            .pointer("/metadata/properties")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|properties| {
                properties.iter().any(|property| {
                    property.get("name").and_then(serde_json::Value::as_str)
                        == Some("aex:artifactSubjectDigest")
                        && property.get("value").and_then(serde_json::Value::as_str)
                            == Some(draft.artifact_subject_digest.as_str())
                })
            });
        if component_count == 0 || !cyclonedx_16 || !sbom_subject_bound {
            return Err(ToolError::single(
                Exit::SupplyChainDenied,
                "certify-sbom-empty",
                "the supplied SBOM must be non-empty CycloneDX 1.6 bound to the exact artifact subject",
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
        (component_count, Some(sbom_digest))
    } else {
        (0, None)
    };

    let (license_bytes, vulnerability_bytes) =
        if let Some((_, licenses, vulnerabilities)) = supply_files {
            (
                Some(read(licenses, "certify-license-inventory-missing")?),
                Some(read(
                    vulnerabilities,
                    "certify-vulnerability-verdict-missing",
                )?),
            )
        } else {
            (None, None)
        };
    if let (Some(license_bytes), Some(vulnerability_bytes)) =
        (license_bytes.as_deref(), vulnerability_bytes.as_deref())
    {
        validate_supply_documents(
            unit,
            &draft,
            &claims.licenses,
            &claims.vulnerabilities,
            license_bytes,
            vulnerability_bytes,
        )?;
    }
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
    let (receipt_refs, mut missing_receipts) = validate_available_receipts(
        &draft,
        unit,
        &claims.workflow,
        &artifact_subject_digest,
        receipts,
        freshness,
    )?;
    if files.defer_supply_chain {
        missing_receipts.retain(|class| {
            !matches!(
                class.as_str(),
                "deny" | "sbom" | "license" | "vulnerability"
            )
        });
    }
    if !missing_receipts.is_empty() {
        return Err(ToolError::many(
            Exit::EvidenceMissing,
            missing_receipts
                .into_iter()
                .map(|required| {
                    Violation::new(
                        "certify-receipt-missing",
                        format!("unit `{}` requires a passing `{required}` receipt", unit.id),
                    )
                })
                .collect(),
        ));
    }
    let mut licenses = claims.licenses;
    if let Some(license_bytes) = license_bytes.as_deref() {
        licenses.inventory_digest = Some(canon::digest_bytes(license_bytes));
    }
    let provenance = Provenance {
        predicate_type: claims.provenance.predicate_type,
        bundle_digest: canon::digest_bytes(&provenance_bytes),
        uri: claims.provenance.uri,
        builder_id: claims.provenance.builder_id,
        attested: claims.provenance.attested,
    };
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

    let signature = validate_signature(unit, claims.signature, files.signature_bundle)?;
    if !files.defer_supply_chain
        && (!valid_sha256(&licenses.policy_digest)
            || licenses.verdict != "allowed"
            || !licenses.denials.is_empty()
            || claims.vulnerabilities.scanner.is_empty()
            || claims.vulnerabilities.database.is_empty()
            || claims.vulnerabilities.scanned_at.is_empty()
            || claims.vulnerabilities.unapproved_critical != 0
            || claims.vulnerabilities.unapproved_high != 0)
    {
        return Err(ToolError::single(
            Exit::SupplyChainDenied,
            "certify-supply-chain-verdict",
            "licence and vulnerability claims must carry exact passing scanner identities",
        ));
    }
    if files.defer_supply_chain {
        draft.sbom = Sbom {
            format: "deferred-startup".to_owned(),
            digest: String::new(),
            uri: String::new(),
            component_count: 0,
        };
        draft.licenses = Licenses {
            policy_digest: String::new(),
            verdict: "deferred-startup".to_owned(),
            denials: Vec::new(),
            inventory_digest: None,
        };
        draft.vulnerabilities = Vulnerabilities {
            scanner: "deferred-startup".to_owned(),
            database: String::new(),
            scanned_at: String::new(),
            unapproved_critical: 0,
            unapproved_high: 0,
            approved_exceptions: Vec::new(),
        };
        draft.supply_chain_deferred = true;
    } else {
        let sbom_digest = sbom_digest.ok_or_else(|| {
            ToolError::single(
                Exit::SupplyChainDenied,
                "certify-sbom-missing",
                "strict supply-chain certification did not compute an SBOM digest",
            )
        })?;
        draft.sbom = Sbom {
            format: claims.sbom_format,
            digest: sbom_digest,
            uri: claims.sbom_uri,
            component_count,
        };
        draft.licenses = licenses;
        draft.vulnerabilities = claims.vulnerabilities;
        draft.supply_chain_deferred = false;
    }
    draft.provenance = provenance;
    draft.signature = signature;
    draft.receipts = receipt_refs;
    let certified = draft.seal()?;
    certified.verify(
        files.artifact,
        crate::artifact::kind_requires_signature(&unit.kind),
    )?;
    Ok(certified)
}

fn validate_supply_documents(
    unit: &Unit,
    draft: &ArtifactEnvelope,
    licenses: &Licenses,
    vulnerabilities: &Vulnerabilities,
    license_bytes: &[u8],
    vulnerability_bytes: &[u8],
) -> Result<()> {
    let license: serde_json::Value = serde_json::from_slice(license_bytes).map_err(|err| {
        ToolError::single(
            Exit::SupplyChainDenied,
            "certify-license-inventory-json",
            format!("the license inventory is not JSON: {err}"),
        )
    })?;
    let components = license
        .get("components")
        .and_then(serde_json::Value::as_array);
    let license_bound = license.get("schema").and_then(serde_json::Value::as_str)
        == Some("aex.license-inventory.v1")
        && license.get("unit").and_then(serde_json::Value::as_str) == Some(unit.id.as_str())
        && license
            .get("artifactSubjectDigest")
            .and_then(serde_json::Value::as_str)
            == Some(draft.artifact_subject_digest.as_str())
        && license
            .get("policyDigest")
            .and_then(serde_json::Value::as_str)
            == Some(licenses.policy_digest.as_str())
        && components.is_some_and(|rows| {
            !rows.is_empty()
                && rows.iter().all(|row| {
                    row.get("denied")
                        .and_then(serde_json::Value::as_array)
                        .is_some_and(Vec::is_empty)
                })
        });
    if !license_bound {
        return Err(ToolError::single(
            Exit::SupplyChainDenied,
            "certify-license-inventory-binding",
            "the complete allowed license inventory must bind the exact unit, artifact subject, and policy",
        ));
    }

    let verdict: serde_json::Value =
        serde_json::from_slice(vulnerability_bytes).map_err(|err| {
            ToolError::single(
                Exit::SupplyChainDenied,
                "certify-vulnerability-verdict-json",
                format!("the vulnerability verdict is not JSON: {err}"),
            )
        })?;
    let vulnerability_bound = verdict.get("schema").and_then(serde_json::Value::as_str)
        == Some("aex.vulnerability-verdict.v1")
        && verdict.get("unit").and_then(serde_json::Value::as_str) == Some(unit.id.as_str())
        && verdict
            .get("artifactSubjectDigest")
            .and_then(serde_json::Value::as_str)
            == Some(draft.artifact_subject_digest.as_str())
        && verdict.get("scanner").and_then(serde_json::Value::as_str)
            == Some(vulnerabilities.scanner.as_str())
        && verdict.get("database").and_then(serde_json::Value::as_str)
            == Some(vulnerabilities.database.as_str())
        && verdict.get("scannedAt").and_then(serde_json::Value::as_str)
            == Some(vulnerabilities.scanned_at.as_str())
        && verdict
            .get("unapprovedCritical")
            .and_then(serde_json::Value::as_u64)
            == Some(u64::from(vulnerabilities.unapproved_critical))
        && verdict
            .get("unapprovedHigh")
            .and_then(serde_json::Value::as_u64)
            == Some(u64::from(vulnerabilities.unapproved_high));
    if !vulnerability_bound {
        return Err(ToolError::single(
            Exit::SupplyChainDenied,
            "certify-vulnerability-verdict-binding",
            "the vulnerability verdict must bind the exact unit, artifact subject, scanner, database, scan time, and severity counts",
        ));
    }
    Ok(())
}

pub(crate) fn validate_draft(draft: &ArtifactEnvelope, unit: &Unit) -> Result<()> {
    let mut violations = Vec::new();
    let recomputed_subject = draft.compute_artifact_subject_digest()?;
    let resealed = draft.clone().seal()?;
    if draft.envelope_digest != resealed.envelope_digest {
        violations.push(Violation::new(
            "certify-draft-envelope-digest-mismatch",
            format!(
                "draft envelopeDigest `{}` does not match the canonical document `{}`",
                draft.envelope_digest, resealed.envelope_digest
            ),
        ));
    }
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

pub(crate) fn validate_workflow(draft: &ArtifactEnvelope, workflow: &Workflow) -> Result<()> {
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

pub(crate) fn validate_available_receipts(
    draft: &ArtifactEnvelope,
    unit: &Unit,
    workflow: &Workflow,
    artifact_subject_digest: &str,
    receipts: &[Receipt],
    freshness: &FreshnessPolicy,
) -> Result<(Vec<ReceiptRef>, Vec<String>)> {
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
    if !violations.is_empty() {
        return Err(ToolError::many(Exit::EvidenceMissing, violations));
    }
    let missing = unit
        .required_receipts
        .iter()
        .filter(|required| !by_class.contains_key(required.as_str()))
        .cloned()
        .collect();
    let refs = by_class
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
        .collect();
    Ok((refs, missing))
}

fn validate_signature(
    unit: &Unit,
    mut signature: Signature,
    signature_bundle: Option<&Path>,
) -> Result<Signature> {
    // The registry itself is the signer. Trusted publishing mints the signing
    // identity from the workflow's own OIDC token and npm attaches the
    // attestation to the version, so there is no detached bundle to read back
    // here — and accepting one would mean accepting a signature this repository
    // produced over bytes the registry never saw. `npm-publication.json` is
    // where that attestation is verified.
    let valid = if unit.kind == "npm-package" {
        if signature_bundle.is_some() {
            return Err(ToolError::single(
                Exit::ProvenanceMissing,
                "certify-signature-bundle",
                "registry provenance is attached by npm, not by a detached bundle",
            ));
        }
        signature.present
            && signature.kind == "npm-provenance"
            && signature.key_id.as_deref().is_some_and(|id| !id.is_empty())
            && signature.bundle_digest.is_none()
    } else if unit.kind == "rust-binary" {
        let bundle = signature_bundle.ok_or_else(|| {
            ToolError::single(
                Exit::ProvenanceMissing,
                "certify-signature-bundle",
                "a downloadable Rust binary requires its verified Sigstore bundle",
            )
        })?;
        let bytes = read(bundle, "certify-signature-bundle")?;
        let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|err| {
            ToolError::single(
                Exit::ProvenanceMissing,
                "certify-signature-bundle",
                format!(
                    "`{}` is not a Sigstore JSON bundle: {err}",
                    bundle.display()
                ),
            )
        })?;
        signature.bundle_digest = Some(canon::digest_bytes(&bytes));
        signature.present
            && signature.kind == "sigstore-cosign"
            && signature.key_id.as_deref().is_some_and(|id| !id.is_empty())
            && value.get("mediaType").and_then(serde_json::Value::as_str)
                == Some("application/vnd.dev.sigstore.bundle.v0.3+json")
            && signature.bundle_digest.as_deref().is_some_and(valid_sha256)
    } else {
        if signature_bundle.is_some() {
            return Err(ToolError::single(
                Exit::ProvenanceMissing,
                "certify-signature-bundle",
                format!(
                    "unit kind `{}` does not accept a detached signature",
                    unit.kind
                ),
            ));
        }
        !signature.present
            && signature.kind == "none"
            && signature.key_id.is_none()
            && signature.bundle_digest.is_none()
    };
    if valid {
        Ok(signature)
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
