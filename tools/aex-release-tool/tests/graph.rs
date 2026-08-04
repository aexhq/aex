//! `graph verify` is fail-closed on every declared condition.
//!
//! Each test names one condition and proves it fails on its own, because a
//! rule that only fires alongside four others is a rule nobody can trust to
//! have fired at all.

mod common;

use aex_release_tool::graph::inputs::GraphInputs;
use aex_release_tool::graph::verify;
use common::{
    CratePlan, Fixture, SOUND_SCENARIOS, SOUND_UNITS, deployable_meta, live_meta, live_meta_for,
    write,
};

fn verify_fixture(root: &std::path::Path) -> aex_release_tool::error::Result<verify::BuiltGraph> {
    let inputs = GraphInputs::load(root)?;
    verify::verify(&inputs)
}

fn classify_generated_api(root: &std::path::Path) {
    let path = root.join("release/path-map.toml");
    let mut policy = std::fs::read_to_string(&path).expect("fixture path map");
    policy.push_str(
        r#"
[[rule]]
id = "generated-api"
prefix = "api/generated/"
kind = "ignored"
"#,
    );
    write(root, "release/path-map.toml", &policy);
}

fn copy_authored_contract(root: &std::path::Path) {
    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for entry in walkdir::WalkDir::new(repository.join("api"))
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
    {
        let relative = entry
            .path()
            .strip_prefix(&repository)
            .expect("inside repository");
        if relative.starts_with("api/generated") {
            continue;
        }
        let target = root.join(relative);
        std::fs::create_dir_all(target.parent().expect("file parent")).expect("create parent");
        std::fs::copy(entry.path(), target).expect("copy authored contract input");
    }
    std::fs::copy(
        repository.join("rust-toolchain.toml"),
        root.join("rust-toolchain.toml"),
    )
    .expect("copy toolchain pin");
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
fn observing_an_artifact_without_a_runnable_claim_is_not_scenario_coverage() {
    let scenarios = SOUND_SCENARIOS
        .replace("package = \"cargo:aex-live-demo-api\"\n", "")
        .replace("target = \"smoke\"\n", "");
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
        .scenarios(&scenarios)
        .build();
    let err = verify_fixture(&root).unwrap_err();
    assert!(err.rules().contains(&"scenario-runnable-missing"));
}

#[test]
fn an_explicit_scenario_deferral_is_valid_but_not_runnable_evidence() {
    let scenarios = SOUND_SCENARIOS
        .replace("package = \"cargo:aex-live-demo-api\"\n", "")
        .replace("target = \"smoke\"\n", "")
        .replace(
            "observes = [\"artifact:demo-api\"]\n",
            "observes = [\"artifact:demo-api\"]\ndeferred = \"live target waits for the production composition\"\n",
        );
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
        .scenarios(&scenarios)
        .build();
    let built = verify_fixture(&root).expect("explicit deferral is structurally valid");
    assert_eq!(built.deferred.len(), 1);
    assert_eq!(built.deferred[0].id, "SC-DEMO");

    let inputs = GraphInputs::load(&root).unwrap();
    let err = verify::verify_release_candidate(&inputs)
        .expect_err("a release cannot promote an all-deferred scenario registry");
    assert_eq!(err.exit.code(), 10);
    assert!(err.rules().contains(&"release-runnable-scenario-missing"));
}

#[test]
fn a_release_candidate_with_a_runnable_scenario_verifies() {
    let root = common::sound_fixture();
    let inputs = GraphInputs::load(&root).unwrap();
    verify::verify_release_candidate(&inputs)
        .expect("the runnable package and target are release evidence");
}

#[test]
fn a_scenario_cannot_claim_runnable_evidence_and_a_deferral() {
    let scenarios = SOUND_SCENARIOS.replace(
        "target = \"smoke\"\n",
        "target = \"smoke\"\ndeferred = \"not actually runnable\"\n",
    );
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
        .scenarios(&scenarios)
        .build();
    let err = verify_fixture(&root).expect_err("conflicting scenario state must fail");
    assert!(err.rules().contains(&"scenario-claim-conflict"));
}

#[test]
fn a_scenario_package_must_claim_the_scenario_and_the_exact_target() {
    let root = Fixture::new()
        .add_crate(
            CratePlan::new("demo-api", "services/demo-api")
                .meta(deployable_meta("demo-api", "aex-live-demo-api")),
        )
        .add_crate(
            CratePlan::new("aex-live-demo-api", "tests/live/aex-live-demo-api")
                .meta(live_meta_for("demo-api", "SC-SOMETHING-ELSE")),
        )
        .units(SOUND_UNITS)
        .scenarios(&SOUND_SCENARIOS.replace("target = \"smoke\"", "target = \"ghost\""))
        .build();
    let err = verify_fixture(&root).unwrap_err();
    assert!(err.rules().contains(&"scenario-package-disagreement"));
    assert!(err.rules().contains(&"scenario-target-unknown"));
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

/// A `ts-lambda` unit whose owning package is an npm member, plus its live
/// companion and scenario. The npm package sits under `services/`, which no
/// `workspaces` glob reaches, so it is only visible through the explicit list
/// `aex-workspace-check` owns.
const TS_EDGE_UNITS: &str = r#"
schema = "aex.units.v1"

[[unit]]
id = "stripe-command-edge"
kind = "ts-lambda"
plane = "central"
package = "@fixture/stripe-command-edge"
target = "nodejs22.x-arm64"
profile = "release"
form = "zip"
entrypoint = "handler.js"
config_env_namespace = "AEX_STRIPE_COMMAND_"
config_schema_version = 1
required_receipts = ["unit", "lint"]
alarm_spec = "stripe-command-edge"
live_suite = "aex-live-stripe-command-edge"

[unit.lambda]
memory_mb = 512
timeout_s = 30
reserved_concurrency = 20
"#;

const TS_EDGE_SCENARIOS: &str = r#"
schema = "aex.scenario-ownership.v1"

[[scenario]]
id = "SC-EDGE"
owner = "central-finance"
observes = ["artifact:stripe-command-edge"]
package = "cargo:aex-live-stripe-command-edge"
target = "smoke"
"#;

fn ts_edge_fixture(units: &str) -> std::path::PathBuf {
    Fixture::new()
        .add_npm(
            common::NpmPlan::new(
                "@fixture/stripe-command-edge",
                "services/stripe-command-edge",
            )
            .meta(common::npm_deployable_meta(
                "stripe-command-edge",
                "aex-live-stripe-command-edge",
            )),
        )
        .add_crate(
            CratePlan::new(
                "aex-live-stripe-command-edge",
                "tests/live/aex-live-stripe-command-edge",
            )
            .meta(live_meta_for("stripe-command-edge", "SC-EDGE")),
        )
        .units(units)
        .scenarios(TS_EDGE_SCENARIOS)
        .build()
}

#[test]
fn a_unit_owned_by_an_npm_package_resolves_to_the_npm_node() {
    // The unit registry names a package, not a node namespace. Resolving every
    // unit into `cargo:` would leave the two TypeScript edges pointing at a
    // Cargo member that does not exist, which stops graph construction.
    let root = ts_edge_fixture(TS_EDGE_UNITS);
    let built = verify_fixture(&root).expect("a TypeScript edge unit must verify");
    let artifact = aex_release_tool::graph::NodeId::artifact("stripe-command-edge");
    let package = aex_release_tool::graph::NodeId::npm("@fixture/stripe-command-edge");
    let slot = built.graph.slot(&artifact).expect("the artifact node");
    let target = built.graph.slot(&package).expect("the npm node");
    assert!(
        built
            .graph
            .forward()
            .neighbours(slot)
            .any(|(neighbour, _)| neighbour == target),
        "the artifact's input closure must reach its owning npm package"
    );
}

#[test]
fn an_npm_package_outside_every_workspace_glob_is_still_read() {
    // `services/*` is in no `workspaces` array. If the delivery graph only read
    // the globs it would derive a smaller live-target set than the registry
    // does, and the two authorities would disagree about the same tree.
    let root = ts_edge_fixture(TS_EDGE_UNITS);
    let built = verify_fixture(&root).unwrap();
    assert!(
        built.live_targets.contains("aex-live-stripe-command-edge"),
        "an explicitly named npm package's live_suite must reach the derived set"
    );
}

#[test]
fn a_unit_naming_a_package_in_neither_namespace_still_names_the_row() {
    let units = TS_EDGE_UNITS.replace("@fixture/stripe-command-edge", "@fixture/ghost");
    let err = verify_fixture(&ts_edge_fixture(&units)).unwrap_err();
    assert!(
        err.rules().contains(&"unit-package-unknown"),
        "the report must name the registry row, not a dangling edge: {:?}",
        err.rules()
    );
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

#[test]
fn a_route_registry_without_routes_fails_closed() {
    let root = common::sound_fixture();
    classify_generated_api(&root);
    write(
        &root,
        "api/generated/bundle.json",
        r#"{"planes":{"regional":{"operations":[]}}}"#,
    );
    write(
        &root,
        "api/generated/registries/routes.json",
        r#"{"schema":"aex.route-registry.v1"}"#,
    );
    let err = verify_fixture(&root).unwrap_err();
    assert!(err.rules().contains(&"route-registry-shape"));
}

#[test]
fn duplicate_route_registry_members_fail_closed() {
    let root = common::sound_fixture();
    classify_generated_api(&root);
    write(
        &root,
        "api/generated/bundle.json",
        r#"{"planes":{"regional":{"operations":[]}}}"#,
    );
    write(
        &root,
        "api/generated/registries/routes.json",
        r#"{"schema":"aex.route-registry.v1","routes":[],"routes":[]}"#,
    );
    let err = verify_fixture(&root).unwrap_err();
    assert!(err.rules().contains(&"route-registry-unparseable"));
}

#[test]
fn duplicate_contract_bundle_members_fail_closed() {
    let root = common::sound_fixture();
    classify_generated_api(&root);
    write(
        &root,
        "api/generated/bundle.json",
        r#"{"planes":{"regional":{"operations":[],"operations":[]}}}"#,
    );
    write(
        &root,
        "api/generated/registries/routes.json",
        r#"{"schema":"aex.route-registry.v1","routes":[]}"#,
    );
    let err = verify_fixture(&root).unwrap_err();
    assert!(err.rules().contains(&"contract-bundle-unparseable"));
}

#[test]
fn authored_openapi_keeps_freshness_live_when_both_output_sentinels_are_deleted() {
    let root = common::sound_fixture();
    classify_generated_api(&root);
    copy_authored_contract(&root);
    aex_contract_gen::build(&root).expect("build fixture contract outputs");
    std::fs::remove_file(root.join("api/generated/bundle.json")).expect("delete bundle");
    std::fs::remove_file(root.join("api/generated/registries/routes.json"))
        .expect("delete route registry");

    let err = verify_fixture(&root).unwrap_err();
    assert!(err.rules().contains(&"generated-contract-stale"));
    assert!(err.rules().contains(&"route-registry-missing"));
}

#[test]
fn deleting_routes_meta_while_outputs_exist_is_unverifiable_not_fresh() {
    let root = common::sound_fixture();
    classify_generated_api(&root);
    copy_authored_contract(&root);
    aex_contract_gen::build(&root).expect("build fixture contract outputs");
    std::fs::remove_file(root.join("api/schemas/registries/routes-meta.yaml"))
        .expect("delete authored metadata");

    let err = verify_fixture(&root).unwrap_err();
    assert!(err.rules().contains(&"generated-contract-unverifiable"));
}

#[test]
fn a_route_scenario_must_observe_the_artifact_that_serves_it() {
    let root = common::sound_fixture();
    classify_generated_api(&root);
    write(
        &root,
        "api/generated/bundle.json",
        r#"{"planes":{"regional":{"operations":[{"operationId":"demo_get"}]}}}"#,
    );
    write(
        &root,
        "api/generated/registries/routes.json",
        r#"{"schema":"aex.route-registry.v1","routes":[{"operationId":"demo_get","plane":"regional","servingArtifact":"demo-api","servedArtifact":"demo-api","scenarios":["SC-GHOST"]}]}"#,
    );
    let err = verify_fixture(&root).unwrap_err();
    assert!(err.rules().contains(&"aex-route-uncovered"));

    let scenario_path = root.join("release/scenario-ownership.toml");
    let scenarios = std::fs::read_to_string(&scenario_path)
        .expect("scenario fixture")
        .replace("artifact:demo-api", "cargo:aex-leaf");
    write(&root, "release/scenario-ownership.toml", &scenarios);
    write(
        &root,
        "api/generated/registries/routes.json",
        r#"{"schema":"aex.route-registry.v1","routes":[{"operationId":"demo_get","plane":"regional","servingArtifact":"demo-api","servedArtifact":"demo-api","scenarios":["SC-DEMO"]}]}"#,
    );
    let err = verify_fixture(&root).unwrap_err();
    assert!(err.rules().contains(&"aex-route-scenario-disagreement"));

    write(
        &root,
        "api/generated/registries/routes.json",
        r#"{"schema":"aex.route-registry.v1","routes":[{"operationId":"demo_get","plane":"regional","servingArtifact":"demo-api","servedArtifact":"demo-api","scenarios":["SC-DEMO"]}]}"#,
    );
    let scenarios = scenarios.replace("cargo:aex-leaf", "artifact:demo-api");
    write(&root, "release/scenario-ownership.toml", &scenarios);
    let err = verify_fixture(&root).expect_err("synthetic generated outputs have no authored tree");
    assert!(err.rules().contains(&"generated-contract-unverifiable"));
    assert!(!err.rules().contains(&"aex-route-scenario-disagreement"));
    assert!(!err.rules().contains(&"aex-route-uncovered"));
}

#[test]
fn planned_ownership_is_not_proof_that_a_route_is_mounted() {
    let root = common::sound_fixture();
    classify_generated_api(&root);
    write(
        &root,
        "api/generated/bundle.json",
        r#"{"planes":{"regional":{"operations":[{"operationId":"demo_get"}]}}}"#,
    );
    write(
        &root,
        "api/generated/registries/routes.json",
        r#"{"schema":"aex.route-registry.v1","routes":[{"operationId":"demo_get","plane":"regional","servingArtifact":"demo-api","scenarios":["SC-DEMO"]}]}"#,
    );
    let err = verify_fixture(&root).unwrap_err();
    assert!(err.rules().contains(&"aex-route-unserved"));
}

#[test]
fn an_unmounted_route_requires_an_explicit_non_empty_deferral() {
    let root = common::sound_fixture();
    classify_generated_api(&root);
    write(
        &root,
        "api/generated/bundle.json",
        r#"{"planes":{"regional":{"operations":[{"operationId":"demo_get"}]}}}"#,
    );
    write(
        &root,
        "api/generated/registries/routes.json",
        r#"{"schema":"aex.route-registry.v1","routes":[{"operationId":"demo_get","plane":"regional","servingArtifact":"demo-api","deferredReason":"production handler is not composed","scenarios":["SC-DEMO"]}]}"#,
    );
    let err = verify_fixture(&root).expect_err("synthetic outputs have no authored source");
    assert!(err.rules().contains(&"generated-contract-unverifiable"));
    assert!(!err.rules().contains(&"aex-route-unserved"));
    assert!(!err.rules().contains(&"aex-route-deferral-invalid"));

    write(
        &root,
        "api/generated/registries/routes.json",
        r#"{"schema":"aex.route-registry.v1","routes":[{"operationId":"demo_get","plane":"regional","servingArtifact":"demo-api","deferredReason":"","scenarios":["SC-DEMO"]}]}"#,
    );
    let err = verify_fixture(&root).expect_err("empty deferral must fail");
    assert!(err.rules().contains(&"aex-route-deferral-invalid"));
}

#[test]
fn a_route_cannot_be_both_served_and_deferred() {
    let root = common::sound_fixture();
    classify_generated_api(&root);
    write(
        &root,
        "api/generated/bundle.json",
        r#"{"planes":{"regional":{"operations":[{"operationId":"demo_get"}]}}}"#,
    );
    write(
        &root,
        "api/generated/registries/routes.json",
        r#"{"schema":"aex.route-registry.v1","routes":[{"operationId":"demo_get","plane":"regional","servingArtifact":"demo-api","servedArtifact":"demo-api","deferredReason":"contradiction","scenarios":["SC-DEMO"]}]}"#,
    );
    let err = verify_fixture(&root).expect_err("conflicting route state must fail");
    assert!(err.rules().contains(&"aex-route-state-conflict"));
}

#[test]
fn malformed_and_cross_plane_route_owners_fail_closed() {
    let root = common::sound_fixture();
    classify_generated_api(&root);
    write(
        &root,
        "api/generated/bundle.json",
        r#"{"planes":{"regional":{"operations":[{"operationId":"demo_get"}]}}}"#,
    );
    write(
        &root,
        "api/generated/registries/routes.json",
        r#"{"schema":"aex.route-registry.v1","routes":[{"operationId":"demo_get","plane":"central","servingArtifact":"-demo-api","servedArtifact":"demo-api","scenarios":["SC-DEMO"]}]}"#,
    );
    let err = verify_fixture(&root).unwrap_err();
    assert!(err.rules().contains(&"aex-route-owner-invalid"));
    assert!(err.rules().contains(&"aex-route-owner-cross-plane"));
}

// ---------------------------------------------------------------------------
// OD-36: what may provision in `prd`
// ---------------------------------------------------------------------------

/// A scenario registry that marks a scenario prd-eligible, with one
/// substitution point for the `[scenario.prd]` block under test.
fn scenarios_with_prd(block: &str) -> String {
    format!(
        "schema = \"aex.scenario-ownership.v1\"\n\n\
         [[scenario]]\n\
         id = \"SC-DEMO\"\n\
         owner = \"delivery\"\n\
         observes = [\"artifact:demo-api\"]\n\
         package = \"cargo:aex-live-demo-api\"\n\
         target = \"smoke\"\n\n\
         {block}\n"
    )
}

fn prd_fixture(block: &str) -> std::path::PathBuf {
    Fixture::new()
        .add_crate(
            CratePlan::new("demo-api", "services/demo-api")
                .meta(deployable_meta("demo-api", "aex-live-demo-api")),
        )
        .add_crate(
            CratePlan::new("aex-live-demo-api", "tests/live/aex-live-demo-api")
                .meta(live_meta("demo-api")),
        )
        .units(SOUND_UNITS)
        .scenarios(&scenarios_with_prd(block))
        .build()
}

/// The hard rule. A scenario that creates something the janitor cannot find by
/// tag has produced residue no sweep can ever remove, and the whole decision to
/// provision in production rests on the sweep working.
#[test]
fn a_prd_eligible_scenario_creating_an_unreclaimable_kind_fails() {
    let root = prd_fixture("[scenario.prd]\nrule = \"money_path\"\nprovisions = [\"sqs_message\"]");
    let err = verify_fixture(&root).unwrap_err();
    assert_eq!(err.exit.code(), 10);
    assert!(
        err.rules().contains(&"scenario-prd-unreclaimable"),
        "{:?}",
        err.rules()
    );
}

/// The review's worst finding, mechanised: a money path that declares the
/// customer but not the card and the auto-recharge policy leaves a recurring
/// charge behind. Declaring one drags in the other two, or the check fails.
#[test]
fn a_prd_money_path_declaring_a_payment_customer_alone_fails() {
    let root =
        prd_fixture("[scenario.prd]\nrule = \"money_path\"\nprovisions = [\"payment_customer\"]");
    let err = verify_fixture(&root).unwrap_err();
    let detail = err
        .violations
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        err.rules().contains(&"scenario-prd-companion-undeclared"),
        "{detail}"
    );
    assert!(detail.contains("auto_recharge_policy"), "{detail}");
    assert!(detail.contains("payment_instrument"), "{detail}");
}

#[test]
fn a_prd_rule_outside_the_closed_set_fails() {
    let root = prd_fixture(
        "[scenario.prd]\nrule = \"because_it_is_convenient\"\nprovisions = [\"api_key\"]",
    );
    let err = verify_fixture(&root).unwrap_err();
    assert!(err.rules().contains(&"scenario-prd-rule-unknown"));
}

#[test]
fn a_prd_scenario_declaring_an_unknown_kind_fails() {
    let root =
        prd_fixture("[scenario.prd]\nrule = \"money_path\"\nprovisions = [\"aurora_cluster\"]");
    let err = verify_fixture(&root).unwrap_err();
    assert!(err.rules().contains(&"scenario-prd-kind-unknown"));
}

#[test]
fn a_prd_scenario_that_creates_nothing_fails_rather_than_being_marked() {
    let root = prd_fixture("[scenario.prd]\nrule = \"money_path\"\nprovisions = []");
    let err = verify_fixture(&root).unwrap_err();
    assert!(err.rules().contains(&"scenario-prd-provisions-nothing"));
}

/// Once anything is marked, the reduced set is one of each singleton rule, not
/// breadth. A registry that opts in and then claims only the money path is
/// missing the other three named paths.
#[test]
fn opting_in_to_prd_without_claiming_every_singleton_rule_fails() {
    let root = prd_fixture(
        "[scenario.prd]\nrule = \"money_path\"\nprovisions = [\n  \"api_key\",\n  \
         \"payment_customer\",\n  \"payment_instrument\",\n  \"auto_recharge_policy\",\n]",
    );
    let err = verify_fixture(&root).unwrap_err();
    let detail = err
        .violations
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        err.rules().contains(&"scenario-prd-rule-cardinality"),
        "{detail}"
    );
    assert!(detail.contains("session_lifecycle"), "{detail}");
    assert!(detail.contains("content_path"), "{detail}");
    assert!(detail.contains("secret_path"), "{detail}");
}

/// A registry that marks nothing is not doing prd provisioning at all, which is
/// the safer state, so the cardinality rules do not fire on it.
#[test]
fn a_registry_that_marks_nothing_prd_eligible_verifies() {
    let root = common::sound_fixture();
    verify_fixture(&root).expect("no prd provisioning is a valid state");
}

/// The shipped registry, not a fixture: the four named journeys are each
/// claimed exactly once, every prd-eligible scenario is reclaimable, and the
/// out-list is out.
#[test]
fn the_shipped_prd_set_is_the_reduced_one() {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../release/scenario-ownership.toml"),
    )
    .expect("the shipped scenario registry");
    let registry: aex_release_tool::graph::inputs::ScenarioOwnership =
        toml::from_str(&text).expect("it parses");
    let policy = aex_workspace_check::policy::Policy::embedded();

    let mut claims: std::collections::BTreeMap<&str, Vec<&str>> = std::collections::BTreeMap::new();
    for scenario in &registry.scenarios {
        if let Some(prd) = &scenario.prd {
            claims
                .entry(prd.rule.as_str())
                .or_default()
                .push(scenario.id.as_str());
            for kind in &prd.provisions {
                let row = policy
                    .janitor
                    .resources
                    .get(kind)
                    .unwrap_or_else(|| panic!("`{kind}` has no [janitor.resource] row"));
                assert!(
                    row.reclaimable_from_tags(),
                    "scenario `{}` provisions unreclaimable `{kind}` in prd",
                    scenario.id
                );
            }
        }
    }
    for rule in [
        "money_path",
        "session_lifecycle",
        "content_path",
        "secret_path",
    ] {
        assert_eq!(
            claims.get(rule).map(Vec::len),
            Some(1),
            "prd rule `{rule}` must be claimed by exactly one scenario, got {:?}",
            claims.get(rule)
        );
    }
    assert_eq!(claims["money_path"], vec!["SC-FINANCE-PAYMENT"]);

    // The out-list: coverage rather than "does production work".
    let out: std::collections::BTreeSet<&str> = registry
        .scenarios
        .iter()
        .filter(|scenario| scenario.prd.is_none())
        .map(|scenario| scenario.id.as_str())
        .collect();
    for excluded in [
        "SC-HANDS-HOSTILE",
        "SC-BRAIN-TURN",
        "SC-SCHEMA-MIGRATE",
        "SC-OBSERVATION-GAP",
        "SC-OBSERVATION-EXPORT",
        "SC-FINANCE-RECONCILE",
        "SC-USAGE-SETTLE",
    ] {
        assert!(
            out.contains(excluded),
            "`{excluded}` is breadth or pressure and must not provision in prd"
        );
    }
}
