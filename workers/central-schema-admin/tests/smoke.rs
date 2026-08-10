//! Schema-admin one-shot CLI smoke evidence.

use std::process::Command;

#[test]
fn artifact_exposes_the_exact_one_shot_command_tree() {
    let output = Command::new(env!("CARGO_BIN_EXE_central-schema-admin"))
        .arg("--help")
        .output()
        .expect("schema admin starts");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8 help");
    for command in [
        "plan",
        "migrate",
        "verify",
        "grants",
        "backfill",
        "repair",
        "seed-pepper",
        "seed-signing-key",
    ] {
        assert!(stdout.contains(command), "missing command {command}");
    }
}
