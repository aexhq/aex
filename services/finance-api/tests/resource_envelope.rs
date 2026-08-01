//! Finance API resource-envelope manifest evidence.

#[test]
fn deployable_manifest_declares_its_artifact_and_live_evidence_owner() {
    let manifest = include_str!("../Cargo.toml");
    assert!(manifest.contains("artifact = \"lambda_zip\""));
    assert!(manifest.contains("deployable = \"finance-api\""));
    assert!(manifest.contains("live_suite = \"aex-live-finance-api\""));
}
