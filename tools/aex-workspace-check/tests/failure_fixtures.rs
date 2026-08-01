//! Deliberate-failure fixtures.
//!
//! A checker nobody has seen fail is a checker nobody knows works. Every rule
//! that guards a class of defect gets a fixture that exhibits exactly that
//! defect, and the assertion is the **exact** message, because the message is
//! the interface: it is what a stream reads at 2 a.m. and acts on.
//!
//! The source-scanning fixtures assemble their needles at run time. A literal
//! ignore attribute or bare environment read in this file would be found by the
//! very scan it tests, and the obvious fix - exempting this file - is the
//! quarantine `Q-FLAKE` removes.

use aex_workspace_check::flake::{
    FlakeInput, JUnitReport, NextestList, SourceScan, scan, scan_env_reads, scan_ignores,
};
use aex_workspace_check::policy::Policy;
use aex_workspace_check::registry::{
    Authorities, PackageKind, PackageRow, Phase, RegistryInput, SourceFindings, check,
};
use std::collections::{BTreeMap, BTreeSet};

fn package(path: &str, name: &str, meta: Option<&str>) -> PackageRow {
    PackageRow {
        path: path.to_owned(),
        name: name.to_owned(),
        kind: PackageKind::Cargo,
        raw_meta: meta.map(|text| serde_json::from_str(text).expect("the fixture parses")),
        test_targets: Vec::new(),
        features: Vec::new(),
        normal_dependencies: Vec::new(),
    }
}

fn registry_input(packages: Vec<PackageRow>) -> RegistryInput<'static> {
    RegistryInput {
        packages,
        policy: Policy::embedded(),
        workloads: Vec::new(),
        authorities: Authorities::default(),
        source: SourceFindings::default(),
        collected: None,
        phase: Phase::SourceRewrite,
    }
}

fn messages(violations: &[aex_workspace_check::Violation], rule: &str) -> Vec<String> {
    violations
        .iter()
        .filter(|violation| violation.rule == rule)
        .map(|violation| violation.detail.clone())
        .collect()
}

// --- 1. a package with no declared evidence ---------------------------------

#[test]
fn fixture_a_package_with_no_declared_evidence_fails() {
    let report = check(&registry_input(vec![package(
        "crates/aex-foo",
        "aex-foo",
        None,
    )]));
    assert_eq!(
        messages(&report.violations, "aex-metadata-missing"),
        vec![
            "`crates/aex-foo` has no [package.metadata.aex] table; every member declares its test ownership"
        ]
    );
}

// --- 2. an orphaned live companion ------------------------------------------

#[test]
fn fixture_an_orphaned_live_companion_fails() {
    let companion = r#"{ "owner": "regional-services", "role": "live_companion",
        "artifact": "none", "deployable": "ghost", "layers": ["smoke", "e2e"],
        "concerns": ["fault", "security", "performance"], "seams": [],
        "security_tier": "public_edge", "risk": ["iam"], "scenarios": [],
        "not_applicable": { "targets": "awaiting the regional-services stream" } }"#;
    let report = check(&registry_input(vec![package(
        "tests/live/aex-live-ghost",
        "aex-live-ghost",
        Some(companion),
    )]));
    assert_eq!(
        messages(&report.violations, "aex-orphan-companion"),
        vec![
            "`tests/live/aex-live-ghost` names deployable `ghost`, which is not a workspace member"
        ]
    );
}

// --- 3. an uncovered deployable ---------------------------------------------

#[test]
fn fixture_an_uncovered_deployable_fails() {
    let deployable = r#"{ "owner": "observations-usage", "role": "deployable",
        "artifact": "lambda_zip", "deployable": "usage-compute-worker",
        "layers": ["unit", "smoke", "e2e"],
        "concerns": ["contract", "fault", "security", "performance"], "seams": [],
        "security_tier": "internal", "risk": ["iam"], "scenarios": [],
        "not_applicable": { "targets": "awaiting the observations-usage stream" } }"#;
    let report = check(&registry_input(vec![package(
        "workers/usage-compute-worker",
        "usage-compute-worker",
        Some(deployable),
    )]));
    assert_eq!(
        messages(&report.violations, "aex-uncovered-deployable"),
        vec!["`workers/usage-compute-worker` declares no live_suite and no not-applicable reason"]
    );
}

// --- the lane-input fixtures ------------------------------------------------

const CLEAN_LIST: &str = r#"{
  "rust-suites": {
    "aex-foo::properties": { "kind": "test", "testcases": {
      "balanced": { "ignored": false, "filter-match": { "status": "matches" } } } }
  }
}"#;

const CLEAN_JUNIT: &str = r#"<testsuites><testsuite name="aex-foo::properties">
  <testcase name="balanced" classname="aex-foo::properties" time="0.01"/>
</testsuite></testsuites>"#;

fn flake_input() -> FlakeInput {
    FlakeInput {
        list: NextestList::parse(CLEAN_LIST).expect("the fixture parses"),
        junit: JUnitReport::parse(CLEAN_JUNIT),
        profile: "ci".to_owned(),
        profile_retries: BTreeMap::from([("ci".to_owned(), 0)]),
        env_retries: None,
        filter: None,
        declared_targets: BTreeMap::new(),
        doctest_crates: BTreeSet::new(),
        doctests: None,
        source: SourceScan::default(),
        rerun: None,
    }
}

#[test]
fn the_clean_lane_fixture_produces_no_violation() {
    let report = scan(&flake_input());
    assert!(report.violations.is_empty(), "{:?}", report.violations);
    assert_eq!(report.inventory.declared, 1);
    assert_eq!(report.inventory.collected, 1);
}

// --- 4. a skipped test ------------------------------------------------------

#[test]
fn fixture_a_skipped_test_fails() {
    let junit = CLEAN_JUNIT.replace(
        r#"<testcase name="balanced" classname="aex-foo::properties" time="0.01"/>"#,
        r#"<testcase name="balanced" classname="aex-foo::properties" time="0.01"><skipped/></testcase>"#,
    );
    let mut input = flake_input();
    input.junit = JUnitReport::parse(&junit);
    let report = scan(&input);
    assert_eq!(
        messages(&report.violations, "flake-skipped-test"),
        vec![
            "`aex-foo::properties::balanced` reported <skipped/>; a skipped test is a deleted test that still reports as coverage"
        ]
    );
    assert_eq!(report.inventory.skipped, 1);
}

// --- 5. an ignored test -----------------------------------------------------

#[test]
fn fixture_an_ignored_test_fails_from_the_declared_inventory() {
    let list = CLEAN_LIST.replace(r#""ignored": false"#, r#""ignored": true"#);
    let mut input = flake_input();
    input.list = NextestList::parse(&list).expect("the fixture parses");
    let report = scan(&input);
    assert_eq!(
        messages(&report.violations, "flake-skipped-test"),
        vec![
            "`aex-foo::properties::balanced` has ignored=true; a skipped test is a deleted test that still reports as coverage"
        ]
    );
    assert_eq!(report.inventory.ignored, 1);
}

#[test]
fn fixture_an_ignore_attribute_in_source_fails() {
    let attribute = format!("{}]", concat!("#[", "ignore"));
    let source = format!("#[test]\n{attribute}\nfn slow() {{}}\n");
    let hits = scan_ignores("crates/aex-foo/tests/properties.rs", &source);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].line, 2);

    let mut input = flake_input();
    input.source.ignore_attributes = vec![aex_workspace_check::flake::SourceHit {
        path: "crates/aex-foo/tests/properties.rs".to_owned(),
        line: 88,
        detail: attribute.clone(),
    }];
    let report = scan(&input);
    assert_eq!(
        messages(&report.violations, "flake-ignored-attribute"),
        vec![format!(
            "crates/aex-foo/tests/properties.rs:88 uses {attribute}; move the case to tests/live/ or delete it"
        )]
    );
}

#[test]
fn fixture_a_bare_environment_read_in_a_test_target_fails() {
    let read = format!(
        "    let url = std::{}(\"AEX_PG_URL\").unwrap();",
        concat!("env::", "var")
    );
    let hits = scan_env_reads("crates/aex-foo/tests/integration.rs", &read);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].detail, "AEX_PG_URL");

    let mut input = flake_input();
    input.source.env_reads = vec![aex_workspace_check::flake::SourceHit {
        path: "crates/aex-foo/tests/integration.rs".to_owned(),
        line: 24,
        detail: "AEX_PG_URL".to_owned(),
    }];
    let report = scan(&input);
    assert_eq!(
        messages(&report.violations, "flake-self-skip"),
        vec![
            "crates/aex-foo/tests/integration.rs:24 reads env `AEX_PG_URL` directly; use aex_test_harness::required_env! so an absent prerequisite fails"
        ]
    );
}

// --- 6. a filtered-to-empty selection ---------------------------------------

#[test]
fn fixture_a_filter_that_selects_nothing_fails() {
    let list = r#"{
      "rust-suites": {
        "aex-live-brain-mux::seams": { "kind": "test", "testcases": {
          "reconnect": { "ignored": false, "filter-match": { "status": "mismatch" } },
          "drain": { "ignored": false, "filter-match": { "status": "mismatch" } } } }
      }
    }"#;
    let mut input = flake_input();
    input.list = NextestList::parse(list).expect("the fixture parses");
    input.junit = JUnitReport::default();
    input.filter = Some("binary(smoke)".to_owned());
    let report = scan(&input);
    assert_eq!(
        messages(&report.violations, "flake-empty-selection"),
        vec!["filter `binary(smoke)` selected 0 of 2 cases in `aex-live-brain-mux`"]
    );
    assert_eq!(report.inventory.filtered_at_runtime, 2);
}

// --- 7. a retried test ------------------------------------------------------

#[test]
fn fixture_a_flaky_pass_fails() {
    let junit = CLEAN_JUNIT.replace(
        r#"<testcase name="balanced" classname="aex-foo::properties" time="0.01"/>"#,
        r#"<testcase name="balanced" classname="aex-foo::properties" time="0.01"><flakyFailure message="assertion failed"/></testcase>"#,
    );
    let mut input = flake_input();
    input.junit = JUnitReport::parse(&junit);
    let report = scan(&input);
    assert_eq!(
        messages(&report.violations, "flake-flaky-pass"),
        vec![
            "`aex-foo::properties::balanced` passed on attempt 2; a flaky pass is a failing receipt"
        ]
    );
    assert_eq!(report.inventory.retried, 1);
}

#[test]
fn fixture_a_profile_that_configures_retries_fails() {
    let mut input = flake_input();
    input.profile = "live".to_owned();
    input.profile_retries = aex_workspace_check::flake::profile_retries(
        "[profile.ci]\nretries = 0\n[profile.live]\nretries = 1\n",
    );
    let report = scan(&input);
    assert_eq!(
        messages(&report.violations, "flake-retry-configured"),
        vec!["profile `live` declares retries = 1; blocking lanes never retry to green"]
    );
}

// --- the surrounding rules, same discipline ---------------------------------

#[test]
fn fixture_an_unknown_metadata_key_fails() {
    let meta = r#"{ "owner": "contracts", "role": "contract", "artifact": "none",
        "layers": ["unit"], "concerns": ["contract", "property", "security"], "seams": [],
        "security_tier": "public_edge", "risk": ["untrusted_input"], "scenarios": [],
        "tier": "gold",
        "not_applicable": { "targets": "awaiting the contracts stream",
                            "live_suite": "contract crate; no deployed seam of its own" } }"#;
    let report = check(&registry_input(vec![package(
        "crates/aex-wire",
        "aex-wire",
        Some(meta),
    )]));
    assert_eq!(
        messages(&report.violations, "aex-metadata-unknown-key"),
        vec!["`crates/aex-wire` declares unknown key `aex.tier`; the schema is closed"]
    );
}

#[test]
fn fixture_a_target_that_collects_no_case_is_an_empty_unit() {
    let meta = r#"{ "owner": "test-architecture", "role": "test_support", "artifact": "none",
        "layers": ["unit"], "concerns": ["property"], "seams": [],
        "security_tier": "diagnostic", "risk": ["none"], "scenarios": [],
        "targets": { "properties": "unit" },
        "not_applicable": { "live_suite": "test-only crate" } }"#;
    let mut row = package("tests/support/aex-x", "aex-x", Some(meta));
    row.test_targets = vec!["properties".to_owned()];
    let mut input = registry_input(vec![row]);
    input.collected = Some(BTreeMap::from([("aex-x::properties".to_owned(), 0)]));
    let report = check(&input);
    assert_eq!(
        messages(&report.violations, "aex-empty-unit"),
        vec![
            "`tests/support/aex-x` target `properties` declares layer `unit` but collects 0 tests"
        ]
    );
}

#[test]
fn fixture_awaiting_owner_is_recorded_and_a_bare_omission_fails() {
    let awaiting = r#"{ "owner": "regional-domains", "role": "domain", "artifact": "none",
        "layers": ["unit"], "concerns": ["property"], "seams": [],
        "security_tier": "authority", "risk": ["none"], "scenarios": [],
        "not_applicable": { "targets": "awaiting the regional-domains stream",
                            "live_suite": "pure domain crate; no deployed seam" } }"#;
    let omitted = awaiting.replace(r#""targets": "awaiting the regional-domains stream","#, "");

    let recorded = check(&registry_input(vec![package(
        "crates/aex-session-domain",
        "aex-session-domain",
        Some(awaiting),
    )]));
    assert!(
        messages(&recorded.violations, "aex-empty-unit").is_empty(),
        "{:?}",
        recorded.violations
    );
    assert_eq!(recorded.unearned[0].reason_class, "awaiting_owner");

    let silent = check(&registry_input(vec![package(
        "crates/aex-session-domain",
        "aex-session-domain",
        Some(&omitted),
    )]));
    assert_eq!(
        messages(&silent.violations, "aex-empty-unit"),
        vec![
            "`crates/aex-session-domain` declares no [package.metadata.aex.targets] row and no not_applicable.targets reason; an unwritten suite must name the stream that owes it"
        ]
    );
}
