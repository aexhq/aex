//! The artifact starts, and refuses to start without its exact configuration.

use std::process::Command;

#[test]
fn the_artifact_fails_closed_and_names_the_first_missing_variable() {
    let output = Command::new(env!("CARGO_BIN_EXE_finance-api"))
        .env_clear()
        .output()
        .expect("the finance API artifact starts");
    assert!(
        !output.status.success(),
        "an unconfigured money surface must never reach the runtime"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("AEX_FINANCE_API_PLANE"),
        "the refusal names the missing variable: {stderr}"
    );
    assert!(
        stderr.contains("refusing to start"),
        "the refusal is explicit: {stderr}"
    );
}

#[test]
fn a_partially_configured_artifact_still_refuses_and_names_the_next_gap() {
    let output = Command::new(env!("CARGO_BIN_EXE_finance-api"))
        .env_clear()
        .env("AEX_FINANCE_API_PLANE", "dev")
        .env("AEX_FINANCE_API_REGION", "eu-west-1")
        .output()
        .expect("the finance API artifact starts");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("AEX_FINANCE_API_AURORA_CLUSTER_ARN"),
        "the refusal names the next missing variable: {stderr}"
    );
}

#[test]
fn an_invalid_plane_is_refused_rather_than_defaulted() {
    let output = Command::new(env!("CARGO_BIN_EXE_finance-api"))
        .env_clear()
        .env("AEX_FINANCE_API_PLANE", "staging")
        .output()
        .expect("the finance API artifact starts");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("AEX_FINANCE_API_PLANE"),
        "the refusal names the rejected variable: {stderr}"
    );
}
