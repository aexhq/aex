//! `central-identity-api` resource-envelope manifest evidence.

#[test]
fn the_manifest_declares_its_artifact_and_live_evidence_owner() {
    let manifest = include_str!("../Cargo.toml");
    assert!(manifest.contains("artifact = \"lambda_zip\""));
    assert!(manifest.contains("deployable = \"central-identity-api\""));
    assert!(manifest.contains("live_suite = \"aex-live-central-identity-api\""));
}

#[test]
fn the_release_registry_declares_this_unit_and_its_lambda_shape() {
    let units = include_str!("../../../release/units.toml");
    let row = units
        .split("[[unit]]")
        .find(|row| row.contains("id = \"central-identity-api\""))
        .expect("`central-identity-api` has a unit row");
    assert!(
        row.contains("[unit.lambda]"),
        "the Lambda shape is declared"
    );
    assert!(row.contains("memory_mb"));
    assert!(row.contains("timeout_s"));
    assert!(row.contains("reserved_concurrency"));
}
