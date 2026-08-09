//! Start-up evidence: the artifact refuses to run without its exact
//! configuration, and it names the variable it is missing.

use std::process::Command;

/// Every variable the deployable requires, in the order it validates them.
const REQUIRED: &[&str] = &[
    "AEX_PLANE",
    "AEX_REGION",
    "AEX_OBSERVATION_TABLE",
    "AEX_OBSERVATION_BUCKET",
    "AEX_SESSION_TABLE",
    "AEX_OBS_INDEX_SETTLE_MS",
    "AEX_OBS_QUERY_SCANNED_ITEMS",
    "AEX_OBS_QUERY_SEGMENTS",
    "AEX_OBS_QUERY_READ_BYTES",
    "AEX_OBS_METRIC_AGGREGATE_SCAN",
    "AEX_CURSOR_SIGNING_KEY_REF",
    "AEX_CREDENTIAL_PEPPER_REF",
    "AEX_AUTHZ_PROJECTION_TABLE",
];

#[test]
fn an_empty_environment_refuses_to_start_and_names_the_first_missing_variable() {
    let output = Command::new(env!("CARGO_BIN_EXE_regional-observation-api"))
        .env_clear()
        .output()
        .expect("the artifact runs");
    assert!(
        !output.status.success(),
        "an unconfigured query surface must never start"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("AEX_PLANE"), "{stderr}");
    assert!(stderr.contains("refusing to start"), "{stderr}");
}

#[test]
fn a_partial_environment_still_refuses_and_names_the_gap() {
    let output = Command::new(env!("CARGO_BIN_EXE_regional-observation-api"))
        .env_clear()
        .env("AEX_PLANE", "dev")
        .env("AEX_REGION", "eu-west-1")
        .output()
        .expect("the artifact runs");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("AEX_OBSERVATION_TABLE"), "{stderr}");
}

#[test]
fn the_refusal_lists_the_whole_required_configuration() {
    let output = Command::new(env!("CARGO_BIN_EXE_regional-observation-api"))
        .env_clear()
        .output()
        .expect("the artifact runs");
    let stderr = String::from_utf8_lossy(&output.stderr);
    for name in REQUIRED {
        assert!(stderr.contains(name), "`{name}` is not reported: {stderr}");
    }
}
