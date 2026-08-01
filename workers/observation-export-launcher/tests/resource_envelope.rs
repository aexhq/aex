//! Resource-envelope evidence.
//!
//! `graph verify` reads the deployable's Lambda shape from `release/units.toml`
//! and rejects a placeholder. This case holds the two halves together: the
//! manifest declares what this package is, and the unit row declares the memory,
//! timeout and reserved concurrency the launcher is bounded by. The timeout in
//! particular is load-bearing: an invocation that is killed between `RunTask`
//! and the identity reconciliation is exactly the ambiguity the launcher exists
//! to survive, so the declared budget has to cover both calls.

/// The unit row this deployable is registered under.
fn unit_row() -> &'static str {
    let units = include_str!("../../../release/units.toml");
    units
        .split("[[unit]]")
        .find(|block| block.contains("id = \"observation-export-launcher\""))
        .expect("observation-export-launcher is a registered deployable")
}

/// One integer field of the unit's Lambda shape.
fn lambda_field(name: &str) -> u32 {
    let prefix = format!("{name} = ");
    unit_row()
        .lines()
        .find_map(|line| line.trim().strip_prefix(&prefix))
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or_else(|| panic!("`{name}` is declared on the unit row"))
}

#[test]
fn the_manifest_declares_its_artifact_and_live_evidence_owner() {
    let manifest = include_str!("../Cargo.toml");
    assert!(manifest.contains("artifact = \"lambda_zip\""));
    assert!(manifest.contains("deployable = \"observation-export-launcher\""));
    assert!(manifest.contains("live_suite = \"aex-live-observation-export-launcher\""));
    assert!(manifest.contains("owner = \"observations-usage\""));
    assert!(manifest.contains("seams = [\"aws.ecs.run_task\", \"aws.iam.denial\"]"));
}

#[test]
fn the_manifest_declares_a_layer_for_every_test_target() {
    let manifest = include_str!("../Cargo.toml");
    assert!(
        !manifest.contains("not_applicable"),
        "a package that declares its targets may not also excuse them"
    );
    for (target, layer) in [
        ("launcher", "unit"),
        ("startup", "smoke"),
        ("resource_envelope", "e2e"),
    ] {
        assert!(
            manifest.contains(&format!("{target} = \"{layer}\"")),
            "`{target}` is not mapped to layer `{layer}`"
        );
        assert!(
            manifest.contains(&format!("name = \"{target}\"")),
            "`{target}` has no [[test]] block"
        );
    }
}

#[test]
fn the_unit_row_declares_memory_timeout_and_reserved_concurrency() {
    let row = unit_row();
    assert!(row.contains("[unit.lambda]"), "{row}");
    assert!(row.contains("kind = \"rust-lambda\""), "{row}");
    assert!(row.contains("form = \"zip\""), "{row}");
    assert!(lambda_field("memory_mb") >= 256, "{row}");
    assert!(lambda_field("timeout_s") > 0, "{row}");
    assert!(lambda_field("reserved_concurrency") > 0, "{row}");
}

#[test]
fn the_declared_timeout_covers_a_launch_plus_its_identity_reconciliation() {
    // One `RunTask` and, when its outcome is unknown, two `ListTasks` calls.
    // Thirty seconds is the floor below which the reconciliation could not
    // finish, which is the only situation that could ever duplicate a launch.
    assert!(
        lambda_field("timeout_s") >= 30,
        "the timeout must cover RunTask plus the RUNNING and STOPPED reconciliation"
    );
}

#[test]
fn the_launcher_is_deployed_at_low_concurrency_by_design() {
    // Its only work is a bounded due-scan, a conditional claim and one launch.
    // A high reserved concurrency would mean many launchers racing the same
    // control partitions for no throughput gain.
    let reserved = lambda_field("reserved_concurrency");
    assert!(
        (1..=16).contains(&reserved),
        "reserved concurrency {reserved} is outside the shape this role was sized for"
    );
    assert!(
        lambda_field("memory_mb") <= 1024,
        "a launcher decodes nothing"
    );
}
