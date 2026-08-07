//! `graph build` and `graph verify`.
//!
//! Verification is fail-closed on every condition in plan 14 §2.4. The one
//! design point worth restating: an unowned repository path both widens the run
//! to repo-wide **and** fails verification. The predecessor implementation only
//! widened, so a file nobody owned ran everything forever and nobody found out.

use std::collections::{BTreeMap, BTreeSet};

use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::error::{Exit, Result, ToolError, Violation};

use super::inputs::GraphInputs;
use super::pathmap::Classification;
use super::{Graph, Node, NodeId, NodeKind};

/// The graph plus everything verification derived from it.
#[derive(Debug)]
pub struct BuiltGraph {
    /// The merged graph.
    pub graph: Graph,
    /// Live-test packages derived from `live_suite` declarations. This set is
    /// never hand-written: a hand-maintained global allowlist is exactly the
    /// drift the test architecture forbids.
    pub live_targets: BTreeSet<String>,
    /// Repository paths matching no `path-map.toml` rule.
    pub unowned: Vec<String>,
    /// Explicit architecture work that is not yet runnable or mounted.
    pub deferred: Vec<DeferredWork>,
}

/// One explicit, non-evidentiary delivery deferral.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeferredWork {
    /// `route` or `scenario`.
    pub kind: &'static str,
    /// Exact `operationId` or scenario id.
    pub id: String,
    /// Architectural reason authored at the source boundary.
    pub reason: String,
}

/// The machine-readable summary `graph build --json` prints.
#[derive(Debug, Serialize)]
pub struct GraphSummary {
    /// Schema discriminator.
    pub schema: &'static str,
    /// Node count by kind.
    pub nodes: BTreeMap<String, usize>,
    /// Total edge count.
    pub edges: usize,
    /// Derived live-test package names.
    pub live_targets: Vec<String>,
    /// Unowned repository paths.
    pub unowned: Vec<String>,
    /// Explicit architecture work excluded from runnable release matrices.
    pub deferred: Vec<DeferredWork>,
}

/// Build the merged graph from loaded inputs.
///
/// # Errors
/// Returns [`Exit::GraphVerification`] when the merge itself is impossible: a
/// duplicate node id, an edge naming an unknown node, or a cycle.
pub fn build(inputs: &GraphInputs) -> Result<BuiltGraph> {
    let mut nodes: Vec<Node> = Vec::new();
    for package in &inputs.cargo {
        nodes.push(Node {
            id: NodeId::cargo(&package.name),
            kind: NodeKind::Crate,
            dir: package.dir.clone(),
            meta: package.meta.clone(),
            publishable: package.publishable,
        });
    }
    for package in &inputs.npm {
        nodes.push(Node {
            id: NodeId::npm(&package.name),
            kind: NodeKind::NpmPackage,
            dir: package.dir.clone(),
            meta: package.meta.clone(),
            publishable: package.publishable,
        });
    }
    for unit in &inputs.terraform {
        nodes.push(Node {
            id: NodeId::terraform(&unit.relative),
            kind: if unit.is_root {
                NodeKind::TerraformRoot
            } else {
                NodeKind::TerraformModule
            },
            dir: unit.dir.clone(),
            meta: unit.meta.clone(),
            publishable: false,
        });
    }
    for unit in &inputs.units.units {
        nodes.push(Node {
            id: NodeId::artifact(&unit.id),
            kind: NodeKind::Artifact,
            dir: format!("release/units.toml#{}", unit.id),
            meta: None,
            publishable: true,
        });
    }
    for scenario in &inputs.scenarios.scenarios {
        nodes.push(Node {
            id: NodeId::scenario(&scenario.id),
            kind: NodeKind::Scenario,
            dir: format!("release/scenario-ownership.toml#{}", scenario.id),
            meta: None,
            publishable: false,
        });
    }
    for bundle in ["contract", "migration", "infra-modules"] {
        nodes.push(Node {
            id: NodeId::bundle(bundle),
            kind: NodeKind::Bundle,
            dir: format!("release/bundles/{bundle}"),
            meta: None,
            publishable: true,
        });
    }

    let npm_dirs = inputs.npm_dirs();
    let mut unowned = Vec::new();
    for file in &inputs.files {
        if matches!(
            inputs.path_map.classify(file, &npm_dirs),
            Classification::Orphan
        ) {
            unowned.push(file.clone());
        }
    }
    let live_targets = inputs
        .cargo
        .iter()
        .filter_map(|package| package.meta.as_ref().and_then(|m| m.live_suite.clone()))
        .chain(
            inputs
                .npm
                .iter()
                .filter_map(|package| package.meta.as_ref().and_then(|m| m.live_suite.clone())),
        )
        .chain(
            inputs
                .units
                .units
                .iter()
                .filter_map(|unit| unit.live_suite.clone()),
        )
        .collect();

    let graph = Graph::new(nodes, &inputs.edges())?;
    Ok(BuiltGraph {
        graph,
        live_targets,
        unowned,
        deferred: collect_deferred_work(inputs),
    })
}

fn collect_deferred_work(inputs: &GraphInputs) -> Vec<DeferredWork> {
    let mut deferred = inputs
        .scenarios
        .scenarios
        .iter()
        .filter_map(|scenario| {
            let reason = scenario.deferred.as_deref()?.trim();
            (!reason.is_empty()).then(|| DeferredWork {
                kind: "scenario",
                id: scenario.id.clone(),
                reason: reason.to_owned(),
            })
        })
        .collect::<Vec<_>>();
    let routes = inputs.root.join("api/generated/registries/routes.json");
    if let Ok(text) = std::fs::read_to_string(routes)
        && let Ok(document) = strict_json(&text)
        && let Some(entries) = document.get("routes").and_then(serde_json::Value::as_array)
    {
        deferred.extend(entries.iter().filter_map(|entry| {
            let id = entry.get("operationId")?.as_str()?;
            let reason = entry.get("deferredReason")?.as_str()?.trim();
            (!reason.is_empty()).then(|| DeferredWork {
                kind: "route",
                id: id.to_owned(),
                reason: reason.to_owned(),
            })
        }));
    }
    deferred.sort();
    deferred
}

/// Run every fail-closed verification rule.
///
/// # Errors
/// Returns [`Exit::GraphVerification`] carrying every violation found.
#[allow(clippy::too_many_lines)]
pub fn verify(inputs: &GraphInputs) -> Result<BuiltGraph> {
    // A registry naming a package that does not exist produces a dangling edge,
    // so the graph cannot be built at all. Running that one check first means
    // the report says which registry row is wrong rather than only that some
    // edge pointed at nothing.
    let mut prelude = registry_reference_violations(inputs);
    let built = match build(inputs) {
        Ok(built) => built,
        Err(err) => {
            prelude.extend(err.violations);
            prelude.sort();
            prelude.dedup();
            return Err(ToolError::many(Exit::GraphVerification, prelude));
        }
    };
    let mut violations = prelude;
    violations.extend(inputs.input_violations.clone());

    // 1. Every classifiable node declares ownership metadata.
    for node in built.graph.nodes() {
        let needs_metadata = matches!(
            node.kind,
            NodeKind::Crate
                | NodeKind::NpmPackage
                | NodeKind::TerraformModule
                | NodeKind::TerraformRoot
        );
        if !needs_metadata {
            continue;
        }
        match &node.meta {
            None => violations.push(Violation::new(
                "aex-metadata-missing",
                format!(
                    "`{}` has no ownership metadata; every member declares its test ownership",
                    node.dir
                ),
            )),
            Some(meta) => violations.extend(meta.validate(&node.dir)),
        }
    }

    // 4. Every repository file is classified.
    for path in &built.unowned {
        violations.push(Violation::new(
            "orphan-path",
            format!(
                "`{path}` matches no release/path-map.toml rule; it widens every run to \
                 repo-wide and owns nothing"
            ),
        ));
    }

    // 2. Every deployable has a recipe, a companion and a scenario owner.
    let cargo_names: BTreeSet<&str> = inputs
        .cargo
        .iter()
        .map(|package| package.name.as_str())
        .collect();
    let scenario_owners: BTreeSet<&str> = inputs
        .scenarios
        .scenarios
        .iter()
        .flat_map(|scenario| scenario.observes.iter().map(String::as_str))
        .collect();
    for unit in &inputs.units.units {
        let companion = unit
            .live_suite
            .clone()
            .unwrap_or_else(|| format!("aex-live-{}", unit.id));
        if !cargo_names.contains(companion.as_str()) {
            violations.push(Violation::new(
                "unit-live-companion-missing",
                format!(
                    "unit `{}` has no companion live package `tests/live/{companion}`",
                    unit.id
                ),
            ));
        }
        if !scenario_owners.contains(format!("artifact:{}", unit.id).as_str()) {
            violations.push(Violation::new(
                "missing-scenario-owner",
                format!(
                    "unit `{}` is observed by no scenario in release/scenario-ownership.toml",
                    unit.id
                ),
            ));
        }
        violations.extend(verify_resource_shape(unit));
        if unit.required_receipts.is_empty() {
            violations.push(Violation::new(
                "unit-required-receipts-empty",
                format!(
                    "unit `{}` requires no receipt class; publication would prove nothing",
                    unit.id
                ),
            ));
        }
    }

    let expected_microvms: BTreeSet<(String, u32, bool)> = [
        ("512mb", 512, false),
        ("1gb", 1_024, false),
        ("2gb", 2_048, false),
        ("2gb-browser", 2_048, true),
        ("4gb", 4_096, false),
        ("4gb-browser", 4_096, true),
        ("8gb", 8_192, false),
        ("8gb-browser", 8_192, true),
    ]
    .into_iter()
    .map(|(variant, memory, browser)| (variant.to_owned(), memory, browser))
    .collect();
    let actual_microvms: BTreeSet<(String, u32, bool)> = inputs
        .units
        .units
        .iter()
        .filter(|unit| unit.kind == "microvm-image")
        .filter_map(|unit| {
            unit.microvm.as_ref().map(|shape| {
                (
                    shape.variant.clone(),
                    shape.minimum_memory_mib,
                    shape.browser,
                )
            })
        })
        .collect();
    if cargo_names.contains("hands-image") && actual_microvms != expected_microvms {
        violations.push(Violation::new(
            "microvm-variant-set",
            format!(
                "MicroVM artifacts must declare exactly the five base and three browser variants; found {actual_microvms:?}"
            ),
        ));
    }

    // 3. Scenario rows name real nodes and a runnable package target. Merely
    //    observing an artifact is selection metadata, not executable evidence.
    let (scenario_claims, scenario_violations) = verify_scenario_claims(inputs);
    violations.extend(scenario_violations);
    for scenario in &inputs.scenarios.scenarios {
        if scenario.observes.is_empty() {
            violations.push(Violation::new(
                "scenario-observes-nothing",
                format!("scenario `{}` observes no node", scenario.id),
            ));
        }
    }

    // A live companion must be named by something.
    for package in &inputs.cargo {
        if !package.name.starts_with("aex-live-") {
            continue;
        }
        if !built.live_targets.contains(&package.name) {
            violations.push(Violation::new(
                "aex-orphan-companion",
                format!(
                    "`{}` is a live companion that no package or unit names as its live_suite",
                    package.dir
                ),
            ));
        }
    }
    // Conversely, a declared live suite must exist.
    for target in &built.live_targets {
        if !cargo_names.contains(target.as_str()) {
            violations.push(Violation::new(
                "aex-orphan-companion",
                format!(
                    "live suite `{target}` is declared but `tests/live/{target}` is not a \
                     workspace member"
                ),
            ));
        }
    }

    // The delivery graph and the derived test registry both derive the live
    // target set from `live_suite` declarations, by different routes. If they
    // disagree, one of them is reading metadata the other is not.
    if inputs
        .root
        .join(crate::test_registry::REGISTRY_PATH)
        .is_file()
    {
        match crate::test_registry::load_document(&inputs.root) {
            Ok(registry) => {
                if let Err(err) = crate::test_registry::check_live_target_agreement(
                    &built.live_targets,
                    &registry,
                ) {
                    violations.extend(err.violations);
                }
            }
            Err(err) => violations.extend(err.violations),
        }
    }

    // 3a. OD-36: a scenario may provision in `prd` only if the janitor can
    //     reclaim everything it creates from tags alone.
    violations.extend(verify_prd_provisioning(inputs));

    // 6. Declared migrations are covered by their bundle.
    violations.extend(verify_migration_coverage(inputs));

    // 7. Every public route has a scenario or contract owner.
    violations.extend(verify_route_coverage(inputs, &built, &scenario_claims));

    if violations.is_empty() {
        Ok(built)
    } else {
        violations.sort();
        violations.dedup();
        Err(ToolError::many(Exit::GraphVerification, violations))
    }
}

#[derive(Debug, Default)]
struct ScenarioClaims {
    runnable: BTreeSet<String>,
    deferred: BTreeSet<String>,
}

fn verify_scenario_claims(inputs: &GraphInputs) -> (ScenarioClaims, Vec<Violation>) {
    let packages: BTreeMap<String, (&str, Option<&crate::meta::AexMeta>)> = inputs
        .cargo
        .iter()
        .map(|package| {
            (
                NodeId::cargo(&package.name).to_string(),
                (package.dir.as_str(), package.meta.as_ref()),
            )
        })
        .chain(inputs.npm.iter().map(|package| {
            (
                NodeId::npm(&package.name).to_string(),
                (package.dir.as_str(), package.meta.as_ref()),
            )
        }))
        .collect();
    let mut claims = ScenarioClaims::default();
    let mut violations = Vec::new();
    for scenario in &inputs.scenarios.scenarios {
        let (package, target) = match (&scenario.package, &scenario.target, &scenario.deferred) {
            (Some(package), Some(target), None) => (package, target),
            (None, None, Some(reason)) if !reason.trim().is_empty() => {
                claims.deferred.insert(scenario.id.clone());
                continue;
            }
            (None, None, Some(_)) => {
                violations.push(Violation::new(
                    "scenario-deferral-invalid",
                    format!("scenario `{}` has an empty deferral reason", scenario.id),
                ));
                continue;
            }
            (Some(_), Some(_), Some(_)) => {
                violations.push(Violation::new(
                    "scenario-claim-conflict",
                    format!(
                        "scenario `{}` cannot be both runnable and explicitly deferred",
                        scenario.id
                    ),
                ));
                continue;
            }
            _ => {
                violations.push(Violation::new(
                    "scenario-runnable-missing",
                    format!(
                        "scenario `{}` must declare both runnable `package` and `target`, or a non-empty `deferred` reason",
                        scenario.id
                    ),
                ));
                continue;
            }
        };
        let Some((dir, meta)) = packages.get(package) else {
            violations.push(Violation::new(
                "scenario-package-unknown",
                format!(
                    "scenario `{}` names package `{package}`, which is not a Cargo or npm graph node",
                    scenario.id
                ),
            ));
            continue;
        };
        let Some(meta) = meta else {
            violations.push(Violation::new(
                "scenario-package-disagreement",
                format!(
                    "scenario `{}` names `{dir}`, which has no aex metadata claim",
                    scenario.id
                ),
            ));
            continue;
        };
        let mut sound = true;
        if !meta.scenarios.contains(&scenario.id) {
            sound = false;
            violations.push(Violation::new(
                "scenario-package-disagreement",
                format!(
                    "scenario `{}` names `{dir}`, but that package does not claim it in `aex.scenarios`",
                    scenario.id
                ),
            ));
        }
        if !meta.targets.contains_key(target) {
            sound = false;
            violations.push(Violation::new(
                "scenario-target-unknown",
                format!(
                    "scenario `{}` names target `{target}` in `{dir}`, but `aex.targets` does not declare it",
                    scenario.id
                ),
            ));
        }
        if sound {
            claims.runnable.insert(scenario.id.clone());
        }
    }
    (claims, violations)
}

/// OD-36, mechanically: a scenario may be marked `prd`-eligible only if every
/// resource it creates is reclaimable by the janitor from tags.
///
/// The selection rule itself (one money path, one session lifecycle, one
/// content path, one secret path, readiness plus one real write per deployable)
/// lives in `[prd_provisioning.rule]`, so which scenarios are in is checked
/// data rather than prose. This function enforces the hard rule around it: a
/// `prd`-eligible scenario that creates an unreclaimable resource kind fails,
/// because it produces residue no sweep can ever remove and the whole
/// provisioning decision rests on the sweep working.
fn verify_prd_provisioning(inputs: &GraphInputs) -> Vec<Violation> {
    let policy = aex_workspace_check::policy::Policy::embedded();
    let mut violations = Vec::new();
    let mut by_rule: BTreeMap<&str, Vec<&str>> = BTreeMap::new();

    for scenario in &inputs.scenarios.scenarios {
        let Some(prd) = &scenario.prd else {
            continue;
        };
        by_rule
            .entry(prd.rule.as_str())
            .or_default()
            .push(scenario.id.as_str());

        if !policy.prd_rules.contains_key(&prd.rule) {
            violations.push(Violation::new(
                "scenario-prd-rule-unknown",
                format!(
                    "scenario `{}` claims prd rule `{}`, which is not declared in \
                     [prd_provisioning.rule]; provisioning in production needs a stated reason \
                     from the closed set",
                    scenario.id, prd.rule
                ),
            ));
        }

        if prd.provisions.is_empty() {
            violations.push(Violation::new(
                "scenario-prd-provisions-nothing",
                format!(
                    "scenario `{}` is marked prd-eligible but declares no resource kinds; a \
                     scenario that creates nothing needs no permission to create in production",
                    scenario.id
                ),
            ));
        }

        let declared: BTreeSet<&str> = prd.provisions.iter().map(String::as_str).collect();
        for kind in &prd.provisions {
            let Some(row) = policy.janitor.resources.get(kind) else {
                violations.push(Violation::new(
                    "scenario-prd-kind-unknown",
                    format!(
                        "scenario `{}` declares it provisions `{kind}`, which has no \
                         [janitor.resource] row",
                        scenario.id
                    ),
                ));
                continue;
            };
            if !row.reclaimable_from_tags() {
                violations.push(Violation::new(
                    "scenario-prd-unreclaimable",
                    format!(
                        "scenario `{}` is prd-eligible and provisions `{kind}`, which the \
                         janitor cannot reclaim from tags ({}); a prd-eligible scenario may \
                         only create what a sweep can remove",
                        scenario.id, row.reclaim
                    ),
                ));
            }
            for companion in &row.requires_declared_with {
                if !declared.contains(companion.as_str()) {
                    violations.push(Violation::new(
                        "scenario-prd-companion-undeclared",
                        format!(
                            "scenario `{}` provisions `{kind}` without declaring `{companion}`; \
                             creating one always creates the other, and reclamation of \
                             `{kind}` is only correct once `{companion}` is reclaimed too",
                            scenario.id
                        ),
                    ));
                }
            }
        }
    }

    // Provisioning in `prd` is opt-in as a whole: a registry that marks nothing
    // is simply not doing it, which is the safer state and is what every
    // synthetic fixture is. Once anything is marked, the shape is fixed.
    if by_rule.is_empty() {
        return violations;
    }
    for (rule, declared) in &policy.prd_rules {
        if declared.cardinality != "exactly_one" {
            continue;
        }
        let claimants = by_rule.get(rule.as_str()).map_or(0, Vec::len);
        if claimants != 1 {
            violations.push(Violation::new(
                "scenario-prd-rule-cardinality",
                format!(
                    "prd rule `{rule}` admits {} ({}), but {claimants} scenario(s) claim it; the \
                     reduced prd set is one of each, not breadth",
                    declared.admits, declared.cardinality
                ),
            ));
        }
    }

    violations
}

/// Registry rows that name a node the workspace does not hold.
///
/// These are checked before the graph is built, because a dangling edge stops
/// construction and the resulting message names an edge rather than the row a
/// human has to fix.
fn registry_reference_violations(inputs: &GraphInputs) -> Vec<Violation> {
    let mut violations = Vec::new();
    for unit in &inputs.units.units {
        let known = inputs
            .cargo
            .iter()
            .any(|package| package.name == unit.package)
            || inputs
                .npm
                .iter()
                .any(|package| package.name == unit.package);
        if !known {
            violations.push(Violation::new(
                "unit-package-unknown",
                format!(
                    "unit `{}` names package `{}`, which is not a workspace member",
                    unit.id, unit.package
                ),
            ));
        }
    }
    violations
}

/// Every deployable declares a real resource shape. A placeholder is rejected:
/// a memory or timeout of zero is not a value anyone can deploy, and shipping
/// one would turn the declaration into decoration.
#[allow(clippy::too_many_lines)]
fn verify_resource_shape(unit: &super::inputs::Unit) -> Vec<Violation> {
    let mut violations = Vec::new();
    let wants_lambda = matches!(unit.kind.as_str(), "rust-lambda" | "ts-lambda");
    let wants_fargate = matches!(unit.kind.as_str(), "rust-oci-service" | "rust-oci-task");
    let wants_microvm = unit.kind == "microvm-image";
    if wants_lambda {
        match unit.lambda {
            None => violations.push(Violation::new(
                "unit-resource-shape-missing",
                format!(
                    "unit `{}` is a Lambda and declares no [unit.lambda] memory, timeout and \
                     reserved concurrency",
                    unit.id
                ),
            )),
            Some(shape) => {
                if !(128..=10_240).contains(&shape.memory_mb) {
                    violations.push(Violation::new(
                        "unit-resource-shape-placeholder",
                        format!(
                            "unit `{}` declares memory_mb = {}; Lambda accepts 128..=10240",
                            unit.id, shape.memory_mb
                        ),
                    ));
                }
                if !(1..=900).contains(&shape.timeout_s) {
                    violations.push(Violation::new(
                        "unit-resource-shape-placeholder",
                        format!(
                            "unit `{}` declares timeout_s = {}; Lambda accepts 1..=900",
                            unit.id, shape.timeout_s
                        ),
                    ));
                }
            }
        }
        if unit.fargate.is_some() {
            violations.push(Violation::new(
                "unit-resource-shape-conflict",
                format!(
                    "unit `{}` is a Lambda and declares a Fargate shape",
                    unit.id
                ),
            ));
        }
    }
    if wants_fargate {
        match unit.fargate {
            None => violations.push(Violation::new(
                "unit-resource-shape-missing",
                format!(
                    "unit `{}` runs on Fargate and declares no [unit.fargate] task shape",
                    unit.id
                ),
            )),
            Some(shape) => {
                if shape.cpu == 0 || shape.memory_mb == 0 || shape.stop_timeout_s == 0 {
                    violations.push(Violation::new(
                        "unit-resource-shape-placeholder",
                        format!(
                            "unit `{}` declares a zero cpu, memory or stop timeout",
                            unit.id
                        ),
                    ));
                }
                if shape.port == 0 && unit.kind == "rust-oci-service" {
                    violations.push(Violation::new(
                        "unit-resource-shape-placeholder",
                        format!("unit `{}` is a service and declares no port", unit.id),
                    ));
                }
                if unit.kind == "rust-oci-task" && (shape.desired_count != 0 || shape.port != 0) {
                    violations.push(Violation::new(
                        "unit-resource-shape-conflict",
                        format!(
                            "unit `{}` is a one-shot task and must declare desired_count = 0 and port = 0",
                            unit.id
                        ),
                    ));
                }
            }
        }
        if unit.lambda.is_some() {
            violations.push(Violation::new(
                "unit-resource-shape-conflict",
                format!(
                    "unit `{}` runs on Fargate and declares a Lambda shape",
                    unit.id
                ),
            ));
        }
    }
    if wants_microvm {
        match &unit.microvm {
            None => violations.push(Violation::new(
                "unit-resource-shape-missing",
                format!(
                    "unit `{}` is a MicroVM image and declares no [unit.microvm] shape",
                    unit.id
                ),
            )),
            Some(shape) => {
                if unit.id != format!("hands-image-{}", shape.variant) {
                    violations.push(Violation::new(
                        "microvm-variant-identity",
                        format!(
                            "unit `{}` must be named `hands-image-{}` so its artifact identity cannot be relabelled",
                            unit.id, shape.variant
                        ),
                    ));
                }
                if shape.browser != shape.variant.ends_with("-browser") {
                    violations.push(Violation::new(
                        "microvm-variant-identity",
                        format!("unit `{}` has an inconsistent browser capability", unit.id),
                    ));
                }
                if unit.target != "aarch64-unknown-linux-musl" || unit.form != "zip" {
                    violations.push(Violation::new(
                        "microvm-artifact-shape",
                        format!(
                            "unit `{}` must be an ARM64 musl guest delivered as an AWS service ZIP",
                            unit.id
                        ),
                    ));
                }
            }
        }
        if unit.lambda.is_some() || unit.fargate.is_some() {
            violations.push(Violation::new(
                "unit-resource-shape-conflict",
                format!(
                    "unit `{}` is a MicroVM image and declares a runtime compute shape",
                    unit.id
                ),
            ));
        }
    } else if unit.microvm.is_some() {
        violations.push(Violation::new(
            "unit-resource-shape-conflict",
            format!(
                "unit `{}` is not a MicroVM image but declares [unit.microvm]",
                unit.id
            ),
        ));
    }
    // Health paths are one convention workspace-wide.
    for (field, value) in [
        ("health_path", unit.health_path.as_deref()),
        ("ready_path", unit.ready_path.as_deref()),
    ] {
        let expected = if field == "health_path" {
            "/internal/healthz"
        } else {
            "/internal/readyz"
        };
        if let Some(value) = value
            && value != expected
        {
            violations.push(Violation::new(
                "unit-health-path-nonstandard",
                format!(
                    "unit `{}` declares {field} `{value}`; every Rust deployable serves \
                     `{expected}`",
                    unit.id
                ),
            ));
        }
    }
    if unit.kind == "rust-oci-service" && (unit.health_path.is_none() || unit.ready_path.is_none())
    {
        violations.push(Violation::new(
            "unit-health-path-missing",
            format!(
                "unit `{}` is a long-lived service and must declare both \
                 `/internal/healthz` and `/internal/readyz`",
                unit.id
            ),
        ));
    }
    violations
}

fn verify_migration_coverage(inputs: &GraphInputs) -> Vec<Violation> {
    let mut violations = Vec::new();
    let central = inputs.root.join("migrations/central");
    let Ok(entries) = std::fs::read_dir(&central) else {
        return violations;
    };
    let mut sql: Vec<String> = entries
        .flatten()
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("sql"))
        })
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .collect();
    if sql.is_empty() {
        return violations;
    }
    sql.sort();
    let lock = inputs.root.join("migrations/central/bundle.lock.json");
    let Ok(text) = std::fs::read_to_string(&lock) else {
        violations.push(Violation::new(
            "migration-bundle-missing",
            format!(
                "{} central migration file(s) exist with no \
                 migrations/central/bundle.lock.json",
                sql.len()
            ),
        ));
        return violations;
    };
    let declared: BTreeSet<String> = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|document| {
            document
                .get("files")
                .and_then(serde_json::Value::as_array)
                .map(|files| {
                    files
                        .iter()
                        .filter_map(|file| {
                            file.get("file")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_owned)
                        })
                        .collect()
                })
        })
        .unwrap_or_default();
    for file in sql {
        if !declared.contains(&file) {
            violations.push(Violation::new(
                "migration-outside-bundle",
                format!("`migrations/central/{file}` is outside the declared bundle"),
            ));
        }
    }
    violations
}

fn verify_route_coverage(
    inputs: &GraphInputs,
    built: &BuiltGraph,
    scenario_claims: &ScenarioClaims,
) -> Vec<Violation> {
    let mut violations = Vec::new();
    let routes = inputs.root.join("api/generated/registries/routes.json");
    let bundle = inputs.root.join("api/generated/bundle.json");
    if !has_authored_route_surface(&inputs.root) && !routes.is_file() && !bundle.is_file() {
        // A reusable delivery-graph fixture or repository with no authored API
        // has no route surface to cover. Authored OpenAPI is the sentinel;
        // generated outputs are deliberately not trusted to announce their own
        // absence.
        return violations;
    }
    violations.extend(verify_generated_contract_freshness(inputs));
    let document = match read_strict_route_registry(&routes) {
        Ok(document) => document,
        Err(violation) => {
            violations.push(violation);
            return violations;
        }
    };
    if document.get("schema").and_then(serde_json::Value::as_str) != Some("aex.route-registry.v1") {
        violations.push(Violation::new(
            "route-registry-schema",
            "route registry schema must be `aex.route-registry.v1`".to_owned(),
        ));
    }
    let Some(registry_operations) = verify_route_entries(
        &document,
        &RouteCoverageContext::new(inputs, built, scenario_claims),
        &mut violations,
    ) else {
        return violations;
    };
    match generated_contract_operations(&bundle) {
        Ok(contract_operations) => compare_route_operation_sets(
            &contract_operations,
            &registry_operations,
            &mut violations,
        ),
        Err(violation) => violations.push(violation),
    }
    violations
}

fn verify_generated_contract_freshness(inputs: &GraphInputs) -> Vec<Violation> {
    if !has_authored_route_surface(&inputs.root)
        && !inputs
            .root
            .join("api/generated/registries/routes.json")
            .is_file()
        && !inputs.root.join("api/generated/bundle.json").is_file()
    {
        return Vec::new();
    }
    match aex_contract_gen::check(&inputs.root) {
        Ok(drift) if drift.is_empty() => Vec::new(),
        Ok(drift) => vec![Violation::new(
            "generated-contract-stale",
            format!(
                "generated contract output is stale: {}",
                drift.into_iter().take(8).collect::<Vec<_>>().join(", ")
            ),
        )],
        Err(error) => vec![Violation::new(
            "generated-contract-unverifiable",
            format!("generated contract freshness could not be verified: {error}"),
        )],
    }
}

fn has_authored_route_surface(root: &std::path::Path) -> bool {
    let openapi = root.join("api/openapi");
    let authored_openapi = walkdir::WalkDir::new(&openapi)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .any(|entry| {
            entry.file_type().is_file()
                && entry.path().extension().is_some_and(|extension| {
                    extension.eq_ignore_ascii_case("yaml") || extension.eq_ignore_ascii_case("yml")
                })
        });
    authored_openapi
        || root
            .join("api/schemas/registries/routes-meta.yaml")
            .is_file()
}

fn read_strict_route_registry(
    path: &std::path::Path,
) -> std::result::Result<serde_json::Value, Violation> {
    let text = std::fs::read_to_string(path).map_err(|_| {
        Violation::new(
            "route-registry-missing",
            "api/generated/bundle.json exists but api/generated/registries/routes.json does not"
                .to_owned(),
        )
    })?;
    strict_json(&text).map_err(|_| {
        Violation::new(
            "route-registry-unparseable",
            "api/generated/registries/routes.json does not parse".to_owned(),
        )
    })
}

struct RouteCoverageContext<'a> {
    scenarios: BTreeMap<&'a str, &'a super::inputs::Scenario>,
    runnable_scenarios: &'a BTreeSet<String>,
    deferred_scenarios: &'a BTreeSet<String>,
    artifacts: BTreeMap<&'a str, &'a str>,
}

impl<'a> RouteCoverageContext<'a> {
    fn new(
        inputs: &'a GraphInputs,
        built: &'a BuiltGraph,
        scenario_claims: &'a ScenarioClaims,
    ) -> Self {
        Self {
            scenarios: inputs
                .scenarios
                .scenarios
                .iter()
                .map(|scenario| (scenario.id.as_str(), scenario))
                .collect(),
            runnable_scenarios: &scenario_claims.runnable,
            deferred_scenarios: &scenario_claims.deferred,
            artifacts: inputs
                .units
                .units
                .iter()
                .map(|unit| (unit.id.as_str(), unit.plane.as_str()))
                .filter(|(artifact, _)| {
                    built
                        .graph
                        .nodes()
                        .iter()
                        .any(|node| node.kind == NodeKind::Artifact && node.id.local() == *artifact)
                })
                .collect(),
        }
    }
}

fn verify_route_entries(
    document: &serde_json::Value,
    context: &RouteCoverageContext<'_>,
    violations: &mut Vec<Violation>,
) -> Option<BTreeSet<String>> {
    let Some(entries) = document.get("routes").and_then(serde_json::Value::as_array) else {
        violations.push(Violation::new(
            "route-registry-shape",
            "route registry has no `routes` array".to_owned(),
        ));
        return None;
    };
    let mut operations = BTreeSet::new();
    for entry in entries {
        violations.extend(verify_route_entry(entry, context, &mut operations));
    }
    Some(operations)
}

fn verify_route_entry(
    entry: &serde_json::Value,
    context: &RouteCoverageContext<'_>,
    operations: &mut BTreeSet<String>,
) -> Vec<Violation> {
    let Some(operation) = entry.get("operationId").and_then(serde_json::Value::as_str) else {
        return vec![Violation::new(
            "route-registry-shape",
            "a route registry row has no string `operationId`".to_owned(),
        )];
    };
    let mut violations = Vec::new();
    if !operations.insert(operation.to_owned()) {
        violations.push(Violation::new(
            "route-registry-duplicate",
            format!("operationId `{operation}` appears more than once"),
        ));
    }
    let plane = entry
        .get("plane")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let Some(planned_artifact) = entry
        .get("servingArtifact")
        .and_then(serde_json::Value::as_str)
    else {
        violations.push(Violation::new(
            "aex-route-owner-invalid",
            format!("operationId `{operation}` has no planned serving artifact"),
        ));
        return violations;
    };
    violations.extend(verify_route_artifact(
        operation,
        "planned",
        planned_artifact,
        plane,
        context,
    ));
    let Some(served_artifact) = entry
        .get("servedArtifact")
        .and_then(serde_json::Value::as_str)
    else {
        match entry.get("deferredReason") {
            Some(serde_json::Value::String(reason)) if !reason.trim().is_empty() => {}
            Some(_) => violations.push(Violation::new(
                "aex-route-deferral-invalid",
                format!(
                    "operationId `{operation}` is not mounted and has no non-empty string `deferredReason`"
                ),
            )),
            None => violations.push(Violation::new(
                "aex-route-unserved",
                format!(
                    "operationId `{operation}` is planned for `{planned_artifact}` but is neither actually mounted nor explicitly deferred"
                ),
            )),
        }
        return violations;
    };
    if entry.get("deferredReason").is_some() {
        violations.push(Violation::new(
            "aex-route-state-conflict",
            format!("operationId `{operation}` cannot be both served and explicitly deferred"),
        ));
    }
    violations.extend(verify_route_artifact(
        operation,
        "actual",
        served_artifact,
        plane,
        context,
    ));
    if served_artifact != planned_artifact {
        violations.push(Violation::new(
            "aex-route-owner-disagreement",
            format!(
                "operationId `{operation}` is planned for `{planned_artifact}` but claims it is served by `{served_artifact}`"
            ),
        ));
    }
    violations.extend(verify_route_scenarios(
        entry,
        operation,
        served_artifact,
        context,
    ));
    violations
}

fn verify_route_artifact(
    operation: &str,
    claim: &str,
    artifact: &str,
    plane: &str,
    context: &RouteCoverageContext<'_>,
) -> Vec<Violation> {
    let valid = !artifact.is_empty()
        && artifact
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && artifact
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_lowercase)
        && artifact
            .as_bytes()
            .last()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && !artifact.contains("--");
    if !valid {
        return vec![Violation::new(
            "aex-route-owner-invalid",
            format!(
                "operationId `{operation}` has malformed {claim} serving artifact `{artifact}`"
            ),
        )];
    }
    match context.artifacts.get(artifact) {
        None => vec![Violation::new(
            if claim == "actual" {
                "aex-route-unserved"
            } else {
                "aex-route-owner-invalid"
            },
            format!(
                "operationId `{operation}` names unknown {claim} serving artifact `{artifact}`"
            ),
        )],
        Some(artifact_plane) if *artifact_plane != plane => vec![Violation::new(
            "aex-route-owner-cross-plane",
            format!(
                "operationId `{operation}` is on `{plane}` but {claim} serving artifact `{artifact}` is on `{artifact_plane}`"
            ),
        )],
        Some(_) => Vec::new(),
    }
}

fn verify_route_scenarios(
    entry: &serde_json::Value,
    operation: &str,
    serving_artifact: &str,
    context: &RouteCoverageContext<'_>,
) -> Vec<Violation> {
    let Some(values) = entry.get("scenarios").and_then(serde_json::Value::as_array) else {
        return vec![Violation::new(
            "aex-route-uncovered",
            format!("operationId `{operation}` has no scenario owner array"),
        )];
    };
    let mut violations = Vec::new();
    let mut owners = BTreeSet::new();
    for value in values {
        let Some(owner) = value.as_str() else {
            violations.push(Violation::new(
                "route-registry-shape",
                format!("operationId `{operation}` has a non-string scenario owner"),
            ));
            continue;
        };
        if !owners.insert(owner) {
            violations.push(Violation::new(
                "route-registry-duplicate",
                format!("operationId `{operation}` repeats scenario `{owner}`"),
            ));
        }
    }
    if owners.is_empty() {
        violations.push(Violation::new(
            "aex-route-uncovered",
            format!("operationId `{operation}` has no scenario owner"),
        ));
        return violations;
    }
    for owner in owners {
        violations.extend(verify_route_scenario(
            operation,
            serving_artifact,
            owner,
            context,
        ));
    }
    violations
}

fn verify_route_scenario(
    operation: &str,
    serving_artifact: &str,
    owner: &str,
    context: &RouteCoverageContext<'_>,
) -> Vec<Violation> {
    let Some(scenario) = context.scenarios.get(owner) else {
        return vec![Violation::new(
            "aex-route-uncovered",
            format!(
                "operationId `{operation}` names scenario `{owner}`, which is not declared in \
                 release/scenario-ownership.toml"
            ),
        )];
    };
    if !context.runnable_scenarios.contains(owner) && !context.deferred_scenarios.contains(owner) {
        return vec![Violation::new(
            "aex-route-uncovered",
            format!(
                "operationId `{operation}` names scenario `{owner}`, but it has no verified runnable package target"
            ),
        )];
    }
    let observed = format!("artifact:{serving_artifact}");
    if scenario.observes.contains(&observed) {
        return Vec::new();
    }
    vec![Violation::new(
        "aex-route-scenario-disagreement",
        format!(
            "operationId `{operation}` is served by `{serving_artifact}`, but scenario `{owner}` \
             does not observe `artifact:{serving_artifact}` in release/scenario-ownership.toml"
        ),
    )]
}

fn compare_route_operation_sets(
    contract: &BTreeSet<String>,
    registry: &BTreeSet<String>,
    violations: &mut Vec<Violation>,
) {
    for missing in contract.difference(registry) {
        violations.push(Violation::new(
            "route-registry-incomplete",
            format!("contract operationId `{missing}` is absent from the route registry"),
        ));
    }
    for extra in registry.difference(contract) {
        violations.push(Violation::new(
            "route-registry-extra",
            format!("route registry operationId `{extra}` is absent from the contract bundle"),
        ));
    }
}

fn generated_contract_operations(
    path: &std::path::Path,
) -> std::result::Result<BTreeSet<String>, Violation> {
    let text = std::fs::read_to_string(path).map_err(|_| {
        Violation::new(
            "contract-bundle-missing",
            "route registry exists but api/generated/bundle.json does not".to_owned(),
        )
    })?;
    let document = strict_json(&text).map_err(|_| {
        Violation::new(
            "contract-bundle-unparseable",
            "api/generated/bundle.json does not parse".to_owned(),
        )
    })?;
    let Some(planes) = document
        .get("planes")
        .and_then(serde_json::Value::as_object)
    else {
        return Err(Violation::new(
            "contract-bundle-shape",
            "contract bundle has no `planes` object".to_owned(),
        ));
    };
    let mut operations = BTreeSet::new();
    for (plane, value) in planes {
        let Some(rows) = value
            .get("operations")
            .and_then(serde_json::Value::as_array)
        else {
            return Err(Violation::new(
                "contract-bundle-shape",
                format!("contract bundle plane `{plane}` has no operations array"),
            ));
        };
        for row in rows {
            let Some(operation) = row.get("operationId").and_then(serde_json::Value::as_str) else {
                return Err(Violation::new(
                    "contract-bundle-shape",
                    format!("contract bundle plane `{plane}` has a row without operationId"),
                ));
            };
            if !operations.insert(operation.to_owned()) {
                return Err(Violation::new(
                    "contract-bundle-duplicate",
                    format!("contract operationId `{operation}` appears more than once"),
                ));
            }
        }
    }
    Ok(operations)
}

/// A JSON value whose deserializer rejects duplicate object members at every
/// depth. `serde_json::Value` otherwise keeps the last member, which is not a
/// safe interpretation for release authorities.
struct StrictJson(serde_json::Value);

impl<'de> Deserialize<'de> for StrictJson {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictJsonVisitor)
    }
}

struct StrictJsonVisitor;

impl<'de> Visitor<'de> for StrictJsonVisitor {
    type Value = StrictJson;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("JSON without duplicate object members")
    }

    fn visit_bool<E>(self, value: bool) -> std::result::Result<Self::Value, E> {
        Ok(StrictJson(serde_json::Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> std::result::Result<Self::Value, E> {
        Ok(StrictJson(serde_json::Value::Number(value.into())))
    }

    fn visit_u64<E>(self, value: u64) -> std::result::Result<Self::Value, E> {
        Ok(StrictJson(serde_json::Value::Number(value.into())))
    }

    fn visit_f64<E>(self, value: f64) -> std::result::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        serde_json::Number::from_f64(value)
            .map(serde_json::Value::Number)
            .map(StrictJson)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        self.visit_string(value.to_owned())
    }

    fn visit_string<E>(self, value: String) -> std::result::Result<Self::Value, E> {
        Ok(StrictJson(serde_json::Value::String(value)))
    }

    fn visit_none<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(StrictJson(serde_json::Value::Null))
    }

    fn visit_unit<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(StrictJson(serde_json::Value::Null))
    }

    fn visit_some<D>(self, deserializer: D) -> std::result::Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        StrictJson::deserialize(deserializer)
    }

    fn visit_seq<A>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<StrictJson>()? {
            values.push(value.0);
        }
        Ok(StrictJson(serde_json::Value::Array(values)))
    }

    fn visit_map<A>(self, mut map: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = serde_json::Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(serde::de::Error::custom(format!(
                    "duplicate JSON member `{key}`"
                )));
            }
            let value = map.next_value::<StrictJson>()?;
            values.insert(key, value.0);
        }
        Ok(StrictJson(serde_json::Value::Object(values)))
    }
}

fn strict_json(text: &str) -> std::result::Result<serde_json::Value, serde_json::Error> {
    serde_json::from_str::<StrictJson>(text).map(|value| value.0)
}

/// Summarize a built graph for `--json` output.
#[must_use]
pub fn summarize(built: &BuiltGraph) -> GraphSummary {
    let mut nodes: BTreeMap<String, usize> = BTreeMap::new();
    for node in built.graph.nodes() {
        *nodes.entry(format!("{:?}", node.kind)).or_default() += 1;
    }
    GraphSummary {
        schema: "aex.graph-summary.v1",
        nodes,
        edges: built.graph.forward().len(),
        live_targets: built.live_targets.iter().cloned().collect(),
        unowned: built.unowned.clone(),
        deferred: built.deferred.clone(),
    }
}
