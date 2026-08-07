//! Selection: the reverse closure, the deployment graph's exclusions, the
//! publication re-expansion, and shadow mode.

mod common;

use aex_release_tool::graph::inputs::GraphInputs;
use aex_release_tool::graph::select::{
    Lane, Mode, Selection, SelectionReason, is_artifact_input, select,
};
use aex_release_tool::graph::verify;
use common::{CratePlan, Fixture, SOUND_SCENARIOS, SOUND_UNITS, deployable_meta, live_meta};

fn run(root: &std::path::Path, changed: &[&str], mode: Mode) -> Selection {
    run_for_lane(root, changed, mode, Lane::Pr)
}

fn run_for_lane(root: &std::path::Path, changed: &[&str], mode: Mode, lane: Lane) -> Selection {
    let inputs = GraphInputs::load(root).expect("fixture inputs");
    let built = verify::build(&inputs).expect("a buildable graph");
    let changed: Vec<String> = changed.iter().map(|path| (*path).to_owned()).collect();
    select(&built, &inputs, &changed, mode, lane).expect("a selection")
}

fn ids(selection: &[aex_release_tool::graph::select::Selected]) -> Vec<String> {
    let mut ids: Vec<String> = selection
        .iter()
        .map(|selected| selected.id.to_string())
        .collect();
    ids.sort();
    ids
}

#[test]
fn a_leaf_change_selects_its_whole_reverse_closure() {
    let root = common::sound_fixture();
    let selection = run(&root, &["crates/aex-leaf/src/lib.rs"], Mode::Affected);
    // The live companion is reached through its dev-dependency on the
    // deployable, which is exactly what the test graph is for.
    assert_eq!(
        ids(&selection.test),
        vec![
            "cargo:aex-leaf",
            "cargo:aex-live-demo-api",
            "cargo:aex-middle",
            "cargo:aex-top",
            "cargo:demo-api",
        ]
    );
}

#[test]
fn main_affected_selection_skips_unrelated_packages_without_losing_dependants() {
    let root = Fixture::new()
        .add_crate(CratePlan::new("aex-leaf", "crates/aex-leaf"))
        .add_crate(CratePlan::new("aex-dependent", "crates/aex-dependent").dep("aex-leaf"))
        .add_crate(CratePlan::new("aex-unrelated", "crates/aex-unrelated"))
        .build();
    let selection = run_for_lane(
        &root,
        &["crates/aex-leaf/src/lib.rs"],
        Mode::Affected,
        Lane::Main,
    );

    assert_eq!(selection.mode, Mode::Affected);
    assert_eq!(
        ids(&selection.test),
        vec!["cargo:aex-dependent", "cargo:aex-leaf"]
    );
    assert!(
        !ids(&selection.test).contains(&"cargo:aex-unrelated".to_owned()),
        "an unchanged disconnected package must not rerun"
    );
}

#[test]
fn a_change_to_a_leaf_records_why_each_dependent_ran() {
    let root = common::sound_fixture();
    let selection = run(&root, &["crates/aex-leaf/src/lib.rs"], Mode::Affected);
    let leaf = selection
        .test
        .iter()
        .find(|selected| selected.id.as_str() == "cargo:aex-leaf")
        .unwrap();
    assert!(matches!(leaf.reason, SelectionReason::ChangedSource { .. }));
    let top = selection
        .test
        .iter()
        .find(|selected| selected.id.as_str() == "cargo:aex-top")
        .unwrap();
    match &top.reason {
        SelectionReason::ReverseDependency { of, hops, .. } => {
            assert_eq!(of.as_str(), "cargo:aex-leaf");
            assert_eq!(*hops, 2);
        }
        other => panic!("expected a reverse-dependency reason, got {other:?}"),
    }
}

#[test]
fn a_dev_only_edge_selects_for_test_and_not_for_deployment() {
    // The live companion dev-depends on the deployable. Changing the
    // deployable must retest the companion; changing the companion must not
    // mint the deployable's bytes.
    let root = common::sound_fixture();
    let forward = run(&root, &["services/demo-api/src/lib.rs"], Mode::Affected);
    assert!(ids(&forward.test).contains(&"cargo:aex-live-demo-api".to_owned()));

    let backward = run(
        &root,
        &["tests/live/aex-live-demo-api/src/lib.rs"],
        Mode::Affected,
    );
    assert!(
        backward.deploy.is_empty(),
        "a live-companion edit minted {:?}",
        ids(&backward.deploy)
    );
}

#[test]
fn a_test_only_edit_retests_and_mints_no_artifact() {
    let root = common::sound_fixture();
    let selection = run(
        &root,
        &["services/demo-api/tests/config.rs"],
        Mode::Affected,
    );
    assert!(
        ids(&selection.test).contains(&"cargo:demo-api".to_owned()),
        "the crate must still be retested"
    );
    assert!(
        selection.deploy.is_empty(),
        "a test edit minted {:?}",
        ids(&selection.deploy)
    );
}

#[test]
fn a_source_edit_selects_the_artifact() {
    let root = common::sound_fixture();
    let selection = run(&root, &["services/demo-api/src/main.rs"], Mode::Affected);
    assert_eq!(ids(&selection.deploy), vec!["artifact:demo-api"]);
}

#[test]
fn a_non_artifact_path_beside_a_source_edit_still_mints_the_artifact() {
    // The seed map remembers one path per node and `git diff --name-only`
    // sorts bytewise, so `tests/` reaches the map before `src/` for the same
    // crate. Deciding the artifact seed from that first path drops the whole
    // deployment on account of a file that cannot reach production bytes.
    // Both orders are asserted because the failure is order-dependent by
    // construction, which is also what makes it invisible in review.
    let root = common::sound_fixture();
    for changed in [
        [
            "services/demo-api/tests/config.rs",
            "services/demo-api/src/main.rs",
        ],
        [
            "services/demo-api/src/main.rs",
            "services/demo-api/tests/config.rs",
        ],
    ] {
        let selection = run(&root, &changed, Mode::Affected);
        assert_eq!(
            ids(&selection.deploy),
            vec!["artifact:demo-api"],
            "a source edit stopped minting bytes because {changed:?} rode along"
        );
    }
}

#[test]
fn a_dependency_source_edit_selects_the_downstream_artifact() {
    let root = common::sound_fixture();
    let selection = run(&root, &["crates/aex-middle/src/lib.rs"], Mode::Affected);
    assert_eq!(ids(&selection.deploy), vec!["artifact:demo-api"]);
}

#[test]
fn a_scenario_observing_a_selected_artifact_is_selected() {
    let root = common::sound_fixture();
    let selection = run(&root, &["services/demo-api/src/main.rs"], Mode::Affected);
    assert_eq!(ids(&selection.scenarios), vec!["scenario:SC-DEMO"]);
}

#[test]
fn artifact_input_classification_excludes_test_and_documentation_paths() {
    assert!(is_artifact_input("crates/aex-wire/src/lib.rs"));
    assert!(is_artifact_input("crates/aex-wire/build.rs"));
    assert!(is_artifact_input("crates/aex-wire/src/generated/wire.rs"));
    assert!(is_artifact_input("Cargo.lock"));
    assert!(is_artifact_input("rust-toolchain.toml"));

    assert!(!is_artifact_input("crates/aex-wire/tests/properties.rs"));
    assert!(!is_artifact_input("crates/aex-wire/benches/decode.rs"));
    assert!(!is_artifact_input("crates/aex-wire/examples/demo.rs"));
    assert!(!is_artifact_input("tests/live/aex-live-x/src/lib.rs"));
    assert!(!is_artifact_input("apps/user-tests/journeys.ts"));
    assert!(!is_artifact_input("references/plan.md"));
    assert!(!is_artifact_input("conformance/valid/session.json"));
}

#[test]
fn a_router_change_forces_the_full_graph() {
    let root = common::sound_fixture();
    let selection = run(&root, &["release/units.toml"], Mode::Affected);
    assert_eq!(selection.mode, Mode::Full);
    assert!(matches!(
        selection.test[0].reason,
        SelectionReason::RouterChanged
    ));
    let selected = ids(&selection.test);
    for crate_node in [
        "cargo:aex-leaf",
        "cargo:aex-live-demo-api",
        "cargo:aex-middle",
        "cargo:aex-top",
        "cargo:demo-api",
    ] {
        assert!(
            selected.contains(&crate_node.to_owned()),
            "the full graph omitted `{crate_node}`: {selected:?}"
        );
    }
    assert!(
        selected.contains(&"bundle:contract".to_owned()),
        "the generated bundles are graph nodes and run with everything else"
    );
}

#[test]
fn a_repo_wide_path_widens_without_claiming_the_router_changed() {
    let root = common::sound_fixture();
    let selection = run(&root, &["Cargo.lock"], Mode::Affected);
    assert!(selection.repo_wide);
    assert!(matches!(
        selection.test[0].reason,
        SelectionReason::RepoWide { .. }
    ));
}

#[test]
fn shadow_mode_runs_everything_and_records_what_affected_would_have_omitted() {
    let root = common::sound_fixture();
    let selection = run(&root, &["crates/aex-top/src/lib.rs"], Mode::Shadow);
    let shadow = selection.shadow.as_ref().expect("shadow observations");
    assert!(
        ids(&selection.test).contains(&"cargo:aex-leaf".to_owned()),
        "shadow mode runs the full graph, not the affected selection"
    );
    let delta: Vec<&str> = shadow
        .delta
        .iter()
        .map(aex_release_tool::graph::NodeId::as_str)
        .collect();
    assert!(
        delta.contains(&"cargo:aex-leaf"),
        "a change to the top of the chain does not reach the leaf; delta was {delta:?}"
    );
    assert!(
        !delta.contains(&"cargo:aex-top"),
        "the changed node itself is never a delta"
    );
}

#[test]
fn publication_re_expands_forward_over_the_publishable_subset() {
    // `sdk` is publishable and depends on `contracts`, which is also
    // publishable. Changing `sdk` must also select `contracts`, because
    // publishing one without the other ships a package whose dependency does
    // not exist at that version.
    let root = Fixture::new()
        .add_crate(CratePlan::new("aex-contracts", "crates/aex-contracts").publishable())
        .add_crate(
            CratePlan::new("aex-sdk", "crates/aex-sdk")
                .dep("aex-contracts")
                .publishable(),
        )
        .build();
    let selection = run(&root, &["crates/aex-sdk/src/lib.rs"], Mode::Affected);
    assert_eq!(
        ids(&selection.test),
        vec!["cargo:aex-contracts", "cargo:aex-sdk"]
    );
    let contracts = selection
        .test
        .iter()
        .find(|selected| selected.id.as_str() == "cargo:aex-contracts")
        .unwrap();
    assert!(matches!(
        contracts.reason,
        SelectionReason::PublishClosure { .. }
    ));
}

#[test]
fn selecting_with_no_change_information_is_undecidable_rather_than_empty() {
    let root = common::sound_fixture();
    let inputs = GraphInputs::load(&root).unwrap();
    let built = verify::build(&inputs).unwrap();
    let err = select(&built, &inputs, &[], Mode::Affected, Lane::Pr).unwrap_err();
    assert_eq!(err.exit.code(), 11);
    assert!(err.rules().contains(&"routing-no-change-information"));
}

#[test]
fn selection_is_a_pure_function_of_manifests_and_paths() {
    let root = common::sound_fixture();
    let first = run(&root, &["crates/aex-leaf/src/lib.rs"], Mode::Affected);
    let second = run(&root, &["crates/aex-leaf/src/lib.rs"], Mode::Affected);
    assert_eq!(
        aex_release_tool::canon::to_string(&first).unwrap(),
        aex_release_tool::canon::to_string(&second).unwrap()
    );
}

#[test]
fn a_change_set_order_does_not_change_the_selection() {
    let root = common::sound_fixture();
    let forward = run(
        &root,
        &["crates/aex-leaf/src/lib.rs", "crates/aex-middle/src/lib.rs"],
        Mode::Affected,
    );
    let reversed = run(
        &root,
        &["crates/aex-middle/src/lib.rs", "crates/aex-leaf/src/lib.rs"],
        Mode::Affected,
    );
    assert_eq!(ids(&forward.test), ids(&reversed.test));
    assert_eq!(ids(&forward.deploy), ids(&reversed.deploy));
}

#[test]
fn a_terraform_module_change_selects_the_module_and_no_artifact() {
    let root = Fixture::new()
        .add_crate(
            CratePlan::new("demo-api", "services/demo-api")
                .meta(deployable_meta("demo-api", "aex-live-demo-api")),
        )
        .add_crate(
            CratePlan::new("aex-live-demo-api", "tests/live/aex-live-demo-api")
                .meta(live_meta("demo-api")),
        )
        .units(SOUND_UNITS)
        .scenarios(SOUND_SCENARIOS)
        .build();
    common::write(
        &root,
        "infra/modules/kms-key/main.tf",
        "resource \"aws_kms_key\" \"this\" {}\n",
    );
    common::write(
        &root,
        "infra/modules/kms-key/aex.toml",
        r#"
[aex]
owner = "delivery"
role = "infra_module"
artifact = "terraform_module"
layers = ["unit"]
concerns = ["security"]
seams = []
security_tier = "internal"
risk = ["iam"]
scenarios = []
[aex.not_applicable]
live_suite = "Terraform module; proved by mock-provider plan tests, not a deployed endpoint"
"#,
    );
    let selection = run(&root, &["infra/modules/kms-key/main.tf"], Mode::Affected);
    assert_eq!(ids(&selection.test), vec!["tf:modules/kms-key"]);
    assert!(selection.deploy.is_empty());
}
