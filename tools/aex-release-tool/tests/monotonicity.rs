//! Selection is monotone and deterministic.
//!
//! Monotonicity is the property that makes an affected selection safe:
//! widening the workspace or the change set may add nodes to the run and may
//! never remove one. A router that can shrink under a wider input is a router
//! that will one day skip the test that would have caught the regression.
//!
//! Shrinking reports the minimal manifest pair that reduced the closure.

mod common;

use aex_release_tool::canon;
use aex_release_tool::graph::inputs::GraphInputs;
use aex_release_tool::graph::select::{Lane, Mode, select};
use aex_release_tool::graph::verify;
use common::{CratePlan, Fixture};
use proptest::prelude::*;
use std::collections::BTreeSet;

/// A synthetic workspace: `size` crates, each depending on a subset of the
/// crates before it, so the graph is acyclic by construction.
fn workspace(size: usize, edges: &[(usize, usize)]) -> std::path::PathBuf {
    let mut fixture = Fixture::new();
    for index in 0..size {
        let name = format!("aex-c{index}");
        let mut plan = CratePlan::new(&name, &format!("crates/{name}"));
        for (from, to) in edges {
            if *from == index && to < from {
                plan = plan.dep(&format!("aex-c{to}"));
            }
        }
        fixture = fixture.add_crate(plan);
    }
    fixture.build()
}

fn selected(root: &std::path::Path, changed: &[String]) -> BTreeSet<String> {
    let inputs = GraphInputs::load(root).expect("inputs");
    let built = verify::build(&inputs).expect("graph");
    match select(&built, &inputs, changed, Mode::Affected, Lane::Pr) {
        Ok(selection) => selection
            .test
            .iter()
            .chain(&selection.deploy)
            .chain(&selection.scenarios)
            .map(|item| item.id.to_string())
            .collect(),
        Err(_) => BTreeSet::new(),
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]

    /// Adding a dependency edge never removes a node from the selection.
    #[test]
    fn adding_an_edge_never_shrinks_the_selection(
        size in 3usize..8,
        raw_edges in prop::collection::vec((0usize..8, 0usize..8), 0..12),
        seed in 0usize..8,
    ) {
        prop_assume!(seed < size);
        let edges: Vec<(usize, usize)> = raw_edges
            .iter()
            .filter(|(from, to)| *from < size && *to < *from)
            .copied()
            .collect();
        let changed = vec![format!("crates/aex-c{seed}/src/lib.rs")];

        let before = selected(&workspace(size, &edges), &changed);

        // The widening mutation: one more edge into the seed's dependents.
        let mut widened = edges.clone();
        if size > 1 {
            widened.push((size - 1, 0));
        }
        let after = selected(&workspace(size, &widened), &changed);

        let lost: Vec<&String> = before.difference(&after).collect();
        prop_assert!(
            lost.is_empty(),
            "adding an edge dropped {lost:?} (size {size}, edges {edges:?})"
        );
    }

    /// Adding a changed path never removes a node from the selection.
    #[test]
    fn adding_a_changed_path_never_shrinks_the_selection(
        size in 2usize..8,
        raw_edges in prop::collection::vec((0usize..8, 0usize..8), 0..12),
        first in 0usize..8,
        second in 0usize..8,
    ) {
        prop_assume!(first < size && second < size && first != second);
        let edges: Vec<(usize, usize)> = raw_edges
            .iter()
            .filter(|(from, to)| *from < size && *to < *from)
            .copied()
            .collect();
        let root = workspace(size, &edges);
        let narrow = vec![format!("crates/aex-c{first}/src/lib.rs")];
        let wide = vec![
            format!("crates/aex-c{first}/src/lib.rs"),
            format!("crates/aex-c{second}/src/lib.rs"),
        ];
        let before = selected(&root, &narrow);
        let after = selected(&root, &wide);
        let lost: Vec<&String> = before.difference(&after).collect();
        prop_assert!(
            lost.is_empty(),
            "adding a changed path dropped {lost:?} (edges {edges:?})"
        );
    }

    /// Two runs over identical inputs produce byte-identical canonical JSON.
    #[test]
    fn selection_is_byte_identical_across_runs(
        size in 2usize..6,
        seed in 0usize..6,
    ) {
        prop_assume!(seed < size);
        let edges: Vec<(usize, usize)> = (1..size).map(|index| (index, index - 1)).collect();
        let root = workspace(size, &edges);
        let changed = vec![format!("crates/aex-c{seed}/src/lib.rs")];
        let inputs = GraphInputs::load(&root).expect("inputs");
        let built = verify::build(&inputs).expect("graph");
        let first = select(&built, &inputs, &changed, Mode::Affected, Lane::Pr).expect("first");
        let second = select(&built, &inputs, &changed, Mode::Affected, Lane::Pr).expect("second");
        prop_assert_eq!(
            canon::to_string(&first).expect("canonical"),
            canon::to_string(&second).expect("canonical")
        );
    }
}

#[test]
fn a_full_selection_is_a_superset_of_every_affected_selection() {
    let edges: Vec<(usize, usize)> = (1..6).map(|index| (index, index - 1)).collect();
    let root = workspace(6, &edges);
    let full = selected(&root, &["Cargo.lock".to_owned()]);
    for index in 0..6 {
        let affected = selected(&root, &[format!("crates/aex-c{index}/src/lib.rs")]);
        let lost: Vec<&String> = affected.difference(&full).collect();
        assert!(lost.is_empty(), "the full graph omitted {lost:?}");
    }
}

#[test]
fn selftest_passes_on_a_sound_workspace() {
    let root = common::sound_fixture();
    let inputs = GraphInputs::load(&root).unwrap();
    let built = verify::build(&inputs).unwrap();
    let report = aex_release_tool::selftest::run(&built, &inputs).unwrap();
    assert!(report.probes > 0, "selftest must actually probe something");
}
