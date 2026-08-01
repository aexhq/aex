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
         observes = [\"artifact:demo-api\"]\n\n\
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
