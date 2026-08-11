//! Resource-envelope evidence.
//!
//! `graph verify` reads the deployable's Lambda shape from `release/units.toml`
//! and rejects a placeholder. This case holds the two halves together: the
//! manifest declares what this package is, and the unit row declares the memory,
//! timeout and reserved concurrency the admission edge is bounded by.

#[test]
fn the_manifest_declares_its_artifact_and_live_evidence_owner() {
    let manifest = include_str!("../Cargo.toml");
    assert!(manifest.contains("artifact = \"lambda_zip\""));
    assert!(manifest.contains("deployable = \"regional-otlp\""));
    assert!(manifest.contains("live_suite = \"aex-live-regional-otlp\""));
    assert!(manifest.contains("owner = \"observations-usage\""));
}

#[test]
fn the_unit_row_declares_dedicated_memory_and_reserved_concurrency() {
    let units = include_str!("../../../release/units.toml");
    let row = units
        .split("[[unit]]")
        .find(|block| block.contains("id = \"regional-otlp\""))
        .expect("regional-otlp is a registered deployable");
    assert!(row.contains("[unit.lambda]"), "{row}");
    assert!(
        row.contains("memory_mb = 3008"),
        "the admission edge runs on dedicated memory: {row}"
    );
    assert!(row.contains("timeout_s ="), "{row}");
    let reserved = row
        .lines()
        .find_map(|line| line.trim().strip_prefix("reserved_concurrency = "))
        .and_then(|value| value.trim().parse::<u32>().ok())
        .expect("a reserved concurrency is declared");
    assert!(
        reserved > 0,
        "reserved concurrency bounds the whole-region decode footprint and must be real"
    );
}

#[test]
fn the_declared_shape_bounds_the_regional_decode_footprint() {
    // `reserved concurrency x decoded ceiling` is the number O-ROLES exists to
    // bound. It must stay inside a budget an operator can actually reason about.
    let units = include_str!("../../../release/units.toml");
    let row = units
        .split("[[unit]]")
        .find(|block| block.contains("id = \"regional-otlp\""))
        .expect("regional-otlp is a registered deployable");
    let reserved: u64 = row
        .lines()
        .find_map(|line| line.trim().strip_prefix("reserved_concurrency = "))
        .and_then(|value| value.trim().parse().ok())
        .expect("a reserved concurrency is declared");
    let decoded_max = aex_otlp_admission::OtlpLimits::REGISTERED.decoded_max as u64;
    assert!(
        reserved * decoded_max <= 64 * 1024 * 1024 * 1024,
        "the regional decode ceiling must stay inside 64 GiB"
    );
}

#[test]
fn a_preparing_receipt_outlives_every_possible_admission_invocation() {
    let units = include_str!("../../../release/units.toml");
    let row = units
        .split("[[unit]]")
        .find(|block| block.contains("id = \"regional-otlp\""))
        .expect("regional-otlp is a registered deployable");
    let timeout_ms = row
        .lines()
        .find_map(|line| line.trim().strip_prefix("timeout_s = "))
        .and_then(|value| value.trim().parse::<i64>().ok())
        .expect("the Lambda timeout is declared")
        * 1_000;
    assert!(
        timeout_ms < aex_observation_domain::limits::OBS_PREPARE_TTL_MS,
        "batch expiry must not tell deletion that a preparer is gone while its body puts can still run"
    );
}
