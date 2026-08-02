//! The artifact starts, and refuses to start without its exact configuration.

use std::process::Command;

#[test]
fn the_artifact_fails_closed_and_names_the_first_missing_variable() {
    let output = Command::new(env!("CARGO_BIN_EXE_usage-receipt-dispatcher"))
        .env_clear()
        .output()
        .expect("the dispatcher artifact starts");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("AEX_USAGE_RECEIPT_PLANE"),
        "the refusal names the missing variable: {stderr}"
    );
}

#[test]
fn an_unbound_category_queue_is_refused_at_start_up() {
    let output = Command::new(env!("CARGO_BIN_EXE_usage-receipt-dispatcher"))
        .env_clear()
        .env("AEX_USAGE_RECEIPT_PLANE", "dev")
        .env("AEX_USAGE_RECEIPT_REGION", "eu-west-1")
        .env(
            "AEX_USAGE_RECEIPT_AURORA_CLUSTER_ARN",
            "arn:aws:rds:eu-west-1:000000000000:cluster:aex-central",
        )
        .env(
            "AEX_USAGE_RECEIPT_AURORA_SECRET_ARN",
            "arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex/receipt",
        )
        .env("AEX_USAGE_RECEIPT_DATABASE_NAME", "aex")
        .env("AEX_USAGE_RECEIPT_DATABASE_ROLE", "aex_receipt_dispatcher")
        .env(
            "AEX_USAGE_RECEIPT_QUEUE_URL_COMPUTE",
            "https://sqs.eu-west-1.amazonaws.com/000000000000/aex-dev-usage-receipt-compute",
        )
        .output()
        .expect("the dispatcher artifact starts");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("AEX_USAGE_RECEIPT_QUEUE_URL_STORAGE"),
        "every category queue is required: {stderr}"
    );
}
