//! The declared deployment envelope, read from the release registry.
//!
//! `graph verify` rejects a placeholder, and this suite refuses one earlier: a
//! deployable that does not state its memory, timeout and reserved concurrency
//! has not decided how it fails under load.

use std::path::Path;

/// The release registry.
fn units() -> toml::Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../release/units.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("`{}` reads: {error}", path.display()));
    toml::from_str(&text).unwrap_or_else(|error| panic!("`{}` parses: {error}", path.display()))
}

/// The row for `id`.
fn unit(id: &str) -> toml::Value {
    units()
        .get("unit")
        .and_then(toml::Value::as_array)
        .unwrap_or_else(|| panic!("release/units.toml declares units"))
        .iter()
        .find(|row| row.get("id").and_then(toml::Value::as_str) == Some(id))
        .unwrap_or_else(|| panic!("release/units.toml declares `{id}`"))
        .clone()
}

#[test]
fn the_deployable_declares_its_artifact_and_live_evidence_owner() {
    let manifest = include_str!("../Cargo.toml");
    assert!(manifest.contains("artifact = \"lambda_zip\""));
    assert!(manifest.contains("deployable = \"finance-api\""));
    assert!(manifest.contains("live_suite = \"aex-live-finance-api\""));
}

#[test]
fn every_finance_lambda_declares_a_real_resource_shape() {
    for id in [
        "finance-api",
        "finance-ingest",
        "finance-settlement-worker",
        "finance-reconcile",
        "usage-receipt-dispatcher",
        "provider-cost-reconciler",
    ] {
        let row = unit(id);
        let shape = row
            .get("lambda")
            .unwrap_or_else(|| panic!("`{id}` declares [unit.lambda]"));
        let memory = shape
            .get("memory_mb")
            .and_then(toml::Value::as_integer)
            .unwrap_or_else(|| panic!("`{id}` declares memory_mb"));
        let timeout = shape
            .get("timeout_s")
            .and_then(toml::Value::as_integer)
            .unwrap_or_else(|| panic!("`{id}` declares timeout_s"));
        let concurrency = shape
            .get("reserved_concurrency")
            .and_then(toml::Value::as_integer)
            .unwrap_or_else(|| panic!("`{id}` declares reserved_concurrency"));
        assert!(
            (128..=10_240).contains(&memory),
            "`{id}` declares memory_mb = {memory}"
        );
        assert!(
            (1..=900).contains(&timeout),
            "`{id}` declares timeout_s = {timeout}"
        );
        assert!(
            concurrency > 0,
            "`{id}` declares reserved_concurrency = {concurrency}; zero throttles it to a stop"
        );
    }
}

#[test]
fn the_one_shot_schema_task_declares_a_task_shape_and_no_lambda_shape() {
    let row = unit("central-schema-admin");
    assert_eq!(
        row.get("kind").and_then(toml::Value::as_str),
        Some("rust-oci-task")
    );
    assert!(
        row.get("lambda").is_none(),
        "a one-shot task declares no Lambda shape"
    );
    let shape = row
        .get("fargate")
        .expect("central-schema-admin declares [unit.fargate]");
    for key in ["cpu", "memory_mb", "stop_timeout_s"] {
        let value = shape
            .get(key)
            .and_then(toml::Value::as_integer)
            .unwrap_or_else(|| panic!("central-schema-admin declares {key}"));
        assert!(value > 0, "central-schema-admin declares {key} = {value}");
    }
    assert_eq!(
        shape.get("desired_count").and_then(toml::Value::as_integer),
        Some(0),
        "a one-shot migration task has no idle instance"
    );
}

#[test]
fn the_finance_ingest_reservation_cannot_be_starved_by_settlement() {
    let ingest = unit("finance-ingest");
    let settlement = unit("finance-settlement-worker");
    let reservation = |row: &toml::Value| {
        row.get("lambda")
            .and_then(|shape| shape.get("reserved_concurrency"))
            .and_then(toml::Value::as_integer)
            .expect("a declared reservation")
    };
    assert!(
        reservation(&ingest) > 0 && reservation(&settlement) > 0,
        "F-29: webhook ingest and settlement each hold their own reservation"
    );
}
