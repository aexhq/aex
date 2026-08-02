//! The artifact starts, and refuses to start without its exact configuration.

use std::process::Command;

#[test]
fn the_artifact_fails_closed_and_names_the_first_missing_variable() {
    let output = Command::new(env!("CARGO_BIN_EXE_finance-reconcile"))
        .env_clear()
        .output()
        .expect("the reconciler artifact starts");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("AEX_FINANCE_RECONCILE_PLANE"),
        "the refusal names the missing variable: {stderr}"
    );
}

#[test]
fn a_replay_window_beyond_the_provider_guarantee_is_refused_at_start_up() {
    let output = Command::new(env!("CARGO_BIN_EXE_finance-reconcile"))
        .env_clear()
        .env("AEX_FINANCE_RECONCILE_PLANE", "dev")
        .env("AEX_FINANCE_RECONCILE_REGION", "eu-west-1")
        .env(
            "AEX_FINANCE_RECONCILE_AURORA_CLUSTER_ARN",
            "arn:aws:rds:eu-west-1:000000000000:cluster:aex-central",
        )
        .env(
            "AEX_FINANCE_RECONCILE_AURORA_SECRET_ARN",
            "arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex/reconcile",
        )
        .env("AEX_FINANCE_RECONCILE_DATABASE_NAME", "aex")
        .env(
            "AEX_FINANCE_RECONCILE_DATABASE_ROLE",
            "aex_finance_reconcile",
        )
        .env(
            "AEX_FINANCE_RECONCILE_STRIPE_COMMAND_EDGE_ARN",
            "arn:aws:lambda:eu-west-1:000000000000:function:aex-dev-stripe-command-edge",
        )
        .env(
            "AEX_FINANCE_RECONCILE_UNKNOWN_EFFECT_RETRY_WINDOW_HOURS",
            "72",
        )
        .env("AEX_FINANCE_RECONCILE_SWEEP_PAGE", "500")
        .env(
            "AEX_FINANCE_RECONCILE_ALARM_TOPIC_ARN",
            "arn:aws:sns:eu-west-1:000000000000:aex-dev-finance-ops",
        )
        .env(
            "AEX_FINANCE_RECONCILE_STATEMENT_BUCKET",
            "aex-dev-statements",
        )
        .env("AEX_FINANCE_RECONCILE_TX_DEADLINE_MS", "30000")
        .output()
        .expect("the reconciler artifact starts");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("AEX_FINANCE_RECONCILE_UNKNOWN_EFFECT_RETRY_WINDOW_HOURS"),
        "the refusal names the rejected variable: {stderr}"
    );
}
