//! The artifact starts, and refuses to start without its exact configuration.

use std::process::Command;

/// A complete environment except for the one variable the case removes.
fn base() -> Vec<(&'static str, &'static str)> {
    vec![
        ("AEX_FINANCE_SETTLEMENT_PLANE", "dev"),
        ("AEX_FINANCE_SETTLEMENT_REGION", "eu-west-1"),
        (
            "AEX_FINANCE_SETTLEMENT_AURORA_CLUSTER_ARN",
            "arn:aws:rds:eu-west-1:000000000000:cluster:aex-central",
        ),
        (
            "AEX_FINANCE_SETTLEMENT_AURORA_SECRET_ARN",
            "arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex/settlement",
        ),
        ("AEX_FINANCE_SETTLEMENT_DATABASE_NAME", "aex"),
        (
            "AEX_FINANCE_SETTLEMENT_DATABASE_ROLE",
            "aex_finance_settlement",
        ),
        (
            "AEX_FINANCE_SETTLEMENT_QUEUE_URL",
            "https://sqs.eu-west-1.amazonaws.com/000000000000/aex-dev-usage-rating.fifo",
        ),
        ("AEX_FINANCE_SETTLEMENT_MAX_GROUP_BATCH", "100"),
        ("AEX_FINANCE_SETTLEMENT_SERIALIZATION_RETRY_MAX", "3"),
        ("AEX_FINANCE_SETTLEMENT_TX_DEADLINE_MS", "20000"),
    ]
}

#[test]
fn the_artifact_fails_closed_and_names_the_first_missing_variable() {
    let output = Command::new(env!("CARGO_BIN_EXE_finance-settlement-worker"))
        .env_clear()
        .output()
        .expect("the settlement worker artifact starts");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("AEX_FINANCE_SETTLEMENT_PLANE"),
        "the refusal names the missing variable: {stderr}"
    );
}

#[test]
fn a_standard_queue_is_refused_before_a_single_message_is_read() {
    let mut command = Command::new(env!("CARGO_BIN_EXE_finance-settlement-worker"));
    command.env_clear();
    for (name, value) in base() {
        if name == "AEX_FINANCE_SETTLEMENT_QUEUE_URL" {
            command.env(
                name,
                "https://sqs.eu-west-1.amazonaws.com/000000000000/aex-dev-usage-rating",
            );
        } else {
            command.env(name, value);
        }
    }
    let output = command.output().expect("the settlement worker starts");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("AEX_FINANCE_SETTLEMENT_QUEUE_URL"),
        "the refusal names the rejected variable: {stderr}"
    );
}
