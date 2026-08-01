//! Resource-envelope evidence.
//!
//! `graph verify` reads the deployable's Lambda shape from `release/units.toml`
//! and rejects a placeholder. This case holds the two halves together: the
//! manifest declares what this package is, and the unit row declares the memory,
//! timeout and reserved concurrency one duty deployment is bounded by.

use aex_observation_domain::keys::ControlDomain;
use aex_observation_domain::limits;

/// The registered row of this deployable.
fn unit_row() -> &'static str {
    let units = include_str!("../../../release/units.toml");
    units
        .split("[[unit]]")
        .find(|block| block.contains("id = \"observation-reconciler\""))
        .expect("observation-reconciler is a registered deployable")
}

#[test]
fn the_manifest_declares_its_artifact_and_live_evidence_owner() {
    let manifest = include_str!("../Cargo.toml");
    assert!(manifest.contains("artifact = \"lambda_zip\""));
    assert!(manifest.contains("deployable = \"observation-reconciler\""));
    assert!(manifest.contains("live_suite = \"aex-live-observation-reconciler\""));
    assert!(manifest.contains("owner = \"observations-usage\""));
    assert!(
        !manifest.contains("not_applicable"),
        "the suites exist; nothing here is owed to another stream"
    );
}

#[test]
fn the_unit_row_declares_memory_timeout_and_reserved_concurrency() {
    let row = unit_row();
    assert!(row.contains("[unit.lambda]"), "{row}");
    let memory: u32 = row
        .lines()
        .find_map(|line| line.trim().strip_prefix("memory_mb = "))
        .and_then(|value| value.trim().parse().ok())
        .expect("a memory allocation is declared");
    assert!(memory >= 512, "one bounded due-scan page needs real memory");
    let timeout: u32 = row
        .lines()
        .find_map(|line| line.trim().strip_prefix("timeout_s = "))
        .and_then(|value| value.trim().parse().ok())
        .expect("a timeout is declared");
    assert!(timeout > 0, "a duty without a deadline can wedge a shard");
    let reserved: u32 = row
        .lines()
        .find_map(|line| line.trim().strip_prefix("reserved_concurrency = "))
        .and_then(|value| value.trim().parse().ok())
        .expect("a reserved concurrency is declared");
    assert!(
        reserved > 0,
        "the reservation is what keeps a stuck duty from consuming the account pool"
    );
}

#[test]
fn the_row_is_a_scheduled_regional_lambda_zip() {
    let row = unit_row();
    assert!(row.contains("kind = \"rust-lambda\""), "{row}");
    assert!(row.contains("plane = \"regional\""), "{row}");
    assert!(row.contains("form = \"zip\""), "{row}");
    assert!(
        row.contains("package = \"observation-reconciler\""),
        "{row}"
    );
    assert!(
        !row.contains("[unit.fargate]"),
        "a scheduled duty is not a task shape: {row}"
    );
}

#[test]
fn the_declared_timeout_covers_a_whole_page_of_bounded_attempts() {
    let row = unit_row();
    let timeout_ms: i64 = row
        .lines()
        .find_map(|line| line.trim().strip_prefix("timeout_s = "))
        .and_then(|value| value.trim().parse::<i64>().ok())
        .expect("a timeout is declared")
        * 1_000;
    // A single claim never sleeps: the backoff is written to the item, not
    // waited on. The deadline therefore only has to dominate one gate interval,
    // which is the shortest cadence any duty is scheduled at.
    assert!(
        timeout_ms >= limits::OBS_GATE_INTERVAL_MS,
        "a deployment whose deadline is under one gate interval could never \
         complete a scheduled evaluation"
    );
}

#[test]
fn the_registry_separates_this_deployable_from_the_export_launcher() {
    let units = include_str!("../../../release/units.toml");
    let launcher = units
        .split("[[unit]]")
        .find(|block| block.contains("id = \"observation-export-launcher\""))
        .expect("the launcher is a registered deployable");
    assert!(
        launcher.contains("package = \"observation-export-launcher\""),
        "`{}` is the launcher's own deployable, never a duty of this one",
        ControlDomain::ExportLaunch.as_str()
    );
    assert_ne!(unit_row(), launcher);
}
