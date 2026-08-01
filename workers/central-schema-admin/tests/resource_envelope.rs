//! Schema-admin resource-envelope manifest evidence.

#[test]
fn deployable_manifest_declares_its_task_and_live_evidence_owner() {
    let manifest = include_str!("../Cargo.toml");
    assert!(manifest.contains("artifact = \"oci_task\""));
    assert!(manifest.contains("deployable = \"central-schema-admin\""));
    assert!(manifest.contains("live_suite = \"aex-live-central-schema-admin\""));
}
