//! Evidence receipts: the only thing a promotion decision reads.
//!
//! A receipt is content-addressed canonical JSON. `JUnit`, logs, traces and
//! profiles are hashed attachments, never the verdict — a verdict that lives in
//! a report format nobody digests is a verdict anybody can edit.
//!
//! The counters are the point. `skipped`, `ignored`, `filteredAtRuntime`,
//! `retried` and `flaky` are all required to be zero, and `collected` must
//! equal `declared`. A skipped test is a deleted test that still reports as
//! coverage; a flaky pass is a failure that happened to be scheduled well.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::canon;
use crate::error::{Exit, Result, ToolError, Violation};

/// Where a receipt came from.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Source {
    /// Repository.
    pub repository: String,
    /// Commit.
    pub commit_sha: String,
    /// Whether the tree was clean.
    pub tree_clean: bool,
    /// Workflow run id.
    pub workflow_run_id: String,
    /// Run attempt.
    pub run_attempt: u32,
    /// Job name.
    pub job_name: String,
    /// Builder identity.
    pub builder_id: String,
}

/// What the run was executed against.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Inputs {
    /// `Cargo.lock` digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cargo_lock_digest: Option<String>,
    /// `bun.lock` digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bun_lock_digest: Option<String>,
    /// Terraform lock digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terraform_lock_digest: Option<String>,
    /// Toolchain digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolchain_digest: Option<String>,
    /// Digest of the test sources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_source_digest: Option<String>,
    /// Digest of the nextest archive the partitions ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_archive_digest: Option<String>,
    /// Digest of the fixture corpus.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixture_corpus_digest: Option<String>,
}

/// What the receipt is about.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Subject {
    /// Receipt-independent artifact subject this receipt is bound to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_subject_digest: Option<String>,
    /// Release this receipt is bound to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_id: Option<String>,
    /// Units covered.
    #[serde(default)]
    pub unit_ids: Vec<String>,
}

/// One partition of a sharded run.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Partition {
    /// Zero-based index.
    pub index: usize,
    /// Total shards.
    pub total: usize,
}

/// How the run selected what it executed.
///
/// Selection is recorded so "not selected" is visibly different from
/// "filtered at runtime". The first is routing; the second is a skip.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectionBlock {
    /// Routing mode.
    pub mode: String,
    /// The nextest filterset used.
    pub filterset: String,
    /// Packages selected.
    pub packages: Vec<String>,
    /// Partition, where the run was sharded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partition: Option<Partition>,
    /// Digest of the recorded selection reasons.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasons_digest: Option<String>,
}

/// The counters `evidence verify` gates on.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Inventory {
    /// How many cases the runner listed.
    pub declared: u64,
    /// How many results were collected.
    pub collected: u64,
    /// How many passed.
    pub passed: u64,
    /// How many failed.
    pub failed: u64,
    /// Must be zero.
    pub skipped: u64,
    /// Must be zero.
    pub ignored: u64,
    /// Must be zero.
    pub filtered_at_runtime: u64,
    /// Must be zero.
    pub retried: u64,
    /// Must be zero.
    pub flaky: u64,
}

/// One failed case.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Failure {
    /// Case id.
    pub id: String,
    /// Failure message.
    pub message: String,
    /// Whether this was the first attempt.
    pub first_attempt: bool,
}

/// A hashed attachment.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Attachment {
    /// What kind of artefact.
    pub kind: String,
    /// Content digest.
    pub digest: String,
    /// Where it is stored.
    pub uri: String,
    /// Byte length.
    pub size_bytes: u64,
}

/// Test-data hygiene.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataBlock {
    /// Declared spend budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_micro_usd: Option<u64>,
    /// Observed spend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spent_micro_usd: Option<u64>,
    /// Digest of the cleanup ledger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_ledger_digest: Option<String>,
    /// `none`, `explained` or `unreclaimed`.
    ///
    /// `unreclaimed` is produced only by [`DataBlock::from_sweep`] and is a
    /// hard failure that no explanation softens. The other two are declarations
    /// a lane makes about itself; this one is a fact a janitor established.
    pub residue: String,
    /// Whether the run's secret canary was observed anywhere it should not be.
    pub secret_canary_observed: bool,
    /// Why residue is acceptable, when it is explained.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub residue_explanation: Option<String>,
}

impl DataBlock {
    /// Build the hygiene block from a real janitor sweep.
    ///
    /// This is the production constructor the receipt's `residue` field never
    /// had. Before it, `residue` was self-reported: the only thing that ever
    /// built a `DataBlock` was a unit-test fixture, so `evidence verify` gated
    /// on a value no lane produced and a leaking run reported `none` and
    /// passed.
    ///
    /// `residue` is now derived from what the sweep could not reclaim, and
    /// `residue_explanation` carries the sweep's own detail rather than a
    /// sentence a lane wrote about itself.
    #[must_use]
    pub fn from_sweep(
        report: &crate::janitor::SweepReport,
        cleanup_ledger_digest: Option<String>,
        budget_micro_usd: Option<u64>,
        spent_micro_usd: Option<u64>,
        secret_canary_observed: bool,
    ) -> Self {
        let verdict = report.residue();
        let (residue, residue_explanation) = match &verdict {
            crate::janitor::Residue::None => ("none".to_owned(), None),
            crate::janitor::Residue::Unreclaimed { count, detail } => (
                "unreclaimed".to_owned(),
                Some(format!(
                    "the janitor could not reclaim {count} resource(s) past their deadline: \
                     {detail}"
                )),
            ),
        };
        Self {
            budget_micro_usd,
            spent_micro_usd,
            cleanup_ledger_digest,
            residue,
            secret_canary_observed,
            residue_explanation,
        }
    }
}

/// `aex.evidence-receipt.v1`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Receipt {
    /// Schema discriminator.
    pub schema: String,
    /// Self-digest.
    pub receipt_digest: String,
    /// Receipt identity.
    pub receipt_id: String,
    /// Evidence class.
    pub class: String,
    /// Which of the five layers.
    pub layer: String,
    /// Which lane produced it.
    pub lane: String,
    /// What it concerns.
    #[serde(default)]
    pub concerns: Vec<String>,
    /// Where it came from.
    pub source: Source,
    /// What it ran against.
    #[serde(default)]
    pub inputs: Inputs,
    /// What it is about.
    #[serde(default)]
    pub subject: Subject,
    /// How it selected.
    pub selection: SelectionBlock,
    /// The counters.
    pub inventory: Inventory,
    /// Failures.
    #[serde(default)]
    pub failures: Vec<Failure>,
    /// Hashed attachments.
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    /// Data hygiene.
    pub data: DataBlock,
    /// When it started.
    pub started_at: String,
    /// When it finished.
    pub completed_at: String,
    /// The verdict.
    pub conclusion: String,
}

impl Receipt {
    /// Recompute and set the self-digest.
    ///
    /// # Errors
    /// Propagates canonicalization failure.
    pub fn seal(mut self) -> Result<Self> {
        "sha256:0".clone_into(&mut self.receipt_digest);
        let value = serde_json::to_value(&self).map_err(|err| {
            ToolError::single(
                Exit::EvidenceMissing,
                "receipt-unserializable",
                err.to_string(),
            )
        })?;
        self.receipt_digest = canon::digest_document_excluding(&value, &["receiptDigest"])?;
        Ok(self)
    }

    /// Whether this receipt can satisfy a promotion requirement.
    ///
    /// `infrastructure_failed` and `incomplete` exist to improve diagnosis, not
    /// to soften a verdict, so only `passed` qualifies.
    #[must_use]
    pub fn is_passing(&self) -> bool {
        self.conclusion == "passed"
    }

    /// Verify the receipt is sound.
    ///
    /// # Errors
    /// Returns [`Exit::EvidenceUnsound`] when any counter that must be zero is
    /// not, when the collected inventory does not match the declared one, when
    /// nothing was declared at all, when a rerun discarded its first failure,
    /// when residue is unexplained, or when a janitor sweep reported residue it
    /// could not reclaim.
    // One function on purpose: every counter, inventory and hygiene rule for
    // one receipt, in the order a reader would check them. Splitting it would
    // scatter the no-skip contract across five call sites.
    #[allow(clippy::too_many_lines)]
    pub fn verify(&self) -> Result<()> {
        let mut violations = Vec::new();
        if self.schema != "aex.evidence-receipt.v1" {
            violations.push(Violation::new(
                "receipt-schema",
                format!("unknown receipt schema `{}`", self.schema),
            ));
        }
        for (field, value) in [
            ("skipped", self.inventory.skipped),
            ("ignored", self.inventory.ignored),
            ("filteredAtRuntime", self.inventory.filtered_at_runtime),
            ("retried", self.inventory.retried),
            ("flaky", self.inventory.flaky),
        ] {
            if value != 0 {
                violations.push(Violation::new(
                    "flake-skipped-test",
                    format!(
                        "receipt `{}` reports {field} = {value}; a skipped, retried or flaky \
                         case is not evidence",
                        self.receipt_id
                    ),
                ));
            }
        }
        if self.inventory.declared == 0 {
            violations.push(Violation::new(
                "flake-empty-selection",
                format!(
                    "receipt `{}` declared 0 cases; an empty suite proves nothing",
                    self.receipt_id
                ),
            ));
        }
        if self.inventory.collected != self.inventory.declared {
            violations.push(Violation::new(
                "flake-inventory-mismatch",
                format!(
                    "receipt `{}` declared {} case(s) and collected {}",
                    self.receipt_id, self.inventory.declared, self.inventory.collected
                ),
            ));
        }
        if self.inventory.passed + self.inventory.failed != self.inventory.collected {
            violations.push(Violation::new(
                "flake-inventory-mismatch",
                format!(
                    "receipt `{}` collected {} case(s) but accounts for {} passed and {} failed",
                    self.receipt_id,
                    self.inventory.collected,
                    self.inventory.passed,
                    self.inventory.failed
                ),
            ));
        }
        if self.source.run_attempt > 1
            && !self
                .attachments
                .iter()
                .any(|attachment| attachment.kind == "first-failure")
        {
            violations.push(Violation::new(
                "flake-first-failure-lost",
                format!(
                    "receipt `{}` is attempt {} and carries no preserved first failure; the \
                     original verdict may not be discarded",
                    self.receipt_id, self.source.run_attempt
                ),
            ));
        }
        // `unreclaimed` is a janitor finding, not a lane's declaration about
        // itself, and no explanation makes it acceptable. OD-36 makes
        // reclamation a release gate; a lane that left something in production
        // past its deadline has not met it.
        if self.data.residue == "unreclaimed" {
            violations.push(Violation::new(
                "data-residue-unreclaimed",
                format!(
                    "receipt `{}` carries residue the janitor could not reclaim: {}",
                    self.receipt_id,
                    self.data
                        .residue_explanation
                        .as_deref()
                        .unwrap_or("(no detail recorded)")
                ),
            ));
        } else if self.data.residue != "none" && self.data.residue_explanation.is_none() {
            violations.push(Violation::new(
                "data-residue",
                format!(
                    "receipt `{}` reports residue `{}` with no explanation",
                    self.receipt_id, self.data.residue
                ),
            ));
        }
        if self.data.secret_canary_observed {
            violations.push(Violation::new(
                "data-secret-leak",
                format!(
                    "receipt `{}` observed its own secret canary outside the run",
                    self.receipt_id
                ),
            ));
        }
        if !self.source.tree_clean {
            violations.push(Violation::new(
                "receipt-dirty-tree",
                format!(
                    "receipt `{}` was produced from a dirty tree",
                    self.receipt_id
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

/// What one `JUnit` report says.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JunitSummary {
    /// Reported cases.
    pub cases: u64,
    /// Reported failures and errors.
    pub failures: u64,
    /// `<skipped/>` elements.
    pub skipped: u64,
    /// `<flakyFailure>` and `<rerunFailure>` elements.
    pub flaky: u64,
}

/// Parse a `JUnit` report far enough to fill the inventory.
///
/// Only element occurrence is read, not attribute totals: the `tests=` and
/// `skipped=` attributes are self-reported by the writer, and the whole point
/// of this counter is that a lane cannot self-report `skipped: 0`.
#[must_use]
pub fn parse_junit(xml: &str) -> JunitSummary {
    let count = |needle: &str| -> u64 {
        let mut total = 0u64;
        let mut rest = xml;
        while let Some(position) = rest.find(needle) {
            let after = &rest[position + needle.len()..];
            // Require a tag boundary so `<testcases>` never counts as
            // `<testcase`.
            if after
                .chars()
                .next()
                .is_some_and(|ch| ch.is_whitespace() || ch == '>' || ch == '/')
            {
                total += 1;
            }
            rest = after;
        }
        total
    };
    JunitSummary {
        cases: count("<testcase"),
        failures: count("<failure") + count("<error"),
        skipped: count("<skipped"),
        flaky: count("<flakyFailure") + count("<rerunFailure"),
    }
}

/// Everything about a run that its `JUnit` report does not say.
///
/// `declared` is here rather than derived from the report on purpose: a report
/// only describes cases that ran, so deriving the declared count from it would
/// make `collected == declared` true by construction and quietly retire the one
/// check that catches a run which lost cases between listing and executing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunContext {
    /// Receipt identity.
    pub receipt_id: String,
    /// Evidence class.
    pub class: String,
    /// Which of the five layers.
    pub layer: String,
    /// Which lane produced it.
    pub lane: String,
    /// What it concerns.
    #[serde(default)]
    pub concerns: Vec<String>,
    /// How many cases the runner listed before executing anything.
    pub declared: u64,
    /// Where the run came from.
    pub source: Source,
    /// What it ran against.
    #[serde(default)]
    pub inputs: Inputs,
    /// What it is about.
    #[serde(default)]
    pub subject: Subject,
    /// How it selected.
    pub selection: SelectionBlock,
    /// Data hygiene.
    pub data: DataBlock,
    /// When it started.
    pub started_at: String,
    /// When it finished.
    pub completed_at: String,
}

/// Build a receipt from a run's context and its `JUnit` report.
///
/// The counters come from element occurrence in the report, never from its
/// self-reported attributes, and the conclusion is derived from those counters
/// rather than supplied: a lane that could write its own verdict could write
/// `passed` over a skip.
///
/// # Errors
/// Propagates canonicalization failure from sealing.
pub fn new_receipt(context: RunContext, junit: &JunitSummary) -> Result<Receipt> {
    let inventory = Inventory {
        declared: context.declared,
        collected: junit.cases,
        passed: junit
            .cases
            .saturating_sub(junit.failures)
            .saturating_sub(junit.skipped),
        failed: junit.failures,
        skipped: junit.skipped,
        ignored: 0,
        filtered_at_runtime: 0,
        retried: 0,
        flaky: junit.flaky,
    };
    let passed = inventory.failed == 0
        && inventory.skipped == 0
        && inventory.flaky == 0
        && inventory.declared == inventory.collected
        && inventory.declared > 0;
    Receipt {
        schema: "aex.evidence-receipt.v1".to_owned(),
        receipt_digest: "sha256:0".to_owned(),
        receipt_id: context.receipt_id,
        class: context.class,
        layer: context.layer,
        lane: context.lane,
        concerns: context.concerns,
        source: context.source,
        inputs: context.inputs,
        subject: context.subject,
        selection: context.selection,
        inventory,
        failures: Vec::new(),
        attachments: Vec::new(),
        data: context.data,
        started_at: context.started_at,
        completed_at: context.completed_at,
        conclusion: if passed {
            "passed".to_owned()
        } else {
            "failed".to_owned()
        },
    }
    .seal()
}

/// The terminal verdict emitted by one Cargo command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CargoCommandSummary {
    /// Cargo's own `build-finished.success` value.
    pub success: bool,
}

/// Read the terminal verdict from Cargo's JSON message stream.
///
/// Every non-empty line must be one Cargo JSON message and the stream must
/// contain exactly one `build-finished` record. The workflow therefore cannot
/// turn an empty, truncated or concatenated log into a passing command receipt.
///
/// # Errors
/// Returns [`Exit::EvidenceMissing`] when Cargo emitted no terminal verdict and
/// [`Exit::EvidenceUnsound`] when the message stream is malformed or carries
/// more than one terminal verdict.
pub fn parse_cargo_messages(messages: &str) -> Result<CargoCommandSummary> {
    let mut verdict = None;
    for (index, line) in messages.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let message: serde_json::Value = serde_json::from_str(line).map_err(|err| {
            ToolError::single(
                Exit::EvidenceUnsound,
                "cargo-output-invalid",
                format!("Cargo message line {} is not JSON: {err}", index + 1),
            )
        })?;
        if message.get("reason").and_then(serde_json::Value::as_str) != Some("build-finished") {
            continue;
        }
        let success = message
            .get("success")
            .and_then(serde_json::Value::as_bool)
            .ok_or_else(|| {
                ToolError::single(
                    Exit::EvidenceUnsound,
                    "cargo-verdict-invalid",
                    format!(
                        "Cargo build-finished message on line {} has no boolean success field",
                        index + 1
                    ),
                )
            })?;
        if verdict.replace(success).is_some() {
            return Err(ToolError::single(
                Exit::EvidenceUnsound,
                "cargo-verdict-ambiguous",
                "Cargo output contains more than one build-finished verdict",
            ));
        }
    }
    verdict
        .map(|success| CargoCommandSummary { success })
        .ok_or_else(|| {
            ToolError::single(
                Exit::EvidenceMissing,
                "cargo-verdict-missing",
                "Cargo output contains no build-finished verdict",
            )
        })
}

/// Build one receipt for one Cargo command from Cargo's terminal verdict.
///
/// `declared` must be exactly one because the inventory counts the selected
/// command, not compiler targets inferred after execution. Package and unit
/// scope remain explicit in the context's selection and subject blocks.
///
/// # Errors
/// Returns [`Exit::EvidenceUnsound`] when the context does not declare exactly
/// one command and propagates canonicalization failures from sealing.
pub fn new_command_receipt(context: RunContext, summary: CargoCommandSummary) -> Result<Receipt> {
    if context.declared != 1 {
        return Err(ToolError::single(
            Exit::EvidenceUnsound,
            "command-inventory-invalid",
            format!(
                "command receipt `{}` declares {} commands; exactly one command was observed",
                context.receipt_id, context.declared
            ),
        ));
    }
    Receipt {
        schema: "aex.evidence-receipt.v1".to_owned(),
        receipt_digest: "sha256:0".to_owned(),
        receipt_id: context.receipt_id,
        class: context.class,
        layer: context.layer,
        lane: context.lane,
        concerns: context.concerns,
        source: context.source,
        inputs: context.inputs,
        subject: context.subject,
        selection: context.selection,
        inventory: Inventory {
            declared: 1,
            collected: 1,
            passed: u64::from(summary.success),
            failed: u64::from(!summary.success),
            ..Inventory::default()
        },
        failures: Vec::new(),
        attachments: Vec::new(),
        data: context.data,
        started_at: context.started_at,
        completed_at: context.completed_at,
        conclusion: if summary.success {
            "passed".to_owned()
        } else {
            "failed".to_owned()
        },
    }
    .seal()
}

/// One machine-readable check emitted only after its named producer ran.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CheckResult {
    /// Stable check identity within the producer.
    pub id: String,
    /// `passed` or `failed`; no skipped/unknown state is evidence.
    pub status: String,
}

/// Closed report consumed by [`new_check_receipt`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CheckReport {
    /// `aex.check-report.v1`.
    pub schema: String,
    /// Exact producer and version, for example `syft 1.50.0`.
    pub producer: String,
    /// Checks the producer actually completed.
    pub checks: Vec<CheckResult>,
}

/// Build a receipt from a closed machine-readable check report.
///
/// The report, rather than the workflow context, supplies the collected and
/// passing counters. Empty, duplicate, unknown or partial check sets are
/// refused, and the declared inventory must match exactly.
///
/// # Errors
/// Returns [`Exit::EvidenceUnsound`] for a malformed or partial report and
/// propagates canonicalization failures from sealing.
pub fn new_check_receipt(context: RunContext, report: &CheckReport) -> Result<Receipt> {
    let mut violations = Vec::new();
    if report.schema != "aex.check-report.v1" || report.producer.trim().is_empty() {
        violations.push(Violation::new(
            "check-report-identity",
            "a check report requires schema `aex.check-report.v1` and an exact producer identity",
        ));
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut passed = 0_u64;
    let mut failed = 0_u64;
    for check in &report.checks {
        if check.id.trim().is_empty() || !ids.insert(check.id.as_str()) {
            violations.push(Violation::new(
                "check-report-inventory",
                "check ids must be non-empty and unique",
            ));
        }
        match check.status.as_str() {
            "passed" => passed += 1,
            "failed" => failed += 1,
            other => violations.push(Violation::new(
                "check-report-status",
                format!("check `{}` has unsupported status `{other}`", check.id),
            )),
        }
    }
    let collected = report.checks.len() as u64;
    if collected == 0 || context.declared != collected {
        violations.push(Violation::new(
            "check-report-inventory",
            format!(
                "receipt `{}` declared {} check(s), but the producer reported {collected}",
                context.receipt_id, context.declared
            ),
        ));
    }
    if !violations.is_empty() {
        return Err(ToolError::many(Exit::EvidenceUnsound, violations));
    }
    Receipt {
        schema: "aex.evidence-receipt.v1".to_owned(),
        receipt_digest: "sha256:0".to_owned(),
        receipt_id: context.receipt_id,
        class: context.class,
        layer: context.layer,
        lane: context.lane,
        concerns: context.concerns,
        source: context.source,
        inputs: context.inputs,
        subject: context.subject,
        selection: context.selection,
        inventory: Inventory {
            declared: context.declared,
            collected,
            passed,
            failed,
            ..Inventory::default()
        },
        failures: Vec::new(),
        attachments: Vec::new(),
        data: context.data,
        started_at: context.started_at,
        completed_at: context.completed_at,
        conclusion: if failed == 0 { "passed" } else { "failed" }.to_owned(),
    }
    .seal()
}

/// Hash a file and record it on a receipt, then reseal.
///
/// The digest is computed here rather than accepted as an argument, because an
/// attachment digest somebody typed proves nothing about the bytes it names.
///
/// # Errors
/// Returns [`Exit::Usage`] when the file cannot be read, and propagates
/// canonicalization failure from resealing.
pub fn attach(receipt: Receipt, kind: &str, file: &std::path::Path, uri: &str) -> Result<Receipt> {
    let bytes =
        std::fs::read(file).map_err(|err| crate::error::io(&file.display().to_string(), &err))?;
    let mut receipt = receipt;
    receipt.attachments.push(Attachment {
        kind: kind.to_owned(),
        digest: canon::digest_bytes(&bytes),
        uri: uri.to_owned(),
        size_bytes: bytes.len() as u64,
    });
    receipt
        .attachments
        .sort_by(|a, b| (&a.kind, &a.digest).cmp(&(&b.kind, &b.digest)));
    receipt.seal()
}

/// Bind an already-earned receipt to one receipt-independent artifact subject.
///
/// This is an explicit, auditable transformation: the input receipt must be
/// sound, the draft subject must recompute exactly, and source plus unit scope
/// must already agree. Certification never performs this mutation implicitly.
///
/// # Errors
/// Returns a classified refusal for an unsound receipt, tampered artifact
/// subject, cross-source/unit binding or attempted rebind.
pub fn bind_artifact(
    mut receipt: Receipt,
    envelope: &crate::artifact::ArtifactEnvelope,
) -> Result<Receipt> {
    receipt.verify()?;
    let artifact_subject_digest = envelope.compute_artifact_subject_digest()?;
    if envelope.artifact_subject_digest != artifact_subject_digest {
        return Err(ToolError::single(
            Exit::ArtifactMismatch,
            "bind-artifact-subject-mismatch",
            format!(
                "envelope artifactSubjectDigest `{}` does not match the canonical artifact subject `{artifact_subject_digest}`",
                envelope.artifact_subject_digest
            ),
        ));
    }
    if receipt.source.repository != envelope.source.repository
        || receipt.source.commit_sha != envelope.source.commit_sha
        || !receipt
            .subject
            .unit_ids
            .iter()
            .any(|unit| unit == &envelope.unit.id)
    {
        return Err(ToolError::single(
            Exit::EvidenceUnsound,
            "bind-artifact-scope",
            format!(
                "receipt `{}` does not cover unit `{}` at the envelope's exact repository and commit",
                receipt.receipt_id, envelope.unit.id
            ),
        ));
    }
    if receipt
        .subject
        .artifact_subject_digest
        .as_deref()
        .is_some_and(|bound| bound != artifact_subject_digest)
    {
        return Err(ToolError::single(
            Exit::EvidenceUnsound,
            "bind-artifact-rebind",
            format!(
                "receipt `{}` is already bound to another artifact subject",
                receipt.receipt_id
            ),
        ));
    }
    receipt.subject.artifact_subject_digest = Some(artifact_subject_digest);
    let bound = receipt.seal()?;
    bound.verify()?;
    Ok(bound)
}

/// Freshness classes and their requirement.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FreshnessPolicy {
    /// Schema discriminator.
    pub schema: String,
    /// Class name to requirement.
    #[serde(default)]
    pub class: BTreeMap<String, FreshnessRule>,
}

/// One freshness rule.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FreshnessRule {
    /// What the receipt must be bound to: `artifact`, `commit`, `release` or
    /// `none`.
    pub bound_to: String,
    /// Maximum age in hours, where one applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age_hours: Option<u64>,
    /// Why this class is gated the way it is.
    pub rationale: String,
}

/// Check a receipt against its freshness class.
///
/// # Errors
/// Returns [`Exit::EvidenceStale`] when the receipt is older than its class
/// permits or is bound to the wrong subject, and [`Exit::EvidenceMissing`] when
/// the class is not in the policy at all.
pub fn check_freshness(
    receipt: &Receipt,
    policy: &FreshnessPolicy,
    release_id: &str,
    now: time::OffsetDateTime,
) -> Result<()> {
    let Some(rule) = policy.class.get(&receipt.class) else {
        return Err(ToolError::single(
            Exit::EvidenceMissing,
            "freshness-class-undeclared",
            format!(
                "receipt class `{}` is not in release/policy/freshness.toml; an ungated \
                 class would be permanently fresh",
                receipt.class
            ),
        ));
    };
    let mut violations = Vec::new();
    if rule.bound_to == "release" && receipt.subject.release_id.as_deref() != Some(release_id) {
        violations.push(Violation::new(
            "evidence-stale",
            format!(
                "receipt `{}` of class `{}` is bound to release `{}`, not `{release_id}`",
                receipt.receipt_id,
                receipt.class,
                receipt.subject.release_id.as_deref().unwrap_or("(none)")
            ),
        ));
    }
    if rule.bound_to == "artifact" && receipt.subject.artifact_subject_digest.is_none() {
        violations.push(Violation::new(
            "evidence-stale",
            format!(
                "receipt `{}` of class `{}` must be bound to an artifact subject digest",
                receipt.receipt_id, receipt.class
            ),
        ));
    }
    if let Some(max_age) = rule.max_age_hours {
        let completed = time::OffsetDateTime::parse(
            &receipt.completed_at,
            &time::format_description::well_known::Rfc3339,
        )
        .map_err(|err| {
            ToolError::single(
                Exit::EvidenceStale,
                "evidence-timestamp-unparseable",
                format!(
                    "receipt `{}` has completedAt `{}`: {err}",
                    receipt.receipt_id, receipt.completed_at
                ),
            )
        })?;
        let age = now - completed;
        if age > time::Duration::hours(i64::try_from(max_age).unwrap_or(i64::MAX)) {
            violations.push(Violation::new(
                "evidence-stale",
                format!(
                    "receipt `{}` of class `{}` is {} hour(s) old; the class permits {max_age}",
                    receipt.receipt_id,
                    receipt.class,
                    age.whole_hours()
                ),
            ));
        }
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(ToolError::many(Exit::EvidenceStale, violations))
    }
}

/// The jobs a lane declared it would run.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredJobs {
    /// Schema discriminator.
    pub schema: String,
    /// Lane name.
    pub lane: String,
    /// Job names.
    pub jobs: Vec<String>,
}

/// The aggregate verdict of a lane.
#[derive(Debug, Clone, Serialize)]
pub struct LaneReceipt {
    /// Schema discriminator.
    pub schema: &'static str,
    /// Lane name.
    pub lane: String,
    /// Declared jobs.
    pub declared: Vec<String>,
    /// Jobs that produced a receipt.
    pub collected: Vec<String>,
    /// Overall verdict.
    pub conclusion: String,
}

/// Compare declared jobs against collected receipts.
///
/// # Errors
/// Returns [`Exit::EvidenceMissing`] when a declared job produced no receipt,
/// and [`Exit::EvidenceUnsound`] when any receipt is unsound or not passing.
pub fn aggregate(receipts: &[Receipt], declared: &DeclaredJobs) -> Result<LaneReceipt> {
    let mut collected: Vec<String> = receipts
        .iter()
        .map(|receipt| receipt.source.job_name.clone())
        .collect();
    collected.sort();
    collected.dedup();
    let mut missing = Vec::new();
    for job in &declared.jobs {
        if !collected.contains(job) {
            missing.push(Violation::new(
                "lane-receipt-missing",
                format!(
                    "lane `{}` declared job `{job}` and collected no receipt for it; a \
                     required job that emitted nothing is not a pass",
                    declared.lane
                ),
            ));
        }
    }
    if !missing.is_empty() {
        return Err(ToolError::many(Exit::EvidenceMissing, missing));
    }
    let mut unsound = Vec::new();
    for receipt in receipts {
        if let Err(err) = receipt.verify() {
            unsound.extend(err.violations);
        }
        if !receipt.is_passing() {
            unsound.push(Violation::new(
                "lane-receipt-not-passed",
                format!(
                    "job `{}` concluded `{}`",
                    receipt.source.job_name, receipt.conclusion
                ),
            ));
        }
    }
    if !unsound.is_empty() {
        return Err(ToolError::many(Exit::EvidenceUnsound, unsound));
    }
    Ok(LaneReceipt {
        schema: "aex.lane-receipt.v1",
        lane: declared.lane.clone(),
        declared: declared.jobs.clone(),
        collected,
        conclusion: "passed".to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        DataBlock, DeclaredJobs, Inventory, JunitSummary, Receipt, SelectionBlock, Source,
        aggregate, parse_junit,
    };

    pub(super) fn receipt(class: &str) -> Receipt {
        Receipt {
            schema: "aex.evidence-receipt.v1".to_owned(),
            receipt_digest: "sha256:0".to_owned(),
            receipt_id: format!("rc_{class}"),
            class: class.to_owned(),
            layer: "unit".to_owned(),
            lane: "pr".to_owned(),
            concerns: vec!["contract".to_owned()],
            source: Source {
                repository: "aexhq/aex".to_owned(),
                commit_sha: "a".repeat(40),
                tree_clean: true,
                workflow_run_id: "1".to_owned(),
                run_attempt: 1,
                job_name: "rust".to_owned(),
                builder_id: "https://github.com/aexhq/aex/.github/workflows/pr.yml".to_owned(),
            },
            inputs: super::Inputs::default(),
            subject: super::Subject::default(),
            selection: SelectionBlock {
                mode: "affected".to_owned(),
                filterset: "all()".to_owned(),
                packages: vec!["aex-wire".to_owned()],
                partition: None,
                reasons_digest: None,
            },
            inventory: Inventory {
                declared: 10,
                collected: 10,
                passed: 10,
                failed: 0,
                ..Inventory::default()
            },
            failures: Vec::new(),
            attachments: Vec::new(),
            data: DataBlock {
                budget_micro_usd: None,
                spent_micro_usd: None,
                cleanup_ledger_digest: None,
                residue: "none".to_owned(),
                secret_canary_observed: false,
                residue_explanation: None,
            },
            started_at: "2026-08-01T00:00:00Z".to_owned(),
            completed_at: "2026-08-01T00:05:00Z".to_owned(),
            conclusion: "passed".to_owned(),
        }
    }

    #[test]
    fn a_sound_receipt_verifies_and_seals_to_a_stable_digest() {
        let sealed = receipt("unit").seal().unwrap();
        sealed.verify().unwrap();
        let again = sealed.clone().seal().unwrap();
        assert_eq!(sealed.receipt_digest, again.receipt_digest);
    }

    #[test]
    fn each_forbidden_counter_fails_on_its_own() {
        for mutate in [
            |i: &mut Inventory| i.skipped = 1,
            |i: &mut Inventory| i.ignored = 1,
            |i: &mut Inventory| i.filtered_at_runtime = 1,
            |i: &mut Inventory| i.retried = 1,
            |i: &mut Inventory| i.flaky = 1,
        ] {
            let mut receipt = receipt("unit");
            mutate(&mut receipt.inventory);
            let err = receipt.verify().unwrap_err();
            assert_eq!(err.exit.code(), 41);
            assert!(err.rules().contains(&"flake-skipped-test"));
        }
    }

    #[test]
    fn an_empty_suite_is_not_a_pass() {
        let mut receipt = receipt("unit");
        receipt.inventory = Inventory::default();
        let err = receipt.verify().unwrap_err();
        assert!(err.rules().contains(&"flake-empty-selection"));
    }

    #[test]
    fn a_declared_case_that_never_ran_is_a_mismatch() {
        let mut receipt = receipt("unit");
        receipt.inventory.collected = 9;
        receipt.inventory.passed = 9;
        let err = receipt.verify().unwrap_err();
        assert!(err.rules().contains(&"flake-inventory-mismatch"));
    }

    #[test]
    fn a_rerun_that_discarded_its_first_failure_is_rejected() {
        let mut receipt = receipt("unit");
        receipt.source.run_attempt = 2;
        let err = receipt.verify().unwrap_err();
        assert!(err.rules().contains(&"flake-first-failure-lost"));
    }

    #[test]
    fn unexplained_residue_fails_the_receipt() {
        let mut receipt = receipt("e2e");
        receipt.data.residue = "leaked".to_owned();
        let err = receipt.verify().unwrap_err();
        assert!(err.rules().contains(&"data-residue"));
        receipt.data.residue_explanation =
            Some("retained snapshot with a verified 24 h TTL".to_owned());
        receipt.verify().unwrap();
    }

    /// The receipt's residue field used to be self-reported, with no
    /// production constructor anywhere in the workspace, so a leaking run wrote
    /// `none` and passed. It is now derived from the janitor's own sweep.
    #[test]
    fn the_residue_field_is_derived_from_a_real_sweep_rather_than_declared() {
        use crate::janitor::{DiscoveredResource, Inventory as PlaneInventory, SweepMode, sweep};

        let policy = aex_workspace_check::policy::Policy::embedded();
        let mut tags = std::collections::BTreeMap::new();
        tags.insert(
            policy.janitor.synthetic_tag.clone(),
            policy.janitor.synthetic_value.clone(),
        );
        tags.insert(
            policy.janitor.run_id_tag.clone(),
            format!("tr_{}", "0".repeat(32)),
        );
        tags.insert(policy.janitor.owner_tag.clone(), "delivery".to_owned());
        tags.insert(policy.janitor.lane_tag.clone(), "e2e".to_owned());
        tags.insert(
            policy.janitor.expires_at_tag.clone(),
            "2026-01-01T00:00:00Z".to_owned(),
        );

        let clean = sweep(
            &PlaneInventory {
                schema: "aex.janitor-inventory.v1".to_owned(),
                plane: crate::admit::Plane::Prd,
                resources: Vec::new(),
            },
            &crate::janitor::DryRun,
            SweepMode::Reclaim,
            time::OffsetDateTime::now_utc(),
        );
        let block = DataBlock::from_sweep(&clean, Some("sha256:abc".to_owned()), None, None, false);
        assert_eq!(block.residue, "none");
        assert_eq!(block.cleanup_ledger_digest.as_deref(), Some("sha256:abc"));
        let mut swept = receipt("e2e");
        swept.data = block;
        swept.verify().expect("a swept-clean lane passes");

        // A kind no sweep can find, from an expired run: real residue.
        let dirty = sweep(
            &PlaneInventory {
                schema: "aex.janitor-inventory.v1".to_owned(),
                plane: crate::admit::Plane::Prd,
                resources: vec![DiscoveredResource {
                    kind: "sqs_message".to_owned(),
                    identity: "msg-1".to_owned(),
                    tags,
                }],
            },
            &crate::janitor::DryRun,
            SweepMode::Reclaim,
            time::OffsetDateTime::now_utc(),
        );
        let block = DataBlock::from_sweep(&dirty, None, None, None, false);
        assert_eq!(block.residue, "unreclaimed");
        assert!(block.residue_explanation.is_some());
        let mut leaked = receipt("e2e");
        leaked.data = block;
        let err = leaked
            .verify()
            .expect_err("unreclaimed residue fails the lane");
        assert_eq!(err.exit.code(), 41);
        assert!(err.rules().contains(&"data-residue-unreclaimed"));
    }

    /// The `explained` escape hatch is for a declared survivor with a verified
    /// TTL. It must not reach the janitor's own verdict: a lane cannot write a
    /// sentence that makes production residue acceptable.
    #[test]
    fn no_explanation_makes_unreclaimed_residue_acceptable() {
        let mut leaked = receipt("e2e");
        leaked.data.residue = "unreclaimed".to_owned();
        leaked.data.residue_explanation =
            Some("we will clean it up by hand later, honestly".to_owned());
        let err = leaked.verify().unwrap_err();
        assert_eq!(err.exit.code(), 41);
        assert!(err.rules().contains(&"data-residue-unreclaimed"));
        assert!(
            !err.rules().contains(&"data-residue"),
            "the unreclaimed rule replaces the explanation rule rather than joining it"
        );
    }

    #[test]
    fn junit_counts_elements_rather_than_trusting_the_summary_attributes() {
        let xml = r#"<testsuite tests="99" skipped="0">
  <testcase name="a"/>
  <testcase name="b"><skipped/></testcase>
  <testcase name="c"><flakyFailure/></testcase>
  <testcases name="not a case"/>
</testsuite>"#;
        assert_eq!(
            parse_junit(xml),
            JunitSummary {
                cases: 3,
                failures: 0,
                skipped: 1,
                flaky: 1
            }
        );
    }

    #[test]
    fn cargo_build_finished_is_the_lint_verdict() {
        let messages = r#"{"reason":"compiler-artifact","package_id":"path+file:///repo#aex-wire@0.1.0"}
{"reason":"build-finished","success":true}"#;
        let summary = super::parse_cargo_messages(messages).unwrap();
        assert!(summary.success);

        let mut context = context(1);
        context.class = "lint".to_owned();
        let built = super::new_command_receipt(context, summary).unwrap();
        assert_eq!(built.class, "lint");
        assert_eq!(built.inventory.declared, 1);
        assert_eq!(built.inventory.collected, 1);
        assert_eq!(built.inventory.passed, 1);
        assert_eq!(built.conclusion, "passed");
        built.verify().unwrap();
    }

    #[test]
    fn failed_cargo_output_cannot_become_a_passing_receipt() {
        let messages = r#"{"reason":"compiler-message"}
{"reason":"build-finished","success":false}"#;
        let summary = super::parse_cargo_messages(messages).unwrap();
        assert!(!summary.success);

        let built = super::new_command_receipt(context(1), summary).unwrap();
        assert_eq!(built.inventory.failed, 1);
        assert_eq!(built.conclusion, "failed");
        assert!(!built.is_passing());
    }

    #[test]
    fn missing_or_ambiguous_cargo_verdict_is_not_evidence() {
        let missing = super::parse_cargo_messages(r#"{"reason":"compiler-artifact"}"#).unwrap_err();
        assert_eq!(missing.exit.code(), 40);
        assert!(missing.rules().contains(&"cargo-verdict-missing"));

        let duplicate = super::parse_cargo_messages(
            r#"{"reason":"build-finished","success":true}
{"reason":"build-finished","success":true}"#,
        )
        .unwrap_err();
        assert_eq!(duplicate.exit.code(), 41);
        assert!(duplicate.rules().contains(&"cargo-verdict-ambiguous"));
    }

    #[test]
    fn a_command_receipt_must_declare_exactly_one_command() {
        let err =
            super::new_command_receipt(context(2), super::CargoCommandSummary { success: true })
                .unwrap_err();
        assert_eq!(err.exit.code(), 41);
        assert!(err.rules().contains(&"command-inventory-invalid"));
    }

    #[test]
    fn a_check_receipt_counts_the_closed_producer_report() {
        let report = super::CheckReport {
            schema: "aex.check-report.v1".to_owned(),
            producer: "syft 1.50.0".to_owned(),
            checks: vec![
                super::CheckResult {
                    id: "cyclonedx-1.6".to_owned(),
                    status: "passed".to_owned(),
                },
                super::CheckResult {
                    id: "artifact-subject-bound".to_owned(),
                    status: "passed".to_owned(),
                },
            ],
        };
        let built = super::new_check_receipt(context(2), &report).unwrap();
        assert_eq!(built.inventory.collected, 2);
        assert_eq!(built.inventory.passed, 2);
        assert_eq!(built.conclusion, "passed");
        built.verify().unwrap();
    }

    #[test]
    fn a_check_report_cannot_hide_empty_duplicate_or_unknown_results() {
        let empty = super::CheckReport {
            schema: "aex.check-report.v1".to_owned(),
            producer: "grype 0.116.1".to_owned(),
            checks: Vec::new(),
        };
        let err = super::new_check_receipt(context(1), &empty).unwrap_err();
        assert!(err.rules().contains(&"check-report-inventory"));

        let invalid = super::CheckReport {
            schema: "aex.check-report.v1".to_owned(),
            producer: "grype 0.116.1".to_owned(),
            checks: vec![
                super::CheckResult {
                    id: "advisories".to_owned(),
                    status: "passed".to_owned(),
                },
                super::CheckResult {
                    id: "advisories".to_owned(),
                    status: "skipped".to_owned(),
                },
            ],
        };
        let err = super::new_check_receipt(context(2), &invalid).unwrap_err();
        assert!(err.rules().contains(&"check-report-inventory"));
        assert!(err.rules().contains(&"check-report-status"));
    }

    fn context(declared: u64) -> super::RunContext {
        let template = receipt("unit");
        super::RunContext {
            receipt_id: "rc_local".to_owned(),
            class: "unit".to_owned(),
            layer: "unit".to_owned(),
            lane: "pr".to_owned(),
            concerns: vec!["contract".to_owned()],
            declared,
            source: template.source,
            inputs: super::Inputs::default(),
            subject: super::Subject::default(),
            selection: template.selection,
            data: template.data,
            started_at: "2026-08-01T00:00:00Z".to_owned(),
            completed_at: "2026-08-01T00:05:00Z".to_owned(),
        }
    }

    const CLEAN_JUNIT: JunitSummary = JunitSummary {
        cases: 3,
        failures: 0,
        skipped: 0,
        flaky: 0,
    };

    #[test]
    fn a_new_receipt_derives_its_verdict_from_the_counters() {
        let built = super::new_receipt(context(3), &CLEAN_JUNIT).unwrap();
        assert_eq!(built.conclusion, "passed");
        assert_eq!(built.inventory.declared, 3);
        assert_eq!(built.inventory.collected, 3);
        assert_eq!(built.inventory.passed, 3);
        built
            .verify()
            .expect("a clean run produces a sound receipt");
    }

    #[test]
    fn a_run_that_lost_cases_between_listing_and_executing_fails() {
        // Deriving `declared` from the report would make this state
        // unrepresentable, and a run that listed ten cases and executed three
        // would report `passed`.
        let built = super::new_receipt(context(10), &CLEAN_JUNIT).unwrap();
        assert_eq!(built.conclusion, "failed");
        let err = built.verify().unwrap_err();
        assert_eq!(err.exit.code(), 41);
        assert!(err.rules().contains(&"flake-inventory-mismatch"));
    }

    #[test]
    fn a_skip_cannot_produce_a_passing_receipt() {
        let junit = JunitSummary {
            cases: 3,
            failures: 0,
            skipped: 1,
            flaky: 0,
        };
        let built = super::new_receipt(context(3), &junit).unwrap();
        assert_eq!(built.conclusion, "failed");
        assert_eq!(built.inventory.skipped, 1);
        let err = built.verify().unwrap_err();
        assert!(err.rules().contains(&"flake-skipped-test"));
    }

    #[test]
    fn a_run_that_declared_nothing_is_not_evidence() {
        let junit = JunitSummary {
            cases: 0,
            failures: 0,
            skipped: 0,
            flaky: 0,
        };
        let built = super::new_receipt(context(0), &junit).unwrap();
        assert_eq!(built.conclusion, "failed");
    }

    #[test]
    fn attaching_hashes_the_bytes_and_reseals() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("report.xml");
        std::fs::write(&file, b"<testsuite/>").unwrap();
        let built = super::new_receipt(context(3), &CLEAN_JUNIT).unwrap();
        let before = built.receipt_digest.clone();
        let attached = super::attach(built, "junit", &file, "s3://bucket/report.xml").unwrap();
        assert_eq!(attached.attachments.len(), 1);
        assert_eq!(
            attached.attachments[0].digest,
            crate::canon::digest_bytes(b"<testsuite/>")
        );
        assert_eq!(attached.attachments[0].size_bytes, 12);
        assert_ne!(
            attached.receipt_digest, before,
            "a receipt that gained an attachment is different bytes"
        );
        attached
            .verify()
            .expect("attaching keeps the receipt sound");
    }

    #[test]
    fn aggregation_fails_when_a_declared_job_produced_no_receipt() {
        let declared = DeclaredJobs {
            schema: "aex.declared-jobs.v1".to_owned(),
            lane: "pr".to_owned(),
            jobs: vec!["rust".to_owned(), "terraform".to_owned()],
        };
        let err = aggregate(&[receipt("unit")], &declared).unwrap_err();
        assert_eq!(err.exit.code(), 40);
        assert!(err.rules().contains(&"lane-receipt-missing"));
    }

    #[test]
    fn aggregation_accepts_no_receipts_when_no_producer_was_selected() {
        let declared = DeclaredJobs {
            schema: "aex.declared-jobs.v1".to_owned(),
            lane: "pr".to_owned(),
            jobs: Vec::new(),
        };

        let lane = aggregate(&[], &declared).unwrap();

        assert!(lane.declared.is_empty());
        assert!(lane.collected.is_empty());
        assert_eq!(lane.conclusion, "passed");
    }

    #[test]
    fn aggregation_fails_when_a_collected_receipt_did_not_pass() {
        let declared = DeclaredJobs {
            schema: "aex.declared-jobs.v1".to_owned(),
            lane: "pr".to_owned(),
            jobs: vec!["rust".to_owned()],
        };
        let mut receipt = receipt("unit");
        receipt.conclusion = "infrastructure_failed".to_owned();
        let err = aggregate(&[receipt], &declared).unwrap_err();
        assert_eq!(err.exit.code(), 41);
        assert!(err.rules().contains(&"lane-receipt-not-passed"));
    }
}

#[cfg(test)]
mod freshness_tests {
    use super::{FreshnessPolicy, check_freshness};

    const POLICY: &str = r#"
schema = "aex.freshness-policy.v1"

[class.unit]
bound_to = "artifact"
rationale = "bound to the exact artifact; there is nothing for age to invalidate"

[class.smoke]
bound_to = "release"
max_age_hours = 24
rationale = "a deployed smoke result describes a plane that keeps changing"

[class.capacity]
bound_to = "none"
max_age_hours = 168
rationale = "expensive to earn; a week bounds drift without re-running every release"
"#;

    #[test]
    fn a_class_outside_the_policy_is_not_permanently_fresh() {
        let policy: FreshnessPolicy = toml::from_str(POLICY).unwrap();
        let receipt = super::tests::receipt("soak");
        let err = check_freshness(
            &receipt,
            &policy,
            "sha256:00",
            time::OffsetDateTime::UNIX_EPOCH,
        )
        .unwrap_err();
        assert_eq!(err.rules(), vec!["freshness-class-undeclared"]);
        assert_eq!(err.exit.code(), 40);
    }

    #[test]
    fn an_envelope_bound_class_must_name_an_envelope() {
        let policy: FreshnessPolicy = toml::from_str(POLICY).unwrap();
        let receipt = super::tests::receipt("unit");
        let err = check_freshness(
            &receipt,
            &policy,
            "sha256:00",
            time::OffsetDateTime::UNIX_EPOCH,
        )
        .unwrap_err();
        assert_eq!(err.exit.code(), 42);
    }

    #[test]
    fn a_release_bound_class_must_name_this_release() {
        let policy: FreshnessPolicy = toml::from_str(POLICY).unwrap();
        let mut receipt = super::tests::receipt("smoke");
        receipt.subject.release_id = Some("sha256:other".to_owned());
        let err = check_freshness(
            &receipt,
            &policy,
            "sha256:00",
            time::OffsetDateTime::parse(
                "2026-08-01T01:00:00Z",
                &time::format_description::well_known::Rfc3339,
            )
            .unwrap(),
        )
        .unwrap_err();
        assert_eq!(err.exit.code(), 42);
    }

    #[test]
    fn an_aged_out_receipt_is_stale() {
        let policy: FreshnessPolicy = toml::from_str(POLICY).unwrap();
        let mut receipt = super::tests::receipt("smoke");
        receipt.subject.release_id = Some("sha256:00".to_owned());
        let fresh = time::OffsetDateTime::parse(
            "2026-08-01T10:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        check_freshness(&receipt, &policy, "sha256:00", fresh).unwrap();
        let stale = time::OffsetDateTime::parse(
            "2026-08-03T10:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        let err = check_freshness(&receipt, &policy, "sha256:00", stale).unwrap_err();
        assert_eq!(err.exit.code(), 42);
        assert!(err.rules().contains(&"evidence-stale"));
    }
}
