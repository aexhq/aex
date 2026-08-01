//! `central-authz` resource-envelope manifest evidence.

#[test]
fn the_manifest_declares_its_artifact_and_live_evidence_owner() {
    let manifest = include_str!("../Cargo.toml");
    assert!(manifest.contains("artifact = \"lambda_zip\""));
    assert!(manifest.contains("deployable = \"central-authz\""));
    assert!(manifest.contains("live_suite = \"aex-live-central-authz\""));
}

#[test]
fn the_release_registry_declares_this_unit_and_its_lambda_shape() {
    let units = include_str!("../../../release/units.toml");
    let row = units
        .split("[[unit]]")
        .find(|row| row.contains("id = \"central-authz\""))
        .expect("`central-authz` has a unit row");
    assert!(
        row.contains("[unit.lambda]"),
        "the Lambda shape is declared"
    );
    assert!(row.contains("memory_mb"));
    assert!(row.contains("timeout_s"));
    assert!(row.contains("reserved_concurrency"));
}

/// This unit declares no health path because the binary serves none.
///
/// It is invoked, not routed. A declared `/internal/healthz` would be a check a
/// deployment runs against a function that can never answer it — the same defect
/// as mounting a route that cannot be served, one layer out.
#[test]
fn the_unit_declares_no_http_probe_it_cannot_answer() {
    let units = include_str!("../../../release/units.toml");
    let row = units
        .split("[[unit]]")
        .find(|row| row.contains("id = \"central-authz\""))
        .expect("`central-authz` has a unit row");
    assert!(!row.contains("health_path"), "{row}");
    assert!(!row.contains("ready_path"), "{row}");
}
