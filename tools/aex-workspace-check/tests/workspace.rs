//! Runs every structural and registry rule against the real workspace.
//!
//! This is the test that proves no production binary can reach test-only code,
//! that Cargo membership and the on-disk tree agree, and that every package in
//! the tree declares what evidence it owes. It shells out to the exact
//! `cargo` that is running it, through `required_env!`, so a missing
//! prerequisite is a failure rather than a reason to skip.

use aex_workspace_check::registry::Phase;
use std::process::Command;

fn metadata_json() -> String {
    let cargo = aex_test_harness::required_env!("CARGO");
    let manifest = aex_test_harness::required_env!("CARGO_MANIFEST_DIR");
    let output = Command::new(cargo)
        .current_dir(manifest)
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .output()
        .expect("`cargo metadata` runs");
    assert!(
        output.status.success(),
        "`cargo metadata` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("`cargo metadata` emits UTF-8")
}

fn report() -> aex_workspace_check::FullReport {
    aex_workspace_check::check_workspace(&metadata_json(), Phase::SourceRewrite)
        .expect("the check runs")
}

#[test]
fn the_real_workspace_satisfies_every_structural_rule() {
    let violations =
        aex_workspace_check::check_metadata_json(&metadata_json()).expect("the check runs");
    assert!(
        violations.is_empty(),
        "{} violation(s):\n{}",
        violations.len(),
        violations
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn the_real_workspace_satisfies_every_registry_rule() {
    let report = report();
    let violations = report.violations();
    assert!(
        violations.is_empty(),
        "{} violation(s):\n{}",
        violations.len(),
        violations
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn every_package_in_the_tree_declares_its_test_ownership() {
    let report = report();
    let undeclared: Vec<&str> = report
        .collected
        .packages
        .iter()
        .filter(|package| package.raw_meta.is_none())
        .map(|package| package.path.as_str())
        .collect();
    assert!(
        undeclared.is_empty(),
        "packages with no aex metadata: {undeclared:?}"
    );
}

#[test]
fn no_production_package_depends_on_a_test_support_crate() {
    let metadata =
        aex_workspace_check::WorkspaceMetadata::parse(&metadata_json()).expect("metadata parses");
    let mut offenders = Vec::new();
    for package in metadata.members() {
        if aex_workspace_check::rules::is_test_support_crate(&package.name) {
            // Test-only code composes with test-only code: the load harness
            // takes `aex-test-harness` as a normal dependency, as every live
            // companion will. What must never happen is a *production* package
            // reaching any of them.
            continue;
        }
        for dependency in &package.dependencies {
            if aex_workspace_check::rules::is_test_support_crate(&dependency.name)
                && dependency.kind() != aex_workspace_check::metadata::DependencyKind::Development
            {
                offenders.push(format!("{} -> {}", package.name, dependency.name));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "test-support crates reachable from production: {offenders:?}"
    );
}

#[test]
fn the_live_target_set_is_derived_from_live_suite_declarations() {
    let report = report();
    let document = aex_workspace_check::registry::build(&report.collected.as_input(
        aex_workspace_check::Policy::embedded(),
        Phase::SourceRewrite,
    ));
    for target in &document.live_targets {
        assert!(
            report
                .collected
                .packages
                .iter()
                .any(|package| &package.name == target),
            "live_suite `{target}` names no package"
        );
    }
    // Companions whose subject package does not exist yet state so with
    // `not_applicable.deployable` and appear in the unearned-evidence ledger.
    // Their union with declared live suites must equal the classified target set.
    let mut accounted: std::collections::BTreeSet<String> = document
        .live_targets
        .iter()
        .map(|suite| {
            suite
                .strip_prefix("aex-live-")
                .unwrap_or_else(|| panic!("live suite `{suite}` lacks the aex-live- prefix"))
                .to_owned()
        })
        .collect();
    accounted.extend(
        report
            .registry
            .unearned
            .iter()
            .filter(|row| {
                row.blocking_rule == "aex-orphan-companion"
                    && row.subject.starts_with("tests/live/")
            })
            .map(|row| {
                row.subject
                    .strip_prefix("tests/live/aex-live-")
                    .unwrap_or_else(|| panic!("invalid companion subject `{}`", row.subject))
                    .to_owned()
            }),
    );
    let expected: std::collections::BTreeSet<String> = aex_workspace_check::inventory::LIVE_TARGETS
        .iter()
        .map(|target| (*target).to_owned())
        .collect();
    assert_eq!(accounted, expected, "live target accounting drifted");
}

#[test]
fn no_source_file_outside_the_harness_names_a_container_image() {
    let report = report();
    assert!(
        report.collected.scan.image_literals.is_empty(),
        "image literals outside aex-test-harness: {:?}",
        report.collected.scan.image_literals
    );
}

#[test]
fn no_ignore_attribute_and_no_bare_environment_read_exists_in_the_tree() {
    let report = report();
    assert!(
        report.collected.scan.ignore_attributes.is_empty(),
        "ignore attributes: {:?}",
        report.collected.scan.ignore_attributes
    );
    assert!(
        report.collected.scan.env_reads.is_empty(),
        "bare environment reads in test targets: {:?}",
        report.collected.scan.env_reads
    );
    assert!(
        report.collected.scan.quarantine_files.is_empty(),
        "quarantine files: {:?}",
        report.collected.scan.quarantine_files
    );
}

#[test]
fn every_unearned_row_names_a_stream_that_can_close_it() {
    let report = report();
    let policy = aex_workspace_check::Policy::embedded();
    assert!(
        !report.registry.unearned.is_empty(),
        "nothing is deployed, so live evidence must be recorded as unearned rather than absent"
    );
    for row in &report.registry.unearned {
        assert!(
            policy.values.owner.contains(&row.owner),
            "unearned row `{}` names owner `{}`, which is not a declared stream",
            row.subject,
            row.owner
        );
        assert!(!row.detail.trim().is_empty(), "{row:?}");
    }
}
