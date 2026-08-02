//! The declared deployment envelope of the reconciliation lane.

use std::path::Path;

fn unit(id: &str) -> toml::Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../release/units.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("`{}` reads: {error}", path.display()));
    let document: toml::Value = toml::from_str(&text)
        .unwrap_or_else(|error| panic!("`{}` parses: {error}", path.display()));
    document
        .get("unit")
        .and_then(toml::Value::as_array)
        .expect("release/units.toml declares units")
        .iter()
        .find(|row| row.get("id").and_then(toml::Value::as_str) == Some(id))
        .unwrap_or_else(|| panic!("release/units.toml declares `{id}`"))
        .clone()
}

fn reservation(id: &str) -> i64 {
    unit(id)
        .get("lambda")
        .and_then(|shape| shape.get("reserved_concurrency"))
        .and_then(toml::Value::as_integer)
        .unwrap_or_else(|| panic!("`{id}` declares a reservation"))
}

#[test]
fn the_deployable_declares_its_artifact_and_live_evidence_owner() {
    let manifest = include_str!("../Cargo.toml");
    assert!(manifest.contains("artifact = \"lambda_zip\""));
    assert!(manifest.contains("deployable = \"finance-reconcile\""));
    assert!(manifest.contains("live_suite = \"aex-live-finance-reconcile\""));
}

#[test]
fn the_reconciler_can_never_outscale_the_authority_it_repairs() {
    let sweeps = reservation("finance-reconcile");
    assert!(sweeps > 0, "the sweeps must be able to run at all");
    for busier in ["finance-api", "finance-ingest", "finance-settlement-worker"] {
        assert!(
            sweeps < reservation(busier),
            "reconciliation must never consume the budget `{busier}` needs"
        );
    }
}

#[test]
fn the_sweep_timeout_covers_a_deep_sweep_without_running_forever() {
    let timeout = unit("finance-reconcile")
        .get("lambda")
        .and_then(|shape| shape.get("timeout_s"))
        .and_then(toml::Value::as_integer)
        .expect("a declared timeout");
    assert!(
        (60..=900).contains(&timeout),
        "finance-reconcile declares timeout_s = {timeout}"
    );
}
