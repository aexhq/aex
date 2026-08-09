//! Exhaustive accounting for certified and deliberately deferred artifacts.
//!
//! A deferral is diagnostic evidence, never a weaker envelope. It binds the
//! exact draft artifact subject, same-run receipts and every field that remains
//! unearned. Composition still requires a certified envelope for every unit.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::artifact::{ArtifactEnvelope, ReceiptRef, UnitIdentity, Workflow};
use crate::describe::UnearnedField;
use crate::error::{Exit, Result, ToolError, Violation};
use crate::evidence::{FreshnessPolicy, Receipt};
use crate::graph::inputs::{Unit, Units};

/// Exact protected-run identity shared by every artifact disposition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedSource<'a> {
    /// GitHub `owner/repository`.
    pub repository: &'a str,
    /// Lowercase 40-character commit.
    pub commit_sha: &'a str,
    /// Positive Actions run id.
    pub run_id: &'a str,
    /// Positive Actions run attempt.
    pub run_attempt: u32,
}

/// A content-addressed statement that a draft cannot yet become release
/// evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CertificationDeferral {
    /// Schema discriminator.
    pub schema: String,
    /// Self-digest.
    pub deferral_digest: String,
    /// Exact registered unit identity.
    pub unit: UnitIdentity,
    /// Receipt-independent artifact subject.
    pub artifact_subject_digest: String,
    /// Self-digest of the local draft being withheld.
    pub draft_envelope_digest: String,
    /// Packaged byte or OCI-manifest digest.
    pub artifact_digest: String,
    /// Packaged byte or OCI-manifest length.
    pub artifact_size_bytes: u64,
    /// Exact source commit that produced the draft.
    pub source_commit_sha: String,
    /// Protected workflow run whose receipts were considered.
    pub workflow: Workflow,
    /// Passing, exact-run receipts available so far.
    pub available_receipts: Vec<ReceiptRef>,
    /// Required receipt classes that have no passing producer.
    pub missing_receipts: Vec<String>,
    /// Fields that were explicitly unearned when the draft was described.
    pub draft_unearned_fields: Vec<UnearnedField>,
}

impl CertificationDeferral {
    /// Canonically order and digest the deferral.
    ///
    /// # Errors
    /// Propagates canonical JSON errors.
    pub fn seal(mut self) -> Result<Self> {
        self.available_receipts
            .sort_by(|a, b| a.class.cmp(&b.class));
        self.missing_receipts.sort();
        self.draft_unearned_fields.sort();
        "sha256:0".clone_into(&mut self.deferral_digest);
        let value = serde_json::to_value(&self).map_err(|err| {
            ToolError::single(
                Exit::EvidenceMissing,
                "certification-deferral-unserializable",
                err.to_string(),
            )
        })?;
        self.deferral_digest =
            crate::canon::digest_document_excluding(&value, &["deferralDigest"])?;
        Ok(self)
    }

    /// Verify the deferral is self-consistent and really names blockers.
    ///
    /// # Errors
    /// Returns [`Exit::EvidenceUnsound`] for a malformed or tampered record.
    pub fn verify(&self) -> Result<()> {
        let mut violations = Vec::new();
        if self.schema != "aex.artifact-certification-deferral.v1" {
            violations.push(Violation::new(
                "certification-deferral-schema",
                format!("unknown deferral schema `{}`", self.schema),
            ));
        }
        let resealed = self.clone().seal()?;
        if self.deferral_digest != resealed.deferral_digest {
            violations.push(Violation::new(
                "certification-deferral-digest-mismatch",
                "deferralDigest does not match the canonical document",
            ));
        }
        if self.missing_receipts.is_empty() && self.draft_unearned_fields.is_empty() {
            violations.push(Violation::new(
                "certification-deferral-no-blocker",
                format!(
                    "unit `{}` is recorded as deferred without any missing evidence",
                    self.unit.id
                ),
            ));
        }
        let mut classes = BTreeSet::new();
        for receipt in &self.available_receipts {
            if receipt.conclusion != "passed" || !classes.insert(receipt.class.as_str()) {
                violations.push(Violation::new(
                    "certification-deferral-receipt",
                    format!(
                        "unit `{}` has a non-passing or duplicate `{}` receipt",
                        self.unit.id, receipt.class
                    ),
                ));
            }
            if receipt.source.repository != self.workflow.repository
                || receipt.source.commit_sha != self.source_commit_sha
                || receipt.source.workflow_run_id != self.workflow.run_id
                || receipt.source.run_attempt != self.workflow.run_attempt
            {
                violations.push(Violation::new(
                    "certification-deferral-receipt-source",
                    format!(
                        "unit `{}` carries a receipt from another workflow run",
                        self.unit.id
                    ),
                ));
            }
        }
        if self.artifact_size_bytes == 0
            || !valid_sha256(&self.artifact_subject_digest)
            || !valid_sha256(&self.draft_envelope_digest)
            || !valid_sha256(&self.artifact_digest)
            || !valid_sha1(&self.source_commit_sha)
        {
            violations.push(Violation::new(
                "certification-deferral-artifact-identity",
                format!("unit `{}` has an invalid artifact identity", self.unit.id),
            ));
        }
        let positive_run = !self.workflow.run_id.is_empty()
            && !self.workflow.run_id.starts_with('0')
            && self
                .workflow
                .run_id
                .bytes()
                .all(|byte| byte.is_ascii_digit());
        if self.workflow.r#ref != "refs/heads/main"
            || self.workflow.path != ".github/workflows/_build-artifacts.yml"
            || !positive_run
            || self.workflow.run_attempt == 0
            || self.workflow.builder_id.is_empty()
        {
            violations.push(Violation::new(
                "certification-deferral-workflow",
                format!(
                    "unit `{}` does not name the protected artifact workflow",
                    self.unit.id
                ),
            ));
        }
        if violations.is_empty() {
            Ok(())
        } else {
            Err(ToolError::many(Exit::EvidenceUnsound, violations))
        }
    }
}

/// Produce a deferral from the exact draft and receipts actually available.
///
/// # Errors
/// Refuses tampered drafts, cross-run receipts, duplicate classes and a
/// deferral with no real blocker.
pub fn defer(
    draft: &ArtifactEnvelope,
    unit: &Unit,
    workflow: Workflow,
    unearned_fields: Vec<UnearnedField>,
    receipts: &[Receipt],
    freshness: &FreshnessPolicy,
) -> Result<CertificationDeferral> {
    crate::certify::validate_draft(draft, unit)?;
    crate::certify::validate_workflow(draft, &workflow)?;
    let subject = draft.compute_artifact_subject_digest()?;
    let (available_receipts, missing_receipts) = crate::certify::validate_available_receipts(
        draft, unit, &workflow, &subject, receipts, freshness,
    )?;
    CertificationDeferral {
        schema: "aex.artifact-certification-deferral.v1".to_owned(),
        deferral_digest: "sha256:0".to_owned(),
        unit: draft.unit.clone(),
        artifact_subject_digest: subject,
        draft_envelope_digest: draft.envelope_digest.clone(),
        artifact_digest: draft.output.digest.clone(),
        artifact_size_bytes: draft.output.size_bytes,
        source_commit_sha: draft.source.commit_sha.clone(),
        workflow,
        available_receipts,
        missing_receipts,
        draft_unearned_fields: unearned_fields,
    }
    .seal()
    .and_then(|deferral| {
        deferral.verify()?;
        Ok(deferral)
    })
}

/// Exhaustive inventory emitted before composition is attempted.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CertificationInventory {
    /// Schema discriminator.
    pub schema: String,
    /// Certified unit ids.
    pub certified: Vec<String>,
    /// Deferred unit ids and their exact missing receipt classes.
    pub deferred: BTreeMap<String, Vec<String>>,
}

impl CertificationInventory {
    /// Turn any explicit deferral into the release-blocking error it represents.
    #[must_use]
    pub fn blocking_error(&self) -> Option<ToolError> {
        (!self.deferred.is_empty()).then(|| {
            ToolError::many(
                Exit::EvidenceMissing,
                self.deferred
                    .iter()
                    .map(|(unit, missing)| {
                        let classes = if missing.is_empty() {
                            "non-receipt certification inputs".to_owned()
                        } else {
                            missing.join(", ")
                        };
                        Violation::new(
                            "artifact-certification-deferred",
                            format!("unit `{unit}` remains deferred: {classes}"),
                        )
                    })
                    .collect(),
            )
        })
    }
}

/// Verify that every registered deployable has exactly one disposition.
///
/// Certified envelopes are verified as release evidence. Deferrals are only
/// accounted for; callers must still fail on [`CertificationInventory::blocking_error`].
///
/// # Errors
/// Refuses drafts masquerading as envelopes, duplicate/conflicting records,
/// cross-run evidence and any registry hole.
pub fn inventory(
    registry: &Units,
    envelopes: &[ArtifactEnvelope],
    deferrals: &[CertificationDeferral],
    expected: &ExpectedSource<'_>,
) -> Result<CertificationInventory> {
    let registered: BTreeMap<&str, &Unit> = registry
        .units
        .iter()
        .map(|unit| (unit.id.as_str(), unit))
        .collect();
    let mut dispositions = BTreeSet::new();
    let mut certified = Vec::new();
    let mut deferred = BTreeMap::new();
    let mut violations = Vec::new();

    for envelope in envelopes {
        let id = envelope.unit.id.as_str();
        let Some(unit) = registered.get(id) else {
            violations.push(Violation::new(
                "certification-inventory-unregistered",
                format!("certified envelope `{id}` is not registered"),
            ));
            continue;
        };
        if !dispositions.insert(id.to_owned()) {
            violations.push(conflict(id));
            continue;
        }
        if let Err(err) = verify_envelope(envelope, unit, expected) {
            violations.extend(err.violations);
        }
        certified.push(id.to_owned());
    }
    for deferral in deferrals {
        let id = deferral.unit.id.as_str();
        let Some(unit) = registered.get(id) else {
            violations.push(Violation::new(
                "certification-inventory-unregistered",
                format!("certification deferral `{id}` is not registered"),
            ));
            continue;
        };
        if !dispositions.insert(id.to_owned()) {
            violations.push(conflict(id));
            continue;
        }
        if let Err(err) = deferral.verify() {
            violations.extend(err.violations);
        }
        if deferral.unit.kind != unit.kind
            || deferral.unit.plane != unit.plane
            || deferral.source_commit_sha != expected.commit_sha
            || !source_matches(&deferral.workflow, expected)
        {
            violations.push(Violation::new(
                "certification-deferral-registry-source",
                format!("deferral `{id}` does not match its registry row and exact source run"),
            ));
        }
        let available: BTreeSet<&str> = deferral
            .available_receipts
            .iter()
            .map(|receipt| receipt.class.as_str())
            .collect();
        let mut expected_missing: Vec<String> = unit
            .required_receipts
            .iter()
            .filter(|class| !available.contains(class.as_str()))
            .cloned()
            .collect();
        expected_missing.sort();
        if deferral.missing_receipts != expected_missing {
            violations.push(Violation::new(
                "certification-deferral-missing-receipts",
                format!("deferral `{id}` does not name every missing required receipt class"),
            ));
        }
        deferred.insert(id.to_owned(), deferral.missing_receipts.clone());
    }
    for id in registered.keys() {
        if !dispositions.contains(*id) {
            violations.push(Violation::new(
                "certification-inventory-hole",
                format!(
                    "registered deployable `{id}` has neither a certified envelope nor a deferral"
                ),
            ));
        }
    }
    if !violations.is_empty() {
        violations.sort();
        violations.dedup();
        return Err(ToolError::many(Exit::CompositionIncompatible, violations));
    }
    certified.sort();
    Ok(CertificationInventory {
        schema: "aex.artifact-certification-inventory.v1".to_owned(),
        certified,
        deferred,
    })
}

fn verify_envelope(
    envelope: &ArtifactEnvelope,
    unit: &Unit,
    expected: &ExpectedSource<'_>,
) -> Result<()> {
    envelope.verify(None, crate::artifact::kind_requires_signature(&unit.kind))?;
    let required_central_head = envelope
        .identities
        .migration
        .as_ref()
        .and_then(|migration| migration.required_central_head.as_ref());
    let carried: BTreeSet<&str> = envelope
        .receipts
        .iter()
        .map(|receipt| receipt.class.as_str())
        .collect();
    let missing: Vec<&str> = unit
        .required_receipts
        .iter()
        .filter(|class| !carried.contains(class.as_str()))
        .map(String::as_str)
        .collect();
    if envelope.unit.kind != unit.kind
        || envelope.unit.plane != unit.plane
        || envelope.media.form != unit.form
        || envelope.output.target.triple != unit.target
        || envelope.identities.config_schema_version != unit.config_schema_version
        || envelope.identities.config_env_namespace.as_deref()
            != unit.config_env_namespace.as_deref()
        || required_central_head != unit.required_central_head.as_ref()
        || !source_matches(&envelope.source.workflow, expected)
        || envelope.source.commit_sha != expected.commit_sha
        || !missing.is_empty()
    {
        return Err(ToolError::single(
            Exit::EvidenceMissing,
            "certification-envelope-registry-source",
            format!(
                "certified envelope `{}` does not match its registry row, exact source run or required receipts",
                envelope.unit.id
            ),
        ));
    }
    Ok(())
}

fn source_matches(workflow: &Workflow, expected: &ExpectedSource<'_>) -> bool {
    workflow.repository == expected.repository
        && workflow.run_id == expected.run_id
        && workflow.run_attempt == expected.run_attempt
}

fn conflict(id: &str) -> Violation {
    Violation::new(
        "certification-inventory-conflict",
        format!("deployable `{id}` has more than one certification disposition"),
    )
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn valid_sha1(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}
