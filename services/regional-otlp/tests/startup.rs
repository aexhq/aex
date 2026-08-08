//! Start-up evidence: the artifact refuses to run without its exact
//! configuration, and it names the variable it is missing.

use std::process::Command;

/// Every variable the deployable requires, in the order it validates them.
const REQUIRED: &[&str] = &[
    "AEX_PLANE",
    "AEX_REGION",
    "AEX_OBSERVATION_TABLE",
    "AEX_OBSERVATION_BUCKET",
    "AEX_SECRET_CUSTODY_TABLE",
    "AEX_OBS_REDACTION_KEY_REF",
    "AEX_OTLP_ENCODED_MAX",
    "AEX_OTLP_DECODED_MAX",
    "AEX_OTLP_MAX_RECORDS",
    "AEX_OTLP_MEMORY_BUDGET_BYTES",
    "AEX_OTLP_RESERVE_WAIT_MS",
    "AEX_OTLP_RESERVED_CONCURRENCY",
    "AEX_AUTHZ_FUNCTION_ARN",
    "AEX_AUTHZ_VERIFY_KEYS_PARAM",
    "AEX_AUTHZ_PROJECTION_TABLE",
    "AEX_ASSERTION_CACHE_BYTES",
];

#[test]
fn an_empty_environment_refuses_to_start_and_names_the_first_missing_variable() {
    let output = Command::new(env!("CARGO_BIN_EXE_regional-otlp"))
        .env_clear()
        .output()
        .expect("the artifact runs");
    assert!(
        !output.status.success(),
        "an unconfigured admission edge must never start"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("AEX_PLANE"), "{stderr}");
    assert!(stderr.contains("refusing to start"), "{stderr}");
}

#[test]
fn a_partial_environment_still_refuses_and_names_the_gap() {
    // Everything but the observation table. A defaulted table identifier is the
    // failure this case exists to make impossible.
    let output = Command::new(env!("CARGO_BIN_EXE_regional-otlp"))
        .env_clear()
        .env("AEX_PLANE", "dev")
        .env("AEX_REGION", "eu-west-1")
        .output()
        .expect("the artifact runs");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("AEX_OBSERVATION_TABLE"), "{stderr}");
}
