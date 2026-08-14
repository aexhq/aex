//! The declared deployment envelope of the webhook ingest lane.

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
    assert!(manifest.contains("deployable = \"finance-ingest\""));
    assert!(manifest.contains("live_suite = \"aex-live-billing-worker\""));
}

#[test]
fn the_ingest_lane_holds_its_own_reservation() {
    let row = unit("finance-ingest");
    let shape = row
        .get("lambda")
        .expect("finance-ingest declares [unit.lambda]");
    let reservation = shape
        .get("reserved_concurrency")
        .and_then(toml::Value::as_integer)
        .expect("a declared reservation");
    assert!(
        reservation > 0,
        "F-29: a settlement backlog must never make Stripe redelivery the customer's problem"
    );
    let timeout = shape
        .get("timeout_s")
        .and_then(toml::Value::as_integer)
        .expect("a declared timeout");
    assert!(
        (1..=900).contains(&timeout),
        "finance-ingest declares timeout_s = {timeout}"
    );
}

#[test]
fn a_direct_invoke_lambda_declares_no_http_health_route() {
    let row = unit("finance-ingest");
    assert!(
        row.get("health_path").is_none() && row.get("ready_path").is_none(),
        "finance-ingest is invoked directly and serves no HTTP; its probes are payload arms"
    );
}
