//! Resource-envelope evidence.
//!
//! `graph verify` reads this deployable's shape from `release/units.toml` and
//! rejects a placeholder. This case holds the two halves together: the manifest
//! declares what the package is, and the unit row declares the Fargate CPU,
//! memory and stop timeout the export is bounded by.
//!
//! The shape matters more here than for a Lambda. The whole memory model is a
//! reservation taken up front, so the declared task memory has to cover the
//! configured working budget with room for the runtime, and the stop timeout has
//! to cover an explicit multipart abort plus a checkpoint flush.

/// The row this deployable owns.
fn unit_row() -> &'static str {
    let units = include_str!("../../../release/units.toml");
    units
        .split("[[unit]]")
        .find(|block| block.contains("id = \"observation-export-task\""))
        .expect("observation-export-task is a registered deployable")
}

/// One numeric field of the row.
fn field(row: &str, name: &str) -> u64 {
    row.lines()
        .find_map(|line| line.trim().strip_prefix(&format!("{name} = ")))
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or_else(|| panic!("`{name}` is declared in the unit row"))
}

#[test]
fn the_manifest_declares_its_artifact_and_live_evidence_owner() {
    let manifest = include_str!("../Cargo.toml");
    assert!(manifest.contains("artifact = \"oci_task\""));
    assert!(manifest.contains("deployable = \"observation-export-task\""));
    assert!(manifest.contains("live_suite = \"aex-live-observation-export-task\""));
    assert!(manifest.contains("owner = \"observations-usage\""));
    assert!(
        !manifest.contains("not_applicable"),
        "the suites are written; nothing here is owed to another stream"
    );
}

#[test]
fn every_declared_test_target_is_mapped_to_a_layer() {
    let manifest = include_str!("../Cargo.toml");
    for (target, layer) in [
        ("export", "unit"),
        ("startup", "smoke"),
        ("resource_envelope", "e2e"),
    ] {
        assert!(
            manifest.contains(&format!("{target} = \"{layer}\"")),
            "`{target}` must be mapped to layer `{layer}`"
        );
        assert!(
            manifest.contains(&format!("name = \"{target}\"")),
            "`{target}` must be a declared [[test]] target"
        );
    }
}

#[test]
fn the_unit_row_declares_a_fargate_shape_and_not_a_lambda_one() {
    let row = unit_row();
    assert!(
        row.contains("kind = \"rust-oci-task\""),
        "this deployable is a one-shot task: {row}"
    );
    assert!(
        row.contains("[unit.fargate]"),
        "a task declares a Fargate shape: {row}"
    );
    assert!(
        !row.contains("[unit.lambda]"),
        "a task never declares a Lambda shape: {row}"
    );
    assert!(
        !row.lines()
            .any(|line| line.trim().starts_with("timeout_s =")),
        "a Lambda invocation timeout has no meaning for a one-shot task: {row}"
    );
    assert!(
        !row.lines()
            .any(|line| line.trim().starts_with("reserved_concurrency =")),
        "reserved concurrency is a Lambda dimension: {row}"
    );
}

#[test]
fn the_declared_cpu_memory_and_stop_timeout_are_real_numbers() {
    let row = unit_row();
    let cpu = field(row, "cpu");
    let memory = field(row, "memory_mb");
    let stop = field(row, "stop_timeout_s");
    assert!(cpu >= 512, "a placeholder CPU size would not run: {cpu}");
    assert!(
        memory >= 1024,
        "the reservation model needs real memory: {memory}"
    );
    assert!(
        stop >= 30,
        "the stop timeout covers the multipart abort and the checkpoint flush: {stop}"
    );
    assert_eq!(
        field(row, "desired_count"),
        0,
        "nothing is long-lived; the launcher starts one task per admitted export"
    );
}

#[test]
fn the_declared_memory_covers_the_reservation_model_with_runtime_headroom() {
    let row = unit_row();
    let memory_bytes = field(row, "memory_mb") * 1024 * 1024;
    // The largest plan the configuration bounds admit: a 1000-observation page,
    // a 64 MiB part and a 256 MiB row group.
    let page_buffer = 1_000_u64 * 64 * 1024;
    let encoder_scratch = 16_u64 * 64 * 1024;
    let widest = page_buffer + encoder_scratch + 64 * 1024 * 1024 + 256 * 1024 * 1024;
    assert!(
        memory_bytes > widest,
        "a {memory_bytes}-byte task cannot hold a {widest}-byte reservation plus its runtime"
    );
}

#[test]
fn the_health_port_is_declared_and_the_task_binds_nothing_by_default() {
    let row = unit_row();
    assert!(
        row.contains("port = 0"),
        "a one-shot task exposes no long-lived listener: {row}"
    );
    let source = include_str!("../src/main.rs");
    assert!(
        source.contains("let Some(port) = config.health_port else"),
        "the listener is bound only when a port is configured"
    );
}
