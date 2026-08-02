//! Schema-admin local-plan conformance evidence.

use std::process::Command;

#[test]
fn local_plan_emits_the_bound_lock_and_linear_head() {
    let output = Command::new(env!("CARGO_BIN_EXE_central-schema-admin"))
        .args([
            "--database-host",
            "db.example",
            "--database-port",
            "5432",
            "--database-name",
            "aex",
            "--database-secret-arn",
            "arn:fixture",
            "--tls-root-ca-path",
            "fixture.pem",
            "--plane",
            "dev",
            "--release",
            "rel_fixture",
            "plan",
        ])
        .output()
        .expect("schema admin starts");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8 receipt");
    assert!(stdout.contains("\"bundleHead\":20260801000700"));
    assert!(stdout.contains("\"lockKey\":4703262552200136530"));
}
