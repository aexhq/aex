//! `graph select` — what runs, and why.
//!
//! Two selections share the nodes and diverge on edges. The test graph takes
//! the reverse closure of everything changed, including dev-dependency and
//! Terraform edges, and adds every scenario observing that set. The deployment
//! graph selects artifacts whose input closure changed, excludes dev edges, and
//! excludes test, benchmark, example and documentation paths from every input
//! closure — so a test-only edit retests everything it should and mints no
//! production bytes.
//!
//! Every selected node carries exactly one reason. "Why did this run" is not a
//! question anybody should have to answer by reading the router's source.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::error::{Exit, Result, ToolError};

use super::pathmap::Classification;
use super::verify::BuiltGraph;
use super::{EdgeKind, NodeId, NodeKind};

/// Which lane is asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[clap(rename_all = "kebab-case")]
pub enum Lane {
    /// Pull request and merge queue.
    Pr,
    /// Protected main build and publication.
    Main,
    /// Scheduled assurance.
    Assurance,
    /// Explicit environment release.
    Release,
}

/// How wide to select.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[clap(rename_all = "kebab-case")]
pub enum Mode {
    /// Only what the change reaches.
    Affected,
    /// Everything.
    Full,
    /// Run everything, record what `affected` would have omitted.
    Shadow,
}

/// Why one node was selected. First match wins, in declaration order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum SelectionReason {
    /// A file this node owns changed.
    ChangedSource {
        /// The changed path.
        path: String,
    },
    /// This node depends on something that changed.
    ReverseDependency {
        /// The changed node reached.
        of: NodeId,
        /// Through which edge class.
        edge: EdgeKind,
        /// How many hops away.
        hops: u16,
    },
    /// A path inside this artifact's input closure changed.
    ArtifactInput {
        /// The artifact.
        artifact: NodeId,
        /// The changed path.
        path: String,
    },
    /// A scenario observes a selected node.
    ScenarioEdge {
        /// The scenario.
        scenario: NodeId,
        /// What it observes.
        observes: NodeId,
    },
    /// Publication re-expands forward over the publishable subset.
    PublishClosure {
        /// The downstream package that forced it.
        downstream: NodeId,
    },
    /// A repo-wide trigger fired.
    RepoWide {
        /// Which trigger.
        trigger: String,
    },
    /// A standing policy always selects this node.
    Evergreen {
        /// Which policy.
        policy: String,
    },
    /// The router itself changed, so the full graph runs.
    RouterChanged,
    /// Explicitly requested.
    Requested {
        /// Who asked.
        by: String,
    },
}

/// One selected node and its single reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selected {
    /// The node.
    pub id: NodeId,
    /// Why it was selected.
    #[serde(flatten)]
    pub reason: SelectionReason,
}

/// What shadow mode observed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shadow {
    /// Nodes the full graph holds that the affected selection omitted.
    pub delta: Vec<NodeId>,
    /// Nodes that failed in the full run and were not selected. A non-empty
    /// list is a proven routing bug, not a discrepancy to triage later.
    #[serde(default)]
    pub unselected_failures: Vec<NodeId>,
}

/// The complete routing decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Selection {
    /// Schema discriminator.
    pub schema: String,
    /// Which lane asked.
    pub lane: Lane,
    /// How wide the selection was taken.
    pub mode: Mode,
    /// Test-graph selection.
    pub test: Vec<Selected>,
    /// Deployment-graph selection.
    pub deploy: Vec<Selected>,
    /// Scenario selection.
    pub scenarios: Vec<Selected>,
    /// Whether a repo-wide trigger fired.
    pub repo_wide: bool,
    /// Paths matching no ownership rule.
    pub unowned: Vec<String>,
    /// How many paths the caller supplied.
    pub changed_paths: usize,
    /// Whether routing could not be computed.
    pub routing_failed: bool,
    /// Why, when it could not.
    pub routing_failure_reason: Option<String>,
    /// Shadow-mode observations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shadow: Option<Shadow>,
}

impl Selection {
    /// Whether the selection chose nothing at all. An empty matrix must emit a
    /// real `no-affected-unit` receipt, never a green skipped required job.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.test.is_empty() && self.deploy.is_empty() && self.scenarios.is_empty()
    }
}

/// Paths excluded from every artifact input closure.
///
/// A change under any of these retests and mints no production bytes.
const NON_ARTIFACT_SEGMENTS: &[&str] = &[
    "/tests/",
    "/benches/",
    "/examples/",
    "/fuzz/",
    "apps/user-tests/",
    "conformance/",
    "docs/",
    "references/",
    "tests/live/",
    "tests/load/",
    "tests/support/",
];

/// Paths included in every artifact input closure regardless of where they sit.
const ALWAYS_ARTIFACT_INPUT: &[&str] = &[
    "Cargo.lock",
    "Cargo.toml",
    "rust-toolchain.toml",
    ".cargo/config.toml",
];

/// Whether a repository path can reach production bytes.
#[must_use]
pub fn is_artifact_input(path: &str) -> bool {
    if ALWAYS_ARTIFACT_INPUT.contains(&path) {
        return true;
    }
    if path.contains("/src/generated/") || path.ends_with("/build.rs") {
        return true;
    }
    !NON_ARTIFACT_SEGMENTS
        .iter()
        .any(|segment| path.contains(segment) || path.starts_with(segment.trim_start_matches('/')))
}

/// Compute the routing decision.
///
/// # Errors
/// Returns [`Exit::RoutingUndecidable`] when the caller asked for an affected
/// selection without supplying any change information at all — an empty change
/// set and "we do not know what changed" are different states, and conflating
/// them is how a router silently runs nothing.
#[allow(clippy::too_many_lines)]
pub fn select(
    built: &BuiltGraph,
    inputs: &super::inputs::GraphInputs,
    changed: &[String],
    mode: Mode,
    lane: Lane,
) -> Result<Selection> {
    let npm_dirs = inputs.npm_dirs();
    let mut repo_wide = false;
    let mut router_changed = false;
    let mut unowned = Vec::new();
    let mut changed_seeds: BTreeMap<u32, String> = BTreeMap::new();
    let mut artifact_input_paths: Vec<String> = Vec::new();

    for path in changed {
        match inputs.path_map.classify(path, &npm_dirs) {
            Classification::Owned { node, .. } => {
                if let Some(slot) = built.graph.slot(&node) {
                    changed_seeds.entry(slot).or_insert_with(|| path.clone());
                } else {
                    unowned.push(path.clone());
                }
            }
            Classification::RepoWide { .. } => repo_wide = true,
            Classification::Router { .. } => {
                router_changed = true;
                repo_wide = true;
            }
            Classification::Ignored { .. } => {}
            Classification::Orphan => {
                unowned.push(path.clone());
                repo_wide = true;
            }
        }
        if is_artifact_input(path) {
            artifact_input_paths.push(path.clone());
        }
    }

    let effective_mode = if router_changed && mode == Mode::Affected {
        Mode::Full
    } else {
        mode
    };
    let full = effective_mode == Mode::Full || effective_mode == Mode::Shadow || repo_wide;

    let mut test: BTreeMap<NodeId, SelectionReason> = BTreeMap::new();
    let mut deploy: BTreeMap<NodeId, SelectionReason> = BTreeMap::new();
    let mut scenarios: BTreeMap<NodeId, SelectionReason> = BTreeMap::new();

    if full {
        let reason = if router_changed {
            SelectionReason::RouterChanged
        } else if repo_wide {
            SelectionReason::RepoWide {
                trigger: "a repo-wide or unowned path changed".to_owned(),
            }
        } else {
            SelectionReason::Evergreen {
                policy: format!("mode = {effective_mode:?}"),
            }
        };
        for node in built.graph.nodes() {
            match node.kind {
                NodeKind::Artifact => {
                    deploy.insert(node.id.clone(), reason.clone());
                }
                NodeKind::Scenario => {
                    scenarios.insert(node.id.clone(), reason.clone());
                }
                _ => {
                    test.insert(node.id.clone(), reason.clone());
                }
            }
        }
    }

    if !full || effective_mode == Mode::Shadow {
        let seeds: Vec<u32> = changed_seeds.keys().copied().collect();
        let mut affected_test: BTreeMap<NodeId, SelectionReason> = BTreeMap::new();
        let mut affected_deploy: BTreeMap<NodeId, SelectionReason> = BTreeMap::new();
        let mut affected_scenarios: BTreeMap<NodeId, SelectionReason> = BTreeMap::new();

        for (slot, path) in &changed_seeds {
            let node = built.graph.node(*slot);
            let reason = SelectionReason::ChangedSource { path: path.clone() };
            match node.kind {
                NodeKind::Artifact => {
                    affected_deploy.insert(node.id.clone(), reason);
                }
                NodeKind::Scenario => {
                    affected_scenarios.insert(node.id.clone(), reason);
                }
                _ => {
                    affected_test.insert(node.id.clone(), reason);
                }
            }
        }

        for (slot, origin, edge, hops) in
            built.graph.reverse_closure(&seeds, EdgeKind::in_test_graph)
        {
            let node = built.graph.node(slot);
            let reason = SelectionReason::ReverseDependency {
                of: built.graph.node(origin).id.clone(),
                edge,
                hops,
            };
            match node.kind {
                NodeKind::Artifact => {
                    affected_deploy.entry(node.id.clone()).or_insert(reason);
                }
                NodeKind::Scenario => {
                    affected_scenarios.entry(node.id.clone()).or_insert(reason);
                }
                _ => {
                    affected_test.entry(node.id.clone()).or_insert(reason);
                }
            }
        }

        // The deployment graph is recomputed rather than filtered: a
        // test-only path never enters an artifact's input closure, so an
        // artifact reached only through a test path is dropped here.
        let artifact_seeds: Vec<u32> = changed_seeds
            .iter()
            .filter(|(_, path)| is_artifact_input(path))
            .map(|(slot, _)| *slot)
            .collect();
        let mut deploy_reasons: BTreeMap<NodeId, SelectionReason> = BTreeMap::new();
        for slot in &artifact_seeds {
            let node = built.graph.node(*slot);
            if node.kind == NodeKind::Artifact {
                deploy_reasons.insert(
                    node.id.clone(),
                    SelectionReason::ArtifactInput {
                        artifact: node.id.clone(),
                        path: changed_seeds[slot].clone(),
                    },
                );
            }
        }
        for (slot, origin, _, _) in built
            .graph
            .reverse_closure(&artifact_seeds, EdgeKind::in_deploy_graph)
        {
            let node = built.graph.node(slot);
            if node.kind != NodeKind::Artifact {
                continue;
            }
            deploy_reasons.entry(node.id.clone()).or_insert_with(|| {
                SelectionReason::ArtifactInput {
                    artifact: node.id.clone(),
                    path: changed_seeds
                        .get(&origin)
                        .cloned()
                        .unwrap_or_else(|| "(closure)".to_owned()),
                }
            });
        }
        affected_deploy = deploy_reasons;

        // Publication re-expands forward over the publishable subset: an SDK
        // change publishes the CLI that embeds it.
        let publish_seeds: Vec<u32> = affected_test
            .keys()
            .filter_map(|id| built.graph.slot(id))
            .filter(|slot| built.graph.node(*slot).publishable)
            .collect();
        for slot in built
            .graph
            .forward_closure(&publish_seeds, EdgeKind::in_deploy_graph)
        {
            let node = built.graph.node(slot);
            if !node.publishable || node.kind == NodeKind::Artifact {
                continue;
            }
            affected_test.entry(node.id.clone()).or_insert_with(|| {
                SelectionReason::PublishClosure {
                    downstream: node.id.clone(),
                }
            });
        }

        // Scenarios observing anything selected are selected.
        let observed: BTreeSet<&NodeId> =
            affected_test.keys().chain(affected_deploy.keys()).collect();
        for scenario in &inputs.scenarios.scenarios {
            for target in &scenario.observes {
                let target = NodeId(target.clone());
                if observed.contains(&target) {
                    affected_scenarios
                        .entry(NodeId::scenario(&scenario.id))
                        .or_insert(SelectionReason::ScenarioEdge {
                            scenario: NodeId::scenario(&scenario.id),
                            observes: target,
                        });
                    break;
                }
            }
        }

        if effective_mode == Mode::Shadow {
            let selected: BTreeSet<&NodeId> = affected_test
                .keys()
                .chain(affected_deploy.keys())
                .chain(affected_scenarios.keys())
                .collect();
            let delta: Vec<NodeId> = test
                .keys()
                .chain(deploy.keys())
                .chain(scenarios.keys())
                .filter(|id| !selected.contains(id))
                .cloned()
                .collect();
            return Ok(Selection {
                schema: "aex.selection.v1".to_owned(),
                lane,
                mode: effective_mode,
                test: flatten(test),
                deploy: flatten(deploy),
                scenarios: flatten(scenarios),
                repo_wide,
                unowned,
                changed_paths: changed.len(),
                routing_failed: false,
                routing_failure_reason: None,
                shadow: Some(Shadow {
                    delta,
                    unselected_failures: Vec::new(),
                }),
            });
        }

        test = affected_test;
        deploy = affected_deploy;
        scenarios = affected_scenarios;
    }

    if !full && changed.is_empty() {
        return Err(ToolError::single(
            Exit::RoutingUndecidable,
            "routing-no-change-information",
            "an affected selection was requested with no changed paths; an empty change \
             set and an unknown change set are different states",
        ));
    }

    Ok(Selection {
        schema: "aex.selection.v1".to_owned(),
        lane,
        mode: effective_mode,
        test: flatten(test),
        deploy: flatten(deploy),
        scenarios: flatten(scenarios),
        repo_wide,
        unowned,
        changed_paths: changed.len(),
        routing_failed: false,
        routing_failure_reason: None,
        shadow: None,
    })
}

fn flatten(map: BTreeMap<NodeId, SelectionReason>) -> Vec<Selected> {
    map.into_iter()
        .map(|(id, reason)| Selected { id, reason })
        .collect()
}

/// Render a selection as the pull-request reason table.
#[must_use]
pub fn to_markdown(selection: &Selection) -> String {
    use std::fmt::Write as _;

    let mut out = String::from("### Delivery routing\n\n");
    let _ = writeln!(
        out,
        "- mode: `{:?}` · lane: `{:?}` · changed paths: {} · repo-wide: {}",
        selection.mode, selection.lane, selection.changed_paths, selection.repo_wide
    );
    let _ = writeln!(
        out,
        "- selected: {} test · {} artifact · {} scenario\n",
        selection.test.len(),
        selection.deploy.len(),
        selection.scenarios.len()
    );
    if !selection.unowned.is_empty() {
        out.push_str("**Unowned paths widened this run to the whole repository:**\n\n");
        for path in &selection.unowned {
            let _ = writeln!(out, "- `{path}`");
        }
        out.push('\n');
    }
    out.push_str("| Node | Reason |\n| --- | --- |\n");
    for selected in selection
        .test
        .iter()
        .chain(&selection.deploy)
        .chain(&selection.scenarios)
    {
        let _ = writeln!(
            out,
            "| `{}` | {} |",
            selected.id,
            describe(&selected.reason)
        );
    }
    out
}

/// One-line description of a selection reason.
#[must_use]
pub fn describe(reason: &SelectionReason) -> String {
    match reason {
        SelectionReason::ChangedSource { path } => format!("changed source `{path}`"),
        SelectionReason::ReverseDependency { of, edge, hops } => {
            format!("depends on `{of}` ({edge:?}, {hops} hop(s))")
        }
        SelectionReason::ArtifactInput { artifact, path } => {
            format!("`{path}` is in the input closure of `{artifact}`")
        }
        SelectionReason::ScenarioEdge { scenario, observes } => {
            format!("`{scenario}` observes `{observes}`")
        }
        SelectionReason::PublishClosure { downstream } => {
            format!("publication closure of `{downstream}`")
        }
        SelectionReason::RepoWide { trigger } => format!("repo-wide: {trigger}"),
        SelectionReason::Evergreen { policy } => format!("evergreen: {policy}"),
        SelectionReason::RouterChanged => {
            "the router itself changed, so the full graph runs".to_owned()
        }
        SelectionReason::Requested { by } => format!("requested by {by}"),
    }
}
