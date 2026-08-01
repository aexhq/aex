//! `selftest` — the router checking its own two invariants at run time.
//!
//! Determinism: the same inputs produce byte-identical canonical output.
//! Monotonicity: widening the graph or the change set never shrinks the
//! selection. Both are proptested at build time, but a CI lane runs `selftest`
//! against the *actual* repository graph, because the property that matters is
//! about this graph, not a generated one.

use crate::error::{Exit, Result, ToolError, Violation};
use crate::graph::inputs::GraphInputs;
use crate::graph::select::{Lane, Mode, Selection, select};
use crate::graph::verify::BuiltGraph;
use crate::graph::{EdgeKind, NodeKind};

/// What `selftest` checked.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SelftestReport {
    /// Schema discriminator.
    pub schema: &'static str,
    /// How many probes ran.
    pub probes: usize,
    /// Node count of the graph probed.
    pub nodes: usize,
}

/// Run the determinism and monotonicity probes against a built graph.
///
/// # Errors
/// Returns [`Exit::SelftestViolation`] naming the probe that failed and the
/// exact inputs that produced the disagreement.
pub fn run(built: &BuiltGraph, inputs: &GraphInputs) -> Result<SelftestReport> {
    let mut violations = Vec::new();
    let mut probes = 0usize;

    // Probe set: every crate node's own directory, one path each, plus the
    // pairwise unions of the first few. Unions are where monotonicity actually
    // gets tested; a single-path selection cannot shrink.
    let seeds: Vec<String> = built
        .graph
        .nodes()
        .iter()
        .filter(|node| matches!(node.kind, NodeKind::Crate | NodeKind::NpmPackage))
        .take(24)
        .map(|node| format!("{}/src/lib.rs", node.dir))
        .collect();

    for seed in &seeds {
        probes += 1;
        let paths = vec![seed.clone()];
        let first = select(built, inputs, &paths, Mode::Affected, Lane::Pr);
        let second = select(built, inputs, &paths, Mode::Affected, Lane::Pr);
        match (first, second) {
            (Ok(first), Ok(second)) => {
                let (Ok(a), Ok(b)) = (
                    crate::canon::to_string(&first),
                    crate::canon::to_string(&second),
                ) else {
                    violations.push(Violation::new(
                        "selftest-canonicalization-failed",
                        format!("a selection over `{seed}` could not be canonicalized"),
                    ));
                    continue;
                };
                if a != b {
                    violations.push(Violation::new(
                        "selftest-nondeterministic",
                        format!(
                            "two selections over the same change set `{seed}` differ; \
                             routing must be a pure function of manifests and paths"
                        ),
                    ));
                }
            }
            (Err(_), Err(_)) => {}
            _ => violations.push(Violation::new(
                "selftest-nondeterministic",
                format!("selecting over `{seed}` succeeded once and failed once"),
            )),
        }
    }

    for window in seeds.windows(2).take(16) {
        probes += 1;
        let single = vec![window[0].clone()];
        let widened = vec![window[0].clone(), window[1].clone()];
        let (Ok(narrow), Ok(wide)) = (
            select(built, inputs, &single, Mode::Affected, Lane::Pr),
            select(built, inputs, &widened, Mode::Affected, Lane::Pr),
        ) else {
            continue;
        };
        if let Some(lost) = first_lost(&narrow, &wide) {
            violations.push(Violation::new(
                "selftest-nonmonotone",
                format!(
                    "adding `{}` to the change set dropped `{lost}` from the selection; \
                     widening a change set may never shrink what runs",
                    window[1]
                ),
            ));
        }
    }

    // The deployment graph never admits a dev edge. A regression here would
    // mint production bytes from a test-only change.
    probes += 1;
    if EdgeKind::CargoDev.in_deploy_graph() {
        violations.push(Violation::new(
            "selftest-deploy-graph-admits-dev-edge",
            "the deployment graph would follow a dev-dependency edge",
        ));
    }

    if violations.is_empty() {
        Ok(SelftestReport {
            schema: "aex.selftest-report.v1",
            probes,
            nodes: built.graph.len(),
        })
    } else {
        Err(ToolError::many(Exit::SelftestViolation, violations))
    }
}

/// The first node the narrower selection holds and the wider one does not.
#[must_use]
pub fn first_lost(narrow: &Selection, wide: &Selection) -> Option<String> {
    let wider: std::collections::BTreeSet<&str> = wide
        .test
        .iter()
        .chain(&wide.deploy)
        .chain(&wide.scenarios)
        .map(|selected| selected.id.as_str())
        .collect();
    narrow
        .test
        .iter()
        .chain(&narrow.deploy)
        .chain(&narrow.scenarios)
        .find(|selected| !wider.contains(selected.id.as_str()))
        .map(|selected| selected.id.to_string())
}
