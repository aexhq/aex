//! `graph verify` is fail-closed on every declared condition.
//!
//! Each test names one condition and proves it fails on its own, because a
//! rule that only fires alongside four others is a rule nobody can trust to
//! have fired at all.

mod common;

use aex_release_tool::graph::inputs::GraphInputs;
use aex_release_tool::graph::verify;
use common::{CratePlan, Fixture, SOUND_SCENARIOS, SOUND_UNITS, deployable_meta, live_meta};

fn verify_fixture(root: &std::path::Path) -> aex_release_tool::error::Result<verify::BuiltGraph> {
    let inputs = GraphInputs::load(root)?;
    verify::verify(&inputs)
}

#[test]
fn a_sound_workspace_verifies() {
    let root = common::sound_fixture();
    let built = verify_fixture(&root).expect("the sound fixture must verify");
    assert!(built.unowned.is_empty());
    assert!(
        built.live_targets.contains("aex-live-demo-api"),
        "the live target set is derived from live_suite, never hand-written"
    );
}

#[test]
fn a_member_with_no_ownership_metadata_fails() {
    let root = Fixture::new()
        .add_crate(CratePlan::new("aex-leaf", "crates/aex-leaf").without_meta())
        .build();
    let err = verify_fixture(&root).unwrap_err();
    assert_eq!(err.exit.code(), 10);
    assert!(err.rules().contains(&"aex-metadata-missing"));
}

#[test]
fn an_unknown_metadata_key_fails() {
    let mut meta = common::default_meta("delivery", "domain");
    meta["tier"] = serde_json::json!("gold");
    let root = Fixture::new()
        .add_crate(CratePlan::new("aex-leaf", "crates/aex-leaf").meta(meta))
        .build();
    let err = verify_fixture(&root).unwrap_err();
    assert_eq!(err.exit.code(), 10);
    assert!(err.rules().contains(&"aex-metadata-unknown-key"));
}

#[test]
fn a_value_outside_a_closed_set_fails() {
    let mut meta = common::default_meta("delivery", "domain");
    meta["security_tier"] = serde_json::json!("bronze");
    let root = Fixture::new()
        .add_crate(CratePlan::new("aex-leaf", "crates/aex-leaf").meta(meta))
        .build();
    let err = verify_fixture(&root).unwrap_err();
    assert!(err.rules().contains(&"aex-metadata-bad-enum"));
}

#[test]
fn a_repository_file_matching_no_rule_is_an_orphan() {
    let root = Fixture::new()
        .add_crate(CratePlan::new("aex-leaf", "crates/aex-leaf"))
        .file("stray/unowned.txt")
        .build();
    let err = verify_fixture(&root).unwrap_err();
    assert_eq!(err.exit.code(), 10);
    assert!(
        err.violations
            .iter()
            .any(|v| v.rule == "orphan-path" && v.detail.contains("stray/unowned.txt"))
    );
}

#[test]
fn an_orphan_also_widens_the_run_rather_than_only_failing() {
    // Failing alone would leave a red build running a narrow selection over a
    // file nobody owns. The safe run and the visible gap are both required.
    let root = Fixture::new()
        .add_crate(CratePlan::new("aex-leaf", "crates/aex-leaf"))
        .file("stray/unowned.txt")
        .build();
    let inputs = GraphInputs::load(&root).unwrap();
    let built = verify::build(&inputs).unwrap();
    let selection = aex_release_tool::graph::select::select(
        &built,
        &inputs,
        &["stray/unowned.txt".to_owned()],
        aex_release_tool::graph::select::Mode::Affected,
        aex_release_tool::graph::select::Lane::Pr,
    )
    .unwrap();
    assert!(selection.repo_wide);
    assert_eq!(selection.unowned, vec!["stray/unowned.txt"]);
    assert!(!selection.test.is_empty());
}

#[test]
fn a_deployable_with_no_scenario_owner_fails() {
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
        .scenarios("schema = \"aex.scenario-ownership.v1\"\n")
        .build();
    let err = verify_fixture(&root).unwrap_err();
    assert_eq!(err.exit.code(), 10);
    assert!(err.rules().contains(&"missing-scenario-owner"));
}

#[test]
fn a_deployable_with_no_live_companion_fails() {
    let root = Fixture::new()
        .add_crate(
            CratePlan::new("demo-api", "services/demo-api")
                .meta(deployable_meta("demo-api", "aex-live-demo-api")),
        )
        .units(SOUND_UNITS)
        .scenarios(SOUND_SCENARIOS)
        .build();
    let err = verify_fixture(&root).unwrap_err();
    assert!(err.rules().contains(&"unit-live-companion-missing"));
}

#[test]
fn a_live_companion_nobody_names_is_orphaned() {
    let root = Fixture::new()
        .add_crate(
            CratePlan::new("aex-live-ghost", "tests/live/aex-live-ghost").meta(live_meta("ghost")),
        )
        .build();
    let err = verify_fixture(&root).unwrap_err();
    assert!(err.rules().contains(&"aex-orphan-companion"));
}

#[test]
fn a_deployable_with_no_resource_shape_fails() {
    let units = r#"
schema = "aex.units.v1"

[[unit]]
id = "demo-api"
kind = "rust-lambda"
plane = "regional"
package = "demo-api"
target = "aarch64-unknown-linux-gnu.2.34"
profile = "release-lambda"
form = "zip"
config_env_namespace = "AEX_DEMO_"
config_schema_version = 1
required_receipts = ["unit"]
alarm_spec = "demo-api"
live_suite = "aex-live-demo-api"
"#;
    let root = Fixture::new()
        .add_crate(
            CratePlan::new("demo-api", "services/demo-api")
                .meta(deployable_meta("demo-api", "aex-live-demo-api")),
        )
        .add_crate(
            CratePlan::new("aex-live-demo-api", "tests/live/aex-live-demo-api")
                .meta(live_meta("demo-api")),
        )
        .units(units)
        .scenarios(SOUND_SCENARIOS)
        .build();
    let err = verify_fixture(&root).unwrap_err();
    assert!(err.rules().contains(&"unit-resource-shape-missing"));
}

#[test]
fn a_placeholder_resource_shape_is_rejected() {
    let units = SOUND_UNITS.replace("memory_mb = 512", "memory_mb = 0");
    let root = Fixture::new()
        .add_crate(
            CratePlan::new("demo-api", "services/demo-api")
                .meta(deployable_meta("demo-api", "aex-live-demo-api")),
        )
        .add_crate(
            CratePlan::new("aex-live-demo-api", "tests/live/aex-live-demo-api")
                .meta(live_meta("demo-api")),
        )
        .units(&units)
        .scenarios(SOUND_SCENARIOS)
        .build();
    let err = verify_fixture(&root).unwrap_err();
    assert!(err.rules().contains(&"unit-resource-shape-placeholder"));
}

#[test]
fn a_nonstandard_health_path_is_rejected() {
    let units = SOUND_UNITS.replace("/internal/healthz", "/livez");
    let root = Fixture::new()
        .add_crate(
            CratePlan::new("demo-api", "services/demo-api")
                .meta(deployable_meta("demo-api", "aex-live-demo-api")),
        )
        .add_crate(
            CratePlan::new("aex-live-demo-api", "tests/live/aex-live-demo-api")
                .meta(live_meta("demo-api")),
        )
        .units(&units)
        .scenarios(SOUND_SCENARIOS)
        .build();
    let err = verify_fixture(&root).unwrap_err();
    assert!(err.rules().contains(&"unit-health-path-nonstandard"));
}

#[test]
fn a_scenario_naming_an_unknown_node_cannot_build_a_graph() {
    let root = Fixture::new()
        .add_crate(CratePlan::new("aex-leaf", "crates/aex-leaf"))
        .scenarios(
            r#"
schema = "aex.scenario-ownership.v1"
[[scenario]]
id = "SC-GHOST"
owner = "delivery"
observes = ["artifact:ghost"]
"#,
        )
        .build();
    let err = verify_fixture(&root).unwrap_err();
    assert_eq!(err.exit.code(), 10);
    assert!(err.rules().contains(&"unknown-node-reference"));
}

#[test]
fn a_unit_naming_an_unknown_package_fails() {
    let root = Fixture::new()
        .add_crate(
            CratePlan::new("aex-live-demo-api", "tests/live/aex-live-demo-api")
                .meta(live_meta("demo-api")),
        )
        .units(SOUND_UNITS)
        .scenarios(SOUND_SCENARIOS)
        .build();
    let err = verify_fixture(&root).unwrap_err();
    assert!(err.rules().contains(&"unit-package-unknown"));
}

#[test]
fn every_violation_is_reported_rather_than_only_the_first() {
    let root = Fixture::new()
        .add_crate(CratePlan::new("aex-a", "crates/aex-a").without_meta())
        .add_crate(CratePlan::new("aex-b", "crates/aex-b").without_meta())
        .file("stray/one.txt")
        .file("stray/two.txt")
        .build();
    let err = verify_fixture(&root).unwrap_err();
    assert!(
        err.violations.len() >= 4,
        "expected at least four violations, got {:?}",
        err.rules()
    );
}
