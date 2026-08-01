//! The exit-code contract and the violation shape every check reports.
//!
//! Nothing exits `0` on a warning and there is no `--force`. A check either
//! reports zero violations and exits `0`, or reports every violation it found
//! and exits the code that classifies the failure class.

use std::fmt;

/// The complete exit-code contract (plan 14 §3.3).
///
/// The numeric values are the public contract: workflows, the private
/// repository and operators branch on them, so a variant may never be
/// renumbered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(i32)]
pub enum Exit {
    /// Success.
    Ok = 0,
    /// Usage error: an argument combination that cannot be executed.
    Usage = 2,
    /// Graph verification failure: unclassified node, orphan path, missing
    /// owner, cycle.
    GraphVerification = 10,
    /// Routing could not be computed.
    RoutingUndecidable = 11,
    /// `selftest` detected a monotonicity or determinism violation.
    SelftestViolation = 12,
    /// The artifact envelope is structurally invalid.
    EnvelopeInvalid = 20,
    /// The artifact digest or size does not match the envelope.
    ArtifactMismatch = 21,
    /// Provenance or attestation is missing or unverifiable.
    ProvenanceMissing = 22,
    /// SBOM, license or advisory denial.
    SupplyChainDenied = 23,
    /// The composition manifest is structurally invalid.
    ManifestInvalid = 30,
    /// The manifest carries an environment identity, mutable tag, branch or
    /// unresolved range.
    ManifestEnvironmentIdentity = 31,
    /// Composition compatibility violation: schema head, adjacency, order.
    CompositionIncompatible = 32,
    /// A required evidence receipt is missing.
    EvidenceMissing = 40,
    /// A receipt reports a skip, ignore, runtime filter, retry or flaky pass.
    EvidenceUnsound = 41,
    /// A receipt is stale for its freshness class.
    EvidenceStale = 42,
    /// The verification statement is missing or not attested.
    VerificationMissing = 43,
    /// Admission denied by policy: quota, backup, approval, maintenance.
    AdmissionDenied = 44,
    /// Deployment ledger fence conflict.
    LedgerFenceConflict = 50,
    /// Desired and actual state diverge.
    StateDiverged = 51,
    /// An unclassified private path.
    PrivatePathUnclassified = 60,
    /// A Terraform plan or source policy violation.
    TerraformPolicy = 70,
}

impl Exit {
    /// The process exit code.
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }
}

impl fmt::Display for Exit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.code())
    }
}

/// One reported rule violation, rendered as `[<rule>] <detail>`.
///
/// The shape is shared with `tools/aex-workspace-check` deliberately: an
/// operator reading CI output should not have to learn two formats.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Violation {
    /// Stable machine-readable rule id, kebab-case.
    pub rule: String,
    /// Human-readable detail naming the offending path or node.
    pub detail: String,
}

impl Violation {
    /// Build a violation from any displayable rule id and detail.
    pub fn new(rule: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            rule: rule.into(),
            detail: detail.into(),
        }
    }
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.rule, self.detail)
    }
}

/// A failure carrying its exit classification and every violation found.
///
/// Checks accumulate rather than short-circuit: reporting one violation per run
/// turns a single review into a sequence of them.
#[derive(Debug, Clone, thiserror::Error)]
pub struct ToolError {
    /// How the failure is classified for the caller.
    pub exit: Exit,
    /// Everything that was wrong, in a stable order.
    pub violations: Vec<Violation>,
}

impl ToolError {
    /// A failure carrying a single violation.
    pub fn single(exit: Exit, rule: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            exit,
            violations: vec![Violation::new(rule, detail)],
        }
    }

    /// A failure carrying an accumulated violation list.
    #[must_use]
    pub fn many(exit: Exit, violations: Vec<Violation>) -> Self {
        Self { exit, violations }
    }

    /// Every rule id reported, in order.
    #[must_use]
    pub fn rules(&self) -> Vec<&str> {
        self.violations.iter().map(|v| v.rule.as_str()).collect()
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "{} violation(s), exit {}",
            self.violations.len(),
            self.exit
        )?;
        for violation in &self.violations {
            writeln!(f, "  {violation}")?;
        }
        Ok(())
    }
}

/// The result type every check returns.
pub type Result<T> = std::result::Result<T, ToolError>;

/// Build a usage error.
pub fn usage(detail: impl Into<String>) -> ToolError {
    ToolError::single(Exit::Usage, "usage", detail)
}

/// Wrap an I/O failure as a usage error naming the path that could not be read.
#[must_use]
pub fn io(path: &str, err: &std::io::Error) -> ToolError {
    ToolError::single(Exit::Usage, "io", format!("`{path}`: {err}"))
}

#[cfg(test)]
mod tests {
    use super::{Exit, ToolError, Violation};

    #[test]
    fn exit_codes_match_the_published_contract() {
        assert_eq!(Exit::Ok.code(), 0);
        assert_eq!(Exit::Usage.code(), 2);
        assert_eq!(Exit::GraphVerification.code(), 10);
        assert_eq!(Exit::RoutingUndecidable.code(), 11);
        assert_eq!(Exit::SelftestViolation.code(), 12);
        assert_eq!(Exit::EnvelopeInvalid.code(), 20);
        assert_eq!(Exit::ArtifactMismatch.code(), 21);
        assert_eq!(Exit::ProvenanceMissing.code(), 22);
        assert_eq!(Exit::SupplyChainDenied.code(), 23);
        assert_eq!(Exit::ManifestInvalid.code(), 30);
        assert_eq!(Exit::ManifestEnvironmentIdentity.code(), 31);
        assert_eq!(Exit::CompositionIncompatible.code(), 32);
        assert_eq!(Exit::EvidenceMissing.code(), 40);
        assert_eq!(Exit::EvidenceUnsound.code(), 41);
        assert_eq!(Exit::EvidenceStale.code(), 42);
        assert_eq!(Exit::VerificationMissing.code(), 43);
        assert_eq!(Exit::AdmissionDenied.code(), 44);
        assert_eq!(Exit::LedgerFenceConflict.code(), 50);
        assert_eq!(Exit::StateDiverged.code(), 51);
        assert_eq!(Exit::PrivatePathUnclassified.code(), 60);
        assert_eq!(Exit::TerraformPolicy.code(), 70);
    }

    #[test]
    fn violation_renders_as_rule_and_detail() {
        let violation = Violation::new("orphan-path", "`docs/stray.md` matches no rule");
        assert_eq!(
            violation.to_string(),
            "[orphan-path] `docs/stray.md` matches no rule"
        );
    }

    #[test]
    fn tool_error_reports_every_rule_it_accumulated() {
        let err = ToolError::many(
            Exit::GraphVerification,
            vec![
                Violation::new("orphan-path", "a"),
                Violation::new("aex-metadata-missing", "b"),
            ],
        );
        assert_eq!(err.rules(), vec!["orphan-path", "aex-metadata-missing"]);
        assert!(err.to_string().contains("2 violation(s), exit 10"));
    }
}
