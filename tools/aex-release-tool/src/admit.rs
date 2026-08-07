//! Promotion admission.
//!
//! `admit` runs unprivileged, holds no cloud credential and builds nothing. It
//! reads digests, provenance, receipts, freshness and compatibility, and says
//! yes or no. Each numbered rule in the promotion contract carries its own exit
//! code, so an operator reading a refusal knows which class of thing is wrong
//! without opening a log.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::artifact::ArtifactEnvelope;
use crate::error::{Exit, Result, ToolError, Violation};
use crate::evidence::{FreshnessPolicy, Receipt, check_freshness};
use crate::manifest::CompositionManifest;
use crate::verification::VerificationStatement;

/// Everything `admit` is given. Nothing is fetched: the caller supplies the
/// evidence, and admission decides.
#[derive(Debug)]
pub struct AdmissionInputs<'a> {
    /// The candidate composition.
    pub manifest: &'a CompositionManifest,
    /// Envelope per unit id.
    pub envelopes: &'a BTreeMap<String, ArtifactEnvelope>,
    /// Receipts collected for this candidate.
    pub receipts: &'a [Receipt],
    /// The dev rehearsal statement, where one exists.
    pub statement: Option<&'a VerificationStatement>,
    /// Freshness policy.
    pub freshness: &'a FreshnessPolicy,
    /// Which plane is being admitted to.
    pub plane: Plane,
    /// The environment binding's operational readiness.
    pub readiness: OperationalReadiness,
    /// Receipt classes each unit kind must carry.
    pub required_receipts: &'a BTreeMap<String, Vec<String>>,
    /// Workflow identities permitted to have built an artifact.
    pub builder_allowlist: &'a [String],
    /// The applied central schema head recorded in the ledger.
    pub applied_central_head: Option<String>,
    /// The applied regional generation recorded in the ledger.
    pub applied_regional_generation: Option<u32>,
    /// Evidence the workspace has already recorded as impossible to earn.
    /// A missing receipt that appears here is an owned gap; one that does not
    /// is a hole nobody noticed. Admission refuses either way and says which.
    pub unearned: &'a crate::test_registry::UnearnedIndex,
    /// Evaluation time.
    pub now: time::OffsetDateTime,
}

/// Which plane is being admitted to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[clap(rename_all = "kebab-case")]
pub enum Plane {
    /// The development plane.
    Dev,
    /// Production.
    Prd,
}

/// Operational preconditions that are not about the artifact at all.
// Four independent yes/no preconditions. They are not a state machine, and
// collapsing them into one enum would lose which one failed.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationalReadiness {
    /// Whether a restore-readiness probe passed recently.
    pub backup_ready: bool,
    /// Whether every quota has headroom.
    pub quota_headroom: bool,
    /// Whether the current instant is inside a permitted maintenance window.
    pub within_maintenance_window: bool,
    /// Whether an unexpired approval exists.
    pub approval_valid: bool,
}

/// The admission verdict.
#[derive(Debug, Clone, Serialize)]
pub struct Admission {
    /// Schema discriminator.
    pub schema: &'static str,
    /// The release admitted.
    pub release_id: String,
    /// Which plane.
    pub plane: Plane,
    /// Units checked.
    pub units: Vec<String>,
    /// Verdict.
    pub admitted: bool,
}

/// Decide whether a candidate may be promoted.
///
/// # Errors
/// Returns the exit code of the first failing rule class, carrying every
/// violation in that class.
#[allow(clippy::too_many_lines)]
pub fn admit(inputs: &AdmissionInputs<'_>) -> Result<Admission> {
    let manifest = inputs.manifest;
    let mut units: Vec<String> = manifest.units.keys().cloned().collect();
    units.sort();

    // Rule 1: every envelope resolves, and its digest and size match the
    // manifest entry.
    let mut mismatches = Vec::new();
    for (id, entry) in &manifest.units {
        let Some(envelope) = inputs.envelopes.get(id) else {
            mismatches.push(Violation::new(
                "admit-envelope-missing",
                format!("unit `{id}` has no artifact envelope"),
            ));
            continue;
        };
        if envelope.output.digest != entry.artifact_digest {
            mismatches.push(Violation::new(
                "admit-digest-mismatch",
                format!(
                    "unit `{id}` envelope records `{}`; the manifest names `{}`",
                    envelope.output.digest, entry.artifact_digest
                ),
            ));
        }
        if envelope.output.size_bytes != entry.size_bytes {
            mismatches.push(Violation::new(
                "admit-size-mismatch",
                format!(
                    "unit `{id}` envelope records {} bytes; the manifest names {}",
                    envelope.output.size_bytes, entry.size_bytes
                ),
            ));
        }
        if !entry.location.immutable {
            mismatches.push(Violation::new(
                "admit-mutable-location",
                format!("unit `{id}` names a mutable destination"),
            ));
        }
    }
    if !mismatches.is_empty() {
        return Err(ToolError::many(Exit::ArtifactMismatch, mismatches));
    }

    // Rule 2 and 4: provenance and signature.
    let mut provenance = Vec::new();
    for (id, envelope) in inputs.envelopes {
        if !envelope.provenance.attested {
            provenance.push(Violation::new(
                "admit-provenance-unattested",
                format!("unit `{id}` carries no verified build provenance"),
            ));
        }
        if !inputs
            .builder_allowlist
            .iter()
            .any(|allowed| allowed == &envelope.provenance.builder_id)
        {
            provenance.push(Violation::new(
                "admit-builder-not-allowlisted",
                format!(
                    "unit `{id}` was built by `{}`, which is not an allowlisted builder",
                    envelope.provenance.builder_id
                ),
            ));
        }
        if envelope.source.r#ref.as_deref() != Some("refs/heads/main") {
            provenance.push(Violation::new(
                "admit-source-ref",
                format!(
                    "unit `{id}` was built from `{}`; only `refs/heads/main` publishes",
                    envelope.source.r#ref.as_deref().unwrap_or("(none)")
                ),
            ));
        }
    }
    if !provenance.is_empty() {
        return Err(ToolError::many(Exit::ProvenanceMissing, provenance));
    }

    // Rule 3: toolchain and policy digests agree with the manifest's policy
    // block.
    let mut policy = Vec::new();
    for (id, envelope) in inputs.envelopes {
        if envelope.inputs.toolchain.channel != manifest.policy.toolchain_channel {
            policy.push(Violation::new(
                "admit-toolchain-drift",
                format!(
                    "unit `{id}` was built with channel `{}`; the composition pins `{}`",
                    envelope.inputs.toolchain.channel, manifest.policy.toolchain_channel
                ),
            ));
        }
        if envelope.identities.contract_digest != manifest.contract_digest {
            policy.push(Violation::new(
                "admit-contract-drift",
                format!(
                    "unit `{id}` was built against contract `{}`; the composition pins `{}`",
                    envelope.identities.contract_digest, manifest.contract_digest
                ),
            ));
        }
    }
    if !policy.is_empty() {
        return Err(ToolError::many(Exit::ManifestInvalid, policy));
    }

    // Rule 6: zero unapproved supply-chain findings.
    let mut supply = Vec::new();
    for (id, envelope) in inputs.envelopes {
        if envelope.vulnerabilities.unapproved_critical > 0
            || envelope.vulnerabilities.unapproved_high > 0
        {
            supply.push(Violation::new(
                "admit-advisory-denied",
                format!("unit `{id}` carries unapproved critical or high advisories"),
            ));
        }
        if envelope.licenses.verdict != "allowed" {
            supply.push(Violation::new(
                "admit-license-denied",
                format!("unit `{id}` carries a licence denial"),
            ));
        }
    }
    if !supply.is_empty() {
        return Err(ToolError::many(Exit::SupplyChainDenied, supply));
    }

    // Rule 5: every required receipt class is present and passing.
    let by_class: BTreeMap<&str, Vec<&Receipt>> =
        inputs
            .receipts
            .iter()
            .fold(BTreeMap::new(), |mut map, receipt| {
                map.entry(receipt.class.as_str()).or_default().push(receipt);
                map
            });
    let mut missing = Vec::new();
    for (id, entry) in &manifest.units {
        let required = inputs
            .required_receipts
            .get(&entry.kind)
            .cloned()
            .unwrap_or_default();
        for class in required {
            let candidates = by_class.get(class.as_str());
            let satisfied = candidates.is_some_and(|receipts| {
                receipts.iter().any(|receipt| {
                    receipt.is_passing()
                        && (receipt.subject.unit_ids.is_empty()
                            || receipt.subject.unit_ids.iter().any(|unit| unit == id))
                })
            });
            if !satisfied {
                let detail = inputs.unearned.reason_for(id).map_or_else(
                    || format!("unit `{id}` requires a passing `{class}` receipt and has none"),
                    |row| {
                        format!(
                            "unit `{id}` requires a passing `{class}` receipt and has none;                              release/unearned-evidence.json records this as owed by the                              `{}` stream ({})",
                            row.owner, row.detail
                        )
                    },
                );
                missing.push(Violation::new("admit-receipt-missing", detail));
            }
        }
    }
    if !missing.is_empty() {
        return Err(ToolError::many(Exit::EvidenceMissing, missing));
    }

    // Rule 5 continued: every present receipt is sound and fresh.
    for receipt in inputs.receipts {
        receipt.verify()?;
        check_freshness(receipt, inputs.freshness, &manifest.release_id, inputs.now)?;
    }

    // Rule 7: production requires a passing complete-manifest dev rehearsal.
    if inputs.plane == Plane::Prd {
        let Some(statement) = inputs.statement else {
            return Err(ToolError::single(
                Exit::VerificationMissing,
                "admit-verification-missing",
                format!(
                    "release `{}` has no dev verification statement; production admits only \
                     a composition somebody rehearsed whole",
                    manifest.release_id
                ),
            ));
        };
        statement.verify(manifest)?;
        if statement.plane != "dev" {
            return Err(ToolError::single(
                Exit::VerificationMissing,
                "admit-verification-plane",
                format!(
                    "the verification statement was produced against `{}`, not dev",
                    statement.plane
                ),
            ));
        }
    }

    // Rule 8: schema head and regional generation.
    let mut composition = Vec::new();
    for (id, entry) in &manifest.units {
        if let (Some(required), Some(applied)) = (
            entry.required_central_head.as_deref(),
            inputs.applied_central_head.as_deref(),
        ) && required > applied
        {
            composition.push(Violation::new(
                "admit-head-mismatch",
                format!(
                    "unit `{id}` requires central head `{required}`; the ledger records \
                     `{applied}` applied"
                ),
            ));
        }
    }
    if let Some(applied) = inputs.applied_regional_generation
        && applied != manifest.migrations.regional.generation
    {
        composition.push(Violation::new(
            "admit-regional-generation-mismatch",
            format!(
                "the composition names regional generation {}; the ledger records {applied}",
                manifest.migrations.regional.generation
            ),
        ));
    }
    if !composition.is_empty() {
        return Err(ToolError::many(Exit::CompositionIncompatible, composition));
    }

    // Rule 9: operational preconditions.
    if inputs.plane == Plane::Prd {
        let mut operational = Vec::new();
        for (name, ready) in [
            ("backup readiness", inputs.readiness.backup_ready),
            ("quota headroom", inputs.readiness.quota_headroom),
            (
                "maintenance window",
                inputs.readiness.within_maintenance_window,
            ),
            ("approval", inputs.readiness.approval_valid),
        ] {
            if !ready {
                operational.push(Violation::new(
                    "admit-operational-precondition",
                    format!("{name} is not satisfied for a production promotion"),
                ));
            }
        }
        if !operational.is_empty() {
            return Err(ToolError::many(Exit::AdmissionDenied, operational));
        }
    }

    Ok(Admission {
        schema: "aex.admission.v1",
        release_id: manifest.release_id.clone(),
        plane: inputs.plane,
        units,
        admitted: true,
    })
}
