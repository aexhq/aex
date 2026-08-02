//! The declared deployment envelope of the COGS lane.

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
    assert!(manifest.contains("deployable = \"provider-cost-reconciler\""));
    assert!(manifest.contains("live_suite = \"aex-live-provider-cost-reconciler\""));
}

#[test]
fn a_duplicate_schedule_is_a_no_op_rather_than_a_second_scan() {
    let shape = unit("provider-cost-reconciler")
        .get("lambda")
        .cloned()
        .expect("provider-cost-reconciler declares [unit.lambda]");
    assert_eq!(
        shape
            .get("reserved_concurrency")
            .and_then(toml::Value::as_integer),
        Some(1),
        "one task at a time: a duplicate schedule must not start a second scan"
    );
}

#[test]
fn the_scan_has_room_for_one_bounded_export_and_no_more() {
    let shape = unit("provider-cost-reconciler")
        .get("lambda")
        .cloned()
        .expect("a declared shape");
    let timeout = shape
        .get("timeout_s")
        .and_then(toml::Value::as_integer)
        .expect("a declared timeout");
    let memory = shape
        .get("memory_mb")
        .and_then(toml::Value::as_integer)
        .expect("a declared memory allocation");
    assert!(
        (1..=900).contains(&timeout),
        "provider-cost-reconciler declares timeout_s = {timeout}"
    );
    assert!(
        (128..=10_240).contains(&memory),
        "provider-cost-reconciler declares memory_mb = {memory}"
    );
}
