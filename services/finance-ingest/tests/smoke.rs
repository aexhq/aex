//! The artifact starts, and refuses to start without its exact configuration.

use std::process::Command;

fn artifact() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_finance-ingest"));
    command.env_clear();
    command
}

fn with_ingest_environment(command: &mut Command) -> &mut Command {
    command
        .env("AEX_BILLING_WORKER_PLANE", "dev")
        .env("AEX_BILLING_WORKER_REGION", "eu-west-1")
        .env(
            "AEX_BILLING_WORKER_AURORA_CLUSTER_ARN",
            "arn:aws:rds:eu-west-1:000000000000:cluster:aex-central",
        )
        .env(
            "AEX_BILLING_WORKER_AURORA_SECRET_ARN",
            "arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex/billing-worker",
        )
        .env("AEX_BILLING_WORKER_DATABASE_NAME", "aex")
        .env(
            "AEX_BILLING_WORKER_PINNED_STRIPE_API_VERSION",
            "2026-06-24.dahlia",
        )
        .env("AEX_BILLING_WORKER_TX_DEADLINE_MS", "6000")
}

fn with_settlement_environment(command: &mut Command) -> &mut Command {
    with_ingest_environment(command)
        .env(
            "AEX_BILLING_WORKER_RATING_QUEUE_URL",
            "https://sqs.eu-west-1.amazonaws.com/000000000000/aex-dev-rating.fifo",
        )
        .env("AEX_BILLING_WORKER_MAX_GROUP_BATCH", "100")
        .env("AEX_BILLING_WORKER_SERIALIZATION_RETRY_MAX", "3")
}

fn refusal(command: &mut Command) -> String {
    let output = command
        .output()
        .expect("the billing worker artifact starts");
    assert!(!output.status.success());
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn the_artifact_fails_closed_and_names_the_first_missing_variable() {
    let stderr = refusal(&mut artifact());
    assert!(
        stderr.contains("AEX_BILLING_WORKER_PLANE"),
        "the refusal names the missing variable: {stderr}"
    );
}

#[test]
fn the_consolidated_artifact_requires_its_settlement_configuration() {
    let stderr = refusal(with_ingest_environment(&mut artifact()));
    assert!(
        stderr.contains("AEX_BILLING_WORKER_RATING_QUEUE_URL"),
        "the refusal names the first missing settlement variable: {stderr}"
    );
}

#[test]
fn the_consolidated_artifact_requires_its_reconciliation_configuration() {
    let stderr = refusal(with_settlement_environment(&mut artifact()));
    assert!(
        stderr.contains("AEX_BILLING_WORKER_STRIPE_API_SECRET_ARN"),
        "the refusal names the first missing reconciliation variable: {stderr}"
    );
}
