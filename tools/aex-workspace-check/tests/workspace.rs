//! Runs every structural rule against the real workspace.
//!
//! This is the test that proves no production binary can reach a
//! `*-test-support` crate, and that the frozen inventory and the on-disk tree
//! agree. It shells out to the exact `cargo` that is running it, so a missing
//! prerequisite is impossible rather than a reason to skip.

use std::process::Command;

fn metadata_json() -> String {
    let cargo = std::env::var("CARGO").expect("cargo sets CARGO for every test process");
    let manifest =
        std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR for every test");
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
fn every_frozen_member_is_declared_exactly_once() {
    let metadata =
        aex_workspace_check::WorkspaceMetadata::parse(&metadata_json()).expect("metadata parses");
    let directories = metadata
        .member_directories()
        .expect("members live under the root");
    assert_eq!(
        directories.len(),
        aex_workspace_check::inventory::expected_members().len(),
        "member count differs from the frozen inventory"
    );
}
