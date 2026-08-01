//! Resource-envelope evidence.
//!
//! `graph verify` reads the deployable's Lambda shape from `release/units.toml`
//! and rejects a placeholder. This case holds the two halves together: the
//! manifest declares what this package is, and the unit row declares the memory,
//! timeout and reserved concurrency the query surface is bounded by.

#[test]
fn the_manifest_declares_its_artifact_and_live_evidence_owner() {
    let manifest = include_str!("../Cargo.toml");
    assert!(manifest.contains("artifact = \"lambda_zip\""));
    assert!(manifest.contains("deployable = \"regional-observation-api\""));
    assert!(manifest.contains("live_suite = \"aex-live-regional-observation-api\""));
    assert!(manifest.contains("owner = \"observations-usage\""));
}

#[test]
fn the_unit_row_declares_a_real_lambda_shape() {
    let units = include_str!("../../../release/units.toml");
    let row = units
        .split("[[unit]]")
        .find(|block| block.contains("id = \"regional-observation-api\""))
        .expect("regional-observation-api is a registered deployable");
    assert!(row.contains("[unit.lambda]"), "{row}");
    for field in ["memory_mb =", "timeout_s =", "reserved_concurrency ="] {
        assert!(row.contains(field), "{field} is missing from {row}");
    }
    let memory: u32 = row
        .lines()
        .find_map(|line| line.trim().strip_prefix("memory_mb = "))
        .and_then(|value| value.trim().parse().ok())
        .expect("a memory allocation is declared");
    assert!(
        (128..=10_240).contains(&memory),
        "Lambda accepts 128..=10240, got {memory}"
    );
}

#[test]
fn the_query_role_is_reserved_separately_from_admission() {
    // A query flood must never take decode memory away from OTLP, which is the
    // whole reason the two roles are separate deployables.
    let units = include_str!("../../../release/units.toml");
    let reserved = |unit: &str| -> u32 {
        units
            .split("[[unit]]")
            .find(|block| block.contains(&format!("id = \"{unit}\"")))
            .and_then(|block| {
                block
                    .lines()
                    .find_map(|line| line.trim().strip_prefix("reserved_concurrency = "))
            })
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(0)
    };
    assert!(reserved("regional-observation-api") > 0);
    assert!(reserved("regional-otlp") > 0);
}
