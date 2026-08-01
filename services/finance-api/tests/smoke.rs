//! Finance API fail-closed artifact smoke evidence.

use std::process::Command;

#[test]
fn artifact_starts_and_fails_closed_without_its_exact_configuration() {
    let output = Command::new(env!("CARGO_BIN_EXE_finance-api"))
        .env_clear()
        .output()
        .expect("finance API starts");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("AEX_AURORA_CLUSTER_ARN"));
}
