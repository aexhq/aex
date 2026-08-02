//! The declared deployment envelope of the receipt replay lane.

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
    assert!(manifest.contains("deployable = \"usage-receipt-dispatcher\""));
    assert!(manifest.contains("live_suite = \"aex-live-usage-receipt-dispatcher\""));
}

#[test]
fn replay_can_never_starve_the_settlement_lane_it_depends_on() {
    let reservation = |id: &str| {
        unit(id)
            .get("lambda")
            .and_then(|shape| shape.get("reserved_concurrency"))
            .and_then(toml::Value::as_integer)
            .unwrap_or_else(|| panic!("`{id}` declares a reservation"))
    };
    assert!(reservation("usage-receipt-dispatcher") > 0);
    assert!(
        reservation("usage-receipt-dispatcher") < reservation("finance-settlement-worker"),
        "replay must never consume the budget settlement needs to produce receipts"
    );
}

#[test]
fn the_dispatcher_scales_to_zero_rather_than_polling_an_idle_outbox() {
    let row = unit("usage-receipt-dispatcher");
    let timeout = row
        .get("lambda")
        .and_then(|shape| shape.get("timeout_s"))
        .and_then(toml::Value::as_integer)
        .expect("a declared timeout");
    assert!(
        (1..=900).contains(&timeout),
        "usage-receipt-dispatcher declares timeout_s = {timeout}"
    );
    assert!(
        row.get("health_path").is_none(),
        "a scheduled worker serves no HTTP; its probe is a payload arm"
    );
}
