//! The artifact starts, and refuses to start without its exact configuration.

use std::process::Command;

#[test]
fn the_artifact_fails_closed_and_names_the_first_missing_variable() {
    let output = Command::new(env!("CARGO_BIN_EXE_finance-ingest"))
        .env_clear()
        .output()
        .expect("the finance ingest artifact starts");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("AEX_FINANCE_INGEST_PLANE"),
        "the refusal names the missing variable: {stderr}"
    );
}

#[test]
fn the_wrong_database_role_is_refused_before_a_connection_exists() {
    let output = Command::new(env!("CARGO_BIN_EXE_finance-ingest"))
        .env_clear()
        .env("AEX_FINANCE_INGEST_PLANE", "dev")
        .env("AEX_FINANCE_INGEST_REGION", "eu-west-1")
        .env(
            "AEX_FINANCE_INGEST_AURORA_CLUSTER_ARN",
            "arn:aws:rds:eu-west-1:000000000000:cluster:aex-central",
        )
        .env(
            "AEX_FINANCE_INGEST_AURORA_SECRET_ARN",
            "arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex/x",
        )
        .env("AEX_FINANCE_INGEST_DATABASE_NAME", "aex")
        .env("AEX_FINANCE_INGEST_DATABASE_ROLE", "aex_finance_api")
        .env(
            "AEX_FINANCE_INGEST_PINNED_STRIPE_API_VERSION",
            "2026-06-24.dahlia",
        )
        .env("AEX_FINANCE_INGEST_TX_DEADLINE_MS", "6000")
        .output()
        .expect("the finance ingest artifact starts");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("AEX_FINANCE_INGEST_DATABASE_ROLE"),
        "the refusal names the rejected variable: {stderr}"
    );
}
