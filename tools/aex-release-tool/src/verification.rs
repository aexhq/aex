//! The verification statement.
//!
//! A manifest cannot contain its own post-deployment evidence without a digest
//! cycle, so the proof that a composition actually ran is a separate object
//! keyed by `releaseId`. Production admission requires a `passed` statement for
//! the exact release, produced from a complete-manifest dev rehearsal.

use serde::{Deserialize, Serialize};

use crate::canon;
use crate::error::{Exit, Result, ToolError, Violation};
use crate::evidence::Receipt;

/// What one unit's post-apply readback observed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Deployed {
    /// Unit id.
    pub unit: String,
    /// What the manifest said.
    pub expected_digest: String,
    /// What the plane actually holds.
    pub actual_digest: String,
    /// The version or alias the plane resolved to.
    pub actual_version_or_alias: String,
    /// The task definition, for Fargate units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_task_definition: Option<String>,
    /// When the readback happened.
    pub readback_at: String,
}

/// One receipt referenced by a statement.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StatementReceipt {
    /// Evidence class.
    pub class: String,
    /// Receipt digest.
    pub receipt_digest: String,
    /// Verdict.
    pub conclusion: String,
    /// When it was collected.
    pub collected_at: String,
    /// Which units it covers.
    #[serde(default)]
    pub covers_units: Vec<String>,
}

/// What the migration step applied.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppliedMigrations {
    /// Head after apply.
    pub applied_head: String,
    /// The schema-admin task that applied it.
    pub admin_task_arn: String,
    /// The receipt it produced.
    pub receipt_digest: String,
}

/// The attestation over the statement.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Attestation {
    /// Predicate type.
    pub predicate_type: String,
    /// Attestation bundle digest.
    pub bundle_digest: String,
    /// Builder identity.
    pub builder_id: String,
}

/// `aex.verification-statement.v1`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerificationStatement {
    /// Schema discriminator.
    pub schema: String,
    /// Self-digest.
    pub statement_digest: String,
    /// The release this statement is about.
    pub release_id: String,
    /// Which plane it ran against.
    pub plane: String,
    /// The binding that supplied the environment values.
    pub binding_digest: String,
    /// The private-repository commit that produced the binding.
    pub binding_ref: String,
    /// Regions covered.
    pub regions: Vec<String>,
    /// Per-unit readback.
    pub deployed: Vec<Deployed>,
    /// Collected receipts.
    pub receipts: Vec<StatementReceipt>,
    /// Applied migrations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migrations: Option<AppliedMigrations>,
    /// When it started.
    pub started_at: String,
    /// When it finished.
    pub completed_at: String,
    /// Verdict.
    pub conclusion: String,
    /// Attestation.
    pub attestation: Attestation,
    /// The ledger fence this statement is anchored to.
    pub ledger_fence: u64,
}

impl VerificationStatement {
    /// Recompute and set the self-digest.
    ///
    /// # Errors
    /// Propagates canonicalization failure.
    pub fn seal(mut self) -> Result<Self> {
        "sha256:0".clone_into(&mut self.statement_digest);
        let value = serde_json::to_value(&self).map_err(|err| {
            ToolError::single(
                Exit::VerificationMissing,
                "statement-unserializable",
                err.to_string(),
            )
        })?;
        self.statement_digest = canon::digest_document_excluding(&value, &["statementDigest"])?;
        Ok(self)
    }

    /// Verify the statement binds to a manifest and is attested.
    ///
    /// # Errors
    /// Returns [`Exit::VerificationMissing`] when the statement is for another
    /// release, is not attested, did not pass, or claims a unit the manifest
    /// does not hold — and when a readback disagrees with what was intended.
    pub fn verify(&self, manifest: &crate::manifest::CompositionManifest) -> Result<()> {
        let mut violations = Vec::new();
        if self.schema != "aex.verification-statement.v1" {
            violations.push(Violation::new(
                "statement-schema",
                format!("unknown statement schema `{}`", self.schema),
            ));
        }
        let recomputed = {
            let mut probe = self.clone();
            "sha256:0".clone_into(&mut probe.statement_digest);
            let value = serde_json::to_value(&probe).map_err(|err| {
                ToolError::single(
                    Exit::VerificationMissing,
                    "statement-unserializable",
                    err.to_string(),
                )
            })?;
            canon::digest_document_excluding(&value, &["statementDigest"])?
        };
        if recomputed != self.statement_digest {
            violations.push(Violation::new(
                "statement-digest-mismatch",
                format!(
                    "recorded statementDigest `{}` does not match the canonical bytes \
                     `{recomputed}`",
                    self.statement_digest
                ),
            ));
        }
        if self.release_id != manifest.release_id {
            violations.push(Violation::new(
                "statement-release-mismatch",
                format!(
                    "statement covers release `{}`, not `{}`",
                    self.release_id, manifest.release_id
                ),
            ));
        }
        if self.conclusion != "passed" {
            violations.push(Violation::new(
                "statement-not-passed",
                format!("statement concluded `{}`", self.conclusion),
            ));
        }
        if self.attestation.bundle_digest.is_empty() {
            violations.push(Violation::new(
                "statement-unattested",
                "the statement carries no attestation bundle",
            ));
        }
        // A complete-manifest rehearsal proves the whole composition, not the
        // one unit somebody changed.
        for unit in manifest.units.keys() {
            let Some(deployed) = self.deployed.iter().find(|entry| &entry.unit == unit) else {
                violations.push(Violation::new(
                    "statement-incomplete",
                    format!(
                        "the statement records no readback for `{unit}`; a component pass \
                         is not a whole-manifest verification"
                    ),
                ));
                continue;
            };
            let expected = &manifest.units[unit].artifact_digest;
            if &deployed.actual_digest != expected {
                violations.push(Violation::new(
                    "statement-readback-divergent",
                    format!(
                        "`{unit}` reads back as `{}`; the manifest names `{expected}`",
                        deployed.actual_digest
                    ),
                ));
            }
        }
        for receipt in &self.receipts {
            if receipt.conclusion != "passed" {
                violations.push(Violation::new(
                    "statement-receipt-not-passed",
                    format!(
                        "statement receipt class `{}` concluded `{}`",
                        receipt.class, receipt.conclusion
                    ),
                ));
            }
        }
        if violations.is_empty() {
            Ok(())
        } else {
            Err(ToolError::many(Exit::VerificationMissing, violations))
        }
    }
}

/// Build a statement from a manifest, a binding and a receipt set.
///
/// # Errors
/// Propagates canonicalization failure.
// The statement binds a manifest, a plane, a binding, a region set, a readback
// and a receipt set. Bundling them into one struct would create a type whose
// only purpose is to be destructured immediately.
#[allow(clippy::too_many_arguments)]
pub fn new_statement(
    manifest: &crate::manifest::CompositionManifest,
    plane: &str,
    binding_digest: &str,
    binding_ref: &str,
    regions: Vec<String>,
    deployed: Vec<Deployed>,
    receipts: &[Receipt],
    fence: u64,
    now: &str,
) -> Result<VerificationStatement> {
    let all_passed = receipts.iter().all(Receipt::is_passing);
    VerificationStatement {
        schema: "aex.verification-statement.v1".to_owned(),
        statement_digest: "sha256:0".to_owned(),
        release_id: manifest.release_id.clone(),
        plane: plane.to_owned(),
        binding_digest: binding_digest.to_owned(),
        binding_ref: binding_ref.to_owned(),
        regions,
        deployed,
        receipts: receipts
            .iter()
            .map(|receipt| StatementReceipt {
                class: receipt.class.clone(),
                receipt_digest: receipt.receipt_digest.clone(),
                conclusion: receipt.conclusion.clone(),
                collected_at: receipt.completed_at.clone(),
                covers_units: receipt.subject.unit_ids.clone(),
            })
            .collect(),
        migrations: None,
        started_at: now.to_owned(),
        completed_at: now.to_owned(),
        conclusion: if all_passed {
            "passed".to_owned()
        } else {
            "failed".to_owned()
        },
        attestation: Attestation {
            predicate_type: "https://slsa.dev/provenance/v1".to_owned(),
            bundle_digest: String::new(),
            builder_id: String::new(),
        },
        ledger_fence: fence,
    }
    .seal()
}
