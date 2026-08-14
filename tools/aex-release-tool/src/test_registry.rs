//! Consuming the derived test registry.
//!
//! The registry, its rules and the flake scanner live in
//! `aex-workspace-check`. Nothing here re-implements any of them: this module
//! reads the two committed documents through that crate's own types, and adds
//! only the one thing neither authority can do alone — checking that the
//! delivery graph and the test registry agree about which live companions
//! exist.
//!
//! `release/test-registry.json` and `release/unearned-evidence.json` are
//! regenerated on conflict with `aex-workspace-check registry build`, never
//! hand-merged, exactly like `Cargo.lock`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use aex_workspace_check::registry::{RegistryDocument, UnearnedRow};

use crate::error::{Exit, Result, ToolError, Violation, io};

/// Where the derived registry is committed.
pub const REGISTRY_PATH: &str = "release/test-registry.json";
/// Where the evidence that cannot yet be earned is listed.
pub const UNEARNED_PATH: &str = "release/unearned-evidence.json";

/// Read the committed registry document.
///
/// # Errors
/// Returns [`Exit::Usage`] when the document is absent or does not parse.
pub fn load_document(root: &Path) -> Result<RegistryDocument> {
    let path = root.join(REGISTRY_PATH);
    let text =
        std::fs::read_to_string(&path).map_err(|err| io(&path.display().to_string(), &err))?;
    serde_json::from_str(&text).map_err(|err| {
        ToolError::single(
            Exit::Usage,
            "test-registry-unparseable",
            format!("`{REGISTRY_PATH}`: {err}"),
        )
    })
}

/// Read the committed unearned-evidence ledger.
///
/// # Errors
/// Returns [`Exit::Usage`] when the document is absent or does not parse.
pub fn load_unearned(root: &Path) -> Result<Vec<UnearnedRow>> {
    let path = root.join(UNEARNED_PATH);
    let text =
        std::fs::read_to_string(&path).map_err(|err| io(&path.display().to_string(), &err))?;
    serde_json::from_str(&text).map_err(|err| {
        ToolError::single(
            Exit::Usage,
            "unearned-evidence-unparseable",
            format!("`{UNEARNED_PATH}`: {err}"),
        )
    })
}

/// Evidence that cannot be earned yet, indexed by what it is about.
///
/// A required receipt that is missing *and* listed here is a different fact
/// from one that is merely absent: the first is a recorded, owned gap and the
/// second is a hole nobody noticed. Admission refuses either way, but it says
/// which.
#[derive(Debug, Clone, Default)]
pub struct UnearnedIndex {
    by_subject: BTreeMap<String, Vec<UnearnedRow>>,
    package_subject_by_unit: BTreeMap<String, String>,
}

impl UnearnedIndex {
    /// Build the index from the ledger rows.
    #[must_use]
    pub fn new(rows: Vec<UnearnedRow>) -> Self {
        let mut by_subject: BTreeMap<String, Vec<UnearnedRow>> = BTreeMap::new();
        for row in rows {
            by_subject.entry(row.subject.clone()).or_default().push(row);
        }
        Self {
            by_subject,
            package_subject_by_unit: BTreeMap::new(),
        }
    }

    /// Load the index from a repository root.
    ///
    /// # Errors
    /// Propagates a missing or unparseable registry or ledger.
    pub fn load(root: &Path) -> Result<Self> {
        let registry = load_document(root)?;
        let mut index = Self::new(load_unearned(root)?);
        index.package_subject_by_unit = registry
            .packages
            .into_iter()
            .filter(|(_, package)| {
                matches!(package.meta.role.as_str(), "deployable" | "runtime_image")
            })
            .filter_map(|(path, package)| package.meta.deployable.map(|unit| (unit, path)))
            .collect();
        Ok(index)
    }

    /// Whether anything about this subject is recorded as unearned.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_subject.is_empty()
    }

    /// The recorded reason a subject cannot produce evidence yet, if any.
    ///
    /// The subject is matched by exact id, by the registry's release-unit to
    /// package-path mapping, and finally by suffix for packages whose unit and
    /// package names are identical. The explicit mapping matters for units such
    /// as `session-api`, whose package path is `services/session-stream-api`.
    #[must_use]
    pub fn reason_for(&self, subject: &str) -> Option<&UnearnedRow> {
        if let Some(rows) = self.by_subject.get(subject) {
            return rows.first();
        }
        if let Some(rows) = self
            .package_subject_by_unit
            .get(subject)
            .and_then(|path| self.by_subject.get(path))
        {
            return rows.first();
        }
        self.by_subject
            .iter()
            .find(|(key, _)| key.rsplit('/').next() == Some(subject))
            .and_then(|(_, rows)| rows.first())
    }

    /// Every distinct owner named in the ledger.
    #[must_use]
    pub fn owners(&self) -> BTreeSet<&str> {
        self.by_subject
            .values()
            .flatten()
            .map(|row| row.owner.as_str())
            .collect()
    }
}

/// Assert the delivery graph and the derived registry agree on the live set.
///
/// Two authorities derive it from the same `live_suite` declarations by
/// different routes. If they disagree, one of them is reading metadata the
/// other is not, and no downstream lane can tell which.
///
/// # Errors
/// Returns [`Exit::GraphVerification`] naming each side of the disagreement.
pub fn check_live_target_agreement(
    graph_live_targets: &BTreeSet<String>,
    registry: &RegistryDocument,
) -> Result<()> {
    let registry_targets: BTreeSet<&str> =
        registry.live_targets.iter().map(String::as_str).collect();
    let mut violations = Vec::new();
    for target in graph_live_targets {
        if !registry_targets.contains(target.as_str()) {
            violations.push(Violation::new(
                "live-target-disagreement",
                format!(
                    "the delivery graph derives live target `{target}` and \
                     release/test-registry.json does not"
                ),
            ));
        }
    }
    for target in &registry_targets {
        if !graph_live_targets.contains(*target) {
            violations.push(Violation::new(
                "live-target-disagreement",
                format!(
                    "release/test-registry.json derives live target `{target}` and the \
                     delivery graph does not"
                ),
            ));
        }
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(ToolError::many(Exit::GraphVerification, violations))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use aex_workspace_check::registry::{Phase, RegistryDocument, UnearnedRow};

    use super::{UnearnedIndex, check_live_target_agreement};

    fn row(subject: &str, owner: &str) -> UnearnedRow {
        UnearnedRow {
            reason_class: "awaiting_owner".to_owned(),
            subject: subject.to_owned(),
            owner: owner.to_owned(),
            blocking_rule: "aex-empty-unit".to_owned(),
            detail: "awaiting the owning stream".to_owned(),
        }
    }

    fn document(live: &[&str]) -> RegistryDocument {
        RegistryDocument {
            schema: "aex.test-registry.v1".to_owned(),
            phase: Phase::SourceRewrite,
            inputs: std::collections::BTreeMap::new(),
            totals: std::collections::BTreeMap::new(),
            packages: std::collections::BTreeMap::new(),
            live_targets: live.iter().map(|name| (*name).to_owned()).collect(),
            seam_claims: std::collections::BTreeMap::new(),
        }
    }

    #[test]
    fn the_index_resolves_a_matching_unit_id_against_a_manifest_path_suffix() {
        let index =
            UnearnedIndex::new(vec![row("workers/file-ingest-worker", "regional-services")]);
        assert_eq!(
            index
                .reason_for("file-ingest-worker")
                .map(|row| row.owner.as_str()),
            Some("regional-services")
        );
        assert_eq!(
            index
                .reason_for("workers/file-ingest-worker")
                .map(|row| row.owner.as_str()),
            Some("regional-services")
        );
        assert!(index.reason_for("ghost").is_none());
    }

    #[test]
    fn the_index_reports_every_owner_that_owes_something() {
        let index = UnearnedIndex::new(vec![
            row("crates/aex-wire", "contracts"),
            row("runtimes/brain-mux", "brain-core"),
        ]);
        assert_eq!(
            index.owners().into_iter().collect::<Vec<_>>(),
            vec!["brain-core", "contracts"]
        );
    }

    #[test]
    fn agreeing_authorities_pass() {
        let graph: BTreeSet<String> = ["aex-live-brain-mux".to_owned()].into_iter().collect();
        check_live_target_agreement(&graph, &document(&["aex-live-brain-mux"])).unwrap();
    }

    #[test]
    fn a_target_only_the_graph_knows_is_a_disagreement() {
        let graph: BTreeSet<String> = ["aex-live-brain-mux".to_owned()].into_iter().collect();
        let err = check_live_target_agreement(&graph, &document(&[])).unwrap_err();
        assert_eq!(err.exit.code(), 10);
        assert!(err.rules().contains(&"live-target-disagreement"));
        assert!(
            err.violations[0]
                .detail
                .contains("the delivery graph derives")
        );
    }

    #[test]
    fn a_target_only_the_registry_knows_is_a_disagreement() {
        let graph: BTreeSet<String> = BTreeSet::new();
        let err = check_live_target_agreement(&graph, &document(&["aex-live-site"])).unwrap_err();
        assert!(
            err.violations[0]
                .detail
                .contains("release/test-registry.json derives")
        );
    }

    #[test]
    fn the_shipped_documents_parse_and_the_two_authorities_agree() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let registry = super::load_document(&root).expect("the committed registry");
        let index = UnearnedIndex::load(&root).expect("the committed ledger");
        assert!(
            !registry.live_targets.is_empty(),
            "the registry derives the live set; an empty one means it read no metadata"
        );
        assert!(
            !index.is_empty(),
            "nothing is deployed, so live evidence must be listed as unearned rather \
             than silently absent"
        );
        assert_eq!(
            index
                .reason_for("session-api")
                .map(|row| row.subject.as_str()),
            Some("services/session-stream-api"),
            "release-unit evidence must resolve through the package mapping"
        );

        let inputs = crate::graph::inputs::GraphInputs::load(&root).expect("graph inputs");
        let built = crate::graph::verify::build(&inputs).expect("a buildable graph");
        check_live_target_agreement(&built.live_targets, &registry)
            .expect("the graph and the registry must derive the same live set");
    }
}
