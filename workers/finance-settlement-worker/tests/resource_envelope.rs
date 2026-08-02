//! The declared deployment envelope of the settlement lane.

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

#[test]
fn the_deployable_declares_its_artifact_and_live_evidence_owner() {
    let manifest = include_str!("../Cargo.toml");
    assert!(manifest.contains("artifact = \"lambda_zip\""));
    assert!(manifest.contains("deployable = \"finance-settlement-worker\""));
    assert!(manifest.contains("live_suite = \"aex-live-finance-settlement-worker\""));
}

#[test]
fn the_settlement_lane_bounds_its_concurrent_aurora_writers() {
    let row = unit("finance-settlement-worker");
    let shape = row
        .get("lambda")
        .expect("finance-settlement-worker declares [unit.lambda]");
    let reservation = shape
        .get("reserved_concurrency")
        .and_then(toml::Value::as_integer)
        .expect("a declared reservation");
    assert!(
        reservation > 0,
        "the reservation bounds concurrent writers on one account authority"
    );
    let timeout = shape
        .get("timeout_s")
        .and_then(toml::Value::as_integer)
        .expect("a declared timeout");
    assert!(
        (1..=900).contains(&timeout),
        "finance-settlement-worker declares timeout_s = {timeout}"
    );
}
