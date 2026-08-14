//! `central-api` resource-envelope manifest evidence.

#[test]
fn the_manifest_declares_its_artifact_and_live_evidence_owner() {
    let manifest = include_str!("../Cargo.toml");
    assert!(manifest.contains("artifact = \"oci_image\""));
    assert!(manifest.contains("deployable = \"control-api\""));
    assert!(manifest.contains("live_suite = \"aex-live-control-api\""));
}

#[test]
fn the_release_registry_declares_this_unit_and_its_fargate_shape() {
    let row = unit_row();
    assert!(
        row.contains("kind = \"rust-oci-service\""),
        "a long-lived service, not a one-shot task"
    );
    assert!(row.contains("[unit.fargate]"), "the task shape is declared");
    for field in [
        "cpu",
        "memory_mb",
        "desired_count",
        "stop_timeout_s",
        "port",
    ] {
        assert!(row.contains(field), "`{field}` is declared");
    }
    assert!(
        !row.contains("[unit.lambda]"),
        "a Fargate unit that also declares a Lambda shape is two deployments \
         claiming one identity"
    );
}

#[test]
fn the_registered_stop_timeout_is_the_one_the_drain_bound_is_derived_from() {
    // The process refuses a drain deadline it could not meet, and it derives
    // that ceiling from `aex_regional_http::drain::FARGATE_STOP_TIMEOUT_S`.
    // Registering a different stop timeout here would make the deadline
    // unreachable in one direction or unbounded in the other, and the failure
    // would show up as requests truncated during a rollout rather than as a
    // refusal at start-up.
    assert!(
        unit_row().contains(&format!(
            "stop_timeout_s = {}",
            aex_regional_http::drain::FARGATE_STOP_TIMEOUT_S
        )),
        "the registered stop timeout must equal the constant the drain bound is \
         derived from"
    );
}

#[test]
fn the_registered_probe_paths_are_the_ones_the_service_actually_serves() {
    let row = unit_row();
    assert!(row.contains(&format!(
        "health_path = \"{}\"",
        aex_central_http::health::HEALTH_PATH
    )));
    assert!(row.contains(&format!(
        "ready_path = \"{}\"",
        aex_central_http::health::READY_PATH
    )));
}

#[test]
fn the_required_receipts_are_the_union_of_the_units_this_one_replaces() {
    // A merge must not quietly retire an obligation. `finance-api` owed
    // `property` and the other two owed `contract`; dropping either would mean
    // the merged deployable ships with less evidence than the sum of its parts.
    let merged = receipts(&unit_row());
    assert!(merged.contains(&"contract".to_owned()));
    assert!(merged.contains(&"property".to_owned()));
}

/// This unit's row in the deployable registry.
fn unit_row() -> String {
    include_str!("../../../release/units.toml")
        .split("[[unit]]")
        .find(|row| row.contains("id = \"control-api\""))
        .expect("`control-api` has a unit row")
        .to_owned()
}

/// The receipt classes one registry row requires.
fn receipts(row: &str) -> Vec<String> {
    let Some((_, rest)) = row.split_once("required_receipts = [") else {
        return Vec::new();
    };
    let (list, _) = rest.split_once(']').expect("a closed receipt list");
    list.split(',')
        .map(|entry| entry.trim().trim_matches('"').to_owned())
        .filter(|entry| !entry.is_empty())
        .collect()
}
