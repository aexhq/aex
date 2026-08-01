//! `graph build` and `graph verify`.
//!
//! Verification is fail-closed on every condition in plan 14 §2.4. The one
//! design point worth restating: an unowned repository path both widens the run
//! to repo-wide **and** fails verification. The predecessor implementation only
//! widened, so a file nobody owned ran everything forever and nobody found out.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

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
    })
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

    // 3. Scenario rows name real nodes. Graph construction already rejects an
    //    unknown edge target, so this rule reports the case the graph could not
    //    have been built for at all.
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

    // 6. Declared migrations are covered by their bundle.
    violations.extend(verify_migration_coverage(inputs));

    // 7. Every public route has a scenario or contract owner.
    violations.extend(verify_route_coverage(inputs, &built));

    if violations.is_empty() {
        Ok(built)
    } else {
        violations.sort();
        violations.dedup();
        Err(ToolError::many(Exit::GraphVerification, violations))
    }
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

fn verify_route_coverage(inputs: &GraphInputs, built: &BuiltGraph) -> Vec<Violation> {
    let mut violations = Vec::new();
    let routes = inputs.root.join("api/generated/registries/routes.json");
    let Ok(text) = std::fs::read_to_string(&routes) else {
        return violations;
    };
    let Ok(document) = serde_json::from_str::<serde_json::Value>(&text) else {
        violations.push(Violation::new(
            "route-registry-unparseable",
            "api/generated/registries/routes.json does not parse".to_owned(),
        ));
        return violations;
    };
    let declared_scenarios: BTreeSet<&str> = built
        .graph
        .nodes()
        .iter()
        .filter(|node| node.kind == NodeKind::Scenario)
        .map(|node| node.id.local())
        .collect();
    let covered: BTreeSet<String> = inputs
        .cargo
        .iter()
        .filter_map(|package| package.meta.as_ref())
        .flat_map(|meta| meta.scenarios.iter().cloned())
        .collect();
    let Some(entries) = document.get("routes").and_then(serde_json::Value::as_array) else {
        return violations;
    };
    for entry in entries {
        let Some(operation) = entry.get("operationId").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let owners = entry
            .get("scenarios")
            .and_then(serde_json::Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(serde_json::Value::as_str)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if owners.is_empty() {
            violations.push(Violation::new(
                "aex-route-uncovered",
                format!("operationId `{operation}` has no scenario owner"),
            ));
            continue;
        }
        for owner in owners {
            if !declared_scenarios.contains(owner) {
                violations.push(Violation::new(
                    "aex-route-uncovered",
                    format!(
                        "operationId `{operation}` names scenario `{owner}`, which is not \
                         declared in release/scenario-ownership.toml"
                    ),
                ));
            } else if !covered.contains(owner) {
                violations.push(Violation::new(
                    "aex-scenario-orphan",
                    format!(
                        "scenario `{owner}` covers operationId `{operation}` but no package \
                         declares it"
                    ),
                ));
            }
        }
    }
    violations
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
    }
}
