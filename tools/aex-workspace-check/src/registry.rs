//! The derived test registry and its rules.
//!
//! Every package declares what it owes; this module joins those declarations
//! against the policy documents, the manifest graph and the tree, and fails when
//! a declaration is absent, outside the schema, below its role's floor, or
//! points at something that does not exist.
//!
//! # The one thing this module is careful about
//!
//! Nothing is deployed in this rewrite (`OD-07`) and most packages are still
//! skeletons, so most evidence genuinely cannot exist yet. That is not a reason
//! to pass quietly. A package with no test target must say so with
//! `not_applicable.targets` naming its owning stream, which lands it in
//! `release/unearned-evidence.json` as an `awaiting_owner` row. A package that
//! says nothing fails. *Awaiting owner* and *silently omitted* are different
//! states and the registry keeps them different.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::policy::Policy;
use crate::rules::{Violation, is_test_support_crate};
use crate::testmeta::{AexMeta, Subject};

/// Which phase the workspace is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Source is being written; no plane exists, so no live receipt can be
    /// earned. Unearned rows are recorded, not failed.
    SourceRewrite,
    /// A release candidate exists; the same rows become blocking.
    Candidate,
}

/// Where a package's manifest lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageKind {
    /// A Cargo workspace member.
    Cargo,
    /// An npm workspace package or a retained `TypeScript` package.
    Npm,
}

/// One package as the registry sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct PackageRow {
    /// Workspace-relative directory, with forward slashes.
    pub path: String,
    /// The package name.
    pub name: String,
    /// Cargo or npm.
    pub kind: PackageKind,
    /// The raw `aex` block, if the manifest declares one.
    pub raw_meta: Option<serde_json::Value>,
    /// Declared integration-test and bench target names.
    pub test_targets: Vec<String>,
    /// Declared Cargo features.
    pub features: Vec<String>,
    /// Normal (non-dev, non-build) dependency names.
    pub normal_dependencies: Vec<String>,
}

impl PackageRow {
    /// The subject used in failure messages.
    #[must_use]
    pub fn subject(&self) -> Subject {
        Subject {
            path: self.path.clone(),
            name: self.name.clone(),
        }
    }
}

/// One declared load workload, as read from `tests/load/workloads`.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkloadRow {
    /// The descriptor path.
    pub path: String,
    /// The declared workload id.
    pub id: String,
    /// The declared owner.
    pub owner: String,
    /// The live package that owns the `load` target.
    pub target: String,
    /// The gate ids the descriptor implements.
    pub gates: Vec<String>,
}

/// Which optional authority documents are present.
///
/// A rule whose authority is absent is recorded as pending, never as a pass:
/// "no route registry exists yet" and "every route has an owner" must not look
/// the same in the report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Authorities {
    /// `api/generated/registries/routes.json`.
    pub routes: Option<Vec<String>>,
    /// `release/scenario-ownership.toml`.
    pub scenario_owners: Option<Vec<String>>,
    /// `migrations/central/*.sql` and the regional generation bundle.
    pub migrations: Option<Vec<String>>,
}

/// Findings from scanning the tree, which the registry reports but does not
/// perform.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceFindings {
    /// Files carrying a container image literal outside the harness.
    pub image_literals: Vec<String>,
    /// Quarantine or expected-failure files that exist.
    pub quarantine_files: Vec<String>,
}

/// Everything the rules read.
#[derive(Debug)]
pub struct RegistryInput<'a> {
    /// Every package, Cargo and npm.
    pub packages: Vec<PackageRow>,
    /// The embedded policy.
    pub policy: &'a Policy,
    /// Declared load workloads.
    pub workloads: Vec<WorkloadRow>,
    /// Which optional authorities exist.
    pub authorities: Authorities,
    /// What the tree scan found.
    pub source: SourceFindings,
    /// Collected case counts per nextest binary id, when a list was supplied.
    pub collected: Option<BTreeMap<String, usize>>,
    /// The workspace phase.
    pub phase: Phase,
}

/// One row of `release/unearned-evidence.json`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct UnearnedRow {
    /// Why the row exists.
    pub reason_class: String,
    /// The package, seam, gate or rule the row is about.
    pub subject: String,
    /// The stream that owes it.
    pub owner: String,
    /// The rule that will fail once the phase is `candidate`.
    pub blocking_rule: String,
    /// What is missing, in words.
    pub detail: String,
}

/// The emitted registry document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegistryDocument {
    /// The document's schema id.
    pub schema: String,
    /// The phase it was built in.
    pub phase: Phase,
    /// Which inputs were available.
    pub inputs: BTreeMap<String, String>,
    /// Aggregate counts.
    pub totals: BTreeMap<String, usize>,
    /// One entry per package, keyed by path.
    pub packages: BTreeMap<String, RegistryPackage>,
    /// The live target set, derived from `live_suite` declarations.
    pub live_targets: Vec<String>,
    /// Seam id to the live companions that claim it.
    pub seam_claims: BTreeMap<String, Vec<String>>,
}

/// One package's registry entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegistryPackage {
    /// The package name.
    pub name: String,
    /// Cargo or npm.
    pub kind: PackageKind,
    /// Its declared ownership metadata.
    pub meta: AexMeta,
    /// Declared test targets that carry no `targets` mapping.
    pub unmapped_targets: Vec<String>,
    /// Collected case counts per declared target, when a list was supplied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collected: Option<BTreeMap<String, usize>>,
    /// Whether the package is waiting for its owning stream to write its tests.
    pub awaiting_owner: bool,
}

/// The result of running every registry rule.
#[derive(Debug, Clone, PartialEq)]
pub struct RegistryReport {
    /// Rules that failed.
    pub violations: Vec<Violation>,
    /// Evidence that cannot be earned yet.
    pub unearned: Vec<UnearnedRow>,
}

/// Cargo feature names no package may declare.
pub const BANNED_FEATURES: &[&str] = &[
    "fault",
    "fault-injection",
    "test-hooks",
    "chaos",
    "debug-endpoints",
];

/// Runs every registry rule.
#[must_use]
pub fn check(input: &RegistryInput<'_>) -> RegistryReport {
    let mut violations = Vec::new();
    let mut unearned = Vec::new();
    let mut parsed: BTreeMap<String, AexMeta> = BTreeMap::new();

    for package in &input.packages {
        let subject = package.subject();
        let Some(raw) = &package.raw_meta else {
            violations.push(Violation {
                rule: "aex-metadata-missing",
                detail: format!(
                    "`{}` has no {} table; every member declares its test ownership",
                    subject.path,
                    match package.kind {
                        PackageKind::Cargo => "[package.metadata.aex]",
                        PackageKind::Npm => "\"aex\"",
                    }
                ),
            });
            continue;
        };
        let (meta, issues) = AexMeta::parse(&subject, raw, input.policy);
        violations.extend(issues);
        if let Some(meta) = meta {
            parsed.insert(package.path.clone(), meta);
        }
    }

    violations.extend(role_profiles(input, &parsed));
    violations.extend(targets(input, &parsed));
    violations.extend(live_suites(input, &parsed));
    violations.extend(seams(input, &parsed));
    violations.extend(evidence_classes(input, &parsed));
    violations.extend(test_support_closure(input, &parsed));
    violations.extend(banned_features(input));
    violations.extend(workloads(input, &parsed));
    violations.extend(source_findings(input));

    unearned.extend(unearned_rows(input, &parsed));
    violations.sort();
    violations.dedup();
    unearned.sort();
    unearned.dedup();

    if input.phase == Phase::Candidate {
        for row in &unearned {
            violations.push(Violation {
                rule: "aex-unearned-evidence",
                detail: format!(
                    "`{}` still owes {} ({}); the candidate phase admits no unearned evidence",
                    row.subject, row.blocking_rule, row.detail
                ),
            });
        }
        violations.sort();
        violations.dedup();
    }

    RegistryReport {
        violations,
        unearned,
    }
}

fn role_profiles(input: &RegistryInput<'_>, parsed: &BTreeMap<String, AexMeta>) -> Vec<Violation> {
    let mut violations = Vec::new();
    for (path, meta) in parsed {
        let Some(profile) = input.policy.role(&meta.role) else {
            continue;
        };
        for layer in &profile.layers {
            if meta.declares_layer(layer) || meta.excused(layer).is_some() {
                continue;
            }
            violations.push(Violation {
                rule: "aex-role-profile-shortfall",
                detail: format!(
                    "`{path}` is role `{}` and must declare layer `{layer}`; declared: {}",
                    meta.role,
                    join_or_none(&meta.layers)
                ),
            });
        }
        for concern in &profile.concerns {
            if meta.declares_concern(concern) {
                continue;
            }
            violations.push(Violation {
                rule: "aex-role-profile-shortfall",
                detail: format!(
                    "`{path}` is role `{}` and must declare concern `{concern}`; declared: {}",
                    meta.role,
                    join_or_none(&meta.concerns)
                ),
            });
        }
        for field in &profile.forbid_not_applicable {
            if meta.excused(field).is_none() {
                continue;
            }
            violations.push(Violation {
                rule: "aex-not-applicable-unjustified",
                detail: format!(
                    "`{path}` cannot mark `{field}` not-applicable; {}",
                    profile
                        .forbid_reason
                        .as_deref()
                        .unwrap_or("the role makes it structurally true")
                ),
            });
        }
        if let Some(reason) = meta.excused("targets")
            && !reason.contains(&meta.owner)
        {
            violations.push(Violation {
                rule: "aex-not-applicable-unjustified",
                detail: format!(
                    "`{path}` marks `targets` not-applicable without naming its owner `{}`; \"not yet written\" is not a structural reason",
                    meta.owner
                ),
            });
        }
    }
    violations
}

fn targets(input: &RegistryInput<'_>, parsed: &BTreeMap<String, AexMeta>) -> Vec<Violation> {
    let mut violations = Vec::new();
    for package in &input.packages {
        let Some(meta) = parsed.get(&package.path) else {
            continue;
        };
        let awaiting = meta.excused("targets").is_some();

        if meta.targets.is_empty() && !awaiting {
            violations.push(Violation {
                rule: "aex-empty-unit",
                detail: format!(
                    "`{}` declares no [package.metadata.aex.targets] row and no not_applicable.targets reason; an unwritten suite must name the stream that owes it",
                    package.path
                ),
            });
        }
        if awaiting && !meta.targets.is_empty() {
            violations.push(Violation {
                rule: "aex-not-applicable-unjustified",
                detail: format!(
                    "`{}` marks `targets` not-applicable and then declares {} of them",
                    package.path,
                    meta.targets.len()
                ),
            });
        }

        for name in meta.targets.keys() {
            if matches!(name.as_str(), "load" | "soak") && meta.role != "live_companion" {
                violations.push(Violation {
                    rule: "aex-load-target-misplaced",
                    detail: format!(
                        "`{}` declares target `{name}`, which the unit lane's default-filter does not exclude; D-11 puts every load and soak executor inside a live companion",
                        package.path
                    ),
                });
            }
            if package.kind == PackageKind::Cargo
                && !package.test_targets.iter().any(|target| target == name)
            {
                violations.push(Violation {
                    rule: "aex-target-unknown",
                    detail: format!(
                        "`{}` maps target `{name}` to a layer but declares no [[test]] target named `{name}`",
                        package.path
                    ),
                });
            }
        }
        for name in &package.test_targets {
            if !meta.targets.contains_key(name.as_str()) {
                violations.push(Violation {
                    rule: "aex-target-unmapped",
                    detail: format!(
                        "`{}` has [[test]] target `{name}` mapped to no layer; every target's layer is a manifest fact",
                        package.path
                    ),
                });
            }
        }

        if !awaiting {
            violations.extend(concern_and_layer_coverage(input, &package.path, meta));
        }

        if let Some(collected) = &input.collected {
            for (name, layer) in &meta.targets {
                let binary_id = format!("{}::{name}", package.name);
                let cases = collected.get(&binary_id).copied().unwrap_or(0);
                if cases == 0 {
                    violations.push(Violation {
                        rule: "aex-empty-unit",
                        detail: format!(
                            "`{}` target `{name}` declares layer `{layer}` but collects 0 tests",
                            package.path
                        ),
                    });
                }
            }
        }
    }
    violations
}

/// Every declared concern has a target at the layer that carries it, and every
/// locally-runnable declared layer has a target or a stated reason.
fn concern_and_layer_coverage(
    input: &RegistryInput<'_>,
    path: &str,
    meta: &AexMeta,
) -> Vec<Violation> {
    let mut violations = Vec::new();
    for concern in &meta.concerns {
        let Some(rule) = input.policy.concerns.get(concern) else {
            continue;
        };
        if !meta.declares_layer(&rule.requires_layer)
            || meta
                .targets
                .values()
                .any(|layer| layer == &rule.requires_layer)
        {
            continue;
        }
        let named = rule
            .conventional_target
            .as_ref()
            .map_or_else(String::new, |name| {
                format!(" (conventionally named `{name}`)")
            });
        violations.push(Violation {
            rule: "aex-target-missing",
            detail: format!(
                "`{path}` declares concern `{concern}` but has no [[test]] target mapped to layer `{}`{named}",
                rule.requires_layer
            ),
        });
    }
    for layer in &meta.layers {
        // `smoke`, `e2e` and `user` evidence lives in the live companion or in
        // a packed artifact, not in this package's own targets.
        if !matches!(layer.as_str(), "unit" | "integration")
            || meta.targets.values().any(|declared| declared == layer)
            || meta.excused(layer).is_some()
        {
            continue;
        }
        violations.push(Violation {
            rule: "aex-layer-uncovered",
            detail: format!(
                "`{path}` declares layer `{layer}` but maps no target to it and gives no not_applicable.{layer} reason"
            ),
        });
    }
    violations
}

fn live_suites(input: &RegistryInput<'_>, parsed: &BTreeMap<String, AexMeta>) -> Vec<Violation> {
    let mut violations = Vec::new();
    let companions: BTreeMap<&str, (&String, &AexMeta)> = parsed
        .iter()
        .filter(|(_, meta)| meta.role == "live_companion")
        .filter_map(|(path, meta)| {
            input
                .packages
                .iter()
                .find(|package| &package.path == path)
                .map(|package| (package.name.as_str(), (path, meta)))
        })
        .collect();
    let deployables: BTreeSet<&str> = parsed
        .values()
        .filter(|meta| {
            matches!(
                meta.role.as_str(),
                "deployable" | "runtime_image" | "web_app" | "client"
            )
        })
        .filter_map(|meta| meta.deployable.as_deref())
        .collect();
    let mut claimed: BTreeSet<&str> = BTreeSet::new();

    for (path, meta) in parsed {
        if let Some(suite) = &meta.live_suite {
            claimed.insert(suite.as_str());
            if !companions.contains_key(suite.as_str()) {
                violations.push(Violation {
                    rule: "aex-unknown-live-suite",
                    detail: format!(
                        "`{path}` names live_suite `{suite}`, which is not a live companion package"
                    ),
                });
            }
        }
        if matches!(meta.role.as_str(), "deployable" | "runtime_image")
            && meta.live_suite.is_none()
            && meta.excused("live_suite").is_none()
        {
            violations.push(Violation {
                rule: "aex-uncovered-deployable",
                detail: format!("`{path}` declares no live_suite and no not-applicable reason"),
            });
        }
    }

    for (name, (path, meta)) in &companions {
        match meta.deployable.as_deref() {
            Some(deployable) if !deployables.contains(deployable) => {
                if meta.excused("deployable").is_none() {
                    violations.push(Violation {
                        rule: "aex-orphan-companion",
                        detail: format!(
                            "`{path}` names deployable `{deployable}`, which is not a workspace member"
                        ),
                    });
                }
            }
            Some(_) => {}
            None => {
                if meta.excused("deployable").is_none() {
                    violations.push(Violation {
                        rule: "aex-orphan-companion",
                        detail: format!(
                            "`{path}` names no deployable and no not-applicable reason; a companion with no subject observes nothing"
                        ),
                    });
                }
            }
        }
        if !claimed.contains(name) && meta.excused("deployable").is_none() {
            violations.push(Violation {
                rule: "aex-live-target-underived",
                detail: format!(
                    "`{path}` is named by no live_suite declaration; the live target set is derived from live_suite, never hand-listed"
                ),
            });
        }
    }
    violations
}

fn seams(input: &RegistryInput<'_>, parsed: &BTreeMap<String, AexMeta>) -> Vec<Violation> {
    let mut violations = Vec::new();
    let by_name: BTreeMap<&str, &AexMeta> = input
        .packages
        .iter()
        .filter_map(|package| {
            parsed
                .get(&package.path)
                .map(|meta| (package.name.as_str(), meta))
        })
        .collect();

    for package in &input.packages {
        let Some(meta) = parsed.get(&package.path) else {
            continue;
        };
        for seam in &meta.seams {
            let Some(declared) = input.policy.seams.get(seam) else {
                violations.push(Violation {
                    rule: "aex-unknown-seam",
                    detail: format!(
                        "`{}` declares seam `{seam}`, which is not in release/policy/seams.toml",
                        package.path
                    ),
                });
                continue;
            };
            if !declared.requires_live || meta.role == "live_companion" {
                continue;
            }
            if meta.excused("live_suite").is_some() {
                // The package states, structurally, that it owns no companion.
                // The seam is then a recorded gap in `unearned-evidence.json`
                // rather than a violation - that is exactly D-18's open item
                // for the SDK and the CLI - but it can never be silent.
                continue;
            }
            let claimed = meta
                .live_suite
                .as_deref()
                .and_then(|suite| by_name.get(suite))
                .is_some_and(|companion| companion.seams.iter().any(|claim| claim == seam));
            if !claimed {
                violations.push(Violation {
                    rule: "aex-unclaimed-seam",
                    detail: format!(
                        "seam `{seam}` is declared by `{}` but no live companion claims it",
                        package.name
                    ),
                });
            }
        }
    }
    violations
}

fn evidence_classes(
    input: &RegistryInput<'_>,
    parsed: &BTreeMap<String, AexMeta>,
) -> Vec<Violation> {
    let by_name: BTreeMap<&str, &AexMeta> = input
        .packages
        .iter()
        .filter_map(|package| {
            parsed
                .get(&package.path)
                .map(|meta| (package.name.as_str(), meta))
        })
        .collect();
    input
        .policy
        .evidence_classes
        .iter()
        .filter_map(
            |(id, class)| match by_name.get(class.owner_package.as_str()) {
                None => Some(Violation {
                    rule: "aex-missing-evidence-class",
                    detail: format!(
                        "`{}` owes evidence class `{id}` (Area 9) but is not a declared package",
                        class.owner_package
                    ),
                }),
                Some(meta) if !meta.declares_concern(&class.requires_concern) => Some(Violation {
                    rule: "aex-missing-evidence-class",
                    detail: format!(
                        "`{}` owes evidence class `{id}` (Area 9); no target declares concern `{}`",
                        class.owner_package, class.requires_concern
                    ),
                }),
                Some(_) => None,
            },
        )
        .collect()
}

fn test_support_closure(
    input: &RegistryInput<'_>,
    parsed: &BTreeMap<String, AexMeta>,
) -> Vec<Violation> {
    let edges: BTreeMap<&str, &Vec<String>> = input
        .packages
        .iter()
        .map(|package| (package.name.as_str(), &package.normal_dependencies))
        .collect();
    let mut violations = Vec::new();
    for package in &input.packages {
        let role = parsed.get(&package.path).map(|meta| meta.role.as_str());
        if is_test_support_crate(&package.name)
            || matches!(role, Some("test_support" | "live_companion"))
        {
            continue;
        }
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut stack: Vec<&str> = package
            .normal_dependencies
            .iter()
            .map(String::as_str)
            .collect();
        while let Some(next) = stack.pop() {
            if !seen.insert(next) {
                continue;
            }
            if is_test_support_crate(next) {
                violations.push(Violation {
                    rule: "aex-test-support-in-production",
                    detail: format!("`{}` links `{next}` as a normal dependency", package.path),
                });
                continue;
            }
            if let Some(next_edges) = edges.get(next) {
                stack.extend(next_edges.iter().map(String::as_str));
            }
        }
    }
    violations
}

fn banned_features(input: &RegistryInput<'_>) -> Vec<Violation> {
    let mut violations = Vec::new();
    for package in &input.packages {
        for feature in &package.features {
            if BANNED_FEATURES.contains(&feature.as_str()) || feature.starts_with("bypass-") {
                violations.push(Violation {
                    rule: "aex-banned-feature",
                    detail: format!(
                        "`{}` declares feature `{feature}`; no package may carry a fault, test-hook, chaos, debug-endpoint or bypass feature",
                        package.path
                    ),
                });
            }
        }
    }
    violations
}

fn workloads(input: &RegistryInput<'_>, parsed: &BTreeMap<String, AexMeta>) -> Vec<Violation> {
    let names: BTreeSet<&str> = input
        .packages
        .iter()
        .map(|package| package.name.as_str())
        .collect();
    let mut violations = Vec::new();
    for workload in &input.workloads {
        if !names.contains(workload.target.as_str()) {
            violations.push(Violation {
                rule: "aex-workload-unowned",
                detail: format!(
                    "workload `{}` names owner package `{}`, which does not exist",
                    workload.id, workload.target
                ),
            });
        }
        if !input.policy.values.owner.contains(&workload.owner) {
            violations.push(Violation {
                rule: "aex-workload-unowned",
                detail: format!(
                    "workload `{}` names owner `{}`, which is not a declared stream",
                    workload.id, workload.owner
                ),
            });
        }
        for gate in &workload.gates {
            if !input.policy.gates.contains_key(gate) {
                violations.push(Violation {
                    rule: "aex-workload-unowned",
                    detail: format!(
                        "workload `{}` implements gate `{gate}`, which is not in release/policy/workload-registry.toml",
                        workload.id
                    ),
                });
            }
        }
    }
    let _ = parsed;
    violations
}

fn source_findings(input: &RegistryInput<'_>) -> Vec<Violation> {
    let mut violations = Vec::new();
    for path in &input.source.image_literals {
        violations.push(Violation {
            rule: "data-image-literal",
            detail: format!(
                "`{path}` names a container image directly; call aex_test_harness::images so the digest pin is the only reference"
            ),
        });
    }
    for path in &input.source.quarantine_files {
        violations.push(Violation {
            rule: "flake-quarantine-file",
            detail: format!("`{path}` exists; Q-FLAKE removes all release exemptions"),
        });
    }
    violations
}

fn unearned_rows(
    input: &RegistryInput<'_>,
    parsed: &BTreeMap<String, AexMeta>,
) -> Vec<UnearnedRow> {
    let mut rows = Vec::new();
    for package in &input.packages {
        let Some(meta) = parsed.get(&package.path) else {
            continue;
        };
        if let Some(reason) = meta.excused("targets") {
            rows.push(UnearnedRow {
                reason_class: "awaiting_owner".to_owned(),
                subject: package.path.clone(),
                owner: meta.owner.clone(),
                blocking_rule: "aex-empty-unit".to_owned(),
                detail: reason.to_owned(),
            });
        }
        if let Some(reason) = meta.excused("deployable") {
            rows.push(UnearnedRow {
                reason_class: "awaiting_owner".to_owned(),
                subject: package.path.clone(),
                owner: meta.owner.clone(),
                blocking_rule: "aex-orphan-companion".to_owned(),
                detail: reason.to_owned(),
            });
        }
        for layer in &meta.layers {
            if matches!(layer.as_str(), "smoke" | "e2e" | "user") {
                rows.push(UnearnedRow {
                    reason_class: "requires_deployment".to_owned(),
                    subject: package.path.clone(),
                    owner: meta.owner.clone(),
                    blocking_rule: "aex-unearned-evidence".to_owned(),
                    detail: format!("layer `{layer}` needs a deployed artifact (OD-07)"),
                });
            }
        }
        for seam in &meta.seams {
            if input
                .policy
                .seams
                .get(seam)
                .is_some_and(|declared| declared.requires_live)
            {
                rows.push(UnearnedRow {
                    reason_class: "requires_live_seam".to_owned(),
                    subject: package.path.clone(),
                    owner: meta.owner.clone(),
                    blocking_rule: "aex-unearned-evidence".to_owned(),
                    detail: format!("seam `{seam}` cannot be satisfied by any local substitute"),
                });
            }
        }
    }
    for (id, gate) in &input.policy.gates {
        if !input
            .workloads
            .iter()
            .any(|workload| workload.gates.iter().any(|declared| declared == id))
        {
            rows.push(UnearnedRow {
                reason_class: "pending_workload".to_owned(),
                subject: id.clone(),
                owner: gate.owner.clone(),
                blocking_rule: "aex-workload-unowned".to_owned(),
                detail: format!(
                    "no descriptor under tests/load/workloads implements this gate on `{}`",
                    gate.target
                ),
            });
        }
    }
    if input.authorities.routes.is_none() {
        rows.push(pending_authority(
            "api/generated/registries/routes.json",
            "contracts",
            "aex-route-uncovered",
        ));
    }
    if input.authorities.scenario_owners.is_none() {
        rows.push(pending_authority(
            "release/scenario-ownership.toml",
            "delivery",
            "aex-scenario-orphan",
        ));
    }
    if input.authorities.migrations.is_none() {
        rows.push(pending_authority(
            "migrations/central/*.sql",
            "central-finance",
            "aex-migration-uncovered",
        ));
    }
    if input.collected.is_none() {
        rows.push(pending_authority(
            "cargo nextest list --message-format json",
            "test-architecture",
            "aex-empty-unit",
        ));
    }
    rows
}

fn pending_authority(subject: &str, owner: &str, rule: &str) -> UnearnedRow {
    UnearnedRow {
        reason_class: "pending_authority".to_owned(),
        subject: subject.to_owned(),
        owner: owner.to_owned(),
        blocking_rule: rule.to_owned(),
        detail: "the authority this rule reads does not exist yet, so the rule is recorded rather than evaluated".to_owned(),
    }
}

fn join_or_none(values: &[String]) -> String {
    if values.is_empty() {
        "none".to_owned()
    } else {
        values.join(", ")
    }
}

/// Builds the registry document.
#[must_use]
pub fn build(input: &RegistryInput<'_>) -> RegistryDocument {
    let mut packages = BTreeMap::new();
    let mut live_targets = BTreeSet::new();
    let mut seam_claims: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut awaiting = 0_usize;

    for package in &input.packages {
        let Some(raw) = &package.raw_meta else {
            continue;
        };
        let (Some(meta), _) = AexMeta::parse(&package.subject(), raw, input.policy) else {
            continue;
        };
        if let Some(suite) = &meta.live_suite {
            live_targets.insert(suite.clone());
        }
        if meta.role == "live_companion" {
            for seam in &meta.seams {
                seam_claims
                    .entry(seam.clone())
                    .or_default()
                    .push(package.name.clone());
            }
        }
        let is_awaiting = meta.excused("targets").is_some();
        if is_awaiting {
            awaiting += 1;
        }
        let unmapped = package
            .test_targets
            .iter()
            .filter(|name| !meta.targets.contains_key(name.as_str()))
            .cloned()
            .collect();
        let collected = input.collected.as_ref().map(|counts| {
            meta.targets
                .keys()
                .map(|name| {
                    let binary_id = format!("{}::{name}", package.name);
                    (name.clone(), counts.get(&binary_id).copied().unwrap_or(0))
                })
                .collect()
        });
        packages.insert(
            package.path.clone(),
            RegistryPackage {
                name: package.name.clone(),
                kind: package.kind,
                meta,
                unmapped_targets: unmapped,
                collected,
                awaiting_owner: is_awaiting,
            },
        );
    }

    let mut totals = BTreeMap::new();
    totals.insert("packages".to_owned(), packages.len());
    totals.insert("awaiting_owner".to_owned(), awaiting);
    totals.insert("live_targets".to_owned(), live_targets.len());
    totals.insert("seams_declared".to_owned(), input.policy.seams.len());
    totals.insert("seams_claimed".to_owned(), seam_claims.len());
    totals.insert("workloads".to_owned(), input.workloads.len());
    totals.insert("capacity_gates".to_owned(), input.policy.gates.len());

    let mut inputs = BTreeMap::new();
    inputs.insert(
        "nextest_list".to_owned(),
        present(input.collected.is_some()),
    );
    inputs.insert(
        "routes_registry".to_owned(),
        present(input.authorities.routes.is_some()),
    );
    inputs.insert(
        "scenario_ownership".to_owned(),
        present(input.authorities.scenario_owners.is_some()),
    );
    inputs.insert(
        "migrations".to_owned(),
        present(input.authorities.migrations.is_some()),
    );

    RegistryDocument {
        schema: "aex.test-registry.v1".to_owned(),
        phase: input.phase,
        inputs,
        totals,
        packages,
        live_targets: live_targets.into_iter().collect(),
        seam_claims,
    }
}

fn present(available: bool) -> String {
    if available { "present" } else { "absent" }.to_owned()
}

#[cfg(test)]
mod tests {
    use super::{
        Authorities, PackageKind, PackageRow, Phase, RegistryInput, SourceFindings, WorkloadRow,
        check,
    };
    use crate::policy::Policy;
    use std::collections::BTreeMap;

    fn row(path: &str, name: &str, meta: Option<&str>) -> PackageRow {
        PackageRow {
            path: path.to_owned(),
            name: name.to_owned(),
            kind: PackageKind::Cargo,
            raw_meta: meta.map(|text| serde_json::from_str(text).expect("the fixture parses")),
            test_targets: Vec::new(),
            features: Vec::new(),
            normal_dependencies: Vec::new(),
        }
    }

    fn input(packages: Vec<PackageRow>, policy: &Policy) -> RegistryInput<'_> {
        RegistryInput {
            packages,
            policy,
            workloads: Vec::new(),
            authorities: Authorities::default(),
            source: SourceFindings::default(),
            collected: None,
            phase: Phase::SourceRewrite,
        }
    }

    fn details(report: &super::RegistryReport, rule: &str) -> Vec<String> {
        report
            .violations
            .iter()
            .filter(|violation| violation.rule == rule)
            .map(|violation| violation.detail.clone())
            .collect()
    }

    const DOMAIN: &str = r#"{ "owner": "regional-domains", "role": "domain", "artifact": "none",
        "layers": ["unit"], "concerns": ["property"], "seams": [],
        "security_tier": "authority", "risk": ["concurrency"], "scenarios": [],
        "not_applicable": { "targets": "awaiting the regional-domains stream",
                            "live_suite": "pure domain crate; no deployed seam" } }"#;

    #[test]
    fn a_package_with_no_declared_evidence_fails_with_the_declared_message() {
        let policy = Policy::embedded();
        let report = check(&input(vec![row("crates/aex-foo", "aex-foo", None)], policy));
        assert_eq!(
            details(&report, "aex-metadata-missing"),
            vec![
                "`crates/aex-foo` has no [package.metadata.aex] table; every member declares its test ownership"
            ]
        );
    }

    #[test]
    fn an_npm_package_with_no_declared_evidence_names_the_npm_key() {
        let policy = Policy::embedded();
        let mut package = row("packages/sdk", "@aexhq/sdk", None);
        package.kind = PackageKind::Npm;
        let report = check(&input(vec![package], policy));
        assert_eq!(
            details(&report, "aex-metadata-missing"),
            vec!["`packages/sdk` has no \"aex\" table; every member declares its test ownership"]
        );
    }

    #[test]
    fn a_role_below_its_profile_names_the_missing_concern_and_what_was_declared() {
        let policy = Policy::embedded();
        let adapter = r#"{ "owner": "regional-stores", "role": "adapter", "artifact": "none",
            "live_suite": "aex-live-x", "layers": ["unit", "integration"],
            "concerns": ["contract", "property", "security"], "seams": [],
            "security_tier": "authority", "risk": ["none"], "scenarios": [],
            "not_applicable": { "targets": "awaiting the regional-stores stream" } }"#;
        let report = check(&input(
            vec![row(
                "crates/aex-content-aws",
                "aex-content-aws",
                Some(adapter),
            )],
            policy,
        ));
        assert_eq!(
            details(&report, "aex-role-profile-shortfall"),
            vec![
                "`crates/aex-content-aws` is role `adapter` and must declare concern `fault`; declared: contract, property, security"
            ]
        );
    }

    #[test]
    fn an_orphaned_live_companion_names_the_deployable_that_does_not_exist() {
        let policy = Policy::embedded();
        let companion = r#"{ "owner": "regional-services", "role": "live_companion",
            "artifact": "none", "deployable": "ghost", "layers": ["smoke", "e2e"],
            "concerns": ["fault", "security", "performance"], "seams": [],
            "security_tier": "public_edge", "risk": ["iam"], "scenarios": [],
            "not_applicable": { "targets": "awaiting the regional-services stream" } }"#;
        let report = check(&input(
            vec![row(
                "tests/live/aex-live-ghost",
                "aex-live-ghost",
                Some(companion),
            )],
            policy,
        ));
        assert_eq!(
            details(&report, "aex-orphan-companion"),
            vec![
                "`tests/live/aex-live-ghost` names deployable `ghost`, which is not a workspace member"
            ]
        );
    }

    #[test]
    fn an_uncovered_deployable_is_reported() {
        let policy = Policy::embedded();
        let deployable = r#"{ "owner": "observations-usage", "role": "deployable",
            "artifact": "lambda_zip", "deployable": "usage-compute-worker",
            "layers": ["unit", "smoke", "e2e"],
            "concerns": ["contract", "fault", "security", "performance"], "seams": [],
            "security_tier": "internal", "risk": ["iam"], "scenarios": [],
            "not_applicable": { "targets": "awaiting the observations-usage stream" } }"#;
        let report = check(&input(
            vec![row(
                "workers/usage-compute-worker",
                "usage-compute-worker",
                Some(deployable),
            )],
            policy,
        ));
        assert_eq!(
            details(&report, "aex-uncovered-deployable"),
            vec![
                "`workers/usage-compute-worker` declares no live_suite and no not-applicable reason"
            ]
        );
    }

    #[test]
    fn a_declared_target_that_collects_no_case_is_an_empty_unit() {
        let policy = Policy::embedded();
        let meta = r#"{ "owner": "regional-domains", "role": "domain", "artifact": "none",
            "layers": ["unit"], "concerns": ["property"], "seams": [],
            "security_tier": "authority", "risk": ["none"], "scenarios": [],
            "targets": { "properties": "unit" },
            "not_applicable": { "live_suite": "pure domain crate; no deployed seam" } }"#;
        let mut package = row("crates/aex-foo", "aex-foo", Some(meta));
        package.test_targets = vec!["properties".to_owned()];
        let mut inputs = input(vec![package], policy);
        inputs.collected = Some(BTreeMap::from([("aex-foo::properties".to_owned(), 0)]));
        let report = check(&inputs);
        assert_eq!(
            details(&report, "aex-empty-unit"),
            vec!["`crates/aex-foo` target `properties` declares layer `unit` but collects 0 tests"]
        );
    }

    #[test]
    fn a_package_with_no_targets_and_no_reason_is_an_empty_unit() {
        let policy = Policy::embedded();
        let meta = r#"{ "owner": "regional-domains", "role": "domain", "artifact": "none",
            "layers": ["unit"], "concerns": ["property"], "seams": [],
            "security_tier": "authority", "risk": ["none"], "scenarios": [],
            "not_applicable": { "live_suite": "pure domain crate; no deployed seam" } }"#;
        let report = check(&input(
            vec![row("crates/aex-foo", "aex-foo", Some(meta))],
            policy,
        ));
        assert!(
            details(&report, "aex-empty-unit")[0].contains("no not_applicable.targets reason"),
            "{:?}",
            details(&report, "aex-empty-unit")
        );
    }

    #[test]
    fn awaiting_owner_must_name_the_owner_so_not_yet_written_is_not_a_reason() {
        let policy = Policy::embedded();
        let meta = DOMAIN.replace("awaiting the regional-domains stream", "not yet written");
        let report = check(&input(
            vec![row("crates/aex-foo", "aex-foo", Some(&meta))],
            policy,
        ));
        assert_eq!(
            details(&report, "aex-not-applicable-unjustified"),
            vec![
                "`crates/aex-foo` marks `targets` not-applicable without naming its owner `regional-domains`; \"not yet written\" is not a structural reason"
            ]
        );
    }

    #[test]
    fn awaiting_owner_is_recorded_as_unearned_rather_than_failed() {
        let policy = Policy::embedded();
        let report = check(&input(
            vec![row("crates/aex-foo", "aex-foo", Some(DOMAIN))],
            policy,
        ));
        assert!(
            details(&report, "aex-empty-unit").is_empty(),
            "{:?}",
            report.violations
        );
        assert!(
            report
                .unearned
                .iter()
                .any(|row| row.subject == "crates/aex-foo" && row.reason_class == "awaiting_owner")
        );
    }

    #[test]
    fn the_candidate_phase_turns_every_unearned_row_into_a_failure() {
        let policy = Policy::embedded();
        let mut inputs = input(vec![row("crates/aex-foo", "aex-foo", Some(DOMAIN))], policy);
        inputs.phase = Phase::Candidate;
        let report = check(&inputs);
        assert!(!details(&report, "aex-unearned-evidence").is_empty());
    }

    #[test]
    fn an_unknown_seam_is_rejected_and_a_requires_live_seam_needs_a_claiming_companion() {
        let policy = Policy::embedded();
        let adapter = r#"{ "owner": "regional-stores", "role": "adapter", "artifact": "none",
            "live_suite": "aex-live-x", "layers": ["unit", "integration"],
            "concerns": ["contract", "property", "fault", "security"],
            "seams": ["aws.firecracker.boot", "aws.dynamodb.streams"],
            "security_tier": "authority", "risk": ["none"], "scenarios": [],
            "not_applicable": { "targets": "awaiting the regional-stores stream" } }"#;
        let companion = r#"{ "owner": "regional-stores", "role": "live_companion",
            "artifact": "none", "deployable": "x", "layers": ["smoke", "e2e"],
            "concerns": ["fault", "security", "performance"], "seams": [],
            "security_tier": "authority", "risk": ["none"], "scenarios": [],
            "not_applicable": { "targets": "awaiting the regional-stores stream" } }"#;
        let deployable = r#"{ "owner": "regional-stores", "role": "deployable",
            "artifact": "lambda_zip", "deployable": "x", "live_suite": "aex-live-x",
            "layers": ["unit", "smoke", "e2e"],
            "concerns": ["contract", "fault", "security", "performance"], "seams": [],
            "security_tier": "authority", "risk": ["none"], "scenarios": [],
            "not_applicable": { "targets": "awaiting the regional-stores stream" } }"#;
        let report = check(&input(
            vec![
                row(
                    "crates/aex-session-dynamodb",
                    "aex-session-dynamodb",
                    Some(adapter),
                ),
                row("tests/live/aex-live-x", "aex-live-x", Some(companion)),
                row("services/x", "x", Some(deployable)),
            ],
            policy,
        ));
        assert_eq!(
            details(&report, "aex-unknown-seam"),
            vec![
                "`crates/aex-session-dynamodb` declares seam `aws.firecracker.boot`, which is not in release/policy/seams.toml"
            ]
        );
        assert_eq!(
            details(&report, "aex-unclaimed-seam"),
            vec![
                "seam `aws.dynamodb.streams` is declared by `aex-session-dynamodb` but no live companion claims it"
            ]
        );
    }

    #[test]
    fn a_banned_feature_is_rejected() {
        let policy = Policy::embedded();
        let mut package = row("runtimes/brain-mux", "brain-mux", Some(DOMAIN));
        package.features = vec!["chaos".to_owned(), "bypass-authz".to_owned()];
        let report = check(&input(vec![package], policy));
        assert_eq!(details(&report, "aex-banned-feature").len(), 2);
    }

    #[test]
    fn a_transitive_normal_dependency_on_test_support_is_reported() {
        let policy = Policy::embedded();
        let mut deployable = row("runtimes/brain-mux", "brain-mux", Some(DOMAIN));
        deployable.normal_dependencies = vec!["aex-brain-app".to_owned()];
        let mut middle = row(
            "crates/aex-brain-app",
            "aex-brain-app",
            Some(DOMAIN),
        );
        middle.normal_dependencies = vec!["aex-brain-test-support".to_owned()];
        let report = check(&input(vec![deployable, middle], policy));
        assert!(
            details(&report, "aex-test-support-in-production").contains(
                &"`runtimes/brain-mux` links `aex-brain-test-support` as a normal dependency"
                    .to_owned()
            ),
            "the closure must reach through the intermediate crate: {:?}",
            details(&report, "aex-test-support-in-production")
        );
    }

    #[test]
    fn a_workload_naming_a_package_that_does_not_exist_is_reported() {
        let policy = Policy::embedded();
        let mut inputs = input(vec![row("crates/aex-foo", "aex-foo", Some(DOMAIN))], policy);
        inputs.workloads = vec![WorkloadRow {
            path: "tests/load/workloads/brain-core/brain-500-offered.toml".to_owned(),
            id: "brain-500-offered".to_owned(),
            owner: "brain-core".to_owned(),
            target: "brain-mux-load".to_owned(),
            gates: vec!["LOAD-500-BOUNDED".to_owned()],
        }];
        let report = check(&inputs);
        assert_eq!(
            details(&report, "aex-workload-unowned"),
            vec![
                "workload `brain-500-offered` names owner package `brain-mux-load`, which does not exist"
            ]
        );
    }

    #[test]
    fn a_quarantine_file_and_an_image_literal_are_reported() {
        let policy = Policy::embedded();
        let mut inputs = input(vec![row("crates/aex-foo", "aex-foo", Some(DOMAIN))], policy);
        inputs.source.quarantine_files = vec![".test-quarantine.json".to_owned()];
        inputs.source.image_literals =
            vec!["crates/aex-content-aws/tests/integration.rs".to_owned()];
        let report = check(&inputs);
        assert_eq!(
            details(&report, "flake-quarantine-file"),
            vec!["`.test-quarantine.json` exists; Q-FLAKE removes all release exemptions"]
        );
        assert_eq!(details(&report, "data-image-literal").len(), 1);
    }
}
