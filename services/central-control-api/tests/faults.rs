//! `central-control-api` fail-closed configuration evidence.

use std::process::Command;

#[test]
fn the_artifact_refuses_to_start_without_its_exact_configuration() {
    let output = Command::new(env!("CARGO_BIN_EXE_central-control-api"))
        .env_clear()
        .output()
        .expect("`central-control-api` starts");
    assert!(!output.status.success(), "an empty environment is refused");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("refusing to start"),
        "the refusal names itself: {stderr}"
    );
}

#[test]
fn the_refusal_names_the_variable_it_wanted() {
    let output = Command::new(env!("CARGO_BIN_EXE_central-control-api"))
        .env_clear()
        .output()
        .expect("`central-control-api` starts");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("AEX_"),
        "the refusal names a variable: {stderr}"
    );
}
