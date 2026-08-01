//! The one delivery graph: namespaced nodes, CSR adjacency, and the two
//! selections built over it.
//!
//! Four authorities contribute nodes and edges — Cargo, npm, Terraform and the
//! release registries — and they are merged into a single graph rather than
//! four disagreeing implementations of workspace discovery. Dependency edges
//! are never written by hand; the only explicit edge file is
//! `release/scenario-ownership.toml`, because an SDK user test observes a
//! deployable it does not import.

pub mod inputs;
pub mod matrix;
pub mod pathmap;
pub mod select;
pub mod verify;

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::{Exit, Result, ToolError, Violation};

/// A namespaced node identity.
///
/// A directory basename stops being a unique identity the moment crates,
/// npm packages and Terraform modules coexist in one graph, so every id
/// carries its authority as a prefix.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub String);

impl NodeId {
    /// A Cargo workspace member.
    #[must_use]
    pub fn cargo(name: &str) -> Self {
        Self(format!("cargo:{name}"))
    }

    /// An npm workspace package, by its package name.
    #[must_use]
    pub fn npm(name: &str) -> Self {
        Self(format!("npm:{name}"))
    }

    /// A Terraform module or root, by its repository-relative directory below
    /// `infra/`.
    #[must_use]
    pub fn terraform(relative_dir: &str) -> Self {
        Self(format!("tf:{relative_dir}"))
    }

    /// A user or release scenario.
    #[must_use]
    pub fn scenario(id: &str) -> Self {
        Self(format!("scenario:{id}"))
    }

    /// A deployable artifact.
    #[must_use]
    pub fn artifact(unit: &str) -> Self {
        Self(format!("artifact:{unit}"))
    }

    /// A generated bundle: contract, migration or infrastructure modules.
    #[must_use]
    pub fn bundle(name: &str) -> Self {
        Self(format!("bundle:{name}"))
    }

    /// The authority prefix, without the colon.
    #[must_use]
    pub fn namespace(&self) -> &str {
        self.0.split_once(':').map_or("", |(prefix, _)| prefix)
    }

    /// The identity within its authority.
    #[must_use]
    pub fn local(&self) -> &str {
        self.0
            .split_once(':')
            .map_or(self.0.as_str(), |(_, rest)| rest)
    }

    /// The raw string form.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a node is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NodeKind {
    /// A Cargo workspace member.
    Crate,
    /// An npm workspace package.
    NpmPackage,
    /// A reusable Terraform module under `infra/modules/`.
    TerraformModule,
    /// An example Terraform root under `infra/examples/`.
    TerraformRoot,
    /// A user or release scenario.
    Scenario,
    /// A deployable artifact recipe.
    Artifact,
    /// A generated bundle.
    Bundle,
}

/// How one node depends on another. Direction is always dependent → dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EdgeKind {
    /// A Cargo normal dependency: linked into the dependent's own targets.
    CargoNormal,
    /// A Cargo build-script dependency.
    CargoBuild,
    /// A Cargo dev-dependency: test graph only.
    CargoDev,
    /// An npm runtime dependency.
    NpmRuntime,
    /// An npm dev dependency. Bundled at build time, so it is also a
    /// deployment edge.
    NpmDev,
    /// A Terraform `module` block.
    TerraformModule,
    /// An artifact's input closure member.
    ArtifactInput,
    /// A scenario observing an artifact or crate without importing it.
    ScenarioObserves,
}

impl EdgeKind {
    /// Whether this edge participates in test-graph selection.
    #[must_use]
    pub const fn in_test_graph(self) -> bool {
        true
    }

    /// Whether this edge participates in deployment-graph selection.
    ///
    /// `CargoDev` is excluded: a test-only dependency never reaches production
    /// bytes. `TerraformModule` and `ScenarioObserves` are excluded because
    /// neither mints an artifact.
    #[must_use]
    pub const fn in_deploy_graph(self) -> bool {
        matches!(
            self,
            Self::CargoNormal
                | Self::CargoBuild
                | Self::NpmRuntime
                | Self::NpmDev
                | Self::ArtifactInput
        )
    }

    /// The cycle-detection class. Cycles are illegal within a class; a normal
    /// edge that closes a loop through a dev edge is legal and common.
    #[must_use]
    pub const fn cycle_class(self) -> u8 {
        match self {
            Self::CargoNormal | Self::CargoBuild => 0,
            Self::CargoDev => 1,
            Self::NpmRuntime | Self::NpmDev => 2,
            Self::TerraformModule => 3,
            Self::ArtifactInput => 4,
            Self::ScenarioObserves => 5,
        }
    }
}

/// One graph node with the ownership metadata that classified it.
#[derive(Debug, Clone, Serialize)]
pub struct Node {
    /// Namespaced identity.
    pub id: NodeId,
    /// What it is.
    pub kind: NodeKind,
    /// Repository-relative directory, `/`-separated, no trailing slash.
    pub dir: String,
    /// Ownership metadata, absent only when the node has no manifest slot.
    pub meta: Option<crate::meta::AexMeta>,
    /// Whether this node is published outside the repository.
    pub publishable: bool,
}

/// An edge before indices are assigned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawEdge {
    /// The dependent.
    pub from: NodeId,
    /// The dependency.
    pub to: NodeId,
    /// How it depends.
    pub kind: EdgeKind,
}

/// Compressed sparse row adjacency.
///
/// The predecessor implementation ran a naive `O(V·E)` fixpoint with no
/// reverse index. Materializing both directions once turns every selection
/// into one breadth-first search.
#[derive(Debug, Clone, Default)]
pub struct Csr {
    offsets: Vec<u32>,
    targets: Vec<u32>,
    kinds: Vec<EdgeKind>,
}

impl Csr {
    fn build(node_count: usize, mut pairs: Vec<(u32, u32, EdgeKind)>) -> Self {
        pairs.sort_unstable();
        pairs.dedup();
        let mut offsets = vec![0u32; node_count + 1];
        for (from, _, _) in &pairs {
            offsets[*from as usize + 1] += 1;
        }
        for index in 0..node_count {
            offsets[index + 1] += offsets[index];
        }
        let mut targets = vec![0u32; pairs.len()];
        let mut kinds = vec![EdgeKind::CargoNormal; pairs.len()];
        let mut cursor = offsets.clone();
        for (from, to, kind) in pairs {
            let slot = cursor[from as usize] as usize;
            targets[slot] = to;
            kinds[slot] = kind;
            cursor[from as usize] += 1;
        }
        Self {
            offsets,
            targets,
            kinds,
        }
    }

    /// The outgoing `(target, kind)` pairs of one node.
    pub fn neighbours(&self, node: u32) -> impl Iterator<Item = (u32, EdgeKind)> + '_ {
        let start = self.offsets[node as usize] as usize;
        let end = self.offsets[node as usize + 1] as usize;
        (start..end).map(move |slot| (self.targets[slot], self.kinds[slot]))
    }

    /// Total edge count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.targets.len()
    }

    /// Whether the adjacency holds no edge.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }
}

/// The merged delivery graph.
#[derive(Debug, Clone)]
pub struct Graph {
    nodes: Vec<Node>,
    forward: Csr,
    reverse: Csr,
    index: BTreeMap<NodeId, u32>,
}

impl Graph {
    /// Merge nodes and edges into one indexed graph.
    ///
    /// # Errors
    /// Returns [`Exit::GraphVerification`] when an edge names an unknown node,
    /// when a node id is declared twice, or when any edge class contains a
    /// cycle.
    pub fn new(nodes: Vec<Node>, edges: &[RawEdge]) -> Result<Self> {
        let mut index = BTreeMap::new();
        let mut violations = Vec::new();
        for (position, node) in nodes.iter().enumerate() {
            let slot = u32::try_from(position).unwrap_or(u32::MAX);
            if index.insert(node.id.clone(), slot).is_some() {
                violations.push(Violation::new(
                    "duplicate-node",
                    format!("node `{}` is declared twice", node.id),
                ));
            }
        }
        let mut forward_pairs = Vec::with_capacity(edges.len());
        let mut reverse_pairs = Vec::with_capacity(edges.len());
        for edge in edges {
            let (Some(from), Some(to)) = (index.get(&edge.from), index.get(&edge.to)) else {
                let missing = if index.contains_key(&edge.from) {
                    &edge.to
                } else {
                    &edge.from
                };
                violations.push(Violation::new(
                    "unknown-node-reference",
                    format!(
                        "edge `{}` -> `{}` names `{missing}`, which is not a graph node",
                        edge.from, edge.to
                    ),
                ));
                continue;
            };
            forward_pairs.push((*from, *to, edge.kind));
            reverse_pairs.push((*to, *from, edge.kind));
        }
        if !violations.is_empty() {
            return Err(ToolError::many(Exit::GraphVerification, violations));
        }
        let graph = Self {
            forward: Csr::build(nodes.len(), forward_pairs),
            reverse: Csr::build(nodes.len(), reverse_pairs),
            nodes,
            index,
        };
        graph.assert_acyclic()?;
        Ok(graph)
    }

    /// Every node, in declaration order.
    #[must_use]
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    /// Forward adjacency: dependent → dependency.
    #[must_use]
    pub fn forward(&self) -> &Csr {
        &self.forward
    }

    /// Reverse adjacency: dependency → dependent.
    #[must_use]
    pub fn reverse(&self) -> &Csr {
        &self.reverse
    }

    /// Resolve an id to its slot.
    #[must_use]
    pub fn slot(&self, id: &NodeId) -> Option<u32> {
        self.index.get(id).copied()
    }

    /// The node at a slot.
    #[must_use]
    pub fn node(&self, slot: u32) -> &Node {
        &self.nodes[slot as usize]
    }

    /// Node count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the graph holds no node.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Breadth-first closure over `reverse`, i.e. everything that depends on
    /// the seeds, restricted to the edge classes `accept` admits.
    ///
    /// Returns each reached node with the seed, edge class and hop count that
    /// first reached it, so a selection can explain itself.
    #[must_use]
    pub fn reverse_closure(
        &self,
        seeds: &[u32],
        accept: fn(EdgeKind) -> bool,
    ) -> Vec<(u32, u32, EdgeKind, u16)> {
        let mut seen = vec![false; self.nodes.len()];
        let mut reached = Vec::new();
        let mut queue: std::collections::VecDeque<(u32, u32, u16)> = seeds
            .iter()
            .map(|seed| {
                seen[*seed as usize] = true;
                (*seed, *seed, 0)
            })
            .collect();
        while let Some((current, origin, hops)) = queue.pop_front() {
            for (dependent, kind) in self.reverse.neighbours(current) {
                if !accept(kind) || seen[dependent as usize] {
                    continue;
                }
                seen[dependent as usize] = true;
                reached.push((dependent, origin, kind, hops + 1));
                queue.push_back((dependent, origin, hops + 1));
            }
        }
        reached
    }

    /// Breadth-first closure over `forward`, i.e. everything the seeds depend
    /// on, restricted to the edge classes `accept` admits.
    #[must_use]
    pub fn forward_closure(&self, seeds: &[u32], accept: fn(EdgeKind) -> bool) -> Vec<u32> {
        let mut seen = vec![false; self.nodes.len()];
        let mut reached = Vec::new();
        let mut queue: std::collections::VecDeque<u32> = seeds
            .iter()
            .map(|seed| {
                seen[*seed as usize] = true;
                *seed
            })
            .collect();
        while let Some(current) = queue.pop_front() {
            for (dependency, kind) in self.forward.neighbours(current) {
                if !accept(kind) || seen[dependency as usize] {
                    continue;
                }
                seen[dependency as usize] = true;
                reached.push(dependency);
                queue.push_back(dependency);
            }
        }
        reached
    }

    fn assert_acyclic(&self) -> Result<()> {
        let mut violations = Vec::new();
        for class in 0u8..=5 {
            if let Some(cycle) = self.find_cycle(class) {
                let rendered: Vec<String> = cycle
                    .iter()
                    .map(|slot| self.nodes[*slot as usize].id.to_string())
                    .collect();
                violations.push(Violation::new(
                    "graph-cycle",
                    format!("dependency cycle: {}", rendered.join(" -> ")),
                ));
            }
        }
        if violations.is_empty() {
            Ok(())
        } else {
            Err(ToolError::many(Exit::GraphVerification, violations))
        }
    }

    fn find_cycle(&self, class: u8) -> Option<Vec<u32>> {
        #[derive(Clone, Copy, PartialEq)]
        enum Mark {
            White,
            Grey,
            Black,
        }
        let mut marks = vec![Mark::White; self.nodes.len()];
        let mut stack: Vec<u32> = Vec::new();
        for start in 0..self.nodes.len() {
            let start = u32::try_from(start).unwrap_or(u32::MAX);
            if marks[start as usize] != Mark::White {
                continue;
            }
            // Iterative depth-first search: an explicit frame stack keeps a
            // deep dependency chain from overflowing the thread stack.
            let mut frames: Vec<(u32, Vec<u32>)> = vec![(
                start,
                self.forward
                    .neighbours(start)
                    .filter(|(_, kind)| kind.cycle_class() == class)
                    .map(|(target, _)| target)
                    .collect(),
            )];
            marks[start as usize] = Mark::Grey;
            stack.push(start);
            while let Some((node, pending)) = frames.last_mut() {
                if let Some(next) = pending.pop() {
                    match marks[next as usize] {
                        Mark::Grey => {
                            let position = stack.iter().position(|slot| *slot == next).unwrap_or(0);
                            let mut cycle = stack[position..].to_vec();
                            cycle.push(next);
                            return Some(cycle);
                        }
                        Mark::Black => {}
                        Mark::White => {
                            marks[next as usize] = Mark::Grey;
                            stack.push(next);
                            frames.push((
                                next,
                                self.forward
                                    .neighbours(next)
                                    .filter(|(_, kind)| kind.cycle_class() == class)
                                    .map(|(target, _)| target)
                                    .collect(),
                            ));
                        }
                    }
                } else {
                    marks[*node as usize] = Mark::Black;
                    stack.pop();
                    frames.pop();
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{EdgeKind, Graph, Node, NodeId, NodeKind, RawEdge};

    fn node(id: NodeId, dir: &str) -> Node {
        Node {
            id,
            kind: NodeKind::Crate,
            dir: dir.to_owned(),
            meta: None,
            publishable: false,
        }
    }

    fn chain() -> Graph {
        // wire <- contracts <- sdk <- cli, written dependent -> dependency.
        let nodes = vec![
            node(NodeId::cargo("aex-wire"), "crates/aex-wire"),
            node(
                NodeId::cargo("aex-internal-contracts"),
                "crates/aex-internal-contracts",
            ),
            node(NodeId::npm("@aexhq/sdk"), "packages/sdk"),
            node(NodeId::cargo("aex-cli"), "tools/aex-cli"),
        ];
        let edges = vec![
            RawEdge {
                from: NodeId::cargo("aex-internal-contracts"),
                to: NodeId::cargo("aex-wire"),
                kind: EdgeKind::CargoNormal,
            },
            RawEdge {
                from: NodeId::npm("@aexhq/sdk"),
                to: NodeId::cargo("aex-wire"),
                kind: EdgeKind::NpmRuntime,
            },
            RawEdge {
                from: NodeId::cargo("aex-cli"),
                to: NodeId::cargo("aex-internal-contracts"),
                kind: EdgeKind::CargoNormal,
            },
        ];
        Graph::new(nodes, &edges).unwrap()
    }

    #[test]
    fn node_ids_are_namespaced_and_decompose() {
        let id = NodeId::cargo("aex-wire");
        assert_eq!(id.as_str(), "cargo:aex-wire");
        assert_eq!(id.namespace(), "cargo");
        assert_eq!(id.local(), "aex-wire");
        assert_eq!(NodeId::npm("@aexhq/sdk").local(), "@aexhq/sdk");
        assert_eq!(
            NodeId::terraform("modules/kms-key").as_str(),
            "tf:modules/kms-key"
        );
    }

    #[test]
    fn a_directory_basename_collision_across_authorities_is_two_nodes() {
        assert_ne!(NodeId::cargo("sdk"), NodeId::npm("sdk"));
    }

    #[test]
    fn reverse_closure_reaches_every_dependent_with_a_hop_count() {
        let graph = chain();
        let wire = graph.slot(&NodeId::cargo("aex-wire")).unwrap();
        let reached = graph.reverse_closure(&[wire], EdgeKind::in_test_graph);
        let mut ids: Vec<String> = reached
            .iter()
            .map(|(slot, _, _, _)| graph.node(*slot).id.to_string())
            .collect();
        ids.sort();
        assert_eq!(
            ids,
            vec![
                "cargo:aex-cli",
                "cargo:aex-internal-contracts",
                "npm:@aexhq/sdk"
            ]
        );
        let cli = reached
            .iter()
            .find(|(slot, _, _, _)| graph.node(*slot).id == NodeId::cargo("aex-cli"))
            .unwrap();
        assert_eq!(cli.3, 2, "cli is two hops from wire");
    }

    #[test]
    fn forward_closure_reaches_every_dependency() {
        let graph = chain();
        let cli = graph.slot(&NodeId::cargo("aex-cli")).unwrap();
        let mut ids: Vec<String> = graph
            .forward_closure(&[cli], EdgeKind::in_test_graph)
            .iter()
            .map(|slot| graph.node(*slot).id.to_string())
            .collect();
        ids.sort();
        assert_eq!(ids, vec!["cargo:aex-internal-contracts", "cargo:aex-wire"]);
    }

    #[test]
    fn dev_edges_are_excluded_from_the_deployment_graph() {
        assert!(!EdgeKind::CargoDev.in_deploy_graph());
        assert!(EdgeKind::CargoDev.in_test_graph());
        assert!(EdgeKind::ArtifactInput.in_deploy_graph());
        assert!(!EdgeKind::ScenarioObserves.in_deploy_graph());
        assert!(!EdgeKind::TerraformModule.in_deploy_graph());
    }

    #[test]
    fn a_cycle_within_one_edge_class_is_rejected() {
        let nodes = vec![
            node(NodeId::cargo("a"), "crates/a"),
            node(NodeId::cargo("b"), "crates/b"),
        ];
        let edges = vec![
            RawEdge {
                from: NodeId::cargo("a"),
                to: NodeId::cargo("b"),
                kind: EdgeKind::CargoNormal,
            },
            RawEdge {
                from: NodeId::cargo("b"),
                to: NodeId::cargo("a"),
                kind: EdgeKind::CargoNormal,
            },
        ];
        let err = Graph::new(nodes, &edges).unwrap_err();
        assert_eq!(err.rules(), vec!["graph-cycle"]);
        assert_eq!(err.exit.code(), 10);
    }

    #[test]
    fn a_loop_closed_through_a_dev_edge_is_legal() {
        let nodes = vec![
            node(NodeId::cargo("a"), "crates/a"),
            node(NodeId::cargo("b"), "crates/b"),
        ];
        let edges = vec![
            RawEdge {
                from: NodeId::cargo("a"),
                to: NodeId::cargo("b"),
                kind: EdgeKind::CargoNormal,
            },
            RawEdge {
                from: NodeId::cargo("b"),
                to: NodeId::cargo("a"),
                kind: EdgeKind::CargoDev,
            },
        ];
        Graph::new(nodes, &edges).expect("cargo permits this and so must the graph");
    }

    #[test]
    fn an_edge_naming_an_unknown_node_is_rejected() {
        let nodes = vec![node(NodeId::cargo("a"), "crates/a")];
        let edges = vec![RawEdge {
            from: NodeId::cargo("a"),
            to: NodeId::cargo("ghost"),
            kind: EdgeKind::CargoNormal,
        }];
        let err = Graph::new(nodes, &edges).unwrap_err();
        assert_eq!(err.rules(), vec!["unknown-node-reference"]);
    }

    #[test]
    fn a_duplicate_node_id_is_rejected() {
        let nodes = vec![
            node(NodeId::cargo("a"), "crates/a"),
            node(NodeId::cargo("a"), "crates/a-again"),
        ];
        let err = Graph::new(nodes, &[]).unwrap_err();
        assert_eq!(err.rules(), vec!["duplicate-node"]);
    }

    #[test]
    fn deep_chains_do_not_overflow_the_cycle_check() {
        let depth = 20_000;
        let nodes: Vec<_> = (0..depth)
            .map(|i| node(NodeId::cargo(&format!("c{i}")), &format!("crates/c{i}")))
            .collect();
        let edges: Vec<_> = (0..depth - 1)
            .map(|i| RawEdge {
                from: NodeId::cargo(&format!("c{i}")),
                to: NodeId::cargo(&format!("c{}", i + 1)),
                kind: EdgeKind::CargoNormal,
            })
            .collect();
        let graph = Graph::new(nodes, &edges).unwrap();
        assert_eq!(graph.len(), depth);
    }
}
